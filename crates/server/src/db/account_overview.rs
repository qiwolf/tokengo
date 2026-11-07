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
#[cfg(test)]
struct Aggregate { name:Option<String>, total:i64,ok:i64,failed:i64,canceled:i64,client_errors:i64,input:i64,output:i64,avg:i64,usage:UsageCoverage }
/// All dimensions derive from this owner-filtered SQL relation, including totals.
#[cfg(test)]
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
#[cfg(test)]
fn stats_since_legacy(c:&Connection,owner:i64,since:i64)->Result<Stats> {
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
/// SQL owns the historical CASE/NULL/boolean semantics; one owner/window scan
/// feeds every dimension without loading administrative projections or caches.
const OWNER_STATS_SQL:&str="SELECT status,input_tokens,output_tokens,latency_ms,
 NOT(input_tokens_known AND output_tokens_known),NOT input_tokens_known,NOT output_tokens_known,usage_source='legacy',
 CASE WHEN endpoint LIKE 'passthrough:%' THEN 'HTTP 透传' WHEN TRIM(served_model)<>'' AND LOWER(TRIM(served_model))<>'auto' THEN served_model WHEN channel_id IS NULL THEN '未调用上游' WHEN LOWER(TRIM(model))='auto' THEN '实际模型未记录' ELSE model END,
 client_ip,CASE endpoint WHEN '/v1/chat/completions' THEN 'chat' WHEN '/v1/embeddings' THEN 'embeddings' WHEN '/v1/rerank' THEN 'rerank' ELSE '' END
 FROM requests INDEXED BY requests_owner_stats_window WHERE user_id=?1 AND ts_ms>=?2";

fn nullable_group(row:&rusqlite::Row<'_>,column:usize,groups:&mut std::collections::HashMap<String,super::requests::RequestAggregate>,missing:&mut super::requests::RequestAggregate,sample:&super::requests::StatsSample<'_>)->Result<()> {
    if row.get_ref(column)?==rusqlite::types::ValueRef::Null {missing.add(sample)}
    else {super::requests::add_stats_group(groups,super::requests::stats_text(row,column)?,sample)}
}
fn ranked_groups(groups:std::collections::HashMap<String,super::requests::RequestAggregate>,missing:super::requests::RequestAggregate)->Vec<(Option<String>,super::requests::RequestAggregate)> {
    let mut rows=groups.into_iter().map(|(name,row)|(Some(name),row)).collect::<Vec<_>>();
    if missing.requests>0 {rows.push((None,missing));}
    // Rank before mapping nullable model names to their display fallback. SQL
    // treats NULL and empty strings as separate groups, including top-50 ties.
    rows.sort_unstable_by(|a,b|b.1.requests.cmp(&a.1.requests).then_with(||b.0.cmp(&a.0)));
    rows.truncate(50);rows
}
pub fn stats_since(c:&Connection,owner:i64,since:i64)->Result<Stats> {
    use super::requests::{RequestAggregate,StatsSample,stats_text};
    let mut total=RequestAggregate::default();
    let mut models=std::collections::HashMap::new();let mut missing_model=RequestAggregate::default();
    let mut clients=std::collections::HashMap::new();let mut missing_client=RequestAggregate::default();
    let mut kinds:[RequestAggregate;4]=std::array::from_fn(|_|RequestAggregate::default());
    let mut query=c.prepare(OWNER_STATS_SQL)?;let mut rows=query.query(params![owner,since])?;
    while let Some(row)=rows.next()? {
        let status=if row.get_ref(0)?==rusqlite::types::ValueRef::Null {""}else{stats_text(row,0)?};
        let sample=StatsSample{status,input_tokens:row.get::<_,Option<i64>>(1)?.unwrap_or(0),output_tokens:row.get::<_,Option<i64>>(2)?.unwrap_or(0),latency:row.get(3)?,usage:UsageCoverage{
            unknown_requests:row.get::<_,Option<i64>>(4)?.unwrap_or(0),unknown_input_requests:row.get::<_,Option<i64>>(5)?.unwrap_or(0),unknown_output_requests:row.get::<_,Option<i64>>(6)?.unwrap_or(0),legacy_requests:row.get::<_,Option<i64>>(7)?.unwrap_or(0)}};
        total.add(&sample)?;
        nullable_group(row,8,&mut models,&mut missing_model,&sample)?;
        nullable_group(row,9,&mut clients,&mut missing_client,&sample)?;
        let kind=match stats_text(row,10)? {"chat"=>0,"embeddings"=>1,"rerank"=>2,_=>3};
        kinds[kind].add(&sample)?;
    }
    let by_model=ranked_groups(models,missing_model).into_iter().map(|(name,a)|ModelStat{model:name.unwrap_or_default(),usage:a.usage,requests:a.requests,ok:a.ok,failed:a.failed,client_errors:a.client_errors,input_tokens:a.input_tokens,output_tokens:a.output_tokens}).collect();
    let by_client=ranked_groups(clients,missing_client).into_iter().map(|(client_ip,a)|ClientStat{client_ip,usage:a.usage,requests:a.requests,ok:a.ok,failed:a.failed,client_errors:a.client_errors,input_tokens:a.input_tokens,output_tokens:a.output_tokens}).collect();
    let by_kind=kinds.into_iter().zip([("chat","对话"),("embeddings","向量"),("rerank","重排"),("","别的")]).filter_map(|(a,(kind,label))|{
        if kind.is_empty()&&a.requests==0 {return None;}
        let avg_ms=a.average_latency();
        Some(KindStat{kind:kind.into(),label:label.into(),usage:a.usage,requests:a.requests,ok:a.ok,failed:a.failed,client_errors:a.client_errors,canceled:a.canceled,input_tokens:a.input_tokens,output_tokens:a.output_tokens,avg_ms})
    }).collect();
    Ok(Stats{usage:total.usage,since_ms:since,total:total.requests,ok:total.ok,failed:total.failed,canceled:total.canceled,client_errors:total.client_errors,input_tokens:total.input_tokens,output_tokens:total.output_tokens,by_channel:vec![],by_kind,by_model,by_client})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_pass_owner_stats_match_sql_oracle_for_nullable_history_and_ties() {
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE requests(id INTEGER PRIMARY KEY,user_id INTEGER,ts_ms INTEGER,endpoint TEXT,model TEXT,served_model TEXT,status TEXT,channel_id INTEGER,latency_ms INTEGER,input_tokens INTEGER,output_tokens INTEGER,input_tokens_known INTEGER,output_tokens_known INTEGER,usage_source TEXT,client_ip TEXT); CREATE INDEX requests_owner_stats_window ON requests(user_id,ts_ms DESC,id DESC,endpoint,status,channel_id,latency_ms,input_tokens,output_tokens,client_ip,served_model,model,input_tokens_known,output_tokens_known,usage_source);").unwrap();
        let mut seed=29u64;
        for i in 0..2000i64 {
            seed=seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let pick=(seed>>32) as usize;
            let endpoint=[None,Some("/v1/chat/completions"),Some(" /v1/chat/completions "),Some("/v1/embeddings"),Some("/v1/rerank"),Some("passthrough:42"),Some("other")][pick%7];
            let model=if i%11==0 {None}else{Some(format!("模型-{:02}",i%70))};
            let served=match i%9 {0=>None,1=>Some(String::new()),2=>Some(" AUTO ".into()),3=>Some(" \tauto\t ".into()),4=>Some("ÄUTO".into()),_=>Some(format!("served-{:02}",i%70))};
            let client=match i%8 {0=>None,1=>Some(String::new()),_=>Some(format!("来源-{:02}",i%70))};
            let status=[None,Some("ok"),Some("failed"),Some("canceled"),Some("client_error"),Some("pending")][pick%6];
            let known=[None,Some(0),Some(1),Some(2),Some(-1)];
            c.execute("INSERT INTO requests VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![i+1,if i%5==0{2}else{1},i,endpoint,model,served,status,if i%13==0{None}else{Some(i%3)},if i%7==0{None}else{Some(i%19-5)},if i%17==0{None}else{Some(i%11-2)},if i%19==0{None}else{Some(i%13-3)},known[pick%5],known[(pick/5)%5],if i%4==0{None}else if i%3==0{Some("legacy")}else{Some("upstream")},client]).unwrap();
        }
        // Force separate NULL and empty-name groups, plus SQL passthrough masking.
        c.execute_batch("INSERT INTO requests(user_id,ts_ms,endpoint,model,served_model,status,channel_id) VALUES(1,2001,'other',NULL,NULL,'ok',1),(1,2002,'other','','','ok',1),(1,2003,'passthrough:1','hidden','hidden','ok',1);").unwrap();
        for owner in [1,2,3] {for since in [0,301,1777,2001,9999] {
            assert_eq!(stats_since(&c,owner,since).unwrap(),stats_since_legacy(&c,owner,since).unwrap(),"owner={owner} since={since}");
        }}
    }
    #[test]
    fn owner_stats_preserve_sql_integer_overflow_errors() {
        let c=crate::db::open_in_memory().unwrap();
        for input in [i64::MAX,1] {
            c.execute("INSERT INTO requests(user_id,ts_ms,endpoint,model,status,input_tokens) VALUES(1,1,'/v1/chat/completions','model','ok',?1)",[input]).unwrap();
        }
        assert!(stats_since_legacy(&c,1,0).is_err());
        assert!(stats_since(&c,1,0).is_err());
        assert_eq!(stats_since(&c,2,0).unwrap().total,0);
    }
    #[test]
    fn owner_index_bounds_recent_and_window_queries_despite_foreign_noise() {
        let c=crate::db::open_in_memory().unwrap();
        c.execute_batch("WITH RECURSIVE n(i) AS(VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<10000) INSERT INTO requests(user_id,ts_ms,endpoint,model,status) SELECT 2,i,'/v1/chat/completions','foreign','ok' FROM n; INSERT INTO requests(user_id,ts_ms,endpoint,model,status) VALUES(1,100,'/v1/chat/completions','own','ok');").unwrap();
        let mut query=c.prepare(&format!("EXPLAIN QUERY PLAN {SELECT} {RECENT_FILTER}")).unwrap();
        let plan=query.query_map(params![1,Option::<i64>::None,101],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert!(plan.iter().any(|s|s.contains("SEARCH r USING INDEX requests_owner_recent (user_id=?)")),"{plan:?}");
        assert!(!plan.iter().any(|s|s.starts_with("SCAN r")),"{plan:?}");
        let mut query=c.prepare(&format!("EXPLAIN QUERY PLAN {OWNER_STATS_SQL}")).unwrap();
        let plan=query.query_map([1,0],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert!(plan.iter().any(|s|s.contains("COVERING INDEX requests_owner_stats_window (user_id=? AND ts_ms>?)")),"{plan:?}");
        let table:i64=c.query_row("SELECT rootpage FROM sqlite_master WHERE type='table' AND name='requests'",[],|r|r.get(0)).unwrap();
        let mut query=c.prepare(&format!("EXPLAIN {OWNER_STATS_SQL}")).unwrap();
        for row in query.query_map([1,0],|r|Ok((r.get::<_,String>(1)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?))).unwrap() {
            let(op,page,database)=row.unwrap();
            assert!(!(op=="OpenRead"&&page==table&&database==0),"owner statistics must not open wide requests table");
        }
        assert_eq!(recent(&c,1,100,None).unwrap().logs.len(),1);assert_eq!(stats_since(&c,1,0).unwrap().total,1);
    }
}
