//! Source-isolated renewal acceptance against real loopback HTTP upstreams.
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokengo_server::{
    app, AppState, Config, SharedState,
    db::{self, user_virtual_routes as vr},
    vector_compat, vector_renewal,
};
use tokengo_shared::{ChannelId, ModelKind};
use tower::ServiceExt;
#[derive(Clone, Copy, Default)]
enum Behavior {
    #[default]
    Good,
    Status(u16),
    Dimension,
    DifferentSpace,
    ReverseRanking,
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
    let sign = if matches!(behavior, Behavior::DifferentSpace) { -1.0 } else { 1.0 };
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
    if let Behavior::Status(code) = behavior {
        return (
            StatusCode::from_u16(code).unwrap(),
            Json(json!({"error":{"message":"controlled failure"}})),
        )
            .into_response();
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
        let data: Vec<_> = input
            .iter()
            .enumerate()
            .map(|(i, text)| {
                json!({
                    "object":"embedding", "index":i,
                    "embedding":embedding(text, behavior)
                })
            })
            .collect();
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
    if matches!(behavior, Behavior::ReverseRanking) { rows.reverse(); }
    let take = body["top_n"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(docs.len());
    let results: Vec<_> = rows
        .into_iter()
        .take(take)
        .map(|(i, score)| {
            let mut row = json!({"index":i,"relevance_score":score});
            if body["return_documents"].as_bool().unwrap_or(true) {
                row["document"] = if docs[i].is_string() {
                    json!({"text":docs[i]})
                } else {
                    docs[i].clone()
                };
            }
            row
        })
        .collect();
    Json(json!({"results":results,"usage":{"prompt_tokens":3,"total_tokens":3}})).into_response()
}
struct Fixture {
    state: SharedState,
    dir: std::path::PathBuf,
    user: i64,
    key: String,
    kind: ModelKind,
    config: vr::Config,
    sources: Vec<ChannelId>,
    upstreams: Vec<Upstream>,
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
        let user = {
            let c = state.conn();
            let user = db::identity::create_user(&c, "vector-owner", "user", &Default::default())
                .unwrap()
                .id;
            user
        };
        let (_, key) = db::identity::create_key(&state.conn(), user, "availability-client", &Default::default(), None, None).unwrap();
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
            kind,
            config,
            sources,
            upstreams,
        }
    }

    async fn publish(&self, shared: bool) {
        if shared {
            let actor = {
                let c = self.state.conn();
                db::accounts::sync_root_hash(&c, "fixture-not-used-for-login").unwrap();
                db::accounts::root(&c).unwrap().unwrap().account.id
            };
            vector_compat::verify_shared(&self.state, &self.config, actor)
                .await
                .unwrap();
            db::shared_virtual_routes::create(&self.state.conn(), &self.config, actor).unwrap();
        } else {
            vector_compat::verify(&self.state, self.user, &self.config)
                .await
                .unwrap();
            vr::create(&self.state.conn(), self.user, &self.config).unwrap();
        }
        self.clear();
    }
    fn clear(&self) {
        for u in &self.upstreams {
            u.clear();
        }
    }
    fn count(&self) -> usize {
        self.upstreams.iter().map(Upstream::count).sum()
    }
    fn scope(&self, shared: bool) -> i64 {
        if shared { 0 } else { self.user }
    }
    fn age(&self, hours: i64) {
        let c = self.state.conn();
        for (k, raw) in db::settings::all(&c).unwrap() {
            if k.starts_with("internal_vector_compat_v1:") {
                let mut v: Value = serde_json::from_str(&raw).unwrap();
                v["verified_at_ms"] = json!(db::now_ms() - hours * 60 * 60 * 1000);
                db::settings::set(&c, &k, &v.to_string()).unwrap();
            }
        }
    }
    fn retry_now(&self) {
        let c = self.state.conn();
        for (k, raw) in db::settings::all(&c).unwrap() {
            if k.starts_with("internal_vector_renewal_v1:") {
                let mut v: Value = serde_json::from_str(&raw).unwrap();
                v["next_retry_at_ms"] = json!(0);
                v["lease_until_ms"] = json!(0);
                db::settings::set(&c, &k, &v.to_string()).unwrap();
            }
        }
    }
    fn status(&self, shared: bool) -> Value {
        vector_renewal::status(&self.state.conn(), self.scope(shared), &self.config)
    }
    fn require(&self, shared: bool) -> Result<vector_compat::Contract, vr::VirtualError> {
        let c = self.state.conn();
        let kind = db::model_routes::routes_for(&c, &self.config.targets[0].logical_model)
            .unwrap()
            .unwrap()
            .kind;
        vector_compat::require(&c, self.scope(shared), &self.config.targets, kind)
    }
    fn diagnostics(&self, shared: bool) {
        let c = self.state.conn();
        let mut q=c.prepare("SELECT user_id,key_id,settled_ms,auth_snapshot FROM billing_ledger WHERE request_id=0").unwrap();
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap();
        let mut count = 0;
        for row in rows {
            let (user, key, settled, raw) = row.unwrap();
            assert_eq!(user, self.scope(shared));
            assert_eq!(key, 0);
            assert!(settled.is_some());
            let auth: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(
                auth["kind"],
                if shared {
                    "admin_diagnostic"
                } else {
                    "user_compatibility"
                }
            );
            count += 1;
        }
        assert!(count > 0);
    }
}


impl Fixture {
    async fn call(&self) -> (StatusCode, Value) {
        let (path, body) = if self.kind == ModelKind::Embeddings {
            ("/v1/embeddings", json!({"model":self.config.name,"input":["hospital medicine","patient care"]}))
        } else {
            ("/v1/rerank", json!({"model":self.config.name,"query":"hospital","documents":["patient care","a train","clinical medicine"],"top_n":2,"return_documents":true}))
        };
        let req = Request::builder().method("POST").uri(path)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", self.key))
            .body(Body::from(body.to_string())).unwrap();
        let res = app(self.state.clone()).oneshot(req).await.unwrap();
        let code = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    async fn assert_only_source(&self, index: usize) {
        self.clear();
        let (code, body) = self.call().await;
        assert_eq!(code, StatusCode::OK, "healthy compatible source must stay callable: {body}");
        for (i, source) in self.upstreams.iter().enumerate() {
            assert_eq!(source.count(), usize::from(i == index), "unexpected outbound source {i}: {body}");
        }
    }
}

#[tokio::test]
async fn unavailable_cloud_preserves_gpu_for_private_shared_embeddings_and_rerank_after_expiry() {
    for shared in [false, true] {
        for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
            for age in [19, 25] {
                let f = Fixture::new(kind, 2).await;
                f.publish(shared).await;
                f.age(age);
                f.upstreams[0].set(Behavior::Status(503));
                vector_renewal::tick(&f.state).await.unwrap();
                let status = f.status(shared);
                assert_eq!(status["state"], "degraded", "{status}");
                assert!(status["message"].as_str().is_some_and(|s| !s.is_empty()), "{status}");
                assert!(status["next_retry_at_ms"].as_i64().unwrap() > db::now_ms());
                assert!(f.require(shared).is_ok(), "fresh per-source evidence must survive old chain expiry");
                f.assert_only_source(1).await;
                f.clear();
                vector_renewal::tick(&f.state).await.unwrap();
                assert_eq!(f.count(), 0, "degraded retry uses five minute backoff");
                f.upstreams[0].set(Behavior::Good);
                f.retry_now();
                vector_renewal::tick(&f.state).await.unwrap();
                assert_eq!(f.status(shared)["state"], "valid");
                f.assert_only_source(0).await;
                f.diagnostics(shared);
            }
        }
    }
}

#[tokio::test]
async fn changed_dimensions_space_and_ranking_quarantine_only_the_incompatible_source() {
    for shared in [false, true] {
        for (kind, bad) in [(ModelKind::Embeddings, Behavior::Dimension),
                            (ModelKind::Embeddings, Behavior::DifferentSpace),
                            (ModelKind::Rerank, Behavior::ReverseRanking)] {
            let f = Fixture::new(kind, 2).await;
            f.publish(shared).await;
            f.age(25);
            f.upstreams[0].set(bad);
            vector_renewal::tick(&f.state).await.unwrap();
            assert_eq!(f.status(shared)["state"], "degraded");
            f.assert_only_source(1).await;
            f.upstreams[0].set(Behavior::Good);
            f.retry_now();
            vector_renewal::tick(&f.state).await.unwrap();
            assert_eq!(f.status(shared)["state"], "valid");
            f.assert_only_source(0).await;
        }
    }
}

#[tokio::test]
async fn all_sources_unavailable_returns_503_without_outbound_then_recovers() {
    for shared in [false, true] {
        for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
            let f = Fixture::new(kind, 2).await;
            f.publish(shared).await;
            f.age(25);
            for source in &f.upstreams { source.set(Behavior::Status(503)); }
            vector_renewal::tick(&f.state).await.unwrap();
            let status = f.status(shared);
            assert_eq!(status["state"], "failed", "{status}");
            assert!(!status["message"].as_str().unwrap().is_empty());
            f.clear();
            let (code, body) = f.call().await;
            assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{body}");
            assert_eq!(f.count(), 0, "quarantined sources must not receive user requests");
            f.upstreams[1].set(Behavior::Good);
            f.retry_now();
            vector_renewal::tick(&f.state).await.unwrap();
            assert_eq!(f.status(shared)["state"], "degraded");
            f.assert_only_source(1).await;
        }
    }
}

#[tokio::test]
async fn absent_initial_evidence_never_bootstraps_compatibility_from_one_surviving_source() {
    for shared in [false, true] {
        for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
            let f = Fixture::new(kind, 2).await;
            f.publish(shared).await;
            // Simulate a persisted legacy/imported route whose original evidence is absent.
            // Route creation itself correctly requires a complete initial compatibility proof.
            {
                let c = f.state.conn();
                let scope = f.scope(shared);
                for (key, _) in db::settings::all(&c).unwrap() {
                    if key.starts_with(&format!("internal_vector_compat_v1:{scope}:"))
                        || key.starts_with(&format!("internal_vector_reference_v1:{scope}:")) {
                        c.execute("DELETE FROM settings WHERE key=?1", [&key]).unwrap();
                    }
                }
            }
            f.upstreams[0].set(Behavior::Status(503));
            vector_renewal::tick(&f.state).await.unwrap();
            assert!(f.require(shared).is_err(), "single surviving source is not proof of initial multi-target compatibility");
            f.clear();
            let (code, body) = f.call().await;
            assert!(!code.is_success(), "unverified chain must stay unavailable: {body}");
            assert_eq!(f.count(), 0);
        }
    }
}

#[tokio::test]
async fn changed_source_identity_and_revoked_private_acl_cannot_reuse_degraded_evidence() {
    for shared in [false, true] {
        let f = Fixture::new(ModelKind::Embeddings, 2).await;
        f.publish(shared).await;
        f.age(25);
        f.upstreams[0].set(Behavior::Status(503));
        vector_renewal::tick(&f.state).await.unwrap();
        f.assert_only_source(1).await;
        db::channels::set_secret(&f.state.conn(), &f.state.secrets, f.sources[1], "new-source-identity-secret").unwrap();
        f.clear();
        assert!(f.require(shared).is_err());
        let (code, _) = f.call().await;
        assert!(!code.is_success());
        assert_eq!(f.count(), 0, "source identity mutation invalidates all old delegated evidence");
        vector_renewal::tick(&f.state).await.unwrap();
        assert!(f.require(shared).is_err(), "new source identity plus cloud outage must not reuse old partial proof");
        assert_ne!(f.status(shared)["state"], "degraded");
        f.upstreams[0].set(Behavior::Good);
        f.retry_now();
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(f.status(shared)["state"], "valid", "new fingerprint can establish fresh full-chain proof after every source recovers");
        f.assert_only_source(0).await;
    }
    let f = Fixture::new(ModelKind::Embeddings, 2).await;
    f.publish(false).await;
    f.age(25);
    f.upstreams[0].set(Behavior::Status(503));
    vector_renewal::tick(&f.state).await.unwrap();
    f.assert_only_source(1).await;
    let mut user = db::identity::user(&f.state.conn(), f.user).unwrap().unwrap();
    user.policy.allowed_models = Some(vec![]);
    db::identity::update_user(&f.state.conn(), &user).unwrap();
    f.clear();
    let (code, _) = f.call().await;
    assert!(!code.is_success());
    assert_eq!(f.count(), 0);
}

#[tokio::test]
async fn queued_request_rechecks_source_quarantine_before_outbound() {
    for shared in [false, true] {
        for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
            let f = Fixture::new(kind, 2).await;
            db::channels::update(&f.state.conn(), f.sources[0], "queued cloud", &f.upstreams[0].base, true, false, 1).unwrap();
            f.publish(shared).await;
            f.age(19);
            let permit = f.state.channel_gate(f.sources[0].0, 1).unwrap().try_acquire_owned().unwrap();
            let renew_while_waiting = async {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        let running: i64 = f.state.conn().query_row("SELECT count(*) FROM requests WHERE status='running'", [], |r| r.get(0)).unwrap();
                        if running > 0 { break; }
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                }).await.expect("user request must have loaded its old route snapshot and be waiting for cloud capacity");
                // A busy source cannot complete its diagnostic; the healthy GPU can.
                vector_renewal::tick(&f.state).await.unwrap();
                assert_eq!(f.status(shared)["state"], "degraded");
                f.clear();
                drop(permit);
            };
            let ((code, body), ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                tokio::join!(f.call(), renew_while_waiting)
            }).await.expect("quarantine recheck must not leave a user request waiting indefinitely");
            assert_eq!(f.upstreams[0].count(), 0, "a queued source quarantined after route loading must not receive user output: {code} {body}");
            assert!(code.is_success() || code == StatusCode::SERVICE_UNAVAILABLE,
                "a changed availability snapshot may safely retry or explicitly reject, never bypass quarantine: {code} {body}");
            if code.is_success() { assert_eq!(f.upstreams[1].count(), 1); }
        }
    }
}
