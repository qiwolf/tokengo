//! Operational scores from attributable real attempts; old ambiguous rows are excluded.
use std::collections::HashMap;
use rusqlite::Connection;
use serde::Serialize;
use super::Result;
use tokengo_shared::normalize_model_name;
pub const WINDOW_MS:i64=15*60*1000;
#[derive(Debug,Clone,Default,Serialize)]
pub struct Score {
 pub samples:i64,pub successes:i64,pub faults:i64,
 pub avg_first_byte_ms:Option<i64>,
 pub avg_ms:Option<i64>,pub score:Option<f64>,pub last_at:Option<i64>,pub last_error:Option<String>,
}
impl Score {
 pub fn value(&self)->f64 {self.score.unwrap_or(-1.0)}
}
pub fn for_channel(conn:&Connection,now:i64,channel:Option<i64>)->Result<HashMap<String,Score>> {
 let mut st=conn.prepare("SELECT a.model_name,a.outcome,a.duration_ms,r.ts_ms,a.error,a.first_byte_ms FROM attempts a JOIN requests r ON r.id=a.request_id WHERE r.ts_ms>=?1 AND a.model_name IS NOT NULL AND a.model_name<>'' AND a.outcome IN ('success','channel_fault','empty_reply') AND r.status<>'canceled' AND (?2 IS NULL OR a.channel_id=?2) ORDER BY a.id")?;
 let mut out:HashMap<String,Score>=HashMap::new();let mut totals:HashMap<String,i64>=HashMap::new();
 let mut firsts:HashMap<String,(i64,i64)>=HashMap::new();
 let mut rows=st.query(rusqlite::params![now-WINDOW_MS,channel])?;
 while let Some(r)=rows.next()? {
  let key=normalize_model_name(&r.get::<_,String>(0)?);let v=out.entry(key.clone()).or_default();
  if let Some(ms)=r.get::<_,Option<i64>>(5)? {let e=firsts.entry(key.clone()).or_default();e.0+=ms;e.1+=1;}
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
#[cfg(test)]mod tests{
 use super::*;
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
