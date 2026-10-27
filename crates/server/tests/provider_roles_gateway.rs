//! Documented source/model role contracts exercised through gateway HTTP.
use axum::{body::Body, extract::State, http::StatusCode, response::{IntoResponse, Response}, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{app, db, protocol::Protocol, protocol_config::Capabilities, AppState, Config, SharedState};

struct Http { url: String, task: tokio::task::JoinHandle<()> }
impl Drop for Http { fn drop(&mut self) { self.task.abort(); } }
async fn serve(router: Router) -> Http {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    Http { url, task: tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); }) }
}
#[derive(Default)]
struct Source { seen: Mutex<Vec<Value>>, reject_developer: bool, single_system: bool }
async fn source(State(source): State<Arc<Source>>, Json(body): Json<Value>) -> Response {
    source.seen.lock().unwrap().push(body.clone());
    let messages = body["messages"].as_array().unwrap();
    if source.reject_developer && messages.iter().any(|m| m["role"] == "developer") {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":{"message":"unknown variant developer","type":"invalid_request_error"}}))).into_response();
    }
    if source.single_system && messages.iter().enumerate().any(|(i,m)|m["role"]=="system" && i!=0) {
        return (StatusCode::BAD_REQUEST, Json(json!({"code":20015,"message":"System message must be at the beginning of the messages sequence."}))).into_response();
    }
    let tool = body["tools"].as_array().is_some_and(|tools| !tools.is_empty())
        && !messages.iter().any(|m| m["role"] == "tool");
    let message = if tool {
        json!({"role":"assistant","content":null,"reasoning_content":"I should call sum","tool_calls":[{"index":0,"id":"call_sum","type":"function","function":{"name":"sum","arguments":"{\"a\":1}"}}]})
    } else { json!({"role":"assistant","content":"ROLE_OK"}) };
    let finish = if tool { "tool_calls" } else { "stop" };
    let usage = json!({"prompt_tokens":7,"completion_tokens":3,"total_tokens":10});
    if body["stream"] == true {
        assert!(!tool, "tool fixture uses buffered mode");
        let chunks = [
            json!({"id":"chat_role","object":"chat.completion.chunk","created":1,"model":"deepseek-flash","choices":[{"index":0,"delta":{"role":"assistant","content":"ROLE_OK"},"finish_reason":null}]}),
            json!({"id":"chat_role","object":"chat.completion.chunk","created":1,"model":"deepseek-flash","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":usage}),
        ];
        let wire = chunks.iter().map(|v| format!("data: {v}\n\n")).collect::<String>() + "data: [DONE]\n\n";
        return ([("content-type","text/event-stream")], Body::from(wire)).into_response();
    }
    Json(json!({"id":"chat_role","object":"chat.completion","created":1,"model":"deepseek-flash","choices":[{"index":0,"message":message,"finish_reason":finish}],"usage":usage})).into_response()
}
struct Fixture {
    state: SharedState, gateway: Http, _source: Http, source: Arc<Source>,
    key: String, _user_id: i64, dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) { self.state.drain_writes(); std::fs::remove_dir_all(&self.dir).ok(); }
}
impl Fixture {
    async fn new(profile: &str, model: &str, allow_conversion: bool) -> Self {
        Self::with_reasoning_replay(profile, model, allow_conversion, true).await
    }
    async fn with_reasoning_replay(profile: &str, model: &str, allow_conversion: bool, chat_reasoning_replay: bool) -> Self {
        let source = Arc::new(Source { reject_developer: matches!(profile, "dashscope" | "moonshot" | "zai") || (profile=="siliconflow" && model=="Qwen/Qwen3.8-27B"), single_system:profile=="siliconflow" && model=="Qwen/Qwen3.8-27B", ..Default::default() });
        let upstream = serve(Router::new().route("/v1/chat/completions", post(source_handler)).with_state(source.clone())).await;
        let dir = std::env::temp_dir().join(format!("deepseek-roles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config { db_path: dir.join("test.db"), master_key_path: dir.join("master").to_string_lossy().into(), ..Default::default() }).unwrap();
        state.routing().cfg.retry_times = 0;
        state.routing().cfg.probe_enabled = false;
        let (key, user_id) = {
            let c = state.conn();
            let user = db::identity::create_user(&c, "role owner", "user", &Default::default()).unwrap();
            let (_, key) = db::identity::create_key(&c, user.id, "role key", &Default::default(), None, None).unwrap();
            let id = db::channels::create(&c, &db::channels::NewChannel { name: "DeepSeek fixture".into(), base_url: format!("{}/v1", upstream.url), enabled: true, unwrap_envelope: false, max_concurrent: 0 }).unwrap();
            db::channels::set_models(&c, id, &[db::channels::ModelMapping::new("DeepSeek-V4.1-Flash", Some(model.into()), true)]).unwrap();
            tokengo_server::protocol_config::save(&c, id.0, &Capabilities { parameter_profile: Some(profile.into()), native: vec![Protocol::Chat], allow_conversion, chat_reasoning_replay, max_input_tokens: Some(8192), max_output_tokens: Some(128), ..Default::default() }).unwrap();
            db::billing::create_price(&c, &db::billing::Price { id: 0, channel_id: id.0, model: "DeepSeek-V4.1-Flash".into(), input_micro_per_million: 1, output_micro_per_million: 1, max_input_tokens: 8192, max_output_tokens: 128 }, "role fixture").unwrap();
            (key, user.id)
        };
        let gateway = serve(app(state.clone())).await;
        Self { state, gateway, _source: upstream, source, key, _user_id: user_id, dir }
    }
    async fn call(&self, protocol: Protocol, body: Value) -> (u16, String) {
        let response = reqwest::Client::new().post(format!("{}/v1/{}", self.gateway.url, protocol.path())).bearer_auth(&self.key).json(&body).send().await.unwrap();
        (response.status().as_u16(), response.text().await.unwrap())
    }
}
// Avoid shadowing the source function with the fixture's source state binding.
async fn source_handler(state: State<Arc<Source>>, body: Json<Value>) -> Response { source(state, body).await }

fn role_request(messages: Value) -> Value {
    json!({"model":"DeepSeek-V4.1-Flash","messages":messages,"max_tokens":32})
}
#[tokio::test]
async fn provider_roles_gateway_outbound_text_order_and_fields() {
    for (profile,model) in [("dashscope","qwen3.8-max"),("moonshot","kimi-k3.1-pro"),("zai","glm-5.3")] {
        let f=Fixture::new(profile,model,true).await;
        for blocks in [false,true] {
            let content=if blocks { json!([{"type":"text","text":"one\n"},{"type":"text","text":"二"}]) } else {json!("one\n二")};
            let (code,wire)=f.call(Protocol::Chat,role_request(json!([
                {"role":"system","content":"root"},{"role":"developer","name":"application","content":content},
                {"role":"user","content":"hi"},{"role":"assistant","content":null,"tool_calls":[{"id":"call1","type":"function","function":{"name":"sum","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"call1","content":"1"}]))).await;
            assert_eq!(code,200,"{profile}/{model}: {wire}");
            let seen=f.source.seen.lock().unwrap();let sent=seen.last().unwrap();
            assert_eq!(sent["model"],model);
            assert_eq!(sent["messages"][0],json!({"role":"system","content":"root"}));
            assert_eq!(sent["messages"][1],json!({"role":"system","content":"one\n二","name":"application"}));
            assert_eq!(sent["messages"][3]["tool_calls"][0]["id"],"call1");
            assert_eq!(sent["messages"][4],json!({"role":"tool","tool_call_id":"call1","content":"1"}));
        }
    }
}
#[tokio::test]
async fn provider_roles_gateway_rejects_unrepresentable_messages_without_upstream() {
    for (profile,model) in [("dashscope","qwen3.8-max"),("moonshot","kimi-k3.1-pro"),("zai","glm-5.3")] {
        let f=Fixture::new(profile,model,true).await;
        for messages in [json!([{"role":"developer","content":"first"},{"role":"user","content":"hi"},{"role":"developer","content":"late"}]),
            json!([{"role":"developer","content":"first"},{"role":"developer","content":[{"type":"image_url","image_url":{"url":"https://example.com/i.png"}}]}]),
            json!([{"role":"developer","content":"first"},{"role":"developer","content":[{"type":"text","text":"x","unknown":true}]}])] {
            let (code,wire)=f.call(Protocol::Chat,role_request(messages)).await;
            assert_eq!(code,400,"{profile}: {wire}");assert!(wire.contains("developer"));
            assert!(f.source.seen.lock().unwrap().is_empty());
        }
    }
    for model in ["qwq-plus","qwq-32b","qvq-max","qvq-plus"] {
        let f=Fixture::new("dashscope",model,true).await;
        for role in ["system","developer"] {
            let (code,wire)=f.call(Protocol::Chat,role_request(json!([{ "role":role,"content":"rules"},{"role":"user","content":"hi"}]))).await;
            assert_eq!(code,400,"{model}: {wire}");assert!(f.source.seen.lock().unwrap().is_empty());
        }
    }
}
#[tokio::test]
async fn provider_roles_gateway_unknown_source_openai_and_siliconflow_preserve_developer() {
    for profile in ["native","openai","siliconflow"] {
        let f=Fixture::new(profile,"qwen3.8-max",true).await;
        let (code,wire)=f.call(Protocol::Chat,role_request(json!([{ "role":"developer","content":"rules"},{"role":"user","content":"hi"}]))).await;
        assert_eq!(code,200,"{profile}: {wire}");
        let seen=f.source.seen.lock().unwrap();assert_eq!(seen.len(),1);assert_eq!(seen[0]["messages"][0]["role"],"developer");
    }
}

#[tokio::test]
async fn provider_roles_gateway_runs_after_responses_to_chat_conversion() {
    for (profile,model) in [("dashscope","qwen3.8-max"),("moonshot","kimi-k3.1-pro"),("zai","glm-5.3")] {
        let f=Fixture::new(profile,model,true).await;
        let (code,wire)=f.call(Protocol::Responses,json!({"model":"DeepSeek-V4.1-Flash","instructions":"root",
            "input":[{"role":"developer","content":[{"type":"input_text","text":"rules"}]},{"role":"user","content":"hi"}],"max_output_tokens":32})).await;
        assert_eq!(code,200,"{profile}: {wire}");
        let seen=f.source.seen.lock().unwrap();assert_eq!(seen.len(),1);
        assert_eq!(seen[0]["messages"],json!([{"role":"system","content":"root"},{"role":"system","content":"rules"},{"role":"user","content":"hi"}]));
    }
}

#[tokio::test]
async fn provider_roles_gateway_unmatched_model_is_not_rewritten() {
    for (profile,model) in [("dashscope","qwen-future"),("dashscope","kimi-k3"),("moonshot","qwen3.8-max"),("zai","glm-future")] {
        let f=Fixture::new(profile,model,true).await;
        let (code,wire)=f.call(Protocol::Chat,role_request(json!([{ "role":"developer","content":"rules"},{"role":"user","content":"hi"}]))).await;
        // Fixture rejects the unchanged native role. Gateway must not guess a
        // different model's contract merely to obtain a successful response.
        assert!(code>=400,"{profile}/{model}: {wire}");
        let seen=f.source.seen.lock().unwrap();assert_eq!(seen.len(),1);assert_eq!(seen[0]["messages"][0]["role"],"developer");
    }
}


#[tokio::test]
async fn siliconflow_verified_model_folds_prefix_after_native_and_responses_conversion() {
    let f=Fixture::new("siliconflow","Qwen/Qwen3.8-27B",true).await;
    let chat=json!({"model":"DeepSeek-V4.1-Flash","reasoning_effort":"none","max_tokens":32,
        "messages":[{"role":"system","content":"root"},{"role":"developer","content":"rules"},
            {"role":"user","content":"hi"},{"role":"assistant","content":null,"tool_calls":[{"id":"call1","type":"function","function":{"name":"sum","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"call1","content":"1"}]});
    let responses=json!({"model":"DeepSeek-V4.1-Flash","reasoning":{"effort":"none"},"max_output_tokens":32,"instructions":"root",
        "input":[{"role":"developer","content":[{"type":"input_text","text":"rules"}]},{"role":"user","content":"hi"},
            {"type":"function_call","call_id":"call1","name":"sum","arguments":"{}"},
            {"type":"function_call_output","call_id":"call1","output":"1"}]});
    for (protocol,body) in [(Protocol::Chat,chat),(Protocol::Responses,responses)] {
        let (code,wire)=f.call(protocol,body).await;assert_eq!(code,200,"{protocol:?}: {wire}");
        let seen=f.source.seen.lock().unwrap();let sent=seen.last().unwrap();
        assert_eq!(sent["model"],"Qwen/Qwen3.8-27B");
        assert_eq!(sent["messages"][0],json!({"role":"system","content":"root\n\nrules"}));
        assert_eq!(sent["messages"][1],json!({"role":"user","content":"hi"}));
        assert_eq!(sent["messages"].as_array().unwrap().len(),4);
        assert_eq!(sent["messages"][2]["tool_calls"][0]["id"],"call1");
        assert_eq!(sent["messages"][3]["tool_call_id"],"call1");assert_eq!(sent["messages"][3]["content"],"1");
    }
}

#[tokio::test]
async fn siliconflow_verified_model_rejects_late_and_nontext_developer_before_transport() {
    let f=Fixture::new("siliconflow","Qwen/Qwen3.8-27B",true).await;
    for (protocol,body) in [
        (Protocol::Chat,json!({"model":"DeepSeek-V4.1-Flash","max_tokens":32,"reasoning_effort":"none","messages":[{"role":"developer","content":"first"},{"role":"user","content":"hi"},{"role":"developer","content":"late"}]})),
        (Protocol::Chat,json!({"model":"DeepSeek-V4.1-Flash","max_tokens":32,"reasoning_effort":"none","messages":[{"role":"developer","content":"first"},{"role":"developer","content":[{"type":"text","text":"rules","unknown":1}]}]})),
        (Protocol::Responses,json!({"model":"DeepSeek-V4.1-Flash","max_output_tokens":32,"reasoning":{"effort":"none"},"input":[{"role":"developer","content":"first"},{"role":"user","content":"hi"},{"role":"developer","content":"late"}]})),
    ] {
        let (code,wire)=f.call(protocol,body).await;assert_eq!(code,400,"{protocol:?}: {wire}");assert!(wire.contains("developer"));
        assert!(f.source.seen.lock().unwrap().is_empty());
    }
    f.state.drain_writes();
    let ledger:i64=f.state.conn().query_row("SELECT count(*) FROM billing_ledger",[],|r|r.get(0)).unwrap();assert_eq!(ledger,0);
}


#[tokio::test]
async fn siliconflow_single_system_prefix_and_rejects_unmergeable_metadata_without_transport() {
    let f=Fixture::new("siliconflow","Qwen/Qwen3.8-27B",true).await;
    let mut good=role_request(json!([{"role":"system","content":"root"},{"role":"system","content":[{"type":"text","text":"one"},{"type":"text","text":"two"}]},{"role":"user","content":"hi"}]));good["reasoning_effort"]=json!("none");
    let (code,wire)=f.call(Protocol::Chat,good).await;assert_eq!(code,200,"{wire}");
    assert_eq!(f.source.seen.lock().unwrap().last().unwrap()["messages"],json!([{"role":"system","content":"root\n\nonetwo"},{"role":"user","content":"hi"}]));
    f.source.seen.lock().unwrap().clear();
    for messages in [
        json!([{"role":"system","content":"root","name":"owner"},{"role":"developer","content":"rules"},{"role":"user","content":"hi"}]),
        json!([{"role":"system","content":"root"},{"role":"developer","content":"rules","unknown":1},{"role":"user","content":"hi"}]),
        json!([{"role":"system","content":[{"type":"image_url","image_url":{"url":"x"}}]},{"role":"developer","content":"rules"},{"role":"user","content":"hi"}]),
        json!([{"role":"user","content":"hi"},{"role":"system","content":"late"}]),
    ] {
        let mut body=role_request(messages);body["reasoning_effort"]=json!("none");
        let (code,wire)=f.call(Protocol::Chat,body).await;assert_eq!(code,400,"{wire}");assert!(f.source.seen.lock().unwrap().is_empty());
    }
    let rejected=Fixture::new("siliconflow","Qwen/Qwen3.8-27B",true).await;
    let (code,wire)=rejected.call(Protocol::Responses,json!({"model":"DeepSeek-V4.1-Flash","reasoning":{"effort":"none"},"max_output_tokens":32,"input":[{"role":"system","content":"root"},{"role":"developer","content":"rules","name":"app"},{"role":"user","content":"hi"}]})).await;
    assert_eq!(code,400,"{wire}");assert!(rejected.source.seen.lock().unwrap().is_empty());
    rejected.state.drain_writes();let ledger:i64=rejected.state.conn().query_row("SELECT count(*) FROM billing_ledger",[],|r|r.get(0)).unwrap();assert_eq!(ledger,0);
}
