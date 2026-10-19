//! Source capability is an explicit input dialect contract, never inferred from output.
use axum::{
    extract::{OriginalUri, State},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
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
async fn serve(router: Router) -> Http {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    Http {
        url,
        task: tokio::spawn(async move { axum::serve(l, router).await.unwrap() }),
    }
}
type Seen = Arc<Mutex<Vec<(String, Value)>>>;
async fn source(
    State(seen): State<Seen>,
    OriginalUri(uri): OriginalUri,
    Json(body): Json<Value>,
) -> Json<Value> {
    seen.lock().unwrap().push((uri.path().into(), body));
    Json(
        json!({"id":"chat_source","object":"chat.completion","model":"source-physical","choices":[{"index":0,"message":{"role":"assistant","content":"answer","reasoning_content":"new source reasoning"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
    )
}
struct Fixture {
    state: SharedState,
    server: Http,
    _source: Http,
    seen: Seen,
    key: String,
    ids: Vec<i64>,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
impl Fixture {
    async fn new(capabilities: &[Option<bool>]) -> Self {
        let seen = Arc::new(Mutex::new(vec![]));
        let upstream = serve(
            Router::new()
                .route("/{kind}/v1/chat/completions", post(source))
                .with_state(seen.clone()),
        )
        .await;
        let dir = std::env::temp_dir().join(format!("source-reasoning-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        let (key, ids) = {
            let c = state.conn();
            let u = db::identity::create_user(&c, "dialect", "user", &Default::default()).unwrap();
            let (_, key) =
                db::identity::create_key(&c, u.id, "dialect", &Default::default(), None, None)
                    .unwrap();
            let mut ids = vec![];
            for (n, capability) in capabilities.iter().enumerate() {
                let id = db::channels::create(
                    &c,
                    &db::channels::NewChannel {
                        name: format!("source{n}"),
                        base_url: format!("{}/{n}/v1", upstream.url),
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
                        Some("source-physical".into()),
                        false,
                    )],
                )
                .unwrap();
                let mut caps = json!({"native":["chat"],"allow_conversion":true});
                if let Some(v) = capability {
                    caps["chat_reasoning_replay"] = json!(v);
                }
                // Raw persistence lets the red test exercise the live HTTP behavior before the new field exists.
                db::settings::set(
                    &c,
                    &format!("channel_protocols:{}", id.0),
                    &caps.to_string(),
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
                        max_output_tokens: 32,
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
                    channel_ids: Some(ids.clone()),
                    retry_times: Some(0),
                    probe_enabled: Some(false),
                    include_in_auto: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
            (key, ids)
        };
        let server = serve(app(state.clone())).await;
        Self {
            state,
            server,
            _source: upstream,
            seen,
            key,
            ids,
            dir,
        }
    }
    async fn send(&self, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}{path}", self.server.url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    fn attempt_count(&self) -> i64 {
        self.state
            .conn()
            .query_row("SELECT COUNT(*) FROM attempts", [], |r| r.get(0))
            .unwrap()
    }
    fn ledger_count(&self) -> i64 {
        self.state
            .conn()
            .query_row("SELECT COUNT(*) FROM billing_ledger", [], |r| r.get(0))
            .unwrap()
    }
    fn no_source_penalty(&self) {
        let c = self.state.conn();
        assert!(db::faults::rows(&c).unwrap().is_empty());
        for id in &self.ids {
            assert!(db::policy::rows(&c, "logical")
                .unwrap()
                .iter()
                .all(|r| r.channel_id != *id || (r.failures == 0 && r.until_ms == 0)));
        }
    }
}
fn replay() -> Value {
    json!({"model":"logical","max_output_tokens":16,"input":[{"type":"message","role":"user","content":"compute"},{"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"exact prior reasoning"}]},{"type":"function_call","call_id":"call_exact","name":"sum","arguments":"{\"a\":1}"},{"type":"function_call_output","call_id":"call_exact","output":"2"}]})
}
#[tokio::test]
async fn missing_capability_rejects_before_any_source_attempt_or_charge() {
    let f = Fixture::new(&[None]).await;
    let response = f.send("/v1/responses", replay()).await;
    assert_eq!(response.status(), 400);
    assert!(f.seen.lock().unwrap().is_empty());
    assert_eq!(f.attempt_count(), 0);
    assert_eq!(f.ledger_count(), 0);
    f.no_source_penalty();
}
#[tokio::test]
async fn declared_capability_replays_exact_reasoning_and_tool_identity() {
    let f = Fixture::new(&[Some(true)]).await;
    let response = f.send("/v1/responses", replay()).await;
    assert_eq!(response.status(), 200);
    let _ = response.json::<Value>().await.unwrap();
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let m = &seen[0].1["messages"][1];
    assert_eq!(m["reasoning_content"], "exact prior reasoning");
    assert_eq!(m["tool_calls"][0]["id"], "call_exact");
    assert_eq!(seen[0].1["messages"][2]["tool_call_id"], "call_exact");
    assert_eq!(f.attempt_count(), 1);
    assert_eq!(f.ledger_count(), 1);
    f.no_source_penalty();
}
#[tokio::test]
async fn incompatible_candidate_skips_locally_then_declared_source_succeeds() {
    let f = Fixture::new(&[None, Some(true)]).await;
    let response = f.send("/v1/responses", replay()).await;
    assert_eq!(response.status(), 200);
    let _ = response.bytes().await.unwrap();
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].0.starts_with("/1/"));
    assert_eq!(f.attempt_count(), 1);
    assert_eq!(f.ledger_count(), 1);
    f.no_source_penalty();
}
#[tokio::test]
async fn native_chat_raw_history_and_vendor_output_remain_unrestricted() {
    let f = Fixture::new(&[None]).await;
    let body = json!({"model":"logical","max_tokens":16,"messages":[{"role":"user","content":"go"},{"role":"assistant","content":"prior","reasoning_content":"native prior"},{"role":"user","content":"next"}]});
    let response = f.send("/v1/chat/completions", body).await;
    assert_eq!(response.status(), 200);
    let result = response.json::<Value>().await.unwrap();
    assert_eq!(
        result["choices"][0]["message"]["reasoning_content"],
        "new source reasoning"
    );
    assert_eq!(
        f.seen.lock().unwrap()[0].1["messages"][1]["reasoning_content"],
        "native prior"
    );
}
#[tokio::test]
async fn plain_input_can_receive_reasoning_output_without_replay_capability() {
    let f = Fixture::new(&[None]).await;
    let response = f
        .send(
            "/v1/responses",
            json!({"model":"logical","input":"go","max_output_tokens":16}),
        )
        .await;
    assert_eq!(response.status(), 200);
    let result = response.json::<Value>().await.unwrap();
    assert!(result["output"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "reasoning"
            && item["content"][0]["text"] == "new source reasoning"));
}
