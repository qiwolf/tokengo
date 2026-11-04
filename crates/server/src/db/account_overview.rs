//! Owner-only projections. Never load a global source catalogue or administrative request DTO.
use super::{requests::{ClientStat, KindStat, ModelStat, Stats, UsageCoverage}, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct RequestRow {
    pub id:i64, pub ts_ms:i64, pub endpoint:String, pub request_path:Option<String>,
    pub model:String, pub served_model:String, pub status:String, pub http_status:Option<i64>, pub latency_ms:i64,
    pub input_tokens:i64, pub output_tokens:i64, pub input_tokens_known:bool,
    pub output_tokens_known:bool, pub usage_source:String, pub key_id:Option<i64>,
    pub key_display_name:Option<String>, pub client_ip:Option<String>, pub has_image:bool,
    pub first_byte_ms:Option<i64>, pub streamed:bool,
}
#[derive(Debug, Serialize)]
pub struct AttemptRow {
    pub ordinal:i64, pub outcome:String, pub http_status:Option<i64>, pub duration_ms:i64,
}
#[derive(Debug, Serialize)]
pub struct RequestDetail { pub request:RequestRow, pub attempts:Vec<AttemptRow> }
#[derive(Debug, Serialize)]
pub struct RequestPage { pub logs:Vec<RequestRow>, pub next_before_id:Option<i64> }
const SELECT:&str="SELECT r.id,r.ts_ms,
 CASE WHEN r.endpoint LIKE 'passthrough:%' THEN 'passthrough' ELSE r.endpoint END,
 r.request_path, CASE WHEN r.endpoint LIKE 'passthrough:%' THEN 'HTTP 透传' ELSE r.model END,
 CASE WHEN r.endpoint LIKE 'passthrough:%' THEN 'HTTP 透传' ELSE r.served_model END,r.status,r.http_status,r.latency_ms,r.input_tokens,r.output_tokens,
 r.input_tokens_known,r.output_tokens_known,r.usage_source,
 CASE WHEN g.user_id=r.user_id THEN r.key_id ELSE NULL END,g.name,r.client_ip,r.has_image,r.first_byte_ms,r.streamed
 FROM requests r LEFT JOIN gateway_keys g ON g.id=r.key_id AND g.user_id=r.user_id";
fn request_row(r:&rusqlite::Row<'_>)->rusqlite::Result<RequestRow> {
    Ok(RequestRow{id:r.get(0)?,ts_ms:r.get(1)?,endpoint:r.get(2)?,request_path:r.get(3)?,model:r.get(4)?,served_model:r.get(5)?,status:r.get(6)?,http_status:r.get(7)?,latency_ms:r.get(8)?,input_tokens:r.get(9)?,output_tokens:r.get(10)?,input_tokens_known:r.get(11)?,output_tokens_known:r.get(12)?,usage_source:r.get(13)?,key_id:r.get(14)?,key_display_name:r.get(15)?,client_ip:r.get(16)?,has_image:r.get(17)?,first_byte_ms:r.get(18)?,streamed:r.get(19)?})
}
/// Sort by recorded time and ID. Resolve the cursor under the same owner gate.
const RECENT_FILTER:&str="WHERE r.user_id=?1 AND (?2 IS NULL OR (r.ts_ms,r.id)<(SELECT ts_ms,id FROM requests WHERE id=?2 AND user_id=?1)) ORDER BY r.ts_ms DESC,r.id DESC LIMIT ?3";
pub fn recent(c:&Connection,owner:i64,limit:u32,before:Option<i64>)->Result<RequestPage> {
    let mut query=c.prepare(&format!("{SELECT} {RECENT_FILTER}"))?;
    let mut logs=query.query_map(params![owner,before,limit+1],request_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let more=logs.len()>limit as usize;logs.truncate(limit as usize);
    let next_before_id=if more {logs.last().map(|r|r.id)} else {None};
    Ok(RequestPage{logs,next_before_id})
}
pub fn detail(c:&Connection,owner:i64,id:i64)->Result<Option<RequestDetail>> {
    let request=c.query_row(&format!("{SELECT} WHERE r.user_id=?1 AND r.id=?2"),params![owner,id],request_row).optional()?;
    let Some(request)=request else {return Ok(None)};
    let mut query=c.prepare("SELECT a.ordinal,
 CASE WHEN a.outcome IN ('success','channel_fault','client_error','canceled','not_sent') THEN a.outcome ELSE 'unknown' END,
 a.http_status,a.duration_ms FROM attempts a JOIN requests r ON r.id=a.request_id
 WHERE r.user_id=?1 AND r.id=?2 ORDER BY a.ordinal,a.id")?;
    let attempts=query.query_map(params![owner,id],|r|Ok(AttemptRow{ordinal:r.get(0)?,outcome:r.get(1)?,http_status:r.get(2)?,duration_ms:r.get(3)?}))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(RequestDetail{request,attempts}))
}
struct Aggregate { name:Option<String>, total:i64,ok:i64,failed:i64,canceled:i64,client_errors:i64,input:i64,output:i64,avg:i64,usage:UsageCoverage }
/// All dimensions derive from this owner-filtered SQL relation, including totals.
fn aggregate(c:&Connection,owner:i64,since:i64,dimension:&str)->Result<Vec<Aggregate>> {
    let expr=match dimension {
        "model"=>"CASE WHEN endpoint LIKE 'passthrough:%' THEN 'HTTP 透传' WHEN TRIM(served_model)<>'' AND LOWER(TRIM(served_model))<>'auto' THEN served_model WHEN channel_id IS NULL THEN '未调用上游' WHEN LOWER(TRIM(model))='auto' THEN '实际模型未记录' ELSE model END",
        "client"=>"client_ip",
        "kind"=>"CASE endpoint WHEN '/v1/chat/completions' THEN 'chat' WHEN '/v1/embeddings' THEN 'embeddings' WHEN '/v1/rerank' THEN 'rerank' ELSE '' END",
        _=>"'total'",
    };
    let mut query=c.prepare(&format!("SELECT {expr} AS dimension, COUNT(*),
 COALESCE(SUM(status='ok'),0),COALESCE(SUM(status='failed'),0),COALESCE(SUM(status='canceled'),0),COALESCE(SUM(status='client_error'),0),
 COALESCE(SUM(input_tokens),0),COALESCE(SUM(output_tokens),0),COALESCE(CAST(AVG(CASE WHEN status='ok' THEN latency_ms END) AS INTEGER),0),
 COALESCE(SUM(NOT(input_tokens_known AND output_tokens_known)),0),COALESCE(SUM(NOT input_tokens_known),0),COALESCE(SUM(NOT output_tokens_known),0),COALESCE(SUM(usage_source='legacy'),0)
 FROM requests WHERE user_id=?1 AND ts_ms>=?2 GROUP BY dimension ORDER BY COUNT(*) DESC,dimension DESC"))?;
    let rows=query.query_map(params![owner,since],|r|Ok(Aggregate{name:r.get(0)?,total:r.get(1)?,ok:r.get(2)?,failed:r.get(3)?,canceled:r.get(4)?,client_errors:r.get(5)?,input:r.get(6)?,output:r.get(7)?,avg:r.get(8)?,usage:UsageCoverage{unknown_requests:r.get(9)?,unknown_input_requests:r.get(10)?,unknown_output_requests:r.get(11)?,legacy_requests:r.get(12)?}}))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn stats_since(c:&Connection,owner:i64,since:i64)->Result<Stats> {
    let total=aggregate(c,owner,since,"total")?.pop();
    let mut result=Stats{usage:UsageCoverage::default(),since_ms:since,total:0,ok:0,failed:0,canceled:0,client_errors:0,input_tokens:0,output_tokens:0,by_channel:vec![],by_kind:vec![],by_model:vec![],by_client:vec![]};
    if let Some(a)=total {result.usage=a.usage;result.total=a.total;result.ok=a.ok;result.failed=a.failed;result.canceled=a.canceled;result.client_errors=a.client_errors;result.input_tokens=a.input;result.output_tokens=a.output;}
    result.by_model=aggregate(c,owner,since,"model")?.into_iter().take(50).map(|a|ModelStat{usage:a.usage,model:a.name.unwrap_or_default(),requests:a.total,ok:a.ok,failed:a.failed,client_errors:a.client_errors,input_tokens:a.input,output_tokens:a.output}).collect();
    result.by_client=aggregate(c,owner,since,"client")?.into_iter().take(50).map(|a|ClientStat{usage:a.usage,client_ip:a.name,requests:a.total,ok:a.ok,failed:a.failed,client_errors:a.client_errors,input_tokens:a.input,output_tokens:a.output}).collect();
    let mut kinds=aggregate(c,owner,since,"kind")?;
    for (kind,label) in [("chat","对话"),("embeddings","向量"),("rerank","重排"),("","别的")] {
        let row=if let Some(index)=kinds.iter().position(|a|a.name.as_deref()==Some(kind)) {let a=kinds.remove(index);KindStat{usage:a.usage,kind:kind.into(),label:label.into(),requests:a.total,ok:a.ok,failed:a.failed,client_errors:a.client_errors,canceled:a.canceled,input_tokens:a.input,output_tokens:a.output,avg_ms:a.avg}} else {KindStat{usage:UsageCoverage::default(),kind:kind.into(),label:label.into(),requests:0,ok:0,failed:0,client_errors:0,canceled:0,input_tokens:0,output_tokens:0,avg_ms:0}};
        if !kind.is_empty()||row.requests>0 {result.by_kind.push(row)}
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_index_bounds_recent_and_window_queries_despite_foreign_noise() {
        let c=crate::db::open_in_memory().unwrap();
        c.execute_batch("WITH RECURSIVE n(i) AS(VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<10000) INSERT INTO requests(user_id,ts_ms,endpoint,model,status) SELECT 2,i,'/v1/chat/completions','foreign','ok' FROM n; INSERT INTO requests(user_id,ts_ms,endpoint,model,status) VALUES(1,100,'/v1/chat/completions','own','ok');").unwrap();
        let mut query=c.prepare(&format!("EXPLAIN QUERY PLAN {SELECT} {RECENT_FILTER}")).unwrap();
        let plan=query.query_map(params![1,Option::<i64>::None,101],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert!(plan.iter().any(|s|s.contains("SEARCH r USING INDEX requests_owner_recent (user_id=?)")),"{plan:?}");
        assert!(!plan.iter().any(|s|s.starts_with("SCAN r")),"{plan:?}");
        let mut query=c.prepare("EXPLAIN QUERY PLAN SELECT COUNT(*),SUM(input_tokens) FROM requests WHERE user_id=?1 AND ts_ms>=?2").unwrap();
        let plan=query.query_map([1,0],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert!(plan.iter().any(|s|s.contains("requests_owner_recent (user_id=? AND ts_ms>?)")),"{plan:?}");
        assert_eq!(recent(&c,1,100,None).unwrap().logs.len(),1);assert_eq!(stats_since(&c,1,0).unwrap().total,1);
    }
}
