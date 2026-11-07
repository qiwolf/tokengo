//! Operational scores from attributable real attempts; old ambiguous rows are excluded.
use std::collections::HashMap;
use rusqlite::Connection;
use serde::Serialize;
use super::Result;
use tokengo_shared::normalize_model_name;
pub const WINDOW_MS:i64=2*60*60*1000;
#[derive(Debug,Clone,Default,Serialize)]
pub struct Score {
 pub samples:i64,pub successes:i64,pub faults:i64,
 pub avg_first_byte_ms:Option<i64>,
 pub avg_ms:Option<i64>,pub score:Option<f64>,pub last_at:Option<i64>,pub last_error:Option<String>,
}
impl Score {
 pub fn value(&self)->f64 {self.score.unwrap_or(-1.0)}
}
const SCORE_SQL:&str="SELECT a.model_name,a.outcome,a.duration_ms,r.ts_ms,a.error,a.first_byte_ms FROM requests r INDEXED BY requests_stats_window CROSS JOIN attempts a INDEXED BY idx_attempts_request ON a.request_id=r.id WHERE r.ts_ms>=?1 AND a.model_name IS NOT NULL AND a.model_name<>'' AND a.outcome IN ('success','channel_fault','empty_reply') AND r.status<>'canceled' AND (?2 IS NULL OR a.channel_id=?2) ORDER BY a.id";
const TOTALS_RAW_SQL:&str="SELECT m.request_id,m.model_name FROM model_invocation_dirty d CROSS JOIN model_invocations m ON m.request_id=d.request_id ORDER BY d.request_id,m.model_name";
pub fn for_channel(conn:&Connection,now:i64,channel:Option<i64>)->Result<HashMap<String,Score>> {
 let mut st=conn.prepare(SCORE_SQL)?;
 let mut out:HashMap<String,Score>=HashMap::new();let mut totals:HashMap<String,i64>=HashMap::new();
 let mut firsts:HashMap<String,(i64,i64)>=HashMap::new();
 let mut rows=st.query(rusqlite::params![now-WINDOW_MS,channel])?;
 while let Some(r)=rows.next()? {
  let key=normalize_model_name(&r.get::<_,String>(0)?);let v=out.entry(key.clone()).or_default();
  if r.get::<_,String>(1)?=="success" { if let Some(ms)=r.get::<_,Option<i64>>(5)? {let e=firsts.entry(key.clone()).or_default();e.0+=ms;e.1+=1;} }
  v.samples+=1;v.last_at=Some(r.get(3)?);
  if r.get::<_,String>(1)?=="success" {v.successes+=1;*totals.entry(key).or_default()+=r.get::<_,i64>(2)?.max(0);} else {v.faults+=1;v.last_error=r.get(4)?;}
 }
 for (name,v) in &mut out {
  v.avg_first_byte_ms=firsts.get(name).map(|(sum,n)|sum/n);
  if v.successes>0 {v.avg_ms=Some(totals.get(name).copied().unwrap_or(0)/v.successes);}
  // 90% stability + 10% bounded speed. This is not intelligence/answer-quality scoring.
  v.score=Some(((90.0*v.successes as f64/v.samples as f64 + v.avg_ms.map(|ms|10.0/(1.0+ms as f64/1000.0)).unwrap_or(0.0))*10.0).round()/10.0);
 }
 Ok(out)
}
pub fn all(conn:&Connection,now:i64)->Result<HashMap<String,Score>> {for_channel(conn,now,None)}
/// Exact lifetime totals; triggers mark only changed requests for recomputation.
/// This is derived data, never an authorization or routing cache.
pub fn total_calls(conn:&Connection)->Result<HashMap<String,i64>> {
 let dirty:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM model_invocation_dirty)",[],|r|r.get(0))?;
 if dirty {
  // SAVEPOINT also supports callers that already have a transaction. Roll back
  // both the aggregate and dirty queue together if normalization/SQL fails.
  conn.execute_batch("SAVEPOINT refresh_model_invocation_totals")?;
  let refreshed=refresh_totals(conn);
  match refreshed {
   Ok(())=>conn.execute_batch("RELEASE refresh_model_invocation_totals")?,
   Err(e)=>{let _=conn.execute_batch("ROLLBACK TO refresh_model_invocation_totals; RELEASE refresh_model_invocation_totals");return Err(e);}
  }
 }
 let mut q=conn.prepare("SELECT model_name,total_calls FROM model_invocation_totals WHERE total_calls>0")?;
 let totals=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
 Ok(totals)
}
fn refresh_totals(conn:&Connection)->Result<()> {
 let mut delta:HashMap<String,i64>=HashMap::new();
 {
  let mut old=conn.prepare("SELECT c.model_name,COUNT(*) FROM model_invocation_dirty d CROSS JOIN model_invocation_canonical c ON c.request_id=d.request_id GROUP BY c.model_name")?;
  for row in old.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))? {
   let(name,count)=row?;delta.insert(name,-count);
  }
 }
 conn.execute("DELETE FROM model_invocation_canonical WHERE request_id IN (SELECT request_id FROM model_invocation_dirty)",[])?;
 {
  let mut raw=conn.prepare(TOTALS_RAW_SQL)?;
  let mut rows=raw.query([])?;
  let mut insert=conn.prepare("INSERT INTO model_invocation_canonical(request_id,model_name) VALUES(?1,?2)")?;
  let mut names=HashMap::<String,String>::new();
  let mut seen=std::collections::HashSet::new();
  let mut previous=None;
  while let Some(row)=rows.next()? {
   let id:i64=row.get(0)?;
   if previous!=Some(id){seen.clear();previous=Some(id);}
   let raw:String=row.get(1)?;
   let key=names.entry(raw).or_insert_with_key(|name|normalize_model_name(name));
   if seen.insert(key.clone()) {
    insert.execute(rusqlite::params![id,key.as_str()])?;
    *delta.entry(key.clone()).or_default()+=1;
   }
  }
 }
 let mut update=conn.prepare("INSERT INTO model_invocation_totals(model_name,total_calls) VALUES(?1,?2) ON CONFLICT(model_name) DO UPDATE SET total_calls=total_calls+excluded.total_calls")?;
 for (name,count) in delta {
  if count>0 {update.execute(rusqlite::params![name,count])?;}
  else if count<0 {conn.execute("UPDATE model_invocation_totals SET total_calls=total_calls+?2 WHERE model_name=?1",rusqlite::params![name,count])?;}
 }
 conn.execute("DELETE FROM model_invocation_totals WHERE total_calls=0",[])?;
 conn.execute("DELETE FROM model_invocation_dirty",[])?;
 Ok(())
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]
 fn bounded_score_and_dirty_invocation_query_plans() {
  let c=crate::db::open_in_memory().unwrap();
  let mut q=c.prepare(&format!("EXPLAIN QUERY PLAN {SCORE_SQL}")).unwrap();
  let plan=q.query_map(rusqlite::params![0,Option::<i64>::None],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap().join("\n");
  assert!(plan.contains("SEARCH r USING COVERING INDEX requests_stats_window"),"{plan}");
  assert!(plan.contains("SEARCH a USING INDEX idx_attempts_request"),"{plan}");
  assert!(!plan.contains("SCAN a"),"{plan}");
  let table:i64=c.query_row("SELECT rootpage FROM sqlite_master WHERE type='table' AND name='requests'",[],|r|r.get(0)).unwrap();
  let mut q=c.prepare(&format!("EXPLAIN {SCORE_SQL}")).unwrap();
  for row in q.query_map(rusqlite::params![0,Option::<i64>::None],|r|Ok((r.get::<_,String>(1)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?))).unwrap() {
   let(op,page,database)=row.unwrap();
   assert!(!(op=="OpenRead"&&page==table&&database==0),"score status must not open wide requests table");
  }
  let mut q=c.prepare(&format!("EXPLAIN QUERY PLAN {TOTALS_RAW_SQL}")).unwrap();
  let plan=q.query_map([],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap().join("\n");
  assert!(plan.contains("SEARCH m USING COVERING INDEX"),"{plan}");
  assert!(!plan.contains("SCAN m"),"{plan}");
 }
 #[test]
 fn aggregate_refresh_rolls_back_with_dirty_queue_on_failure() {
  let c=crate::db::open_in_memory().unwrap();
  c.execute("INSERT INTO model_invocations VALUES(1,'a')",[]).unwrap();
  assert_eq!(total_calls(&c).unwrap()["a"],1);
  c.execute("UPDATE model_invocations SET model_name='b'",[]).unwrap();
  c.execute_batch("CREATE TEMP TRIGGER reject_aggregate BEFORE INSERT ON model_invocation_canonical BEGIN SELECT RAISE(ABORT,'synthetic refresh error'); END;").unwrap();
  assert!(total_calls(&c).is_err());
  assert_eq!(c.query_row("SELECT total_calls FROM model_invocation_totals WHERE model_name='a'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
  assert_eq!(c.query_row("SELECT COUNT(*) FROM model_invocation_dirty",[],|r|r.get::<_,i64>(0)).unwrap(),1);
  c.execute_batch("DROP TRIGGER reject_aggregate").unwrap();
  assert_eq!(total_calls(&c).unwrap(),HashMap::from([("b".into(),1)]));
 }
 #[test]
 fn one_late_invocation_dirties_only_its_request_among_large_history() {
  let c=crate::db::open_in_memory().unwrap();
  c.execute_batch("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<20000) INSERT INTO model_invocations SELECT i,'model-a' FROM n").unwrap();
  assert_eq!(total_calls(&c).unwrap()["model a"],20000);
  c.execute("INSERT INTO model_invocations VALUES(1,'model-b')",[]).unwrap();
  assert_eq!(c.query_row("SELECT COUNT(*) FROM model_invocation_dirty",[],|r|r.get::<_,i64>(0)).unwrap(),1);
  assert_eq!(total_calls(&c).unwrap(),HashMap::from([("model a".into(),20000),("model b".into(),1)]));
 }
 #[test]
 fn lifetime_totals_refresh_late_changes_and_avoid_history_on_clean_reads() {
  let c=crate::db::open_in_memory().unwrap();
  for (id,name) in [(1,"Model-A"),(1,"model_a free"),(2,"Model-A")] {
   c.execute("INSERT INTO model_invocations VALUES(?1,?2)",rusqlite::params![id,name]).unwrap();
  }
  assert_eq!(total_calls(&c).unwrap()["model a"],2);
  c.execute("INSERT INTO model_invocations VALUES(1,'Model-B')",[]).unwrap();
  assert_eq!(total_calls(&c).unwrap()["model b"],1);
  c.execute("DELETE FROM model_invocations WHERE request_id=1 AND model_name='Model-A'",[]).unwrap();
  assert_eq!(total_calls(&c).unwrap()["model a"],2);
  c.execute("UPDATE model_invocations SET request_id=3,model_name='Model-C' WHERE request_id=1 AND model_name='model_a free'",[]).unwrap();
  let actual=total_calls(&c).unwrap();
  assert_eq!(actual["model a"],1);assert_eq!(actual["model c"],1);
  assert_eq!(c.query_row("SELECT COUNT(*) FROM model_invocation_dirty",[],|r|r.get::<_,i64>(0)).unwrap(),0);
  // A clean read must consult the aggregate rather than prepare any raw-history query.
  c.execute_batch("DROP TABLE model_invocations").unwrap();
  assert_eq!(total_calls(&c).unwrap(),actual);
 }
 #[test]
 #[ignore = "60k lifetime invocation benchmark; run in release mode"]
 fn totals_60k_performance_and_legacy_oracle() {
  let c=crate::db::open_in_memory().unwrap();
  c.execute_batch("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<60000) INSERT INTO model_invocations SELECT (i+1)/2,CASE i%3 WHEN 0 THEN 'Model-'||((i+1)/2%57) WHEN 1 THEN 'model_'||((i+1)/2%57)||' (provider) free' ELSE 'MODEL '||((i+1)/2%57) END FROM n;").unwrap();
  let start=std::time::Instant::now();
  let mut expected=HashMap::new();let mut seen=std::collections::HashSet::new();
  let mut st=c.prepare("SELECT request_id,model_name FROM model_invocations").unwrap();
  for pair in st.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).unwrap() {
   let(id,name)=pair.unwrap();let key=normalize_model_name(&name);
   if seen.insert((id,key.clone())) {*expected.entry(key).or_insert(0)+=1;}
  }
  let old_ms=start.elapsed().as_secs_f64()*1000.0;
  let mut timings=Vec::new();
  for _ in 0..20 {
   let start=std::time::Instant::now();let actual=total_calls(&c).unwrap();
   timings.push(start.elapsed().as_secs_f64()*1000.0);assert_eq!(actual,expected);
  }
  timings.sort_by(f64::total_cmp);let p95=timings[18];
  eprintln!("lifetime 60k invocation rows: legacy={old_ms:.2}ms new_p50={:.2}ms new_p95={p95:.2}ms",timings[9]);
  assert!(p95<300.0,"lifetime totals P95 {p95:.2}ms exceeds 300ms");
 }
 #[test]
 fn lifetime_totals_preserve_canonical_retry_deduplication() {
  let c=crate::db::open_in_memory().unwrap();
  let spellings=["DeepSeek-V4.1-Flash", "deepseek_v4.1_flash (provider) free", "DeepSeek V4.1 Flash", "command-r+", "command-r", "智谱新"];
  for id in 1..=300 {
   for (i,name) in spellings.iter().enumerate() {
    if (id+i)%3!=0 {c.execute("INSERT INTO model_invocations VALUES(?1,?2)",rusqlite::params![id as i64,name]).unwrap();}
   }
  }
  let mut expected=HashMap::new();
  let mut seen=std::collections::HashSet::new();
  let mut st=c.prepare("SELECT request_id,model_name FROM model_invocations").unwrap();
  for pair in st.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).unwrap() {
   let (id,name)=pair.unwrap();let key=normalize_model_name(&name);
   if seen.insert((id,key.clone())) {*expected.entry(key).or_insert(0)+=1;}
  }
  assert_eq!(total_calls(&c).unwrap(),expected);
  assert_eq!(expected["deepseek v4.1 flash"],300);
  assert!(expected.contains_key("command r+"));assert!(expected.contains_key("command r"));
 }
 #[test]fn totals_deduplicate_and_survive_pruning(){
 let c=crate::db::open_in_memory().unwrap();c.execute("INSERT INTO requests(ts_ms,endpoint,model,status) VALUES(1000,'chat','auto','ok')",[]).unwrap();
 for name in ["a","a","b"]{c.execute("INSERT INTO attempts(request_id,ordinal,channel_id,outcome,model_name) VALUES(1,0,3,'success',?1)",[name]).unwrap();}
 assert_eq!(total_calls(&c).unwrap()["a"],1);assert_eq!(total_calls(&c).unwrap()["b"],1);
 assert_eq!(all(&c,60*60*1000).unwrap()["a"].samples,2);assert!(all(&c,3*60*60*1000).unwrap().is_empty());
 crate::db::requests::prune_before(&c,2000).unwrap();assert_eq!(total_calls(&c).unwrap()["a"],1);
 }
 #[test]fn same_channel_different_models_have_independent_scores(){
  let c=crate::db::open_in_memory().unwrap();
  c.execute("INSERT INTO requests(ts_ms,endpoint,model,status) VALUES(1000,'chat','auto','ok')",[]).unwrap();
  for (name,outcome,dur) in [("a","channel_fault",10),("b","success",100)] {
   c.execute("INSERT INTO attempts(request_id,ordinal,channel_id,outcome,duration_ms,model_name) VALUES(1,0,3,?1,?2,?3)",rusqlite::params![outcome,dur,name]).unwrap();
  }
  let v=all(&c,1001).unwrap();assert!(v["b"].value()>v["a"].value());assert_eq!(v["a"].samples,1);assert_eq!(v["b"].samples,1);
 }
#[test]fn ambiguous_and_canceled_attempts_never_score(){
let c=crate::db::open_in_memory().unwrap();
c.execute("INSERT INTO requests(ts_ms,endpoint,model,status) VALUES(1000,'chat','auto','canceled')",[]).unwrap();
c.execute("INSERT INTO attempts(request_id,ordinal,channel_id,outcome,model_name) VALUES(1,0,3,'success','a')",[]).unwrap();
c.execute("INSERT INTO requests(ts_ms,endpoint,model,status) VALUES(1000,'chat','auto','ok')",[]).unwrap();
c.execute("INSERT INTO attempts(request_id,ordinal,channel_id,outcome) VALUES(2,0,3,'success')",[]).unwrap();
c.execute("INSERT INTO attempts(request_id,ordinal,channel_id,outcome,model_name) VALUES(2,1,3,'client_fault','b')",[]).unwrap();
assert!(all(&c,1001).unwrap().is_empty());
}
}
