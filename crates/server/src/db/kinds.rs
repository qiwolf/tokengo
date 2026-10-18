//! 「模型名 → 用途」：这张表记的是**名字**的属性，不是模型组的（v0.10.0）。
//!
//! 为什么单独立一张表：面板「模型」页改成一名字一行的列表（行来自「渠道里勾过的名字」），
//! 而用途原先只挂在模型组上（迁移 v5）—— 没建组的名字就没地方标用途，图像 / 视频 / 音频
//! 这类网关暂时不转发、只能手标的名字永远归不了档。所以用途改成挂在**名字**上。
//!
//! 模型组那一层已经拿掉（v0.11.0），所以**只有这一份**了：路由（该挑哪一类候选）、
//! 对外清单（`db::model_routes` 的视觉判断）、探活（该打哪个端点）读的都是它。

use rusqlite::{params, Connection};

use super::{now_ms, Result};
use tokengo_shared::{KindSource, ModelKind};

/// 一个名字的用途。
#[derive(Debug, Clone, PartialEq)]
pub struct NameKind {
    pub name: String,
    pub kind: ModelKind,
    pub source: KindSource,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<NameKind> {
    Ok(NameKind {
        name: r.get(0)?,
        kind: ModelKind::parse(&r.get::<_, String>(1)?),
        source: KindSource::parse(&r.get::<_, String>(2)?),
    })
}

/// 全部名字的用途（面板一次拿走）。按名字排序，界面上顺序稳定。
pub fn all(conn: &Connection) -> Result<Vec<NameKind>> {
    let mut stmt =
        conn.prepare("SELECT model_name, kind, kind_source FROM model_kinds ORDER BY model_name")?;
    let v = stmt
        .query_map([], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// 一个名字的用途。**没记过就是 None**，绝不编一个默认值出来。
pub fn get(conn: &Connection, name: &str) -> Result<Option<NameKind>> {
    let mut stmt = conn.prepare(
        "SELECT model_name, kind, kind_source FROM model_kinds WHERE model_name = ?1",
    )?;
    let mut rows = stmt.query_map([name.trim()], row)?;
    Ok(rows.next().transpose()?)
}

/// 人在面板上定的类型：写死，之后实测不许覆盖。
///
/// `ModelKind::Unknown` = 「交还给实测」：类型与来源一起清空，下次真有请求再学。
pub fn set(conn: &Connection, name: &str, kind: ModelKind) -> Result<()> {
    let (k, src) = if kind.is_set() {
        (kind.as_str(), KindSource::Manual.as_str())
    } else {
        ("", KindSource::Unknown.as_str())
    };
    conn.execute(
        "INSERT INTO model_kinds (model_name, kind, kind_source, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(model_name) DO UPDATE SET kind = ?2, kind_source = ?3, updated_at = ?4",
        params![name.trim(), k, src, now_ms()],
    )?;
    Ok(())
}

/// 实测学到的类型：**只在还没定过、也没被人定过时**写进去（返回是否真写了）。
///
/// 与手工定过的那份同一口径：已经有结论就一个字都不动 ——
/// 用户拍过的板不被流量改掉。
pub fn learn(conn: &Connection, name: &str, kind: ModelKind) -> Result<bool> {
    if !kind.is_set() {
        return Ok(false);
    }
    let n = conn.execute(
        "INSERT INTO model_kinds (model_name, kind, kind_source, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(model_name) DO UPDATE SET kind = ?2, kind_source = ?3, updated_at = ?4
         WHERE model_kinds.kind = '' AND model_kinds.kind_source = ''",
        params![name.trim(), kind.as_str(), KindSource::Auto.as_str(), now_ms()],
    )?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    /// 手工定过的类型不许被实测覆盖 —— 用户拍过的板不被流量改掉。
    #[test]
    fn a_manual_choice_wins_over_what_traffic_learns() {
        let conn = open_in_memory().unwrap();
        assert!(get(&conn, "auto").unwrap().is_none(), "没记过就是没有，不许编");

        assert!(learn(&conn, "auto", ModelKind::Chat).unwrap());
        let k = get(&conn, "auto").unwrap().unwrap();
        assert_eq!(k.kind, ModelKind::Chat);
        assert_eq!(k.source, KindSource::Auto);
        assert!(!learn(&conn, "auto", ModelKind::Embeddings).unwrap(), "学过一次就不再改");

        set(&conn, "auto", ModelKind::Image).unwrap();
        let k = get(&conn, "auto").unwrap().unwrap();
        assert_eq!(k.kind, ModelKind::Image);
        assert_eq!(k.source, KindSource::Manual);
        assert!(!learn(&conn, "auto", ModelKind::Chat).unwrap(), "人定过之后实测不许动");

        // 「交还给实测」：类型与来源一起清空，之后又能学了
        set(&conn, "auto", ModelKind::Unknown).unwrap();
        let k = get(&conn, "auto").unwrap().unwrap();
        assert_eq!(k.kind, ModelKind::Unknown);
        assert_eq!(k.source, KindSource::Unknown);
        assert!(learn(&conn, "auto", ModelKind::Chat).unwrap());
    }

    /// 迁移 v9 把模型组上已有的类型搬到名字上 —— 现网那几个已定类型的组不许丢。
    #[test]
    fn the_migration_copies_the_kinds_models_already_have() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                 version    INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        // 装成一个还没跑 v9 的库
        for (v, sql) in crate::db::schema::MIGRATIONS
            .iter()
            .take_while(|(v, _)| *v <= 8)
        {
            conn.execute_batch(sql).unwrap();
            conn.execute("INSERT INTO schema_migrations (version) VALUES (?1)", [v])
                .unwrap();
        }
        conn.execute(
            "INSERT INTO model_groups (name, enabled, kind, kind_source)
             VALUES ('BAAI/bge-m3', 1, 'embeddings', 'manual')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO model_groups (name, enabled) VALUES ('auto', 1)", [])
            .unwrap();

        crate::db::migrate(&conn).unwrap();

        let k = get(&conn, "BAAI/bge-m3").unwrap().unwrap();
        assert_eq!(k.kind, ModelKind::Embeddings);
        assert_eq!(k.source, KindSource::Manual);
        assert!(
            get(&conn, "auto").unwrap().is_none(),
            "没定过用途的名字不许凭空编一条出来"
        );
        // 幂等：再跑一遍迁移不改数据
        crate::db::migrate(&conn).unwrap();
        assert_eq!(all(&conn).unwrap().len(), 1);
    }

    /// 图像 / 视频 / 音频这三种网关不转发、只能手标的类型必须能存下来。
    #[test]
    fn the_three_hand_only_kinds_round_trip() {
        let conn = open_in_memory().unwrap();
        for (name, kind) in [
            ("flux-1-schnell", ModelKind::Image),
            ("veo-3", ModelKind::Video),
            ("gpt-4o-mini-tts", ModelKind::Audio),
        ] {
            set(&conn, name, kind).unwrap();
            let got = get(&conn, name).unwrap().unwrap();
            assert_eq!(got.kind, kind, "{name} 存进去又读出来变了");
            assert_eq!(got.source, KindSource::Manual);
        }
        assert_eq!(all(&conn).unwrap().len(), 3);
    }
}
