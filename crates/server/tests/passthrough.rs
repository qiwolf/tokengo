use axum::{
    body::{to_bytes, Body},
    extract::Request,
    response::Response,
    Router,
};
use tokengo_server::{
    app,
    db::{
        self,
        identity::{self, Policy},
        passthrough,
    },
    AppState, Config, SharedState,
};
use tower::ServiceExt;

struct Fixture {
    state: SharedState,
    dir: std::path::PathBuf,
    key: String,
    key_id: i64,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-raw-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        let (k, key) =
            identity::create_key(&state.conn(), 1, "photos", &Policy::default(), None, None)
                .unwrap();
        Self {
            state,
            dir,
            key,
            key_id: k.id,
        }
    }
    fn route(&self, upstream: &str, prefix: &str, extra: serde_json::Value) -> passthrough::Route {
        let mut v = serde_json::json!({"name":"photos","prefix":prefix,"upstream":upstream,"strip_prefix":true,"enabled":true,"price_micro_usd_per_request":50,"timeout_secs":5,"max_concurrent":2,"max_body_bytes":20971520,"allowed_key_ids":[self.key_id],"headers":[{"name":"api-key","value":"backend-secret"}],"allowed_paths":null});
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        passthrough::create(
            &self.state.conn(),
            &self.state.secrets,
            &serde_json::from_value(v).unwrap(),
        )
        .unwrap()
    }
    async fn call(&self, path: &str, body: Vec<u8>) -> Response {
        app(self.state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("authorization", format!("Bearer {}", self.key))
                    .header("x-api-key", &self.key)
                    .header("cookie", "tokengo_account=private")
                    .header("connection", "x-remove")
                    .header("x-remove", "secret-hop")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
async fn mock(handler: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, handler).await.unwrap() });
    (url, task)
}
#[tokio::test]
async fn forwards_binary_query_and_only_backend_auth_and_counts_one_without_tokens() {
    let bytes = vec![0, 255, 1, 2, 13, 10, 0, 128];
    let expected = bytes.clone();
    let (url, task) = mock(Router::new().fallback(move |r: Request| {
        let expected = expected.clone();
        async move {
            assert_eq!(r.uri().to_string(), "/represent?q=a%2Fb&q=+&token=business");
            assert_eq!(r.headers()["api-key"], "backend-secret");
            for h in [
                "authorization",
                "proxy-authorization",
                "x-api-key",
                "cookie",
                "x-remove",
            ] {
                assert!(!r.headers().contains_key(h), "{h}");
            }
            assert_eq!(
                to_bytes(r.into_body(), 1024).await.unwrap().as_ref(),
                expected
            );
            let mut out = Response::new(Body::from(vec![255, 0, 128]));
            out.headers_mut()
                .append("set-cookie", "a=1".parse().unwrap());
            out.headers_mut()
                .append("set-cookie", "b=2".parse().unwrap());
            out
        }
    }))
    .await;
    let f = Fixture::new();
    f.route(&url, "/face/", serde_json::json!({}));
    let response = f
        .call("/face/represent?q=a%2Fb&q=+&token=business", bytes)
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers().get_all("set-cookie").iter().count(), 2);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        [255, 0, 128]
    );
    let row = db::billing::usage(&f.state.conn(), &Default::default())
        .unwrap()
        .remove(0);
    assert_eq!(row.debit_micro, Some(50));
    assert_eq!(row.request_units, Some(1));
    assert_eq!(row.input_tokens, None);
    assert_eq!(row.output_tokens, None);
    task.abort();
}
#[tokio::test]
async fn denied_longer_route_never_falls_back_and_allowlist_blocks_restart() {
    let f = Fixture::new();
    f.route(
        "http://127.0.0.1:1",
        "/face/",
        serde_json::json!({"allowed_paths":["/represent"]}),
    );
    assert_eq!(f.call("/face/restart", vec![]).await.status(), 403);
    f.route(
        "http://127.0.0.1:1",
        "/face/private/",
        serde_json::json!({"allowed_key_ids":[]}),
    );
    assert_eq!(
        f.call("/face/private/represent", vec![]).await.status(),
        403
    );
    assert!(db::billing::usage(&f.state.conn(), &Default::default())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn mtphotos_api_key_alias_all_methods_strip_false_and_encoded_paths() {
    let (url, task) = mock(Router::new().fallback(|r: Request| async move {
        assert_eq!(r.headers()["api-key"], "backend-secret");
        assert!(!r.headers().contains_key("x-api-key"));
        Response::new(Body::from(format!("{} {}", r.method(), r.uri())))
    }))
    .await;
    let f = Fixture::new();
    f.route(&url, "/clip/", serde_json::json!({"strip_prefix":false}));
    for method in [
        "GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "PROPFIND",
    ] {
        let response = app(f.state.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/clip/a%2Fb?q=a%20b&x=+&x=%2B")
                    .header("api-key", &f.key)
                    .body(Body::from(vec![0, 255]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            format!("{method} /clip/a%2Fb?q=a%20b&x=+&x=%2B")
        );
    }
    let conflict = app(f.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/clip/test")
                .header("api-key", &f.key)
                .header("authorization", "Bearer wrong")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflict.status(), 400);
    task.abort();
}

#[tokio::test]
async fn rejects_budget_tpm_and_upload_bound_before_upstream() {
    let f = Fixture::new();
    f.route(
        "http://127.0.0.1:1",
        "/face/",
        serde_json::json!({"max_body_bytes":4}),
    );
    db::billing::set_budget(&f.state.conn(), "key", f.key_id, Some(0), "test").unwrap();
    assert_eq!(f.call("/face/represent", vec![]).await.status(), 402);
    assert!(db::billing::usage(&f.state.conn(), &Default::default())
        .unwrap()
        .is_empty());
    db::billing::set_budget(&f.state.conn(), "key", f.key_id, Some(100), "test").unwrap();
    {
        let c = f.state.conn();
        let mut key = identity::key(&c, f.key_id).unwrap().unwrap();
        key.policy.tpm = Some(100);
        identity::update_key(&c, &key).unwrap();
    }
    assert_eq!(f.call("/face/represent", vec![]).await.status(), 403);
    {
        let c = f.state.conn();
        let mut key = identity::key(&c, f.key_id).unwrap().unwrap();
        key.policy.tpm = None;
        identity::update_key(&c, &key).unwrap();
    }
    let too_large = app(f.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/face/represent")
                .header("api-key", &f.key)
                .header("content-length", "5")
                .body(Body::from(vec![0; 5]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(too_large.status(), 413);
    assert!(db::billing::usage(&f.state.conn(), &Default::default())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn rejects_normalized_path_and_preserves_raw_upstream_error_and_rpm() {
    let (url, task) = mock(Router::new().fallback(|| async {
        Response::builder()
            .status(503)
            .header("content-type", "application/octet-stream")
            .body(Body::from(vec![0, 255, 42]))
            .unwrap()
    }))
    .await;
    let f = Fixture::new();
    f.route(&url, "/face/", serde_json::json!({}));
    assert_eq!(f.call("/face/%2e%2e/private", vec![]).await.status(), 400);
    {
        let c = f.state.conn();
        let mut key = identity::key(&c, f.key_id).unwrap().unwrap();
        key.policy.rpm = Some(1);
        identity::update_key(&c, &key).unwrap();
    }
    let response = f.call("/face/represent", vec![]).await;
    assert_eq!(response.status(), 503);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        [0, 255, 42]
    );
    assert_eq!(f.call("/face/represent", vec![]).await.status(), 429);
    let rows = db::billing::usage(&f.state.conn(), &Default::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].request_units, Some(1));
    assert_eq!(rows[0].debit_micro, Some(50));
    task.abort();
}

#[tokio::test]
async fn manual_probe_records_system_cost_without_user_budget_or_body_echo() {
    let (url, task) = mock(Router::new().fallback(|r: Request| async move {
        assert_eq!(r.method(), "POST");
        assert_eq!(r.uri().path(), "/check");
        assert_eq!(r.headers()["api-key"], "backend-secret");
        Response::new(Body::from("private backend health details"))
    }))
    .await;
    let f = Fixture::new();
    let route = f.route(
        &url,
        "/face/",
        serde_json::json!({"allowed_paths":["/check","/represent"]}),
    );
    db::billing::set_budget(&f.state.conn(), "key", f.key_id, Some(0), "test").unwrap();
    let session = {
        let c = f.state.conn();
        db::sessions::set_panel_password(&c, "probe-safe-test-password").unwrap();
        let root = db::accounts::by_username(&c, "admin").unwrap().unwrap();
        db::accounts::create_session(&c, &root).unwrap()
    };
    let path = format!("/api/passthrough-routes/{}/probe", route.id);
    let response = app(f.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "localhost")
                .header("origin", "http://localhost")
                .header("x-tokengo-panel", "1")
                .header("cookie", format!("tokengo_account_session={session}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"path":"/check","method":"POST","expected_status":200}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["healthy"], true);
    assert_eq!(result["inference_verified"], false);
    assert!(!String::from_utf8_lossy(&bytes).contains("private"));
    let rows = db::billing::usage(&f.state.conn(), &Default::default()).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!((row.user_id, row.key_id, row.request_id), (0, 0, 0));
    assert_eq!(row.debit_micro, Some(0));
    assert_eq!(row.observed_cost_micro, Some(50));
    assert_eq!(row.request_units, Some(1));
    let budget = db::billing::budgets(&f.state.conn(), Some("key"), Some(f.key_id))
        .unwrap()
        .remove(0);
    assert_eq!(budget.spent_micro, 0);
    assert_eq!(budget.reserved_micro, 0);
    task.abort();
}
