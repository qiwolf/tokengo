//! Real HTTP native opaque replay without fictitious pricing, with full-context admission.
use axum::{extract::State, routing::post, Json, Router};
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
type Seen = Arc<Mutex<Vec<Value>>>;
async fn source(State(seen): State<Seen>, Json(body): Json<Value>) -> Json<Value> {
    let second = body["messages"].as_array().unwrap().len() > 1;
    seen.lock().unwrap().push(body);
    let content = if second {
        json!([{"type":"text","text":"done"}])
    } else {
        json!([{"type":"thinking","thinking":"signed original","signature":"source-signature"},{"type":"tool_use","id":"call_exact","name":"sum","input":{"a":1}}])
    };
    Json(
        json!({"id":"msg_test","type":"message","role":"assistant","model":"physical","content":content,"stop_reason":if second{"end_turn"}else{"tool_use"},"stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":2}}),
    )
}
struct Fixture {
    state: SharedState,
    http: Http,
    _source: Http,
    seen: Seen,
    key: String,
    uid: i64,
    parent: i64,
    id: i64,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
impl Fixture {
    async fn new(price_input: Option<i64>) -> Self {
        let seen = Arc::new(Mutex::new(vec![]));
        let source = serve(
            Router::new()
                .route("/v1/messages", post(source))
                .with_state(seen.clone()),
        )
        .await;
        let dir = std::env::temp_dir().join(format!("source-context-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        let (key, uid, parent, id) = {
            let c = state.conn();
            let u = db::identity::create_user(&c, "context", "user", &Default::default()).unwrap();
            let (p, _) =
                db::identity::create_key(&c, u.id, "parent", &Default::default(), None, None)
                    .unwrap();
            let (_, key) =
                db::identity::create_key(&c, u.id, "child", &Default::default(), None, Some(p.id))
                    .unwrap();
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
            .unwrap()
            .0;
            db::channels::set_models(
                &c,
                tokengo_shared::ChannelId(id),
                &[db::channels::ModelMapping::new(
                    "logical",
                    Some("physical".into()),
                    false,
                )],
            )
            .unwrap();
            let cap = tokengo_server::protocol_config::Capabilities {
                native: vec![tokengo_server::protocol::Protocol::Messages],
                max_input_tokens: Some(4096),
                max_output_tokens: Some(64),
                ..Default::default()
            };
            tokengo_server::protocol_config::save(&c, id, &cap).unwrap();
            if let Some(input) = price_input {
                db::billing::create_price(
                    &c,
                    &db::billing::Price {
                        id: 0,
                        channel_id: id,
                        model: "logical".into(),
                        input_micro_per_million: 1,
                        output_micro_per_million: 1,
                        max_input_tokens: input,
                        max_output_tokens: 64,
                    },
                    "test",
                )
                .unwrap();
            }
            db::route_policy::save(
                &c,
                "logical",
                &db::route_policy::RoutePolicy {
                    retry_times: Some(0),
                    probe_enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
            (key, u.id, p.id, id)
        };
        let http = serve(app(state.clone())).await;
        Self {
            state,
            http,
            _source: source,
            seen,
            key,
            uid,
            parent,
            id,
            dir,
        }
    }
    async fn send(&self, messages: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/v1/messages", self.http.url))
            .bearer_auth(&self.key)
            .header("x-opencode-session", "stable-context-test")
            .json(&json!({"model":"logical","max_tokens":16,"messages":messages}))
            .send()
            .await
            .unwrap()
    }
    async fn first(&self) -> Value {
        let response = self
            .send(json!([{"role":"user","content":"compute"}]))
            .await;
        assert_eq!(response.status(), 200);
        response.json().await.unwrap()
    }
    fn replay(first: &Value) -> Value {
        json!([{"role":"user","content":"compute"},{"role":"assistant","content":first["content"]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_exact","content":"2"}]}])
    }
    fn parent_limit(&self, tpm: u64) {
        let c = self.state.conn();
        let mut key = db::identity::keys(&c, self.uid)
            .unwrap()
            .into_iter()
            .find(|x| x.id == self.parent)
            .unwrap();
        key.policy.tpm = Some(tpm);
        db::identity::update_key(&c, &key).unwrap();
    }
}
#[tokio::test]
async fn unpriced_source_bounds_allow_native_signed_history_two_rounds() {
    let f = Fixture::new(None).await;
    let first = f.first().await;
    let response = f.send(Fixture::replay(&first)).await;
    assert_eq!(response.status(), 200);
    let _ = response.bytes().await.unwrap();
    assert_eq!(f.seen.lock().unwrap().len(), 2);
    let rows = db::billing::usage(&f.state.conn(), &Default::default()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].input_bound, 4096);
    assert_eq!(rows[0].debit_micro, None);
    assert_eq!(rows[0].usage_certainty, "unpriced");
    assert_eq!(rows[0].input_tokens, Some(3));
}
#[tokio::test]
async fn ancestor_tpm_reserves_full_opaque_context_before_send() {
    let f = Fixture::new(None).await;
    let first = f.first().await;
    f.parent_limit(4095);
    let response = f.send(Fixture::replay(&first)).await;
    assert_eq!(response.status(), 429);
    assert_eq!(f.seen.lock().unwrap().len(), 1);
    assert_eq!(
        db::billing::usage(&f.state.conn(), &Default::default())
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn source_bounds_change_invalidates_existing_opaque_pin() {
    let f = Fixture::new(None).await;
    let first = f.first().await;
    let mut cap = tokengo_server::protocol_config::load(&f.state.conn(), f.id).unwrap();
    cap.max_input_tokens = Some(4097);
    tokengo_server::protocol_config::save(&f.state.conn(), f.id, &cap).unwrap();
    let response = f.send(Fixture::replay(&first)).await;
    assert_eq!(response.status(), 400);
    assert_eq!(f.seen.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn smaller_price_cap_cannot_replace_full_source_context() {
    let f = Fixture::new(Some(512)).await;
    let first = f.first().await;
    let response = f.send(Fixture::replay(&first)).await;
    assert_eq!(response.status(), 400);
    assert_eq!(f.seen.lock().unwrap().len(), 1);
    assert_eq!(
        db::billing::usage(&f.state.conn(), &Default::default())
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn context_declaration_does_not_turn_unknown_money_into_free_price() {
    let f = Fixture::new(None).await;
    db::billing::set_budget(&f.state.conn(), "user", f.uid, Some(1500), "test").unwrap();
    let response = f.send(json!([{"role":"user","content":"compute"}])).await;
    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["type"], "permission_error");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("config_required"));
    assert!(f.seen.lock().unwrap().is_empty());
    assert!(db::billing::usage(&f.state.conn(), &Default::default())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn queued_opaque_price_context_growth_cannot_exceed_admitted_input() {
    let f = Fixture::new(None).await;
    let mut cap = tokengo_server::protocol_config::load(&f.state.conn(), f.id).unwrap();
    cap.max_input_tokens = None;
    cap.max_output_tokens = None;
    tokengo_server::protocol_config::save(&f.state.conn(), f.id, &cap).unwrap();
    let price = |input| {
        db::billing::create_price(
            &f.state.conn(),
            &db::billing::Price {
                id: 0,
                channel_id: f.id,
                model: "logical".into(),
                input_micro_per_million: 1,
                output_micro_per_million: 1,
                max_input_tokens: input,
                max_output_tokens: 64,
            },
            "test",
        )
        .unwrap()
    };
    price(512);
    f.parent_limit(2000);
    let first = f.first().await;
    let count_before: i64 = f
        .state
        .conn()
        .query_row("SELECT COUNT(*) FROM quota_events", [], |r| r.get(0))
        .unwrap();
    let held = f
        .state
        .channel_gate(f.id, 1)
        .unwrap()
        .try_acquire_owned()
        .unwrap();
    let url = format!("{}/v1/messages", f.http.url);
    let key = f.key.clone();
    let body = json!({"model":"logical","max_tokens":16,"messages":Fixture::replay(&first)});
    let task = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .bearer_auth(key)
            .header("x-opencode-session", "stable-context-test")
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
            if count > count_before {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    price(1024);
    drop(held);
    let r = task.await.unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(f.seen.lock().unwrap().len(), 1);
    assert_eq!(
        db::billing::usage(&f.state.conn(), &Default::default())
            .unwrap()
            .len(),
        1
    );
}
