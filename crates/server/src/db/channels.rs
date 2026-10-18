//! 渠道的仓储。渠道 = 一条上游通道，全部由用户自定义添加（不做内置 provider 适配器）。

use rusqlite::{params, Connection, OptionalExtension, Row};

use tokengo_shared::ChannelId;

use super::{DbError, Result};
use crate::crypto::SecretBox;

/// 一条渠道的「按渠道模型名」映射：对外名 → 上游名 + 能不能看图 + 这个模型要丢的字段。
///
/// 用结构体而不是元组：这些字段一起读写，元组顺序错了不会报错、只会静默错位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMapping {
    /// 客户端打的名字（如 `BAAI/bge-m3` 或保留名 `auto`）。
    pub model: String,
    /// 发给上游的名字。None = 原样透传。
    pub upstream: Option<String>,
    /// 这条渠道上的这个模型能不能吃图片输入（面板上勾「能看图」）。
    pub vision: bool,
    /// 发上游前要丢掉的请求体顶层字段（面板上填「要丢的字段」，逗号分隔）。
    ///
    /// 例：opencode 的 `beam-free` 不认 `reasoning_effort`，不删就必然 400。
    pub drop_fields: Vec<String>,
}

impl ModelMapping {
    pub fn new(model: impl Into<String>, upstream: Option<String>, vision: bool) -> Self {
        Self {
            model: model.into(),
            upstream,
            vision,
            drop_fields: Vec::new(),
        }
    }

    /// 面板上填的是一行文本（`a, b`），库里也存这一行；用的时候拆成表。
    pub fn drop_fields_from_text(text: &str) -> Vec<String> {
        text.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// 反过来：存库用的一行文本。
    pub fn drop_fields_text(&self) -> String {
        self.drop_fields.join(",")
    }
}

/// 库里的一条渠道（不含明文密钥，只带"有没有配密钥"）。
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelRow {
    pub id: ChannelId,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub has_secret: bool,
    /// 非流式响应外面套了一层壳时自动剥掉（面板上勾「响应解包」）。
    pub unwrap_envelope: bool,
    /// 这条渠道「同时最多几发」（面板上填「同时最多几发」）。0 = 不限。
    ///
    /// 上游免费额度多数按凭据限并发，超过就回 429/503；配了它，网关会在门口
    /// 排队或改走别的渠道，而不是硬压上去。
    pub max_concurrent: u32,
    /// 全局顺序：小的先试，相同按 id（面板「渠道」页从上到下就是这个顺序）。
    ///
    /// 模型组那一层拿掉之后（v0.11.0），「同一个模型名下先试哪条渠道」就由它决定 ——
    /// 每条渠道只有一份顺序，不用为每个模型名各排一遍。
    pub priority: i64,
    /// 自定义请求头（上游没有的能力，本项目增项）
    pub headers: Vec<(String, String)>,
    /// 该渠道认的模型名（含"能不能看图"、"要丢的字段"）
    pub models: Vec<ModelMapping>,
}

/// 新建渠道要给的字段。
#[derive(Debug, Clone)]
pub struct NewChannel {
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub unwrap_envelope: bool,
    /// 同时最多几发；0 = 不限。
    pub max_concurrent: u32,
}

const SELECT_CHANNEL: &str = "SELECT l.id, l.name, l.base_url, l.enabled, l.unwrap_envelope,
       EXISTS(SELECT 1 FROM channel_secrets s WHERE s.channel_id = l.id) AS has_secret,
       l.max_concurrent, l.priority
  FROM channels l";

fn row_to_channel(r: &Row<'_>) -> rusqlite::Result<ChannelRow> {
    Ok(ChannelRow {
        id: ChannelId(r.get(0)?),
        name: r.get(1)?,
        base_url: r.get(2)?,
        enabled: r.get::<_, i64>(3)? != 0,
        unwrap_envelope: r.get::<_, i64>(4)? != 0,
        has_secret: r.get::<_, i64>(5)? != 0,
        max_concurrent: r.get::<_, i64>(6)?.max(0) as u32,
        priority: r.get::<_, i64>(7)?,
        headers: Vec::new(),
        models: Vec::new(),
    })
}

fn load_children(conn: &Connection, channel: &mut ChannelRow) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT name, value FROM channel_headers WHERE channel_id = ?1 ORDER BY ordinal, id",
    )?;
    channel.headers = stmt
        .query_map([channel.id.0], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare(
        "SELECT model_name, upstream_model, vision, drop_fields FROM channel_models
          WHERE channel_id = ?1 ORDER BY model_name",
    )?;
    channel.models = stmt
        .query_map([channel.id.0], |r| {
            Ok(ModelMapping {
                model: r.get(0)?,
                upstream: r.get(1)?,
                vision: r.get::<_, i64>(2)? != 0,
                drop_fields: ModelMapping::drop_fields_from_text(&r.get::<_, String>(3)?),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(())
}

/// 全部渠道，**按全局顺序排**（`priority` 小的先，相同按 id）。
///
/// 面板「渠道」页与路由都用这一份顺序：页面上从上到下就是「先试哪条」。
pub fn list(conn: &Connection) -> Result<Vec<ChannelRow>> {
    let mut stmt = conn.prepare(&format!("{SELECT_CHANNEL} ORDER BY l.priority, l.id"))?;
    let mut rows: Vec<ChannelRow> = stmt.query_map([], row_to_channel)?.collect::<rusqlite::Result<_>>()?;
    for l in &mut rows {
        load_children(conn, l)?;
    }
    Ok(rows)
}

pub fn get(conn: &Connection, id: ChannelId) -> Result<Option<ChannelRow>> {
    let mut stmt = conn.prepare(&format!("{SELECT_CHANNEL} WHERE l.id = ?1"))?;
    let mut channel: Option<ChannelRow> = stmt.query_row([id.0], row_to_channel).optional()?;
    if let Some(l) = &mut channel {
        load_children(conn, l)?;
    }
    Ok(channel)
}

pub fn create(conn: &Connection, new: &NewChannel) -> Result<ChannelId> {
    if new.name.trim().is_empty() {
        return Err(DbError::Other("渠道名不能为空".into()));
    }
    if new.base_url.trim().is_empty() {
        return Err(DbError::Other("base_url 不能为空".into()));
    }
    // 新渠道排到最后：面板上从上到下 = 先试，新加的默认不插队。
    let next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(priority), -1) + 1 FROM channels",
        [],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO channels (name, base_url, enabled, unwrap_envelope, max_concurrent, priority)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            new.name.trim(),
            new.base_url.trim(),
            new.enabled as i64,
            new.unwrap_envelope as i64,
            new.max_concurrent as i64,
            next
        ],
    )?;
    Ok(ChannelId(conn.last_insert_rowid()))
}

pub fn update(
    conn: &Connection,
    id: ChannelId,
    name: &str,
    base_url: &str,
    enabled: bool,
    unwrap_envelope: bool,
    max_concurrent: u32,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE channels SET name = ?2, base_url = ?3, enabled = ?4, unwrap_envelope = ?5,
                max_concurrent = ?6, updated_at = datetime('now')
          WHERE id = ?1",
        params![
            id.0,
            name.trim(),
            base_url.trim(),
            enabled as i64,
            unwrap_envelope as i64,
            max_concurrent as i64
        ],
    )?;
    if n == 0 {
        return Err(DbError::Other(format!("渠道 {id} 不存在")));
    }
    Ok(())
}

pub fn delete(conn: &Connection, id: ChannelId) -> Result<()> {
    // 子行（请求头/模型/密钥/模型组引用）靠外键 ON DELETE CASCADE 一起清掉
    let n = conn.execute("DELETE FROM channels WHERE id = ?1", [id.0])?;
    if n == 0 {
        return Err(DbError::Other(format!("渠道 {id} 不存在")));
    }
    Ok(())
}

/// 一键启用/停用这条渠道（面板状态开关点的就是它）。只改一列，不碰密钥/请求头/模型名。
pub fn set_enabled(conn: &Connection, id: ChannelId, enabled: bool) -> Result<()> {
    let n = conn.execute(
        "UPDATE channels SET enabled = ?2, updated_at = datetime('now') WHERE id = ?1",
        params![id.0, enabled as i64],
    )?;
    if n == 0 {
        return Err(DbError::Other(format!("渠道 {id} 不存在")));
    }
    Ok(())
}

/// 把这条渠道在全局顺序里上移/下移一位（面板「渠道」页的上下移按钮）。
///
/// **每次先把顺序压成一条 0..n-1 的密排名再换位。** 不这么做的话，两条渠道的
/// priority 相同（老库刚升级、或手工改过库时很常见）时换位等于什么都没换 ——
/// 用户点了按钮没反应，还看不出为什么。
///
/// 到顶/到底就原样返回（不报错）：按钮点多了不该红一片。
pub fn reorder(conn: &Connection, id: ChannelId, up: bool) -> Result<()> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM channels ORDER BY priority, id")?;
        let v = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        v
    };
    let Some(pos) = ids.iter().position(|x| *x == id.0) else {
        return Err(DbError::Other(format!("渠道 {id} 不存在")));
    };
    let other = if up {
        if pos == 0 {
            return Ok(());
        }
        pos - 1
    } else {
        if pos + 1 >= ids.len() {
            return Ok(());
        }
        pos + 1
    };
    let mut ids = ids;
    ids.swap(pos, other);
    super::in_tx(conn, |c| {
        for (ord, cid) in ids.iter().enumerate() {
            c.execute(
                "UPDATE channels SET priority = ?2 WHERE id = ?1",
                params![cid, ord as i64],
            )?;
        }
        Ok(())
    })
}

/// 存密钥（加密）。明文只在内存里过一下，绝不落库、绝不回显。
pub fn set_secret(conn: &Connection, sb: &SecretBox, id: ChannelId, plaintext: &str) -> Result<()> {
    let (nonce, cipher) = sb
        .seal(plaintext)
        .map_err(|e| DbError::Other(e.to_string()))?;
    conn.execute(
        "INSERT INTO channel_secrets (channel_id, nonce, cipher) VALUES (?1, ?2, ?3)
         ON CONFLICT(channel_id) DO UPDATE SET nonce = excluded.nonce, cipher = excluded.cipher",
        params![id.0, nonce, cipher],
    )?;
    Ok(())
}

/// 取密钥明文。**只给代理转发用**，任何对外接口都不许调它。
pub fn secret(conn: &Connection, sb: &SecretBox, id: ChannelId) -> Result<Option<String>> {
    let row: Option<(Vec<u8>, Vec<u8>)> = conn
        .query_row(
            "SELECT nonce, cipher FROM channel_secrets WHERE channel_id = ?1",
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

pub fn clear_secret(conn: &Connection, id: ChannelId) -> Result<()> {
    conn.execute("DELETE FROM channel_secrets WHERE channel_id = ?1", [id.0])?;
    Ok(())
}

/// 整体替换一条渠道的请求头（面板保存时用）。
pub fn set_headers(conn: &Connection, id: ChannelId, headers: &[(String, String)]) -> Result<()> {
    conn.execute("DELETE FROM channel_headers WHERE channel_id = ?1", [id.0])?;
    for (ord, (name, value)) in headers.iter().enumerate() {
        conn.execute(
            "INSERT INTO channel_headers (channel_id, name, value, ordinal) VALUES (?1, ?2, ?3, ?4)",
            params![id.0, name.trim(), value, ord as i64],
        )?;
    }
    Ok(())
}

/// 整体替换一条渠道的模型名映射。
pub fn set_models(conn: &Connection, id: ChannelId, models: &[ModelMapping]) -> Result<()> {
    conn.execute("DELETE FROM channel_models WHERE channel_id = ?1", [id.0])?;
    for m in models {
        conn.execute(
            "INSERT INTO channel_models (channel_id, model_name, upstream_model, vision, drop_fields)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id.0,
                m.model.trim(),
                m.upstream,
                m.vision as i64,
                m.drop_fields_text()
            ],
        )?;
    }
    conn.execute("DELETE FROM registry_associations WHERE channel_id=?1 AND NOT EXISTS (
        SELECT 1 FROM channel_models m WHERE m.channel_id=registry_associations.channel_id
        AND m.model_name=registry_associations.model_name
        AND COALESCE(NULLIF(TRIM(m.upstream_model),''),m.model_name)=registry_associations.upstream_model)", [id.0])?;
    conn.execute("DELETE FROM channel_context_overrides WHERE channel_id=?1 AND NOT EXISTS (
        SELECT 1 FROM channel_models m WHERE m.channel_id=channel_context_overrides.channel_id
        AND COALESCE(NULLIF(TRIM(m.upstream_model),''),m.model_name)=channel_context_overrides.upstream_model)", [id.0])?;
    super::catalog::adopt_vision(conn, id)?;
    super::policy::sync_order(conn)?;
    Ok(())
}

/// 只改「这个模型名在这条渠道上能不能看图」。
///
/// 面板上的「测视觉」实测通过后直接落到这一行（不必整条渠道重存一遍，
/// 免得把用户正在编辑、还没保存的其它字段一起写坏）。
pub fn set_model_vision(
    conn: &Connection,
    id: ChannelId,
    model_name: &str,
    vision: bool,
) -> Result<usize> {
    let n = conn.execute(
        "UPDATE channel_models SET vision = ?3 WHERE channel_id = ?1 AND model_name = ?2",
        params![id.0, model_name.trim(), vision as i64],
    )?;
    if n == 0 {
        return Err(DbError::Other(format!(
            "渠道 {id} 上没有「{}」这个模型名，没法标视觉能力",
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

    fn new_channel() -> NewChannel {
        NewChannel {
            name: "硅基流动".into(),
            base_url: "https://api.siliconflow.cn/v1".into(),
            enabled: true,
            unwrap_envelope: false,
            max_concurrent: 0,
        }
    }

    #[test]
    fn create_read_update_delete() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_channel()).unwrap();
        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.name, "硅基流动");
        assert!(!got.has_secret);

        update(&conn, id, "硅基流动 v2", "https://api.siliconflow.cn/v2", false, false, 0).unwrap();
        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.name, "硅基流动 v2");
        assert!(!got.enabled);

        delete(&conn, id).unwrap();
        assert!(get(&conn, id).unwrap().is_none());
    }

    #[test]
    fn rejects_empty_fields() {
        let conn = open_in_memory().unwrap();
        let mut bad = new_channel();
        bad.name = "  ".into();
        assert!(create(&conn, &bad).is_err());
        let mut bad = new_channel();
        bad.base_url = "".into();
        assert!(create(&conn, &bad).is_err());
    }

    #[test]
    fn secret_round_trips_and_is_never_plain_in_the_db() {
        let conn = open_in_memory().unwrap();
        let sb = SecretBox::from_hex(KEY).unwrap();
        let id = create(&conn, &new_channel()).unwrap();
        set_secret(&conn, &sb, id, "sk-abcdef123456").unwrap();

        assert!(get(&conn, id).unwrap().unwrap().has_secret);
        assert_eq!(secret(&conn, &sb, id).unwrap().unwrap(), "sk-abcdef123456");

        // 反证：库里任何一个字节都不该出现明文
        let blob: Vec<u8> = conn
            .query_row(
                "SELECT cipher FROM channel_secrets WHERE channel_id = ?1",
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
    fn deleting_a_channel_wipes_its_secret_too() {
        let conn = open_in_memory().unwrap();
        let sb = SecretBox::from_hex(KEY).unwrap();
        let id = create(&conn, &new_channel()).unwrap();
        set_secret(&conn, &sb, id, "sk-x").unwrap();
        delete(&conn, id).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM channel_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "渠道删了密钥还留着");
    }

    #[test]
    fn headers_are_replaced_wholesale_and_keep_order() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_channel()).unwrap();
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
    fn per_channel_model_names_are_stored() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_channel()).unwrap();
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
        let id = create(&conn, &new_channel()).unwrap();
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
        let id = create(&conn, &new_channel()).unwrap();
        conn.execute(
            "INSERT INTO channel_models (channel_id, model_name, upstream_model) VALUES (?1, 'auto', 'deepseek-v4.1-flash')",
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
        let id = create(&conn, &new_channel()).unwrap();
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
        let a = create(&conn, &new_channel()).unwrap();
        let mut b = new_channel();
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

    /// 反证（真踩过）：建渠道是四步写入，中途失败必须整体回滚 ——
    /// 不能留下"名字在、请求头/模型名丢了"的半截渠道。
    #[test]
    fn a_failed_channel_create_rolls_back_completely() {
        let conn = open_in_memory().unwrap();
        let sb = SecretBox::from_hex(KEY).unwrap();

        // 故意给两个同名模型，第二条会撞 UNIQUE(channel_id, model_name)
        let r = crate::db::in_tx(&conn, |c| {
            let id = create(c, &new_channel())?;
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
        assert!(list(&conn).unwrap().is_empty(), "半截渠道留在库里了");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM channel_headers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "请求头没跟着回滚");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM channel_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "密钥没跟着回滚");
    }

    /// 反证：同一批里模型名重复要报错，不能悄悄只存一条。
    #[test]
    fn duplicate_model_names_are_rejected() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, &new_channel()).unwrap();
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
