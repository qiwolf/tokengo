//! Documentary parameter contracts through real local gateway HTTP; no live provider calls.
use axum::{extract::State, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{app, db, protocol::Protocol, protocol_config::Capabilities, AppState, Config, SharedState};
struct Http { url: String, task: tokio::task::JoinHandle<()> }
impl Drop for Http { fn drop(&mut self) { self.task.abort(); } }
async fn serve(router: Router) -> Http {
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url=format!("http://{}",listener.local_addr().unwrap());
    Http {url,task:tokio::spawn(async move {axum::serve(listener,router).await.unwrap()})}
}
async fn source(State(seen):State<Arc<Mutex<Vec<Value>>>>,Json(body):Json<Value>)->Json<Value> {
    seen.lock().unwrap().push(body.clone());
    let mut result=if body.get("input").is_some() {
        json!({"id":"resp_combo","object":"response","model":body["model"],"status":"completed","output":[{"id":"msg_combo","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"OK","annotations":[]}]}],"usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}})
    } else {
        json!({"id":"chat_combo","object":"chat.completion","model":body["model"],"choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}})
    };
    if body["prompt_cache_key"] == "summary-fixture" {
        result["output"].as_array_mut().unwrap().insert(0,json!({"type":"reasoning","id":"rs_kimi","status":"completed","summary":[{"type":"summary_text","text":"Provider summary"}],"encrypted_content":null}));
    }
    Json(result)
}
struct Fixture {state:SharedState,gateway:Http,_source:Http,seen:Arc<Mutex<Vec<Value>>>,key:String,dir:std::path::PathBuf}
impl Drop for Fixture {fn drop(&mut self){self.state.drain_writes();std::fs::remove_dir_all(&self.dir).ok();}}
impl Fixture {
    async fn new(profile:&str,model:&str,protocol:Protocol)->Self {
        let dir=std::env::temp_dir().join(format!("provider-combinations-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config {db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();
        state.routing().cfg.retry_times=0;state.routing().cfg.probe_enabled=false;
        let seen=Arc::new(Mutex::new(vec![]));
        let source=serve(Router::new().route("/v1/chat/completions",post(source)).route("/v1/responses",post(source)).with_state(seen.clone())).await;
        let key={let c=state.conn();
            let user=db::identity::create_user(&c,"combo owner","user",&Default::default()).unwrap();
            let(_,key)=db::identity::create_key(&c,user.id,"combo-client",&Default::default(),None,None).unwrap();
            let id=db::channels::create(&c,&db::channels::NewChannel {name:"source-contract".into(),base_url:format!("{}/v1",source.url),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
            db::channels::set_models(&c,id,&[db::channels::ModelMapping::new("logical",Some(model.into()),false)]).unwrap();
            tokengo_server::protocol_config::save(&c,id.0,&Capabilities {parameter_profile:Some(profile.into()),native:vec![protocol],allow_conversion:true,chat_reasoning_replay:protocol==Protocol::Chat,max_input_tokens:Some(4096),max_output_tokens:Some(1024),..Default::default()}).unwrap();
            db::billing::create_price(&c,&db::billing::Price {id:0,channel_id:id.0,model:"logical".into(),input_micro_per_million:1,output_micro_per_million:1,max_input_tokens:4096,max_output_tokens:2048},"synthetic").unwrap();key};
        let gateway=serve(app(state.clone())).await;Self {state,gateway,_source:source,seen,key,dir}
    }
    async fn call(&self,protocol:Protocol,extra:Value)->(u16,String) {
        let mut b=if protocol==Protocol::Chat {json!({"model":"logical","messages":[{"role":"user","content":"hi"}],"max_tokens":32})} else {json!({"model":"logical","input":"hi","max_output_tokens":32})};
        b.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let r=reqwest::Client::new().post(format!("{}/v1/{}",self.gateway.url,protocol.path())).bearer_auth(&self.key).json(&b).send().await.unwrap();(r.status().as_u16(),r.text().await.unwrap())
    }
}
#[tokio::test]
async fn china_parameter_rejection_precedes_upstream_and_reservation() {
    for (profile,model,protocol,bad,field) in [
        ("deepseek","deepseek-flash",Protocol::Responses,json!({"parallel_tool_calls":false}),"parallel_tool_calls"),
        ("deepseek","deepseek-flash",Protocol::Responses,json!({"input":[{"type":"reasoning","summary":[{"type":"summary_text","text":"summary"}]}]}),"explicit_session_required"),
        ("deepseek","deepseek-v4-pro",Protocol::Chat,json!({"tool_choice":"required"}),"tool_choice"),
        ("dashscope","qwen3.8-max",Protocol::Chat,json!({"tool_choice":"required","enable_thinking":false}),"tool_choice"),
        ("dashscope","kimi-k3",Protocol::Chat,json!({"parallel_tool_calls":false}),"parallel_tool_calls"),
        ("dashscope","MiniMax/MiniMax-M3.1-Flash",Protocol::Chat,json!({"reasoning_effort":"none"}),"reasoning_effort"),
        ("dashscope","kimi/kimi-k3.1-pro",Protocol::Chat,json!({"reasoning_effort":"none"}),"reasoning_effort"),
        ("moonshot","kimi-k3",Protocol::Responses,json!({"tool_choice":"required"}),"tool_choice"),
        ("zai","glm-5.3-flash",Protocol::Chat,json!({"response_format":{"type":"json_schema"}}),"response_format"),
        ("zai","glm-5.3",Protocol::Chat,json!({"tool_stream":true}),"tool_stream"),
    ] {
        let f=Fixture::new(profile,model,protocol).await;
        let(code,text)=f.call(protocol,bad).await;assert_eq!(code,400,"{profile}/{model}: {text}");assert!(text.contains(field),"{text}");
        assert!(f.seen.lock().unwrap().is_empty());
        let count:i64=f.state.conn().query_row("SELECT count(*) FROM billing_ledger",[],|r|r.get(0)).unwrap();assert_eq!(count,0);
        let(code,text)=f.call(protocol,json!({})).await;assert_eq!(code,200,"default {profile}/{model}: {text}");assert_eq!(f.seen.lock().unwrap().len(),1);
    }
}
#[tokio::test]
async fn converted_requests_enforce_destination_contract_without_changing_defaults() {
    let f=Fixture::new("moonshot","kimi-k3",Protocol::Responses).await;
    let(code,text)=f.call(Protocol::Chat,json!({"tool_choice":"required"})).await;
    assert_eq!(code,400,"{text}");assert!(text.contains("tool_choice"));assert!(f.seen.lock().unwrap().is_empty());
    let(code,text)=f.call(Protocol::Chat,json!({"tool_choice":"auto"})).await;
    assert_eq!(code,200,"{text}");assert_eq!(f.seen.lock().unwrap()[0]["tool_choice"],"auto");
}

// G09 native Kimi summaries require their independently issued source proof.
#[tokio::test]
async fn kimi_summary_output_replays_only_with_issued_source_proof() {
    let f=Fixture::new("moonshot","kimi-k3",Protocol::Responses).await;
    let(code,text)=f.call(Protocol::Responses,json!({"prompt_cache_key":"summary-fixture"})).await;
    assert_eq!(code,200,"{text}");
    let first:Value=serde_json::from_str(&text).unwrap();
    assert_eq!(first["output"][0]["summary"][0]["text"],"Provider summary");
    assert!(first["output"][0].get("content").is_none());
    let mut history=first["output"].as_array().unwrap().clone();history.push(json!({"role":"user","content":"continue"}));
    let(code,text)=f.call(Protocol::Responses,json!({"input":history.clone()})).await;
    assert_eq!(code,200,"{text}");
    assert_eq!(f.seen.lock().unwrap()[1]["input"],json!(history));
    history[0]["summary"][0]["text"]=json!("never issued");
    let(code,text)=f.call(Protocol::Responses,json!({"input":history})).await;
    assert_eq!(code,400,"{text}");
    assert_eq!(f.seen.lock().unwrap().len(),2,"unissued summary history must not reach the source");
}

#[tokio::test]
async fn deepseek_strict_default_does_not_escape_to_unverified_beta_transport() {
    let f=Fixture::new("deepseek","deepseek-flash",Protocol::Chat).await;
    let request=json!({"tools":[{"type":"function","name":"f","parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}]});
    let(code,text)=f.call(Protocol::Responses,request.clone()).await;
    assert_eq!(code,400,"{text}");assert!(text.contains("Beta"),"{text}");
    assert!(f.seen.lock().unwrap().is_empty());
    assert!(request["tools"][0].get("strict").is_none());
    let count:i64=f.state.conn().query_row("SELECT count(*) FROM billing_ledger",[],|r|r.get(0)).unwrap();assert_eq!(count,0);
}
