//! Actual-source image bounds and default output reservation, through real HTTP.
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
        json!({"id":"chat_bound","object":"chat.completion","model":"physical","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
    )
}
struct Fixture {
    state: SharedState,
    http: Http,
    _source: Http,
    seen: Seen,
    key: String,
    uid: i64,
    ids: Vec<i64>,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
impl Fixture {
    async fn new(
        contexts: &[Option<(i64, i64)>],
        tpm: Option<u64>,
        price_input: Option<i64>,
    ) -> Self {
        let seen = Arc::new(Mutex::new(vec![]));
        let source = serve(
            Router::new()
                .route("/{id}/v1/chat/completions", post(source))
                .with_state(seen.clone()),
        )
        .await;
        let dir = std::env::temp_dir().join(format!("admission-bounds-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        let (key, uid, ids) = {
            let c = state.conn();
            let u = db::identity::create_user(&c, "bounds", "user", &Default::default()).unwrap();
            let policy = db::identity::Policy {
                tpm,
                ..Default::default()
            };
            let (mut parent, _) =
                db::identity::create_key(&c, u.id, "parent", &Default::default(), None, None)
                    .unwrap();
            let (_, key) = db::identity::create_key(
                &c,
                u.id,
                "child",
                &Default::default(),
                None,
                Some(parent.id),
            )
            .unwrap();
            parent.policy = policy;
            db::identity::update_key(&c, &parent).unwrap();
            let mut ids = vec![];
            for (i, context) in contexts.iter().enumerate() {
                let id = db::channels::create(
                    &c,
                    &db::channels::NewChannel {
                        name: format!("source-{i}"),
                        base_url: format!("{}/{i}/v1", source.url),
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
                        Some("physical".into()),
                        true,
                    )],
                )
                .unwrap();
                tokengo_server::protocol_config::save(
                    &c,
                    id.0,
                    &tokengo_server::protocol_config::Capabilities {
                        native: vec![tokengo_server::protocol::Protocol::Chat],
                        native_input_audio: true,
                        max_input_tokens: context.map(|v| v.0),
                        max_output_tokens: context.map(|v| v.1),
                        ..Default::default()
                    },
                )
                .unwrap();
                if let Some(input) = price_input {
                    Self::price(&c, id.0, input);
                }
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
            (key, u.id, ids)
        };
        let http = serve(app(state.clone())).await;
        Self {
            state,
            http,
            _source: source,
            seen,
            key,
            uid,
            ids,
            dir,
        }
    }
    fn price(c: &rusqlite::Connection, id: i64, input: i64) {
        db::billing::create_price(
            c,
            &db::billing::Price {
                id: 0,
                channel_id: id,
                model: "logical".into(),
                input_micro_per_million: 1_000_000,
                output_micro_per_million: 1_000_000,
                max_input_tokens: input,
                max_output_tokens: 64,
            },
            "test",
        )
        .unwrap();
    }
    async fn send(&self, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/v1/chat/completions", self.http.url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    fn reserved(&self) -> i64 {
        self.state.conn().query_row("SELECT tokens FROM quota_events WHERE scope='user' AND scope_id=?1 ORDER BY rowid DESC LIMIT 1",[self.uid],|r|r.get(0)).unwrap()
    }
    fn attempts(&self) -> i64 {
        self.state
            .conn()
            .query_row("SELECT COUNT(*) FROM attempts", [], |r| r.get(0))
            .unwrap()
    }
}
fn image() -> Value {
    json!({"model":"logical","max_tokens":16,"messages":[{"role":"user","content":[{"type":"text","text":"color?"},{"type":"image_url","image_url":{"url":"data:image/png;base64,iVBORw0KGgo="}}]}]})
}
fn audio() -> Value {
    json!({"model":"logical","max_tokens":16,"messages":[{"role":"user","content":[{"type":"input_audio","input_audio":{"data":"YQ==","format":"wav"}}]}]})
}
#[tokio::test]
async fn ancestor_tpm_image_skips_source_without_own_context() {
    let f = Fixture::new(&[None, Some((512, 64))], Some(1000), None).await;
    let r = f.send(image()).await;
    assert_eq!(r.status(), 200);
    r.bytes().await.unwrap();
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "/1/v1/chat/completions");
    drop(seen);
    assert_eq!(f.attempts(), 1);
    assert_eq!(f.reserved(), 528);
    let rows = db::billing::usage(&f.state.conn(), &Default::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].channel_id, f.ids[1]);
    assert_eq!(rows[0].input_bound, 512);
    assert_eq!(rows[0].input_tokens, Some(3));
    assert_eq!(rows[0].debit_micro, None);
}
#[tokio::test]
async fn image_without_tpm_or_budget_retains_unbounded_native_behavior() {
    let f = Fixture::new(&[None], None, None).await;
    let r = f.send(image()).await;
    assert_eq!(r.status(), 200);
    r.bytes().await.unwrap();
    assert_eq!(f.attempts(), 1);
}
async fn default_output(n: i64) {
    let f = Fixture::new(&[Some((512, 64))], Some(1000), None).await;
    let body = json!({"model":"logical","n":n,"messages":[{"role":"user","content":"hi"}]});
    let input = serde_json::to_vec(&body).unwrap().len() as i64 + 256;
    let r = f.send(body).await;
    assert_eq!(r.status(), 200);
    r.bytes().await.unwrap();
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1["max_tokens"], 64 / n);
    drop(seen);
    assert_eq!(f.reserved(), input + 64);
    let rows = db::billing::usage(&f.state.conn(), &Default::default()).unwrap();
    assert_eq!(rows[0].output_bound, 64);
    assert_eq!(f.attempts(), 1);
}
#[tokio::test]
async fn default_output_cap_uses_source_ceiling_before_tpm_admission() {
    default_output(1).await;
}
#[tokio::test]
async fn default_parallel_output_cap_reserves_total_source_ceiling() {
    default_output(2).await;
}
async fn queued_price_growth(body: Value) {
    let f = Fixture::new(&[None], Some(2000), Some(512)).await;
    let held = f
        .state
        .channel_gate(f.ids[0], 1)
        .unwrap()
        .try_acquire_owned()
        .unwrap();
    let url = format!("{}/v1/chat/completions", f.http.url);
    let key = f.key.clone();
    let task = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let count: i64 = f
                .state
                .conn()
                .query_row("SELECT COUNT(*) FROM quota_events", [], |r| r.get(0))
                .unwrap();
            if count > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.reserved(), 528);
    Fixture::price(&f.state.conn(), f.ids[0], 1024);
    drop(held);
    let r = task.await.unwrap();
    assert_eq!(r.status(), 400);
    assert!(f.seen.lock().unwrap().is_empty());
    assert_eq!(f.attempts(), 0);
    assert!(db::billing::usage(&f.state.conn(), &Default::default())
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn queued_image_price_context_growth_cannot_exceed_admitted_input() {
    queued_price_growth(image()).await;
}
#[tokio::test]
async fn queued_audio_price_context_growth_cannot_exceed_admitted_input() {
    queued_price_growth(audio()).await;
}

#[tokio::test]
async fn default_output_admission_uses_maximum_of_candidate_source_ceilings() {
    let f = Fixture::new(&[Some((512, 64)), Some((512, 128))], Some(1000), None).await;
    let body = json!({"model":"logical","messages":[{"role":"user","content":"hi"}]});
    let input = serde_json::to_vec(&body).unwrap().len() as i64 + 256;
    let r = f.send(body).await;
    assert_eq!(r.status(), 200);
    r.bytes().await.unwrap();
    assert_eq!(f.reserved(), input + 128);
    assert_eq!(f.seen.lock().unwrap()[0].1["max_tokens"], 64);
    assert_eq!(f.attempts(), 1);
}
