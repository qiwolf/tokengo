//! Native Kimi Responses history through local HTTP fixtures; no real account calls.
use axum::{body::Body, extract::State, response::{IntoResponse, Response}, routing::post, Json, Router};
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
fn summary() -> Value { json!({"id":"rs_kimi","type":"reasoning","status":"completed","summary":[{"type":"summary_text","text":"Provider summary"}],"encrypted_content":null}) }
fn output() -> Value { json!([summary(),{"id":"msg_kimi","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"answer","annotations":[]}]}]) }
async fn source(State(seen):State<Arc<Mutex<Vec<Value>>>>,Json(body):Json<Value>) -> Response {
    seen.lock().unwrap().push(body.clone());
    let mut emitted=output();
    if body["input"]=="foreign-large" {let mut items=vec![summary();512];items.push(json!({"id":"rs_cipher","type":"reasoning","status":"completed","summary":[],"encrypted_content":"legacy-cipher"}));emitted=json!(items);}
    let response=json!({"id":"resp_kimi","object":"response","model":body["model"],"status":"completed","output":emitted,"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}});
    if body["stream"]==true {
        let frames=[json!({"type":"response.created","response":{"id":"resp_kimi","status":"in_progress","output":[]}}),json!({"type":"response.output_item.added","output_index":0,"item":{"id":"rs_kimi","type":"reasoning","summary":[],"encrypted_content":null,"status":"in_progress"}}),json!({"type":"response.reasoning_summary_text.delta","item_id":"rs_kimi","output_index":0,"summary_index":0,"delta":"Provider summary"}),json!({"type":"response.output_item.done","output_index":0,"item":summary()}),json!({"type":"response.output_item.done","output_index":1,"item":output()[1]}),json!({"type":"response.completed","response":response})];
        return ([("content-type","text/event-stream")],Body::from(frames.iter().map(|v|format!("event: {}\ndata: {v}\n\n",v["type"].as_str().unwrap())).collect::<String>())).into_response();
    }
    Json(response).into_response()
}
struct Fixture { state:SharedState, http:Http, _source:Http, seen:Arc<Mutex<Vec<Value>>>, key:String, channel:i64, dir:std::path::PathBuf }
impl Drop for Fixture {fn drop(&mut self){self.state.drain_writes();std::fs::remove_dir_all(&self.dir).ok();}}
impl Fixture {
    async fn new(profile:&str,model:&str)->Self {
        let dir=std::env::temp_dir().join(format!("kimi-summary-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();state.routing().cfg.retry_times=0;state.routing().cfg.probe_enabled=false;
        let seen=Arc::new(Mutex::new(vec![]));let upstream=serve(Router::new().route("/v1/responses",post(source)).with_state(seen.clone())).await;
        let(key,channel)={let c=state.conn();let(_,key)=db::identity::create_key(&c,1,"summary",&Default::default(),None,None).unwrap();let id=db::channels::create(&c,&db::channels::NewChannel{name:"summary source".into(),base_url:format!("{}/v1",upstream.url),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();db::channels::set_models(&c,id,&[db::channels::ModelMapping::new("logical",Some(model.into()),false)]).unwrap();tokengo_server::protocol_config::save(&c,id.0,&Capabilities{parameter_profile:Some(profile.into()),native:vec![Protocol::Responses],allow_conversion:true,max_input_tokens:Some(4096),max_output_tokens:Some(64),..Default::default()}).unwrap();(key,id.0)};
        let http=serve(app(state.clone())).await;Self{state,http,_source:upstream,seen,key,channel,dir}
    }
    async fn send(&self,key:&str,session:Option<&str>,input:Value,stream:bool)->(u16,String) {
        let mut r=reqwest::Client::new().post(format!("{}/v1/responses",self.http.url)).bearer_auth(key).json(&json!({"model":"logical","input":input,"max_output_tokens":32,"stream":stream}));if let Some(s)=session {r=r.header("x-session-id",s)}let r=r.send().await.unwrap();(r.status().as_u16(),r.text().await.unwrap())
    }
}
#[tokio::test]
async fn native_kimi_json_and_sse_complete_output_replays_anonymous_and_explicit() {
    for stream in [false,true] {for session in [None,Some("explicit-a")] {
        let f=Fixture::new("moonshot","kimi-k3").await;
        let(code,text)=f.send(&f.key,session,json!("hello"),stream).await;assert_eq!(code,200,"{text}");
        let emitted=if stream {text.lines().filter_map(|l|l.strip_prefix("data: ")).filter_map(|s|serde_json::from_str::<Value>(s).ok()).find(|v|v["type"]=="response.completed").unwrap()["response"]["output"].clone()}else{serde_json::from_str::<Value>(&text).unwrap()["output"].clone()};assert_eq!(emitted,output());
        let mut history=emitted.as_array().unwrap().clone();history.push(json!({"role":"user","content":"continue"}));let history=json!(history);
        let(code,text)=f.send(&f.key,session,history.clone(),false).await;assert_eq!(code,200,"{stream}/{session:?}: {text}");assert_eq!(f.seen.lock().unwrap()[1]["input"],history);
        let count=f.seen.lock().unwrap().len();let foreign={let c=f.state.conn();db::identity::create_key(&c,1,"foreign",&Default::default(),None,None).unwrap().1};
        let(code,text)=f.send(&foreign,session,history.clone(),false).await;assert_eq!(code,400,"{text}");
        for other in [Some("foreign-session"),if session.is_some(){None}else{Some("explicit-a")}] {let(code,text)=f.send(&f.key,other,history.clone(),false).await;assert_eq!(code,400,"{text}");}
        for malformed in [json!([{ "type":"summary_text","text":"tampered"}]),json!([{ "type":"summary_text","text":7}]),json!([{ "type":"image","image_url":"x"}])] {let mut h=history.clone();h[0]["summary"]=malformed;let(code,text)=f.send(&f.key,session,h,false).await;assert_eq!(code,400,"{text}");}
        for invalid in [json!(""),json!(7),json!({"opaque":"forged"})] {let mut h=history.clone();h[0]["encrypted_content"]=invalid;let(code,text)=f.send(&f.key,session,h,false).await;assert_eq!(code,400,"invalid ciphertext type borrowed empty proof: {text}");}
        let mut mixed=history.clone();mixed.as_array_mut().unwrap().push(json!({"type":"reasoning","encrypted_content":"not-issued"}));let(code,text)=f.send(&f.key,session,mixed,false).await;assert_eq!(code,400,"{text}");
        let mut cipher=summary();cipher["encrypted_content"]=json!(summary().to_string());cipher["summary"]=json!([]);let(code,text)=f.send(&f.key,session,json!([cipher]),false).await;assert_eq!(code,400,"{text}");
        let r=reqwest::Client::new().post(format!("{}/v1/chat/completions",f.http.url)).bearer_auth(&f.key).json(&json!({"model":"logical","max_tokens":32,"messages":[{"role":"assistant","content":[summary()]}]})).send().await.unwrap();assert_eq!(r.status(),400);
        assert_eq!(f.seen.lock().unwrap().len(),count,"rejected history reached source");
    }}
}
#[tokio::test]
async fn summary_replay_requires_exact_declared_source_and_current_generation() {
    for (profile,model) in [("moonshot","kimi-k3"),("moonshot","kimi-k3.1-pro"),("moonshot","kimi-k3.1-pro-highspeed"),("moonshot","kimi-future"),("deepseek","deepseek-flash"),("native","kimi-k3")] {
        let f=Fixture::new(profile,model).await;
        let(code,text)=f.send(&f.key,None,json!([summary()]),false).await;assert_eq!(code,400,"unissued: {text}");
        let(code,text)=f.send(&f.key,None,json!("hello"),false).await;assert_eq!(code,200,"{text}");
        let supported=profile=="moonshot" && model!="kimi-future";
        let(code,text)=f.send(&f.key,None,output(),false).await;assert_eq!(code,if supported {200}else{400},"{profile}/{model}: {text}");
        if supported {let c=f.state.conn();let mut caps=tokengo_server::protocol_config::load(&c,f.channel).unwrap();caps.max_output_tokens=Some(63);tokengo_server::protocol_config::save(&c,f.channel,&caps).unwrap();drop(c);let(code,text)=f.send(&f.key,None,output(),false).await;assert_eq!(code,400,"generation: {text}");}
    }
}

#[tokio::test]
async fn summary_proofs_are_separate_from_ciphertext_and_bound_to_complete_snapshots() {
    use tokengo_server::protocol_sessions::{anonymous_key, resolve_anonymous, verify_artifacts, ArtifactCollector, Pin};
    let f=Fixture::new("moonshot","kimi-k3").await;let c=f.state.conn();
    let generation=c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[f.channel],|r|r.get(0)).unwrap();
    let pin=Pin{channel:f.channel,generation,upstream_model:"kimi-k3".into(),protocol:Protocol::Responses};let owner=anonymous_key("namespace","logical",&pin);let body=json!({"input":output()});
    let frame=|v:Value|tokengo_server::protocol::sse::Frame{event:None,data:v.to_string()};
    let mut collector=ArtifactCollector::for_source(&c,&pin).unwrap();
    collector.frames(&[frame(json!({"type":"response.output_item.added","item":summary()})),frame(json!({"type":"response.reasoning_summary_part.done","part":{"type":"summary_text","text":"Provider summary"}}))]).unwrap();collector.persist(&c,&owner,&pin).unwrap();
    assert!(resolve_anonymous(&c,"namespace","logical",&[pin.clone()],&body).unwrap().is_none(),"added and part-done are not complete item issuance");
    collector.frames(&[frame(json!({"type":"response.output_item.done","item":summary()}))]).unwrap();collector.persist(&c,&owner,&pin).unwrap();
    assert!(verify_artifacts(&c,&owner,&pin,&body).unwrap());assert!(resolve_anonymous(&c,"namespace","logical",&[pin.clone()],&body).unwrap().is_some());
    assert!(resolve_anonymous(&c,"namespace","other-model",&[pin.clone()],&body).unwrap().is_none());
    for changed in [Pin{channel:f.channel+1,..pin.clone()},Pin{generation:generation+1,..pin.clone()},Pin{upstream_model:"kimi-k3.1-pro".into(),..pin.clone()},Pin{protocol:Protocol::Chat,..pin.clone()}] {assert!(!verify_artifacts(&c,&owner,&changed,&body).unwrap());}
    for value in [summary().to_string(),summary()["summary"].to_string()] {assert!(!verify_artifacts(&c,&owner,&pin,&json!({"input":[{"type":"reasoning","summary":[],"encrypted_content":value}]})).unwrap(),"summary digest lent to ciphertext");}
    let mut edited=summary();edited["id"]=json!("changed-id");assert!(!verify_artifacts(&c,&owner,&pin,&json!({"input":[edited]})).unwrap());
    // Issued ciphertext never lends proof to the summary domain either.
    let mut collector=ArtifactCollector::for_source(&c,&pin).unwrap();let other=anonymous_key("other","logical",&pin);collector.value(&json!({"output":[{"type":"reasoning","encrypted_content":summary().to_string()}]}));collector.persist(&c,&other,&pin).unwrap();assert!(!verify_artifacts(&c,&other,&pin,&body).unwrap());
    let reordered:Value=serde_json::from_str(r#"{"encrypted_content":null,"summary":[{"text":"Provider summary","type":"summary_text"}],"status":"completed","type":"reasoning","id":"rs_kimi"}"#).unwrap();assert!(verify_artifacts(&c,&owner,&pin,&json!({"input":[reordered]})).unwrap());
    c.execute("UPDATE protocol_artifacts SET last_used_ms=?1",[db::now_ms()-tokengo_server::protocol_sessions::INACTIVE_TTL_MS]).unwrap();assert!(!verify_artifacts(&c,&owner,&pin,&body).unwrap());
}

#[tokio::test]
async fn malformed_or_oversized_summary_cannot_be_issued() {
    use tokengo_server::protocol_sessions::{verify_artifacts,ArtifactCollector,Pin};
    let f=Fixture::new("moonshot","kimi-k3").await;let c=f.state.conn();let pin=Pin{channel:f.channel,generation:c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[f.channel],|r|r.get(0)).unwrap(),upstream_model:"kimi-k3".into(),protocol:Protocol::Responses};
    for (name,items) in [
        ("nontext",json!([{"type":"reasoning","summary":[{"type":"summary_text","text":7}],"encrypted_content":null}])),
        ("too-many",json!(vec![summary();513])),
        ("too-large",{let mut item=summary();item["summary"][0]["text"]=json!("x".repeat((1<<20)+1));json!([item])}),
        ("total-too-large",{let mut item=summary();item["summary"][0]["text"]=json!("x".repeat((1<<20)-1024));json!(vec![item;17])}),
    ] {let mut collector=ArtifactCollector::for_source(&c,&pin).unwrap();collector.value(&json!({"output":items}));let _=collector.persist(&c,name,&pin);assert!(!verify_artifacts(&c,name,&pin,&json!({"input":items})).unwrap(),"{name}");}
    // Summary snapshots lacking completion are never replay authority.
    let mut incomplete=summary();incomplete["status"]=json!("in_progress");let mut collector=ArtifactCollector::for_source(&c,&pin).unwrap();collector.value(&json!({"output":[incomplete.clone()]}));let _=collector.persist(&c,"open",&pin);assert!(!verify_artifacts(&c,"open",&pin,&json!({"input":[incomplete]})).unwrap());
}

#[tokio::test]
async fn summary_source_policy_uses_official_endpoint_or_explicit_profile_not_branding() {
    use tokengo_server::protocol_sessions::{ArtifactCollector,Pin};
    for (url,profile,native,model,accepted) in [
        ("https://api.moonshot.cn/v1",None,true,"kimi-k3",true),
        ("https://api.moonshot.ai/v1",None,true,"kimi-k3",true),
        ("https://api.moonshot.cn/v1",Some("native"),true,"kimi-k3",false),
        ("https://api.moonshot.cn.evil.test/v1",None,true,"kimi-k3",false),
        ("https://api.moonshot.cn/v1?x=1",None,true,"kimi-k3",false),
        ("http://api.moonshot.cn/v1",None,true,"kimi-k3",false),
        ("http://compatible.test/v1",Some("moonshot"),true,"kimi-k3",true),
        ("http://compatible.test/v1",Some("moonshot"),false,"kimi-k3",false),
        ("https://api.moonshot.cn/v1",None,true,"Kimi-K3",false),
    ] {
        let f=Fixture::new("moonshot",model).await;let c=f.state.conn();
        c.execute("UPDATE channels SET base_url=?2 WHERE id=?1",rusqlite::params![f.channel,url]).unwrap();
        let mut caps=tokengo_server::protocol_config::load(&c,f.channel).unwrap();caps.parameter_profile=profile.map(str::to_string);caps.native=vec![if native{Protocol::Responses}else{Protocol::Chat}];tokengo_server::protocol_config::save(&c,f.channel,&caps).unwrap();
        let pin=Pin{channel:f.channel,generation:c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[f.channel],|r|r.get(0)).unwrap(),upstream_model:model.into(),protocol:Protocol::Responses};
        let owner=tokengo_server::protocol_sessions::anonymous_key("policy","logical",&pin);
        let mut collector=ArtifactCollector::for_source(&c,&pin).unwrap();collector.value(&json!({"output":output()}));collector.persist(&c,&owner,&pin).unwrap();
        assert_eq!(tokengo_server::protocol_sessions::resolve_anonymous(&c,"policy","logical",&[pin],&json!({"input":output()})).unwrap().is_some(),accepted,"{url}/{profile:?}/{native}/{model}");
    }
}

#[test]
fn generic_conversion_keeps_summary_distinct_from_raw_reasoning() {
    let body=json!({"model":"kimi-k3","input":output(),"max_output_tokens":32});
    for target in [Protocol::Chat,Protocol::Messages] {
        assert!(tokengo_server::protocol::convert_request(Protocol::Responses,target,&body).is_err());
    }
    assert_eq!(tokengo_server::protocol::convert_request(Protocol::Responses,Protocol::Responses,&body).unwrap(),body);
}

#[tokio::test]
async fn valid_openai_explicit_summary_history_keeps_legacy_native_behavior() {
    for stream in [false,true] {
        let f=Fixture::new("openai","gpt-test").await;
        let(code,text)=f.send(&f.key,Some("legacy-openai"),json!("hello"),stream).await;assert_eq!(code,200,"{text}");
        let(code,text)=f.send(&f.key,Some("legacy-openai"),output(),false).await;assert_eq!(code,200,"legacy explicit history regressed: {text}");assert_eq!(f.seen.lock().unwrap()[1]["input"],output());
        let(code,text)=f.send(&f.key,None,output(),false).await;assert_eq!(code,400,"non-Kimi summary-only must not acquire anonymous ownership: {text}");
        let(code,text)=f.send(&f.key,None,json!("hello"),false).await;assert_eq!(code,200,"{text}");
        let(code,text)=f.send(&f.key,None,output(),false).await;assert_eq!(code,400,"existing anonymous empty-digest owner must not lend summary ownership: {text}");
        assert_eq!(f.seen.lock().unwrap().len(),3);
    }
}

#[tokio::test]
async fn other_native_providers_do_not_apply_kimi_summary_bounds_to_output() {
    for profile in ["openai","native"] {
        let f=Fixture::new(profile,"gpt-test").await;
        let(code,text)=f.send(&f.key,Some("foreign-output"),json!("foreign-large"),false).await;
        assert_eq!(code,200,"foreign provider incorrectly ran Kimi summary codec: {text}");
        let value:Value=serde_json::from_str(&text).unwrap();assert_eq!(value["output"].as_array().unwrap().len(),513);
    }
}

#[tokio::test]
async fn invalid_summary_source_never_falls_back_to_legacy_verification() {
    use tokengo_server::protocol_sessions::{verify_artifacts,ArtifactCollector,Pin};
    for profile in ["moonshot","openai"] {for change in ["disabled","missing","generation","protocol"] {
        let f=Fixture::new(profile,if profile=="moonshot"{"kimi-k3"}else{"gpt-test"}).await;let c=f.state.conn();
        let pin=Pin{channel:f.channel,generation:c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[f.channel],|r|r.get(0)).unwrap(),upstream_model:if profile=="moonshot"{"kimi-k3".into()}else{"gpt-test".into()},protocol:Protocol::Responses};
        let mut collector=ArtifactCollector::for_source(&c,&pin).unwrap();collector.value(&json!({"output":output()}));collector.persist(&c,"bound-owner",&pin).unwrap();assert!(verify_artifacts(&c,"bound-owner",&pin,&json!({"input":output()})).unwrap());
        match change {
            "disabled"=>{c.execute("UPDATE channels SET enabled=0 WHERE id=?1",[f.channel]).unwrap();},
            "missing"=>{c.execute("DELETE FROM channels WHERE id=?1",[f.channel]).unwrap();},
            "generation"=>{c.execute("UPDATE channels SET binding_generation=binding_generation+1 WHERE id=?1",[f.channel]).unwrap();},
            _=>{let mut caps=tokengo_server::protocol_config::load(&c,f.channel).unwrap();caps.native=vec![Protocol::Chat];tokengo_server::protocol_config::save(&c,f.channel,&caps).unwrap();},
        }
        assert!(!verify_artifacts(&c,"bound-owner",&pin,&json!({"input":output()})).unwrap(),"{profile}/{change} borrowed legacy proof");
    }}
}
