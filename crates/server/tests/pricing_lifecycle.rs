//! Price lifecycle changes never rewrite historical financial snapshots.
use axum::{body::{Body,to_bytes},http::{Request,StatusCode}};
use serde_json::{json,Value};
use tokengo_server::{app,db,AppState,Config,SharedState};
use tower::ServiceExt;
struct Fixture { state:SharedState, admin:String, key_id:i64, key:String, dir:std::path::PathBuf }
impl Fixture {
 fn new()->Self {let dir=std::env::temp_dir().join(format!("price-lifecycle-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into_owned(),..Default::default()}).unwrap();let admin=db::sessions::create_session(&state.conn(),None).unwrap();let(k,key)=db::identity::create_key(&state.conn(),1,"lifecycle",&Default::default(),None,None).unwrap();Self{state,admin,key_id:k.id,key,dir}}
 async fn call(&self,method:&str,path:&str,auth:&str,body:Value)->(StatusCode,Value){let response=app(self.state.clone()).oneshot(Request::builder().method(method).uri(path).header("content-type","application/json").header("authorization",format!("Bearer {auth}")).body(Body::from(body.to_string())).unwrap()).await.unwrap();let status=response.status();let bytes=to_bytes(response.into_body(),1<<20).await.unwrap();(status,serde_json::from_slice(&bytes).unwrap_or(Value::Null))}
}
impl Drop for Fixture {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.dir);}}
fn spec(kind:&str)->(&'static str,&'static str,&'static str,Value){match kind {
 "token"=>("/api/pricing","price","prices",json!({"channel_id":1,"model":"lifecycle-token","currency":"CNY","input_micro_per_million":1000000,"output_micro_per_million":2000000,"max_input_tokens":10000,"max_output_tokens":1024})),
 "unit"=>("/api/unit-pricing","unit_price","unit_prices",json!({"channel_id":1,"model":"lifecycle-unit","currency":"CNY","unit":"second","micro_per_unit":1000,"max_quantity_milli":10000})),
 _=>("/api/request-pricing","request_price","request_prices",json!({"channel_id":1,"model":"lifecycle-request","currency":"CNY","micro_per_call":1000})),}}
fn selected(state:&SharedState,kind:&str,model:&str)->Option<i64>{let c=state.conn();match kind {"token"=>db::billing::price(&c,model,1).unwrap().map(|p|p.id),"unit"=>db::billing::unit_price(&c,model,1).unwrap().map(|p|p.id),_=>db::billing::request_price(&c,model,1).unwrap().map(|p|p.id)}}
fn reserve_version(f:&Fixture,kind:&str,id:i64,attempt:&str)->db::Result<i64> {
 let c=f.state.conn();match kind {
  "token"=>{let p=db::billing::list_prices(&c)?.into_iter().find(|p|p.id==id).unwrap();db::billing::reserve(&c,1,f.key_id,2,attempt,&p,100,10)},
  "unit"=>{let p=db::billing::list_unit_prices(&c)?.into_iter().find(|p|p.id==id).unwrap();db::billing::reserve_units(&c,1,f.key_id,2,attempt,&p,1000)},
  _=>{let p=db::billing::list_request_prices(&c)?.into_iter().find(|p|p.id==id).unwrap();db::billing::reserve_request(&c,1,f.key_id,2,attempt,&p)},
 }
}
#[tokio::test]
async fn edit_disable_restore_and_archive_all_price_kinds_preserve_reserved_snapshots(){
 let f=Fixture::new();
 for kind in ["token","unit","request"] {
  let(path,item,array,mut payload)=spec(kind);let model=payload["model"].as_str().unwrap().to_owned();let(status,first)=f.call("POST",path,&f.admin,payload.clone()).await;assert_eq!(status,StatusCode::OK,"{first}");let first_id=first[item]["id"].as_i64().unwrap();
  let ledger={let c=f.state.conn();match kind {"token"=>{let p=db::billing::price(&c,&model,1).unwrap().unwrap();db::billing::reserve(&c,1,f.key_id,1,&format!("pending-{kind}"),&p,100,10).unwrap()},"unit"=>{let p=db::billing::unit_price(&c,&model,1).unwrap().unwrap();db::billing::reserve_units(&c,1,f.key_id,1,&format!("pending-{kind}"),&p,1000).unwrap()},_=>{let p=db::billing::request_price(&c,&model,1).unwrap().unwrap();db::billing::reserve_request(&c,1,f.key_id,1,&format!("pending-{kind}"),&p).unwrap()}}};
  let amount_field=match kind{"token"=>"input_micro_per_million","unit"=>"micro_per_unit",_=>"micro_per_call"};payload[amount_field]=json!(9000000);
  let(status,edited)=f.call("PUT",&format!("{path}/{first_id}"),&f.admin,payload.clone()).await;assert_eq!(status,StatusCode::OK,"{kind}: {edited}");let next_id=edited[item]["id"].as_i64().unwrap();assert_ne!(first_id,next_id);assert_eq!(edited[item]["enabled"],true);assert_eq!(edited[item]["is_latest"],true);assert_eq!(selected(&f.state,kind,&model),Some(next_id));
  let existing_attempt=format!("latest-held-{kind}");
  let existing_hold=reserve_version(&f,kind,next_id,&existing_attempt).unwrap();
  let(status,v)=f.call("PATCH",&format!("{path}/{next_id}"),&f.admin,json!({"enabled":false})).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v[item]["enabled"],false);assert_eq!(selected(&f.state,kind,&model),None,"a disabled latest price must not fall back to the older price");
  assert_eq!(reserve_version(&f,kind,next_id,&existing_attempt).unwrap(),existing_hold,"an already reserved attempt remains idempotent after disable");
  assert!(reserve_version(&f,kind,next_id,&format!("disabled-new-attempt-{kind}")).is_err(),"a disabled immutable price object must not fund a new attempt");
  db::billing::release(&f.state.conn(),existing_hold,"not_sent").unwrap();
  let(status,v)=f.call("GET",path,&f.admin,Value::Null).await;assert_eq!(status,StatusCode::OK);let rows=v[array].as_array().unwrap();assert_eq!(rows.len(),2);assert!(rows.iter().any(|r|r["id"]==next_id&&r["enabled"]==false&&r["effective"]==false));
  let(status,v)=f.call("PATCH",&format!("{path}/{next_id}"),&f.admin,json!({"enabled":true})).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(selected(&f.state,kind,&model),Some(next_id));
  assert_eq!(f.call("DELETE",&format!("{path}/{first_id}"),&f.admin,Value::Null).await.0,StatusCode::OK);
  {let c=f.state.conn();match kind{"token"=>db::billing::settle(&c,ledger,Some(3),Some(2),"ok").unwrap(),"unit"=>db::billing::settle_units(&c,ledger,Some(500),"ok").unwrap(),_=>db::billing::settle_request(&c,ledger,Some(1),"ok").unwrap()};let row=c.query_row("SELECT debit_micro,status FROM billing_ledger WHERE id=?1",[ledger],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).unwrap();assert_eq!(row,(match kind{"token"=>7,"unit"=>500,_=>1000},"ok".into()),"pending hold must settle from its original immutable price");}
  assert_eq!(f.call("DELETE",&format!("{path}/{next_id}"),&f.admin,Value::Null).await.0,StatusCode::OK);assert_eq!(selected(&f.state,kind,&model),None);
  let(_,v)=f.call("GET",path,&f.admin,Value::Null).await;assert!(v[array].as_array().unwrap().is_empty());let(_,v)=f.call("GET",&format!("{path}?include_deleted=true"),&f.admin,Value::Null).await;assert_eq!(v[array].as_array().unwrap().len(),2);assert!(v[array].as_array().unwrap().iter().all(|p|p["deleted"]==true));
  let(status,_)=f.call("PATCH",&format!("{path}/{next_id}"),&f.admin,json!({"enabled":true})).await;assert_eq!(status,StatusCode::CONFLICT,"deleted prices must not be silently reactivated");
 }
}
#[tokio::test]
async fn lifecycle_mutations_require_admin_and_current_immutable_identity(){
 let f=Fixture::new();let(path,item,_,mut p)=spec("token");let(_,v)=f.call("POST",path,&f.admin,p.clone()).await;let id=v[item]["id"].as_i64().unwrap();
 for method in ["PUT","PATCH","DELETE"] {let body=if method=="PUT"{p.clone()}else{json!({"enabled":false})};assert_eq!(f.call(method,&format!("{path}/{id}"),&f.key,body).await.0,StatusCode::FORBIDDEN);}
 p["model"]=json!("different identity");assert_eq!(f.call("PUT",&format!("{path}/{id}"),&f.admin,p.clone()).await.0,StatusCode::BAD_REQUEST);p["model"]=json!("lifecycle-token");assert_eq!(f.call("PUT",&format!("{path}/{id}"),&f.admin,p.clone()).await.0,StatusCode::OK);assert_eq!(f.call("PUT",&format!("{path}/{id}"),&f.admin,p).await.0,StatusCode::CONFLICT);assert_eq!(f.call("PATCH",&format!("{path}/{id}"),&f.admin,json!({"enabled":false})).await.0,StatusCode::CONFLICT);
}
