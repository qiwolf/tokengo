//! Real HTTP protocol selection contracts. No external providers or credentials.
use axum::{
    body::Body,
    extract::{OriginalUri, State},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{
    app, db, protocol::Protocol, protocol_config::Capabilities, AppState, Config, SharedState,
};
struct Http {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Http {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(r: Router) -> Http {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    Http {
        url,
        task: tokio::spawn(async move { axum::serve(l, r).await.unwrap() }),
    }
}
#[derive(Default)]
struct Source {
    seen: Mutex<Vec<(String, Value)>>,
}
async fn source(
    State(s): State<Arc<Source>>,
    OriginalUri(uri): OriginalUri,
    Json(v): Json<Value>,
) -> Response {
    s.seen.lock().unwrap().push((uri.path().into(), v.clone()));
    if v["provider_fail"] == true {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":{"message":"synthetic source unavailable","type":"server_error"}}))).into_response();
    }
    let responses = uri.path().ends_with("responses");
    let mut value = if responses {
        json!({"id":"resp_matrix","object":"response","status":"completed","model":"upstream","output":[{"id":"msg_matrix","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"matrix answer","annotations":[]}]}],"usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}})
    } else {
        json!({"id":"chat_matrix","object":"chat.completion","model":"upstream","choices":[{"index":0,"message":{"role":"assistant","content":"matrix answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}})
    };
    if responses && v["provider_opaque"] == true {
        value["output"].as_array_mut().unwrap().insert(0,json!({"id":"rs_matrix","type":"reasoning","summary":[],"encrypted_content":"test-opaque-state"}));
    }
    if responses && v["provider_tool"] == true {
        value["output"].as_array_mut().unwrap().push(json!({
            "id":"fc_matrix","type":"function_call","status":"completed",
            "call_id":"call_matrix","name":"sum","arguments":"{\"a\":1}"
        }));
    }
    if v["stream"] == true {
        let data = if responses {
            format!("event: response.created\ndata: {}\n\nevent: response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",json!({"type":"response.created","response":{"id":"resp_matrix","object":"response","status":"in_progress"}}),json!({"type":"response.output_text.delta","item_id":"msg_matrix","output_index":0,"content_index":0,"delta":"matrix answer"}),json!({"type":"response.completed","response":value}))
        } else {
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"id":"chat_matrix","object":"chat.completion.chunk","model":"upstream","choices":[{"index":0,"delta":{"role":"assistant","content":"matrix answer"},"finish_reason":null}]}),
                json!({"id":"chat_matrix","object":"chat.completion.chunk","model":"upstream","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}})
            )
        };
        return ([("content-type", "text/event-stream")], Body::from(data)).into_response();
    }
    Json(value).into_response()
}
struct Fixture {
    state: SharedState,
    gateway: Http,
    _source: Http,
    seen: Arc<Source>,
    key: String,
    channel_id: i64,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.drain_writes();
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
impl Fixture {
    async fn new(native: Vec<Protocol>, allow_conversion: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-matrix-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            ..Default::default()
        })
        .unwrap();
        state.routing().cfg.retry_times = 0;
        state.routing().cfg.probe_enabled = false;
        let seen = Arc::new(Source::default());
        let upstream = serve(
            Router::new()
                .route("/v1/chat/completions", post(source))
                .route("/v1/responses", post(source))
                .with_state(seen.clone()),
        )
        .await;
        let (key, channel_id) = {
            let c = state.conn();
            let (_, key) =
                db::identity::create_key(&c, 1, "matrix", &Default::default(), None, None).unwrap();
            let id = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "matrix source".into(),
                    base_url: format!("{}/v1", upstream.url),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap();
            db::channels::set_models(
                &c,
                id,
                &[db::channels::ModelMapping::new(
                    "matrix",
                    Some("upstream".into()),
                    false,
                )],
            )
            .unwrap();
            tokengo_server::protocol_config::save(
                &c,
                id.0,
                &Capabilities {
                    native,
                    allow_conversion,
                    ..Default::default()
                },
            )
            .unwrap();
            (key, id.0)
        };
        let gateway = serve(app(state.clone())).await;
        Self {
            state,
            gateway,
            _source: upstream,
            seen,
            key,
            channel_id,
            dir,
        }
    }
    async fn call(&self, p: Protocol, body: Value) -> (u16, String) {
        self.call_with_key(p, &self.key, body).await
    }
    async fn call_with_key(&self, p: Protocol, key: &str, body: Value) -> (u16, String) {
        self.call_with_headers(p, key, &[], body).await
    }
    async fn call_with_headers(&self, p: Protocol, key: &str, headers: &[(&str, &str)], body: Value) -> (u16, String) {
        let mut request = reqwest::Client::new()
            .post(format!("{}/v1/{}", self.gateway.url, p.path()))
            .bearer_auth(key)
            .json(&body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let r = request.send().await.unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    }
}
fn body(p: Protocol, stream: bool) -> Value {
    if p == Protocol::Chat {
        json!({"model":"matrix","messages":[{"role":"user","content":"hello"}],"max_tokens":16,"stream":stream})
    } else {
        json!({"model":"matrix","input":"hello","max_output_tokens":16,"stream":stream})
    }
}
#[tokio::test]
async fn chat_only_consumer_keeps_chat_native_for_chat_only_or_dual_upstream() {
    for native in [
        vec![Protocol::Chat],
        vec![Protocol::Responses, Protocol::Chat],
    ] {
        for stream in [false, true] {
            let f = Fixture::new(native.clone(), true).await;
            let mut b = body(Protocol::Chat, stream);
            b["provider_specific"] = json!({"opaque":"keep-native"});
            let (code, text) = f.call(Protocol::Chat, b).await;
            assert_eq!(code, 200, "{text}");
            assert!(text.contains("matrix answer"));
            let seen = f.seen.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].0, "/v1/chat/completions");
            assert_eq!(seen[0].1["provider_specific"]["opaque"], "keep-native");
        }
    }
}
#[tokio::test]
async fn dual_consumer_chooses_its_endpoint_and_dual_upstream_preserves_it() {
    let f = Fixture::new(vec![Protocol::Responses, Protocol::Chat], true).await;
    for p in [Protocol::Chat, Protocol::Responses] {
        for stream in [false, true] {
            let (code, text) = f.call(p, body(p, stream)).await;
            assert_eq!(code, 200, "{text}");
            assert!(text.contains("matrix answer"));
            assert_eq!(
                f.seen.seen.lock().unwrap().last().unwrap().0,
                format!("/v1/{}", p.path())
            );
        }
    }
    assert_eq!(f.seen.seen.lock().unwrap().len(), 4);
}
#[tokio::test]
async fn dual_consumer_responses_request_bridges_to_chat_only_upstream_when_enabled() {
    for stream in [false, true] {
        let f = Fixture::new(vec![Protocol::Chat], true).await;
        let mut b = body(Protocol::Responses, stream);
        b["instructions"] = json!("system instruction");
        b["reasoning"] = json!({"effort":"high"});
        b["service_tier"] = json!("priority");
        let (code, text) = f.call(Protocol::Responses, b).await;
        assert_eq!(code, 200, "{text}");
        assert!(text.contains("matrix answer"));
        if stream {
            assert!(text.contains("response.completed"));
        } else {
            let v: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["object"], "response");
        }
        let seen = f.seen.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "/v1/chat/completions");
        assert_eq!(
            seen[0].1["messages"],
            json!([{"role":"system","content":"system instruction"},{"role":"user","content":"hello"}])
        );
        assert_eq!(seen[0].1["reasoning_effort"], "high");
        assert_eq!(seen[0].1["service_tier"], "priority");
        assert!(seen[0].1.get("input").is_none());
    }
}
#[tokio::test]
async fn chat_consumer_bridges_to_responses_only_upstream_when_enabled() {
    for stream in [false, true] {
        let f = Fixture::new(vec![Protocol::Responses], true).await;
        let (code, text) = f.call(Protocol::Chat, body(Protocol::Chat, stream)).await;
        assert_eq!(code, 200, "{text}");
        assert!(text.contains("matrix answer"));
        let seen = f.seen.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "/v1/responses");
        assert_eq!(seen[0].1["store"], false);
        assert!(seen[0].1.get("messages").is_none());
    }
}
#[tokio::test]
async fn unsupported_protocol_with_conversion_off_never_calls_upstream() {
    for (native, p) in [
        (Protocol::Chat, Protocol::Responses),
        (Protocol::Responses, Protocol::Chat),
    ] {
        let f = Fixture::new(vec![native], false).await;
        let (code, _) = f.call(p, body(p, false)).await;
        assert_eq!(code, 400);
        assert!(f.seen.seen.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn nonrepresentable_responses_fields_are_rejected_without_hiding_them_in_user_text() {
    for (key, value) in [
        ("previous_response_id", json!("resp_opaque")),
        ("store", json!(true)),
        ("tools", json!([{"type":"web_search_preview"}])),
        ("reasoning", json!({"summary":"auto"})),
        ("include", json!(["reasoning.encrypted_content"])),
    ] {
        let f = Fixture::new(vec![Protocol::Chat], true).await;
        let mut b = body(Protocol::Responses, false);
        b[key] = value;
        let (code, text) = f.call(Protocol::Responses, b).await;
        assert_eq!(code, 400, "{key}: {text}");
        assert!(
            f.seen.seen.lock().unwrap().is_empty(),
            "{key} was incorrectly forwarded"
        );
    }
}
#[tokio::test]
async fn native_responses_does_not_strip_its_native_controls() {
    let f = Fixture::new(vec![Protocol::Responses, Protocol::Chat], true).await;
    let mut b = body(Protocol::Responses, false);
    b["reasoning"] = json!({"effort":"high","summary":"auto"});
    b["store"] = json!(true);
    let (code, text) = f.call(Protocol::Responses, b.clone()).await;
    assert_eq!(code, 200, "{text}");
    let seen = f.seen.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    for key in ["reasoning", "store"] {
        assert_eq!(seen[0].1[key], b[key]);
    }
}

#[tokio::test]
async fn native_responses_opaque_output_is_preserved_without_chat_fabrication() {
    let f = Fixture::new(vec![Protocol::Responses, Protocol::Chat], true).await;
    let mut b = body(Protocol::Responses, false);
    b["provider_opaque"] = json!(true);
    let (code, text) = f.call(Protocol::Responses, b).await;
    assert_eq!(code, 200, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["output"][0]["encrypted_content"], "test-opaque-state");
    assert!(v.get("choices").is_none());
    assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_hosted_tools_are_explicitly_rejected_by_metering_boundary() {
    let f = Fixture::new(vec![Protocol::Responses, Protocol::Chat], true).await;
    let mut b = body(Protocol::Responses, false);
    b["tools"] = json!([{"type":"web_search_preview"}]);
    let (code, text) = f.call(Protocol::Responses, b).await;
    assert_eq!(code, 400, "{text}");
    assert!(text.contains("separate metering"));
    assert!(f.seen.seen.lock().unwrap().is_empty());
}

fn prompt_cache_first(stream: bool) -> Value {
    json!({"model":"matrix","input":[{"role":"user","content":"compute sum"}],
        "max_output_tokens":16,"stream":stream,"store":false,
        "prompt_cache_key":"shared-cache-bucket","client_metadata":{"session_id":"real-session-a"},
        "include":["reasoning.encrypted_content"],
        "provider_opaque":true,"provider_tool":true,
        "tools":[{"type":"function","name":"sum","parameters":{"type":"object","properties":{"a":{"type":"integer"}}}}]})
}
fn completed_response(text: &str, stream: bool) -> Value {
    if !stream {
        return serde_json::from_str(text).unwrap();
    }
    text.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|event| event["type"] == "response.completed")
        .expect("stream must complete before replay")["response"]
        .clone()
}
fn prompt_cache_replay(first: &Value, stream: bool) -> Value {
    let mut replay = prompt_cache_first(stream);
    let mut input = vec![json!({"role":"user","content":"compute sum"})];
    input.extend(first["output"].as_array().unwrap().iter().cloned());
    input.push(json!({"type":"function_call_output","call_id":"call_matrix","output":"2"}));
    replay["input"] = json!(input);
    replay["provider_opaque"] = json!(false);
    replay["provider_tool"] = json!(false);
    replay
}
async fn prompt_cache_fixture() -> Fixture {
    let f = Fixture::new(vec![Protocol::Responses], false).await;
    let mut caps = tokengo_server::protocol_config::load(&f.state.conn(), f.channel_id).unwrap();
    caps.max_input_tokens = Some(4096);
    caps.max_output_tokens = Some(64);
    tokengo_server::protocol_config::save(&f.state.conn(), f.channel_id, &caps).unwrap();
    f
}

#[tokio::test]
async fn responses_real_session_preserves_opaque_tool_history_in_both_transport_modes() {
    // Both rounds may independently choose buffered JSON or SSE.
    for first_stream in [false, true] {
        for second_stream in [false, true] {
            let f = prompt_cache_fixture().await;
            let (code, text) = f.call(Protocol::Responses, prompt_cache_first(first_stream)).await;
            assert_eq!(code, 200, "first stream={first_stream}: {text}");
            let first = completed_response(&text, first_stream);
            assert_eq!(first["output"][0]["encrypted_content"], "test-opaque-state");
            assert_eq!(first["output"][2]["call_id"], "call_matrix");
            let mut replay = prompt_cache_replay(&first, second_stream);
            replay["prompt_cache_key"] = json!("cache-after-prefix-change");
            let (code, text) = f.call(Protocol::Responses, replay.clone()).await;
            assert_eq!(code, 200, "first stream={first_stream}, second stream={second_stream}: {text}");
            assert_eq!(completed_response(&text, second_stream)["status"], "completed");
            let seen = f.seen.seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert_eq!(seen[1].0, "/v1/responses");
            assert_eq!(seen[1].1["prompt_cache_key"], "cache-after-prefix-change");
            assert_eq!(seen[1].1["client_metadata"]["session_id"], "real-session-a");
            assert_eq!(seen[1].1["input"], replay["input"], "opaque reasoning and tool identities must reach the native source unchanged");
        }
    }
}

#[tokio::test]
async fn responses_real_session_cannot_borrow_another_key_or_shared_cache_conversation_binding() {
    for stream in [false, true] {
        let f = prompt_cache_fixture().await;
        let (code, text) = f.call(Protocol::Responses, prompt_cache_first(stream)).await;
        assert_eq!(code, 200, "{text}");
        let replay = prompt_cache_replay(&completed_response(&text, stream), stream);
        let (_, other_key) = db::identity::create_key(
            &f.state.conn(), 1, "other-cache-client", &Default::default(), None, None,
        ).unwrap();
        let (code, text) = f.call_with_key(Protocol::Responses, &other_key, replay.clone()).await;
        assert_eq!(code, 400, "another API key must not borrow opaque history: {text}");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
        let mut unidentified = replay.clone();
        unidentified.as_object_mut().unwrap().remove("client_metadata");
        assert!(unidentified.get("prompt_cache_key").is_some(), "cache hint remains present but cannot establish identity");
        let (code, text) = f.call(Protocol::Responses, unidentified).await;
        assert_eq!(code, 400, "opaque replay without an explicit identity must fail: {text}");
        let error: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(error["error"]["type"], "invalid_request_error");
        assert_eq!(error["error"]["code"], "explicit_session_required");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 1);

        // Establish an independent source pin, without issuing the original opaque artifact.
        let mut other_first = prompt_cache_first(stream);
        other_first["client_metadata"]["session_id"] = json!("real-session-b");
        other_first["provider_opaque"] = json!(false);
        other_first["provider_tool"] = json!(false);
        let (code, text) = f.call(Protocol::Responses, other_first).await;
        assert_eq!(code, 200, "another real session sharing the cache bucket may establish its own pin: {text}");
        assert_eq!(completed_response(&text, stream)["status"], "completed");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 2);
        let mut other_conversation = replay.clone();
        other_conversation["client_metadata"]["session_id"] = json!("real-session-b");
        let (code, text) = f.call(Protocol::Responses, other_conversation).await;
        assert_eq!(code, 400, "another real session sharing the cache key must not borrow opaque history: {text}");
        let error: Value = serde_json::from_str(&text).unwrap();
        assert!(error["error"]["message"].as_str().unwrap().contains("artifacts not issued"), "{text}");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 2);
        // Rejected replays must not damage the original binding.
        let (code, text) = f.call(Protocol::Responses, replay).await;
        assert_eq!(code, 200, "original conversation must remain usable: {text}");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn responses_real_session_rejects_opaque_replay_after_source_generation_changes() {
    for stream in [false, true] {
        let f = prompt_cache_fixture().await;
        let (code, text) = f.call(Protocol::Responses, prompt_cache_first(stream)).await;
        assert_eq!(code, 200, "{text}");
        let replay = prompt_cache_replay(&completed_response(&text, stream), stream);
        db::channels::set_secret(
            &f.state.conn(), &f.state.secrets, tokengo_shared::ChannelId(f.channel_id),
            "ROTATED-SYNTHETIC-CREDENTIAL",
        ).unwrap();
        let (code, text) = f.call(Protocol::Responses, replay).await;
        assert_eq!(code, 400, "credential generation change must invalidate opaque ownership: {text}");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 1, "reject before a source attempt");
    }
}

#[tokio::test]
async fn responses_real_session_header_survives_cache_prefix_changes_and_isolates_shared_cache_keys() {
    for stream in [false, true] {
        let f = prompt_cache_fixture().await;
        let owner = [("session_id", "real-conversation-a")];
        let other = [("session_id", "real-conversation-b")];
        let mut first_request = prompt_cache_first(stream);
        first_request["instructions"] = json!("Compute the requested sum.");
        let (code, text) = f.call_with_headers(
            Protocol::Responses, &f.key, &owner, first_request,
        ).await;
        assert_eq!(code, 200, "{text}");
        let first = completed_response(&text, stream);
        let mut replay = prompt_cache_replay(&first, stream);
        replay["prompt_cache_key"] = json!("cache-after-prefix-change");
        replay["instructions"] = json!("Compute the sum and explain the result concisely.");
        replay["tools"][0]["description"] = json!("Sum integers; refreshed tool description.");
        let (code, text) = f.call_with_headers(
            Protocol::Responses, &f.key, &owner, replay.clone(),
        ).await;
        assert_eq!(code, 200, "real session header must preserve ownership when the cache prefix changes: {text}");
        assert_eq!(completed_response(&text, stream)["status"], "completed");
        {
            let seen = f.seen.seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert_ne!(seen[0].1["prompt_cache_key"], seen[1].1["prompt_cache_key"]);
            assert_ne!(seen[0].1["instructions"], seen[1].1["instructions"]);
            assert_ne!(seen[0].1["tools"], seen[1].1["tools"]);
            assert_eq!(seen[1].1["input"], replay["input"], "opaque reasoning and tool output must be unchanged");
        }
        // A second real session may share the cache bucket, but must establish its own ownership.
        let mut other_first = replay.clone();
        other_first["input"] = json!([{"role":"user","content":"a separate computation"}]);
        let (code, text) = f.call_with_headers(
            Protocol::Responses, &f.key, &other, other_first,
        ).await;
        assert_eq!(code, 200, "independent real session must establish its own pin: {text}");
        assert_eq!(completed_response(&text, stream)["status"], "completed");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 3);
        let (code, text) = f.call_with_headers(
            Protocol::Responses, &f.key, &other, replay,
        ).await;
        assert_eq!(code, 400, "shared cache key cannot override distinct real session ownership: {text}");
        let error: Value = serde_json::from_str(&text).unwrap();
        assert!(error["error"]["message"].as_str().unwrap().contains("artifacts not issued"), "{text}");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 3, "foreign artifacts must fail before the source attempt");
    }
}


fn anonymous_request(mut request: Value, cache_hint: bool) -> Value {
    request.as_object_mut().unwrap().remove("client_metadata");
    if !cache_hint {
        request.as_object_mut().unwrap().remove("prompt_cache_key");
    }
    request
}

#[tokio::test]
async fn responses_anonymous_opaque_tool_replay_preserves_json_sse_and_cache_changes() {
    for cache_hint in [false, true] {
        for first_stream in [false, true] {
            for second_stream in [false, true] {
                let f = prompt_cache_fixture().await;
                let first_request = anonymous_request(prompt_cache_first(first_stream), cache_hint);
                let (code, text) = f.call(Protocol::Responses, first_request).await;
                assert_eq!(code, 200, "fresh anonymous request: {text}");
                let first = completed_response(&text, first_stream);
                assert_eq!(first["output"][0]["encrypted_content"], "test-opaque-state");
                let mut replay = anonymous_request(prompt_cache_replay(&first, second_stream), cache_hint);
                if cache_hint {
                    replay["prompt_cache_key"] = json!("cache-after-prefix-change");
                }
                replay["instructions"] = json!("Updated instructions must not change artifact ownership.");
                replay["tools"][0]["description"] = json!("Updated sum description.");
                let (code, text) = f.call(Protocol::Responses, replay.clone()).await;
                assert_eq!(code, 200, "cache={cache_hint}, first SSE={first_stream}, second SSE={second_stream}: {text}");
                assert_eq!(completed_response(&text, second_stream)["status"], "completed");
                let seen = f.seen.seen.lock().unwrap();
                assert_eq!(seen.len(), 2);
                assert_eq!(seen[1].1["input"], replay["input"], "reasoning and tool identities must reach their original source unchanged");
                assert_eq!(seen[1].1["prompt_cache_key"], replay["prompt_cache_key"]);
                assert!(seen[1].1.get("client_metadata").is_none());
            }
        }
    }
}

#[tokio::test]
async fn responses_anonymous_opaque_replay_rejects_foreign_key_unknown_artifacts_and_explicit_session() {
    for stream in [false, true] {
        let f = prompt_cache_fixture().await;
        let (code, text) = f.call(Protocol::Responses, anonymous_request(prompt_cache_first(stream), true)).await;
        assert_eq!(code, 200, "{text}");
        let replay = anonymous_request(prompt_cache_replay(&completed_response(&text, stream), stream), true);
        let (_, other_key) = db::identity::create_key(
            &f.state.conn(), 1, "anonymous-other-key", &Default::default(), None, None,
        ).unwrap();
        let (code, text) = f.call_with_key(Protocol::Responses, &other_key, replay.clone()).await;
        assert_eq!(code, 400, "another key cannot borrow anonymous artifacts: {text}");
        let mut unknown = replay.clone();
        unknown["input"][1]["encrypted_content"] = json!("never-issued-opaque-state");
        let (code, text) = f.call(Protocol::Responses, unknown).await;
        assert_eq!(code, 400, "unknown history cannot establish ownership: {text}");
        let (code, text) = f.call_with_headers(
            Protocol::Responses, &f.key, &[("x-session-id", "new-explicit-session")], replay.clone(),
        ).await;
        assert_eq!(code, 400, "explicit identity must not fall back to anonymous ownership: {text}");
        // Even an established explicit session cannot import anonymous artifacts.
        let mut explicit_first = prompt_cache_first(stream);
        explicit_first["provider_opaque"] = json!(false);
        explicit_first["provider_tool"] = json!(false);
        let (code, text) = f.call(Protocol::Responses, explicit_first).await;
        assert_eq!(code, 200, "{text}");
        let mut explicit_replay = replay.clone();
        explicit_replay["client_metadata"] = json!({"session_id":"real-session-a"});
        let (code, text) = f.call(Protocol::Responses, explicit_replay).await;
        assert_eq!(code, 400, "established explicit identity cannot import anonymous artifacts: {text}");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 2, "all invalid replays fail before outbound attempts");
        let (code, text) = f.call(Protocol::Responses, replay).await;
        assert_eq!(code, 200, "rejections must preserve anonymous ownership: {text}");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn responses_anonymous_opaque_replay_rejects_changed_source_generation() {
    for stream in [false, true] {
        let f = prompt_cache_fixture().await;
        let (code, text) = f.call(Protocol::Responses, anonymous_request(prompt_cache_first(stream), false)).await;
        assert_eq!(code, 200, "{text}");
        let replay = anonymous_request(prompt_cache_replay(&completed_response(&text, stream), stream), false);
        db::channels::set_secret(
            &f.state.conn(), &f.state.secrets, tokengo_shared::ChannelId(f.channel_id),
            "ROTATED-ANONYMOUS-SYNTHETIC-CREDENTIAL",
        ).unwrap();
        let (code, text) = f.call(Protocol::Responses, replay).await;
        assert_eq!(code, 400, "changed credential generation cannot inherit anonymous artifacts: {text}");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn responses_anonymous_opaque_replay_rejects_replacement_source() {
    let f = prompt_cache_fixture().await;
    let (code, text) = f.call(Protocol::Responses, anonymous_request(prompt_cache_first(false), false)).await;
    assert_eq!(code, 200, "{text}");
    let replay = anonymous_request(prompt_cache_replay(&completed_response(&text, false), false), false);
    {
        let c = f.state.conn();
        db::channels::set_models(&c, tokengo_shared::ChannelId(f.channel_id), &[]).unwrap();
        let replacement = db::channels::create(&c, &db::channels::NewChannel {
            name: "replacement source".into(), base_url: format!("{}/v1", f._source.url),
            enabled: true, unwrap_envelope: false, max_concurrent: 0,
        }).unwrap();
        db::channels::set_models(&c, replacement, &[db::channels::ModelMapping::new(
            "matrix", Some("upstream".into()), false,
        )]).unwrap();
        let caps = tokengo_server::protocol_config::load(&c, f.channel_id).unwrap();
        tokengo_server::protocol_config::save(&c, replacement.0, &caps).unwrap();
    }
    let (code, text) = f.call(Protocol::Responses, replay).await;
    assert_eq!(code, 400, "replacement source must not inherit old artifacts: {text}");
    assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
}


#[tokio::test]
async fn responses_anonymous_opaque_source_failure_does_not_fallback() {
    for stream in [false, true] {
        let f = prompt_cache_fixture().await;
        let (code, text) = f.call(Protocol::Responses, anonymous_request(prompt_cache_first(stream), false)).await;
        assert_eq!(code, 200, "{text}");
        let mut replay = anonymous_request(prompt_cache_replay(&completed_response(&text, stream), stream), false);
        replay["provider_fail"] = json!(true);
        let other_seen = Arc::new(Source::default());
        let other_source = serve(Router::new().route("/v1/responses", post(source)).with_state(other_seen.clone())).await;
        {
            let c = f.state.conn();
            let id = db::channels::create(&c, &db::channels::NewChannel {
                name: "available fallback".into(), base_url: format!("{}/v1", other_source.url),
                enabled: true, unwrap_envelope: false, max_concurrent: 0,
            }).unwrap();
            db::channels::set_models(&c, id, &[db::channels::ModelMapping::new("matrix", Some("upstream".into()), false)]).unwrap();
            let caps = tokengo_server::protocol_config::load(&c, f.channel_id).unwrap();
            tokengo_server::protocol_config::save(&c, id.0, &caps).unwrap();
        }
        let (code, text) = f.call(Protocol::Responses, replay).await;
        assert!(code >= 500, "original source failure must remain a failure: {code}: {text}");
        assert_eq!(f.seen.seen.lock().unwrap().len(), 2, "replay must reach its original source");
        assert!(other_seen.seen.lock().unwrap().is_empty(), "opaque history must never reach another source after failure");
    }
}

#[tokio::test]
async fn responses_anonymous_opaque_replay_honors_live_key_revocation() {
    let f = prompt_cache_fixture().await;
    let (row, key) = db::identity::create_key(&f.state.conn(), 1, "revocable-anonymous", &Default::default(), None, None).unwrap();
    let (code, text) = f.call_with_key(Protocol::Responses, &key, anonymous_request(prompt_cache_first(false), false)).await;
    assert_eq!(code, 200, "{text}");
    let replay = anonymous_request(prompt_cache_replay(&completed_response(&text, false), false), false);
    assert!(db::identity::revoke(&f.state.conn(), row.id).unwrap());
    let (code, text) = f.call_with_key(Protocol::Responses, &key, replay).await;
    assert_eq!(code, 401, "issued artifacts cannot override revoked credentials: {text}");
    assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
}


#[tokio::test]
async fn converted_chat_effort_alias_keeps_explicit_effort_ahead_of_mapping_defaults() {
    let f=Fixture::new(vec![Protocol::Responses],true).await;
    tokengo_server::reasoning_capabilities::configure(
        &f.state.conn(),f.channel_id,"matrix",Some(&["low".into(),"high".into()]),Some("high"),
    ).unwrap();
    for stream in [false,true] {
        let request=json!({"model":"matrix","messages":[{"role":"user","content":"hello"}],"reasoning":{"effort":"low"},"stream":stream,"max_tokens":16});
        let (status,text)=f.call(Protocol::Chat,request).await;
        assert_eq!(status,200,"{text}");
        let seen=f.seen.seen.lock().unwrap();let (path,body)=seen.last().unwrap();
        assert_eq!(path,"/v1/responses");
        assert_eq!(body["reasoning"],json!({"effort":"low"}),"configured high must not override explicit low");
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["model"],"upstream");
    }
    assert_eq!(f.seen.seen.lock().unwrap().len(),2);
}

#[tokio::test]
async fn invalid_converted_chat_effort_aliases_are_rejected_before_outbound_attempts() {
    let f=Fixture::new(vec![Protocol::Responses],true).await;
    tokengo_server::reasoning_capabilities::configure(
        &f.state.conn(),f.channel_id,"matrix",Some(&["low".into(),"high".into()]),Some("high"),
    ).unwrap();
    for extra in [
        json!({"reasoning":{"effort":"low"},"reasoning_effort":"high"}),
        json!({"reasoning":{"effort":"low","summary":"none"}}),
        json!({"reasoning":{"effort":true}}),
        json!({"reasoning":{"effort":"ultra"}}),
    ] {
        let mut body=json!({"model":"matrix","messages":[{"role":"user","content":"hello"}],"max_tokens":16});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let (status,text)=f.call(Protocol::Chat,body).await;
        assert_eq!(status,400,"{text}");
    }
    assert!(f.seen.seen.lock().unwrap().is_empty());
    f.state.drain_writes();
    let attempts:i64=f.state.conn().query_row("SELECT COUNT(*) FROM attempts",[],|row|row.get(0)).unwrap();
    assert_eq!(attempts,0,"invalid aliases must not cause upstream calls or retry attempts");
}
