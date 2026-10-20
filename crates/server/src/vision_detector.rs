//! Active image-input verification, isolated from user traffic and routing state.
use std::{hash::{Hash,Hasher},time::Duration};
use crate::{SharedState,db::{self,channels,model_routes,now_ms},gateway::EndpointKind};
use tokengo_shared::ModelKind;

// v32 persisted fingerprints hash the Debug bytes of ChannelRow/ModelMapping.
// Keep that exact projection: grouping and the new mapping switch are not source
// identity. Eligibility is still checked by channels_for/provider_enabled.
struct V32Mapping<'a>(&'a channels::ModelMapping);
impl std::fmt::Debug for V32Mapping<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelMapping")
            .field("model", &self.0.model)
            .field("upstream", &self.0.upstream)
            .field("vision", &self.0.vision)
            .field("drop_fields", &self.0.drop_fields)
            .finish()
    }
}
struct V32Channel<'a>(&'a channels::ChannelRow);
impl std::fmt::Debug for V32Channel<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let row = self.0;
        let models: Vec<_> = row.models.iter().map(V32Mapping).collect();
        f.debug_struct("ChannelRow")
            .field("id", &row.id)
            .field("name", &row.name)
            .field("base_url", &row.base_url)
            .field("enabled", &row.enabled)
            .field("has_secret", &row.has_secret)
            .field("unwrap_envelope", &row.unwrap_envelope)
            .field("max_concurrent", &row.max_concurrent)
            .field("priority", &row.priority)
            .field("headers", &row.headers)
            .field("models", &models)
            .finish()
    }
}
fn fingerprint(row: &channels::ChannelRow) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", V32Channel(row)).hash(&mut h);
    h.finish().to_string()
}
fn answer(text:&str)->String {
    let Ok(mut v)=serde_json::from_str::<serde_json::Value>(text) else{return String::new()};
    if v["choices"].is_null() && v["data"].is_object(){v=v["data"].clone();}
    v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string()
}
fn correct(text:&str)->bool {
    matches!(answer(text).to_lowercase().trim_matches(|c:char|c.is_ascii_punctuation()||c.is_whitespace()),"红"|"红色"|"紅"|"紅色"|"red")
}
pub fn spawn(state:SharedState)->tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut timer=tokio::time::interval(Duration::from_secs(5));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {timer.tick().await;if let Err(e)=tick(&state).await{tracing::warn!("自动测视觉失败：{e}");}}
    })
}
pub async fn tick(state:&SharedState)->db::Result<()> {
    let job={
        let conn=state.conn();let mut job=None;
        'outer: for route in model_routes::list(&conn)? {
            if db::route_policy::load(&conn, &route.name)?.probe_enabled == Some(false) {continue;}
            if route.kind!=ModelKind::Chat || route.kind_conflict {continue;}
            for mut channel in model_routes::channels_for(&conn,&route.name)? {
                let Some(row)=channels::get(&conn,channel.id)? else{continue};
                if !crate::protocol_config::background_probe_allowed(&conn,channel.id.0)? {continue;}
                if crate::db::faults::has_fault(&conn,channel.id.0,Some(&route.name))? {continue;}
                let fp=fingerprint(&row);
                let old=conn.query_row("SELECT fingerprint,tries,next_at,status FROM model_vision_checks WHERE channel_id=?1 AND model_name=?2",rusqlite::params![channel.id.0,route.name],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?))).ok();
                let tries=match old {
                    Some((f,n,at,status)) if f==fp=>{if status=="视觉已验证"||n>=3||at>now_ms(){continue;}n},
                    _=>0,
                };
                conn.execute("INSERT INTO model_vision_checks(channel_id,model_name,fingerprint,tries,next_at,status) VALUES(?1,?2,?3,?4,?5,'检测中') ON CONFLICT(channel_id,model_name) DO UPDATE SET fingerprint=excluded.fingerprint,tries=excluded.tries,next_at=excluded.next_at,status='检测中'",rusqlite::params![channel.id.0,route.name,fp,tries+1,now_ms()+1800000])?;
                channel.headers=channels::headers(&conn,&state.secrets,channel.id)?;
                let secret=channels::secret(&conn,&state.secrets,channel.id)?;
                job=Some((channel,route.name,fp,secret));break 'outer;
            }
        } job
    };
    let Some((channel,name,fp,secret))=job else{return Ok(())};
    let mut passed=false;let mut status="视觉未确认，稍后重试".to_string();
    for budget in [crate::prober::VISION_TEST_TOKENS,crate::prober::VISION_TEST_TOKENS_RETRY] {
        {
            let conn=state.conn();
            if db::route_policy::load(&conn, &name)?.probe_enabled == Some(false) {return Ok(());}
            if channels::get(&conn,channel.id)?.map(|r|fingerprint(&r)!=fp).unwrap_or(true){return Ok(());}
            if !model_routes::channels_for(&conn,&name)?.iter().any(|c|c.id==channel.id){return Ok(());}
        }
        let sent=channel.model_alias.as_deref().unwrap_or(&name);
        let body=crate::prober::vision_test_body(sent,budget);
        let result=tokio::time::timeout(Duration::from_secs(35),async {
            let mut resp=state.upstream.call(&channel,secret.as_deref(),EndpointKind::Chat.path(),body,false,Duration::from_secs(30),&crate::upstream::new_uuid(),now_ms(),None).await.ok()?;
            let code=resp.status().as_u16();let mut bytes=Vec::new();
            while let Some(chunk)=resp.chunk().await.ok()?{if bytes.len()+chunk.len()>262144{return None;}bytes.extend_from_slice(&chunk);}
            Some((code,String::from_utf8_lossy(&bytes).to_string()))
        }).await.ok().flatten();
        match result {
            Some((code,text)) if (200..300).contains(&code)=>{
                if correct(&text){passed=true;status="视觉已验证".into();break;}
                if budget==crate::prober::VISION_TEST_TOKENS && crate::prober::reply_was_truncated(&text){continue;}
                break;
            }
            Some((code,_))=>{status=format!("视觉检测 HTTP {code}，稍后重试");break;}
            None=>{status="视觉检测连接失败或超时，稍后重试".into();break;}
        }
    }
    let conn=state.conn();
    if channels::get(&conn,channel.id)?.map(|r|fingerprint(&r)==fp).unwrap_or(false){
        if passed {channels::set_model_vision(&conn,channel.id,&name,true)?;}
        let newfp=channels::get(&conn,channel.id)?.map(|r|fingerprint(&r)).unwrap_or(fp);
        conn.execute("UPDATE model_vision_checks SET status=?3,fingerprint=?4 WHERE channel_id=?1 AND model_name=?2",rusqlite::params![channel.id.0,name,status,newfp])?;
    }
    tracing::info!("自动测视觉 {} 渠道 {}：{}",name,channel.id.0,status);
    Ok(())
}
#[cfg(test)]mod tests{use super::*;#[test]fn only_an_explicit_correct_answer_passes(){assert!(correct(r#"{"choices":[{"message":{"content":"红色"}}]}"#));assert!(!correct(r#"{"choices":[{"message":{"content":"我看不到图片，可能是红色"}}]}"#));assert!(!correct(r#"{"choices":[]}"#));}}

#[cfg(test)]
mod fingerprint_contract_tests {
    use super::*;
    use tokengo_shared::ChannelId;
    const V32_HASH: &str = "11489058105077496160";
    fn legacy_row() -> channels::ChannelRow {
        channels::ChannelRow {
            provider_id: None,
            provider_name: None,
            provider_enabled: true,
            id: ChannelId(9),
            name: "legacy".into(),
            base_url: "https://fixture.invalid/v1".into(),
            enabled: true,
            has_secret: true,
            unwrap_envelope: false,
            max_concurrent: 2,
            priority: 7,
            headers: vec![("x-test".into(), "static".into())],
            models: vec![channels::ModelMapping {
                enabled: true,
                model: "logical".into(),
                upstream: Some("upstream".into()),
                vision: true,
                drop_fields: vec!["vendor".into()],
            }],
        }
    }
    #[test]
    fn fingerprint_keeps_fixed_v32_debug_hash_contract() {
        assert_eq!(fingerprint(&legacy_row()), V32_HASH);
    }
    #[test]
    fn provider_metadata_and_model_switch_do_not_reset_existing_backoff() {
        let original = legacy_row();
        let mut assigned = original.clone();
        assigned.provider_id = Some(12);
        assigned.provider_name = Some("Provider A".into());
        let mut renamed = assigned.clone();
        renamed.provider_name = Some("Provider B".into());
        let mut disabled = renamed.clone();
        disabled.provider_enabled = false;
        let mut mapping_off = disabled.clone();
        mapping_off.models[0].enabled = false;
        for row in [original, assigned, renamed, disabled, mapping_off] {
            // Existing v32 records keep their tries/next_at when the fingerprint matches.
            assert_eq!(fingerprint(&row), V32_HASH);
        }
    }
    #[test]
    fn source_and_vision_contract_changes_still_invalidate_fingerprint() {
        let original = legacy_row();
        let old = fingerprint(&original);
        let mut upstream = original.clone();
        upstream.models[0].upstream = Some("different".into());
        let mut vision = original.clone();
        vision.models[0].vision = false;
        let mut headers = original.clone();
        headers.headers[0].1 = "different".into();
        let mut url = original.clone();
        url.base_url = "https://other.invalid/v1".into();
        let mut dropped = original.clone();
        dropped.models[0].drop_fields.push("another".into());
        let mut connection_off = original.clone();
        connection_off.enabled = false;
        for changed in [upstream, vision, headers, url, dropped, connection_off] {
            assert_ne!(fingerprint(&changed), old);
        }
    }
}
