//! Source-authenticated model limit discovery never rewrites manual configuration or pricing.
use axum::{body::{Body,to_bytes},extract::State,http::{HeaderMap,Request,StatusCode},routing::{get,post},Json,Router};
use serde_json::{json,Value};
use std::sync::{Arc,Mutex,atomic::{AtomicBool,AtomicUsize,Ordering}};
use tokio::sync::Notify;
use tokengo_server::{app,db,protocol_config,AppState,Config,SharedState};
use tokengo_shared::ChannelId;
use tower::ServiceExt;
use rusqlite::OptionalExtension;
struct Catalog { body:Mutex<Value>, auth:Mutex<Vec<String>>,block:AtomicBool,entered:Notify,resume:Notify,inference_calls:AtomicUsize }
async fn models(State(c):State<Arc<Catalog>>,headers:HeaderMap)->Json<Value>{c.auth.lock().unwrap().push(headers.get("authorization").and_then(|v|v.to_str().ok()).unwrap_or("").into());if c.block.load(Ordering::SeqCst){c.entered.notify_one();c.resume.notified().await;}Json(c.body.lock().unwrap().clone())}
async fn completion(State(c):State<Arc<Catalog>>,Json(body):Json<Value>)->Json<Value>{
 c.inference_calls.fetch_add(1,Ordering::SeqCst);Json(json!({"id":"synthetic-limit-call","object":"chat.completion","model":body["model"],"choices":[{"index":0,"message":{"role":"assistant","content":"synthetic reply"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}))
}
struct Fixture {state:SharedState,admin:String,key:String,channel:ChannelId,dir:std::path::PathBuf,catalog:Arc<Catalog>,server:tokio::task::JoinHandle<()>}
impl Drop for Fixture {fn drop(&mut self){self.server.abort();let _=std::fs::remove_dir_all(&self.dir);}}
impl Fixture {
 async fn new(body:Value)->Self {
  let catalog=Arc::new(Catalog{body:Mutex::new(body),auth:Mutex::new(vec![]),block:AtomicBool::new(false),entered:Notify::new(),resume:Notify::new(),inference_calls:AtomicUsize::new(0)});let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let base=format!("http://{}/v1",listener.local_addr().unwrap());let router=Router::new().route("/v1/models",get(models)).route("/v1/chat/completions",post(completion)).with_state(catalog.clone());let server=tokio::spawn(async move{axum::serve(listener,router).await.unwrap()});
  let dir=std::env::temp_dir().join(format!("limits-discovery-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into_owned(),..Default::default()}).unwrap();
  let(admin,key,channel)={let c=state.conn();let admin=db::sessions::create_session(&c,None).unwrap();let(_,key)=db::identity::create_key(&c,1,"limits-reader",&Default::default(),None,None).unwrap();let channel=db::channels::create(&c,&db::channels::NewChannel{name:"limits-source".into(),base_url:base,enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();db::channels::set_secret(&c,&state.secrets,channel,"synthetic-source-key").unwrap();db::channels::set_models(&c,channel,&[db::channels::ModelMapping::new("logical",Some("native-model".into()),false)]).unwrap();db::kinds::set(&c,"logical",tokengo_shared::ModelKind::Chat).unwrap();(admin,key,channel)};
  Self{state,admin,key,channel,dir,catalog,server}
 }
 async fn call(&self,method:&str,path:&str,token:&str,body:Value)->(StatusCode,Value){let r=app(self.state.clone()).oneshot(Request::builder().method(method).uri(path).header("content-type","application/json").header("authorization",format!("Bearer {token}")).body(Body::from(body.to_string())).unwrap()).await.unwrap();let s=r.status();let b=to_bytes(r.into_body(),2<<20).await.unwrap();(s,serde_json::from_slice(&b).unwrap())}
 async fn discover(&self)->(StatusCode,Value){self.call("POST",&format!("/api/channels/{}/limits/discover",self.channel.0),&self.admin,json!({"model":"logical"})).await}
 async fn option(&self)->Value{let(s,v)=self.call("GET","/api/pricing/options",&self.admin,Value::Null).await;assert_eq!(s,StatusCode::OK);v["options"].as_array().unwrap().iter().find(|v|v["channel_id"]==self.channel.0&&v["model"]=="logical").unwrap().clone()}
}
#[tokio::test]
async fn authenticated_discovery_fills_missing_limits_but_never_overrides_admin_parameters(){
 let f=Fixture::new(json!({"data":[{"id":"unrelated","limit":{"context":9999999,"output":9999999}},{"id":"native-model","limit":{"context":1000000,"output":16000}}]})).await;
 let before=db::settings::get(&f.state.conn(),&format!("channel_protocols:{}",f.channel.0)).unwrap();let(s,v)=f.discover().await;assert_eq!(s,StatusCode::OK,"{v}");assert_eq!(v["limits"]["max_input_tokens"],1000000);assert_eq!(v["limits"]["max_output_tokens"],16000);assert_eq!(v["status"],"supplier_declared");assert_eq!(v["persisted"],true);assert!(v["source_url"].as_str().unwrap().ends_with("/v1/models"));assert!(v["discovered_at_ms"].as_i64().unwrap()>0);assert_eq!(f.catalog.auth.lock().unwrap().as_slice(),&["Bearer synthetic-source-key".to_owned()]);
 let option=f.option().await;assert_eq!(option["max_input_tokens"],1000000);assert_eq!(option["max_output_tokens"],16000);assert_eq!(option["input_bound_source"],"source_model_discovery");assert_eq!(option["limits_discovery"]["source_url"],v["source_url"]);assert_eq!(option["limits_discovery"]["discovered_at_ms"],v["discovered_at_ms"]);assert_eq!(option["limits_discovery"]["status"],"supplier_declared");assert!(!option["limits_discovery"].to_string().contains("synthetic-source-key"));
 assert_eq!(db::settings::get(&f.state.conn(),&format!("channel_protocols:{}",f.channel.0)).unwrap(),before);assert_eq!(db::billing::list_prices(&f.state.conn()).unwrap().len(),0);
 protocol_config::save(&f.state.conn(),f.channel.0,&protocol_config::Capabilities{max_input_tokens:Some(500000),max_output_tokens:Some(64000),..Default::default()}).unwrap();let option=f.option().await;assert_eq!(option["max_input_tokens"],500000);assert_eq!(option["max_output_tokens"],64000);assert_eq!(option["input_bound_source"],"source_protocols");
 let(s,_)=f.discover().await;assert_eq!(s,StatusCode::OK);let option=f.option().await;assert_eq!(option["max_output_tokens"],64000,"fresh discoveries must not overwrite administrator limits");
}
#[tokio::test]
async fn unknown_values_are_not_guessed_and_discovery_is_root_only(){
 let f=Fixture::new(json!({"data":[{"id":"native-model","context_window":0,"max_output_tokens":-1,"description":"1M context, 64k output"}]})).await;
 let(s,v)=f.discover().await;assert_eq!(s,StatusCode::OK,"{v}");assert!(v["limits"]["max_input_tokens"].is_null());assert!(v["limits"]["max_output_tokens"].is_null());assert_eq!(v["status"],"not_declared");let option=f.option().await;assert!(option["max_input_tokens"].is_null());assert!(option["max_output_tokens"].is_null());
 let path=format!("/api/channels/{}/limits/discover",f.channel.0);let(s,_)=f.call("POST",&path,&f.key,json!({"model":"logical"})).await;assert_eq!(s,StatusCode::UNAUTHORIZED);let(s,_)=f.call("POST",&path,&f.admin,json!({"model":"unmapped"})).await;assert_eq!(s,StatusCode::BAD_REQUEST);assert_eq!(f.catalog.auth.lock().unwrap().len(),1);
}

#[tokio::test]
async fn source_key_changed_during_discovery_rejects_stale_result_without_holding_db_lock(){
 let f=Fixture::new(json!({"data":[{"id":"native-model","limit":{"context":1000000,"output":16000}}]})).await;
 // Preserve an existing declaration so a stale network result cannot overwrite it.
 let(s,v)=f.discover().await;assert_eq!(s,StatusCode::OK,"{v}");
 let before:Vec<(String,String)>={let c=f.state.conn();let mut q=c.prepare("SELECT key,value FROM settings WHERE key LIKE 'model_limits_discovery_v1:%'").unwrap();let rows=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();rows};assert_eq!(before.len(),1);
 *f.catalog.body.lock().unwrap()=json!({"data":[{"id":"native-model","limit":{"context":2000000,"output":32000}}]});f.catalog.block.store(true,Ordering::SeqCst);
 let state=f.state.clone();let request=Request::builder().method("POST").uri(format!("/api/channels/{}/limits/discover",f.channel.0)).header("authorization",format!("Bearer {}",f.admin)).header("content-type","application/json").body(Body::from(json!({"model":"logical"}).to_string())).unwrap();
 let task=tokio::spawn(async move{app(state).oneshot(request).await.unwrap()});tokio::time::timeout(std::time::Duration::from_secs(3),f.catalog.entered.notified()).await.expect("catalog request must start");
 let state=f.state.clone();let channel=f.channel;let change=tokio::task::spawn_blocking(move||db::channels::set_secret(&state.conn(),&state.secrets,channel,"replacement-source-key"));
 let changed=tokio::time::timeout(std::time::Duration::from_secs(2),change).await;
 f.catalog.resume.notify_one();
 changed.expect("discovery must not hold the database lock while waiting for the catalog").unwrap().unwrap();
 let response=task.await.unwrap();assert_eq!(response.status(),StatusCode::CONFLICT);let raw=to_bytes(response.into_body(),1<<20).await.unwrap();let v:Value=serde_json::from_slice(&raw).unwrap();assert_eq!(v["error"]["type"],"source_changed");
 let after:Vec<(String,String)>={let c=f.state.conn();let mut q=c.prepare("SELECT key,value FROM settings WHERE key LIKE 'model_limits_discovery_v1:%'").unwrap();let rows=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();rows};assert_eq!(after,before,"stale catalog result must not replace the prior discovery");
 let option=f.option().await;assert!(option["max_input_tokens"].is_null(),"the old key's cache must be invalid for the replacement key");assert!(option["max_output_tokens"].is_null());
}

#[tokio::test]
async fn failed_discovery_audit_rolls_back_the_cache_replacement(){
 let f=Fixture::new(json!({"data":[{"id":"native-model","limit":{"context":1000000,"output":16000}}]})).await;
 let(s,v)=f.discover().await;assert_eq!(s,StatusCode::OK,"{v}");
 let before:Vec<(String,String)>={let c=f.state.conn();let mut q=c.prepare("SELECT key,value FROM settings WHERE key LIKE 'model_limits_discovery_v1:%'").unwrap();let rows=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();rows};
 *f.catalog.body.lock().unwrap()=json!({"data":[{"id":"native-model","limit":{"context":2000000,"output":32000}}]});
 f.state.conn().execute_batch("CREATE TRIGGER reject_limits_audit BEFORE INSERT ON audit_events WHEN NEW.action='source.limits_discovered' BEGIN SELECT RAISE(ABORT,'synthetic audit write failure'); END").unwrap();
 let(s,_)=f.discover().await;assert_eq!(s,StatusCode::INTERNAL_SERVER_ERROR);
 let after:Vec<(String,String)>={let c=f.state.conn();let mut q=c.prepare("SELECT key,value FROM settings WHERE key LIKE 'model_limits_discovery_v1:%'").unwrap();let rows=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();rows};assert_eq!(after,before,"failed audit must roll back the paired cache write");
 let option=f.option().await;assert_eq!(option["max_input_tokens"],1000000);assert_eq!(option["max_output_tokens"],16000);
}

async fn queued_discovery_input_window_growth(tpm:Option<u64>){
 let f=Fixture::new(json!({"data":[{"id":"native-model","limit":{"context":1000,"output":1024}}]})).await;
 f.state.routing().cfg.probe_enabled=false;f.state.routing().cfg.retry_times=0;
 let (key_id,key)={let c=f.state.conn();let row=db::channels::get(&c,f.channel).unwrap().unwrap();db::channels::update(&c,f.channel,&row.name,&row.base_url,true,false,1).unwrap();db::provider_pool::backfill(&c,&f.state.secrets).unwrap();let(k,key)=db::identity::create_key(&c,1,"tpm-source-context",&db::identity::Policy{tpm,..Default::default()},None,None).unwrap();db::billing::create_price(&c,&db::billing::Price{currency:"CNY".into(),id:0,channel_id:f.channel.0,model:"logical".into(),input_micro_per_million:1000000,output_micro_per_million:1000000,max_input_tokens:50000,max_output_tokens:5000},"fixture").unwrap();(k.id,key)};
 let(s,v)=f.discover().await;assert_eq!(s,StatusCode::OK,"{v}");
 let gate=f.state.channel_gate(f.channel.0,1).unwrap();let held=gate.try_acquire_owned().unwrap();
 let body=json!({"model":"logical","messages":[{"role":"user","content":"x".repeat(3000)}],"max_tokens":64});let state=f.state.clone();let request=Request::builder().method("POST").uri("/v1/chat/completions").header("authorization",format!("Bearer {key}")).header("content-type","application/json").body(Body::from(body.to_string())).unwrap();let task=tokio::spawn(async move{app(state).oneshot(request).await.unwrap()});
 let admitted=tokio::time::timeout(std::time::Duration::from_secs(5),async {loop {let tokens:Option<i64>=f.state.conn().query_row("SELECT tokens FROM quota_events WHERE scope='key' AND scope_id=?1 ORDER BY at_ms DESC LIMIT 1",[key_id],|r|r.get(0)).optional().unwrap();if let Some(tokens)=tokens{break tokens;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}}).await.expect("request must enter admission before source queue");
 assert!(admitted<(body.to_string().len() as i64),"old source window must clip the conservative input reservation");
 *f.catalog.body.lock().unwrap()=json!({"data":[{"id":"native-model","limit":{"context":10000,"output":1024}}]});let(s,v)=f.discover().await;assert_eq!(s,StatusCode::OK,"{v}");drop(held);
 let response=tokio::time::timeout(std::time::Duration::from_secs(5),task).await.unwrap().unwrap();
 let status=response.status();let raw=to_bytes(response.into_body(),1<<20).await.unwrap();let v:Value=serde_json::from_slice(&raw).unwrap();
 if tpm.is_some(){
  assert_eq!(status,StatusCode::BAD_REQUEST,"{v}");assert_eq!(v["error"]["type"],"source_context_changed","{v}");assert_eq!(f.catalog.inference_calls.load(Ordering::SeqCst),0,"the larger execution reservation must not bypass the admitted TPM hold");
  let recorded:i64=f.state.conn().query_row("SELECT tokens FROM quota_events WHERE scope='key' AND scope_id=?1 ORDER BY at_ms DESC LIMIT 1",[key_id],|r|r.get(0)).unwrap();assert_eq!(recorded,admitted);
 }else{
  assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(f.catalog.inference_calls.load(Ordering::SeqCst),1,"without a TPM limit a larger current source window must remain usable");
 }
}
#[tokio::test]
async fn queued_discovery_input_window_growth_cannot_exceed_the_admitted_tpm_reservation(){queued_discovery_input_window_growth(Some(20000)).await;}
#[tokio::test]
async fn queued_discovery_input_window_growth_without_tpm_forwards_normally(){queued_discovery_input_window_growth(None).await;}
