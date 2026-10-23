//! Vector fallback acceptance tests use real loopback HTTP, including verification probes.
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{
    app,
    db::{self, user_virtual_routes as vr},
    vector_compat, AppState, Config, SharedState,
};
use tokengo_shared::{ChannelId, ModelKind};
use tower::ServiceExt;

#[derive(Clone, Copy, Default)]
enum Behavior {
    #[default]
    Good,
    Status(u16),
    AccountFault,
    Slow,
    Dimension,
    DifferentSpace,
    ReverseRanking,
    BadIndex,
    WideEmbedding,
    Capacity,
}
#[derive(Default)]
struct UpstreamState {
    behavior: Mutex<Behavior>,
    seen: Mutex<Vec<Value>>,
    hold: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}
struct Upstream {
    state: Arc<UpstreamState>,
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Upstream {
    async fn start() -> Self {
        let state = Arc::new(UpstreamState::default());
        let router = Router::new()
            .route("/v1/embeddings", post(upstream))
            .route("/v1/rerank", post(upstream))
            .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { state, base, task }
    }
    fn set(&self, value: Behavior) {
        *self.state.behavior.lock().unwrap() = value;
    }
    fn clear(&self) {
        self.state.seen.lock().unwrap().clear();
    }
    fn count(&self) -> usize {
        self.state.seen.lock().unwrap().len()
    }
}
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |v, b| {
        (v ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
fn embedding(text: &str, behavior: Behavior) -> Vec<f64> {
    let mut seed = hash(text);
    let n = if matches!(behavior, Behavior::Dimension) {
        7
    } else if matches!(behavior, Behavior::WideEmbedding) {
        1024
    } else {
        8
    };
    let values: Vec<f64> = (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed % 10001) as f64 - 5000.0) / 5000.0
        })
        .collect();
    let norm = values.iter().map(|v| v * v).sum::<f64>().sqrt();
    let sign = if matches!(behavior, Behavior::DifferentSpace) {
        -1.0
    } else {
        1.0
    };
    values.into_iter().map(|v| sign * v / norm).collect()
}
async fn upstream(
    State(state): State<Arc<UpstreamState>>,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    state.seen.lock().unwrap().push(body.clone());
    let behavior = *state.behavior.lock().unwrap();
    let hold = state.hold.lock().unwrap().clone();
    if let Some((entered, release)) = hold {
        entered.notify_one();
        release.notified().await;
    }
    if matches!(behavior, Behavior::Capacity) {
        if uri.path().ends_with("embeddings") {
            return (StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"message":"input (8193 tokens) is too large to process. increase the physical batch size (current batch size: 8192)"}}))).into_response();
        }
        return (StatusCode::UNPROCESSABLE_ENTITY,Json(json!({"detail":{"reason":"encoded_query_document_pair_exceeds_8192_no_truncation_performed","max_encoded_pair_tokens":8193}}))).into_response();
    }
    if matches!(behavior, Behavior::AccountFault) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":{"code":"invalid_api_key","message":"fixture expired key"}})),
        )
            .into_response();
    }
    if let Behavior::Status(code) = behavior {
        return (
            StatusCode::from_u16(code).unwrap(),
            Json(json!({"error":{"message":"controlled failure"}})),
        )
            .into_response();
    }
    if matches!(behavior, Behavior::Slow) {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    if uri.path().ends_with("embeddings") {
        let input: Vec<&str> = if let Some(s) = body["input"].as_str() {
            vec![s]
        } else {
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect()
        };
        let data: Vec<_> = input.iter().enumerate().map(|(i,text)| json!({
            "object":"embedding", "index":if matches!(behavior,Behavior::BadIndex) { 999 } else { i },
            "embedding":embedding(text, behavior)
        })).collect();
        return Json(json!({"object":"list","model":body["model"],"data":data,"usage":{"prompt_tokens":3,"total_tokens":3}})).into_response();
    }
    let docs = body["documents"].as_array().unwrap();
    let query = body["query"].as_str().unwrap_or("");
    let mut rows: Vec<(usize, f64)> = docs
        .iter()
        .enumerate()
        .map(|(i, doc)| {
            let text = doc.as_str().or_else(|| doc["text"].as_str()).unwrap_or("");
            (
                i,
                (hash(&format!("{query}:{text}")) % 1_000_000) as f64 / 1_000_000.0,
            )
        })
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    if matches!(behavior, Behavior::ReverseRanking) {
        rows.reverse();
    }
    let take = body["top_n"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(docs.len());
    let results: Vec<_> = rows.into_iter().take(take).map(|(i,score)| {
        let mut row = json!({"index":if matches!(behavior,Behavior::BadIndex) {999} else {i},"relevance_score":score});
        if body["return_documents"].as_bool().unwrap_or(true) {
            row["document"] = if docs[i].is_string() {json!({"text":docs[i]})} else {docs[i].clone()};
        }
        row
    }).collect();
    Json(json!({"results":results,"usage":{"prompt_tokens":3,"total_tokens":3}})).into_response()
}
struct Fixture {
    state: SharedState,
    dir: std::path::PathBuf,
    user: i64,
    key: String,
    config: vr::Config,
    sources: Vec<ChannelId>,
    upstreams: Vec<Upstream>,
    kind: ModelKind,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl Fixture {
    async fn new(kind: ModelKind, count: usize) -> Self {
        let dir = std::env::temp_dir().join(format!("vector-fallback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        let (user, key) = {
            let c = state.conn();
            let user = db::identity::create_user(&c, "vector-owner", "user", &Default::default())
                .unwrap()
                .id;
            let (_, key) =
                db::identity::create_key(&c, user, "client", &Default::default(), None, None)
                    .unwrap();
            (user, key)
        };
        db::billing::set_budget(
            &state.conn(), "user", user, Some(1_000_000_000), "fixture-full-workload",
        ).unwrap();
        let mut upstreams = Vec::new();
        let mut sources = Vec::new();
        let mut targets = Vec::new();
        for i in 0..count {
            let upstream = Upstream::start().await;
            let c = state.conn();
            let name = format!("vector-{i}");
            let source = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: name.clone(),
                    base_url: upstream.base.clone(),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap();
            db::channels::set_secret(&c, &state.secrets, source, "fixture-upstream-secret")
                .unwrap();
            db::channels::set_models(
                &c,
                source,
                &[db::channels::ModelMapping::new(&name, None, false)],
            )
            .unwrap();
            db::kinds::set(&c, &name, kind).unwrap();
            db::provider_pool::backfill(&c, &state.secrets).unwrap();
            let provider = db::provider_pool::source_credential(&c, source.0)
                .unwrap()
                .unwrap()
                .provider_id;
            targets.push(vr::Target {
                provider_id: provider,
                logical_model: name.clone(),
                switch_on_429: true,
                switch_on_400: false,
            });
            db::billing::create_price(
                &c,
                &db::billing::Price {
                    id: 0,
                    channel_id: source.0,
                    model: name,
                    input_micro_per_million: 1_000_000,
                    output_micro_per_million: 0,
                    max_input_tokens: 8_000_000,
                    max_output_tokens: 0,
                },
                "fixture",
            )
            .unwrap();
            sources.push(source);
            upstreams.push(upstream);
        }
        state.routing().cfg.retry_times = 0;
        let config = vr::Config {
            name: "my-vector".into(),
            enabled: true,
            mode: "ordered".into(),
            timezone: "UTC".into(),
            targets,
            windows: vec![],
            default_positions: vec![],
        };
        Self {
            state,
            dir,
            user,
            key,
            config,
            sources,
            upstreams,
            kind,
        }
    }
    async fn verify_save(&self) {
        vector_compat::verify(&self.state, self.user, &self.config)
            .await
            .unwrap();
        vr::create(&self.state.conn(), self.user, &self.config).unwrap();
        self.assert_diagnostics();
        self.clear();
    }
    fn clear(&self) {
        for upstream in &self.upstreams {
            upstream.clear();
        }
    }
    fn assert_diagnostics(&self) {
        let c = self.state.conn();
        let rows: Vec<(i64, i64, i64, Option<i64>, String)> = {
            let mut q=c.prepare("SELECT user_id,key_id,channel_id,settled_ms,auth_snapshot FROM billing_ledger WHERE request_id=0 ORDER BY id").unwrap();
            let rows = q
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            rows
        };
        assert!(
            !rows.is_empty(),
            "real compatibility probes must be accounted for"
        );
        for (user, key, source, settled, raw) in rows {
            assert_eq!(user, self.user);
            assert_eq!(key, 0);
            assert!(self.sources.iter().any(|id| id.0 == source));
            assert!(settled.is_some(), "compatibility probe must be settled");
            let auth: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(auth["kind"], "user_compatibility");
        }
    }
    fn cookie(&self) -> String {
        let c = self.state.conn();
        let hash = db::sessions::hash_password("vector-test-browser-password").unwrap();
        let account =
            db::accounts::create(&c, "vector-browser", "Vector Browser", self.user, &hash).unwrap();
        let credential = db::accounts::by_id(&c, account.id).unwrap().unwrap();
        let token = db::accounts::create_session(&c, &credential).unwrap();
        format!("tokengo_account_session={token}")
    }
    async fn browser(
        &self,
        method: &str,
        path: &str,
        cookie: &str,
        csrf: bool,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("cookie", cookie);
        if csrf {
            request = request.header("X-Tokengo-Panel", "1");
        }
        let response = app(self.state.clone())
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    fn body(&self) -> Value {
        if self.kind == ModelKind::Embeddings {
            json!({"model":self.config.name,"input":["hospital medicine","patient care"]})
        } else {
            json!({"model":self.config.name,"query":"hospital","documents":["patient care","a train","clinical medicine"],"top_n":2,"return_documents":true})
        }
    }
    async fn call(&self, body: Value) -> (StatusCode, Value) {
        let path = if self.kind == ModelKind::Embeddings {
            "/v1/embeddings"
        } else {
            "/v1/rerank"
        };
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", self.key))
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    async fn flush(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while self.state.pending_writes() > 0 {
                self.state.drain_writes();
                if self.state.pending_writes() > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }
        })
        .await
        .unwrap();
    }
    fn attempts(&self) -> Vec<i64> {
        let c = self.state.conn();
        let mut q = c
            .prepare(
                "SELECT channel_id FROM attempts WHERE request_id>0 ORDER BY request_id,ordinal",
            )
            .unwrap();
        let rows = q
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows
    }
}

#[tokio::test]
async fn single_target_keeps_working_without_a_compatibility_certificate() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 1).await;
        vr::create(&f.state.conn(), f.user, &f.config).unwrap();
        let (status, body) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert_eq!(f.upstreams[0].count(), 1);
    }
}

#[tokio::test]
async fn multiple_targets_require_real_verification_before_save() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        assert!(vr::create(&f.state.conn(), f.user, &f.config).is_err());
        f.verify_save().await;
        let (status, body) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert_eq!(f.upstreams[0].count(), 1);
        assert_eq!(f.upstreams[1].count(), 0);
    }
}

#[tokio::test]
async fn incompatible_dimensions_spaces_and_ranking_are_rejected() {
    for (kind, behavior) in [
        (ModelKind::Embeddings, Behavior::Dimension),
        (ModelKind::Embeddings, Behavior::DifferentSpace),
        (ModelKind::Rerank, Behavior::ReverseRanking),
    ] {
        let f = Fixture::new(kind, 2).await;
        f.upstreams[1].set(behavior);
        assert!(
            vector_compat::verify(&f.state, f.user, &f.config)
                .await
                .is_err(),
            "{kind:?} must not certify incompatible output"
        );
        assert!(vr::create(&f.state.conn(), f.user, &f.config).is_err());
    }
}

#[tokio::test]
async fn server_error_and_malformed_success_fall_back_with_real_attempt_records() {
    for (kind, behavior) in [
        (ModelKind::Embeddings, Behavior::Status(503)),
        (ModelKind::Embeddings, Behavior::Dimension),
        (ModelKind::Rerank, Behavior::Status(503)),
        (ModelKind::Rerank, Behavior::BadIndex),
    ] {
        let f = Fixture::new(kind, 2).await;
        f.verify_save().await;
        f.upstreams[0].set(behavior);
        let (status, body) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert_eq!(f.upstreams[0].count(), 1);
        assert_eq!(f.upstreams[1].count(), 1);
        f.flush().await;
        assert_eq!(
            f.attempts(),
            f.sources.iter().map(|s| s.0).collect::<Vec<_>>()
        );
        let c = f.state.conn();
        let requested: String = c
            .query_row("SELECT model FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(requested, f.config.name);
        let billed:i64=c.query_row("SELECT channel_id FROM billing_ledger WHERE request_id>0 AND settled_ms IS NOT NULL ORDER BY id DESC LIMIT 1",[],|r|r.get(0)).unwrap();
        assert_eq!(billed, f.sources[1].0);
    }
}

#[tokio::test]
async fn explicit_400_and_429_follow_each_target_switch_setting() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        for (code, allow) in [(400, false), (400, true), (429, false), (429, true)] {
            let mut f = Fixture::new(kind, 2).await;
            f.config.targets[0].switch_on_400 = allow;
            f.config.targets[0].switch_on_429 = allow;
            f.verify_save().await;
            f.upstreams[0].set(Behavior::Status(code));
            let (status, body) = f.call(f.body()).await;
            assert_eq!(
                status.is_success(),
                allow,
                "{kind:?}, HTTP {code}, allow={allow}: {body}"
            );
            assert_eq!(f.upstreams[0].count(), 1);
            assert_eq!(f.upstreams[1].count(), usize::from(allow));
        }
    }
}

#[tokio::test]
async fn verified_alias_rejects_changed_source_mapping_and_credentials() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        for change_mapping in [false, true] {
            let f = Fixture::new(kind, 2).await;
            f.verify_save().await;
            {
                let c = f.state.conn();
                if change_mapping {
                    db::channels::set_models(
                        &c,
                        f.sources[1],
                        &[db::channels::ModelMapping::new(
                            "vector-1",
                            Some("changed-model".into()),
                            false,
                        )],
                    )
                    .unwrap();
                } else {
                    db::channels::set_secret(
                        &c,
                        &f.state.secrets,
                        f.sources[1],
                        "rotated-fixture-secret",
                    )
                    .unwrap();
                }
            }
            let (status, body) = f.call(f.body()).await;
            assert!(
                !status.is_success(),
                "{kind:?}, changed mapping={change_mapping}: {body}"
            );
            assert_eq!(
                f.upstreams[0].count() + f.upstreams[1].count(),
                0,
                "stale contracts must fail before sending a request"
            );
        }
    }
}

#[tokio::test]
async fn valid_provider_parameters_and_inputs_beyond_sample_domain_forward_unchanged() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        f.verify_save().await;
        let mut custom = f.body();
        custom["provider_option"] = json!({"original": "preserve-me"});
        let mut long = f.body();
        if kind == ModelKind::Embeddings {
            long["input"] = json!("x".repeat(4097));
        } else {
            long["query"] = json!("x".repeat(4097));
        }
        let mut large = f.body();
        if kind == ModelKind::Embeddings {
            large["input"] = json!(vec!["short"; 301]);
        } else {
            large["documents"] = json!(vec!["short"; 301]);
        }
        for body in [custom, long, large] {
            let (status, output) = f.call(body.clone()).await;
            assert_eq!(status, StatusCode::OK, "{kind:?}: {output}");
            let mut sent = f.upstreams[0].state.seen.lock().unwrap().last().unwrap().clone();
            sent["model"] = body["model"].clone();
            assert_eq!(sent, body);
        }
        assert_eq!(f.upstreams[0].count(), 3);
        assert_eq!(f.upstreams[1].count(), 0);
    }
}

#[tokio::test]
async fn scheduled_reverse_chain_preserves_certificate_and_execution_order() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let mut f = Fixture::new(kind, 2).await;
        f.config.mode = "scheduled".into();
        f.config.default_positions = vec![2, 1];
        f.verify_save().await;
        f.upstreams[1].set(Behavior::Status(503));
        let (status, body) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        f.flush().await;
        assert_eq!(f.attempts(), vec![f.sources[1].0, f.sources[0].0]);
    }
}

#[tokio::test]
async fn verified_first_target_timeout_falls_back_within_request_budget() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        f.verify_save().await;
        {
            let mut routing = f.state.routing();
            routing.cfg.attempt_timeout_ms = 50;
            routing.cfg.nonstream_timeout_ms = 50;
            routing.cfg.request_budget_ms = 1000;
        }
        f.upstreams[0].set(Behavior::Slow);
        let (status, body) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert_eq!(f.upstreams[1].count(), 1);
    }
}

#[tokio::test]
async fn connection_failure_after_verification_falls_back() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        f.verify_save().await;
        f.upstreams[0].task.abort();
        // Ensure the listener has dropped before submitting the gateway request.
        tokio::task::yield_now().await;
        let (status, body) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert_eq!(f.upstreams[1].count(), 1);
        f.flush().await;
        assert_eq!(
            f.attempts(),
            f.sources.iter().map(|s| s.0).collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn expired_evidence_blocks_requests_and_can_be_refreshed() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        f.verify_save().await;
        {
            let c = f.state.conn();
            let certificates: Vec<_> = db::settings::all(&c)
                .unwrap()
                .into_iter()
                .filter(|(key, _)| key.starts_with("internal_vector_compat_v1:"))
                .collect();
            assert_eq!(certificates.len(), 1);
            for (key, raw) in certificates {
                let mut value: Value = serde_json::from_str(&raw).unwrap();
                value["verified_at_ms"] = json!(db::now_ms() - 25 * 60 * 60 * 1000);
                db::settings::set(&c, &key, &value.to_string()).unwrap();
            }
        }
        let (status, body) = f.call(f.body()).await;
        assert!(!status.is_success(), "{kind:?}: {body}");
        assert_eq!(f.upstreams[0].count() + f.upstreams[1].count(), 0);
        vector_compat::verify(&f.state, f.user, &f.config)
            .await
            .unwrap();
        f.clear();
        let (status, body) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
    }
}

#[tokio::test]
async fn browser_save_automatically_verifies_and_bills_the_owning_user() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        let cookie = f.cookie();
        let (status, body) = f
            .browser(
                "POST",
                "/api/account/virtual-models",
                &cookie,
                true,
                serde_json::to_value(&f.config).unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert!(body["virtual_model"]["id"].as_i64().is_some());
        assert!(f.upstreams.iter().all(|source| source.count() > 0));
        f.assert_diagnostics();
        assert!(
            f.attempts().is_empty(),
            "compatibility probes are diagnostics, not user proxy requests"
        );
        f.clear();
        f.upstreams[0].set(Behavior::Status(503));
        for _ in 0..3 {
            assert_eq!(f.call(f.body()).await.0, StatusCode::OK);
        }
        assert_eq!(f.upstreams[0].count(), 3);
        {
            let c = f.state.conn();
            let until: i64 = c
                .query_row(
                    "SELECT until_ms FROM model_cooldowns WHERE channel_id=?1",
                    [f.sources[0].0],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(
                until > db::now_ms(),
                "first source must actually be cooling"
            );
        }
        f.clear();
        let (status, body) = f.call(f.body()).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "cooled first source must not invalidate certificate: {body}"
        );
        assert_eq!(f.upstreams[0].count(), 0);
        assert_eq!(f.upstreams[1].count(), 1);
    }
}

#[tokio::test]
async fn browser_forbidden_csrf_and_unknown_edit_never_dispatch_probe_requests() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        let cookie = f.cookie();
        let config = serde_json::to_value(&f.config).unwrap();
        assert_eq!(
            f.browser(
                "POST",
                "/api/account/virtual-models",
                &cookie,
                false,
                config.clone()
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        let mut edit = config.clone();
        edit["version"] = json!(1);
        assert_eq!(
            f.browser(
                "PATCH",
                "/api/account/virtual-models/999999",
                &cookie,
                true,
                edit
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        {
            let c = f.state.conn();
            let mut user = db::identity::user(&c, f.user).unwrap().unwrap();
            user.policy.allowed_models = Some(vec!["vector-0".into()]);
            db::identity::update_user(&c, &user).unwrap();
        }
        let (status, body) = f
            .browser("POST", "/api/account/virtual-models", &cookie, true, config)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{kind:?}: {body}");
        assert_eq!(f.upstreams.iter().map(Upstream::count).sum::<usize>(), 0);
        let c = f.state.conn();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM billing_ledger", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(vr::list(&c, f.user).unwrap().is_empty());
    }
}

#[tokio::test]
async fn browser_verification_respects_owner_budget_before_network_and_rpm_between_probes() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        for budget_block in [true, false] {
            let f = Fixture::new(kind, 2).await;
            let cookie = f.cookie();
            {
                let c = f.state.conn();
                if budget_block {
                    db::billing::set_budget(&c, "user", f.user, Some(0), "test").unwrap();
                } else {
                    let mut user = db::identity::user(&c, f.user).unwrap().unwrap();
                    user.policy.rpm = Some(1);
                    db::identity::update_user(&c, &user).unwrap();
                }
            }
            let (status, body) = f
                .browser(
                    "POST",
                    "/api/account/virtual-models",
                    &cookie,
                    true,
                    serde_json::to_value(&f.config).unwrap(),
                )
                .await;
            assert!(
                !status.is_success(),
                "{kind:?}, budget_block={budget_block}: {body}"
            );
            assert_eq!(
                f.upstreams.iter().map(Upstream::count).sum::<usize>(),
                if budget_block { 0 } else { 1 }
            );
            if !budget_block {
                f.assert_diagnostics();
            }
            let c = f.state.conn();
            assert!(vr::list(&c, f.user).unwrap().is_empty());
            assert!(
                db::settings::all(&c)
                    .unwrap()
                    .iter()
                    .all(|(k, _)| !k.starts_with("internal_vector_compat_v1:")),
                "partial verification must not issue a certificate"
            );
        }
    }
}

#[tokio::test]
async fn account_fault_falls_back_and_keeps_the_certificate_valid() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        f.verify_save().await;
        f.upstreams[0].set(Behavior::AccountFault);
        let (status, body) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert_eq!(f.upstreams[1].count(), 1);
        assert!(!db::faults::rows(&f.state.conn()).unwrap().is_empty());
        f.clear();
        let (status, body) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {body}");
        assert_eq!(f.upstreams[0].count(), 0);
        assert_eq!(f.upstreams[1].count(), 1);
    }
}

#[tokio::test]
async fn in_flight_kind_credentials_or_acl_changes_stop_before_the_next_attempt() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        for change in ["first_kind", "backup_secret", "backup_acl"] {
            let f = Fixture::new(kind, 2).await;
            f.verify_save().await;
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            *f.upstreams[0].state.hold.lock().unwrap() = Some((entered.clone(), release.clone()));
            f.upstreams[0].set(Behavior::Status(503));
            let mutate = async {
                tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
                    .await
                    .expect("first request reached upstream");
                {
                    let c = f.state.conn();
                    match change {
                        "first_kind" => db::kinds::set(&c, "vector-0", ModelKind::Chat).unwrap(),
                        "backup_secret" => db::channels::set_secret(
                            &c,
                            &f.state.secrets,
                            f.sources[1],
                            "rotated-in-flight",
                        )
                        .unwrap(),
                        _ => {
                            let mut user = db::identity::user(&c, f.user).unwrap().unwrap();
                            user.policy.allowed_channels = Some(vec![f.sources[0].0]);
                            db::identity::update_user(&c, &user).unwrap();
                        }
                    }
                }
                release.notify_one();
            };
            let ((status, body), ()) = tokio::join!(f.call(f.body()), mutate);
            assert!(!status.is_success(), "{kind:?}, change={change}: {body}");
            assert_eq!(f.upstreams[0].count(), 1);
            assert_eq!(
                f.upstreams[1].count(),
                0,
                "changed compatibility scope must fail closed before backup"
            );
        }
    }
}

#[tokio::test]
async fn revoked_browser_session_cannot_commit_after_async_verification() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        for editing in [false, true] {
            let mut f = Fixture::new(kind, 2).await;
            let id = if editing {
                f.verify_save().await;
                let id = vr::list(&f.state.conn(), f.user).unwrap()[0].id;
                f.config.targets[0].switch_on_400 = true;
                Some(id)
            } else {
                None
            };
            let cookie = f.cookie();
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            *f.upstreams[0].state.hold.lock().unwrap() = Some((entered.clone(), release.clone()));
            let mut body = serde_json::to_value(&f.config).unwrap();
            let path = if let Some(id) = id {
                body["version"] = json!(1);
                format!("/api/account/virtual-models/{id}")
            } else {
                "/api/account/virtual-models".into()
            };
            let revoke = async {
                tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
                    .await
                    .unwrap();
                db::accounts::revoke(
                    &f.state.conn(),
                    cookie.strip_prefix("tokengo_account_session=").unwrap(),
                )
                .unwrap();
                *f.upstreams[0].state.hold.lock().unwrap() = None;
                release.notify_one();
            };
            let ((status, response), ()) = tokio::join!(
                f.browser(
                    if editing { "PATCH" } else { "POST" },
                    &path,
                    &cookie,
                    true,
                    body
                ),
                revoke
            );
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{kind:?} edit={editing}: {response}"
            );
            let routes = vr::list(&f.state.conn(), f.user).unwrap();
            if editing {
                assert_eq!(routes[0].version, 1);
                assert!(!routes[0].config.targets[0].switch_on_400);
            } else {
                assert!(routes.is_empty());
            }
        }
    }
}


#[tokio::test]
async fn vector_refusals_keep_original_code_safe_size_and_identity_without_attempts() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        f.verify_save().await;
        let marker = "PRIVATE-DOCUMENT-DO-NOT-LOG";
        let mut body = f.body();
        if kind == ModelKind::Embeddings {
            body["input"] = json!([marker]);
        } else {
            body["query"] = json!(marker);
            body["documents"] = json!([marker, marker]);
        }
        if kind == ModelKind::Embeddings {
            body["dimensions"] = json!(0);
        } else {
            body["top_n"] = json!(0);
        }
        let expected_bytes = body.to_string().len();
        let (status, output) = f.call(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{output}");
        f.flush().await;
        let c = f.state.conn();
        let (status, http, code, note, user): (String, i64, String, String, i64) = c.query_row(
            "SELECT status,http_status,error_code,error,user_id FROM requests ORDER BY id DESC LIMIT 1",
            [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).unwrap();
        assert_eq!(status, "client_error");
        assert_eq!(http, 400);
        assert_eq!(code, output["error"]["type"].as_str().unwrap());
        assert_eq!(user, f.user);
        assert!(!note.contains(marker), "input text must never appear in refusal logs");
        let shape: Value = serde_json::from_str(note.split_once("request_shape=").unwrap().1).unwrap();
        assert_eq!(shape["json_bytes"], expected_bytes);
        if kind == ModelKind::Embeddings {
            assert_eq!(shape["input_count"], 1);
            assert_eq!(shape["input_total_chars"], marker.len());
        } else {
            assert_eq!(shape["query_chars"], marker.len());
            assert_eq!(shape["documents_count"], 2);
            assert_eq!(shape["document_max_chars"], marker.len());
        }
        let attempts: i64 = c.query_row("SELECT COUNT(*) FROM attempts WHERE request_id>0", [], |r|r.get(0)).unwrap();
        assert_eq!(attempts, 0);
        assert_eq!(f.upstreams.iter().map(Upstream::count).sum::<usize>(), 0);
        assert_eq!(c.query_row("SELECT COUNT(*) FROM billing_ledger WHERE request_id>0", [], |r|r.get::<_,i64>(0)).unwrap(), 0);
    }
}

#[tokio::test]
async fn changed_vector_source_refusal_is_queryable_before_any_attempt() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        f.verify_save().await;
        db::channels::set_secret(&f.state.conn(), &f.state.secrets, f.sources[1], "rotated-after-verification").unwrap();
        let (status, output) = f.call(f.body()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{output}");
        f.flush().await;
        let c = f.state.conn();
        let (http, code, note): (i64, String, String) = c.query_row(
            "SELECT http_status,error_code,error FROM requests ORDER BY id DESC LIMIT 1", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).unwrap();
        assert_eq!(http, 400);
        assert_eq!(code, output["error"]["type"].as_str().unwrap());
        assert_ne!(code, "model_not_configured");
        assert!(note.contains("request_shape="));
        drop(c);
        assert!(f.attempts().is_empty());
        assert_eq!(f.upstreams.iter().map(Upstream::count).sum::<usize>(), 0);
    }
}

#[tokio::test]
async fn route_budget_overrides_short_global_deadline_and_preserves_large_rerank_fallback() {
    for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
        let f = Fixture::new(kind, 2).await;
        f.verify_save().await;
        {
            let mut routing = f.state.routing();
            routing.cfg.nonstream_timeout_ms = 1;
            routing.cfg.request_budget_ms = 1;
        }
        db::route_policy::save(&f.state.conn(), &f.config.name, &db::route_policy::RoutePolicy {
            nonstream_timeout_ms: Some(500),
            request_budget_ms: Some(5000),
            retry_times: Some(0),
            ..Default::default()
        }).unwrap();
        f.upstreams[0].set(Behavior::Slow);
        let mut input = f.body();
        if kind == ModelKind::Rerank {
            input["documents"] = json!((0..300).map(|i|format!("{i}: {}", "original-document ".repeat(160))).collect::<Vec<_>>());
            input["top_n"] = json!(300);
            input["return_documents"] = json!(false);
        }
        let started = std::time::Instant::now();
        let (status, output) = f.call(input.clone()).await;
        assert_eq!(status, StatusCode::OK, "{kind:?}: {output}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(f.upstreams[0].count(), 1);
        assert_eq!(f.upstreams[1].count(), 1);
        for upstream in &f.upstreams {
            let mut sent = upstream.state.seen.lock().unwrap()[0].clone();
            sent["model"] = input["model"].clone();
            assert_eq!(sent, input, "fallback must preserve every candidate and original text");
        }
        if kind == ModelKind::Rerank {
            assert_eq!(output["results"].as_array().unwrap().len(), 300);
            let mut indices = output["results"].as_array().unwrap().iter().map(|row|row["index"].as_u64().unwrap()).collect::<Vec<_>>();
            indices.sort_unstable();
            assert_eq!(indices, (0..300).collect::<Vec<_>>());
        }
        f.flush().await;
        assert_eq!(f.attempts(), f.sources.iter().map(|id| id.0).collect::<Vec<_>>());
    }
}


#[tokio::test]
async fn real_proxy_accepts_300_long_chinese_documents_above_default_body_limit() {
    let f = Fixture::new(ModelKind::Rerank, 2).await;
    f.verify_save().await;
    let docs: Vec<String> = (0..300).map(|i| {
        format!("{i}: ").chars().chain("医院证据资料与临床研究".chars().cycle()).take(4096).collect()
    }).collect();
    let input = json!({"model":f.config.name,"query":"医院证据","documents":docs,"top_n":300,"return_documents":false});
    assert!(input.to_string().len() > 2 * 1024 * 1024);
    let (status, output) = f.call(input.clone()).await;
    assert_eq!(status, StatusCode::OK, "{output}");
    assert_eq!(output["results"].as_array().unwrap().len(), 300);
    assert_eq!(f.upstreams[0].state.seen.lock().unwrap()[0]["documents"], input["documents"]);
    assert_eq!(f.upstreams[1].count(), 0);
}

#[tokio::test]
async fn real_proxy_preserves_300_by_1024_embedding_response_above_old_four_mib_cap() {
    let f = Fixture::new(ModelKind::Embeddings, 2).await;
    for source in &f.upstreams { source.set(Behavior::WideEmbedding); }
    f.verify_save().await;
    let input = json!({"model":f.config.name,"input":(0..300).map(|i| format!("Original full input {i}")).collect::<Vec<_>>()});
    let (status, output) = f.call(input.clone()).await;
    assert_eq!(status, StatusCode::OK, "wide embedding must survive proxy response collection");
    assert!(output.to_string().len() > 4 * 1024 * 1024);
    let rows = output["data"].as_array().unwrap();
    assert_eq!(rows.len(), 300);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row["index"], index);
        assert_eq!(row["embedding"].as_array().unwrap().len(), 1024);
    }
    assert_eq!(f.upstreams[0].state.seen.lock().unwrap()[0]["input"], input["input"]);
    assert_eq!(f.upstreams[1].count(), 0);
}

#[tokio::test]
async fn precise_vector_capacity_refusals_fall_back_unchanged_without_cooldown_or_400_switch() {
    for kind in [ModelKind::Embeddings,ModelKind::Rerank] {
        let f=Fixture::new(kind,2).await;f.verify_save().await;
        assert!(!f.config.targets[0].switch_on_400);
        f.upstreams[0].set(Behavior::Capacity);
        let input=f.body();let(status,result)=f.call(input.clone()).await;
        assert_eq!(status,StatusCode::OK,"{kind:?}: {result}");
        assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),1);
        let key=if kind==ModelKind::Embeddings{"input"}else{"documents"};
        assert_eq!(f.upstreams[1].state.seen.lock().unwrap()[0][key],input[key]);
        f.flush().await;
        let c=f.state.conn();
        let(note,outcome):(String,String)=c.query_row("SELECT error,outcome FROM attempts ORDER BY id LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(outcome,"context_limit");assert!(note.contains("reported_limit_tokens"));
        assert!(!note.contains("hospital"));
        assert!(db::policy::peek(&c,f.sources[0].0,&f.config.targets[0].logical_model,&f.config.targets[0].logical_model,db::now_ms()).unwrap().is_none());
        drop(c);
        // A normal malformed422 remains a client error; the previous capacity attempt
        // must not suspend the source or silently bypass its next request.
        f.clear();f.upstreams[0].set(Behavior::Status(422));
        let(status,_)=f.call(f.body()).await;
        assert_eq!(status,StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),0);
        f.clear();for source in &f.upstreams {source.set(Behavior::Capacity);}
        let(status,original)=f.call(f.body()).await;
        assert_eq!(status,if kind==ModelKind::Embeddings {StatusCode::INTERNAL_SERVER_ERROR}else{StatusCode::UNPROCESSABLE_ENTITY});
        assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),1);
        if kind==ModelKind::Embeddings {assert!(original["error"]["message"].as_str().unwrap().starts_with("input (8193 tokens)"));}
        else {assert_eq!(original["detail"]["reason"],"encoded_query_document_pair_exceeds_8192_no_truncation_performed");}
    }
}
