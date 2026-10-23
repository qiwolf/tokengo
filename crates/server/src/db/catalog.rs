//! 渠道的「上游目录」：上游自己报回来的模型清单，落库记住。
//!
//! 为什么单独一张表：这份清单是**上游说的**，不是我们配的 ——
//! `channel_models` 是我们认的（别名/能看图/丢字段），`channel_catalog` 是上游报的。
//! 两者分开存，才能回答「上游报了这个名字、但我还没勾」「我勾了、上游这次没报」。
//!
//! 纪律：**上游没说的字段一律存 NULL**，读出来也不猜（界面据此决定显示哪些小标签）。
//! 拉失败时不写：一次连不上不能把上次真拿到手的清单删掉。

use rusqlite::{params, Connection, OptionalExtension};

use tokengo_shared::ChannelId;

use super::Result;
use crate::upstream::UpstreamModel;

/// 库里记住的一条上游模型。
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogEntry {
    pub reasoning: crate::reasoning_capabilities::ReasoningCapabilities,
    pub model: String,
    pub kind: Option<String>,
    pub owned_by: Option<String>,
    pub context_window: Option<i64>,
    pub price_note: Option<String>,
    /// 上游说这档价格是 0 / free。`None` = 上游没说价格。
    pub is_free: Option<bool>,
    /// Explicit upstream image-input claim; true can enable matching channel mappings.
    pub vision: Option<bool>,
}

impl From<&UpstreamModel> for CatalogEntry {
    fn from(m: &UpstreamModel) -> Self {
        Self {
            reasoning: m.reasoning.clone(),
            model: m.id.clone(),
            kind: m.kind.clone(),
            owned_by: m.owned_by.clone(),
            context_window: m.context_window,
            price_note: m.price_note.clone(),
            is_free: m.is_free.then_some(true),
            vision: m.vision,
        }
    }
}

/// 这份目录跟上次比，多了哪些、少了哪些名字（按归一后的名字比，报的是原样的字符串）。
///
/// 上游改名/下线是常事，一次问答看不出漂移 —— 把差异报出来，面板上就能一眼看到。
pub fn diff(old: &[CatalogEntry], new: &[UpstreamModel]) -> (Vec<String>, Vec<String>) {
    let new_names: Vec<&str> = new.iter().map(|m| m.id.as_str()).collect();
    let added = new_names
        .iter()
        .filter(|n| {
            !old.iter()
                .any(|o| tokengo_shared::same_model_name(&o.model, n))
        })
        .map(|n| n.to_string())
        .collect();
    let removed = old
        .iter()
        .filter(|o| {
            !new_names
                .iter()
                .any(|n| tokengo_shared::same_model_name(&o.model, n))
        })
        .map(|o| o.model.clone())
        .collect();
    (added, removed)
}

/// 把这条渠道的目录整条换掉（上游这次报的就是全部），并记下这次问的时间（毫秒）。
pub fn replace(
    conn: &Connection,
    id: ChannelId,
    catalog: &[UpstreamModel],
    now_ms: i64,
) -> Result<usize> {
    conn.execute("DELETE FROM channel_catalog WHERE channel_id = ?1", [id.0])?;
    let mut n = 0usize;
    for m in catalog {
        let e = CatalogEntry::from(m);
        conn.execute(
            "INSERT INTO channel_catalog
               (channel_id, model_id, owned_by, context_window, price_note, is_free, vision_claim, kind_claim)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id.0,
                e.model,
                e.owned_by,
                e.context_window,
                e.price_note,
                e.is_free.map(|b| b as i64),
                e.vision.map(|b| b as i64),
                e.kind
            ],
        )?;
        n += 1;
    }
    conn.execute(
        "UPDATE channels SET catalog_at = ?2 WHERE id = ?1",
        params![id.0, now_ms],
    )?;
    adopt_vision(conn, id)?;
    crate::reasoning_capabilities::store_catalog(conn, id.0, catalog, now_ms)?;
    Ok(n)
}

/// Only positive, explicit claims can promote mappings; unknown/false never revoke them.
pub(super) fn adopt_vision(conn: &Connection, id: ChannelId) -> Result<()> {
    let entries = list(conn, id)?;
    let Some(channel) = super::channels::get(conn, id)? else {
        return Ok(());
    };
    for mapping in channel.models.iter().filter(|m| !m.vision) {
        let upstream = mapping.upstream.as_deref()
            .filter(|u| !u.trim().is_empty()).unwrap_or(&mapping.model);
        if entries.iter().any(|e| e.vision == Some(true)
            && tokengo_shared::same_model_name(&e.model, upstream)) {
            super::channels::set_model_vision(conn, id, &mapping.model, true)?;
        }
    }
    Ok(())
}

/// 读回上次记住的目录（按上游报的顺序排，同一次问答里的顺序别乱）。
pub fn list(conn: &Connection, id: ChannelId) -> Result<Vec<CatalogEntry>> {
    let mut stmt = conn.prepare(
        "SELECT model_id, owned_by, context_window, price_note, is_free, vision_claim, kind_claim
           FROM channel_catalog WHERE channel_id = ?1 ORDER BY rowid",
    )?;
    let rows = stmt.query_map([id.0], |r| {
        Ok(CatalogEntry {
            reasoning: Default::default(),
            model: r.get(0)?,
            kind: r.get(6)?,
            owned_by: r.get(1)?,
            context_window: r.get(2)?,
            price_note: r.get(3)?,
            is_free: r.get::<_, Option<i64>>(4)?.map(|v| v != 0),
            vision: r.get::<_, Option<i64>>(5)?.map(|v| v != 0),
        })
    })?;
    let mut entries=rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let identity=crate::reasoning_capabilities::fingerprint(conn,id.0)?;
    for entry in &mut entries {
        entry.reasoning=crate::reasoning_capabilities::for_model_with_identity(conn,id.0,&entry.model,&identity)?;
    }
    Ok(entries)
}

/// Routing needs only matching context/kind claims, not panel reasoning metadata.
/// Keep row order and the shared Rust name normalization used by mapped_context.
pub(crate) fn routing_entries(conn: &Connection, id: ChannelId, names: &[&str]) -> Result<Vec<CatalogEntry>> {
    if names.is_empty() { return Ok(Vec::new()); }
    let names = names.iter().map(|name| tokengo_shared::normalize_model_name(name))
        .collect::<std::collections::HashSet<_>>();
    let mut statement = conn.prepare("SELECT model_id,context_window,kind_claim FROM channel_catalog WHERE channel_id=?1 ORDER BY rowid")?;
    let mut rows = statement.query([id.0])?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next()? {
        let model: String = row.get(0)?;
        if !names.contains(&tokengo_shared::normalize_model_name(&model)) { continue; }
        entries.push(CatalogEntry {
            model, context_window:row.get(1)?, kind:row.get(2)?,
            reasoning:Default::default(), owned_by:None, price_note:None,
            is_free:None, vision:None,
        });
    }
    Ok(entries)
}

/// 上次问上游是什么时候（毫秒；0 = 从来没问过）。
pub fn fetched_at(conn: &Connection, id: ChannelId) -> Result<i64> {
    let at: Option<i64> = conn
        .query_row(
            "SELECT catalog_at FROM channels WHERE id = ?1",
            [id.0],
            |r| r.get(0),
        )
        .optional()?;
    Ok(at.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{channels, open_in_memory};
    use crate::upstream::UpstreamModel;

    fn up(id: &str, owned: Option<&str>, ctx: Option<i64>) -> UpstreamModel {
        UpstreamModel {
            id: id.to_string(),
            kind: None,
            owned_by: owned.map(|s| s.to_string()),
            context_window: ctx,
            price_note: None,
            is_free: false,
            vision: None,
            reasoning: Default::default(),
        }
    }

    fn a_channel(conn: &Connection) -> ChannelId {
        channels::create(
            conn,
            &channels::NewChannel {
                name: "上游A".into(),
                max_concurrent: 0,
                base_url: "http://127.0.0.1/v1".into(),
                enabled: true,
                unwrap_envelope: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn vision_claims_are_adopted_on_refresh_and_save() {
        let conn = open_in_memory().unwrap();
        let id = a_channel(&conn);
        let other = a_channel(&conn);
        let models = vec![
            channels::ModelMapping::new("alias", Some("real-model".into()), false),
            channels::ModelMapping::new("real-model", None, false),
            channels::ModelMapping::new("wrong-target", Some("text-only".into()), false),
            channels::ModelMapping::new("manual", None, true),
            channels::ModelMapping::new("vision-vl-name-only", None, false),
        ];
        channels::set_models(&conn, id, &models).unwrap();
        channels::set_models(&conn, other, &models).unwrap();
        let entries = crate::upstream::parse_model_catalog(r#"{"data":[
            {"id":"real-model","architecture":{"input_modalities":["text","image"]}},
            {"id":"wrong-target","vision":true},
            {"id":"text-only","vision":false},
            {"id":"manual","vision":false},
            {"id":"vision-vl-name-only","output_modalities":["image"]}
        ]}"#);
        replace(&conn, id, &entries, 123).unwrap();
        let flags = |id| channels::get(&conn, id).unwrap().unwrap().models
            .into_iter().map(|m| (m.model, m.vision)).collect::<std::collections::HashMap<_, _>>();
        let got = flags(id);
        assert!(got["alias"], "refresh must adopt the mapped upstream's image-input claim");
        assert!(got["real-model"]);
        assert!(!got["wrong-target"], "match the upstream mapping, not the public alias");
        assert!(got["manual"], "negative claims must not clear existing true");
        assert!(!got["vision-vl-name-only"], "names and image output are not image input");
        assert!(!flags(other)["alias"], "catalog claims are channel-local");

        channels::set_models(&conn, id, &[
            channels::ModelMapping::new("new-alias", Some("real-model".into()), false),
        ]).unwrap();
        assert!(flags(id)["new-alias"], "save must use the cached upstream catalog");
        replace(&conn, id, &[up("real-model", None, None)], 124).unwrap();
        assert!(flags(id)["new-alias"], "unknown claims must not clear existing true");
    }

    #[test]
    fn explicit_type_claims_survive_catalog_round_trip() {
        let conn = open_in_memory().unwrap(); let id = a_channel(&conn);
        let mut entry = up("real",None,None); entry.kind = Some("audio".into());
        replace(&conn,id,&[entry],123).unwrap();
        assert_eq!(list(&conn,id).unwrap()[0].kind.as_deref(),Some("audio"));
        crate::db::migrate(&conn).unwrap();
        assert_eq!(list(&conn,id).unwrap()[0].kind.as_deref(),Some("audio"));
    }

    #[test]
    fn what_the_upstream_did_not_say_is_stored_as_null() {
        let conn = open_in_memory().unwrap();
        let id = a_channel(&conn);
        replace(
            &conn,
            id,
            &[up("deepseek-v4.1-flash", None, None)],
            1_700_000_000_000,
        )
        .unwrap();
        let got = list(&conn, id).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model, "deepseek-v4.1-flash");
        assert_eq!(got[0].owned_by, None);
        assert_eq!(got[0].context_window, None);
        assert_eq!(got[0].is_free, None, "上游没说价格就不能当免费");
        assert_eq!(got[0].vision, None, "上游没报能不能收图就不能猜");
        assert_eq!(fetched_at(&conn, id).unwrap(), 1_700_000_000_000);
    }

    #[test]
    fn a_second_pull_replaces_the_whole_thing() {
        let conn = open_in_memory().unwrap();
        let id = a_channel(&conn);
        replace(&conn, id, &[up("a", None, None), up("b", None, None)], 1).unwrap();
        replace(&conn, id, &[up("b", Some("b-vendor"), Some(8192))], 2).unwrap();
        let got = list(&conn, id).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].owned_by.as_deref(), Some("b-vendor"));
        assert_eq!(fetched_at(&conn, id).unwrap(), 2);
    }

    #[test]
    fn the_diff_reports_renames_by_name_not_by_position() {
        let old = vec![
            CatalogEntry::from(&up("alpha", None, None)),
            CatalogEntry::from(&up("Beta-2", None, None)),
        ];
        let (added, removed) = diff(&old, &[up("beta_2", None, None), up("gamma", None, None)]);
        assert_eq!(added, vec!["gamma".to_string()], "写法差异不算新增");
        assert_eq!(removed, vec!["alpha".to_string()]);
    }

    #[test]
    fn deleting_a_channel_takes_its_catalog_with_it() {
        let conn = open_in_memory().unwrap();
        let id = a_channel(&conn);
        replace(&conn, id, &[up("a", None, None)], 7).unwrap();
        channels::delete(&conn, id).unwrap();
        assert!(list(&conn, id).unwrap().is_empty());
    }
}
