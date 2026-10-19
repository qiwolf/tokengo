//! Per-logical-model policies exercise the real admin API and HTTP forwarding.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokengo_server::{app, AppState, Config};
use tower::ServiceExt;

struct Fixture {
    state: Arc<AppState>,
    token: String,
    dir: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("tokengo-route-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into(),
            ..Config::default()
        })
        .unwrap();
        tokengo_server::db::sessions::set_gateway_key(&state.conn(), &state.secrets, "test-key")
            .unwrap();
        let token = tokengo_server::db::sessions::create_session(&state.conn(), None).unwrap();
        Self { state, token, dir }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(t) = token {
            request = request.header("authorization", format!("Bearer {t}"));
        }
        let response = app(self.state.clone())
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }
    async fn policy(&self, model: &str, value: Value) -> (StatusCode, Value) {
        self.request(
            "PUT",
            &format!("/api/model-routing?model={model}"),
            Some(&self.token),
            value,
        )
        .await
    }
    async fn call(&self, model: &str) -> (StatusCode, Value) {
        self.request(
            "POST",
            "/v1/chat/completions",
            Some("test-key"),
            json!({"model":model,"messages":[{"role":"user","content":"hello"}]}),
        )
        .await
    }
    async fn upstream(
        &self,
        name: &str,
        status: StatusCode,
    ) -> (i64, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let calls = counter.clone();
        let label = name.to_string();
        let service = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let calls = calls.clone();
                let label = label.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        status,
                        Json(if status.is_success() {
                            json!({"choices":[{"message":{"content":label}}]})
                        } else {
                            json!({"error":{"message":"temporary outage"}})
                        }),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
        let c = self.state.conn();
        let id = tokengo_server::db::channels::create(
            &c,
            &tokengo_server::db::channels::NewChannel {
                name: name.into(),
                base_url: base,
                enabled: true,
                max_concurrent: 0,
                unwrap_envelope: false,
            },
        )
        .unwrap();
        tokengo_server::db::channels::set_models(
            &c,
            id,
            &[
                tokengo_server::db::channels::ModelMapping::new("chat", None, false),
                tokengo_server::db::channels::ModelMapping::new("other", None, false),
            ],
        )
        .unwrap();
        (id.0, counter, handle)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn auto_respects_the_selected_logical_models_zero_retry_policy() {
    let f = Fixture::new();
    let (_, calls, server) = f.upstream("a", StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(
        f.policy("chat", json!({"retry_times":0})).await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.policy("other", json!({"include_in_auto":false})).await.0,
        StatusCode::OK
    );
    tokengo_server::db::kinds::set(&f.state.conn(), "chat", tokengo_shared::ModelKind::Chat)
        .unwrap();
    assert_ne!(f.call("auto").await.0, StatusCode::OK);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "auto must not bypass a selected model's zero-retry limit"
    );
    server.abort();
}

#[tokio::test]
async fn policy_orders_targets_overrides_retries_and_survives_restart() {
    let f = Fixture::new();
    let (a, ac, ah) = f.upstream("a", StatusCode::INTERNAL_SERVER_ERROR).await;
    let (b, bc, bh) = f.upstream("b", StatusCode::OK).await;
    assert_eq!(f.policy("chat",json!({"channel_ids":[b,a],"retry_times":0,"probe_enabled":false,"reprobe_interval_ms":60000})).await.0,StatusCode::OK);
    assert_eq!(
        f.call("chat").await.1["choices"][0]["message"]["content"],
        "b"
    );
    assert_eq!(ac.load(Ordering::SeqCst), 0);
    assert_eq!(f.policy("chat",json!({"channel_ids":[a,b],"retry_times":0,"reprobe_interval_ms":60000,"probe_enabled":false})).await.0,StatusCode::OK);
    assert_eq!(f.call("chat").await.0, StatusCode::OK);
    assert_eq!(ac.load(Ordering::SeqCst), 1);
    assert_eq!(bc.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.state.routing().cfg.retry_times,
        3,
        "per-model override must not mutate the global config"
    );
    let due = f
        .state
        .routing()
        .state("chat")
        .unwrap()
        .next_probe_at_ms
        .unwrap();
    let remaining = due - tokengo_server::db::now_ms();
    assert!(
        (55000..=60000).contains(&remaining),
        "route recovery interval was not applied: {remaining}"
    );
    let restarted = AppState::init(f.state.cfg.clone()).unwrap();
    let token = tokengo_server::db::sessions::create_session(&restarted.conn(), None).unwrap();
    let request = Request::builder()
        .uri("/api/model-routing?model=chat")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app(restarted).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["policy"]["channel_ids"], json!([a, b]));
    assert_eq!(value["policy"]["retry_times"], 0);
    ah.abort();
    bh.abort();
}

#[tokio::test]
async fn per_model_probe_override_controls_scheduled_recovery() {
    let f = Fixture::new();
    let (a, calls, server) = f.upstream("a", StatusCode::OK).await;
    assert_eq!(
        f.policy("chat", json!({"probe_enabled":false})).await.0,
        StatusCode::OK
    );
    let now = tokengo_server::db::now_ms();
    let due = tokengo_shared::routing::RouteState {
        active: Some(tokengo_shared::ChannelId(a)),
        next_probe_at_ms: Some(now - 1),
        ..Default::default()
    };
    f.state.routing().restore("chat", due.clone());
    assert!(tokengo_server::prober::tick(&f.state, now).await.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.policy("chat", json!({"probe_enabled":true})).await.0,
        StatusCode::OK
    );
    f.state.routing().cfg.probe_enabled = false;
    f.state.routing().restore("chat", due);
    assert_eq!(tokengo_server::prober::tick(&f.state, now).await.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn explicit_target_list_is_an_allowlist_and_delete_restores_defaults() {
    let f = Fixture::new();
    let (a, _, ah) = f.upstream("a", StatusCode::INTERNAL_SERVER_ERROR).await;
    let (_, bc, bh) = f.upstream("b", StatusCode::OK).await;
    assert_eq!(
        f.policy("chat", json!({"channel_ids":[a],"retry_times":0}))
            .await
            .0,
        StatusCode::OK
    );
    assert_ne!(f.call("chat").await.0, StatusCode::OK);
    assert_eq!(
        bc.load(Ordering::SeqCst),
        0,
        "unlisted sources must never be attempted"
    );
    assert_eq!(
        f.request(
            "DELETE",
            "/api/model-routing?model=chat",
            Some(&f.token),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.call("chat").await.0, StatusCode::OK);
    assert_eq!(bc.load(Ordering::SeqCst), 1);
    ah.abort();
    bh.abort();
}

#[tokio::test]
async fn policy_requires_admin_validates_atomically_and_excludes_auto() {
    let f = Fixture::new();
    let (a, _, ah) = f.upstream("a", StatusCode::OK).await;
    for method in ["GET", "PUT", "DELETE"] {
        assert_eq!(
            f.request(method, "/api/model-routing?model=chat", None, json!({}))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        f.policy("chat", json!({"include_in_auto":false,"channel_ids":[a]}))
            .await
            .0,
        StatusCode::OK
    );
    for bad in [
        json!({"channel_ids":[a,a]}),
        json!({"channel_ids":[99999]}),
        json!({"channel_ids":[]}),
        json!({"retry_times":3}),
        json!({"reprobe_interval_ms":-1}),
        json!({"typo":true}),
    ] {
        assert_eq!(f.policy("chat", bad).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        f.policy("missing", json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(f.policy("auto", json!({})).await.0, StatusCode::BAD_REQUEST);
    let saved = f
        .request(
            "GET",
            "/api/model-routing?model=chat",
            Some(&f.token),
            json!({}),
        )
        .await
        .1;
    assert_eq!(saved["policy"]["include_in_auto"], false);
    assert_eq!(saved["policy"]["channel_ids"], json!([a]));
    tokengo_server::db::kinds::set(&f.state.conn(), "chat", tokengo_shared::ModelKind::Chat)
        .unwrap();
    tokengo_server::db::kinds::set(&f.state.conn(), "other", tokengo_shared::ModelKind::Chat)
        .unwrap();
    let names = tokengo_server::db::model_routes::auto_candidates(
        &f.state.conn(),
        tokengo_shared::ModelKind::Chat,
        false,
        tokengo_server::db::now_ms(),
    )
    .unwrap();
    assert!(!names.iter().any(|r| r == "chat"));
    assert_eq!(
        f.call("chat").await.0,
        StatusCode::OK,
        "exclusion from auto does not disable explicit access"
    );
    ah.abort();
}
