//! Source-scoped DeepSeek roles through real gateway HTTP and protocol conversion.
//! The local source intentionally rejects developer, reproducing the official 422.
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
struct Source { seen: Mutex<Vec<Value>>, reject_developer: bool }
async fn source(State(source): State<Arc<Source>>, Json(body): Json<Value>) -> Response {
    source.seen.lock().unwrap().push(body.clone());
    let messages = body["messages"].as_array().unwrap();
    if source.reject_developer && messages.iter().any(|m| m["role"] == "developer") {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":{"message":"unknown variant developer","type":"invalid_request_error"}}))).into_response();
    }
    let tool = body["tools"].as_array().is_some_and(|tools| !tools.is_empty())
        && !messages.iter().any(|m| m["role"] == "tool");
    let message = if tool {
        json!({"role":"assistant","content":null,"reasoning_content":"I should call sum","tool_calls":[{"id":"call_sum","type":"function","function":{"name":"sum","arguments":"{\"a\":1}"}}]})
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
    key: String, user_id: i64, dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) { self.state.drain_writes(); std::fs::remove_dir_all(&self.dir).ok(); }
}
impl Fixture {
    async fn new(profile: &str, allow_conversion: bool) -> Self {
        Self::with_reasoning_replay(profile, allow_conversion, true).await
    }
    async fn with_reasoning_replay(profile: &str, allow_conversion: bool, chat_reasoning_replay: bool) -> Self {
        let source = Arc::new(Source { reject_developer: profile == "deepseek", ..Default::default() });
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
            db::channels::set_models(&c, id, &[db::channels::ModelMapping::new("DeepSeek-V4.1-Flash", Some("deepseek-flash".into()), false)]).unwrap();
            tokengo_server::protocol_config::save(&c, id.0, &Capabilities { parameter_profile: Some(profile.into()), native: vec![Protocol::Chat], allow_conversion, chat_reasoning_replay, ..Default::default() }).unwrap();
            db::billing::create_price(&c, &db::billing::Price { id: 0, channel_id: id.0, model: "DeepSeek-V4.1-Flash".into(), input_micro_per_million: 1, output_micro_per_million: 1, max_input_tokens: 8192, max_output_tokens: 128 }, "role fixture").unwrap();
            (key, user.id)
        };
        let gateway = serve(app(state.clone())).await;
        Self { state, gateway, _source: upstream, source, key, user_id, dir }
    }
    async fn call(&self, protocol: Protocol, body: Value) -> (u16, String) {
        let response = reqwest::Client::new().post(format!("{}/v1/{}", self.gateway.url, protocol.path())).bearer_auth(&self.key).json(&body).send().await.unwrap();
        (response.status().as_u16(), response.text().await.unwrap())
    }
}
// Avoid shadowing the source function with the fixture's source state binding.
async fn source_handler(state: State<Arc<Source>>, body: Json<Value>) -> Response { source(state, body).await }
fn request(protocol: Protocol, stream: bool, blocks: bool) -> Value {
    let developer = if blocks { json!([{"type":"text","text":"Be concise."},{"type":"text","text":" Keep numbers."}]) } else { json!("Be concise. Keep numbers.") };
    if protocol == Protocol::Chat {
        json!({"model":"DeepSeek-V4.1-Flash","messages":[{"role":"system","content":"System instruction."},{"role":"developer","content":developer},{"role":"user","content":"hello"}],"max_tokens":32,"stream":stream})
    } else {
        let developer = if blocks { json!([{"type":"input_text","text":"Be concise."},{"type":"input_text","text":" Keep numbers."}]) } else { json!("Be concise. Keep numbers.") };
        json!({"model":"DeepSeek-V4.1-Flash","instructions":"System instruction.","input":[{"role":"developer","content":developer},{"role":"user","content":"hello"}],"max_output_tokens":32,"stream":stream})
    }
}
fn events(wire: &str) -> Vec<Value> {
    wire.lines().filter_map(|line| line.strip_prefix("data: ")).filter(|line| *line != "[DONE]").map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[tokio::test]
async fn deepseek_chat_and_responses_preserve_instructions_alias_usage_and_wire_formats() {
    for protocol in [Protocol::Chat, Protocol::Responses] {
        for stream in [false, true] {
            for blocks in [false, true] {
                let f = Fixture::new("deepseek", true).await;
                let (code, wire) = f.call(protocol, request(protocol, stream, blocks)).await;
                assert_eq!(code, 200, "{protocol:?} stream={stream} blocks={blocks}: {wire}");
                if stream {
                    let values = events(&wire);
                    if protocol == Protocol::Chat {
                        assert!(wire.contains("data: [DONE]"));
                        assert_eq!(values.iter().find_map(|v| v.pointer("/choices/0/delta/content").and_then(Value::as_str)), Some("ROLE_OK"));
                        assert_eq!(values.last().unwrap()["usage"]["total_tokens"], 10);
                    } else {
                        let done = values.iter().find(|v| v["type"] == "response.completed").expect("complete response event");
                        assert_eq!(done["response"]["status"], "completed");
                        assert_eq!(done["response"]["usage"]["total_tokens"], 10);
                        assert!(values.iter().any(|v| v["type"] == "response.output_text.delta" && v["delta"] == "ROLE_OK"));
                    }
                } else {
                    let value: Value = serde_json::from_str(&wire).unwrap();
                    assert_eq!(value["usage"]["total_tokens"], 10);
                    if protocol == Protocol::Chat { assert_eq!(value["choices"][0]["message"]["content"], "ROLE_OK"); }
                    else { assert_eq!(value["object"], "response"); assert_eq!(value["status"], "completed"); assert_eq!(value["output"][0]["content"][0]["text"], "ROLE_OK"); }
                }
                let seen = f.source.seen.lock().unwrap();
                assert_eq!(seen.len(), 1);
                assert_eq!(seen[0]["model"], "deepseek-flash");
                assert_eq!(seen[0]["messages"], json!([{"role":"system","content":"System instruction."},{"role":"system","content":"Be concise. Keep numbers."},{"role":"user","content":"hello"}]));
                assert!(seen[0].get("input").is_none());
            }
        }
    }
}

#[tokio::test]
async fn deepseek_responses_function_call_and_output_history_round_trip() {
    let f = Fixture::new("deepseek", true).await;
    let mut first = request(Protocol::Responses, false, false);
    let tools = json!([{"type":"function","name":"sum","description":"Sum a number","parameters":{"type":"object","properties":{"a":{"type":"integer"}},"required":["a"],"additionalProperties":false}}]);
    first["tools"] = tools.clone();
    let (code, wire) = f.call(Protocol::Responses, first.clone()).await;
    assert_eq!(code, 200, "{wire}");
    let response: Value = serde_json::from_str(&wire).unwrap();
    let output = response["output"].as_array().unwrap();
    let reasoning = output.iter().find(|v| v["type"] == "reasoning").expect("reasoning returned before tool call");
    assert_eq!(reasoning["content"][0]["text"], "I should call sum");
    assert_eq!(reasoning["summary"], json!([]));
    let call = output.iter().find(|v| v["type"] == "function_call").unwrap().clone();
    assert_eq!(call["call_id"], "call_sum");
    assert_eq!(call["name"], "sum");
    assert_eq!(call["arguments"], "{\"a\":1}");
    let mut next = first;
    // Replay all returned items exactly, including the plaintext reasoning item.
    next["input"].as_array_mut().unwrap().extend(output.iter().cloned());
    next["input"].as_array_mut().unwrap().push(json!({"type":"function_call_output","call_id":"call_sum","output":"1"}));
    let (code, wire) = f.call(Protocol::Responses, next).await;
    assert_eq!(code, 200, "{wire}");
    let response: Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(response["status"], "completed");
    assert_eq!(response["output"][0]["content"][0]["text"], "ROLE_OK");
    let seen = f.source.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let messages = seen[1]["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "system");
    assert_eq!(messages[3]["reasoning_content"], "I should call sum");
    assert_eq!(messages[3]["tool_calls"][0]["id"], "call_sum");
    assert_eq!(messages[4], json!({"role":"tool","tool_call_id":"call_sum","content":"1"}));
}


#[tokio::test]
async fn converted_reasoning_replay_requires_explicit_source_capability() {
    let f = Fixture::with_reasoning_replay("deepseek", true, false).await;
    let mut body = request(Protocol::Responses, false, false);
    body["input"].as_array_mut().unwrap().extend([
        json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"I should call sum"}]}),
        json!({"type":"function_call","call_id":"call_sum","name":"sum","arguments":"{\"a\":1}"}),
        json!({"type":"function_call_output","call_id":"call_sum","output":"1"}),
    ]);
    let (code, wire) = f.call(Protocol::Responses, body).await;
    assert_eq!(code, 400, "{wire}");
    assert!(wire.contains("reasoning"), "expected reasoning replay rejection: {wire}");
    assert!(f.source.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn conversion_disabled_still_rejects_before_upstream() {
    let f = Fixture::new("deepseek", false).await;
    let (code, wire) = f.call(Protocol::Responses, request(Protocol::Responses, false, false)).await;
    assert_eq!(code, 400, "{wire}");
    assert!(wire.contains("does not declare this protocol"));
    assert!(f.source.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn openai_profile_preserves_native_developer_role() {
    let f = Fixture::new("openai", true).await;
    let (code, wire) = f.call(Protocol::Chat, request(Protocol::Chat, false, false)).await;
    assert_eq!(code, 200, "{wire}");
    let seen = f.source.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["messages"][1]["role"], "developer");
    assert_eq!(seen[0]["messages"][1]["content"], "Be concise. Keep numbers.");
}

#[tokio::test]
async fn deepseek_role_adapter_does_not_bypass_model_permissions_or_opaque_checks() {
    let f = Fixture::new("deepseek", true).await;
    {
        let c = f.state.conn();
        let mut user = db::identity::user(&c, f.user_id).unwrap().unwrap();
        user.policy.allowed_models = Some(vec![]);
        db::identity::update_user(&c, &user).unwrap();
    }
    let (code, wire) = f.call(Protocol::Chat, request(Protocol::Chat, false, false)).await;
    assert_eq!(code, 403, "{wire}");
    assert!(f.source.seen.lock().unwrap().is_empty());
    {
        let c = f.state.conn();
        let mut user = db::identity::user(&c, f.user_id).unwrap().unwrap();
        user.policy.allowed_models = None;
        db::identity::update_user(&c, &user).unwrap();
    }
    let mut body = request(Protocol::Responses, false, false);
    body["input"].as_array_mut().unwrap().push(json!({"type":"reasoning","id":"foreign_reasoning","summary":[],"encrypted_content":"unowned opaque value"}));
    let (code, wire) = f.call(Protocol::Responses, body).await;
    assert_eq!(code, 400, "{wire}");
    assert!(f.source.seen.lock().unwrap().is_empty());
}
