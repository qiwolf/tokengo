//! Provider-shaped loopback acceptance; not a live manufacturer claim.
use axum::{extract::State,routing::post,Json,Router};
use serde_json::{json,Value};
use std::sync::{Arc,Mutex};
use tokengo_server::{app,db,protocol::Protocol,protocol_config::Capabilities,AppState,Config};
struct Http{url:String,task:tokio::task::JoinHandle<()>}
impl Drop for Http{fn drop(&mut self){self.task.abort();}}
async fn serve(router:Router)->Http{let l=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}",l.local_addr().unwrap());Http{url,task:tokio::spawn(async move{axum::serve(l,router).await.unwrap()})}}
#[derive(Clone)]struct Source{seen:Arc<Mutex<Vec<Value>>>,to:Protocol}
async fn source(State(s):State<Source>,Json(body):Json<Value>)->Json<Value>{let active=body["thinking"]["type"]=="adaptive";s.seen.lock().unwrap().push(body);Json(if s.to==Protocol::Chat{json!({"id":"chat_test","object":"chat.completion","model":"MiniMax-M3","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}})}else{json!({"id":"msg_test","type":"message","role":"assistant","model":"MiniMax-M3","content":if active{json!([{"type":"thinking","thinking":"native thought"},{"type":"text","text":"ok"}])}else{json!([{"type":"text","text":"ok"}])},"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":4,"output_tokens":2}})})}
#[tokio::test]
async fn minimax_protocol_defaults_and_explicit_modes_preserve_source_intent(){
 for (from,to,expected) in [(Protocol::Chat,Protocol::Messages,"adaptive"),(Protocol::Messages,Protocol::Chat,"disabled")]{for configured_off in [false,true]{
  let seen=Arc::new(Mutex::new(vec![]));let upstream=serve(Router::new().route(&format!("/v1/{}",to.path()),post(source)).with_state(Source{seen:seen.clone(),to})).await;
  let dir=std::env::temp_dir().join(format!("provider-defaults-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
  let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();state.routing().cfg.retry_times=0;state.routing().cfg.probe_enabled=false;
  let key={let c=state.conn();let(_,key)=db::identity::create_key(&c,1,"synthetic",&Default::default(),None,None).unwrap();let ch=db::channels::create(&c,&db::channels::NewChannel{name:"synthetic MiniMax".into(),base_url:format!("{}/v1",upstream.url),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();db::channels::set_models(&c,ch,&[db::channels::ModelMapping::new("logical",Some("MiniMax-M3".into()),false)]).unwrap();tokengo_server::protocol_config::save(&c,ch.0,&Capabilities{parameter_profile:Some("minimax".into()),native:vec![to],allow_conversion:true,max_input_tokens:Some(8192),max_output_tokens:Some(1024),..Default::default()}).unwrap();if configured_off{tokengo_server::reasoning_capabilities::configure(&c,ch.0,"logical",Some(&["none".to_string()]),Some("none")).unwrap();}key};
  let gateway=serve(app(state.clone())).await;
  for mode in [None,Some("adaptive"),Some("disabled")]{
   let mut body=json!({"model":"logical","messages":[{"role":"user","content":"hello"}],"max_tokens":256});if let Some(mode)=mode{body["thinking"]=json!({"type":mode});}
   let before=seen.lock().unwrap().len();let result=reqwest::Client::new().post(format!("{}/v1/{}",gateway.url,from.path())).bearer_auth(&key).json(&body).send().await.unwrap();let status=result.status();let wire=result.text().await.unwrap();
   let effective=mode.unwrap_or(if configured_off{"disabled"}else{expected});
   if effective=="adaptive" {assert_eq!(status,400,"active thinking needs the native transport codec: {wire}");assert_eq!(seen.lock().unwrap().len(),before,"must refuse before charging upstream");continue;}
   assert_eq!(status,200,"{from:?}->{to:?}: {wire}");let sent=seen.lock().unwrap().last().unwrap().clone();assert_eq!(sent["thinking"]["type"],mode.unwrap_or(if configured_off{"disabled"}else{expected}));assert_eq!(sent["max_tokens"],256);
  }
  state.drain_writes();drop(gateway);drop(state);std::fs::remove_dir_all(dir).ok();
 }
 }
}
