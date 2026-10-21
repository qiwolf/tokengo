//! Root-only management contracts, independent of transport fixtures.
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tokengo_server::{AppState, Config, SharedState, app, db};
use tower::ServiceExt;
struct Fixture {
    state: SharedState,
    dir: std::path::PathBuf,
    root: String,
    user: String,
    ops: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-pass-admin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        let (root, user, ops) = {
            let c = state.conn();
            db::sessions::set_panel_password(&c, "passthrough-safe-test-password").unwrap();
            let root = db::accounts::by_username(&c, "admin").unwrap().unwrap();
            let root_session = db::accounts::create_session(&c, &root).unwrap();
            let u = db::identity::create_user(
                &c,
                "ordinary",
                "admin",
                &db::identity::Policy {
                    admin_access: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let account =
                db::accounts::create(&c, "ordinary", "ordinary", u.id, &root.hash).unwrap();
            let user = db::accounts::create_session(
                &c,
                &db::accounts::by_id(&c, account.id).unwrap().unwrap(),
            )
            .unwrap();
            let ops = db::operations_tokens::create(&c, "agent", 90, root.account.id).unwrap();
            (
                format!("tokengo_account_session={root_session}"),
                format!("tokengo_account_session={user}"),
                ops.secret,
            )
        };
        Self {
            state,
            dir,
            root,
            user,
            ops,
        }
    }
    async fn req(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        bearer: Option<&str>,
        v: Value,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json");
        if let Some(c) = cookie {
            b = b
                .header("cookie", c)
                .header("origin", "http://localhost")
                .header("x-tokengo-panel", "1");
        }
        if let Some(t) = bearer {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let r = app(self.state.clone())
            .oneshot(b.body(Body::from(v.to_string())).unwrap())
            .await
            .unwrap();
        let s = r.status();
        let bytes = to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    async fn root(&self, method: &str, path: &str, v: Value) -> (StatusCode, Value) {
        self.req(method, path, Some(&self.root), None, v).await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn route() -> Value {
    json!({"name":"Face","prefix":"/face/","upstream":"http://127.0.0.1:8066","enabled":true,"price_micro_usd_per_request":50,"headers":[{"name":"api-key","value":"private-backend-marker"}],"allowed_paths":["/represent"],"allowed_key_ids":[]})
}
#[tokio::test]
async fn anonymous_and_nonroot_and_ops_cannot_manage_routes() {
    let f = Fixture::new();
    for (method, path) in [
        ("GET", "/api/passthrough-routes"),
        ("POST", "/api/passthrough-routes"),
        ("PUT", "/api/passthrough-routes/1"),
        ("DELETE", "/api/passthrough-routes/1?revision=1"),
    ] {
        let (s, v) = f.req(method, path, None, None, route()).await;
        assert_eq!(s, 401, "{v}");
        let (s, v) = f.req(method, path, Some(&f.user), None, route()).await;
        assert_eq!(s, 403, "{v}");
        let (s, v) = f.req(method, path, None, Some(&f.ops), route()).await;
        assert!(s == 401 || s == 403, "{s} {v}");
    }
    assert!(db::passthrough::list(&f.state.conn()).unwrap().is_empty());
}
#[tokio::test]
async fn root_crud_secret_masking_revision_and_hidden_sources() {
    let f = Fixture::new();
    let (s, created) = f.root("POST", "/api/passthrough-routes", route()).await;
    assert_eq!(s, 200, "{created}");
    let id = created["id"].as_i64().unwrap();
    let cid = created["channel_id"].as_i64().unwrap();
    assert!(!created.to_string().contains("private-backend-marker"));
    assert_eq!(
        created["headers"],
        json!([{"name":"api-key","has_value":true}])
    );
    let (s, list) = f.root("GET", "/api/passthrough-routes", json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains("private-backend-marker"));
    let mut updated = route();
    updated["revision"] = json!(1);
    updated["headers"] = json!([{"name":"api-key","value":null}]);
    updated["name"] = json!("Face renamed");
    let (s, v) = f
        .root(
            "PUT",
            &format!("/api/passthrough-routes/{id}"),
            updated.clone(),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["revision"], 2);
    let (s, _) = f
        .root("PUT", &format!("/api/passthrough-routes/{id}"), updated)
        .await;
    assert_eq!(s, 409);
    let (s, _) = f
        .root(
            "DELETE",
            &format!("/api/passthrough-routes/{id}?revision=1"),
            json!({}),
        )
        .await;
    assert_eq!(s, 409);
    for path in [
        format!("/api/channels/{cid}"),
        format!("/api/channels/{cid}/provider"),
    ] {
        let(s,_)=f.root("PUT",&path,json!({"name":"attack","base_url":"http://localhost:1","enabled":true,"provider_id":null})).await;
        assert!(!s.is_success(), "{path}");
    }
    let (s, v) = f
        .root(
            "POST",
            "/api/request-pricing",
            json!({"channel_id":cid,"model":format!("passthrough:{id}"),"micro_per_call":0}),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(
        db::passthrough::get(&f.state.conn(), id)
            .unwrap()
            .unwrap()
            .price_micro_usd_per_request,
        50
    );
    let (s, v) = f
        .root(
            "DELETE",
            &format!("/api/passthrough-routes/{id}?revision=2"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(db::passthrough::list(&f.state.conn()).unwrap().is_empty());
    let next = db::channels::create(
        &f.state.conn(),
        &db::channels::NewChannel {
            name: "next".into(),
            base_url: "http://localhost".into(),
            enabled: true,
            unwrap_envelope: false,
            max_concurrent: 1,
        },
    )
    .unwrap();
    assert!(next.0 > cid);
}
