//! Synthetic provider-shaped HTTP acceptance, not a claim of live provider validation.
use axum::{body::Body, extract::{OriginalUri, State}, response::{IntoResponse, Response}, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{app, db, protocol::Protocol, protocol_config::Capabilities, AppState, Config, SharedState};
struct Http { url: String, task: tokio::task::JoinHandle<()> }
impl Drop for Http { fn drop(&mut self) { self.task.abort(); } }
async fn serve(router: Router) -> Http {
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url=format!("http://{}",listener.local_addr().unwrap());
    Http{url,task:tokio::spawn(async move { axum::serve(listener,router).await.unwrap() })}
}
fn assistant(kind:&str, value:&str)->Value {
    match kind {
        "gemini"=>json!({"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_sum","type":"function","function":{"name":"sum","arguments":"{}"},"extra_content":{"google":{"thought_signature":value}}}]}),
        "plain"=>json!({"role":"assistant","content":"answer"}),
        _=>json!({"role":"assistant","content":"answer","encrypted_content":value}),
    }
}
#[derive(Clone)]
struct Source { seen:Arc<Mutex<Vec<Value>>>, kind:&'static str }
async fn source(State(s):State<Source>,OriginalUri(uri):OriginalUri,Json(body):Json<Value>)->Response {
    s.seen.lock().unwrap().push(body.clone());
    if uri.path().ends_with("responses") {return Json(json!({"id":"resp_native","object":"response","status":"completed","model":"physical","output":[{"id":"msg_native","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"answer","annotations":[]}]}],"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}})).into_response();}
    if uri.path().ends_with("messages") {return Json(json!({"id":"msg_native","type":"message","role":"assistant","model":"physical","content":[{"type":"text","text":"answer"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}})).into_response();}
    let message=assistant(s.kind,"issued-complete");
    if body["stream"]==true {
        // Opaque fields may be fragmented; only the closed choice is issuable.
        let mut a=assistant(s.kind,"issued-");a.as_object_mut().unwrap().remove("role");
        let mut b=assistant(s.kind,"complete");b.as_object_mut().unwrap().remove("role");
        if s.kind=="gemini" { for d in [&mut a,&mut b] {d["tool_calls"][0].as_object_mut().unwrap().remove("id");} }
        let frames=[
            json!({"id":"chat_test","object":"chat.completion.chunk","model":"physical","choices":[{"index":0,"delta":a,"finish_reason":null}]}),
            json!({"id":"chat_test","object":"chat.completion.chunk","model":"physical","choices":[{"index":0,"delta":b,"finish_reason":null}]}),
            json!({"id":"chat_test","object":"chat.completion.chunk","model":"physical","choices":[{"index":0,"delta":{},"finish_reason":if s.kind=="gemini"{"tool_calls"}else{"stop"}}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
        ];
        let mut data=frames.iter().map(|x|format!("data: {x}\n\n")).collect::<String>();data.push_str("data: [DONE]\n\n");
        return ([("content-type","text/event-stream")],Body::from(data)).into_response()
    }
    Json(json!({"id":"chat_test","object":"chat.completion","model":"physical","choices":[{"index":0,"message":message,"finish_reason":if s.kind=="gemini"{"tool_calls"}else{"stop"}}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}})).into_response()
}
struct Fixture { state:SharedState,http:Http,_upstream:Http,seen:Arc<Mutex<Vec<Value>>>,key:String,channel:i64,dir:std::path::PathBuf }
impl Drop for Fixture {fn drop(&mut self){self.state.drain_writes();std::fs::remove_dir_all(&self.dir).ok();}}
impl Fixture {
    async fn new(kind:&'static str)->Self {
        let dir=std::env::temp_dir().join(format!("chat-opaque-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();
        state.routing().cfg.retry_times=0;state.routing().cfg.probe_enabled=false;
        let seen=Arc::new(Mutex::new(vec![]));let upstream=serve(Router::new().route("/v1/chat/completions",post(source)).route("/v1/responses",post(source)).route("/v1/messages",post(source)).with_state(Source{seen:seen.clone(),kind})).await;
        let (key,channel)={let c=state.conn();let(_,key)=db::identity::create_key(&c,1,"opaque",&Default::default(),None,None).unwrap();let id=db::channels::create(&c,&db::channels::NewChannel{name:"opaque source".into(),base_url:format!("{}/v1",upstream.url),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();db::channels::set_models(&c,id,&[db::channels::ModelMapping::new("logical",Some("physical".into()),false)]).unwrap();tokengo_server::protocol_config::save(&c,id.0,&Capabilities{native:vec![Protocol::Chat],allow_conversion:true,max_input_tokens:Some(4096),max_output_tokens:Some(64),..Default::default()}).unwrap();(key,id.0)};
        let http=serve(app(state.clone())).await;Self{state,http,_upstream:upstream,seen,key,channel,dir}
    }
    async fn send(&self,key:&str,session:Option<&str>,messages:Value,stream:bool)->(u16,String) {
        let mut r=reqwest::Client::new().post(format!("{}/v1/chat/completions",self.http.url)).bearer_auth(key).json(&json!({"model":"logical","max_tokens":32,"messages":messages,"stream":stream}));
        if let Some(s)=session {r=r.header("x-session-id",s)}
        let r=r.send().await.unwrap();(r.status().as_u16(),r.text().await.unwrap())
    }
    fn replay(kind:&str,value:&str)->Value {json!([{"role":"user","content":"compute"},assistant(kind,value),{"role":"user","content":"continue"}])}
}
#[tokio::test]
async fn chat_opaque_json_and_sse_replay_preserve_and_reject_foreign_values() {
    for kind in ["gemini","ark"] {for stream in [false,true] {for session in [None,Some("explicit-a")] {
        let f=Fixture::new(kind).await;
        let(code,text)=f.send(&f.key,session,json!([{"role":"user","content":"compute"}]),stream).await;assert_eq!(code,200,"{kind} {stream}: {text}");
        let replay=Fixture::replay(kind,"issued-complete");let(code,text)=f.send(&f.key,session,replay.clone(),false).await;assert_eq!(code,200,"{kind} {stream} {session:?}: {text}");
        assert_eq!(f.seen.lock().unwrap()[1]["messages"],replay);
        let count=f.seen.lock().unwrap().len();
        for value in ["issued-","complete","tampered"] {let(code,text)=f.send(&f.key,session,Fixture::replay(kind,value),false).await;assert_eq!(code,400,"{kind} {stream} {value}: {text}");}
        let foreign={let c=f.state.conn();db::identity::create_key(&c,1,"foreign",&Default::default(),None,None).unwrap().1};
        let(code,text)=f.send(&foreign,session,replay.clone(),false).await;assert_eq!(code,400,"foreign key: {text}");
        if session.is_some() {let(code,text)=f.send(&f.key,Some("explicit-b"),replay,false).await;assert_eq!(code,400,"foreign session: {text}");}
        let response=reqwest::Client::new().post(format!("{}/v1/responses",f.http.url)).bearer_auth(&f.key)
            .json(&json!({"model":"logical","max_output_tokens":32,"input":[assistant(kind,"issued-complete")]})).send().await.unwrap();
        let status=response.status();let text=response.text().await.unwrap();assert_eq!(status,400,"cross-protocol history: {text}");
        assert_eq!(f.seen.lock().unwrap().len(),count,"invalid history reached upstream");
    }}}
}
#[tokio::test]
async fn chat_opaque_replay_rejects_changed_source_generation() {
    for kind in ["gemini","ark"] {let f=Fixture::new(kind).await;let(code,text)=f.send(&f.key,None,json!([{"role":"user","content":"compute"}]),false).await;assert_eq!(code,200,"{text}");
        let c=f.state.conn();let mut cap=tokengo_server::protocol_config::load(&c,f.channel).unwrap();cap.max_output_tokens=Some(63);tokengo_server::protocol_config::save(&c,f.channel,&cap).unwrap();drop(c);
        let(code,text)=f.send(&f.key,None,Fixture::replay(kind,"issued-complete"),false).await;assert_eq!(code,400,"{text}");assert_eq!(f.seen.lock().unwrap().len(),1);
    }
}
#[test]
fn chat_opaque_extraction_ignores_arguments_and_preserves_scope() {
    use tokengo_server::protocol_sessions::{artifact_values,opaque_history,verify_artifacts,ArtifactCollector,Pin};
    for kind in ["gemini","ark"] {
        let c=db::open_in_memory().unwrap();let pin=Pin{channel:1,generation:1,upstream_model:"physical".into(),protocol:Protocol::Chat};let body=json!({"messages":[assistant(kind,"issued-complete")]});
        assert!(opaque_history(Protocol::Chat,&body));assert_eq!(artifact_values(&body),vec!["issued-complete"]);
        let mut collector=ArtifactCollector::default();collector.value(&json!({"choices":[{"message":assistant(kind,"issued-complete")}]}));collector.persist(&c,"owner",&pin).unwrap();assert!(verify_artifacts(&c,"owner",&pin,&body).unwrap());
        for changed in [Pin{channel:2,..pin.clone()},Pin{generation:2,..pin.clone()},Pin{upstream_model:"other".into(),..pin.clone()},Pin{protocol:Protocol::Responses,..pin.clone()}] {assert!(!verify_artifacts(&c,"owner",&changed,&body).unwrap());}
    }
    let text=json!({"messages":[{
        "role":"assistant",
        "tool_calls":[{
            "type":"function",
            "function":{
                "name":"f",
                "arguments":"{\"encrypted_content\":\"not-state\"}",
                "parameters":{"extra_content":{"google":{"thought_signature":"not-state"}}}
            }
        }]
    }]});
    assert!(!opaque_history(Protocol::Chat,&text));assert!(artifact_values(&text).is_empty());
}

#[tokio::test]
async fn ordinary_chat_does_not_acquire_native_source_pin() {
    for stream in [false,true] {
        let f=Fixture::new("plain").await;
        let(code,text)=f.send(&f.key,Some("plain-session"),json!([{"role":"user","content":"compute"}]),stream).await;
        assert_eq!(code,200,"{text}");
        let count:i64=f.state.conn().query_row("SELECT COUNT(*) FROM protocol_sessions",[],|r|r.get(0)).unwrap();
        assert_eq!(count,0,"ordinary Chat routing must remain unpinned");
    }
}
#[test]
fn chat_stream_does_not_issue_open_choice_and_bounds_new_state() {
    use tokengo_server::protocol_sessions::{verify_artifacts,ArtifactCollector,Pin};
    let c=db::open_in_memory().unwrap();let pin=Pin{channel:1,generation:1,upstream_model:"physical".into(),protocol:Protocol::Chat};
    let mut collector=ArtifactCollector::default();
    let frame=|delta:Value,finish:Value|tokengo_server::protocol::sse::Frame{event:None,data:json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]}).to_string()};
    collector.frames(&[frame(json!({"encrypted_content":"prefix"}),Value::Null)]).unwrap();collector.persist(&c,"owner",&pin).unwrap();
    assert!(!verify_artifacts(&c,"owner",&pin,&json!({"messages":[assistant("ark","prefix")]})).unwrap());
    collector.frames(&[frame(json!({"encrypted_content":"suffix"}),json!("stop"))]).unwrap();collector.persist(&c,"owner",&pin).unwrap();
    assert!(verify_artifacts(&c,"owner",&pin,&json!({"messages":[assistant("ark","prefixsuffix")]})).unwrap());
    assert!(!verify_artifacts(&c,"owner",&pin,&json!({"messages":[assistant("ark","prefix")]})).unwrap());
    for malformed in [json!(7),json!({"ciphertext":"unknown"}),json!("")] {
        let mut bad=assistant("ark","value");bad["encrypted_content"]=malformed;
        let mut collector=ArtifactCollector::default();collector.value(&json!({"choices":[{"message":bad}]}));
        assert!(collector.persist(&c,"owner",&pin).is_err());
    }
    let too_many=json!({"messages":vec![assistant("ark","value");513]});
    assert!(!verify_artifacts(&c,"owner",&pin,&too_many).unwrap());
}

#[test]
fn chat_stream_message_snapshots_require_choice_close_and_do_not_concatenate() {
    use tokengo_server::protocol_sessions::{verify_artifacts,ArtifactCollector,Pin};
    for kind in ["ark","gemini"] {
        let c=db::open_in_memory().unwrap();let pin=Pin{channel:1,generation:1,upstream_model:"physical".into(),protocol:Protocol::Chat};
        let mut collector=ArtifactCollector::default();
        let frame=|value:&str,finish:Value| {
            let mut message=assistant(kind,value);
            if kind=="gemini" {message["tool_calls"][0].as_object_mut().unwrap().remove("index");}
            tokengo_server::protocol::sse::Frame{event:None,data:json!({"choices":[{"index":0,"message":message,"finish_reason":finish}]}).to_string()}
        };
        collector.frames(&[frame("prefix",Value::Null)]).unwrap();collector.persist(&c,"owner",&pin).unwrap();
        assert!(!verify_artifacts(&c,"owner",&pin,&json!({"messages":[assistant(kind,"prefix")]})).unwrap());
        collector.frames(&[frame("prefixsuffix",json!("stop"))]).unwrap();collector.persist(&c,"owner",&pin).unwrap();
        assert!(verify_artifacts(&c,"owner",&pin,&json!({"messages":[assistant(kind,"prefixsuffix")]})).unwrap());
    }
}

#[tokio::test]
async fn foreign_chat_opaque_fields_never_pass_native_responses_or_messages() {
    for protocol in [Protocol::Responses,Protocol::Messages] {for kind in ["gemini","ark"] {
        let f=Fixture::new("plain").await;
        {let c=f.state.conn();let mut caps=tokengo_server::protocol_config::load(&c,f.channel).unwrap();caps.native=vec![protocol];tokengo_server::protocol_config::save(&c,f.channel,&caps).unwrap();}
        let call=|messages:Value| {
            let body=if protocol==Protocol::Responses {json!({"model":"logical","max_output_tokens":32,"input":messages})}else{json!({"model":"logical","max_tokens":32,"messages":messages})};
            reqwest::Client::new().post(format!("{}/v1/{}",f.http.url,protocol.path())).bearer_auth(&f.key).header("x-session-id","native-owned").json(&body).send()
        };
        let response=call(json!([{"role":"user","content":"hello"}])).await.unwrap();let status=response.status();let text=response.text().await.unwrap();assert_eq!(status,200,"{protocol:?}: {text}");
        let response=call(json!([assistant(kind,"unissued-chat-state"),{"role":"user","content":"continue"}])).await.unwrap();let status=response.status();let text=response.text().await.unwrap();assert_eq!(status,400,"{protocol:?}/{kind}: {text}");
        assert_eq!(f.seen.lock().unwrap().len(),1,"foreign Chat state was sent on native {protocol:?}");
    }}
}
#[test]
fn foreign_chat_state_cannot_borrow_an_identical_native_artifact() {
    use tokengo_server::protocol_sessions::{opaque_history,verify_artifacts,ArtifactCollector,Pin};
    for protocol in [Protocol::Responses,Protocol::Messages] {for kind in ["gemini","ark"] {
        let c=db::open_in_memory().unwrap();let pin=Pin{channel:1,generation:1,upstream_model:"physical".into(),protocol};
        let issued=if protocol==Protocol::Responses {json!({"type":"reasoning","encrypted_content":"same-value"})}else{json!({"type":"thinking","signature":"same-value"})};
        let mut collector=ArtifactCollector::default();collector.value(&issued);collector.persist(&c,"owner",&pin).unwrap();
        let body=if protocol==Protocol::Responses {json!({"input":[assistant(kind,"same-value")]})}else{json!({"messages":[assistant(kind,"same-value")]})};
        assert!(opaque_history(protocol,&body),"{protocol:?}/{kind} must enter ownership protection");
        assert!(!verify_artifacts(&c,"owner",&pin,&body).unwrap(),"{protocol:?}/{kind} must not borrow a differently-shaped issued artifact");
    }}
}
#[test]
fn authoritative_chat_message_snapshot_retires_stale_delta_fields() {
    use tokengo_server::protocol_sessions::{verify_artifacts,ArtifactCollector,Pin};
    for kind in ["ark","gemini"] {for terminal in ["plain","different-tool","both"] {
        let c=db::open_in_memory().unwrap();let pin=Pin{channel:1,generation:1,upstream_model:"physical".into(),protocol:Protocol::Chat};let mut collector=ArtifactCollector::default();
        let frame=|choice:Value|tokengo_server::protocol::sse::Frame{event:None,data:json!({"choices":[choice]}).to_string()};
        collector.frames(&[frame(json!({"index":0,"delta":assistant(kind,"prefix"),"finish_reason":null}))]).unwrap();
        let mut choice=json!({"index":0,"message":assistant("plain","unused"),"finish_reason":"stop"});
        if terminal=="different-tool" {let mut complete=assistant("gemini","complete");complete["tool_calls"][0]["index"]=json!(1);choice["message"]=complete;}
        if terminal=="both" {choice["delta"]=assistant(kind,"prefix");choice["message"]=assistant(kind,"complete");}
        collector.frames(&[frame(choice)]).unwrap();collector.persist(&c,"owner",&pin).unwrap();
        assert!(!verify_artifacts(&c,"owner",&pin,&json!({"messages":[assistant(kind,"prefix")]})).unwrap(),"{kind}/{terminal} issued a stale prefix");
        if terminal!="plain" {let target=if terminal=="both"{kind}else{"gemini"};assert!(verify_artifacts(&c,"owner",&pin,&json!({"messages":[assistant(target,"complete")]})).unwrap());}
    }}
}
#[test]
fn chat_message_snapshot_followed_by_delta_keeps_complete_value() {
    use tokengo_server::protocol_sessions::{verify_artifacts,ArtifactCollector,Pin};
    for kind in ["ark","gemini"] {
        let c=db::open_in_memory().unwrap();let pin=Pin{channel:1,generation:1,upstream_model:"physical".into(),protocol:Protocol::Chat};let mut collector=ArtifactCollector::default();
        let frame=|choice:Value|tokengo_server::protocol::sse::Frame{event:None,data:json!({"choices":[choice]}).to_string()};
        collector.frames(&[frame(json!({"index":0,"message":assistant(kind,"prefix"),"finish_reason":null})),frame(json!({"index":0,"delta":assistant(kind,"suffix"),"finish_reason":"stop"}))]).unwrap();collector.persist(&c,"owner",&pin).unwrap();
        assert!(verify_artifacts(&c,"owner",&pin,&json!({"messages":[assistant(kind,"prefixsuffix")]})).unwrap());
        assert!(!verify_artifacts(&c,"owner",&pin,&json!({"messages":[assistant(kind,"prefix")]})).unwrap());
    }
}
