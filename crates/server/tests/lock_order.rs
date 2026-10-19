//! Observe the DB lock while a policy handler waits on routing, without a permanent deadlock.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use tokengo_server::{app, db, AppState, Config};
use tower::ServiceExt;

async fn policy_releases_database_before_waiting_for_routing(method: &str) {
    let dir = std::env::temp_dir().join(format!("tokengo-lock-order-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.db");
    let state = AppState::init(Config {
        db_path: path.clone(),
        master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
        ..Config::default()
    })
    .unwrap();
    let token = {
        let conn = state.conn();
        let id = db::channels::create(
            &conn,
            &db::channels::NewChannel {
                name: "one".into(),
                base_url: "http://127.0.0.1:1/v1".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        db::channels::set_models(
            &conn,
            id,
            &[db::channels::ModelMapping::new("test", None, false)],
        )
        .unwrap();
        if method == "DELETE" {
            db::settings::set(&conn, "model_routing:test", r#"{"retry_times":1}"#).unwrap();
        }
        db::sessions::create_session(&conn, None).unwrap()
    };
    let deleting = method == "DELETE";
    let observed = state.clone();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let observer = std::thread::spawn(move || {
        let routing = observed.routing();
        // Read through a separate SQLite connection: it does not need the process DB mutex.
        let external = rusqlite::Connection::open(path).unwrap();
        external.busy_timeout(Duration::from_millis(10)).unwrap();
        ready_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut committed = false;
        let mut database_available = false;
        while Instant::now() < deadline {
            let stored = db::settings::get(&external, "model_routing:test");
            committed = match stored {
                Ok(value) if deleting => value.is_none(),
                Ok(Some(value)) => value == r#"{"retry_times":0}"#,
                _ => false,
            };
            if committed {
                database_available = observed.db.try_lock().is_ok();
                if database_available {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        // Always release routing before returning/asserting. Even the buggy handler
        // can now finish and the test runtime is never left in a two-lock deadlock.
        drop(routing);
        (committed, database_available)
    });
    ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let request = Request::builder()
        .method(method)
        .uri("/api/model-routing?model=test")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"retry_times":0}"#))
        .unwrap();
    let response = app(state.clone()).oneshot(request).await.unwrap();
    let (committed, available) = observer.join().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        committed,
        "{method}: observer did not see the policy transaction commit"
    );
    assert!(
        available,
        "{method}: committed handler retained DB mutex while waiting for routing mutex"
    );
}

#[tokio::test]
async fn put_policy_does_not_hold_db_while_waiting_for_routing() {
    policy_releases_database_before_waiting_for_routing("PUT").await;
}
#[tokio::test]
async fn delete_policy_does_not_hold_db_while_waiting_for_routing() {
    policy_releases_database_before_waiting_for_routing("DELETE").await;
}
