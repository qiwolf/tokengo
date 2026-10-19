//! Real HTTP regressions for per-route automatic inference opt-out.
use axum::{extract::State, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokengo_server::{
    db::{
        channels, kinds,
        route_policy::{self, RoutePolicy},
    },
    type_detector, vision_detector, AppState, Config, SharedState,
};
use tokengo_shared::ModelKind;

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    DisableType,
    DisableVision,
}
#[derive(Clone)]
struct Mock {
    state: SharedState,
    calls: Arc<AtomicUsize>,
    mode: Mode,
}
struct Fixture {
    state: SharedState,
    calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn disable(state: &SharedState) {
    route_policy::save(
        &state.conn(),
        "test-model",
        &RoutePolicy {
            probe_enabled: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
}
async fn answer(State(mock): State<Mock>, Json(_body): Json<Value>) -> Json<Value> {
    mock.calls.fetch_add(1, Ordering::SeqCst);
    match mock.mode {
        Mode::Normal => {
            Json(json!({"choices":[{"message":{"content":"红色"},"finish_reason":"stop"}]}))
        }
        Mode::DisableType => {
            disable(&mock.state);
            Json(json!({"choices":[]}))
        }
        Mode::DisableVision => {
            disable(&mock.state);
            Json(json!({"choices":[{"message":{"content":""},"finish_reason":"length"}]}))
        }
    }
}
impl Fixture {
    async fn new(mode: Mode) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tokengo-background-policy-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(answer))
            .route("/v1/embeddings", post(answer))
            .route("/v1/rerank", post(answer))
            .with_state(Mock {
                state: state.clone(),
                calls: calls.clone(),
                mode,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        {
            let conn = state.conn();
            let id = channels::create(
                &conn,
                &channels::NewChannel {
                    name: "mock".into(),
                    base_url: url,
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap();
            channels::set_models(
                &conn,
                id,
                &[channels::ModelMapping::new("test-model", None, false)],
            )
            .unwrap();
        }
        Self {
            state,
            calls,
            task,
            dir,
        }
    }
    fn count(&self, table: &str) -> i64 {
        self.state
            .conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn chat(&self) {
        kinds::set(&self.state.conn(), "test-model", ModelKind::Chat).unwrap();
    }
}

#[tokio::test]
async fn disabled_route_creates_no_type_or_vision_jobs_and_sends_no_inference() {
    let f = Fixture::new(Mode::Normal).await;
    disable(&f.state);
    type_detector::tick(&f.state).await.unwrap();
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.count("model_type_checks"), 0);
    f.chat();
    vision_detector::tick(&f.state).await.unwrap();
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.count("model_vision_checks"), 0);
}

#[tokio::test]
async fn inherited_route_still_discovers_type_and_checks_vision_over_http() {
    let f = Fixture::new(Mode::Normal).await;
    type_detector::tick(&f.state).await.unwrap();
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.count("model_type_checks"), 1);
    vision_detector::tick(&f.state).await.unwrap();
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.count("model_vision_checks"), 1);
}

#[tokio::test]
async fn disabling_during_type_detection_prevents_later_endpoint_requests() {
    let f = Fixture::new(Mode::DisableType).await;
    type_detector::tick(&f.state).await.unwrap();
    assert_eq!(
        f.calls.load(Ordering::SeqCst),
        1,
        "disabled policy must be rechecked before the next endpoint"
    );
}

#[tokio::test]
async fn disabling_during_vision_detection_prevents_truncation_retry() {
    let f = Fixture::new(Mode::DisableVision).await;
    f.chat();
    vision_detector::tick(&f.state).await.unwrap();
    assert_eq!(
        f.calls.load(Ordering::SeqCst),
        1,
        "disabled policy must be rechecked before a second vision request"
    );
}
