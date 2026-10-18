use axum::{routing::get, Json, Router};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokengo_server::{
    capability_detector,
    db::{
        self,
        channels::{self, ModelMapping, NewChannel},
        model_routes,
    },
    AppState, Config, SharedState,
};
use tokengo_shared::ChannelId;
use tower::ServiceExt;

fn fixture() -> (SharedState, std::path::PathBuf) {
    let dir = std::path::PathBuf::from("/tmp")
        .join(format!("capability-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = AppState::init(Config {
        db_path: dir.join("test.db"),
        master_key_path: dir.join("key").to_string_lossy().into_owned(),
        ..Default::default()
    })
    .unwrap();
    (state, dir)
}
fn channel(state: &SharedState, url: &str, enabled: bool) -> ChannelId {
    let conn = state.conn();
    let id = channels::create(
        &conn,
        &NewChannel {
            name: "test".into(),
            base_url: url.into(),
            enabled,
            unwrap_envelope: false,
            max_concurrent: 0,
        },
    )
    .unwrap();
    channels::set_models(
        &conn,
        id,
        &[ModelMapping::new("alias", Some("real".into()), false)],
    )
    .unwrap();
    id
}
async fn upstream(
    delay: Duration,
    fail: bool,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let app=Router::new().route("/models",get(move |headers:axum::http::HeaderMap| {
        let seen=seen.clone(); async move {
            seen.fetch_add(1,Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            if headers.contains_key("authorization") { assert_eq!(headers["authorization"],"Bearer rotated"); }
            (if fail {axum::http::StatusCode::SERVICE_UNAVAILABLE} else {axum::http::StatusCode::OK},
            Json(json!({"data":[{"id":"real","context_length":8192,"input_modalities":["text","image"]},{"id":"fresh","context_window":4096}]})))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, count, handle)
}
#[tokio::test]
async fn discovers_new_alias_and_persists_without_accounting_or_disabled_fetches() {
    let (url, count, server) = upstream(Duration::ZERO, false).await;
    let (state, dir) = fixture();
    let id = channel(&state, &url, true);
    let disabled = channel(&state, &url, false);
    capability_detector::tick(&state).await.unwrap();
    {
        let conn = state.conn();
        let route = model_routes::routes_for(&conn, "alias").unwrap().unwrap();
        assert_eq!(route.context_window, Some(8192));
        assert_eq!(route.context_source.as_deref(), Some("upstream"));
        assert!(route.can_see());
        assert!(db::catalog::list(&conn, disabled).unwrap().is_empty());
        for table in ["requests", "attempts", "routing_state"] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        channels::set_models(
            &conn,
            id,
            &[
                ModelMapping::new("alias", Some("real".into()), false),
                ModelMapping::new("fresh", None, false),
            ],
        )
        .unwrap();
    }
    capability_detector::tick(&state).await.unwrap();
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "new declarations trigger a refresh"
    );
    assert_eq!(
        model_routes::routes_for(&state.conn(), "fresh")
            .unwrap()
            .unwrap()
            .context_window,
        Some(4096)
    );
    db::sessions::set_gateway_key(&state.conn(), &state.secrets, "test-key").unwrap();
    let response = tokengo_server::app(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri("/v1/models")
                .header("authorization", "Bearer test-key")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for (name, context) in [("alias", 8192), ("fresh", 4096), ("auto", 4096)] {
        let item = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == name)
            .unwrap();
        assert_eq!(item["context_window"], context);
        assert_eq!(item["context_length"], context);
        assert_eq!(item["context_source"], "upstream");
    }
    let cfg = state.cfg.clone();
    drop(state);
    let state = AppState::init(cfg).unwrap();
    capability_detector::tick(&state).await.unwrap();
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "successful scheduling survives restart"
    );
    assert_eq!(
        model_routes::routes_for(&state.conn(), "alias")
            .unwrap()
            .unwrap()
            .context_window,
        Some(8192)
    );
    server.abort();
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn late_key_change_or_disable_cannot_apply_old_results() {
    for disable in [false, true] {
        let (url, count, server) = upstream(Duration::from_millis(100), false).await;
        let (state, dir) = fixture();
        let id = channel(&state, &url, true);
        let task = tokio::spawn({
            let state = state.clone();
            async move {
                capability_detector::tick(&state).await.unwrap();
            }
        });
        while count.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        {
            let conn = state.conn();
            if disable {
                channels::set_enabled(&conn, id, false).unwrap();
            } else {
                channels::set_secret(&conn, &state.secrets, id, "rotated").unwrap();
            }
        }
        task.await.unwrap();
        assert!(db::catalog::list(&state.conn(), id).unwrap().is_empty());
        if !disable {
            capability_detector::tick(&state).await.unwrap();
            assert_eq!(db::catalog::list(&state.conn(), id).unwrap().len(), 2);
        }
        server.abort();
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn aggregation_requires_all_eligible_mapped_metadata_and_ignores_disabled() {
    let (state, dir) = fixture();
    let a = channel(&state, "http://a", true);
    let b = channel(&state, "http://b", true);
    let conn = state.conn();
    let entries = tokengo_server::upstream::parse_model_catalog(
        r#"{"data":[{"id":"real","context_length":8192}]}"#,
    );
    db::catalog::replace(&conn, a, &entries, 1).unwrap();
    assert_eq!(
        model_routes::routes_for(&conn, "alias")
            .unwrap()
            .unwrap()
            .context_window,
        None
    );
    let unknown = tokengo_server::upstream::parse_model_catalog(r#"{"data":[{"id":"real"}]}"#);
    db::catalog::replace(&conn, b, &unknown, 1).unwrap();
    assert_eq!(
        model_routes::routes_for(&conn, "alias")
            .unwrap()
            .unwrap()
            .context_window,
        None,
        "missing fields remain unknown"
    );
    let entries = tokengo_server::upstream::parse_model_catalog(
        r#"{"data":[{"id":"real","context_length":2048}]}"#,
    );
    db::catalog::replace(&conn, b, &entries, 2).unwrap();
    assert_eq!(
        model_routes::routes_for(&conn, "alias")
            .unwrap()
            .unwrap()
            .context_window,
        Some(2048)
    );
    channels::set_enabled(&conn, b, false).unwrap();
    assert_eq!(
        model_routes::routes_for(&conn, "alias")
            .unwrap()
            .unwrap()
            .context_window,
        Some(8192)
    );
    assert_eq!(
        capability_detector::conservative_context([Some(8192), None]),
        None
    );
    drop(conn);
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn failures_back_off_without_erasing_previous_metadata() {
    let (url, count, server) = upstream(Duration::ZERO, true).await;
    let (state, dir) = fixture();
    let id = channel(&state, &url, true);
    let entries = tokengo_server::upstream::parse_model_catalog(
        r#"{"data":[{"id":"real","context_length":512}]}"#,
    );
    db::catalog::replace(&state.conn(), id, &entries, 1).unwrap();
    capability_detector::tick(&state).await.unwrap();
    capability_detector::tick(&state).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        db::catalog::list(&state.conn(), id).unwrap()[0].context_window,
        Some(512)
    );
    server.abort();
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}
