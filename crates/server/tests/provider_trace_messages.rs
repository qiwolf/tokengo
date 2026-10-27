//! Neutral request_id compatibility through the production Messages converters.
//! Synthetic provider shapes; no real supplier or persisted trace-log claim.
use serde_json::{json,Value};
use tokengo_server::protocol::{convert_response,sse::{Observer,Transcoder},Protocol};

fn chat()->Value {
    json!({"id":"completion-original","object":"chat.completion","model":"glm-5.3","request_id":"trace-original","choices":[{"index":0,"message":{"role":"assistant","content":"hello","tool_calls":[{"id":"call-original","type":"function","function":{"name":"lookup","arguments":"{\"x\":1}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}})
}
fn frame(value:Value)->Vec<u8>{format!("data: {value}\n\n").into_bytes()}
fn events(wire:&[u8])->Vec<Value>{std::str::from_utf8(wire).unwrap().lines().filter_map(|line|line.strip_prefix("data: ")).filter_map(|data|serde_json::from_str(data).ok()).collect()}
fn export(name:&str,bytes:&[u8]){
    if let Some(path)=std::env::var_os("TOKENGO_PROVIDER_TRACE_EXPORT") {
        let path=std::path::PathBuf::from(path);std::fs::create_dir_all(&path).unwrap();std::fs::write(path.join(name),bytes).unwrap();
    }
}
fn source_stream(late:bool)->Vec<u8>{
    let mut first=json!({"id":"completion-original","object":"chat.completion.chunk","model":"glm-5.3","choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]});
    if !late{first["request_id"]=json!("trace-original");}
    let mut tail=json!({"id":"completion-original","object":"chat.completion.chunk","model":"glm-5.3","choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}});
    tail["request_id"]=json!("trace-original");
    [frame(first),frame(json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call-original","type":"function","function":{"name":"lookup","arguments":"{\"x\":1}"}}]},"finish_reason":null}]})),frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})),frame(tail),b"data: [DONE]\n\n".to_vec()].concat()
}
fn transcode(from:Protocol,to:Protocol,input:&[u8],chunk:usize)->Vec<u8>{
    let mut converter=Transcoder::new(from,to);let mut output=Vec::new();
    for part in input.chunks(chunk){output.extend(converter.feed(part).unwrap());}
    output.extend(converter.finish().unwrap());output
}
#[test]
fn typed_trace_json_messages_roundtrips_preserve_message_tool_and_usage_identity(){
    for source in [Protocol::Chat,Protocol::Responses] {
        let body=if source==Protocol::Chat{chat()}else{convert_response(Protocol::Chat,Protocol::Responses,&chat()).unwrap()};
        let original=body.clone();let message=convert_response(source,Protocol::Messages,&body).unwrap();
        assert_eq!(message["id"],"completion-original");assert_eq!(message["content"][1]["id"],"call-original");
        assert!(message.get("request_id").is_none());assert_eq!(message["tokengo_metadata"],json!({"request_id":"trace-original"}));
        assert_eq!(message["usage"],json!({"input_tokens":7,"output_tokens":3}));assert_eq!(body,original);
        for target in [Protocol::Chat,Protocol::Responses] {
            let result=convert_response(Protocol::Messages,target,&message).unwrap();
            assert_eq!(result["id"],"completion-original");assert_eq!(result["request_id"],"trace-original");
            if target==Protocol::Chat {
                assert_eq!(result["choices"][0]["message"]["tool_calls"][0]["id"],"call-original");
                assert_eq!(result["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],"{\"x\":1}");
            }else{assert_eq!(result["output"][1]["call_id"],"call-original");}
        }
        if source==Protocol::Chat {export("trace.messages.json",&serde_json::to_vec(&message).unwrap());}
    }
}
#[test]
fn early_and_late_trace_sse_preserves_tool_text_terminal_and_roundtrip_for_all_chunks(){
    for late in [false,true] {let input=source_stream(late);
        for size in [1,7,input.len()] {
            let output=transcode(Protocol::Chat,Protocol::Messages,&input,size);let values=events(&output);
            let start=values.iter().find(|v|v["type"]=="message_start").unwrap();
            let id=start["message"]["id"].as_str().unwrap();assert_ne!(id,"trace-original");
            if late {assert!(start["message"].get("tokengo_metadata").is_none());}
            else {assert_eq!(start["message"]["tokengo_metadata"]["request_id"],"trace-original");}
            let tail=values.iter().find(|v|v["type"]=="message_delta").unwrap();
            assert_eq!(tail["tokengo_metadata"]["request_id"],"trace-original");assert_eq!(tail["delta"]["stop_reason"],"tool_use");
            assert_eq!(values.iter().filter(|v|v["type"]=="message_stop").count(),1);
            let tool=values.iter().find(|v|v["content_block"]["type"]=="tool_use").unwrap();assert_eq!(tool["content_block"]["id"],"call-original");
            let mut observer=Observer::new(Protocol::Messages);observer.feed(&output).unwrap();observer.finish().unwrap();assert_eq!((observer.input,observer.output),(Some(7),Some(3)));
            for target in [Protocol::Chat,Protocol::Responses] {
                let back=transcode(Protocol::Messages,target,&output,1);let back_values=events(&back);
                if target==Protocol::Chat {
                    let terminal=back_values.iter().find(|v|v.pointer("/choices/0/finish_reason")==Some(&json!("tool_calls"))).unwrap();assert_eq!(terminal["request_id"],"trace-original");
                    let tool=back_values.iter().find(|v|v.pointer("/choices/0/delta/tool_calls/0/id").is_some()).unwrap();assert_eq!(tool["choices"][0]["delta"]["tool_calls"][0]["id"],"call-original");
                }else{let terminal=back_values.iter().find(|v|v["type"]=="response.completed").unwrap();assert_eq!(terminal["response"]["request_id"],"trace-original");}
            }
            if size==input.len(){export(if late{"trace-late.messages.sse"}else{"trace-early.messages.sse"},&output);}
        }
    }
}
#[test]
fn trace_metadata_rejects_invalid_conflicting_or_semantic_expansions_without_mutating_source(){
    for invalid in [Value::Null,json!(7),json!(""),json!("x".repeat(1025)),json!({})] {
        let mut body=chat();body["request_id"]=invalid;assert!(convert_response(Protocol::Chat,Protocol::Messages,&body).is_err());
    }
    let message=convert_response(Protocol::Chat,Protocol::Messages,&chat()).unwrap();
    for extension in [json!({"request_id":null}),json!({"request_id":"trace-original","web_search":[]}),json!({"request_id":"x".repeat(1025)}),json!([])] {
        let mut body=message.clone();body["tokengo_metadata"]=extension;let before=body.clone();assert!(convert_response(Protocol::Messages,Protocol::Chat,&body).is_err());assert_eq!(body,before);
    }
    let mut conflict=message.clone();conflict["request_id"]=json!("different");assert!(convert_response(Protocol::Messages,Protocol::Chat,&conflict).is_err());
    for semantic in ["web_search","content_filter","video_result","future_semantics"] {
        let mut body=chat();body[semantic]=json!([]);assert!(convert_response(Protocol::Chat,Protocol::Messages,&body).is_err());
        let mut converter=Transcoder::new(Protocol::Chat,Protocol::Messages);let mut delta=json!({"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}],"request_id":"trace-original"});delta[semantic]=json!([]);assert!(converter.feed(&frame(delta)).is_err());
    }
    for invalid in [json!(7),Value::Null,json!("x".repeat(1025))] {
        let mut converter=Transcoder::new(Protocol::Chat,Protocol::Messages);
        assert!(converter.feed(&frame(json!({"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}],"request_id":invalid}))).is_err());
    }
    let start=frame(json!({"type":"message_start","message":{"id":"message-original","type":"message","role":"assistant","model":"m","content":[],"usage":{"input_tokens":7,"output_tokens":0},"tokengo_metadata":{"request_id":"trace-original"}}}));
    let mut from_messages=Transcoder::new(Protocol::Messages,Protocol::Chat);from_messages.feed(&start).unwrap();
    assert!(from_messages.feed(&frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":3},"tokengo_metadata":{"request_id":"conflicting-late-trace"}}))).is_err());
    let mut converter=Transcoder::new(Protocol::Chat,Protocol::Messages);
    converter.feed(&frame(json!({"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}],"request_id":"trace-original"}))).unwrap();
    assert!(converter.feed(&frame(json!({"choices":[],"request_id":"conflict"}))).is_err());assert!(converter.finish().is_err());
    let native=message.clone();assert_eq!(convert_response(Protocol::Messages,Protocol::Messages,&native).unwrap(),native);
}

struct Http {url:String,task:tokio::task::JoinHandle<()>}
impl Drop for Http{fn drop(&mut self){self.task.abort();}}
async fn serve(router:axum::Router)->Http{
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}",listener.local_addr().unwrap());
    Http{url,task:tokio::spawn(async move{axum::serve(listener,router).await.unwrap()})}
}
struct Temp(std::path::PathBuf);
impl Drop for Temp{fn drop(&mut self){std::fs::remove_dir_all(&self.0).ok();}}
#[tokio::test]
async fn native_messages_json_and_sse_trace_extensions_are_forwarded_without_rewriting(){
    use axum::{routing::post,Json,response::IntoResponse};
    use tokengo_server::{app,db,AppState,Config,protocol_config::Capabilities};
    let body=json!({"id":"native-message-original","type":"message","role":"assistant","model":"native-model","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":7,"output_tokens":3},"tokengo_metadata":{"request_id":"native-trace-original"},"vendor_future":{"preserved":true}});
    let wire=transcode(Protocol::Chat,Protocol::Messages,&source_stream(true),1);
    let upstream_body=body.clone();let upstream_wire=wire.clone();
    let upstream=serve(axum::Router::new().route("/v1/messages",post(move|Json(request):Json<Value>|{
        let body=upstream_body.clone();let wire=upstream_wire.clone();async move {
            if request["stream"]==true{([(axum::http::header::CONTENT_TYPE,"text/event-stream")],wire).into_response()}
            else{Json(body).into_response()}
        }
    }))).await;
    let directory=std::env::temp_dir().join(format!("tokengo-native-trace-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&directory).unwrap();let _temp=Temp(directory.clone());
    let state=AppState::init(Config{db_path:directory.join("db"),master_key_path:directory.join("master").to_string_lossy().into(),..Default::default()}).unwrap();state.routing().cfg.retry_times=0;state.routing().cfg.probe_enabled=false;
    let key={let c=state.conn();let (_,key)=db::identity::create_key(&c,1,"synthetic trace",&Default::default(),None,None).unwrap();
        let channel=db::channels::create(&c,&db::channels::NewChannel{name:"native trace".into(),base_url:format!("{}/v1",upstream.url),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
        db::channels::set_models(&c,channel,&[db::channels::ModelMapping::new("logical",Some("native-model".into()),false)]).unwrap();
        tokengo_server::protocol_config::save(&c,channel.0,&Capabilities{native:vec![Protocol::Messages],allow_conversion:false,..Default::default()}).unwrap();key};
    let gateway=serve(app(state.clone())).await;
    for stream in [false,true]{
        let response=reqwest::Client::new().post(format!("{}/v1/messages",gateway.url)).bearer_auth(&key).json(&json!({"model":"logical","max_tokens":16,"stream":stream,"messages":[{"role":"user","content":"hello"}]})).send().await.unwrap();
        assert_eq!(response.status(),200);let bytes=response.bytes().await.unwrap();
        if stream{assert_eq!(bytes.as_ref(),wire.as_slice());}else{assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(),body);}
    }
    state.drain_writes();drop(gateway);drop(upstream);drop(state);
}
