//! Local admission failures must not masquerade as upstream 400 responses.
//! Requests exercise the real router, immutable prices and two loopback sources.
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{
    app,
    db::{self, user_virtual_routes as vr},
    protocol_config::{self, Capabilities},
    AppState, Config, SharedState,
};
use tokengo_shared::{ChannelId, ModelKind};
use tower::ServiceExt;

#[derive(Default)]
struct SourceState {
    seen: Mutex<Vec<Value>>,
    authorization: Mutex<Vec<String>>,
    fail_400: Mutex<bool>,
}
struct Source {
    state: Arc<SourceState>,
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Source {
    fn drop(&mut self) { self.task.abort(); }
}
async fn source(State(state): State<Arc<SourceState>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    state.seen.lock().unwrap().push(body.clone());
    state.authorization.lock().unwrap().push(headers.get("authorization").and_then(|value| value.to_str().ok()).unwrap_or("").to_owned());
    if *state.fail_400.lock().unwrap() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":{"message":"upstream rejected request"}}))).into_response();
    }
    Json(json!({
        "id":"chat_local_capacity", "object":"chat.completion", "model":body["model"],
        "choices":[{"index":0,"message":{"role":"assistant","content":"fallback recovered"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}
    })).into_response()
}
impl Source {
    async fn start() -> Self {
        let state = Arc::new(SourceState::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let router = Router::new().route("/v1/chat/completions", post(source)).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { state, base, task }
    }
    fn count(&self) -> usize { self.state.seen.lock().unwrap().len() }
}
struct Fixture {
    state: SharedState,
    dir: std::path::PathBuf,
    key: String,
    channels: Vec<ChannelId>,
    sources: Vec<Source>,
    config: vr::Config,
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
}
impl Fixture {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("billing-fallback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into_owned(),
            ..Config::default()
        }).unwrap();
        state.routing().cfg.retry_times = 0;
        state.routing().cfg.probe_enabled = false;
        let key = db::identity::create_key(&state.conn(), 1, "capacity-caller", &Default::default(), None, None).unwrap().1;
        db::sessions::set_panel_password(&state.conn(), "capacity-root-test-password").unwrap();
        let mut channels = Vec::new();
        let mut sources = Vec::new();
        let mut targets = Vec::new();
        for index in 0..2 {
            let upstream = Source::start().await;
            let name = format!("capacity-source-{index}");
            let c = state.conn();
            let id = db::channels::create(&c, &db::channels::NewChannel {
                name: name.clone(), base_url: upstream.base.clone(), enabled: true,
                unwrap_envelope: false, max_concurrent: 0,
            }).unwrap();
            db::channels::set_secret(&c, &state.secrets, id, "synthetic-capacity-source-key").unwrap();
            db::channels::set_models(&c, id, &[db::channels::ModelMapping::new(&name, None, false)]).unwrap();
            db::kinds::set(&c, &name, ModelKind::Chat).unwrap();
            db::provider_pool::backfill(&c, &state.secrets).unwrap();
            let provider = db::provider_pool::source_credential(&c, id.0).unwrap().unwrap().provider_id;
            targets.push(vr::Target { provider_id: provider, logical_model: name, switch_on_429: true, switch_on_400: false });
            channels.push(id);
            sources.push(upstream);
        }
        Self { state, dir, key, channels, sources, config: vr::Config {
            name: "capacity-fallback".into(), enabled: true, mode: "ordered".into(), timezone: "UTC".into(),
            targets, windows: vec![], default_positions: vec![],
        }}
    }
    fn price(&self, index: usize, input: i64, output: i64) {
        db::billing::create_price(&self.state.conn(), &db::billing::Price {
            currency: "CNY".into(), id: 0, channel_id: self.channels[index].0,
            model: self.config.targets[index].logical_model.clone(),
            input_micro_per_million: 1_000_000, output_micro_per_million: 1_000_000,
            max_input_tokens: input, max_output_tokens: output,
        }, "capacity-fixture").unwrap();
    }
    fn source_output(&self, index: usize, output: i64) {
        self.source_bounds(index, 100_000, output);
    }
    fn source_bounds(&self, index: usize, input: i64, output: i64) {
        protocol_config::save(&self.state.conn(), self.channels[index].0, &Capabilities {
            max_input_tokens: Some(input), max_output_tokens: Some(output), ..Default::default()
        }).unwrap();
    }
    fn publish(&self) {
        let c = self.state.conn();
        let root = db::accounts::root(&c).unwrap().unwrap().account.id;
        db::shared_virtual_routes::create(&c, &self.config, root).unwrap();
    }
    fn publish_private(&self) {
        db::user_virtual_routes::create(&self.state.conn(), 1, &self.config).unwrap();
    }
    fn combine_credentials(&mut self) -> i64 {
        let c = self.state.conn();
        let provider = self.config.targets[0].provider_id;
        let name = self.config.targets[0].logical_model.clone();
        db::channels::update(&c, self.channels[1], "second capacity credential", &self.sources[0].base, true, false, 0).unwrap();
        db::channels::set_secret(&c, &self.state.secrets, self.channels[1], "synthetic-second-capacity-key").unwrap();
        db::channels::set_models(&c, self.channels[1], &[db::channels::ModelMapping::new(&name, None, false)]).unwrap();
        db::provider_pool::adopt_source(&c, &self.state.secrets, self.channels[1].0, provider).unwrap();
        db::provider_channels::set_membership(&c, self.channels[1].0, Some(provider)).unwrap();
        self.config.targets[1].provider_id = provider;
        self.config.targets[1].logical_model = name;
        provider
    }
    async fn call(&self, content: &str) -> (StatusCode, Value) {
        let request = Request::builder().method("POST").uri("/v1/chat/completions")
            .header("content-type", "application/json").header("authorization", format!("Bearer {}", self.key))
            .body(Body::from(json!({"model":"capacity-fallback","messages":[{"role":"user","content":content}],"max_tokens":64}).to_string())).unwrap();
        let response = app(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn flush(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                self.state.drain_writes();
                let reserved: i64 = self.state.conn().query_row("SELECT count(*) FROM billing_ledger WHERE status='reserved'", [], |row| row.get(0)).unwrap();
                if self.state.pending_writes() == 0 && reserved == 0 { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("financial records must settle");
    }
}

#[tokio::test]
async fn local_billing_output_limit_skips_unsuitable_target_before_dispatch() {
    for switch_on_400 in [false, true] {
        let mut f = Fixture::new().await;
        f.config.targets[0].switch_on_400 = switch_on_400;
        f.price(0, 100_000, 16);
        f.price(1, 100_000, 1024);
        f.publish();
        let (status, body) = f.call("small request").await;
        assert_eq!(status, StatusCode::OK, "switch_on_400={switch_on_400}: {body}");
        assert_eq!(body["choices"][0]["message"]["content"], "fallback recovered");
        assert_eq!(f.sources[0].count(), 0, "local capacity failure must not be sent upstream");
        assert_eq!(f.sources[1].count(), 1, "next configured target must receive the original request");
        assert_eq!(f.sources[1].state.seen.lock().unwrap()[0]["max_tokens"], 64, "explicit caller cap must not be reduced to fit the first target");
        f.flush().await;
        let rows: Vec<i64> = f.state.conn().prepare("SELECT channel_id FROM billing_ledger WHERE request_id>0 ORDER BY id").unwrap()
            .query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(rows, vec![f.channels[1].0], "only the dispatched source owns a financial reservation");
    }
}

#[tokio::test]
async fn local_source_output_limit_skips_unsuitable_target_before_dispatch() {
    let f = Fixture::new().await;
    f.price(0, 100_000, 1024);
    f.price(1, 100_000, 1024);
    f.source_output(0, 16);
    f.source_output(1, 128);
    f.publish();
    let (status, body) = f.call("small request").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(f.sources[0].count(), 0);
    assert_eq!(f.sources[1].count(), 1);
}

#[tokio::test]
async fn all_local_billing_output_limits_return_the_actual_bound_without_dispatch() {
    let f = Fixture::new().await;
    f.price(0, 100_000, 16);
    f.price(1, 100_000, 32);
    f.publish();
    let (status, body) = f.call("small request").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["type"], "billing_token_limit", "{body}");
    let message = body["error"]["message"].as_str().unwrap().to_ascii_lowercase();
    assert!(message.contains("output") && message.contains("64") && message.contains("32"), "the exhausted route must report the requested output and the final source's limit: {body}");
    assert_eq!(f.sources.iter().map(Source::count).sum::<usize>(), 0);
    assert_eq!(f.state.conn().query_row("SELECT count(*) FROM billing_ledger WHERE request_id>0", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[tokio::test]
async fn real_upstream_400_still_respects_the_target_switch() {
    for allow in [false, true] {
        let mut f = Fixture::new().await;
        f.config.targets[0].switch_on_400 = allow;
        f.price(0, 100_000, 1024);
        f.price(1, 100_000, 1024);
        f.publish();
        *f.sources[0].state.fail_400.lock().unwrap() = true;
        let (status, body) = f.call("small request").await;
        assert_eq!(status.is_success(), allow, "switch_on_400={allow}: {body}");
        if !allow { assert_eq!(status, StatusCode::BAD_REQUEST); }
        assert_eq!(f.sources[0].count(), 1);
        assert_eq!(f.sources[1].count(), usize::from(allow));
    }
}

#[tokio::test]
async fn billing_input_limit_skips_only_the_source_that_cannot_reserve_its_input() {
    let f = Fixture::new().await;
    f.price(0, 128, 1024);
    f.price(1, 100_000, 1024);
    f.source_bounds(0, 100_000, 128);
    f.source_bounds(1, 100_000, 128);
    f.publish();
    let (status, body) = f.call(&"a".repeat(8000)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(f.sources[0].count(), 0);
    assert_eq!(f.sources[1].count(), 1);
    f.flush().await;
    let (channel, input): (i64, i64) = f.state.conn().query_row(
        "SELECT channel_id,input_bound FROM billing_ledger WHERE request_id>0 ORDER BY id DESC LIMIT 1",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(channel, f.channels[1].0);
    assert!(input >= 8000, "the financial reservation must not shrink to the first source's price cap: {input}");
}

#[tokio::test]
async fn configured_source_input_ceiling_caps_reservation_in_token_units() {
    let f = Fixture::new().await;
    f.price(0, 128, 1024);
    f.price(1, 100_000, 1024);
    f.source_bounds(0, 128, 128);
    f.publish();
    let (status, body) = f.call(&"a".repeat(2000)).await;
    assert_eq!(status, StatusCode::OK, "the source enforces its configured token ceiling; JSON bytes are not its token count: {body}");
    assert_eq!(f.sources[0].count(), 1);
    assert_eq!(f.sources[1].count(), 0);
    f.flush().await;
    let input: i64 = f.state.conn().query_row(
        "SELECT input_bound FROM billing_ledger WHERE request_id>0 ORDER BY id DESC LIMIT 1", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(input, 128, "reserve the entire configured source input ceiling, without dividing bytes or under-reserving");
}

#[tokio::test]
async fn private_alias_advances_target_after_local_capacity_rejection() {
    let f = Fixture::new().await;
    f.price(0, 100_000, 16);
    f.price(1, 100_000, 1024);
    f.publish_private();
    let (status, body) = f.call("small request").await;
    assert_eq!(status, StatusCode::OK, "a local capacity mismatch must advance the private alias cursor too: {body}");
    assert_eq!(f.sources[0].count(), 0);
    assert_eq!(f.sources[1].count(), 1);
}

#[tokio::test]
async fn provider_credential_capacity_rejection_does_not_block_the_next_key() {
    for preexisting_affinity in [false, true] {
        let mut f = Fixture::new().await;
        let provider = f.combine_credentials();
        f.price(0, 100_000, 16);
        f.price(1, 100_000, 1024);
        f.config.targets.truncate(1);
        f.publish();
        let first = db::provider_pool::source_credential(&f.state.conn(), f.channels[0].0).unwrap().unwrap();
        let second = db::provider_pool::source_credential(&f.state.conn(), f.channels[1].0).unwrap().unwrap();
        assert_ne!(first.id, second.id);
        let previous = if preexisting_affinity {
            Some(db::provider_pool::ensure_affinity_for_selection(&f.state.conn(), 1, provider, first.id).unwrap())
        } else { None };
        let (status, body) = f.call("small request").await;
        assert_eq!(status, StatusCode::OK, "preexisting_affinity={preexisting_affinity}: {body}");
        assert_eq!(f.sources[0].count(), 1, "one eligible credential should be dispatched at their common endpoint");
        assert_eq!(f.sources[0].state.authorization.lock().unwrap().as_slice(), &["Bearer synthetic-second-capacity-key".to_owned()], "the insufficient first key must never reach the endpoint");
        f.flush().await;
        let channel: i64 = f.state.conn().query_row("SELECT channel_id FROM billing_ledger WHERE request_id>0 ORDER BY id DESC LIMIT 1", [], |row| row.get(0)).unwrap();
        assert_eq!(channel, f.channels[1].0);
        let after = db::provider_pool::affinity_token(&f.state.conn(), 1, provider).unwrap().unwrap();
        if let Some(previous) = previous {
            assert_eq!(after, previous, "request-local capacity rejection must not rotate persistent user affinity");
        } else {
            assert_eq!(after.credential_id, second.id, "new affinity belongs to the key actually dispatched");
        }
    }
}

#[tokio::test]
async fn pinned_native_source_does_not_cross_sources_after_local_capacity_rejection() {
    use tokengo_server::{protocol::Protocol, protocol_sessions};
    let f = Fixture::new().await;
    f.price(0, 100_000, 16);
    f.price(1, 100_000, 1024);
    f.publish();
    let generation: i64 = f.state.conn().query_row(
        "SELECT binding_generation FROM channels WHERE id=?1", [f.channels[0].0], |row| row.get(0),
    ).unwrap();
    let pin = protocol_sessions::Pin { channel: f.channels[0].0, generation, upstream_model: f.config.targets[0].logical_model.clone(), protocol: Protocol::Chat };
    let owner = protocol_sessions::key(&tokengo_shared::short_hash(&f.key), "existing-native-session", "capacity-fallback", Protocol::Chat);
    assert!(protocol_sessions::claim(&f.state.conn(), &owner, &pin).unwrap());
    let request = Request::builder().method("POST").uri("/v1/chat/completions")
        .header("content-type", "application/json").header("authorization", format!("Bearer {}", f.key))
        .header("x-session-id", "existing-native-session")
        .body(Body::from(json!({"model":"capacity-fallback","messages":[{"role":"user","content":"continue the pinned session"}],"max_tokens":64}).to_string())).unwrap();
    let response = app(f.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["type"], "billing_token_limit", "{body}");
    assert_eq!(f.sources.iter().map(Source::count).sum::<usize>(), 0, "a pinned session must not migrate to a larger source");
    assert_eq!(protocol_sessions::load(&f.state.conn(), &owner).unwrap(), Some(pin));
}
