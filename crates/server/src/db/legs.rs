//! 腿的仓储。腿 = 一条上游通道，全部由用户自定义添加（不做内置 provider 适配器）。

use rusqlite::{params, Connection, OptionalExtension, Row};

use tokengo_shared::LegId;

use super::{DbError, Result};
use crate::crypto::SecretBox;

/// 一条腿的「按腿模型名」映射：对外名 → 上游名 + 能不能看图。
///
/// 用结构体而不是三元组：这三个字段一起读写，元组顺序错了不会报错、只会静默错位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMapping {
    /// 客户端打的名字（通常就是组名，如 `auto`）。
    pub model: String,
    /// 发给上游的名字。None = 原样透传。
    pub upstream: Option<String>,
    /// 这条腿上的这个模型能不能吃图片输入（面板上勾「能看图」）。
    pub vision: bool,
}

impl ModelMapping {
    pub fn new(model: impl Into<String>, upstream: Option<String>, vision: bool) -> Self {
        Self {
            model: model.into(),
            upstream,
            vision,
        }
    }
}

/// 库里的一条腿（不含明文密钥，只带"有没有配密钥"）。
#[derive(Debug, Clone, PartialEq)]
pub struct LegRow {
    pub id: LegId,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub has_secret: bool,
    /// 自定义请求头（上游没有的能力，本项目增项）
    pub headers: Vec<(String, String)>,
    /// 该腿认的模型名（含"能不能看图"）
    pub models: Vec<ModelMapping>,
}

/// 新建腿要给的字段。
#[derive(Debug, Clone)]
pub struct NewLeg {
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
}

const SELECT_LEG: &str = "SELECT l.id, l.name, l.base_url, l.enabled,
       EXISTS(SELECT 1 FROM leg_secrets s WHERE s.leg_id = l.id) AS has_secret
  FROM legs l";

fn row_to_leg(r: &Row<'_>) -> rusqlite::Result<LegRow> {
    Ok(LegRow {
        id: LegId(r.get(0)?),
        name: r.get(1)?,
        base_url: r.get(2)?,
        enabled: r.get::<_, i64>(3)? != 0,
        has_secret: r.get::<_, i64>(4)? != 0,
        headers: Vec::new(),
        models: Vec::new(),
    })
}

fn load_children(conn: &Connection, leg: &mut LegRow) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT name, value FROM leg_headers WHERE leg_id = ?1 ORDER BY ordinal, id",
    )?;
    leg.headers = stmt
        .query_map([leg.id.0], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare(
        "SELECT model_name, upstream_model, vision FROM leg_models WHERE leg_id = ?1 ORDER BY model_name",
    )?;
    leg.models = stmt
        .query_map([leg.id.0], |r| {
            Ok(ModelMapping {
                model: r.get(0)?,
                upstream: r.get(1)?,
                vision: r.get::<_, i64>(2)? != 0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(())
}

pub fn list(conn: &Connection) -> Result<Vec<LegRow>> {
    let mut stmt = conn.prepare(&format!("{SELECT_LEG} ORDER BY l.id"))?;
    let mut rows: Vec<LegRow> = stmt.query_map([], row_to_leg)?.collect::<rusqlite::Result<_>>()?;
    for l in &mut rows {
        load_children(conn, l)?;
    }
    Ok(rows)
}

pub fn get(conn: &Connection, id: LegId) -> Result<Option<LegRow>> {
    let mut stmt = conn.prepare(&format!("{SELECT_LEG} WHERE l.id = ?1"))?;
    let mut leg: Option<LegRow> = stmt.query_row([id.0], row_to_leg).optional()?;
    if let Some(l) = &mut leg {
        load_children(conn, l)?;
    }
    Ok(leg)
}

pub fn create(conn: &Connection, new: &NewLeg) -> Result<LegId> {
    if new.name.trim().is_empty() {
        return Err(DbError::Other("腿名不能为空".into()));
    }
    if new.base_url.trim().is_empty() {
        return Err(DbError::Other("base_url 不能为空".into()));
    }
    conn.execute(
        "INSERT INTO legs (name, base_url, enabled) VALUES (?1, ?2, ?3)",
        params![new.name.trim(), new.base_url.trim(), new.enabled as i64],
    )?;
    Ok(LegId(conn.last_insert_rowid()))
}

pub fn update(
    conn: &Connection,
    id: LegId,
    name: &str,
    base_url: &str,
    enabled: bool,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE legs SET name = ?2, base_url = ?3, enabled = ?4, updated_at = datetime('now')
          WHERE id = ?1",
        params![id.0, name.trim(), base_url.trim(), enabled as i64],
    )?;
    if n == 0 {
        return Err(DbError::Other(format!("腿 {id} 不存在")));
    }
    Ok(())
}

pub fn delete(conn: &Connection, id: LegId) -> Result<()> {
    // 子行（请求头/模型/密钥/模型组引用）靠外键 ON DELETE CASCADE 一起清掉
    let n = conn.execute("DELETE FROM legs WHERE id = ?1", [id.0])?;
    if n == 0 {
        return Err(DbError::Other(format!("腿 {id} 不存在")));
    }
    Ok(())
}

/// 存密钥（加密）。明文只在内存里过一下，绝不落库、绝不回显。
pub fn set_secret(conn: &Connection, sb: &SecretBox, id: LegId, plaintext: &str) -> Result<()> {
    let (nonce, cipher) = sb
        .seal(plaintext)
        .map_err(|e| DbError::Other(e.to_string()))?;
    conn.execute(
        "INSERT INTO leg_secrets (leg_id, nonce, cipher) VALUES (?1, ?2, ?3)
         ON CONFLICT(leg_id) DO UPDATE SET nonce = excluded.nonce, cipher = excluded.cipher",
        params![id.0, nonce, cipher],
    )?;
    Ok(())
}

/// 取密钥明文。**只给代理转发用**，任何对外接口都不许调它。
pub fn secret(conn: &Connection, sb: &SecretBox, id: LegId) -> Result<Option<String>> {
    let row: Option<(Vec<u8>, Vec<u8>)> = conn
        .query_row(
            "SELECT nonce, cipher FROM leg_secrets WHERE leg_id = ?1",
            [id.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((nonce, cipher)) => sb
            .open(&nonce, &cipher)
            .map(Some)
            .map_err(|e| DbError::Other(e.to_string())),
    }
}

pub fn clear_secret(conn: &Connection, id: LegId) -> Result<()> {
    conn.execute("DELETE FROM leg_secrets WHERE leg_id = ?1", [id.0])?;
    Ok(())
}

/// 整体替换一条腿的请求头（面板保存时用）。
pub fn set_headers(conn: &Connection, id: LegId, headers: &[(String, String)]) -> Result<()> {
    conn.execute("DELETE FROM leg_headers WHERE leg_id = ?1", [id.0])?;
    for (ord, (name, value)) in headers.iter().enumerate() {
        conn.execute(
            "INSERT INTO leg_headers (leg_id, name, value, ordinal) VALUES (?1, ?2, ?3, ?4)",
            params![id.0, name.trim(), value, ord as i64],
        )?;
    }
    Ok(())
}

/// 整体替换一条腿的模型名映射。
pub fn set_models(conn: &Connection, id: LegId, models: &[ModelMapping]) -> Result<()> {
    conn.execute("DELETE FROM leg_models WHERE leg_id = ?1", [id.0])?;
    for m in models {
        conn.execute(
            "INSERT INTO leg_models (leg_id, model_name, upstream_model, vision) VALUES (?1, ?2, ?3, ?4)",
            params![id.0, m.model.trim(), m.upstream, m.vision as i64],
        )?;
    }
    Ok(())
}

/// 只改「这个模型名在这条腿上能不能看图」。
///
/// 面板上的「测视觉」实测通过后直接落到这一行（不必整条腿重存一遍，
/// 免得把用户正在编辑、还没保存的其它字段一起写坏）。
pub fn set_model_vision(
    conn: &Connection,
    id: LegId,
    model_name: &str,
    vision: bool,
) -> Result<usize> {
    let n = conn.execute(
        "UPDATE leg_models SET vision = ?3 WHERE leg_id = ?1 AND model_name = ?2",
        params![id.0, model_name.trim(), vision as i64],
    )?;
    if n == 0 {
        return Err(DbError::Other(format!(
            "腿 {id} 上没有「{}」这个模型名，没法标视觉能力",
            model_name.trim()
        )));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::crypto::SecretBox;

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn new_leg() -> NewLeg {
        NewLeg {
            name: "硅基流动".into(),
            base_url: "https://api.siliconflow.cn/v1".into(),
            enabled: true,
        }
    }

    #[test]
    fn create_read_update_delete() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_leg()).unwrap();
        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.name, "硅基流动");
        assert!(!got.has_secret);

        update(&conn, id, "硅基流动 v2", "https://api.siliconflow.cn/v2", false).unwrap();
        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.name, "硅基流动 v2");
        assert!(!got.enabled);

        delete(&conn, id).unwrap();
        assert!(get(&conn, id).unwrap().is_none());
    }

    #[test]
    fn rejects_empty_fields() {
        let conn = open_in_memory().unwrap();
        let mut bad = new_leg();
        bad.name = "  ".into();
        assert!(create(&conn, &bad).is_err());
        let mut bad = new_leg();
        bad.base_url = "".into();
        assert!(create(&conn, &bad).is_err());
    }

    #[test]
    fn secret_round_trips_and_is_never_plain_in_the_db() {
        let conn = open_in_memory().unwrap();
        let sb = SecretBox::from_hex(KEY).unwrap();
        let id = create(&conn, &new_leg()).unwrap();
        set_secret(&conn, &sb, id, "sk-abcdef123456").unwrap();

        assert!(get(&conn, id).unwrap().unwrap().has_secret);
        assert_eq!(secret(&conn, &sb, id).unwrap().unwrap(), "sk-abcdef123456");

        // 反证：库里任何一个字节都不该出现明文
        let blob: Vec<u8> = conn
            .query_row(
                "SELECT cipher FROM leg_secrets WHERE leg_id = ?1",
                [id.0],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !blob.windows(3).any(|w| w == b"sk-"),
            "密文里出现了明文片段"
        );
    }

    #[test]
    fn deleting_a_leg_wipes_its_secret_too() {
        let conn = open_in_memory().unwrap();
        let sb = SecretBox::from_hex(KEY).unwrap();
        let id = create(&conn, &new_leg()).unwrap();
        set_secret(&conn, &sb, id, "sk-x").unwrap();
        delete(&conn, id).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM leg_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "腿删了密钥还留着");
    }

    #[test]
    fn headers_are_replaced_wholesale_and_keep_order() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_leg()).unwrap();
        set_headers(
            &conn,
            id,
            &[
                ("x-opencode-client".into(), "cli".into()),
                ("x-opencode-session".into(), "{{uuid}}".into()),
            ],
        )
        .unwrap();
        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.headers.len(), 2);
        assert_eq!(got.headers[0].0, "x-opencode-client");
        assert_eq!(got.headers[1].1, "{{uuid}}");

        set_headers(&conn, id, &[("User-Agent".into(), "tokengo/1.0".into())]).unwrap();
        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.headers, vec![("User-Agent".to_string(), "tokengo/1.0".to_string())]);
    }

    #[test]
    fn per_leg_model_names_are_stored() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_leg()).unwrap();
        set_models(
            &conn,
            id,
            &[
                ModelMapping::new("deepseek-v4.1-flash", Some("deepseek/deepseek-v4.1-flash".into()), false),
                ModelMapping::new("bge-m3", None, false),
            ],
        )
        .unwrap();
        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.models.len(), 2);
        assert!(got
            .models
            .iter()
            .any(|m| m.model == "bge-m3" && m.upstream.is_none()));
    }

    /// 视觉标记要跟着模型名一起落库、一起读回来。
    #[test]
    fn vision_flag_round_trips_per_model_name() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_leg()).unwrap();
        set_models(
            &conn,
            id,
            &[
                ModelMapping::new("auto", Some("glm-4v-flash".into()), true),
                ModelMapping::new("BAAI/bge-m3", None, false),
            ],
        )
        .unwrap();
        let got = get(&conn, id).unwrap().unwrap();
        let auto = got.models.iter().find(|m| m.model == "auto").unwrap();
        let vec = got.models.iter().find(|m| m.model == "BAAI/bge-m3").unwrap();
        assert!(auto.vision, "标了能看图的要读回来");
        assert!(!vec.vision, "没标的就是不能看图");
    }

    /// 反证：**老库升上来的行没有这一列，读出来必须是"不能看图"（false），
    /// 不能是 true**。默认 true 会让纯文本模型被当成能看图，图发过去就报错。
    #[test]
    fn rows_written_without_the_column_default_to_not_vision() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_leg()).unwrap();
        conn.execute(
            "INSERT INTO leg_models (leg_id, model_name, upstream_model) VALUES (?1, 'auto', 'deepseek-v4.1-flash')",
            [id.0],
        )
        .unwrap();
        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.models.len(), 1);
        assert!(!got.models[0].vision, "没这一列的老行必须按「不能看图」算");
    }

    #[test]
    fn set_model_vision_only_touches_that_one_row() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_leg()).unwrap();
        set_models(
            &conn,
            id,
            &[
                ModelMapping::new("auto", Some("a".into()), false),
                ModelMapping::new("deepseek-v4.1-flash", Some("b".into()), false),
            ],
        )
        .unwrap();
        assert_eq!(set_model_vision(&conn, id, "auto", true).unwrap(), 1);
        let got = get(&conn, id).unwrap().unwrap();
        assert!(got.models.iter().find(|m| m.model == "auto").unwrap().vision);
        assert!(!got
            .models
            .iter()
            .find(|m| m.model == "deepseek-v4.1-flash")
            .unwrap()
            .vision);
        // 没有这个模型名时必须报错，不能悄悄成功
        assert!(set_model_vision(&conn, id, "没这个名", true).is_err());
    }

    #[test]
    fn list_returns_everything_with_children() {
        let conn = open_in_memory().unwrap();
        let a = create(&conn, &new_leg()).unwrap();
        let mut b = new_leg();
        b.name = "opencode-go".into();
        let b = create(&conn, &b).unwrap();
        set_headers(&conn, a, &[("X".into(), "1".into())]).unwrap();
        let all = list(&conn).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, a);
        assert_eq!(all[0].headers.len(), 1);
        assert_eq!(all[1].id, b);
        assert!(all[1].headers.is_empty());
    }

    /// 反证（真踩过）：建腿是四步写入，中途失败必须整体回滚 ——
    /// 不能留下"名字在、请求头/模型名丢了"的半截腿。
    #[test]
    fn a_failed_leg_create_rolls_back_completely() {
        let conn = open_in_memory().unwrap();
        let sb = SecretBox::from_hex(KEY).unwrap();

        // 故意给两个同名模型，第二条会撞 UNIQUE(leg_id, model_name)
        let r = crate::db::in_tx(&conn, |c| {
            let id = create(c, &new_leg())?;
            set_headers(c, id, &[("X-Test".into(), "1".into())])?;
            set_secret(c, &sb, id, "sk-x")?;
            set_models(
                c,
                id,
                &[
                    ModelMapping::new("auto", None, false),
                    ModelMapping::new("auto", Some("dup".into()), false),
                ],
            )?;
            Ok(id)
        });
        assert!(r.is_err(), "同名模型必须报错");
        assert!(list(&conn).unwrap().is_empty(), "半截腿留在库里了");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM leg_headers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "请求头没跟着回滚");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM leg_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "密钥没跟着回滚");
    }

    /// 反证：同一批里模型名重复要报错，不能悄悄只存一条。
    #[test]
    fn duplicate_model_names_are_rejected() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_leg()).unwrap();
        let r = set_models(
            &conn,
            id,
            &[
                ModelMapping::new("auto", None, false),
                ModelMapping::new("auto", Some("x".into()), false),
            ],
        );
        assert!(r.is_err());
    }
}
