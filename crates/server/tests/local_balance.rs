//! Explicit local vector balancing exercises real HTTP, authentication, limits and fault recovery.
use axum::{body::Body, extract::State, http::{Request,StatusCode}, response::{IntoResponse,Response}, routing::post, Json, Router};
use serde_json::{json,Value};
use std::sync::{Arc,Mutex,atomic::{AtomicU16,Ordering}};
use tokengo_server::{app,db,AppState,Config,SharedState};
use tokengo_shared::{ChannelId,ModelKind};
use tower::ServiceExt;

#[derive(Default)]
struct NodeState { status:AtomicU16, seen:Mutex<Vec<Value>> }
struct Node { state:Arc<NodeState>,base:String,task:tokio::task::JoinHandle<()> }
impl Drop for Node {fn drop(&mut self){self.task.abort();}}
impl Node {
 async fn new()->Self {
  let state=Arc::new(NodeState::default());state.status.store(200,Ordering::SeqCst);
  let router=Router::new().route("/v1/embeddings",post(reply)).route("/v1/rerank",post(reply)).route("/v1/chat/completions",post(reply)).with_state(state.clone());
  let l=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let base=format!("http://{}/v1",l.local_addr().unwrap());
  let task=tokio::spawn(async move {axum::serve(l,router).await.unwrap()});Self{state,base,task}
 }
 fn count(&self)->usize{self.state.seen.lock().unwrap().len()}
 fn status(&self,status:u16){self.state.status.store(status,Ordering::SeqCst);}
}
async fn reply(State(state):State<Arc<NodeState>>,Json(body):Json<Value>)->Response {
 state.seen.lock().unwrap().push(body.clone());let code=state.status.load(Ordering::SeqCst);
 if code!=200{return(StatusCode::from_u16(code).unwrap(),Json(json!({"error":{"message":"controlled source failure"}}))).into_response();}
 if body.get("input").is_some(){let count=body["input"].as_array().map_or(1,Vec::len);return Json(json!({"data":(0..count).map(|i|json!({"index":i,"embedding":[0.6,0.8]})).collect::<Vec<_>>(),"usage":{"prompt_tokens":3,"total_tokens":3}})).into_response();}
 if let Some(docs)=body["documents"].as_array(){return Json(json!({"results":docs.iter().enumerate().map(|(i,d)|json!({"index":i,"relevance_score":0.9-(i as f64)*0.1,"document":{"text":d}})).collect::<Vec<_>>(),"usage":{"prompt_tokens":3,"total_tokens":3}})).into_response();}
 Json(json!({"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}})).into_response()
}
struct Fixture {state:SharedState,dir:std::path::PathBuf,admin:String,user:i64,key:String,ids:Vec<ChannelId>,provider:i64,nodes:Vec<Node>,kind:ModelKind}
impl Drop for Fixture {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.dir);}}
impl Fixture {
 async fn new(kind:ModelKind)->Self {
  let dir=std::env::temp_dir().join(format!("local-vector-lb-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
  let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Config::default()}).unwrap();
  state.routing().cfg.retry_times=0;state.routing().cfg.probe_enabled=false;
  let (admin,user,key,provider)={let c=state.conn();let admin=db::sessions::create_session(&c,None).unwrap();let user=db::identity::create_user(&c,"vector-caller","user",&Default::default()).unwrap().id;let (_,key)=db::identity::create_key(&c,user,"vector-key",&Default::default(),None,None).unwrap();c.execute("INSERT INTO provider_channels(name,enabled,created_ms,endpoint_mode,canonical_state) VALUES('local-gpu',1,0,'local','canonical')",[]).unwrap();(admin,user,key,c.last_insert_rowid())};
  let mut ids=vec![];let mut nodes=vec![];
  for i in 0..2 {let node=Node::new().await;let c=state.conn();let id=db::channels::create(&c,&db::channels::NewChannel{name:format!("gpu-{i}"),base_url:node.base.clone(),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();db::channels::set_secret(&c,&state.secrets,id,&format!("synthetic-node-{i}")).unwrap();db::channels::set_models(&c,id,&[db::channels::ModelMapping::new("bge",Some("exact-model".into()),false)]).unwrap();db::provider_channels::set_membership(&c,id.0,Some(provider)).unwrap();db::provider_pool::adopt_source(&c,&state.secrets,id.0,provider).unwrap();ids.push(id);nodes.push(node);}
  db::kinds::set(&state.conn(),"bge",kind).unwrap();
  Self{state,dir,admin,user,key,ids,provider,nodes,kind}
 }
 async fn request(&self,method:&str,path:&str,key:&str,body:Value)->(StatusCode,Value) {
  let r=Request::builder().method(method).uri(path).header("authorization",format!("Bearer {key}")).header("content-type","application/json").body(Body::from(body.to_string())).unwrap();
  let r=app(self.state.clone()).oneshot(r).await.unwrap();let status=r.status();let b=axum::body::to_bytes(r.into_body(),1<<20).await.unwrap();(status,serde_json::from_slice(&b).unwrap_or(Value::Null))
 }
 async fn policy(&self,value:Value){let(s,b)=self.request("PUT","/api/model-routing?model=bge",&self.admin,value).await;assert_eq!(s,StatusCode::OK,"{b}");}
 fn body(&self)->Value{match self.kind{ModelKind::Embeddings=>json!({"model":"bge","input":["中文输入","second input"],"dimensions":2}),ModelKind::Rerank=>json!({"model":"bge","query":"query","documents":["first doc","second doc"],"top_n":2,"return_documents":true}),_=>json!({"model":"bge","messages":[{"role":"user","content":"hi"}]})}}
 async fn call(&self)->(StatusCode,Value){let path=match self.kind{ModelKind::Embeddings=>"/v1/embeddings",ModelKind::Rerank=>"/v1/rerank",_=>"/v1/chat/completions"};self.request("POST",path,&self.key,self.body()).await}
 fn no_affinity(&self){assert!(db::provider_pool::affinity_token(&self.state.conn(),self.user,self.provider).unwrap().is_none());}
}
#[tokio::test]
async fn same_user_local_vectors_round_robin_without_affinity_and_keep_full_payload(){
 for kind in [ModelKind::Embeddings,ModelKind::Rerank] {let f=Fixture::new(kind).await;f.policy(json!({"local_strategy":"round_robin","retry_times":0})).await;for _ in 0..6{let(s,b)=f.call().await;assert_eq!(s,StatusCode::OK,"{b}");}assert_eq!((f.nodes[0].count(),f.nodes[1].count()),(3,3));f.no_affinity();for node in &f.nodes{for sent in node.state.seen.lock().unwrap().iter(){let mut expected=f.body();expected["model"]=json!("exact-model");assert_eq!(sent,&expected);}}}
}
#[tokio::test]
async fn http_fault_and_transport_fault_reach_the_other_credential(){
 for transport in [false,true]{let f=Fixture::new(ModelKind::Embeddings).await;f.policy(json!({"local_strategy":"round_robin","retry_times":0,"attempt_timeout_ms":1000})).await;if transport{f.nodes[0].task.abort();tokio::task::yield_now().await;}else{f.nodes[0].status(503);}let(s,b)=f.call().await;assert_eq!(s,StatusCode::OK,"transport={transport}: {b}");assert_eq!(f.nodes[1].count(),1);f.no_affinity();}
}
#[tokio::test]
async fn disabled_cooling_and_unauthorized_nodes_are_skipped_then_recovery_rejoins(){
 let f=Fixture::new(ModelKind::Embeddings).await;f.policy(json!({"local_strategy":"round_robin","retry_times":0})).await;
 {let c=f.state.conn();c.execute("UPDATE channels SET enabled=0 WHERE id=?1",[f.ids[0].0]).unwrap();}
 assert_eq!(f.call().await.0,StatusCode::OK);assert_eq!(f.nodes[0].count(),0);
 {let c=f.state.conn();c.execute("UPDATE channels SET enabled=1 WHERE id=?1",[f.ids[0].0]).unwrap();c.execute("INSERT INTO model_cooldowns(channel_id,model_name,upstream_model,until_ms) VALUES(?1,'bge','exact-model',?2)",rusqlite::params![f.ids[0].0,db::now_ms()+60000]).unwrap();}
 assert_eq!(f.call().await.0,StatusCode::OK);assert_eq!(f.nodes[0].count(),0);
 {let c=f.state.conn();c.execute("DELETE FROM model_cooldowns WHERE channel_id=?1",[f.ids[0].0]).unwrap();}
 for _ in 0..4 {assert_eq!(f.call().await.0,StatusCode::OK);}assert_eq!(f.nodes[0].count(),2);
 {let c=f.state.conn();let mut u=db::identity::user(&c,f.user).unwrap().unwrap();u.policy.allowed_channels=Some(vec![f.ids[1].0]);db::identity::update_user(&c,&u).unwrap();}
 for _ in 0..3 {assert_eq!(f.call().await.0,StatusCode::OK);}assert_eq!(f.nodes[0].count(),2);f.no_affinity();
}
#[tokio::test]
async fn full_credential_or_channel_uses_other_local_node_without_waiting(){
 for credential in [false,true]{let f=Fixture::new(ModelKind::Embeddings).await;f.policy(json!({"local_strategy":"round_robin","retry_times":0,"request_budget_ms":1000})).await;
 let gate=if credential {let c=f.state.conn();let k=db::provider_pool::source_credential(&c,f.ids[0].0).unwrap().unwrap();c.execute("UPDATE provider_credentials SET max_concurrent=1 WHERE id=?1",[k.id]).unwrap();f.state.credential_gate(k.id,1).unwrap()}else{f.state.conn().execute("UPDATE channels SET max_concurrent=1 WHERE id=?1",[f.ids[0].0]).unwrap();f.state.channel_gate(f.ids[0].0,1).unwrap()};let _held=gate.try_acquire_owned().unwrap();
 let(s,b)=tokio::time::timeout(std::time::Duration::from_millis(700),f.call()).await.expect("a busy node must not delay the other node");assert_eq!(s,StatusCode::OK,"{b}");assert_eq!((f.nodes[0].count(),f.nodes[1].count()),(0,1));}
}
#[tokio::test]
async fn local_strategy_is_validated_and_does_not_change_chat_identity(){
 let f=Fixture::new(ModelKind::Chat).await;let(s,_)=f.request("PUT","/api/model-routing?model=bge",&f.admin,json!({"local_strategy":"random"})).await;assert_eq!(s,StatusCode::BAD_REQUEST);
 f.policy(json!({"local_strategy":"round_robin","retry_times":0})).await;for _ in 0..4{assert_eq!(f.call().await.0,StatusCode::OK);}assert_eq!((f.nodes[0].count(),f.nodes[1].count()),(4,0));assert!(db::provider_pool::affinity_token(&f.state.conn(),f.user,f.provider).unwrap().is_some());
}
#[tokio::test]
async fn local_400_and_429_follow_explicit_provider_switch_rules(){
 for (status,allow) in [(400,false),(400,true),(429,false),(429,true)] {let f=Fixture::new(ModelKind::Embeddings).await;f.policy(json!({"local_strategy":"round_robin","retry_times":0})).await;f.nodes[0].status(status);f.state.conn().execute("UPDATE provider_channels SET switch_on_400=?2,switch_on_429=?2 WHERE id=?1",rusqlite::params![f.provider,allow]).unwrap();let(s,b)=f.call().await;assert_eq!(s.as_u16(),if allow{200}else{status},"status={status},allow={allow}: {b}");assert_eq!(f.nodes[1].count(),usize::from(allow));f.no_affinity();}
}
#[tokio::test]
async fn all_nodes_busy_waits_for_whichever_node_frees_capacity(){
 let f=Fixture::new(ModelKind::Embeddings).await;f.policy(json!({"local_strategy":"round_robin","retry_times":0,"request_budget_ms":1000})).await;
 let mut held=vec![];
 for id in &f.ids {let c=f.state.conn();let k=db::provider_pool::source_credential(&c,id.0).unwrap().unwrap();c.execute("UPDATE provider_credentials SET max_concurrent=1 WHERE id=?1",[k.id]).unwrap();held.push(f.state.credential_gate(k.id,1).unwrap().try_acquire_owned().unwrap());}
 let second=held.pop().unwrap();let release=tokio::spawn(async move{tokio::time::sleep(std::time::Duration::from_millis(50)).await;drop(second);});
 let(s,b)=tokio::time::timeout(std::time::Duration::from_millis(700),f.call()).await.expect("available second node should release the wait");assert_eq!(s,StatusCode::OK,"{b}");assert_eq!((f.nodes[0].count(),f.nodes[1].count()),(0,1));release.await.unwrap();
}
#[tokio::test]
async fn single_endpoint_vector_provider_retains_key_affinity_despite_local_policy(){
 let f=Fixture::new(ModelKind::Embeddings).await;f.state.conn().execute("UPDATE provider_channels SET endpoint_mode='single',base_url=?2 WHERE id=?1",rusqlite::params![f.provider,f.nodes[0].base]).unwrap();
 f.policy(json!({"local_strategy":"round_robin","retry_times":0})).await;for _ in 0..4{assert_eq!(f.call().await.0,StatusCode::OK);}assert_eq!((f.nodes[0].count(),f.nodes[1].count()),(4,0));assert!(db::provider_pool::affinity_token(&f.state.conn(),f.user,f.provider).unwrap().is_some());
}
#[tokio::test]
async fn shared_target_balances_the_target_model_not_the_public_alias(){
 let f=Fixture::new(ModelKind::Embeddings).await;f.policy(json!({"local_strategy":"round_robin","retry_times":0})).await;
 {let c=f.state.conn();db::sessions::set_panel_password(&c,"synthetic-local-vector-root").unwrap();let root=db::accounts::root(&c).unwrap().unwrap();db::shared_virtual_routes::create(&c,&db::user_virtual_routes::Config{name:"shared-bge".into(),enabled:true,mode:"ordered".into(),timezone:"UTC".into(),targets:vec![db::user_virtual_routes::Target{provider_id:f.provider,logical_model:"bge".into(),switch_on_429:true,switch_on_400:false}],windows:vec![],default_positions:vec![]},root.account.id).unwrap();}
 for _ in 0..4{let mut body=f.body();body["model"]=json!("shared-bge");let(s,b)=f.request("POST","/v1/embeddings",&f.key,body).await;assert_eq!(s,StatusCode::OK,"{b}");}assert_eq!((f.nodes[0].count(),f.nodes[1].count()),(2,2));f.no_affinity();
}
#[tokio::test]
async fn failed_node_is_not_retried_while_waiting_for_busy_survivor(){
 let f=Fixture::new(ModelKind::Embeddings).await;f.policy(json!({"local_strategy":"round_robin","retry_times":0,"request_budget_ms":1000})).await;f.nodes[0].status(503);
 let k=db::provider_pool::source_credential(&f.state.conn(),f.ids[1].0).unwrap().unwrap();f.state.conn().execute("UPDATE provider_credentials SET max_concurrent=1 WHERE id=?1",[k.id]).unwrap();let hold=f.state.credential_gate(k.id,1).unwrap().try_acquire_owned().unwrap();
 let release=tokio::spawn(async move{tokio::time::sleep(std::time::Duration::from_millis(50)).await;drop(hold);});let(s,b)=f.call().await;assert_eq!(s,StatusCode::OK,"{b}");assert_eq!((f.nodes[0].count(),f.nodes[1].count()),(1,1));release.await.unwrap();f.no_affinity();
}
#[tokio::test]
async fn balanced_and_ordinary_aliases_do_not_invert_channel_and_credential_gates(){
 let f=Arc::new(Fixture::new(ModelKind::Embeddings).await);f.policy(json!({"local_strategy":"round_robin","retry_times":0,"request_budget_ms":1200})).await;
 let mut held=vec![];
 {let c=f.state.conn();db::channels::set_models(&c,f.ids[0],&[db::channels::ModelMapping::new("bge",Some("exact-model".into()),false),db::channels::ModelMapping::new("diagnostic",Some("exact-model".into()),false)]).unwrap();db::kinds::set(&c,"diagnostic",ModelKind::Embeddings).unwrap();}
 for id in &f.ids {let c=f.state.conn();c.execute("UPDATE channels SET max_concurrent=1 WHERE id=?1",[id.0]).unwrap();let k=db::provider_pool::source_credential(&c,id.0).unwrap().unwrap();c.execute("UPDATE provider_credentials SET max_concurrent=1 WHERE id=?1",[k.id]).unwrap();held.push(f.state.credential_gate(k.id,1).unwrap().try_acquire_owned().unwrap());}
 let balanced={let f=f.clone();tokio::spawn(async move{f.call().await})};
 tokio::time::sleep(std::time::Duration::from_millis(50)).await;assert!(!balanced.is_finished());
 let legacy={let f=f.clone();tokio::spawn(async move{let mut body=f.body();body["model"]=json!("diagnostic");f.request("POST","/v1/embeddings",&f.key,body).await})};
 // The ordinary alias owns source 0's channel gate and is awaiting its credential.
 let gate=f.state.channel_gate(f.ids[0].0,1).unwrap();let mut channel_held=false;
 for _ in 0..100 {if gate.clone().try_acquire_owned().is_err(){channel_held=true;break;}tokio::task::yield_now().await;}
 assert!(channel_held,"legacy alias did not reach its channel gate");
 let first=held.remove(0);drop(first);
 let (balanced,legacy)=tokio::time::timeout(std::time::Duration::from_millis(900),async{tokio::join!(balanced,legacy)}).await.expect("mixed aliases must not form credential/channel wait cycle");
 assert_eq!(balanced.unwrap().0,StatusCode::OK);assert_eq!(legacy.unwrap().0,StatusCode::OK);
}
