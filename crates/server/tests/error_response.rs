//! Real HTTP regressions for bounded upstream error responses.
use axum::{
    body::{Body, Bytes},
    http::StatusCode,
    response::Response,
    routing::post,
    Router,
};
use futures_util::stream;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokengo_server::{
    gateway::{self, EndpointKind, ForwardRequest},
    AppState, Config,
};

#[derive(Clone, Copy)]
enum Mode {
    Hang,
    Drip,
    Huge,
    Finite,
    Broken,
    Healthy,
}
struct Fixture {
    state: Arc<AppState>,
    dir: std::path::PathBuf,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    primary: i64,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
async fn upstream(mode: Mode, status: u16) -> (String, tokio::task::JoinHandle<()>) {
    let router = Router::new().route("/v1/chat/completions", post(move || async move {
        let body = match mode {
            Mode::Hang => Body::from_stream(stream::pending::<Result<Bytes, std::io::Error>>()),
            Mode::Drip => Body::from_stream(stream::unfold((), |_| async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Some((Ok::<_, std::io::Error>(Bytes::from_static(b"x")), ()))
            })),
            Mode::Huge => Body::from(vec![b'x'; 65 * 1024]),
            Mode::Finite => Body::from(r#"{"error":{"message":"limited"}}"#),
            Mode::Broken => Body::from_stream(stream::iter(vec![Err::<Bytes, _>(std::io::Error::other("broken body"))])),
            Mode::Healthy => Body::from("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"fallback\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"),
        };
        Response::builder().status(status).header("content-type", if matches!(mode, Mode::Healthy) {"text/event-stream"} else {"application/json"})
            .header("retry-after", "7").body(body).unwrap()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (base, task)
}
impl Fixture {
    async fn new(mode: Mode, status: u16, fallback: bool, attempt: u64, budget: u64) -> Self {
        let dir =
            std::env::temp_dir().join(format!("tokengo-error-response-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        {
            let mut routing = state.routing();
            routing.cfg.retry_times = 0;
            routing.cfg.attempt_timeout_ms = attempt;
            routing.cfg.nonstream_timeout_ms = attempt;
            routing.cfg.request_budget_ms = budget;
        }
        let mut tasks = Vec::new();
        let mut primary = 0;
        for (i, mode) in [mode, Mode::Healthy]
            .into_iter()
            .enumerate()
            .take(if fallback { 2 } else { 1 })
        {
            let (base, task) = upstream(mode, if i == 0 { status } else { 200 }).await;
            tasks.push(task);
            let conn = state.conn();
            let id = tokengo_server::db::channels::create(
                &conn,
                &tokengo_server::db::channels::NewChannel {
                    name: format!("channel-{i}"),
                    base_url: base,
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 1,
                },
            )
            .unwrap();
            tokengo_server::db::channels::set_models(
                &conn,
                id,
                &[tokengo_server::db::channels::ModelMapping::new(
                    "test-model",
                    None,
                    false,
                )],
            )
            .unwrap();
            if i == 0 {
                primary = id.0;
            }
        }
        Self {
            state,
            dir,
            tasks,
            primary,
        }
    }
    async fn request(&self) -> (StatusCode, Bytes, Option<String>) {
        let response = gateway::forward(
            self.state.clone(),
            ForwardRequest {
                explicit_session: None,
                session_namespace: "test".into(),
                endpoint: EndpointKind::Chat,
                model: "test-model".into(),
                body: json!({"model":"test-model","messages":[],"stream":true}),
                stream: true,
                client_ip: None,
                user_agent: None,
            },
        )
        .await
        .unwrap();
        let status = response.status();
        let retry = response
            .headers()
            .get("retry-after")
            .map(|v| v.to_str().unwrap().to_owned());
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, body, retry)
    }
    fn assert_ledger(&self, success: bool, error_note: &str) {
        self.state.drain_writes();
        let conn = self.state.conn();
        let (status, error): (String, Option<String>) = conn
            .query_row(
                "SELECT status,error FROM requests ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, if success { "ok" } else { "failed" });
        if !success {
            assert!(error.unwrap().contains(error_note));
        }
        let (outcome, note): (String, Option<String>) = conn
            .query_row(
                "SELECT outcome,error FROM attempts ORDER BY id LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(outcome, "channel_fault");
        assert!(note.unwrap().contains(error_note));
        if success {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM attempts WHERE outcome='success'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1);
        }
        drop(conn);
        assert!(
            self.state
                .channel_gate(self.primary, 1)
                .unwrap()
                .try_acquire_owned()
                .is_ok(),
            "primary capacity leaked"
        );
    }
}

#[tokio::test]
async fn hanging_400_429_and_500_bodies_fail_over_and_release_capacity() {
    for status in [400, 429, 500] {
        let f = Fixture::new(Mode::Hang, status, true, 80, 600).await;
        let (s, b, _) = tokio::time::timeout(Duration::from_millis(500), f.request())
            .await
            .expect("error body must have an idle deadline");
        assert_eq!(s, StatusCode::OK);
        assert!(String::from_utf8_lossy(&b).contains("fallback"));
        f.assert_ledger(true, "error body");
    }
}
#[tokio::test]
async fn continuously_dripping_error_has_total_attempt_deadline() {
    let f = Fixture::new(Mode::Drip, 500, true, 90, 600).await;
    let (s, _, _) = tokio::time::timeout(Duration::from_millis(500), f.request())
        .await
        .expect("small chunks cannot extend an attempt forever");
    assert_eq!(s, StatusCode::OK);
    f.assert_ledger(true, "error body");
}
#[tokio::test]
async fn oversized_error_is_rejected_before_fallback() {
    let f = Fixture::new(Mode::Huge, 500, true, 100, 600).await;
    assert_eq!(f.request().await.0, StatusCode::OK);
    f.assert_ledger(true, "65536");
}
#[tokio::test]
async fn error_body_remaining_request_budget_terminates_without_fallback() {
    let f = Fixture::new(Mode::Drip, 429, false, 1000, 90).await;
    let (s, _, _) = tokio::time::timeout(Duration::from_millis(450), f.request())
        .await
        .expect("remaining request budget must bound error reading");
    assert!(s.is_server_error());
    f.assert_ledger(false, "error body");
}
#[tokio::test]
async fn finite_errors_preserve_status_body_and_retry_after() {
    for status in [400, 429, 500] {
        let f = Fixture::new(Mode::Finite, status, false, 100, 600).await;
        let (s, b, retry) = f.request().await;
        assert_eq!(s.as_u16(), status);
        assert_eq!(&b[..], br#"{"error":{"message":"limited"}}"#);
        assert_eq!(retry.as_deref(), Some("7"));
        assert!(f
            .state
            .channel_gate(f.primary, 1)
            .unwrap()
            .try_acquire_owned()
            .is_ok());
    }
}
#[tokio::test]
async fn broken_error_transport_can_fall_back() {
    let f = Fixture::new(Mode::Broken, 500, true, 100, 600).await;
    assert_eq!(f.request().await.0, StatusCode::OK);
    // Hyper may reject before delivering headers; either network or body fault is valid.
    f.state.drain_writes();
    assert!(f
        .state
        .channel_gate(f.primary, 1)
        .unwrap()
        .try_acquire_owned()
        .is_ok());
}
