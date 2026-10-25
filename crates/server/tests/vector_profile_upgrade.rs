//! Voluntary workload-profile upgrades must preserve valid old service on failure.
use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokengo_server::{
    AppState, Config, SharedState,
    db::{self, user_virtual_routes as vr},
    vector_compat, vector_renewal,
};
use tokengo_shared::{ChannelId, ModelKind};
#[derive(Clone, Copy, Default)]
enum Behavior {
    #[default]
    Good,
    WorkloadUnavailable,
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
fn embedding(text: &str, _behavior: Behavior) -> Vec<f64> {
    let mut seed = hash(text);
    let n = 8;
    let values: Vec<f64> = (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed % 10001) as f64 - 5000.0) / 5000.0
        })
        .collect();
    let norm = values.iter().map(|v| v * v).sum::<f64>().sqrt();
    values.into_iter().map(|v| v / norm).collect()
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
    if matches!(behavior, Behavior::WorkloadUnavailable)
        && (body["documents"].as_array().is_some_and(|docs| docs.len() >= 300)
            || body["input"].as_array().is_some_and(|items| items.len() >= 300))
    {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":{"message":"fixture large workload unavailable"}}))).into_response();
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


fn compatibility(f: &Fixture, shared: bool) -> (String, Value) {
    let prefix = format!("internal_vector_compat_v1:{}:", f.scope(shared));
    let (key, raw) = db::settings::all(&f.state.conn()).unwrap().into_iter().find(|(key, _)| key.starts_with(&prefix)).unwrap();
    (key, serde_json::from_str(&raw).unwrap())
}
fn old_profile(f: &Fixture, shared: bool) -> Value {
    let (key, mut value) = compatibility(f, shared);
    value["version"] = json!(1);
    value["max_input_chars"] = json!(512);
    value["max_query_chars"] = json!(0);
    value["max_batch"] = json!(16);
    value["max_documents"] = json!(128);
    value["max_total_chars"] = json!(8192);
    value["workload_profile"] = json!("");
    value["workload_verified_at_ms"] = Value::Null;
    db::settings::set(&f.state.conn(), &key, &value.to_string()).unwrap();
    assert!(f.require(shared).is_ok());
    value
}
async fn manual(f: &Fixture, shared: bool) -> Result<(), vr::VirtualError> {
    if shared {
        let actor = db::accounts::root(&f.state.conn()).unwrap().unwrap().account.id;
        vector_compat::verify_shared(&f.state, &f.config, actor).await
    } else {
        vector_compat::verify(&f.state, f.user, &f.config).await
    }
}

#[tokio::test]
async fn explicit_validation_upgrades_fresh_old_profiles_with_full_workload_while_tick_stays_light() {
    for shared in [false, true] {
        for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
            let f = Fixture::new(kind, 2).await;
            f.publish(shared).await;
            old_profile(&f, shared);
            vector_renewal::tick(&f.state).await.unwrap();
            assert_eq!(f.count(), 0, "fresh old profile does not trigger paid background load tests");
            manual(&f, shared).await.unwrap();
            f.diagnostics(shared);
            let (_, upgraded) = compatibility(&f, shared);
            assert_eq!(upgraded["version"], 4);
            assert!(upgraded["workload_verified_at_ms"].as_i64().unwrap() > 0);
            for source in &f.upstreams {
                let seen = source.state.seen.lock().unwrap();
                assert!(seen.iter().any(|body| if kind == ModelKind::Rerank {
                    body["documents"].as_array().is_some_and(|docs| docs.len() == 300)
                } else {
                    body["input"].as_array().is_some_and(|items| items.len() == 300)
                }), "explicit validation must execute actual 300-item verification on every source");
            }
            f.clear();
            vector_renewal::tick(&f.state).await.unwrap();
            assert_eq!(f.count(), 0);
        }
    }
}

#[tokio::test]
async fn failed_workload_upgrade_preserves_old_profile_and_complete_degraded_source_state() {
    for shared in [false, true] {
        for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
            let f = Fixture::new(kind, 2).await;
            f.publish(shared).await;
            let original_contract = old_profile(&f, shared);
            let prefix = format!("internal_vector_renewal_v1:{}:", f.scope(shared));
            let (renewal_key, raw) = db::settings::all(&f.state.conn()).unwrap().into_iter().find(|(key, _)| key.starts_with(&prefix)).unwrap();
            let mut previous: Value = serde_json::from_str(&raw).unwrap();
            previous["state"] = json!("degraded");
            previous["message"] = json!("fixture GPU healthy; backup isolated");
            previous["error_code"] = json!("");
            previous["next_retry_at_ms"] = json!(db::now_ms() + 300_000);
            previous["sources"] = json!(f.sources.iter().enumerate().map(|(index, id)| json!({
                "source_id": id.0, "logical_model": format!("vector-{index}"),
                "state": if index == 0 { "valid" } else { "isolated" },
                "checked_at_ms": db::now_ms(), "message": "fixture old source health",
                "error_code": if index == 0 { "" } else { "vector_source_unavailable" },
            })).collect::<Vec<_>>());
            db::settings::set(&f.state.conn(), &renewal_key, &previous.to_string()).unwrap();
            assert!(f.require(shared).is_ok());
            f.upstreams[1].set(Behavior::WorkloadUnavailable);
            let error = manual(&f, shared).await.unwrap_err();
            assert_eq!(error.code, "vector_source_unavailable");
            assert_eq!(compatibility(&f, shared).1, original_contract, "failed profile is never published");
            assert!(f.require(shared).is_ok(), "previous healthy source must remain callable");
            let mut after: Value = serde_json::from_str(&db::settings::get(&f.state.conn(), &renewal_key).unwrap().unwrap()).unwrap();
            assert_eq!(after["profile_upgrade_error"], "vector_source_unavailable");
            assert!(!after["profile_upgrade_message"].as_str().unwrap().is_empty());
            for field in ["profile_upgrade_error", "profile_upgrade_message", "profile_upgrade_at_ms"] {
                after[field] = previous[field].clone();
            }
            assert_eq!(after, previous, "all original source and service state must be restored");
            let status = f.status(shared);
            assert_eq!(status["state"], "degraded");
            assert_eq!(status["available_sources"], 1);
            assert_eq!(status["profile_upgrade_error"], "vector_source_unavailable");
            assert!(status["message"].as_str().unwrap().contains("新负载范围校验未通过"));
            f.upstreams[1].set(Behavior::Good);
            manual(&f, shared).await.unwrap();
            assert_eq!(compatibility(&f, shared).1["version"], 4);
            assert_eq!(f.status(shared)["state"], "valid");
            assert_eq!(f.status(shared)["profile_upgrade_error"], "");
        }
    }
}

#[tokio::test]
async fn initial_workload_failure_does_not_create_usable_evidence_or_inherit_changed_identity() {
    let f = Fixture::new(ModelKind::Rerank, 2).await;
    f.upstreams[1].set(Behavior::WorkloadUnavailable);
    assert!(manual(&f, false).await.is_err());
    assert!(f.require(false).is_err());
    assert_eq!(f.status(false)["state"], "failed");
    f.retry_now();
    f.upstreams[1].set(Behavior::Good);
    manual(&f, false).await.unwrap();
    old_profile(&f, false);
    db::channels::set_secret(&f.state.conn(), &f.state.secrets, f.sources[1], "fixture-new-identity").unwrap();
    f.upstreams[1].set(Behavior::WorkloadUnavailable);
    assert!(manual(&f, false).await.is_err());
    assert!(f.require(false).is_err(), "changed source identity must not inherit old profile");
    assert_eq!(f.status(false)["state"], "failed");
}

#[tokio::test]
async fn full_profile_probe_lease_outlives_multi_source_verification_budget() {
    let f = Fixture::new(ModelKind::Rerank, 2).await;
    f.publish(false).await;
    old_profile(&f, false);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *f.upstreams[0].state.hold.lock().unwrap() = Some((entered.clone(), release.clone()));
    let state = f.state.clone();
    let user = f.user;
    let config = f.config.clone();
    let task = tokio::spawn(async move { vector_compat::verify(&state, user, &config).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified()).await.unwrap();
    let status = f.status(false);
    assert_eq!(status["state"], "checking");
    assert!(status["lease_until_ms"].as_i64().unwrap() - db::now_ms() >= 550_000,
        "two-source full verification needs 2 × 270s plus publication margin");
    *f.upstreams[0].state.hold.lock().unwrap() = None;
    release.notify_one();
    task.await.unwrap().unwrap();
}
