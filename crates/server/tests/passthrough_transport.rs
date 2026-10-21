//! Real sockets expose no-body and downstream backpressure behavior.
use axum::{body::Body, http::StatusCode, response::Response, Router};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokengo_server::{
    app,
    db::{
        self,
        billing::Ledger,
        identity::{self, Policy},
        passthrough,
    },
    AppState, Config, SharedState,
};
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
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server { url, task }
}
struct Fixture {
    state: SharedState,
    dir: std::path::PathBuf,
    key: String,
    key_id: i64,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-transport-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        let (k, key) = identity::create_key(
            &state.conn(),
            1,
            "photos",
            &Policy {
                max_concurrent: Some(1),
                ..Policy::default()
            },
            None,
            None,
        )
        .unwrap();
        Self {
            state,
            dir,
            key,
            key_id: k.id,
        }
    }
    fn route(&self, url: &str, timeout: u32) {
        let input=serde_json::from_value(serde_json::json!({"name":"photos","prefix":"/face/","upstream":url,"strip_prefix":true,"enabled":true,"price_micro_usd_per_request":50,"timeout_secs":timeout,"max_concurrent":1,"max_body_bytes":20971520,"allowed_key_ids":[self.key_id],"headers":[{"name":"api-key","value":"backend-secret"}]})).unwrap();
        passthrough::create(&self.state.conn(), &self.state.secrets, &input).unwrap();
    }
    async fn settled(&self, count: usize) -> Vec<Ledger> {
        tokio::time::timeout(Duration::from_secs(6), async {
            loop {
                let rows = db::billing::usage(&self.state.conn(), &Default::default()).unwrap();
                if rows.len() == count && rows.iter().all(|r| r.settled_ms.is_some()) {
                    return rows;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("ledger must settle including disconnected/no-body responses")
    }
    fn no_holds(&self) {
        for b in db::billing::budgets(&self.state.conn(), None, None).unwrap() {
            assert_eq!(b.reserved_micro, 0);
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .build()
        .unwrap()
}
fn endless() -> Body {
    Body::from_stream(
        stream::once(async { Ok::<_, Infallible>(Bytes::from_static(b"first")) })
            .chain(stream::pending()),
    )
}
#[tokio::test]
async fn invalid_gzip_stays_opaque() {
    let up = serve(Router::new().fallback(|| async {
        let mut r = Response::new(Body::from(vec![255, 0, 1, 2, 128]));
        r.headers_mut()
            .insert("content-encoding", "gzip".parse().unwrap());
        r
    }))
    .await;
    let f = Fixture::new();
    f.route(&up.url, 3);
    let g = serve(app(f.state.clone())).await;
    let r = client()
        .get(format!("{}/face/represent", g.url))
        .header("api-key", &f.key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-encoding"], "gzip");
    assert_eq!(r.bytes().await.unwrap().as_ref(), [255, 0, 1, 2, 128]);
    assert_eq!(f.settled(1).await[0].request_units, Some(1));
    f.no_holds();
}
#[tokio::test]
async fn redirect_does_not_follow() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let target = serve(Router::new().fallback(move || {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            "must not reach"
        }
    }))
    .await;
    let location = target.url.clone();
    let up = serve(Router::new().fallback(move || {
        let location = location.clone();
        async move {
            let mut r = Response::new(Body::from("redirect body"));
            *r.status_mut() = StatusCode::TEMPORARY_REDIRECT;
            r.headers_mut()
                .insert("location", location.parse().unwrap());
            r
        }
    }))
    .await;
    let f = Fixture::new();
    f.route(&up.url, 3);
    let g = serve(app(f.state.clone())).await;
    let r = client()
        .get(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 307);
    assert_eq!(r.headers()["location"], target.url);
    assert_eq!(r.text().await.unwrap(), "redirect body");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.settled(1).await[0].request_units, Some(1));
    f.no_holds();
}
#[tokio::test]
async fn real_http_204_and_304_settle_once() {
    for status in [StatusCode::NO_CONTENT, StatusCode::NOT_MODIFIED] {
        let up = serve(Router::new().fallback(move || async move {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = status;
            r
        }))
        .await;
        let f = Fixture::new();
        f.route(&up.url, 3);
        let g = serve(app(f.state.clone())).await;
        let r = client()
            .get(format!("{}/face/represent", g.url))
            .bearer_auth(&f.key)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), status);
        assert!(r.bytes().await.unwrap().is_empty());
        let rows = f.settled(1).await;
        assert_eq!(rows[0].request_units, Some(1), "{status}");
        assert_eq!(rows[0].outcome.as_deref(), Some("completed"));
        assert_eq!(rows[0].debit_micro, Some(50));
        f.no_holds();
    }
}

#[tokio::test]
async fn head_settles_at_headers_without_polling_downstream_body() {
    let up =
        serve(Router::new().fallback(|| async { Response::new(Body::from("upstream metadata")) }))
            .await;
    let f = Fixture::new();
    f.route(&up.url, 3);
    let g = serve(app(f.state.clone())).await;
    let response = client()
        .head(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    drop(response);
    let rows = f.settled(1).await;
    assert_eq!(rows[0].request_units, Some(1));
    assert_eq!(rows[0].debit_micro, Some(50));
    f.no_holds();
    let response = client()
        .head(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    drop(response);
    assert_eq!(f.settled(2).await.len(), 2);
}

#[tokio::test]
async fn chunked_upload_limit_returns_413_and_settles_unknown_without_holds() {
    let up = serve(
        Router::new().fallback(|request: axum::extract::Request| async move {
            let _ = axum::body::to_bytes(request.into_body(), 1024).await;
            Response::new(Body::from("unexpected full upload"))
        }),
    )
    .await;
    let f = Fixture::new();
    f.route(&up.url, 3);
    f.state
        .conn()
        .execute("UPDATE passthrough_routes SET max_body_bytes=4", [])
        .unwrap();
    let g = serve(app(f.state.clone())).await;
    let chunks = stream::iter(vec![
        Ok::<_, std::io::Error>(Bytes::from_static(b"abc")),
        Ok(Bytes::from_static(b"def")),
    ]);
    let response = client()
        .post(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    let _ = response.bytes().await.unwrap();
    let rows = f.settled(1).await;
    assert_eq!(rows[0].request_units, None);
    assert_eq!(rows[0].debit_micro, Some(50));
    assert_eq!(rows[0].input_tokens, None);
    f.no_holds();
    let response = client()
        .post(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .body("a")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let _ = response.bytes().await.unwrap();
    assert_eq!(
        f.settled(2)
            .await
            .iter()
            .filter(|r| r.request_units == Some(1))
            .count(),
        1
    );
}
async fn run_interruption(cancel: bool) {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let up = serve(Router::new().fallback(move || {
        let c = c.clone();
        async move {
            Response::new(if c.fetch_add(1, Ordering::SeqCst) == 0 {
                endless()
            } else {
                Body::from("next")
            })
        }
    }))
    .await;
    let f = Fixture::new();
    f.route(&up.url, 1);
    let g = serve(app(f.state.clone())).await;
    let client = client();
    let mut r = client
        .get(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .send()
        .await
        .unwrap();
    if cancel {
        assert_eq!(r.chunk().await.unwrap().unwrap().as_ref(), b"first");
        drop(r);
    } else {
        assert!(r.bytes().await.is_err());
    }
    let rows = f.settled(1).await;
    assert_eq!(rows[0].request_units, None);
    assert_eq!(rows[0].input_tokens, None);
    assert_eq!(rows[0].output_tokens, None);
    if !cancel {
        assert_eq!(rows[0].outcome.as_deref(), Some("upstream_timeout"));
    }
    f.no_holds();
    let next = client
        .get(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .send()
        .await
        .unwrap();
    assert_eq!(next.status(), 200);
    assert_eq!(next.text().await.unwrap(), "next");
    let rows = f.settled(2).await;
    assert_eq!(
        rows.iter().filter(|r| r.request_units == Some(1)).count(),
        1
    );
    f.no_holds();
}
#[tokio::test]
async fn read_timeout_releases_concurrency_and_keeps_unknown_units() {
    run_interruption(false).await;
}
#[tokio::test]
async fn client_disconnect_releases_concurrency_and_keeps_unknown_units() {
    run_interruption(true).await;
}
#[tokio::test]
async fn deadline_fires_even_when_downstream_does_not_read() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let up = serve(Router::new().fallback(move || {
        let c = c.clone();
        async move {
            if c.fetch_add(1, Ordering::SeqCst) == 0 {
                Response::new(Body::from_stream(stream::repeat_with(|| {
                    Ok::<_, Infallible>(Bytes::from(vec![42; 65536]))
                })))
            } else {
                Response::new(Body::from("next"))
            }
        }
    }))
    .await;
    let f = Fixture::new();
    f.route(&up.url, 1);
    let g = serve(app(f.state.clone())).await;
    let client = client();
    let unread = client
        .get(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .send()
        .await
        .unwrap();
    let rows = f.settled(1).await;
    assert_eq!(rows[0].request_units, None);
    assert_eq!(rows[0].outcome.as_deref(), Some("upstream_timeout"));
    f.no_holds();
    let next = client
        .get(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .send()
        .await
        .unwrap();
    assert_eq!(next.status(), 200);
    assert_eq!(next.text().await.unwrap(), "next");
    drop(unread);
    f.settled(2).await;
    f.no_holds();
}

#[tokio::test]
async fn api_key_alias_rejects_conflicts_and_replaces_backend_credential() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let up = serve(
        Router::new().fallback(move |request: axum::extract::Request| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                assert_eq!(request.headers()["api-key"], "backend-secret");
                assert!(!request.headers().contains_key("authorization"));
                assert!(!request.headers().contains_key("x-api-key"));
                "ok"
            }
        }),
    )
    .await;
    let f = Fixture::new();
    f.route(&up.url, 3);
    let g = serve(app(f.state.clone())).await;
    let client = client();
    let conflict = client
        .get(format!("{}/face/represent", g.url))
        .bearer_auth(&f.key)
        .header("api-key", "different-inbound-key")
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 400);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let valid = client
        .get(format!("{}/face/represent", g.url))
        .header("api-key", &f.key)
        .send()
        .await
        .unwrap();
    assert_eq!(valid.status(), 200);
    assert_eq!(valid.text().await.unwrap(), "ok");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.settled(1).await[0].request_units, Some(1));
}
