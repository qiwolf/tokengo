//! Native Responses/Messages routes over real loopback HTTP, never real provider keys.
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU16, Ordering},
    Arc, Mutex,
};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tokengo_shared::ChannelId;
const MODEL: &str = "logical-native";
const UPSTREAM: &str = "Source/Native:Model";
const SOURCE_KEY: &str = "TEST-UPSTREAM-ONLY-CREDENTIAL";
struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server { url, task }
}
#[derive(Clone)]
struct Seen {
    path: String,
    headers: HeaderMap,
    body: Value,
}
#[derive(Default)]
struct Source {
    seen: Mutex<Vec<Seen>>,
    status: AtomicU16,
}
async fn native_source(
    State(source): State<Arc<Source>>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let path = uri.path().to_string();
    source.seen.lock().unwrap().push(Seen {
        path: path.clone(),
        headers,
        body: body.clone(),
    });
    let status = source.status.load(Ordering::SeqCst);
    if status >= 400 {
        return (StatusCode::from_u16(status).unwrap(),Json(json!({"type":"error","error":{"type":"overloaded_error","message":"synthetic source outage"}}))).into_response();
    }
    if body["stream"] == true {
        let start = if path.ends_with("/messages") {
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_cancel\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n"
        } else {
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_cancel\",\"object\":\"response\",\"status\":\"in_progress\"}}\n\n"
        };
        let payload = if body["x_test_precommit_pending"] == true {
            ""
        } else if path.ends_with("/messages") {
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"first text\"}}\n\n"
        } else {
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_partial\",\"output_index\":0,\"content_index\":0,\"delta\":\"first text\"}\n\n"
        };
        let first = bytes::Bytes::from(format!("{start}{payload}"));
        let stream = futures_util::stream::unfold(Some(first), move |first| async move {
            if let Some(first) = first {
                Some((Ok::<_, std::convert::Infallible>(first), None))
            } else {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                None
            }
        });
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response();
    }
    if path.ends_with("/responses") {
        Json(json!({"id":"resp_native_test","object":"response","status":"completed","model":UPSTREAM,"output":[{"id":"msg_output","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"native response","annotations":[]}]}],"usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}})).into_response()
    } else {
        Json(json!({"id":"msg_native_test","type":"message","role":"assistant","model":UPSTREAM,"content":[{"type":"text","text":"native message"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":5,"output_tokens":2}})).into_response()
    }
}
struct Fixture {
    state: SharedState,
    gateway: Server,
    _source: Server,
    seen: Arc<Source>,
    key: String,
    channel: ChannelId,
}
impl Fixture {
    async fn new() -> Self {
        let seen = Arc::new(Source::default());
        let source = serve(
            Router::new()
                .route("/v1/responses", post(native_source))
                .route("/v1/messages", post(native_source))
                .with_state(seen.clone()),
        )
        .await;
        let dir =
            std::env::temp_dir().join(format!("tokengo-native-routes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        state.routing().cfg.retry_times = 0;
        state.routing().cfg.probe_enabled = false;
        let (_, key) = db::identity::create_key(
            &state.conn(),
            1,
            "native-client",
            &Default::default(),
            None,
            None,
        )
        .unwrap();
        let channel = {
            let c = state.conn();
            let id = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "native-source".into(),
                    base_url: format!("{}/v1", source.url),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 1,
                },
            )
            .unwrap();
            db::channels::set_models(
                &c,
                id,
                &[db::channels::ModelMapping::new(
                    MODEL,
                    Some(UPSTREAM.into()),
                    false,
                )],
            )
            .unwrap();
            db::channels::set_secret(&c, &state.secrets, id, SOURCE_KEY).unwrap();
            tokengo_server::protocol_config::save(
                &c,
                id.0,
                &tokengo_server::protocol_config::Capabilities {
                    native: vec![
                        tokengo_server::protocol::Protocol::Responses,
                        tokengo_server::protocol::Protocol::Messages,
                    ],
                    allow_conversion: false,
                    native_input_audio: false,
                    chat_reasoning_replay: false,
                    max_input_tokens: None,
                    max_output_tokens: None,
                },
            )
            .unwrap();
            id
        };
        let gateway = serve(app(state.clone())).await;
        Self {
            state,
            gateway,
            _source: source,
            seen,
            key,
            channel,
        }
    }
    async fn call(&self, path: &str, headers: &[(&str, &str)], body: Value) -> reqwest::Response {
        let mut req = reqwest::Client::new()
            .post(format!("{}{path}", self.gateway.url))
            .json(&body);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        req.send().await.unwrap()
    }
    fn body(path: &str) -> Value {
        if path.ends_with("/messages") {
            json!({"model":MODEL,"max_tokens":32,"messages":[{"role":"user","content":"hello"}]})
        } else {
            json!({"model":MODEL,"max_output_tokens":32,"input":"hello"})
        }
    }
}
#[tokio::test]
async fn responses_and_messages_reach_their_native_paths_with_virtual_key_auth() {
    let f = Fixture::new().await;
    for (path, header) in [
        ("/v1/responses", "authorization"),
        ("/v1/messages", "x-api-key"),
    ] {
        let credential = if header == "authorization" {
            format!("Bearer {}", f.key)
        } else {
            f.key.clone()
        };
        let response = f
            .call(path, &[(header, &credential)], Fixture::body(path))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body: Value = response.json().await.unwrap();
        if path.ends_with("/messages") {
            assert_eq!(body["type"], "message");
            assert_eq!(body["content"][0]["text"], "native message");
        } else {
            assert_eq!(body["object"], "response");
            assert_eq!(body["status"], "completed");
        }
    }
    let seen = f.seen.seen.lock().unwrap();
    assert_eq!(
        seen.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
        vec!["/v1/responses", "/v1/messages"]
    );
    for request in seen.iter() {
        assert_eq!(request.body["model"], UPSTREAM);
        if request.path.ends_with("/messages") {
            assert_eq!(request.headers["x-api-key"], SOURCE_KEY);
            assert!(request.headers.get("authorization").is_none());
            assert_eq!(request.headers["anthropic-version"], "2023-06-01");
        } else {
            assert_eq!(
                request.headers["authorization"],
                format!("Bearer {SOURCE_KEY}")
            );
        }
        assert!(!request
            .headers
            .values()
            .any(|v| v.to_str().ok().is_some_and(|s| s.contains(&f.key))));
    }
}

#[tokio::test]
async fn native_auth_conflicts_duplicates_and_invalid_shapes_never_reach_upstream() {
    let f = Fixture::new().await;
    let bearer = format!("Bearer {}", f.key);
    for path in ["/v1/responses", "/v1/messages"] {
        let response = f.call(path, &[], Fixture::body(path)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let error: Value = response.json().await.unwrap();
        if path.ends_with("messages") {
            assert_eq!(error["type"], "error");
            assert_eq!(error["error"]["type"], "authentication_error");
        }
        let response = f
            .call(
                path,
                &[
                    ("authorization", &bearer),
                    ("x-api-key", "WRONG-SECRET-NOT-FOR-LOGS"),
                ],
                Fixture::body(path),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!response
            .text()
            .await
            .unwrap()
            .contains("WRONG-SECRET-NOT-FOR-LOGS"));
        assert_eq!(
            f.call(
                path,
                &[("authorization", &bearer), ("authorization", &bearer)],
                Fixture::body(path)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            f.call(
                path,
                &[("x-api-key", &f.key), ("x-api-key", &f.key)],
                Fixture::body(path)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            f.call(
                path,
                &[("authorization", "Basic invalid"), ("x-api-key", &f.key)],
                Fixture::body(path)
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        let mut invalid = Fixture::body(path);
        invalid["stream"] = json!("true");
        let response = f.call(path, &[("x-api-key", &f.key)], invalid).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        if path.ends_with("messages") {
            let err: Value = response.json().await.unwrap();
            assert_eq!(err["type"], "error");
            assert_eq!(err["error"]["type"], "invalid_request_error");
        }
    }
    assert_eq!(f.seen.seen.lock().unwrap().len(), 0);
    for (path, invalid) in [
        ("/v1/responses", json!({"model":MODEL,"input":{}})),
        (
            "/v1/messages",
            json!({"model":MODEL,"messages":[],"max_tokens":0}),
        ),
        (
            "/v1/messages",
            json!({"model":MODEL,"messages":[],"max_tokens":1.5}),
        ),
        (
            "/v1/messages",
            json!({"model":MODEL,"messages":"invalid","max_tokens":32}),
        ),
    ] {
        assert_eq!(
            f.call(path, &[("x-api-key", &f.key)], invalid)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(f.seen.seen.lock().unwrap().len(), 0);
    for path in ["/v1/responses", "/v1/messages"] {
        assert_eq!(
            f.call(
                path,
                &[("authorization", &bearer), ("x-api-key", &f.key)],
                Fixture::body(path)
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    assert_eq!(f.seen.seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_tool_history_and_unknown_fields_are_preserved_without_internal_body_keys() {
    let f = Fixture::new().await;
    let response = json!({"model":MODEL,"max_output_tokens":32,"input":[
  {"role":"user","content":"weather"},
  {"type":"function_call","id":"fc_native_1","call_id":"call_keep_1","name":"lookup","arguments":"{\"city\":\"上海\"}"},
  {"type":"function_call_output","call_id":"call_keep_1","output":"{\"temperature\":23}"}],
  "tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}],
  "x_future_native":{"opaque":["preserve",1]}});
    let messages = json!({"model":MODEL,"max_tokens":32,"system":[{"type":"text","text":"system"}],"messages":[
  {"role":"user","content":"weather"},
  {"role":"assistant","content":[{"type":"tool_use","id":"toolu_keep_1","name":"lookup","input":{"city":"上海"}}]},
  {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_keep_1","content":[{"type":"text","text":"{\"temperature\":23}"}]}]}],
  "tools":[{"name":"lookup","description":"weather","input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],
  "x_future_native":{"opaque":["preserve",1]}});
    for (path, body) in [("/v1/responses", response), ("/v1/messages", messages)] {
        let response = f.call(path, &[("x-api-key", &f.key)], body.clone()).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await.unwrap()
        );
        let sent = f.seen.seen.lock().unwrap().last().unwrap().body.clone();
        let mut expected = body;
        expected["model"] = json!(UPSTREAM);
        assert_eq!(sent, expected);
    }
}

#[tokio::test]
async fn claude_session_header_is_namespaced_and_client_version_headers_do_not_override_source() {
    let f = Fixture::new().await;
    db::channels::set_headers(
        &f.state.conn(),
        &f.state.secrets,
        f.channel,
        &[
            ("X-Session-Key".into(), "{{session_id}}".into()),
            ("anthropic-version".into(), "2023-06-01".into()),
            ("anthropic-beta".into(), "configured-native-beta".into()),
        ],
    )
    .unwrap();
    for session in ["claude-a", "claude-a", "claude-b"] {
        let response = f
            .call(
                "/v1/messages",
                &[
                    ("x-api-key", &f.key),
                    ("x-claude-code-session-id", session),
                    ("anthropic-version", "untrusted-client-version"),
                    ("anthropic-beta", "untrusted-client-beta"),
                ],
                Fixture::body("/v1/messages"),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let seen = f.seen.seen.lock().unwrap();
    assert_eq!(
        seen[0].headers["x-session-key"],
        seen[1].headers["x-session-key"]
    );
    assert_ne!(
        seen[0].headers["x-session-key"],
        seen[2].headers["x-session-key"]
    );
    for request in seen.iter() {
        assert_eq!(request.headers["anthropic-version"], "2023-06-01");
        assert_eq!(request.headers["anthropic-beta"], "configured-native-beta");
        assert!(request.headers.get("x-claude-code-session-id").is_none());
    }
}

#[tokio::test]
async fn cancelling_each_native_http_stream_releases_vkey_and_channel_concurrency() {
    for path in ["/v1/responses", "/v1/messages"] {
        let f = Fixture::new().await;
        let auth = f.state.authenticate_context(f.key.clone()).await.unwrap();
        let mut row = db::identity::key(&f.state.conn(), auth.key_id)
            .unwrap()
            .unwrap();
        row.policy.max_concurrent = Some(1);
        db::identity::update_key(&f.state.conn(), &row).unwrap();
        let mut body = Fixture::body(path);
        body["stream"] = json!(true);
        let mut response = f.call(path, &[("x-api-key", &f.key)], body).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.chunk().await.unwrap().is_some());
        drop(response);
        let mut canceled = false;
        for _ in 0..100 {
            f.state.drain_writes();
            canceled = f
                .state
                .conn()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM requests WHERE status='canceled')",
                    [],
                    |r| r.get::<_, bool>(0),
                )
                .unwrap();
            if canceled {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            canceled,
            "HTTP disconnect did not finalize {path} as canceled"
        );
        assert_eq!(
            f.call(path, &[("x-api-key", &f.key)], Fixture::body(path))
                .await
                .status(),
            StatusCode::OK,
            "canceled stream retained concurrency slots"
        );
    }
}

#[tokio::test]
async fn cancelling_native_http_before_precommit_releases_the_pending_request() {
    for path in ["/v1/responses", "/v1/messages"] {
        let f = Fixture::new().await;
        let mut body = Fixture::body(path);
        body["stream"] = json!(true);
        body["x_test_precommit_pending"] = json!(true);
        let url = format!("{}{path}", f.gateway.url);
        let key = f.key.clone();
        let pending = tokio::spawn(async move {
            reqwest::Client::new()
                .post(url)
                .header("x-api-key", key)
                .json(&body)
                .send()
                .await
        });
        for _ in 0..100 {
            if !f.seen.seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
        assert!(
            !pending.is_finished(),
            "metadata-only native stream committed before useful output"
        );
        pending.abort();
        let _ = pending.await;
        let mut canceled = false;
        for _ in 0..100 {
            f.state.drain_writes();
            canceled = f
                .state
                .conn()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM requests WHERE status='canceled')",
                    [],
                    |r| r.get::<_, bool>(0),
                )
                .unwrap();
            if canceled {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(canceled, "precommit HTTP cancellation left {path} running");
        assert_eq!(
            f.call(path, &[("x-api-key", &f.key)], Fixture::body(path))
                .await
                .status(),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn native_routes_do_not_bypass_virtual_key_policy_or_unknown_continuation() {
    let f = Fixture::new().await;
    for field in ["previous_response_id", "conversation"] {
        let mut body = Fixture::body("/v1/responses");
        body[field] = json!("unknown-or-other-owner-id");
        assert_eq!(
            f.call("/v1/responses", &[("x-api-key", &f.key)], body)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let auth = f.state.authenticate_context(f.key.clone()).await.unwrap();
    let mut row = db::identity::key(&f.state.conn(), auth.key_id)
        .unwrap()
        .unwrap();
    row.policy.allowed_models = Some(vec!["different-model".into()]);
    db::identity::update_key(&f.state.conn(), &row).unwrap();
    for path in ["/v1/responses", "/v1/messages"] {
        let response = f
            .call(path, &[("x-api-key", &f.key)], Fixture::body(path))
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        if path.ends_with("/messages") {
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["type"], "error");
            assert_eq!(body["error"]["type"], "permission_error");
        }
    }
    row.policy.allowed_models = None;
    row.policy.allowed_channels = Some(vec![]);
    db::identity::update_key(&f.state.conn(), &row).unwrap();
    for path in ["/v1/responses", "/v1/messages"] {
        assert_eq!(
            f.call(path, &[("x-api-key", &f.key)], Fixture::body(path))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert!(f.seen.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn explicit_native_session_pins_source_and_isolates_keys_and_credential_generations() {
    let f = Fixture::new().await;
    let secondary_seen = Arc::new(Source::default());
    let secondary = serve(
        Router::new()
            .route("/v1/messages", post(native_source))
            .with_state(secondary_seen.clone()),
    )
    .await;
    let bid = {
        let c = f.state.conn();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "secondary-account".into(),
                base_url: format!("{}/v1", secondary.url),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 1,
            },
        )
        .unwrap();
        db::channels::set_models(
            &c,
            id,
            &[db::channels::ModelMapping::new(
                MODEL,
                Some(UPSTREAM.into()),
                false,
            )],
        )
        .unwrap();
        db::channels::set_secret(&c, &f.state.secrets, id, "SECOND-SYNTHETIC-ACCOUNT").unwrap();
        tokengo_server::protocol_config::save(
            &c,
            id.0,
            &tokengo_server::protocol_config::Capabilities {
                native: vec![tokengo_server::protocol::Protocol::Messages],
                allow_conversion: false,
                    native_input_audio: false,
                    chat_reasoning_replay: false,
                    max_input_tokens: None,
                    max_output_tokens: None,
            },
        )
        .unwrap();
        id
    };
    let headers = [
        ("x-api-key", f.key.as_str()),
        ("x-claude-code-session-id", "pinned-claude-session"),
    ];
    assert_eq!(
        f.call("/v1/messages", &headers, Fixture::body("/v1/messages"))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
    assert_eq!(secondary_seen.seen.lock().unwrap().len(), 0);
    f.seen.status.store(503, Ordering::SeqCst);
    assert!(!f
        .call("/v1/messages", &headers, Fixture::body("/v1/messages"))
        .await
        .status()
        .is_success());
    assert_eq!(f.seen.seen.lock().unwrap().len(), 2);
    assert_eq!(
        secondary_seen.seen.lock().unwrap().len(),
        0,
        "a pinned failed session must not cross account"
    );
    let (_, other) = db::identity::create_key(
        &f.state.conn(),
        1,
        "isolated-native-key",
        &db::identity::Policy {
            allowed_channels: Some(vec![bid.0]),
            ..Default::default()
        },
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        f.call(
            "/v1/messages",
            &[
                ("x-api-key", &other),
                ("x-claude-code-session-id", "pinned-claude-session")
            ],
            Fixture::body("/v1/messages")
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        secondary_seen.seen.lock().unwrap().len(),
        1,
        "same session text under a different VKey must have an independent source binding"
    );
    f.seen.status.store(200, Ordering::SeqCst);
    db::channels::set_secret(
        &f.state.conn(),
        &f.state.secrets,
        f.channel,
        "ROTATED-SYNTHETIC-ACCOUNT",
    )
    .unwrap();
    let before = (
        f.seen.seen.lock().unwrap().len(),
        secondary_seen.seen.lock().unwrap().len(),
    );
    assert!(!f
        .call("/v1/messages", &headers, Fixture::body("/v1/messages"))
        .await
        .status()
        .is_success());
    assert_eq!(
        (
            f.seen.seen.lock().unwrap().len(),
            secondary_seen.seen.lock().unwrap().len()
        ),
        before,
        "credential rotation must not silently reuse or move old session"
    );
}

#[tokio::test]
async fn opaque_native_history_requires_its_key_binding_and_reserves_full_input_bound() {
    let f = Fixture::new().await;
    db::billing::create_price(
        &f.state.conn(),
        &db::billing::Price {
            id: 0,
            channel_id: f.channel.0,
            model: MODEL.into(),
            input_micro_per_million: 0,
            output_micro_per_million: 0,
            max_input_tokens: 5000,
            max_output_tokens: 100,
        },
        "synthetic-test",
    )
    .unwrap();
    let auth = f.state.authenticate_context(f.key.clone()).await.unwrap();
    let mut key = db::identity::key(&f.state.conn(), auth.key_id)
        .unwrap()
        .unwrap();
    key.policy.tpm = Some(2000);
    db::identity::update_key(&f.state.conn(), &key).unwrap();
    let headers = [
        ("x-api-key", f.key.as_str()),
        ("x-claude-code-session-id", "opaque-session"),
    ];
    assert_eq!(
        f.call("/v1/messages", &headers, Fixture::body("/v1/messages"))
            .await
            .status(),
        StatusCode::OK
    );
    let mut opaque = Fixture::body("/v1/messages");
    opaque["messages"] = json!([{"role":"assistant","content":[{"type":"thinking","thinking":"synthetic private reasoning","signature":"synthetic-signature"}]},{"role":"user","content":"continue"}]);
    let before = f.seen.seen.lock().unwrap().len();
    assert_eq!(
        f.call("/v1/messages", &headers, opaque.clone())
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS,
        "tiny opaque body must reserve the configured5000 input bound, not its JSON byte count"
    );
    assert_eq!(f.seen.seen.lock().unwrap().len(), before);
    let (_, other) = db::identity::create_key(
        &f.state.conn(),
        1,
        "no-opaque-ownership",
        &Default::default(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        f.call(
            "/v1/messages",
            &[
                ("x-api-key", &other),
                ("x-claude-code-session-id", "opaque-session")
            ],
            opaque
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.seen.seen.lock().unwrap().len(),
        before,
        "another VKey must not borrow an existing opaque history binding"
    );
}

#[tokio::test]
async fn native_source_probe_uses_declared_protocol_instead_of_assuming_chat() {
    let f = Fixture::new().await;
    let outcome = tokengo_server::prober::probe_route(&f.state, MODEL, db::now_ms()).await;
    assert!(
        matches!(
            outcome.outcome,
            tokengo_server::prober::ProbeOutcome::SwitchedBack
        ),
        "{outcome:?}"
    );
    assert_eq!(
        f.seen.seen.lock().unwrap().last().unwrap().path,
        "/v1/responses"
    );
}

#[tokio::test]
async fn hidden_provider_state_and_unmetered_server_tools_are_rejected_before_send() {
    let f = Fixture::new().await;
    for (path, body) in [
        (
            "/v1/responses",
            json!({"model":MODEL,"input":[{"type":"item_reference","id":"msg_someone_else"}]}),
        ),
        (
            "/v1/responses",
            json!({"model":MODEL,"input":[{"type":"message","role":"user","content":[{"type":"input_file","file_id":"file_hidden"}]}]}),
        ),
        (
            "/v1/responses",
            json!({"model":MODEL,"input":"search","tools":[{"type":"web_search"}]}),
        ),
        (
            "/v1/messages",
            json!({"model":MODEL,"max_tokens":32,"messages":[{"role":"user","content":"search"}],"tools":[{"type":"web_search_20250305","name":"web_search"}]}),
        ),
        (
            "/v1/messages",
            json!({"model":MODEL,"max_tokens":32,"messages":[{"role":"user","content":[{"type":"document","source":{"type":"url","url":"https://example.com/file"}}]}]}),
        ),
        (
            "/v1/messages",
            json!({"model":MODEL,"max_tokens":32,"messages":[],"container":"another-account-container"}),
        ),
    ] {
        let response = f.call(path, &[("x-api-key", &f.key)], body).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{}",
            response.text().await.unwrap()
        );
    }
    assert!(f.seen.seen.lock().unwrap().is_empty());
}
