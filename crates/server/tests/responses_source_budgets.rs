//! Provider-shaped HTTP budget acceptance; this does not contact Alibaba Cloud.
use axum::{body::Body, extract::State, response::{IntoResponse, Response}, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{app, db, protocol::Protocol, protocol_config::Capabilities, AppState, Config, SharedState};
struct Http { url: String, task: tokio::task::JoinHandle<()> }
impl Drop for Http { fn drop(&mut self) { self.task.abort(); } }
async fn serve(router: Router) -> Http {
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url=format!("http://{}",listener.local_addr().unwrap());
    Http { url, task: tokio::spawn(async move { axum::serve(listener,router).await.unwrap() }) }
}
async fn source(State(seen):State<Arc<Mutex<Vec<Value>>>>, Json(body):Json<Value>)->Response {
    seen.lock().unwrap().push(body.clone());
    let output=if body["model"]=="qwen3.8-max" {32} else {35};
    let response=json!({"id":"resp_budget","object":"response","model":body["model"],"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"id":"msg_budget","type":"message","status":"incomplete","role":"assistant","content":[{"type":"output_text","text":"answer","annotations":[]}]}],"usage":{"input_tokens":5,"output_tokens":output,"total_tokens":5+output}});
    if body["stream"]==true {
        let data=format!("event: response.output_text.delta\ndata: {}\n\nevent: response.incomplete\ndata: {}\n\n",json!({"type":"response.output_text.delta","item_id":"msg_budget","output_index":0,"content_index":0,"delta":"answer"}),json!({"type":"response.incomplete","response":response}));
        ([("content-type","text/event-stream")],Body::from(data)).into_response()
    } else { Json(response).into_response() }
}
struct Fixture { state:SharedState, gateway:Http, _source:Http, seen:Arc<Mutex<Vec<Value>>>, key:String, dir:std::path::PathBuf }
impl Drop for Fixture { fn drop(&mut self) { self.state.drain_writes();std::fs::remove_dir_all(&self.dir).ok(); } }
impl Fixture {
    async fn new(model:&str,bounded:bool)->Self {
        let dir=std::env::temp_dir().join(format!("responses-budget-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config {db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();
        state.routing().cfg.retry_times=0;state.routing().cfg.probe_enabled=false;
        let seen=Arc::new(Mutex::new(vec![]));let source=serve(Router::new().route("/v1/responses",post(source)).with_state(seen.clone())).await;
        let key={let c=state.conn();let(_,key)=db::identity::create_key(&c,1,"budget-client",&Default::default(),None,None).unwrap();
            let id=db::channels::create(&c,&db::channels::NewChannel {name:"budget-source".into(),base_url:format!("{}/v1",source.url),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
            db::channels::set_models(&c,id,&[db::channels::ModelMapping::new("logical",Some(model.into()),false)]).unwrap();
            tokengo_server::protocol_config::save(&c,id.0,&Capabilities {parameter_profile:Some("dashscope".into()),native:vec![Protocol::Responses],allow_conversion:true,max_input_tokens:bounded.then_some(4096),max_output_tokens:bounded.then_some(1024),..Default::default()}).unwrap();
            db::billing::create_price(&c,&db::billing::Price {id:0,channel_id:id.0,model:"logical".into(),input_micro_per_million:1_000_000,output_micro_per_million:1_000_000,max_input_tokens:4096,max_output_tokens:2048},"synthetic").unwrap();key};
        let gateway=serve(app(state.clone())).await;Self {state,gateway,_source:source,seen,key,dir}
    }
    async fn call(&self,path:&str,body:Value)->(u16,String) {
        let r=reqwest::Client::new().post(format!("{}{path}",self.gateway.url)).bearer_auth(&self.key).json(&body).send().await.unwrap();(r.status().as_u16(),r.text().await.unwrap())
    }
}
#[tokio::test]
async fn native_answer_caps_reserve_verified_total_and_settle_incomplete_once() {
    for model in ["qwen3.7-plus","deepseek-v4-pro","future-model","qwen3.8-max"] { for stream in [false,true] {
        let f=Fixture::new(model,true).await;
        let(code,text)=f.call("/v1/responses",json!({"model":"logical","input":"hi","max_output_tokens":32,"stream":stream})).await;
        assert_eq!(code,200,"{model} {stream}: {text}");assert!(text.contains("incomplete"),"{text}");
        f.state.drain_writes();
        assert_eq!(f.seen.lock().unwrap().len(),1);
        assert_eq!(f.seen.lock().unwrap()[0]["max_output_tokens"],32,"the answer cap must be unchanged on wire");
        let c=f.state.conn();
        let row:(i64,i64,i64,String)=c.query_row("SELECT count(*),output_bound,debit_micro,status FROM billing_ledger",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!(row.0,1,"one request must reserve only once");
        assert_eq!(row.1,if model=="qwen3.8-max" {32} else {1024});
        assert_eq!(row.2,if model=="qwen3.8-max" {37} else {40});
        assert_ne!(row.3,"reserved");
        let reserved:i64=c.query_row("SELECT COALESCE(SUM(reserved_micro),0) FROM billing_budgets",[],|r|r.get(0)).unwrap();assert_eq!(reserved,0);
    }}
}
#[tokio::test]
async fn native_unknown_budget_uses_price_estimate_but_unrepresentable_conversion_still_rejects() {
    for model in ["qwen3.7-plus","future-model"] {
        let f=Fixture::new(model,false).await;
        let(code,text)=f.call("/v1/responses",json!({"model":"logical","input":"hi","max_output_tokens":32,"thinking_budget":64})).await;
        assert_eq!(code,200,"{model}: {text}");assert_eq!(f.seen.lock().unwrap().len(),1);
        f.state.drain_writes();let c=f.state.conn();
        let (count,bound,snapshot):(i64,i64,String)=c.query_row("SELECT count(*),output_bound,source_snapshot FROM billing_ledger",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!((count,bound),(1,2048));let snapshot:Value=serde_json::from_str(&snapshot).unwrap();
        assert_eq!(snapshot["reservation_estimate"]["origin"],"price_configured");assert_eq!(snapshot["reservation_estimate"]["confidence"],"estimate_not_upstream_bound");
        assert_eq!(f.seen.lock().unwrap()[0]["max_output_tokens"],32);

    }
    let f=Fixture::new("qwen3.7-plus",true).await;
    let(code,text)=f.call("/v1/chat/completions",json!({"model":"logical","messages":[{"role":"user","content":"hi"}],"max_tokens":32})).await;
    assert_eq!(code,400,"{text}");assert_eq!(f.seen.lock().unwrap().len(),0);
    let count:i64=f.state.conn().query_row("SELECT count(*) FROM billing_ledger",[],|r|r.get(0)).unwrap();assert_eq!(count,0);
    let f=Fixture::new("qwen3.8-max",true).await;
    let(code,text)=f.call("/v1/responses",json!({"model":"logical","input":"hi","max_output_tokens":15})).await;
    assert_eq!(code,400,"{text}");assert_eq!(f.seen.lock().unwrap().len(),0);
    let count:i64=f.state.conn().query_row("SELECT count(*) FROM billing_ledger",[],|r|r.get(0)).unwrap();assert_eq!(count,0);
}
