//! Explicit, opt-in real-provider acceptance. Never runs in the default test suite.
//! The source key is supplied to this process only; no real configuration is copied.
use axum::Router;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokengo_server::{app, db, protocol::{convert_request, Protocol}, protocol_config::Capabilities, AppState, Config, SharedState};

struct Fixture {
    state: SharedState,
    task: tokio::task::JoinHandle<()>,
    url: String,
    key: String,
    dir: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        self.state.drain_writes();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl Fixture {
    async fn new(source_key: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-live-siliconflow-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let state = AppState::init(Config { db_path:dir.join("test.db"), master_key_path:dir.join("master").to_string_lossy().into(), ..Default::default() }).unwrap();
        state.routing().cfg.retry_times = 0;
        state.routing().cfg.probe_enabled = false;
        let key = {
            let c=state.conn();
            let user=db::identity::create_user(&c,"synthetic live acceptance","user",&Default::default()).unwrap();
            let (_,key)=db::identity::create_key(&c,user.id,"synthetic live key",&Default::default(),None,None).unwrap();
            let channel=db::channels::create(&c,&db::channels::NewChannel { name:"isolated SiliconFlow acceptance".into(),base_url:"https://api.siliconflow.cn/v1".into(),enabled:true,unwrap_envelope:false,max_concurrent:0 }).unwrap();
            db::channels::set_secret(&c,&state.secrets,channel,source_key).unwrap();
            db::channels::set_models(&c,channel,&[db::channels::ModelMapping::new("sf-live-qwen",Some("Qwen/Qwen3.8-27B".into()),false)]).unwrap();
            tokengo_server::protocol_config::save(&c,channel.0,&Capabilities { parameter_profile:Some("siliconflow".into()),native:vec![Protocol::Chat],allow_conversion:true,chat_reasoning_replay:true,..Default::default() }).unwrap();
            // Fixture admission prices are synthetic; this does not verify provider pricing.
            db::billing::create_price(&c,&db::billing::Price { id:0,channel_id:channel.0,model:"sf-live-qwen".into(),input_micro_per_million:1,output_micro_per_million:1,max_input_tokens:16384,max_output_tokens:1024 },"synthetic acceptance").unwrap();
            key
        };
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            for name in ["test.db","master"] {
                std::fs::set_permissions(dir.join(name),std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url=format!("http://{}",listener.local_addr().unwrap());
        let router:Router=app(state.clone());
        let task=tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); });
        Self { state,task,url,key,dir }
    }
    async fn call(&self,protocol:Protocol,body:Value)->(u16,String) {
        let result=reqwest::Client::builder().timeout(Duration::from_secs(120)).build().unwrap()
            .post(format!("{}/v1/{}",self.url,protocol.path())).bearer_auth(&self.key)
            .header("User-Agent","tokengo-provider-p1-live-acceptance/20261027").json(&body).send().await.unwrap();
        (result.status().as_u16(),result.text().await.unwrap())
    }
}
fn events(wire:&str)->Vec<Value> {
    wire.lines().filter_map(|s|s.strip_prefix("data: ")).filter(|s|*s!="[DONE]")
        .filter_map(|s|serde_json::from_str(s).ok()).collect()
}
fn body(protocol:Protocol,stream:bool)->Value {
    let mut request=match protocol {
        Protocol::Chat=>json!({"model":"sf-live-qwen","messages":[{"role":"system","content":"Answer briefly."},{"role":"developer","content":"Keep answers exact, use the declared tool when asked."},{"role":"user","content":"Reply with the word LIVE_OK."}],"reasoning_effort":"none","max_tokens":256,"stream":stream}),
        Protocol::Responses=>json!({"model":"sf-live-qwen","input":[{"role":"developer","content":"Keep answers exact, use the declared tool when asked."},{"role":"user","content":"Reply briefly with the word LIVE_OK."}],"reasoning":{"effort":"none"},"max_output_tokens":256,"stream":stream,"store":false}),
        _=>unreachable!(),
    };
    if stream && protocol==Protocol::Chat {request["stream_options"]=json!({"include_usage":true});}
    request
}
fn text(protocol:Protocol,value:&Value)->String {
    if protocol==Protocol::Chat { value.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("").to_owned() }
    else { value["output"].as_array().into_iter().flatten().flat_map(|v|v["content"].as_array().into_iter().flatten()).filter_map(|v|v["text"].as_str()).collect() }
}
fn error_excerpt(value:&Value)->String {
    let text=value.as_str().map(str::to_owned).unwrap_or_else(||value.to_string());
    // Bound bytes without cutting a UTF-8 code point.
    let mut end=text.len().min(2048);
    while !text.is_char_boundary(end) {end-=1;}
    text[..end].to_owned()
}
fn record_status(records:&mut Vec<Value>,label:&str,status:u16,wire:&str) {
    let error=serde_json::from_str::<Value>(wire).ok().and_then(|v|v.pointer("/error/code").cloned());
    let parsed=serde_json::from_str::<Value>(wire).ok();
    let usage=parsed.as_ref().and_then(|v|v.get("usage").cloned()).or_else(|| {
        events(wire).into_iter().rev().find_map(|v| v.get("usage").filter(|u|u.is_object()).cloned()
            .or_else(||v.pointer("/response/usage").cloned()))
    });
    let mut record=json!({"case":label,"http_status":status,"gateway_error_code":error,"usage":usage});
    if status!=200 {
        let message=parsed.as_ref().and_then(|v|v.pointer("/error/message").or_else(||v.get("message")).or_else(||v.get("detail")))
            .map(error_excerpt).unwrap_or_else(||error_excerpt(&json!(wire)));
        let error_type=parsed.as_ref().and_then(|v|v.pointer("/error/type").or_else(||v.get("type"))).map(error_excerpt);
        record["gateway_error_message"]=json!(message);
        record["gateway_error_type"]=json!(error_type);
    }
    records.push(record);
}

fn request_log_errors(f:&Fixture)->Vec<Value> {
    // Verified db/schema.rs requests.error and migrated error_code; no payload query.
        let c=f.state.conn();
        let mut columns=c.prepare("PRAGMA table_info(requests)").unwrap();
        let columns=columns.query_map([],|r|r.get::<_,String>(1)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        assert!(columns.iter().any(|v|v=="error") && columns.iter().any(|v|v=="error_code"));
        let mut query=c.prepare("SELECT id,status,http_status,error_code,error FROM requests WHERE error IS NOT NULL OR error_code IS NOT NULL ORDER BY id").unwrap();
        let rows=query.query_map([],|r| {
            let error:Option<String>=r.get(4)?;
            Ok(json!({"request_id":r.get::<_,i64>(0)?,"status":r.get::<_,String>(1)?,"http_status":r.get::<_,Option<i64>>(2)?,"error_code":r.get::<_,Option<String>>(3)?,"error":error.map(|e|error_excerpt(&json!(e)))}))
        }).unwrap();
        rows.collect::<Result<Vec<Value>,_>>().unwrap()

}

#[tokio::test]
#[ignore = "paid real SiliconFlow API; requires explicit authorization and in-memory key"]
async fn live_siliconflow_qwen_json_sse_and_tool_roundtrip() {
    let source_key=std::env::var("TOKENGO_TEST_SILICONFLOW_KEY").expect("explicit source key required");
    let report=std::env::var_os("TOKENGO_TEST_REPORT").map(PathBuf::from).expect("report path required");
    let f=Fixture::new(&source_key).await;
    let mut records=Vec::new();
    let mut failures=Vec::new();
    for protocol in [Protocol::Chat,Protocol::Responses] {
        for stream in [false,true] {
            let label=format!("{protocol:?}_stream_{stream}");
            let (status,wire)=f.call(protocol,body(protocol,stream)).await;
            record_status(&mut records,&label,status,&wire);
            let ok=if status!=200 {false} else if stream {
                let es=events(&wire);
                if protocol==Protocol::Chat {
                    wire.contains("data: [DONE]") && es.iter().filter_map(|v|v.pointer("/choices/0/delta/content").and_then(Value::as_str)).collect::<String>().trim()=="LIVE_OK"
                        && es.iter().any(|v|v.get("usage").is_some_and(Value::is_object))
                } else {es.iter().any(|v|v["type"]=="response.completed") && es.iter().filter(|v|v["type"]=="response.output_text.delta").filter_map(|v|v["delta"].as_str()).collect::<String>().trim()=="LIVE_OK"}
            } else {
                serde_json::from_str::<Value>(&wire).is_ok_and(|v|text(protocol,&v).trim()=="LIVE_OK" && v.get("usage").is_some_and(Value::is_object))
            };
            records.last_mut().unwrap()["passed"]=json!(ok);
            if !ok {failures.push(label);}
        }
        let mut first=body(protocol,false);
        let mut function=json!({"name":"lookup_test_value","description":"Return the test value for a key. Use this tool to answer the user's request.","parameters":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}});
        if protocol==Protocol::Chat {
            function["strict"]=json!(false);
            first["messages"]=json!([{"role":"system","content":"Answer briefly."},{"role":"developer","content":"Keep answers exact, use the declared tool when asked."},{"role":"user","content":"Use lookup_test_value with key alpha, then report the returned test value. Do not guess it."}]);
            first["tools"]=json!([{"type":"function","function":function}]);
            first["max_tokens"]=json!(256);
        } else {
            first["input"]=json!([{"role":"developer","content":"Keep answers exact, use the declared tool when asked."},{"role":"user","content":"Use lookup_test_value with key alpha, then report the returned test value. Do not guess it."}]);
            let mut tool=function;tool["type"]=json!("function");first["tools"]=json!([tool]);first["max_output_tokens"]=json!(256);
        }
        // This same client request goes to the live gateway unchanged. Inspect the
        // public converter separately so strict inference is observable in evidence.
        let converted=convert_request(protocol,Protocol::Chat,&first).expect("live tool request must convert");
        let expected_strict=protocol==Protocol::Responses;
        assert_eq!(converted["tools"][0]["function"]["strict"],json!(expected_strict));
        if protocol==Protocol::Responses { assert!(first["tools"][0].get("strict").is_none()); }
        else { assert_eq!(first["tools"][0]["function"]["strict"],false); }
        let (status,wire)=f.call(protocol,first.clone()).await;
        let label=format!("{protocol:?}_tool_first");record_status(&mut records,&label,status,&wire);
        records.last_mut().unwrap()["strict_contract"]=json!({"client":if expected_strict {"omitted"} else {"explicit_false"},"native_chat_function_strict":expected_strict,"client_schema_closed":true,"converter_observed":true,"upstream_wire_captured":false});
        let parsed=serde_json::from_str::<Value>(&wire).unwrap_or(Value::Null);
        let tool_valid=if protocol==Protocol::Chat {
            parsed.pointer("/choices/0/message/tool_calls/0/function/name").and_then(Value::as_str)==Some("lookup_test_value")
                && parsed.pointer("/choices/0/message/tool_calls/0/function/arguments").and_then(Value::as_str)
                    .and_then(|s|serde_json::from_str::<Value>(s).ok())==Some(json!({"key":"alpha"}))
        } else {
            parsed["output"].as_array().and_then(|xs|xs.iter().find(|v|v["type"]=="function_call"))
                .is_some_and(|call|call["name"]=="lookup_test_value" && call["arguments"].as_str()
                    .and_then(|s|serde_json::from_str::<Value>(s).ok())==Some(json!({"key":"alpha"})))
        };
        let continuation=if status==200 && tool_valid && protocol==Protocol::Chat {
            parsed.pointer("/choices/0/message/tool_calls/0/id").and_then(Value::as_str).map(|id| {
                let mut next=first.clone();let mut message=parsed["choices"][0]["message"].clone();
                // Native Chat history preserves every provider field.
                message["role"]=json!("assistant");
                next["messages"].as_array_mut().unwrap().push(message);
                next["messages"].as_array_mut().unwrap().push(json!({"role":"tool","tool_call_id":id,"content":"ALPHA_LIVE_42"}));
                next
            })
        } else if status==200 && tool_valid {
            parsed["output"].as_array().and_then(|xs|xs.iter().find(|v|v["type"]=="function_call"))
                .and_then(|call|call["call_id"].as_str()).map(|id| {
                    let mut next=first.clone();next["input"].as_array_mut().unwrap().extend(parsed["output"].as_array().unwrap().clone());
                    next["input"].as_array_mut().unwrap().push(json!({"type":"function_call_output","call_id":id,"output":"ALPHA_LIVE_42"}));next
                })
        } else {None};
        records.last_mut().unwrap()["assertions"]=json!({"http_200":status==200,"tool_name_and_arguments":tool_valid,"call_id_present":continuation.is_some(),"usage_present":parsed.get("usage").is_some_and(Value::is_object)});
        records.last_mut().unwrap()["passed"]=json!(continuation.is_some() && parsed.get("usage").is_some_and(Value::is_object));
        if !parsed.get("usage").is_some_and(Value::is_object) {failures.push(format!("{label}_missing_usage"));}
        if let Some(next)=continuation {
            let (status,wire)=f.call(protocol,next).await;let label=format!("{protocol:?}_tool_final");record_status(&mut records,&label,status,&wire);
            let ok=status==200 && serde_json::from_str::<Value>(&wire).is_ok_and(|v|text(protocol,&v).contains("ALPHA_LIVE_42") && v.get("usage").is_some_and(Value::is_object));
            records.last_mut().unwrap()["assertions"]=json!({"http_200":status==200,"tool_result_and_usage":ok});
            records.last_mut().unwrap()["passed"]=json!(ok);if !ok {failures.push(label);}
        } else {failures.push(label);}
    }
    f.state.drain_writes();
    let request_count:i64=f.state.conn().query_row("SELECT count(*) FROM requests",[],|r|r.get(0)).unwrap();
    let total_usage:(i64,i64)=f.state.conn().query_row("SELECT coalesce(sum(input_tokens),0),coalesce(sum(output_tokens),0) FROM requests",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    let request_errors=request_log_errors(&f);
    let result=json!({"source":"https://api.siliconflow.cn/v1","model":"Qwen/Qwen3.8-27B","host":std::env::var("HOSTNAME").unwrap_or_default(),"synthetic_material":true,"native_protocol":"chat","client_protocols":["chat","responses"],"scope":"SiliconFlow hosted source only; not Qwen/DashScope official acceptance","gateway_request_records":request_count,"request_errors":request_errors,"usage":{"input_tokens":total_usage.0,"output_tokens":total_usage.1},"records":records,"expected_calls":8,"thinking":"none","developer_role_scope":"leading plaintext developer instruction in both Chat and Responses clients; hosted source acceptance only","strict_scope":"Chat explicit false; Responses omitted strict on already closed schema inferred true by gateway converter; no client changes","passed":failures.is_empty() && request_count==8,"failures":failures,"provider_prices_verified":false,"production_modified":false});
    let sanitized=serde_json::to_string_pretty(&result).unwrap().replace(&source_key,"[REDACTED_SOURCE_KEY]").replace(&f.key,"[REDACTED_FIXTURE_KEY]");
    std::fs::write(report,sanitized).unwrap();
    drop(f);
    assert!(result["passed"]==true,"Real-provider acceptance failed; inspect sanitized report");
}


#[tokio::test]
#[ignore = "one paid SiliconFlow diagnostic; requires explicit authorization and in-memory key"]
async fn live_siliconflow_developer_diagnostic() {
    let source_key=std::env::var("TOKENGO_TEST_SILICONFLOW_KEY").expect("explicit source key required");
    let report=std::env::var_os("TOKENGO_TEST_REPORT").map(PathBuf::from).expect("report path required");
    let f=Fixture::new(&source_key).await;
    let (status,wire)=f.call(Protocol::Chat,body(Protocol::Chat,false)).await;
    let mut records=Vec::new();
    record_status(&mut records,"Chat_developer_diagnostic",status,&wire);
    f.state.drain_writes();
    let request_log_errors=request_log_errors(&f);
    let result=json!({"source":"https://api.siliconflow.cn/v1","model":"Qwen/Qwen3.8-27B","host":std::env::var("HOSTNAME").unwrap_or_default(),"synthetic_material":true,"expected_calls":1,"automatic_retry":false,"request_payload":"unchanged body(Chat,false), system then plaintext developer then user","records":records,"request_log_errors":request_log_errors,"passed":status==200,"production_modified":false,"scope":"SiliconFlow hosted source diagnostic only; not official Qwen or other vendor acceptance"});
    let sanitized=serde_json::to_string_pretty(&result).unwrap().replace(&source_key,"[REDACTED_SOURCE_KEY]").replace(&f.key,"[REDACTED_FIXTURE_KEY]");
    std::fs::write(report,sanitized).unwrap();
    drop(f);
}
