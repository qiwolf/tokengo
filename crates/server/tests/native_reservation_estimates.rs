//! Ordinary native sources remain usable with local estimates, never fake supplier caps.
use axum::{extract::State,response::{IntoResponse,Response},routing::post,Json,Router};
use serde_json::{json,Value};
use std::sync::{Arc,Mutex};
use tokengo_server::{app,db,protocol::Protocol,protocol_config::Capabilities,AppState,Config,SharedState};
struct Http{url:String,task:tokio::task::JoinHandle<()>}
impl Drop for Http{fn drop(&mut self){self.task.abort();}}
async fn serve(router:Router)->Http{let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}",listener.local_addr().unwrap());Http{url,task:tokio::spawn(async move{axum::serve(listener,router).await.unwrap()})}}
struct Source{seen:Mutex<Vec<Value>>,output:i64}
async fn source(State(state):State<Arc<Source>>,Json(body):Json<Value>)->Response{
    state.seen.lock().unwrap().push(body.clone());let usage=json!({"prompt_tokens":7,"completion_tokens":state.output,"total_tokens":7+state.output,"completion_tokens_details":{"reasoning_tokens":state.output-3}});
    if body["stream"]==true {
        let wire=format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",json!({"id":"source-original","object":"chat.completion.chunk","model":"Hosted/New-Reasoner","choices":[{"index":0,"delta":{"role":"assistant","content":"ANSWER"},"finish_reason":null}]}),json!({"id":"source-original","object":"chat.completion.chunk","model":"Hosted/New-Reasoner","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":usage}));
        ([("content-type","text/event-stream")],wire).into_response()
    }else{Json(json!({"id":"source-original","object":"chat.completion","model":"Hosted/New-Reasoner","choices":[{"index":0,"message":{"role":"assistant","content":"ANSWER"},"finish_reason":"stop"}],"usage":usage})).into_response()}
}
struct Fixture{state:SharedState,gateway:Http,_source:Http,seen:Arc<Source>,key:String,user:i64,dir:std::path::PathBuf}
impl Drop for Fixture{fn drop(&mut self){self.state.drain_writes();std::fs::remove_dir_all(&self.dir).ok();}}
impl Fixture{
    async fn new(price:bool,output:i64)->Self{
        let dir=std::env::temp_dir().join(format!("native-estimate-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();state.routing().cfg.retry_times=0;state.routing().cfg.probe_enabled=false;
        let seen=Arc::new(Source{seen:Mutex::new(vec![]),output});let upstream=serve(Router::new().route("/v1/chat/completions",post(source)).with_state(seen.clone())).await;
        let(key,user)={let c=state.conn();let user=db::identity::create_user(&c,"native estimate owner","user",&Default::default()).unwrap();let(_,key)=db::identity::create_key(&c,user.id,"native estimate key",&Default::default(),None,None).unwrap();
            let id=db::channels::create(&c,&db::channels::NewChannel{name:"native estimate source".into(),base_url:format!("{}/v1",upstream.url),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
            db::channels::set_models(&c,id,&[db::channels::ModelMapping::new("logical",Some("Hosted/New-Reasoner".into()),false)]).unwrap();
            tokengo_server::protocol_config::save(&c,id.0,&Capabilities{native:vec![Protocol::Chat],allow_conversion:true,parameter_profile:Some("siliconflow".into()),..Default::default()}).unwrap();
            if price{db::billing::create_price(&c,&db::billing::Price{id:0,channel_id:id.0,model:"logical".into(),input_micro_per_million:1_000_000,output_micro_per_million:1_000_000,max_input_tokens:8192,max_output_tokens:2048},"synthetic").unwrap();}(key,user.id)};
        let gateway=serve(app(state.clone())).await;Self{state,gateway,_source:upstream,seen,key,user,dir}
    }
    async fn call(&self,stream:bool,cap:i64)->(u16,String){let mut body=json!({"model":"logical","messages":[{"role":"user","content":"hello"}],"stream":stream,"max_tokens":cap});if stream{body["stream_options"]=json!({"include_usage":true});}let response=reqwest::Client::new().post(format!("{}/v1/chat/completions",self.gateway.url)).bearer_auth(&self.key).json(&body).send().await.unwrap();(response.status().as_u16(),response.text().await.unwrap())}
}
#[tokio::test]
async fn native_json_and_sse_use_price_estimate_without_source_metadata_and_settle_reported_usage_once(){
    for stream in [false,true]{let f=Fixture::new(true,3).await;let(code,wire)=f.call(stream,64).await;assert_eq!(code,200,"{wire}");assert!(wire.contains("ANSWER"));if stream{assert!(wire.contains("data: [DONE]"));}
        f.state.drain_writes();let seen=f.seen.seen.lock().unwrap();assert_eq!(seen.len(),1);assert_eq!(seen[0]["max_tokens"],64);assert!(seen[0].get("thinking_budget").is_none());drop(seen);
        let c=f.state.conn();let row:(i64,i64,i64,i64,i64,String,String)=c.query_row("SELECT count(*),output_bound,input_tokens,output_tokens,debit_micro,usage_certainty,source_snapshot FROM billing_ledger",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).unwrap();
        assert_eq!((row.0,row.1,row.2,row.3,row.4),(1,2048,7,3,10));assert_eq!(row.5,"reported");let snapshot:Value=serde_json::from_str(&row.6).unwrap();
        assert_eq!(snapshot["reservation_estimate"]["origin"],"price_configured");assert_eq!(snapshot["reservation_estimate"]["confidence"],"estimate_not_upstream_bound");assert_eq!(snapshot["reservation_estimate"]["supplier_hard_cap_verified"],false);
        assert_eq!(c.query_row("SELECT count(*) FROM attempts",[],|r|r.get::<_,i64>(0)).unwrap(),1);assert_eq!(c.query_row("SELECT count(*) FROM billing_budgets WHERE reserved_micro<>0",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }
}
#[tokio::test]
async fn native_default_estimate_is_unpriced_and_finite_money_budget_still_requires_price(){
    let f=Fixture::new(false,3).await;let(code,wire)=f.call(false,64).await;assert_eq!(code,200,"{wire}");f.state.drain_writes();
    {let c=f.state.conn();let(bound,debit,certainty,snapshot):(i64,Option<i64>,String,String)=c.query_row("SELECT output_bound,debit_micro,usage_certainty,source_snapshot FROM billing_ledger",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();assert_eq!((bound,debit,certainty.as_str()),(4096,None,"unpriced"));let snapshot:Value=serde_json::from_str(&snapshot).unwrap();assert_eq!(snapshot["reservation_estimate"]["origin"],"gateway_default");}
    let denied=Fixture::new(false,3).await;db::billing::set_budget(&denied.state.conn(),"user",denied.user,Some(1000),"test").unwrap();let(code,wire)=denied.call(false,64).await;assert_eq!(code,403,"{wire}");assert!(wire.contains("config_required"));assert!(denied.seen.seen.lock().unwrap().is_empty());
}
#[tokio::test]
async fn estimates_do_not_override_explicit_price_permissions_or_tpm_limits(){
    let f=Fixture::new(true,3).await;let(code,wire)=f.call(false,4096).await;assert_eq!(code,400,"{wire}");assert!(f.seen.seen.lock().unwrap().is_empty());
    let denied=Fixture::new(true,3).await;{let c=denied.state.conn();let mut user=db::identity::user(&c,denied.user).unwrap().unwrap();user.policy.allowed_models=Some(vec![]);db::identity::update_user(&c,&user).unwrap();}
    assert_eq!(denied.call(false,64).await.0,403);assert!(denied.seen.seen.lock().unwrap().is_empty());
    let limited=Fixture::new(true,3).await;{let c=limited.state.conn();let mut user=db::identity::user(&c,limited.user).unwrap().unwrap();user.policy.tpm=Some(1);db::identity::update_user(&c,&user).unwrap();}
    assert_eq!(limited.call(false,64).await.0,429);assert!(limited.seen.seen.lock().unwrap().is_empty());
}
#[tokio::test]
async fn real_usage_overrun_is_recorded_once_and_existing_freeze_blocks_followup(){
    let f=Fixture::new(true,4096).await;let(code,wire)=f.call(false,64).await;assert_eq!(code,200,"{wire}");f.state.drain_writes();
    {let c=f.state.conn();let(bound,actual,reserved,debit,observed,overrun):(i64,i64,i64,i64,i64,bool)=c.query_row("SELECT output_bound,output_tokens,reserved_micro,debit_micro,observed_cost_micro,overrun FROM billing_ledger",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();assert_eq!((bound,actual,observed),(2048,4096,4103));assert_eq!(debit,reserved);assert!(overrun);}
    let(code,wire)=f.call(false,64).await;assert_eq!(code,429,"{wire}");assert_eq!(f.seen.seen.lock().unwrap().len(),1);f.state.drain_writes();assert_eq!(f.state.conn().query_row("SELECT COUNT(*) FROM billing_ledger",[],|r|r.get::<_,i64>(0)).unwrap(),1);
}
