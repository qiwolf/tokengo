//! Auto strategies, persistent order and generation-safe candidate cooldowns.
use rusqlite::{Connection,params,OptionalExtension};
use serde::{Serialize,Deserialize};
use tokengo_shared::normalize_model_name as norm;
use super::{Result,settings,model_routes,scores};
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct Policy { pub strategy:String,pub explore:bool }
impl Default for Policy {fn default()->Self{Self{strategy:"sequence".into(),explore:false}}}
pub fn get(c:&Connection)->Result<Policy>{Ok(settings::get(c,"auto_policy")?.and_then(|v|serde_json::from_str(&v).ok()).unwrap_or_default())}
pub fn save(c:&Connection,p:&Policy)->Result<()>{if !["sequence","balanced","fastest","stable","idle"].contains(&p.strategy.as_str()){return Err(super::DbError::Other("不支持的路由策略".into()));}settings::set(c,"auto_policy",&serde_json::to_string(p).unwrap())}
pub fn ordered(c:&Connection)->Result<Vec<String>>{
 let routes=model_routes::list(c)?;let mut names:Vec<_>=routes.into_iter().map(|r|r.name).collect();
 let saved:Vec<String>=settings::get(c,"auto_order")?.and_then(|s|serde_json::from_str(&s).ok()).unwrap_or_default();
 names.sort_by_key(|n|saved.iter().position(|s|norm(s)==norm(n)).unwrap_or(usize::MAX));Ok(names)
}
pub fn sync_order(c:&Connection)->Result<()>{let names=ordered(c)?;settings::set(c,"auto_order",&serde_json::to_string(&names).unwrap())}
pub fn move_model(c:&Connection,name:&str,up:bool)->Result<()>{let mut list=ordered(c)?;let Some(i)=list.iter().position(|n|norm(n)==norm(name)) else{return Err(super::DbError::Other("模型未勾选".into()));};let j=if up{i.saturating_sub(1)}else{(i+1).min(list.len()-1)};list.swap(i,j);settings::set(c,"auto_order",&serde_json::to_string(&list).unwrap())}
/// Names are already capability-filtered; time and random inputs are injected for testing.
pub fn rank(c:&Connection,names:&mut Vec<String>,p:&Policy,seq:u64,draw:u64)->Result<Option<String>>{
 let order=ordered(c)?;names.sort_by_key(|n|order.iter().position(|x|norm(x)==norm(n)).unwrap_or(usize::MAX));
 let stats=scores::all(c,super::now_ms())?;
 match p.strategy.as_str(){
 "balanced"=>{if !names.is_empty(){let n=names.len();names.rotate_left(seq as usize%n);}},
 "fastest"=>names.sort_by_key(|n|{let s=stats.get(&norm(n));(s.map(|s|s.samples<5).unwrap_or(true),s.and_then(|s|s.avg_first_byte_ms.or(s.avg_ms)).unwrap_or(i64::MAX))}),
 "stable"=>names.sort_by(|a,b|{let confidence=|n:&String|stats.get(&norm(n)).map(|s|s.successes as f64/(s.samples+5) as f64).unwrap_or(-1.0);confidence(b).total_cmp(&confidence(a)).then_with(||stats.get(&norm(a)).and_then(|s|s.avg_ms).unwrap_or(i64::MAX).cmp(&stats.get(&norm(b)).and_then(|s|s.avg_ms).unwrap_or(i64::MAX)))}),_=>{}
 }
 let mut explored=None;
 if p.explore && draw%100<5 && names.len()>1 {
  // Lifetime attribution determines unused; a window expiry is not 'never used'.
  let unused:Vec<String>=names.iter().filter(|n|c.query_row("SELECT COUNT(*) FROM attempts WHERE model_name=?1",[n.as_str()],|r|r.get::<_,i64>(0)).unwrap_or(0)==0).cloned().collect();
  let pool=if unused.is_empty(){let mut low=names.clone();low.sort_by(|a,b|stats.get(&norm(a)).map(|s|s.value()).unwrap_or(-1.0).total_cmp(&stats.get(&norm(b)).map(|s|s.value()).unwrap_or(-1.0)));low.truncate((low.len()+1)/2);low}else{unused};
  if !pool.is_empty(){let pick=pool[(draw/100) as usize%pool.len()].clone();if let Some(i)=names.iter().position(|n|n==&pick){let n=names.remove(i);names.insert(0,n);explored=Some(pick);}}
 }
 Ok(explored)
}
#[derive(Debug,Clone,Serialize)]
pub struct Cooldown {pub channel_id:i64,pub model:String,pub upstream:String,pub failures:i64,pub until_ms:i64,pub reason:String,pub generation:i64,pub lease_until:i64}
pub fn rows(c:&Connection,name:&str)->Result<Vec<Cooldown>>{
 let mut st=c.prepare("SELECT channel_id,model_name,upstream_model,failures,until_ms,reason,generation,lease_until FROM model_cooldowns WHERE model_name=?1")?;
 let r=st.query_map([norm(name)],|r|Ok(Cooldown{channel_id:r.get(0)?,model:r.get(1)?,upstream:r.get(2)?,failures:r.get(3)?,until_ms:r.get(4)?,reason:r.get(5)?,generation:r.get(6)?,lease_until:r.get(7)?}))?.collect::<rusqlite::Result<_>>()?;Ok(r)
}
pub fn delay(n:i64)->i64{match n{0|1=>15000,2=>60000,3=>300000,4=>900000,_=>1800000}}
pub fn peek(c:&Connection,id:i64,name:&str,upstream:&str,now:i64)->Result<Option<i64>>{
 Ok(c.query_row("SELECT MAX(until_ms,lease_until) FROM model_cooldowns WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3",params![id,norm(name),upstream],|r|r.get::<_,i64>(0)).optional()?.filter(|t|*t>now))
}
/// Called under the connection mutex. A failed candidate gets a single half-open lease.
pub fn claim(c:&Connection,id:i64,name:&str,upstream:&str,now:i64)->Result<Option<i64>>{
 c.execute("INSERT OR IGNORE INTO model_cooldowns(channel_id,model_name,upstream_model) VALUES(?1,?2,?3)",params![id,norm(name),upstream])?;
 let row:(i64,i64,i64,i64)=c.query_row("SELECT failures,until_ms,lease_until,generation FROM model_cooldowns WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3",params![id,norm(name),upstream],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
 if row.1>now||row.2>now{return Ok(None);}
 if row.1>0 {c.execute("UPDATE model_cooldowns SET lease_until=?4 WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3",params![id,norm(name),upstream,now+660000])?;}
 Ok(Some(row.3))
}
pub fn complete(c:&Connection,id:i64,name:&str,upstream:&str,generation:i64,success:bool,reason:&str,retry_ms:i64,now:i64)->Result<()> {
 if success {c.execute("UPDATE model_cooldowns SET failures=0,until_ms=0,lease_until=0,reason='' WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3 AND generation=?4",params![id,norm(name),upstream,generation])?;}else{
 let n:Option<i64>=c.query_row("SELECT failures+1 FROM model_cooldowns WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3 AND generation=?4",params![id,norm(name),upstream,generation],|r|r.get(0)).optional()?;
 let recovering:bool=c.query_row("SELECT lease_until>0 FROM model_cooldowns WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3",params![id,norm(name),upstream],|r|r.get(0))?;
 if let Some(n)=n{c.execute("UPDATE model_cooldowns SET failures=?5,until_ms=?6,lease_until=0,reason=?7 WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3 AND generation=?4",params![id,norm(name),upstream,generation,n,if n>=3 || recovering {now+delay((n-2).max(1)).max(retry_ms)} else {0},reason])?;}
 }Ok(())
}
pub fn release(c:&Connection,id:i64,name:&str,upstream:&str,generation:i64)->Result<()>{c.execute("UPDATE model_cooldowns SET lease_until=0 WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3 AND generation=?4",params![id,norm(name),upstream,generation])?;Ok(())}
pub fn reset(c:&Connection,name:&str,id:Option<i64>)->Result<()>{c.execute("UPDATE model_cooldowns SET failures=0,until_ms=0,lease_until=0,reason='',generation=generation+1 WHERE model_name=?1 AND (?2 IS NULL OR channel_id=?2)",params![norm(name),id])?;Ok(())}
#[cfg(test)]mod ranking_tests {
 use super::*;
 fn fixture()->Connection{let c=crate::db::open_in_memory().unwrap();c.execute("INSERT INTO channels(name,base_url,enabled) VALUES('x','http://x',1)",[]).unwrap();for n in ["a","b","c"]{c.execute("INSERT INTO channel_models(channel_id,model_name) VALUES(1,?1)",[n]).unwrap();}c}
 #[test]fn sequence_is_persistent_and_balance_rotates(){let c=fixture();move_model(&c,"b",true).unwrap();let mut n=vec!["c".into(),"a".into(),"b".into()];rank(&c,&mut n,&Policy::default(),10,0).unwrap();assert_eq!(n,vec!["b","a","c"]);let p=Policy{strategy:"balanced".into(),explore:false};rank(&c,&mut n,&p,1,0).unwrap();assert_eq!(n,vec!["a","c","b"]);}
 #[test]fn exploration_is_disabled_by_default_and_only_five_percent_of_draws(){let c=fixture();let mut count=0;for draw in 0..100{let mut n=vec!["a".into(),"b".into()];assert!(rank(&c,&mut n,&Policy::default(),0,draw).unwrap().is_none());let mut p=Policy::default();p.explore=true;if rank(&c,&mut n,&p,0,draw).unwrap().is_some(){count+=1;}}assert_eq!(count,5);}
 #[test]fn cooldown_survives_reopen_and_success_clears_failures(){let c=fixture();let g=claim(&c,1,"a","a",0).unwrap().unwrap();for _ in 0..3 {complete(&c,1,"a","a",g,false,"429",120000,0).unwrap();}assert_eq!(peek(&c,1,"a","a",1).unwrap(),Some(120000));let g=claim(&c,1,"a","a",120001).unwrap().unwrap();complete(&c,1,"a","a",g,true,"",0,120002).unwrap();assert_eq!(rows(&c,"a").unwrap()[0].failures,0);}
}
#[cfg(test)]mod tests{use super::*;#[test]fn gradient_reset_and_single_recovery_lease(){let c=crate::db::open_in_memory().unwrap();let g=claim(&c,1,"a","up",100).unwrap().unwrap();for _ in 0..3 {complete(&c,1,"a","up",g,false,"timeout",0,100).unwrap();}assert_eq!(peek(&c,1,"a","up",100).unwrap(),Some(15100));assert_eq!(claim(&c,1,"a","up",200).unwrap(),None);assert!(claim(&c,1,"a","up",15101).unwrap().is_some());assert!(claim(&c,1,"a","up",15102).unwrap().is_none());reset(&c,"a",None).unwrap();complete(&c,1,"a","up",g,false,"late",0,20000).unwrap();assert_eq!(rows(&c,"a").unwrap()[0].failures,0);assert_eq!([1,2,3,4,5,6].map(delay),[15000,60000,300000,900000,1800000,1800000]);}}

#[cfg(test)]mod threshold_tests {
 use super::*;
 #[test]fn only_third_failure_cools_and_recovery_failure_escalates(){
 let c=crate::db::open_in_memory().unwrap();let g=claim(&c,1,"a","up",100).unwrap().unwrap();
 for n in 1..=2{complete(&c,1,"a","up",g,false,"500",0,100).unwrap();assert_eq!(rows(&c,"a").unwrap()[0].failures,n);assert_eq!(peek(&c,1,"a","up",101).unwrap(),None);}
 complete(&c,1,"a","up",g,false,"500",0,100).unwrap();assert_eq!(peek(&c,1,"a","up",101).unwrap(),Some(15100));
 let g=claim(&c,1,"a","up",15101).unwrap().unwrap();assert!(claim(&c,1,"a","up",15102).unwrap().is_none());
 release(&c,1,"a","up",g).unwrap();assert_eq!(rows(&c,"a").unwrap()[0].failures,3);
 let g=claim(&c,1,"a","up",15103).unwrap().unwrap();complete(&c,1,"a","up",g,false,"500",0,15104).unwrap();assert_eq!(peek(&c,1,"a","up",15105).unwrap(),Some(75104));
 reset(&c,"a",None).unwrap();complete(&c,1,"a","up",g,false,"late",0,15106).unwrap();assert_eq!(rows(&c,"a").unwrap()[0].failures,0);
 }
}
