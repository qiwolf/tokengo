//! Automatic compatibility renewal against real loopback upstreams; no production calls.
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
    AppState, Config, SharedState,
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
                    max_input_tokens: 50000,
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

#[tokio::test]
async fn expired_private_and_shared_embedding_and_rerank_renew_without_saving() {
    for shared in [false, true] {
        for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
            let f = Fixture::new(kind, 2).await;
            f.publish(shared).await;
            f.age(25);
            assert!(f.require(shared).is_err());
            vector_renewal::tick(&f.state).await.unwrap();
            let renewed = f
                .require(shared)
                .expect("expired certificate must renew automatically");
            assert!(renewed.verified_at_ms > db::now_ms() - 60_000);
            assert_eq!(f.count(), if kind == ModelKind::Embeddings { 4 } else { 6 });
            let status = f.status(shared);
            assert_eq!(status["state"], "valid", "{status}");
            assert!(status["expires_at_ms"].as_i64().unwrap() > db::now_ms() + 23 * 60 * 60 * 1000);
            f.diagnostics(shared);
        }
    }
}

#[tokio::test]
async fn renewal_starts_before_expiry_and_does_not_probe_fresh_contracts() {
    for shared in [false, true] {
        let f = Fixture::new(ModelKind::Embeddings, 2).await;
        f.publish(shared).await;
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(
            f.count(),
            0,
            "fresh certificate must not create paid diagnostics"
        );
        f.age(19);
        assert!(f.require(shared).is_ok());
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(f.count(), 4);
        let renewed = f.require(shared).unwrap();
        assert!(renewed.verified_at_ms > db::now_ms() - 60_000);
        f.clear();
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(f.count(), 0);
    }
}

#[tokio::test]
async fn failed_renewal_pauses_chain_explains_failure_and_recovers_on_retry() {
    for shared in [false, true] {
        for kind in [ModelKind::Embeddings, ModelKind::Rerank] {
            let f = Fixture::new(kind, 2).await;
            f.publish(shared).await;
            f.age(19);
            for upstream in &f.upstreams {upstream.set(Behavior::Status(503));}
            vector_renewal::tick(&f.state).await.unwrap();
            let status = f.status(shared);
            assert_eq!(status["state"], "failed", "{status}");
            assert!(!status["message"].as_str().unwrap().is_empty());
            assert!(status["next_retry_at_ms"].as_i64().unwrap() > db::now_ms());
            assert!(
                f.require(shared).is_err(),
                "all sources failed, so even unexpired evidence cannot admit a request"
            );
            f.clear();
            vector_renewal::tick(&f.state).await.unwrap();
            assert_eq!(f.count(), 0, "backoff must prevent repeated paid probes");
            for upstream in &f.upstreams {upstream.set(Behavior::Good);}
            f.retry_now();
            vector_renewal::tick(&f.state).await.unwrap();
            assert_eq!(f.status(shared)["state"], "valid");
            assert!(f.require(shared).is_ok());
            f.diagnostics(shared);
        }
    }
}

#[tokio::test]
async fn numerical_incompatibility_isolates_only_the_affected_source() {
    for shared in [false, true] {
        let f = Fixture::new(ModelKind::Embeddings, 2).await;
        f.publish(shared).await;
        f.age(19);
        f.upstreams[1].set(Behavior::Dimension);
        vector_renewal::tick(&f.state).await.unwrap();
        let status=f.status(shared);assert_eq!(status["state"],"degraded");
        assert_eq!(status["available_sources"],1);
        assert!(f.require(shared).is_ok(),"healthy source remains usable after incompatible backup is isolated");
        let sources=status["sources"].as_array().unwrap();
        assert!(sources.iter().any(|source|source["source_id"]==f.sources[0].0&&source["state"]=="valid"));
        assert!(sources.iter().any(|source|source["source_id"]==f.sources[1].0&&source["state"]=="isolated"));
        f.upstreams[1].set(Behavior::Good);
        f.retry_now();
        vector_renewal::tick(&f.state).await.unwrap();
        assert!(f.require(shared).is_ok());
    }
}

#[tokio::test]
async fn disabled_routes_skip_renewal_and_expose_disabled_status() {
    for shared in [false, true] {
        let mut f = Fixture::new(ModelKind::Embeddings, 2).await;
        f.publish(shared).await;
        f.age(25);
        {
            let c = f.state.conn();
            if shared {
                c.execute("UPDATE shared_virtual_models SET enabled=0", [])
                    .unwrap();
            } else {
                c.execute("UPDATE user_virtual_models SET enabled=0", [])
                    .unwrap();
            }
        }
        f.config.enabled = false;
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(f.count(), 0);
        assert_eq!(f.status(shared)["state"], "disabled");
    }
}

#[tokio::test]
async fn concurrent_ticks_share_one_paid_probe_batch() {
    for shared in [false, true] {
        let f = Fixture::new(ModelKind::Embeddings, 2).await;
        f.publish(shared).await;
        f.age(25);
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *f.upstreams[0].state.hold.lock().unwrap() = Some((entered.clone(), release.clone()));
        let state = f.state.clone();
        let first = tokio::spawn(async move { vector_renewal::tick(&state).await });
        tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
            .await
            .expect("renewal should begin real outbound probe");
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(f.count(), 1, "second tick must not issue duplicate request");
        assert_eq!(f.status(shared)["state"], "checking");
        *f.upstreams[0].state.hold.lock().unwrap() = None;
        release.notify_one();
        first.await.unwrap().unwrap();
        assert_eq!(f.count(), 4, "one batch only");
        f.diagnostics(shared);
    }
}

#[tokio::test]
async fn disabled_sources_and_revoked_private_permissions_never_probe() {
    for shared in [false, true] {
        let f = Fixture::new(ModelKind::Embeddings, 2).await;
        f.publish(shared).await;
        f.age(25);
        db::channels::set_enabled(&f.state.conn(), f.sources[1], false).unwrap();
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(f.count(), 0);
        assert!(f.require(shared).is_err());
        let status = f.status(shared);
        assert_eq!(status["state"], "failed", "{status}");
        assert!(!status["message"].as_str().unwrap().is_empty());
    }
    let f = Fixture::new(ModelKind::Embeddings, 2).await;
    f.publish(false).await;
    f.age(25);
    {
        let c = f.state.conn();
        let mut owner = db::identity::user(&c, f.user).unwrap().unwrap();
        owner.policy.allowed_models = Some(vec![]);
        db::identity::update_user(&c, &owner).unwrap();
    }
    vector_renewal::tick(&f.state).await.unwrap();
    assert_eq!(f.count(), 0);
    assert!(f.require(false).is_err());
    assert_eq!(f.status(false)["state"], "failed");
}

#[tokio::test]
async fn changing_or_disabling_route_during_probe_prevents_publication() {
    for shared in [false, true] {
        for disable in [false, true] {
            let f = Fixture::new(ModelKind::Embeddings, 2).await;
            f.publish(shared).await;
            f.age(19);
            let before = f.require(shared).unwrap().verified_at_ms;
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            *f.upstreams[0].state.hold.lock().unwrap() = Some((entered.clone(), release.clone()));
            let state = f.state.clone();
            let renewal = tokio::spawn(async move { vector_renewal::tick(&state).await });
            tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
                .await
                .unwrap();
            {
                let c = f.state.conn();
                let sql = match (shared, disable) {
                    (true, true) => "UPDATE shared_virtual_models SET enabled=0",
                    (true, false) => "UPDATE shared_virtual_models SET version=version+1",
                    (false, true) => "UPDATE user_virtual_models SET enabled=0",
                    (false, false) => "UPDATE user_virtual_models SET version=version+1",
                };
                c.execute(sql, []).unwrap();
            }
            *f.upstreams[0].state.hold.lock().unwrap() = None;
            release.notify_one();
            renewal.await.unwrap().unwrap();
            assert_eq!(
                f.count(),
                1,
                "after route change, no further probe is authorized"
            );
            let c = f.state.conn();
            let certificates = db::settings::all(&c)
                .unwrap()
                .into_iter()
                .filter(|(k, _)| k.starts_with("internal_vector_compat_v1:"))
                .collect::<Vec<_>>();
            assert_eq!(certificates.len(), 1);
            let certificate: Value = serde_json::from_str(&certificates[0].1).unwrap();
            assert_eq!(
                certificate["verified_at_ms"], before,
                "stale probe must not publish fresh evidence"
            );
        }
    }
}

#[tokio::test]
async fn retry_in_progress_does_not_temporarily_restore_failed_evidence() {
    for shared in [false, true] {
        let f = Fixture::new(ModelKind::Embeddings, 2).await;
        f.publish(shared).await;
        f.age(19);
        for upstream in &f.upstreams {upstream.set(Behavior::Status(503));}
        vector_renewal::tick(&f.state).await.unwrap();
        assert!(f.require(shared).is_err());
        for upstream in &f.upstreams {upstream.set(Behavior::Good);}
        f.retry_now();
        f.clear();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *f.upstreams[0].state.hold.lock().unwrap() = Some((entered.clone(), release.clone()));
        let state = f.state.clone();
        let renewal = tokio::spawn(async move { vector_renewal::tick(&state).await });
        tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        assert_eq!(f.status(shared)["state"], "checking");
        assert!(
            f.require(shared).is_err(),
            "checking must retain prior failure until all probes pass"
        );
        *f.upstreams[0].state.hold.lock().unwrap() = None;
        release.notify_one();
        renewal.await.unwrap().unwrap();
        assert!(f.require(shared).is_ok());
        assert_eq!(f.status(shared)["state"], "valid");
    }
}

#[tokio::test]
async fn cancelled_scan_releases_billing_and_expired_durable_lease_can_recover() {
    for shared in [false, true] {
        let f = Fixture::new(ModelKind::Embeddings, 2).await;
        f.publish(shared).await;
        f.age(25);
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *f.upstreams[0].state.hold.lock().unwrap() = Some((entered.clone(), release.clone()));
        let state = f.state.clone();
        let renewal = tokio::spawn(async move { vector_renewal::tick(&state).await });
        tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        renewal.abort();
        assert!(renewal.await.unwrap_err().is_cancelled());
        assert!(
            f.require(shared).is_err(),
            "cancelled expired renewal cannot grant fallback"
        );
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(
            f.count(),
            1,
            "live durable lease prevents overlapping retry after cancellation"
        );
        *f.upstreams[0].state.hold.lock().unwrap() = None;
        release.notify_one();
        f.retry_now();
        f.clear();
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(f.count(), 4);
        assert!(f.require(shared).is_ok());
        f.diagnostics(shared);
    }
}

#[tokio::test]
async fn invalid_saved_administrator_is_visible_as_failed_without_new_probes() {
    let f=Fixture::new(ModelKind::Embeddings,2).await;f.publish(true).await;f.age(19);
    {let c=f.state.conn();let account=db::accounts::create(&c,"former-root","Former root",f.user,"fixture-no-login").unwrap();c.execute("UPDATE shared_virtual_models SET actor_account_id=?1",[account.id]).unwrap();}
    vector_renewal::tick(&f.state).await.unwrap();assert_eq!(f.count(),0,"ordinary account cannot authorize shared diagnostic calls");
    let status=f.status(true);assert_eq!(status["state"],"failed","{status}");
    let message=status["message"].as_str().unwrap();assert!(!message.is_empty());assert!(message.contains("管理员"),"failure must identify administrator authorization: {message}");
}

#[tokio::test]
async fn corrupted_renewal_state_blocks_old_evidence_and_automatically_repairs() {
    for shared in [false,true] {
        let f=Fixture::new(ModelKind::Embeddings,2).await;f.publish(shared).await;
        {let c=f.state.conn();let keys=db::settings::all(&c).unwrap().into_iter().filter(|(k,_)|k.starts_with("internal_vector_renewal_v1:")).map(|(k,_)|k).collect::<Vec<_>>();assert_eq!(keys.len(),1);db::settings::set(&c,&keys[0],"{corrupted-renewal-state").unwrap();}
        assert!(f.require(shared).is_err(),"corrupted persisted state must not erase fail-closed protection");
        assert_eq!(f.status(shared)["state"],"failed");
        vector_renewal::tick(&f.state).await.unwrap();assert_eq!(f.count(),4,"invalid state must trigger revalidation despite fresh certificate");
        assert!(f.require(shared).is_ok());assert_eq!(f.status(shared)["state"],"valid");f.diagnostics(shared);
    }
}

#[tokio::test]
async fn missing_legacy_reference_never_bootstraps_from_only_one_healthy_source() {
    for shared in [false,true] {
        let f=Fixture::new(ModelKind::Embeddings,2).await;f.publish(shared).await;
        let before=f.require(shared).unwrap().verified_at_ms;
        {let c=f.state.conn();for(k,_)in db::settings::all(&c).unwrap(){if k.starts_with("internal_vector_reference_v1:"){db::settings::delete(&c,&k).unwrap();}}}
        f.upstreams[1].set(Behavior::Status(503));
        vector_renewal::tick(&f.state).await.unwrap();
        let c=f.state.conn();assert!(!db::settings::all(&c).unwrap().iter().any(|(k,_)|k.starts_with("internal_vector_reference_v1:")),"a single surviving source cannot establish missing historical numeric reference");
        let certificates=db::settings::all(&c).unwrap().into_iter().filter(|(k,_)|k.starts_with("internal_vector_compat_v1:")).collect::<Vec<_>>();assert_eq!(certificates.len(),1);let certificate:Value=serde_json::from_str(&certificates[0].1).unwrap();assert_eq!(certificate["verified_at_ms"],before,"incomplete reference bootstrap must not refresh evidence");
    }
}

#[tokio::test]
async fn fresh_healthy_contract_automatically_backfills_missing_reference() {
    for shared in [false,true] {
        let f=Fixture::new(ModelKind::Embeddings,2).await;f.publish(shared).await;
        {let c=f.state.conn();for(k,_)in db::settings::all(&c).unwrap(){if k.starts_with("internal_vector_reference_v1:"){db::settings::delete(&c,&k).unwrap();}}}
        vector_renewal::tick(&f.state).await.unwrap();assert_eq!(f.count(),4,"fresh legacy contract still needs complete numeric-reference bootstrap");
        let c=f.state.conn();let references=db::settings::all(&c).unwrap().into_iter().filter(|(k,_)|k.starts_with("internal_vector_reference_v1:")).collect::<Vec<_>>();assert_eq!(references.len(),1);let reference:Value=serde_json::from_str(&references[0].1).unwrap();assert!(reference["vectors"].as_array().is_some_and(|v|!v.is_empty()));assert!(reference["single"].as_array().is_some_and(|v|!v.is_empty()));drop(c);
        assert!(f.require(shared).is_ok());assert_eq!(f.status(shared)["state"],"valid");
    }
}

#[tokio::test]
async fn corrupted_reference_revokes_previous_healthy_source_records() {
    for shared in [false,true] {
        let f=Fixture::new(ModelKind::Embeddings,2).await;f.publish(shared).await;f.age(19);vector_renewal::tick(&f.state).await.unwrap();assert!(f.require(shared).is_ok());
        assert_eq!(f.status(shared)["sources"].as_array().unwrap().iter().filter(|s|s["state"]=="valid").count(),2);
        {let c=f.state.conn();for(k,_)in db::settings::all(&c).unwrap(){if k.starts_with("internal_vector_reference_v1:"){db::settings::set(&c,&k,"{corrupted-reference").unwrap();}}}
        f.age(19);f.retry_now();f.clear();vector_renewal::tick(&f.state).await.unwrap();
        let status=f.status(shared);assert_eq!(status["state"],"failed","{status}");assert!(f.require(shared).is_err(),"old valid source records must not override a global reference integrity failure");assert_eq!(f.count(),0,"corrupt reference must be rejected before paid probing");
    }
}

#[tokio::test]
async fn legacy_reference_bootstrap_preserves_existing_dimensions() {
    for shared in [false,true] {
        let f=Fixture::new(ModelKind::Embeddings,2).await;f.publish(shared).await;
        let original=f.require(shared).unwrap();assert_eq!(original.dimensions,Some(8));
        {let c=f.state.conn();for(k,_)in db::settings::all(&c).unwrap(){if k.starts_with("internal_vector_reference_v1:"){db::settings::delete(&c,&k).unwrap();}}}
        for source in &f.upstreams {source.set(Behavior::Dimension);}
        vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(f.status(shared)["state"],"failed");assert!(f.require(shared).is_err());
        let c=f.state.conn();for(k,raw)in db::settings::all(&c).unwrap(){if k.starts_with("internal_vector_compat_v1:"){let old:Value=serde_json::from_str(&raw).unwrap();assert_eq!(old["dimensions"],8);assert_eq!(old["verified_at_ms"],original.verified_at_ms);}assert!(!k.starts_with("internal_vector_reference_v1:"));}
    }
}


#[tokio::test]
async fn manual_http_save_cannot_reuse_degraded_certificate_and_keeps_healthy_routing() {
    for shared in [false,true] {
        let f=Fixture::new(ModelKind::Embeddings,2).await;f.publish(shared).await;f.age(19);
        f.upstreams[1].set(Behavior::Status(503));vector_renewal::tick(&f.state).await.unwrap();
        assert_eq!(f.status(shared)["state"],"degraded");assert!(f.require(shared).is_ok());f.retry_now();f.clear();
        let (id,version,cookie,key)={let c=f.state.conn();
            let (id,version,account)=if shared {let route=db::shared_virtual_routes::list(&c).unwrap().remove(0);(route.id,route.version,db::accounts::root(&c).unwrap().unwrap())}else{let route=vr::list(&c,f.user).unwrap().remove(0);let account=db::accounts::create(&c,"manual-save-owner","Manual Save Owner",f.user,"fixture-not-used-for-login").unwrap();(route.id,route.version,db::accounts::by_id(&c,account.id).unwrap().unwrap())};
            let cookie=format!("tokengo_account_session={}",db::accounts::create_session(&c,&account).unwrap());let key=db::identity::create_key(&c,f.user,"healthy-caller",&Default::default(),None,None).unwrap().1;(id,version,cookie,key)};
        let base=if shared{"/api/shared-virtual-models"}else{"/api/account/virtual-models"};let mut body=serde_json::to_value(&f.config).unwrap();body["version"]=json!(version);
        let request=Request::builder().method("PATCH").uri(format!("{base}/{id}")).header("host","localhost").header("content-type","application/json").header("X-Tokengo-Panel","1").header("cookie",cookie).body(Body::from(body.to_string())).unwrap();
        let response=tokengo_server::app(f.state.clone()).oneshot(request).await.unwrap();assert!(!response.status().is_success(),"saving must probe and reject unavailable target even when healthy source keeps alias usable");
        assert!(f.upstreams[1].count()>0,"manual save must probe the isolated source instead of reusing degraded evidence");
        {let c=f.state.conn();let saved=if shared{let row=db::shared_virtual_routes::list(&c).unwrap().into_iter().find(|r|r.id==id).unwrap();(row.version,row.config)}else{let row=vr::list(&c,f.user).unwrap().into_iter().find(|r|r.id==id).unwrap();(row.version,row.config)};assert_eq!(saved.0,version);assert_eq!(saved.1,f.config,"failed save cannot change stored route");}
        assert!(f.require(shared).is_ok(),"rejected config edit must preserve serving healthy source");f.clear();
        let request=Request::builder().method("POST").uri("/v1/embeddings").header("content-type","application/json").header("authorization",format!("Bearer {key}")).body(Body::from(json!({"model":f.config.name,"input":"healthy source remains callable"}).to_string())).unwrap();
        let response=tokengo_server::app(f.state.clone()).oneshot(request).await.unwrap();assert_eq!(response.status(),StatusCode::OK);assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),0,"isolated source must stay excluded from user calls");
    }
}
