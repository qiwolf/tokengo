//! Explicit parent grouping only. Connection IDs, credentials and limits remain independent.
use super::{DbError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tokengo_shared::{same_model_name, ChannelId};
pub fn name(raw: &str) -> std::result::Result<String, &'static str> {
    let n = raw.trim();
    if n.is_empty() || n.chars().count() > 128 || n.chars().any(char::is_control) {
        return Err("Provider name must be 1..128 characters without controls");
    }
    Ok(n.into())
}
pub fn exists(c: &Connection, id: i64) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM provider_channels WHERE id=?1)",
        [id],
        |r| r.get(0),
    )?)
}
pub fn membership(c: &Connection, id: i64) -> Result<Option<(i64, String, bool)>> {
    Ok(c.query_row("SELECT p.id,p.name,p.enabled FROM provider_connections x JOIN provider_channels p ON p.id=x.provider_id WHERE x.channel_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get::<_,i64>(2)?!=0))).optional()?)
}
pub fn set_membership(c: &Connection, id: i64, parent: Option<i64>) -> Result<()> {
    match parent {
        Some(p) => {
            c.execute("INSERT INTO provider_connections(channel_id,provider_id) VALUES(?1,?2) ON CONFLICT(channel_id) DO UPDATE SET provider_id=excluded.provider_id",params![id,p])?;
        }
        None => {
            c.execute("DELETE FROM provider_connections WHERE channel_id=?1", [id])?;
        }
    }
    Ok(())
}
pub fn model_switch(c: &Connection, ids: &[i64], model: &str, enabled: bool) -> Result<()> {
    for id in ids {
        let row = super::channels::get(c, ChannelId(*id))?
            .ok_or_else(|| DbError::Other("Connection not found".into()))?;
        let m = row
            .models
            .iter()
            .find(|m| same_model_name(&m.model, model))
            .ok_or_else(|| DbError::Other("Model not declared by connection".into()))?;
        c.execute(
            "UPDATE channel_models SET enabled=?3 WHERE channel_id=?1 AND model_name=?2",
            params![id, m.model, enabled as i64],
        )?;
    }
    Ok(())
}
pub fn source_json(c: &Connection, name: &str) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for ch in super::channels::list(c)? {
        if let Some(m) = ch.models.iter().find(|m| same_model_name(&m.model, name)) {
            rows.push(json!({"connection_id":ch.id.0,"connection_name":ch.name,"provider_id":ch.provider_id,"provider_name":ch.provider_name,"provider_enabled":ch.provider_enabled,"connection_enabled":ch.enabled,"model_enabled":m.enabled,"reasoning":crate::reasoning_capabilities::for_mapping(c,ch.id.0,&m.model)?}));
        }
    }
    Ok(rows)
}
pub fn one(c: &Connection, id: i64) -> Result<Value> {
    let (name, enabled): (String, bool) = c.query_row(
        "SELECT name,enabled FROM provider_channels WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
    )?;
    let chans: Vec<_> = super::channels::list(c)?
        .into_iter()
        .filter(|x| x.provider_id == Some(id))
        .collect();
    let ids: Vec<_> = chans.iter().map(|x| x.id.0).collect();
    let (base_url,adapter_kind,canonical_state,key_strategy,switch_on_429,switch_on_400,config_version):(Option<String>,String,String,String,bool,bool,i64)=c.query_row("SELECT base_url,adapter_kind,canonical_state,key_strategy,switch_on_429,switch_on_400,config_version FROM provider_channels WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?;
    let endpoint_mode:String=c.query_row("SELECT endpoint_mode FROM provider_channels WHERE id=?1",[id],|r|r.get(0))?;
    let mut base_urls:Vec<_>=chans.iter().map(|ch|ch.base_url.clone()).collect();base_urls.sort();base_urls.dedup();
    let routes = super::model_routes::list(c)?;
    let mut models = Vec::new();
    for r in routes {
        if !r.declared_by.iter().any(|id| ids.contains(&id.0)) {
            continue;
        }
        let declared: Vec<_> = r
            .declared_by
            .iter()
            .map(|x| x.0)
            .filter(|id| ids.contains(id))
            .collect();
        let active: Vec<_> = chans
            .iter()
            .filter(|ch| {
                ch.models
                    .iter()
                    .any(|m| same_model_name(&m.model, &r.name) && m.enabled)
            })
            .map(|ch| ch.id.0)
            .collect();
        let mut routable = Vec::new();
        if r.enabled {
            for source in r
                .channels
                .iter()
                .map(|x| x.0)
                .filter(|source| ids.contains(source))
            {
                if super::provider_pool::source_credential(c, source)?
                    .is_none_or(|key| key.enabled && key.provider_id == id)
                {
                    routable.push(source);
                }
            }
        }
        let visible = super::model_routes::visible(c, &r.name)?;
        models.push(json!({
            "name":r.name,"display_name":super::model_access::display_name(c,&r.name)?,"catalog_purpose":super::model_access::catalog_purpose(c,&r.name)?,"catalog_purpose_label":super::model_access::catalog_purpose_label(&super::model_access::catalog_purpose(c,&r.name)?),"kind":r.kind.as_str(),"kind_label":r.kind.label(),"global_enabled":r.enabled,"visible":visible,
            "declared_connection_ids":declared,"enabled_connection_ids":active,"routable_connection_ids":routable
        }));
    }
    Ok(
        json!({"endpoint_mode":endpoint_mode,"base_urls":base_urls,"id":id,"name":name,"enabled":enabled,"connection_ids":ids,"models":models,"base_url":base_url,"adapter_kind":adapter_kind,"canonical_state":canonical_state,"key_strategy":key_strategy,"switch_on_429":switch_on_429,"switch_on_400":switch_on_400,"config_version":config_version}),
    )
}
pub fn list(c: &Connection) -> Result<Value> {
    let mut st = c.prepare("SELECT id FROM provider_channels ORDER BY name COLLATE NOCASE,id")?;
    let ids = st
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let providers = ids
        .into_iter()
        .map(|id| one(c, id))
        .collect::<Result<Vec<_>>>()?;
    let ungrouped: Vec<_> = super::channels::list(c)?
        .into_iter()
        .filter(|x| x.provider_id.is_none())
        .map(|x| x.id.0)
        .collect();
    Ok(json!({"providers":providers,"ungrouped_connection_ids":ungrouped}))
}
