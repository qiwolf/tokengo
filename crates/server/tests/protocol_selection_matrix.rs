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
    let responses = uri.path().ends_with("responses");
    let mut value = if responses {
        json!({"id":"resp_matrix","object":"response","status":"completed","model":"upstream","output":[{"id":"msg_matrix","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"matrix answer","annotations":[]}]}],"usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}})
    } else {
        json!({"id":"chat_matrix","object":"chat.completion","model":"upstream","choices":[{"index":0,"message":{"role":"assistant","content":"matrix answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}})
    };
    if responses && v["provider_opaque"] == true {
        value["output"].as_array_mut().unwrap().insert(0,json!({"id":"rs_matrix","type":"reasoning","summary":[],"encrypted_content":"test-opaque-state"}));
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
        let key = {
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
            key
        };
        let gateway = serve(app(state.clone())).await;
        Self {
            state,
            gateway,
            _source: upstream,
            seen,
            key,
            dir,
        }
    }
    async fn call(&self, p: Protocol, body: Value) -> (u16, String) {
        let r = reqwest::Client::new()
            .post(format!("{}/v1/{}", self.gateway.url, p.path()))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
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
