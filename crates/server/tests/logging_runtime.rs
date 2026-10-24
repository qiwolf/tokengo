//! Runtime log expiry must respect newer authenticated settings.
//! Paused Tokio time advances real timer tasks without waiting minutes.
use std::{path::PathBuf, sync::Arc, time::Duration};
use axum::{body::Body, http::{Request, StatusCode}};
use serde_json::{json, Value};
use tokengo_server::{app, state::AppState, Config};
use tower::ServiceExt;

struct Fixture { dir: PathBuf, state: Arc<AppState>, token: String }
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
}

async fn request(state: Arc<AppState>, method: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let path = if token.is_some() { "/api/logs/level" } else { "/api/auth/setup" };
    let mut builder = Request::builder().method(method).uri(path).header("content-type", "application/json");
    if let Some(token) = token { builder = builder.header("authorization", format!("Bearer {token}")); }
    let response = app(state).oneshot(builder.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!("tokengo-log-runtime-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = AppState::init(Config {
        listen: "127.0.0.1:0".into(), db_path: dir.join("test.db"),
        master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
        web_dir: None, log_dir: None,
    }).unwrap();
    let (status, response) = request(state.clone(), "POST", None, json!({"password":"log-expiry-test-password"})).await;
    assert_eq!(status, StatusCode::OK, "setup: {response}");
    Fixture { dir, state, token: response["token"].as_str().unwrap().to_owned() }
}

async fn set(f: &Fixture, level: &str, minutes: Option<u32>) {
    let (status, response) = request(f.state.clone(), "PUT", Some(&f.token), json!({"level":level,"minutes":minutes})).await;
    assert_eq!(status, StatusCode::OK, "setting: {response}");
    let expected = tokengo_server::logging::normalize_filter(level).unwrap();
    assert_eq!(response["level"].as_str(), Some(expected.as_str()), "HTTP response must report the requested filter");
    assert_eq!(f.state.log_control().level(), expected, "requested filter must be applied");
    // Let the newly spawned expiry task register its sleep before advancing.
    drain_scheduler().await;
}

async fn drain_scheduler() {
    // advance() wakes timers but does not wait for their tasks to run. A single
    // yield may select the keep-alive task instead of the expired timer, making
    // a stale timer appear harmless. Give both ready tasks repeated turns.
    for _ in 0..32 { tokio::task::yield_now().await; }
}

async fn advance(seconds: u64) {
    tokio::time::advance(Duration::from_secs(seconds)).await;
    drain_scheduler().await;
}

#[tokio::test]
async fn obsolete_log_timer_cannot_override_a_newer_manual_setting() {
    let f = fixture().await;
    tokio::time::pause();
    // HTTP handlers may await blocking authentication work. Keep the runtime
    // runnable so paused time advances only through our explicit advance calls.
    let keep_clock = tokio::spawn(async { loop { tokio::task::yield_now().await; } });
    set(&f, "debug", Some(1)).await;
    advance(30).await;
    set(&f, "warn", None).await;
    advance(31).await;
    assert_eq!(f.state.log_control().level(), tokengo_server::logging::normalize_filter("warn").unwrap(),
        "an obsolete debug timer overwrote a newer manual level");
    assert_eq!(f.state.log_control().revert_at_ms(), None);
    keep_clock.abort();
}

#[tokio::test]
async fn obsolete_log_timer_cannot_shorten_a_newer_debug_window() {
    let f = fixture().await;
    tokio::time::pause();
    // HTTP handlers may await blocking authentication work. Keep the runtime
    // runnable so paused time advances only through our explicit advance calls.
    let keep_clock = tokio::spawn(async { loop { tokio::task::yield_now().await; } });
    set(&f, "debug", Some(1)).await;
    advance(30).await;
    set(&f, "debug", Some(2)).await;
    let newer_deadline = f.state.log_control().revert_at_ms();
    advance(31).await;
    assert_eq!(f.state.log_control().level(), tokengo_server::logging::normalize_filter("debug").unwrap(),
        "old timer shortened the newer debug window");
    assert_eq!(f.state.log_control().revert_at_ms(), newer_deadline);
    advance(90).await;
    assert_eq!(f.state.log_control().level(), tokengo_server::logging::DEFAULT_FILTER);
    assert_eq!(f.state.log_control().revert_at_ms(), None);
    keep_clock.abort();
}
