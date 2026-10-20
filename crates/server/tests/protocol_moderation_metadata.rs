//! Known moderation telemetry is optional presentation data; control metadata is not.
//! Synthetic values reproduce the captured envelope shape, never private audit values.
use axum::{
    body::Body,
    extract::{OriginalUri, State},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokengo_server::protocol::{sse::Transcoder, Protocol};
use tokengo_server::{app, db, AppState, Config, SharedState};
struct Http {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Http {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(app: Router) -> Http {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    Http {
        url,
        task: tokio::spawn(async move { axum::serve(l, app).await.unwrap() }),
    }
}
fn event(v: Value) -> String {
    format!("event: {}\ndata: {}\n\n", v["type"].as_str().unwrap(), v)
}
fn created() -> String {
    event(
        json!({"type":"response.created","sequence_number":0,"response":{"id":"resp_fixture","model":"source","status":"in_progress","output":[]}}),
    )
}
fn text() -> String {
    event(
        json!({"type":"response.output_text.delta","sequence_number":4,"item_id":"msg_fixture","output_index":0,"content_index":0,"delta":"OK"}),
    )
}
// Envelope and event order follow the safe live capture. Nested audit values
// and outputs keys are synthetic; private provider metadata was never retained.
fn moderation() -> Value {
    json!({"type":"response.metadata","response_id":"resp_fixture","sequence_number":5,"metadata":{"openai_chatgpt_moderation_metadata":{"prompt":{"omnimod":{"protection_type":"default","outputs":[{"result":{},"source":"test"}]}},"generation":{},"tool_call":{},"tool_response":{}}}})
}
fn complete() -> String {
    event(
        json!({"type":"response.completed","sequence_number":9,"response":{"id":"resp_fixture","model":"source","status":"completed","output":[{"id":"msg_fixture","type":"message","role":"assistant","content":[{"type":"output_text","text":"OK","annotations":[]}]}],"usage":{"input_tokens":11,"output_tokens":5,"total_tokens":16}}}),
    )
}
fn full(metadata: Value) -> String {
    created()
        + &event(
            json!({"type":"response.in_progress","sequence_number":1,"response":{"id":"resp_fixture","model":"source","status":"in_progress","output":[]}}),
        )
        + &event(
            json!({"type":"response.output_item.added","sequence_number":2,"output_index":0,"item":{"id":"msg_fixture","type":"message","role":"assistant","content":[]}}),
        )
        + &event(
            json!({"type":"response.content_part.added","sequence_number":3,"output_index":0,"content_index":0,"item_id":"msg_fixture","part":{"type":"output_text","text":"","annotations":[]}}),
        )
        + &text()
        + &event(metadata)
        + &event(
            json!({"type":"response.output_text.done","sequence_number":6,"output_index":0,"content_index":0,"item_id":"msg_fixture","text":"OK"}),
        )
        + &event(
            json!({"type":"response.content_part.done","sequence_number":7,"output_index":0,"content_index":0,"item_id":"msg_fixture","part":{"type":"output_text","text":"OK","annotations":[]}}),
        )
        + &event(
            json!({"type":"response.output_item.done","sequence_number":8,"output_index":0,"item":{"id":"msg_fixture","type":"message","role":"assistant","content":[{"type":"output_text","text":"OK","annotations":[]}]}}),
        )
        + &complete()
}
fn convert(metadata: Value, to: Protocol) -> Result<(String, Transcoder), String> {
    let mut t = Transcoder::new(Protocol::Responses, to);
    let mut out = t.feed(full(metadata).as_bytes())?;
    out.extend(t.finish()?);
    Ok((String::from_utf8(out).unwrap(), t))
}
#[test]
fn captured_moderation_shape_converts_to_chat_and_messages_with_exact_usage() {
    for to in [Protocol::Chat, Protocol::Messages] {
        let (out, t) = convert(moderation(), to).unwrap();
        assert!(out.contains("OK"));
        assert!(!out.contains("openai_chatgpt_moderation_metadata"));
        assert_eq!(t.observer.input, Some(11));
        assert_eq!(t.observer.output, Some(5));
        assert!(t.observer.done);
        assert!(out.contains(if to == Protocol::Chat {
            "[DONE]"
        } else {
            "message_stop"
        }));
    }
}
#[test]
fn optional_identity_and_opaque_moderation_values_are_supported() {
    for value in [
        Value::Null,
        json!(42),
        json!(["headers",{"verification":true,"reasoning":{"signature":"opaque audit classification"},"safety_buffering":true}]),
    ] {
        let metadata = json!({"type":"response.metadata","metadata":{"openai_chatgpt_moderation_metadata":value}});
        assert!(convert(metadata, Protocol::Chat).is_ok());
    }
}
#[test]
fn unknown_control_namespaces_or_invalid_envelopes_are_rejected() {
    let mut variants = vec![];
    for key in [
        "safety_buffering",
        "openai_verification_recommendation",
        "headers",
        "turn_state",
        "unknown_telemetry",
    ] {
        let mut v = moderation();
        v["metadata"][key] = json!({"active":true});
        variants.push(v);
    }
    for key in ["headers", "safety_buffering", "response", "unknown"] {
        let mut v = moderation();
        v[key] = json!({"active":true});
        variants.push(v);
    }
    for value in [json!(-1), json!("5"), json!(true)] {
        let mut v = moderation();
        v["sequence_number"] = value;
        variants.push(v);
    }
    let mut v = moderation();
    v["response_id"] = json!("different_response");
    variants.push(v);
    for v in variants {
        assert!(convert(v, Protocol::Chat).is_err());
    }
}
#[test]
fn bounded_opaque_moderation_refuses_oversize_deep_or_excess_nodes() {
    let mut deep = json!(0);
    for _ in 0..18 {
        deep = json!([deep]);
    }
    for value in [json!("x".repeat(65536)), deep, json!(vec![0; 4097])] {
        let mut v = moderation();
        v["metadata"]["openai_chatgpt_moderation_metadata"] = value;
        assert!(convert(v, Protocol::Chat).is_err());
    }
}
struct Source {
    mode: usize,
    bad: AtomicUsize,
    good: AtomicUsize,
    release: tokio::sync::Notify,
}
async fn source(State(s): State<Arc<Source>>, OriginalUri(uri): OriginalUri) -> Response {
    let good = uri.path().contains("/good/");
    if good {
        s.good.fetch_add(1, Ordering::SeqCst);
    } else {
        s.bad.fetch_add(1, Ordering::SeqCst);
    }
    if !good && s.mode == 4 {
        let error = created()
            + &event(moderation())
            + &event(
                json!({"type":"error","error":{"code":"controlled_source_error","message":"synthetic source error while connection remains open"}}),
            );
        let stream = futures_util::StreamExt::chain(
            futures_util::stream::once(async move {
                Ok::<_, std::io::Error>(bytes::Bytes::from(error))
            }),
            futures_util::stream::pending::<Result<bytes::Bytes, std::io::Error>>(),
        );
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response();
    }
    let mut unsafe_meta = moderation();
    unsafe_meta["metadata"]["safety_buffering"] = json!({"retry_model":"other"});
    if !good && s.mode == 3 {
        let first = created() + &text();
        let tail = event(unsafe_meta) + &complete();
        let stream =
            futures_util::stream::unfold((0, first, tail, s), |(n, first, tail, s)| async move {
                match n {
                    0 => Some((
                        Ok::<_, std::io::Error>(bytes::Bytes::from(first.clone())),
                        (1, first, tail, s),
                    )),
                    1 => {
                        tokio::time::timeout(
                            std::time::Duration::from_secs(15),
                            s.release.notified(),
                        )
                        .await
                        .ok();
                        Some((Ok(bytes::Bytes::from(tail.clone())), (2, first, tail, s)))
                    }
                    _ => None,
                }
            });
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response();
    }
    let body = if good {
        created() + &text() + &complete()
    } else {
        match s.mode {
            0 => full(moderation()),
            5 => full(moderation()).replace(&complete(), &event(json!({"type":"response.completed","response":{"id":"resp_fixture","status":"completed","output":[],"usage":{"input_tokens":11,"output_tokens":5}}}))),
            1 => {
                created()
                    + &event(moderation())
                    + &event(
                        json!({"type":"response.completed","response":{"id":"resp_fixture","status":"completed","output":[],"usage":{"input_tokens":11,"output_tokens":0}}}),
                    )
            }
            _ => created() + &event(unsafe_meta) + &complete(),
        }
    };
    ([("content-type", "text/event-stream")], body).into_response()
}
struct Fixture {
    state: SharedState,
    http: Http,
    _upstream: Http,
    src: Arc<Source>,
    key: String,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
impl Fixture {
    async fn new(mode: usize) -> Self {
        let src = Arc::new(Source {
            mode,
            bad: AtomicUsize::new(0),
            good: AtomicUsize::new(0),
            release: tokio::sync::Notify::new(),
        });
        let upstream = serve(
            Router::new()
                .route("/{kind}/v1/responses", post(source))
                .with_state(src.clone()),
        )
        .await;
        let dir = std::env::temp_dir().join(format!("moderation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        let key = {
            let c = state.conn();
            let u = db::identity::create_user(&c, "metadata", "user", &Default::default()).unwrap();
            let (_, key) =
                db::identity::create_key(&c, u.id, "metadata", &Default::default(), None, None)
                    .unwrap();
            let mut ids = vec![];
            for kind in ["bad", "good"] {
                let id = db::channels::create(
                    &c,
                    &db::channels::NewChannel {
                        name: kind.into(),
                        base_url: format!("{}/{kind}/v1", upstream.url),
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
                        "logical",
                        Some("source".into()),
                        false,
                    )],
                )
                .unwrap();
                tokengo_server::protocol_config::save(
                    &c,
                    id.0,
                    &tokengo_server::protocol_config::Capabilities {
                        native: vec![Protocol::Responses],
                        allow_conversion: true,
                        ..Default::default()
                    },
                )
                .unwrap();
                db::billing::create_price(
                    &c,
                    &db::billing::Price {
                        id: 0,
                        channel_id: id.0,
                        model: "logical".into(),
                        input_micro_per_million: 1,
                        output_micro_per_million: 1,
                        max_input_tokens: 8192,
                        max_output_tokens: 64,
                    },
                    "test",
                )
                .unwrap();
                ids.push(id.0);
            }
            db::route_policy::save(
                &c,
                "logical",
                &db::route_policy::RoutePolicy {
                    channel_ids: Some(ids),
                    retry_times: Some(0),
                    attempt_timeout_ms: Some(30000),
                    request_budget_ms: Some(60000),
                    probe_enabled: Some(false),
                    include_in_auto: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
            key
        };
        let http = serve(app(state.clone())).await;
        Self {
            state,
            http,
            _upstream: upstream,
            src,
            key,
            dir,
        }
    }
    async fn call(&self, native: bool) -> reqwest::Response {
        let (path, body) = if native {
            (
                "/v1/responses",
                json!({"model":"logical","input":"hi","max_output_tokens":16,"stream":true}),
            )
        } else {
            (
                "/v1/chat/completions",
                json!({"model":"logical","messages":[{"role":"user","content":"hi"}],"max_tokens":16,"stream":true}),
            )
        };
        reqwest::Client::new()
            .post(format!("{}{path}", self.http.url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    fn ledger(&self, n: usize) {
        self.state.drain_writes();
        let rows = db::billing::usage(&self.state.conn(), &Default::default()).unwrap();
        assert_eq!(rows.len(), n);
        assert!(rows.iter().all(|r| r.settled_ms.is_some()));
    }
}
#[tokio::test]
async fn real_http_moderation_stream_completes_with_usage_and_adaptation_notice() {
    let f = Fixture::new(0).await;
    let r = f.call(false).await;
    assert_eq!(r.status(), 200);
    let text = r.text().await.unwrap();
    assert!(text.contains("OK") && text.contains("[DONE]"));
    assert!(!text.contains("event: error"));
    assert_eq!(f.src.bad.load(Ordering::SeqCst), 1);
    assert_eq!(f.src.good.load(Ordering::SeqCst), 0);
    f.ledger(1);
    let c = f.state.conn();
    let row = db::billing::usage(&c, &Default::default())
        .unwrap()
        .remove(0);
    assert_eq!(row.input_tokens, Some(11));
    assert_eq!(row.output_tokens, Some(5));
    let note: String = c
        .query_row("SELECT error FROM attempts LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(note.contains("moderation telemetry"));
}
#[tokio::test]
async fn metadata_only_cannot_commit_and_falls_back() {
    let f = Fixture::new(1).await;
    let r = f.call(false).await;
    assert_eq!(r.status(), 200);
    let text = r.text().await.unwrap();
    assert!(text.contains("OK") && text.contains("[DONE]"));
    assert_eq!(f.src.bad.load(Ordering::SeqCst), 1);
    assert_eq!(f.src.good.load(Ordering::SeqCst), 1);
    f.ledger(2);
}
#[tokio::test]
async fn unsafe_control_metadata_before_output_is_local_rejection_and_falls_back() {
    let f = Fixture::new(2).await;
    let r = f.call(false).await;
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("[DONE]"));
    assert_eq!(f.src.bad.load(Ordering::SeqCst), 1);
    assert_eq!(f.src.good.load(Ordering::SeqCst), 1);
    f.ledger(2);
    assert!(db::policy::rows(&f.state.conn(), "logical")
        .unwrap()
        .iter()
        .all(|r| r.failures == 0));
}
#[tokio::test]
async fn unsafe_control_after_committed_text_errors_without_replay() {
    let f = Fixture::new(3).await;
    let mut r = f.call(false).await;
    assert_eq!(r.status(), 200);
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), r.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("OK"));
    f.src.release.notify_one();
    let rest = r.text().await.unwrap();
    assert!(
        rest.contains("\"error\"")
            && rest.contains("Response conversion is unsupported after output")
    );
    assert!(!rest.contains("[DONE]"));
    assert_eq!(f.src.bad.load(Ordering::SeqCst), 1);
    assert_eq!(f.src.good.load(Ordering::SeqCst), 0);
    f.ledger(1);
}
#[tokio::test]
async fn native_responses_keep_original_moderation_frame_bytes() {
    let f = Fixture::new(0).await;
    let r = f.call(true).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), full(moderation()));
    assert_eq!(f.src.good.load(Ordering::SeqCst), 0);
    f.ledger(1);
}

#[test]
fn opaque_audit_fields_cannot_set_payload_meter_or_terminal_state() {
    let value = json!({"type":"response.metadata","metadata":{"openai_chatgpt_moderation_metadata":{"usage":{"input_tokens":999,"output_tokens":999},"response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"fake telemetry text"}]}]},"headers":{"x-turn-state":"synthetic opaque classification"},"type":"safety_buffering"}}});
    let mut t = Transcoder::new(Protocol::Responses, Protocol::Chat);
    assert!(t.feed(event(value).as_bytes()).unwrap().is_empty());
    assert!(!t.observer.meaningful);
    assert!(!t.observer.done);
    assert_eq!(t.observer.input, None);
    assert_eq!(t.observer.output, None);
    assert!(t.finish().is_err());
}
#[tokio::test]
async fn metadata_then_error_falls_back_without_waiting_for_open_source_eof() {
    let f = Fixture::new(4).await;
    let text = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let r = f.call(false).await;
        assert_eq!(r.status(), 200);
        r.text().await.unwrap()
    })
    .await
    .expect(
        "Explicit source error must fallback before the 30s attempt deadline, even with no EOF",
    );
    assert!(text.contains("OK") && text.contains("[DONE]"));
    assert_eq!(f.src.bad.load(Ordering::SeqCst), 1);
    assert_eq!(f.src.good.load(Ordering::SeqCst), 1);
    f.ledger(2);
}

#[tokio::test]
async fn real_http_sparse_final_output_finishes_without_fallback_or_lost_usage() {
    let f = Fixture::new(5).await;
    let r = f.call(false).await;
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("OK") && body.contains("[DONE]") && !body.contains("\"error\""));
    assert_eq!(f.src.bad.load(Ordering::SeqCst), 1);
    assert_eq!(f.src.good.load(Ordering::SeqCst), 0);
    f.ledger(1);
    let rows = db::billing::usage(&f.state.conn(), &Default::default()).unwrap();
    assert_eq!(rows[0].input_tokens, Some(11));
    assert_eq!(rows[0].output_tokens, Some(5));
}
#[tokio::test]
async fn native_sparse_final_output_bytes_are_unchanged() {
    let f = Fixture::new(5).await;
    let r = f.call(true).await;
    let expected=full(moderation()).replace(&complete(),&event(json!({"type":"response.completed","response":{"id":"resp_fixture","status":"completed","output":[],"usage":{"input_tokens":11,"output_tokens":5}}})));
    assert_eq!(r.text().await.unwrap(), expected);
    f.ledger(1);
}
