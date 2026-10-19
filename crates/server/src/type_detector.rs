//! Autonomous, serial type discovery. No user traffic or routing-state writes.
use std::{hash::{Hash, Hasher}, time::Duration};

use crate::{SharedState, db::{self, channels, kinds, model_routes, now_ms}, gateway::EndpointKind};

const RETRY_MS: i64 = 30 * 60 * 1000;
const MAX_TRIES: i64 = 3;

pub fn spawn(state: SharedState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(5));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            timer.tick().await;
            if let Err(e) = tick(&state).await { tracing::warn!("自动识别类型失败：{e}"); }
        }
    })
}

fn valid(kind: EndpointKind, body: &str) -> bool {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(body) else { return false; };
    if v.get("choices").is_none() && v.get("data").and_then(|x| x.get("choices")).is_some() { v = v["data"].clone(); }
    match kind {
        EndpointKind::Chat => v["choices"].as_array().map(|a| a.iter().any(|c| {
            let m = &c["message"];
            ["content","reasoning_content"].iter().any(|k| m[*k].as_str().map(|s| !s.trim().is_empty()).unwrap_or(false))
                || m["tool_calls"].as_array().map(|t| !t.is_empty()).unwrap_or(false)
        })).unwrap_or(false),
        EndpointKind::Embeddings => v["data"].as_array().map(|a| a.iter().any(|x| x["embedding"].as_array().map(|e| !e.is_empty() && e.iter().all(|n| n.is_number())).unwrap_or(false))).unwrap_or(false),
        EndpointKind::Rerank => v["results"].as_array().map(|a| a.iter().any(|x| x["index"].is_u64() && x["relevance_score"].is_number())).unwrap_or(false),
    }
}

/// One model/channel per tick; every endpoint is re-authorized immediately before sending.
pub async fn tick(state: &SharedState) -> db::Result<()> {
    let job = {
        let conn = state.conn();
        let routes = model_routes::list(&conn)?;
        let mut job = None;
        'outer: for r in routes {
            if db::route_policy::load(&conn, &r.name)?.probe_enabled == Some(false) { continue; }
            if r.kind.is_set() || r.kind_conflict { continue; }
            for id in r.channels {
                let Some(row) = channels::get(&conn,id)? else { continue; };
                if crate::db::faults::has_fault(&conn,id.0,Some(&r.name))? {continue;}
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                format!("{:?}",row).hash(&mut hasher);
                let fingerprint = hasher.finish().to_string();
                let prior = conn.query_row("SELECT fingerprint, tries, next_at FROM model_type_checks WHERE channel_id=?1 AND model_name=?2",rusqlite::params![id.0,r.name],|x| Ok((x.get::<_,String>(0)?,x.get::<_,i64>(1)?,x.get::<_,i64>(2)?))).ok();
                let tries = match prior {
                    Some((fp,n,next)) if fp==fingerprint => { if n>=MAX_TRIES || next>now_ms() { continue; } n },
                    _ => 0,
                };
                conn.execute("INSERT INTO model_type_checks(channel_id,model_name,fingerprint,tries,next_at,status) VALUES(?1,?2,?3,?4,?5,'检测中') ON CONFLICT(channel_id,model_name) DO UPDATE SET fingerprint=excluded.fingerprint,tries=excluded.tries,next_at=excluded.next_at,status='检测中'",rusqlite::params![id.0,r.name,fingerprint,tries+1,now_ms()+RETRY_MS])?;
                job=Some((id,r.name,fingerprint)); break 'outer;
            }
        }
        job
    };
    let Some((id,name,fingerprint))=job else { return Ok(()); };
    let mut detected = None;
    let mut reason = "识别失败".to_string();
    for endpoint in [EndpointKind::Chat,EndpointKind::Embeddings,EndpointKind::Rerank] {
        let loaded = {
            let conn=state.conn();
            if db::route_policy::load(&conn, &name)?.probe_enabled == Some(false) { break; }
            let route=model_routes::routes_for(&conn,&name)?;
            if route.as_ref().map(|r| r.kind.is_set() || !r.channels.contains(&id)).unwrap_or(true) { break; }
            let channel=model_routes::channels_for(&conn,&name)?.into_iter().find(|c| c.id==id);
            match channel { Some(mut c) => { c.headers=channels::headers(&conn,&state.secrets,id)?; Some((c,channels::secret(&conn,&state.secrets,id)?)) }, None=>None }
        };
        let Some((channel,secret))=loaded else { break; };
        let sent=channel.model_alias.as_deref().unwrap_or(&name);
        let mut body:serde_json::Value=serde_json::from_slice(&crate::prober::probe_body(endpoint,sent)).unwrap();
        if endpoint==EndpointKind::Chat { body["max_tokens"]=serde_json::json!(16); }
        if let Some(obj)=body.as_object_mut() {
            for field in &channel.drop_fields { obj.remove(field); }
        }
        let body=bytes::Bytes::from(serde_json::to_vec(&body).unwrap());
        let result=tokio::time::timeout(Duration::from_secs(12),async {
            let mut response=state.upstream.call(&channel,secret.as_deref(),endpoint.path(),body,false,Duration::from_secs(10),&crate::upstream::new_uuid(),now_ms(),None).await.ok()?;
            let status=response.status().as_u16();
            let mut buf=Vec::new();
            while let Some(chunk)=response.chunk().await.ok()? {
                if buf.len()+chunk.len()>262144 { return None; }
                buf.extend_from_slice(&chunk);
            }
            Some((status,String::from_utf8_lossy(&buf).to_string()))
        }).await.ok().flatten();
        match result {
            Some((code,text)) if (200..300).contains(&code) && valid(endpoint,&text) => { detected=Some(endpoint.as_model_kind()); break; }
            Some((code,_)) if [401,403,429].contains(&code) || code>=500 => { reason=format!("识别失败 HTTP {code}，稍后重试"); break; }
            None => { reason="识别超时或连接失败，稍后重试".into(); break; }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    let conn=state.conn();
    // A deleted/changed mapping must never receive a late result from an old request.
    let current=channels::get(&conn,id)?;
    let unchanged=current.map(|row| { let mut h=std::collections::hash_map::DefaultHasher::new();format!("{:?}",row).hash(&mut h);h.finish().to_string()==fingerprint }).unwrap_or(false);
    if unchanged {
        if let Some(kind)=detected {
            if model_routes::routes_for(&conn,&name)?.map(|r| !r.kind.is_set() && !r.kind_conflict).unwrap_or(false) {
                kinds::learn(&conn,&name,kind)?;
                reason=format!("已识别：{}",kind.label());
            }
        }
        conn.execute("UPDATE model_type_checks SET status=?3 WHERE channel_id=?1 AND model_name=?2",rusqlite::params![id.0,name,reason])?;
    }
    tracing::info!("自动识别模型 {} 渠道 {}：{}",name,id.0,reason);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_endpoint_specific_payload_not_just_http_200() {
        assert!(!valid(EndpointKind::Chat,r#"{"choices":[]}"#));
        assert!(valid(EndpointKind::Chat,r#"{"choices":[{"message":{"content":"ok"}}]}"#));
        assert!(!valid(EndpointKind::Embeddings,r#"{"choices":[{"message":{"content":"ok"}}]}"#));
        assert!(valid(EndpointKind::Embeddings,r#"{"data":[{"embedding":[0.1,0.2]}]}"#));
        assert!(valid(EndpointKind::Rerank,r#"{"results":[{"index":0,"relevance_score":0.5}]}"#));
    }
}
