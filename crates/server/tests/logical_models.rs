//! Real HTTP tests for administrator-declared logical -> upstream model mappings.
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU16, Ordering},
    Arc, Mutex,
};
use tokengo_server::{app, AppState, Config, SharedState};

const KEY: &str = "logical-model-http-test-key";
const LOGICAL: &str = "deepseek-v4.1-flash";

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server { url, task }
}
struct Gateway {
    cfg: Config,
    state: SharedState,
    server: Server,
    token: String,
}
impl Gateway {
    async fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("tokengo-logical-http-{}", uuid::Uuid::new_v4()));
        let cfg = Config {
            db_path: dir.join("gateway.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        };
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(cfg.clone()).unwrap();
        let mut config = state.routing().cfg.clone();
        config.retry_times = 0;
        config.probe_enabled = false;
        tokengo_server::db::routing::save_config(&state.conn(), &config).unwrap();
        state.routing().cfg = config;
        tokengo_server::db::sessions::set_gateway_key(&state.conn(), &state.secrets, KEY).unwrap();
        let token = tokengo_server::db::sessions::create_session(&state.conn(), None).unwrap();
        let server = serve(app(state.clone())).await;
        Self {
            cfg,
            state,
            server,
            token,
        }
    }
    async fn admin(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let response = reqwest::Client::new()
            .request(
                method.parse().unwrap(),
                format!("{}{path}", self.server.url),
            )
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap())
    }
    async fn chat(&self, model: &str, image: bool) -> (StatusCode, Value) {
        let content = if image {
            json!([{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}])
        } else {
            json!("hello")
        };
        let response = reqwest::Client::new()
            .post(format!("{}/v1/chat/completions", self.server.url))
            .bearer_auth(KEY)
            .json(&json!({"model":model,"messages":[{"role":"user","content":content}]}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap())
    }
    async fn channel(&self, name: &str, base: &str, mappings: Value) -> (StatusCode, Value) {
        self.admin(
            "POST",
            "/api/channels",
            json!({"name":name,"base_url":base,"models":mappings}),
        )
        .await
    }
}
#[derive(Default)]
struct Seen {
    status: AtomicU16,
    models: Mutex<Vec<String>>,
    error_code: Mutex<Option<String>>,
}
impl Seen {
    fn count(&self) -> usize {
        self.models.lock().unwrap().len()
    }
    fn status(&self, status: u16) {
        self.status.store(status, Ordering::SeqCst);
    }
}
async fn upstream(State(seen): State<Arc<Seen>>, Json(body): Json<Value>) -> Response {
    let model = body["model"].as_str().unwrap().to_string();
    seen.models.lock().unwrap().push(model.clone());
    let status = seen.status.load(Ordering::SeqCst);
    if status != 0 && status != 200 {
        let mut error = json!({"message":"synthetic upstream failure"});
        if let Some(code) = seen.error_code.lock().unwrap().as_ref() {
            error["code"] = json!(code);
        }
        return (
            StatusCode::from_u16(status).unwrap(),
            Json(json!({"error":error})),
        )
            .into_response();
    }
    Json(json!({"id":"logical-test","object":"chat.completion","model":model,"choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})).into_response()
}
async fn source() -> (Server, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    (
        serve(
            Router::new()
                .route("/v1/chat/completions", post(upstream))
                .with_state(seen.clone()),
        )
        .await,
        seen,
    )
}

#[tokio::test]
async fn logical_mapping_api_accepts_aliases_and_trims_without_normalizing_upstream() {
    let g = Gateway::new().await;
    let (status, body) = g.channel("aliases", "http://127.0.0.1:9/v1", json!([
        {"model":" logical-main ","upstream":" Provider/DeepSeek_V4.1:Flash+ ","vision":true},
        {"model":"diag/a/model","upstream":"   "}
    ])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mappings = body["models"].as_array().unwrap();
    let main = mappings
        .iter()
        .find(|m| m["model"] == "logical-main")
        .unwrap();
    assert_eq!(main["upstream"], "Provider/DeepSeek_V4.1:Flash+");
    let diag = mappings
        .iter()
        .find(|m| m["model"] == "diag/a/model")
        .unwrap();
    assert!(diag["upstream"].is_null());
}

#[tokio::test]
async fn invalid_logical_mappings_are_rejected_without_partial_create_or_update() {
    let g = Gateway::new().await;
    let (status, row) = g
        .channel(
            "unchanged",
            "http://127.0.0.1:9/v1",
            json!([{"model":"valid","upstream":"real"}]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{row}");
    let id = row["id"].as_i64().unwrap();
    let invalid = vec![
        json!([{"model":"   ","upstream":"real"}]),
        json!([{"model":"---","upstream":"real"}]),
        json!([{"model":"(provider)","upstream":"real"}]),
        json!([{"model":" AUTO (provider) Free ","upstream":"real"}]),
        json!([{"model":"a\nb","upstream":"real"}]),
        json!([{"model":"valid","upstream":"a\nb"}]),
        json!([{"model":"same-model","upstream":"one"},{"model":"Same_Model (provider)","upstream":"two"}]),
    ];
    for mappings in invalid {
        let before = tokengo_server::db::channels::list(&g.state.conn()).unwrap();
        let (status, body) = g
            .channel("bad-create", "http://127.0.0.1:8/v1", mappings.clone())
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, body) = g
            .admin(
                "PUT",
                &format!("/api/channels/{id}"),
                json!({"name":"bad-update","base_url":"http://127.0.0.1:8/v1","models":mappings}),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            tokengo_server::db::channels::list(&g.state.conn()).unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn three_source_alias_failover_and_single_source_diagnostics_survive_restart() {
    let mut g = Gateway::new().await;
    let (a, sa) = source().await;
    let (b, sb) = source().await;
    let (c, sc) = source().await;
    let upstream_names = [
        "Provider-A/DeepSeek_V4.1:Flash+",
        "deepseek-flash-b",
        "VendorC/Flash",
    ];
    let diagnostics = [
        "diag/provider-a/deepseek-v4.1-flash",
        "diag/provider-b/deepseek-v4.1-flash",
        "diag/provider-c/deepseek-v4.1-flash",
    ];
    for (i, server) in [&a, &b, &c].iter().enumerate() {
        assert_ne!(
            tokengo_shared::normalize_model_name(diagnostics[i]),
            tokengo_shared::normalize_model_name(LOGICAL)
        );
        for other in diagnostics.iter().take(i) {
            assert!(!tokengo_shared::same_model_name(diagnostics[i], other));
        }
        let (status, row) = g
            .channel(
                &format!("provider-{i}"),
                &format!("{}/v1", server.url),
                json!([
                    {"model":LOGICAL,"upstream":upstream_names[i]},
                    {"model":diagnostics[i],"upstream":upstream_names[i]}
                ]),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{row}");
    }
    sa.status(503);
    sb.status(503);
    assert_eq!(g.chat(LOGICAL, false).await.0, StatusCode::OK);
    assert_eq!((sa.count(), sb.count(), sc.count()), (1, 1, 1));
    sa.status(200);
    sb.status(200);
    for (i, diag) in diagnostics.iter().enumerate() {
        let before = [sa.count(), sb.count(), sc.count()];
        assert_eq!(g.chat(diag, false).await.0, StatusCode::OK);
        let after = [sa.count(), sb.count(), sc.count()];
        for j in 0..3 {
            assert_eq!(after[j] - before[j], usize::from(i == j));
        }
    }
    sa.status(503);
    let before = [sa.count(), sb.count(), sc.count()];
    assert!(!g.chat(diagnostics[0], false).await.0.is_success());
    assert_eq!(
        [
            sa.count() - before[0],
            sb.count() - before[1],
            sc.count() - before[2]
        ],
        [1, 0, 0]
    );
    sa.status(400);
    // Start a fresh request route at A after the earlier successful fallback selected C.
    let cfg = g.state.routing().cfg.clone();
    *g.state.routing() = tokengo_shared::routing::Router::new(cfg);
    let before = [sa.count(), sb.count(), sc.count()];
    assert_eq!(g.chat(LOGICAL, false).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        [
            sa.count() - before[0],
            sb.count() - before[1],
            sc.count() - before[2]
        ],
        [1, 0, 0]
    );
    for (seen, expected) in [&sa, &sb, &sc].iter().zip(upstream_names) {
        assert!(seen
            .models
            .lock()
            .unwrap()
            .iter()
            .all(|model| model == expected));
    }
    g.server.task.abort();
    g.state = AppState::init(g.cfg.clone()).unwrap();
    g.server = serve(app(g.state.clone())).await;
    sa.status(200);
    let before = [sa.count(), sb.count(), sc.count()];
    assert_eq!(g.chat(diagnostics[0], false).await.0, StatusCode::OK);
    assert_eq!(
        [
            sa.count() - before[0],
            sb.count() - before[1],
            sc.count() - before[2]
        ],
        [1, 0, 0]
    );
    assert_eq!(
        tokengo_server::db::channels::list(&g.state.conn())
            .unwrap()
            .len(),
        3
    );
    println!("three real HTTP upstreams: independent names, fallback A->B->C, single-source diagnostics, 400 stops, restart persistence");
}

#[tokio::test]
async fn logical_aliases_keep_disabled_channel_model_and_vision_boundaries() {
    let g = Gateway::new().await;
    let (a, seen) = source().await;
    let (status, row) = g
        .channel(
            "boundary",
            &format!("{}/v1", a.url),
            json!([{"model":LOGICAL,"upstream":"Actual/Flash"}]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{row}");
    let id = row["id"].as_i64().unwrap();
    assert_eq!(
        g.chat(LOGICAL, true).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(seen.count(), 0);
    assert_eq!(
        g.admin(
            "PUT",
            "/api/model-enabled",
            json!({"model":LOGICAL,"enabled":false})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(g.chat(LOGICAL, false).await.0, StatusCode::FORBIDDEN);
    assert_eq!(seen.count(), 0);
    g.admin(
        "PUT",
        "/api/model-enabled",
        json!({"model":LOGICAL,"enabled":true}),
    )
    .await;
    assert_eq!(
        g.admin(
            "PUT",
            &format!("/api/channels/{id}/enabled"),
            json!({"enabled":false})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(!g.chat(LOGICAL, false).await.0.is_success());
    assert_eq!(seen.count(), 0);
    let (status, body) = g.admin("PUT", &format!("/api/channels/{id}"), json!({"name":"boundary","base_url":format!("{}/v1",a.url),"enabled":true,"models":[{"model":LOGICAL,"upstream":"Actual/Flash","vision":true}]})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(g.chat(LOGICAL, true).await.0, StatusCode::OK);
    assert_eq!(seen.models.lock().unwrap().as_slice(), ["Actual/Flash"]);
    assert_eq!(
        g.chat("unknown-logical", false).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(seen.count(), 1);
}

#[tokio::test]
async fn configured_logical_route_slice_uses_per_model_zero_retry_and_diagnostic_isolation() {
    let g = Gateway::new().await;
    // Keep the production-like global retry count; only API policies may suppress it.
    let mut global = g.state.routing().cfg.clone();
    global.retry_times = 3;
    tokengo_server::db::routing::save_config(&g.state.conn(), &global).unwrap();
    g.state.routing().cfg = global;
    let (a, sa) = source().await;
    let (b, sb) = source().await;
    let (c, sc) = source().await;
    let upstream_names = [
        "Provider-A/DeepSeek_V4.1:Flash+",
        "deepseek-flash-b",
        "VendorC/Flash",
    ];
    let diagnostics = [
        "diag/provider-a/deepseek-v4.1-flash",
        "diag/provider-b/deepseek-v4.1-flash",
        "diag/provider-c/deepseek-v4.1-flash",
    ];
    let servers = [&a, &b, &c];
    let mut ids = [0i64; 3];
    // Create C before B before A, so successful A->B->C proves route-local ordering.
    for i in [2, 1, 0] {
        let (status, row) = g
            .channel(
                &format!("slice-provider-{i}"),
                &format!("{}/v1", servers[i].url),
                json!([
                    {"model":LOGICAL,"upstream":upstream_names[i]},
                    {"model":diagnostics[i],"upstream":upstream_names[i]}
                ]),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{row}");
        ids[i] = row["id"].as_i64().unwrap();
    }
    let (status, body) = g
        .admin(
            "PUT",
            &format!("/api/model-routing?model={LOGICAL}"),
            json!({
                "channel_ids":ids,"retry_times":0,"probe_enabled":false,"reprobe_interval_ms":60000
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (i, diagnostic) in diagnostics.iter().enumerate() {
        let (status, body) = g.admin("PUT", &format!("/api/model-routing?model={diagnostic}"), json!({
            "channel_ids":[ids[i]],"retry_times":0,"probe_enabled":false,"include_in_auto":false
        })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        tokengo_server::db::kinds::set(
            &g.state.conn(),
            diagnostic,
            tokengo_shared::ModelKind::Chat,
        )
        .unwrap();
    }
    tokengo_server::db::kinds::set(&g.state.conn(), LOGICAL, tokengo_shared::ModelKind::Chat)
        .unwrap();
    sa.status(429);
    sb.status(400);
    *sb.error_code.lock().unwrap() = Some("insufficient_balance".into());
    let (status, body) = g.chat(LOGICAL, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (sa.count(), sb.count(), sc.count()),
        (1, 1, 1),
        "each configured source must receive exactly one attempt"
    );
    let before = [sa.count(), sb.count(), sc.count()];
    let (status, _) = g.chat(diagnostics[0], false).await;
    assert!(!status.is_success());
    assert_eq!(
        [
            sa.count() - before[0],
            sb.count() - before[1],
            sc.count() - before[2]
        ],
        [1, 0, 0]
    );
    for (seen, expected) in [&sa, &sb, &sc].iter().zip(upstream_names) {
        assert!(seen
            .models
            .lock()
            .unwrap()
            .iter()
            .all(|model| model == expected));
    }
    let candidates = tokengo_server::db::model_routes::auto_candidates(
        &g.state.conn(),
        tokengo_shared::ModelKind::Chat,
        false,
        tokengo_server::db::now_ms(),
    )
    .unwrap();
    assert!(candidates
        .iter()
        .any(|name| tokengo_shared::same_model_name(name, LOGICAL)));
    assert!(candidates.iter().all(|name| diagnostics
        .iter()
        .all(|diag| !tokengo_shared::same_model_name(name, diag))));
    assert_eq!(g.state.routing().cfg.retry_times, 3);
    assert_eq!(
        tokengo_server::db::routing::load_config(&g.state.conn())
            .unwrap()
            .retry_times,
        3
    );
    println!("route slice: global retry=3 unchanged; policy A429->B400 insufficient_balance->C200 each once; diagnostic A one failure/no cross-source; diagnostics excluded from auto");
}
