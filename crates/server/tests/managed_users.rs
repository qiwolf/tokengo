use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use serde_json::{Value, json};
use tokengo_server::{AppState, Config, SharedState, app, db};
use tower::ServiceExt;
const PASSWORD: &str = "legacy-password";
#[derive(Clone)]
struct Fixture {
    state: SharedState,
    legacy: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-managed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into_owned(),
            ..Config::default()
        };
        let state = AppState::init(cfg.clone()).unwrap();
        db::sessions::set_panel_password(&state.conn(), PASSWORD).unwrap();
        let legacy = db::sessions::create_session(&state.conn(), None).unwrap();
        Self { state, legacy }
    }
    async fn req(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        bearer: Option<&str>,
        csrf: bool,
        origin: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value, HeaderMap) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json");
        if let Some(c) = cookie {
            b = b.header("cookie", c)
        }
        if let Some(t) = bearer {
            b = b.header("authorization", format!("Bearer {t}"))
        }
        if csrf {
            b = b.header("x-tokengo-panel", "1")
        }
        if let Some(o) = origin {
            b = b.header("origin", o)
        }
        let r = app(self.state.clone())
            .oneshot(b.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = r.status();
        let headers = r.headers().clone();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            headers,
        )
    }
    async fn admin(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value, HeaderMap) {
        self.req(method, path, None, Some(&self.legacy), false, None, body)
            .await
    }
    async fn login(&self, username: &str, password: &str) -> (Value, String) {
        let (s, v, h) = self
            .req(
                "POST",
                "/api/account/login",
                None,
                None,
                true,
                Some("http://localhost"),
                json!({"username":username,"password":password}),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let raw = h.get("set-cookie").unwrap().to_str().unwrap();
        assert!(raw.contains("HttpOnly") && raw.contains("SameSite=Lax") && raw.contains("Path=/"));
        assert!(v.get("token").is_none());
        (v, raw.split(';').next().unwrap().to_owned())
    }
    fn user(&self, name: &str) -> i64 {
        db::identity::create_user(
            &self.state.conn(),
            name,
            "admin",
            &db::identity::Policy {
                admin_access: true,
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }
}
#[tokio::test]
async fn unified_creation_is_atomic_and_login_and_self_service_work() {
    let f = Fixture::new();
    let policy = json!({"allowed_models":["allowed"],"allowed_channels":[2],"allow_paid":false,"rpm":20,"tpm":1000,"max_concurrent":2});
    let (s,v,_)=f.admin("POST","/api/managed-users",json!({"username":"alice","display_name":"Alice","password":PASSWORD,"policy":policy,"limit_micro":12000})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["user"]["id"].as_i64().unwrap();
    assert_eq!(v["user"]["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(v["user"]["budget"]["limit_micro"], 12000);
    let (_, cookie) = f.login("alice", PASSWORD).await;
    let (s, v, h) = f
        .req(
            "POST",
            "/api/account/keys",
            Some(&cookie),
            None,
            true,
            None,
            json!({"name":"phone"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(h["cache-control"], "no-store");
    let secret = v["secret"].as_str().unwrap();
    let auth = f
        .state
        .authenticate_context(secret.to_owned())
        .await
        .unwrap();
    assert_eq!(auth.user_id, id);
    assert!(!auth.effective_policy().admin_access);
    assert_eq!(
        auth.effective_policy().allowed_models,
        Some(vec!["allowed".to_string()])
    );
    let (s, listed, _) = f
        .req(
            "GET",
            "/api/account/keys",
            Some(&cookie),
            None,
            false,
            None,
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{listed}");
    assert!(listed["keys"][0].get("secret").is_none());
    assert_eq!(listed["budget"]["limit_micro"], 12000);
    let before_users = db::identity::users(&f.state.conn()).unwrap().len();
    let before_accounts = db::accounts::accounts(&f.state.conn()).unwrap().len();
    let (s, _, _) = f
        .admin(
            "POST",
            "/api/managed-users",
            json!({"username":"alice","display_name":"Duplicate","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(
        db::identity::users(&f.state.conn()).unwrap().len(),
        before_users
    );
    assert_eq!(
        db::accounts::accounts(&f.state.conn()).unwrap().len(),
        before_accounts
    );
}

#[tokio::test]
async fn self_service_enforces_csrf_scope_and_only_narrows_keys() {
    let f = Fixture::new();
    for name in ["alice", "bob"] {
        let (s,v,_)=f.admin("POST","/api/managed-users",json!({"username":name,"display_name":name,"password":PASSWORD,"policy":{"allowed_models":["a"],"rpm":10}})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    let (_, a) = f.login("alice", PASSWORD).await;
    let (_, b) = f.login("bob", PASSWORD).await;
    assert_eq!(
        f.req(
            "POST",
            "/api/account/keys",
            Some(&a),
            None,
            false,
            None,
            json!({"name":"bad"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for policy in [
        json!({"admin_access":true}),
        json!({"allowed_models":["b"],"rpm":10}),
        json!({"allowed_models":["a"],"rpm":11}),
    ] {
        assert_eq!(
            f.req(
                "POST",
                "/api/account/keys",
                Some(&a),
                None,
                true,
                None,
                json!({"name":"bad","policy":policy})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.req(
            "POST",
            "/api/account/keys",
            Some(&a),
            None,
            true,
            None,
            json!({"name":"bad","user_id":1})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (s, v, _) = f
        .req(
            "POST",
            "/api/account/keys",
            Some(&a),
            None,
            true,
            None,
            json!({"name":"own"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["key"]["id"].as_i64().unwrap();
    let path = format!("/api/account/keys/{id}");
    let secret = v["secret"].as_str().unwrap();
    assert_eq!(
        f.req("DELETE", &path, Some(&b), None, true, None, json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.req(
            "PATCH",
            &path,
            Some(&b),
            None,
            true,
            None,
            json!({"name":"stolen"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (s, v, _) = f
        .req(
            "PATCH",
            &path,
            Some(&a),
            None,
            true,
            None,
            json!({"policy":{"allowed_models":["a"],"rpm":5}}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        f.req(
            "PATCH",
            &path,
            Some(&a),
            None,
            true,
            None,
            json!({"policy":{"allowed_models":["a"],"rpm":8}})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.req(
            "PATCH",
            &path,
            Some(&a),
            None,
            true,
            None,
            json!({"enabled":false})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        f.state
            .authenticate_context(secret.to_owned())
            .await
            .is_none()
    );
    assert_eq!(
        f.req(
            "PATCH",
            &path,
            Some(&a),
            None,
            true,
            None,
            json!({"enabled":true})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.req("DELETE", &path, Some(&a), None, true, None, json!({}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req(
            "GET",
            "/api/managed-users",
            Some(&a),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, root) = f.login("admin", PASSWORD).await;
    assert_eq!(
        f.req(
            "POST",
            "/api/account/keys",
            Some(&root),
            None,
            true,
            None,
            json!({"name":"root"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn existing_identity_gains_login_without_rewriting_keys_and_root_stays_separate() {
    let f = Fixture::new();
    let id = f.user("existing");
    let (key, secret) =
        db::identity::create_key(&f.state.conn(), id, "old", &Default::default(), None, None)
            .unwrap();
    let (s, v, _) = f.admin("GET", "/api/managed-users", json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["root_accounts"][0]["user_id"], Value::Null);
    let row = v["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id)
        .unwrap();
    assert!(row["accounts"].as_array().unwrap().is_empty());
    let path = format!("/api/managed-users/{id}/login-credential");
    let (s, v, _) = f
        .admin(
            "POST",
            &path,
            json!({"username":"existing","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["user"]["id"], id);
    assert_eq!(
        f.admin(
            "POST",
            &path,
            json!({"username":"duplicate","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (_, cookie) = f.login("existing", PASSWORD).await;
    let (s, v, _) = f
        .req(
            "GET",
            "/api/account/keys",
            Some(&cookie),
            None,
            false,
            None,
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["policy"]["admin_access"], false);
    assert_eq!(
        f.state
            .authenticate_context(secret.clone())
            .await
            .unwrap()
            .key_id,
        key.id
    );
    let original = db::identity::key(&f.state.conn(), key.id).unwrap().unwrap();
    assert_eq!(original.user_id, id);
    assert!(original.parent_key_id.is_none());
    let (s, v, _) = f
        .admin(
            "PATCH",
            &format!("/api/managed-users/{id}"),
            json!({"enabled":false}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(f.state.authenticate_context(secret.clone()).await.is_none());
    assert_eq!(
        f.req(
            "POST",
            "/api/account/keys",
            Some(&cookie),
            None,
            true,
            None,
            json!({"name":"after-disable"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        db::identity::key(&f.state.conn(), key.id)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn self_service_budgets_cannot_raise_clear_or_escape_user_or_parent_caps() {
    let f = Fixture::new();
    let (s,v,_)=f.admin("POST","/api/managed-users",json!({"username":"budgeted","display_name":"Budget","password":PASSWORD,"limit_micro":1000})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let user = v["user"]["id"].as_i64().unwrap();
    let (_, cookie) = f.login("budgeted", PASSWORD).await;
    for limit in [Value::Null, json!(1001)] {
        assert_eq!(
            f.req(
                "POST",
                "/api/account/keys",
                Some(&cookie),
                None,
                true,
                None,
                json!({"name":"wide","limit_micro":limit})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let (s, v, _) = f
        .req(
            "POST",
            "/api/account/keys",
            Some(&cookie),
            None,
            true,
            None,
            json!({"name":"limited","limit_micro":400}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let key = v["key"]["id"].as_i64().unwrap();
    let path = format!("/api/account/keys/{key}");
    assert_eq!(v["key"]["budget"]["limit_micro"], 400);
    for limit in [Value::Null, json!(401), json!(1001)] {
        assert_eq!(
            f.req(
                "PATCH",
                &path,
                Some(&cookie),
                None,
                true,
                None,
                json!({"limit_micro":limit})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    {
        let c = f.state.conn();
        c.execute("UPDATE billing_budgets SET spent_micro=20,reserved_micro=30 WHERE scope='key' AND scope_id=?1",[key]).unwrap();
    }
    let (s, v, _) = f
        .req(
            "PATCH",
            &path,
            Some(&cookie),
            None,
            true,
            None,
            json!({"limit_micro":200}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["key"]["budget"]["limit_micro"], 200);
    assert_eq!(v["key"]["budget"]["spent_micro"], 20);
    assert_eq!(v["key"]["budget"]["reserved_micro"], 30);
    let parent = db::identity::create_key(
        &f.state.conn(),
        user,
        "parent",
        &Default::default(),
        None,
        None,
    )
    .unwrap()
    .0;
    db::billing::set_budget(&f.state.conn(), "key", parent.id, Some(100), "test").unwrap();
    let child = db::identity::create_key(
        &f.state.conn(),
        user,
        "child",
        &Default::default(),
        None,
        Some(parent.id),
    )
    .unwrap()
    .0;
    let path = format!("/api/account/keys/{}", child.id);
    assert_eq!(
        f.req(
            "PATCH",
            &path,
            Some(&cookie),
            None,
            true,
            None,
            json!({"limit_micro":101})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.req(
            "PATCH",
            &path,
            Some(&cookie),
            None,
            true,
            None,
            json!({"limit_micro":80})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn concurrent_creation_obeys_active_key_cap_without_deleting_legacy_keys() {
    let f = Fixture::new();
    let (s, v, _) = f
        .admin(
            "POST",
            "/api/managed-users",
            json!({"username":"manykeys","display_name":"Many","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let user = v["user"]["id"].as_i64().unwrap();
    let (_, cookie) = f.login("manykeys", PASSWORD).await;
    for i in 0..31 {
        db::identity::create_key(
            &f.state.conn(),
            user,
            &format!("existing-{i}"),
            &Default::default(),
            None,
            None,
        )
        .unwrap();
    }
    let futures = (0..4).map(|i| {
        let f = f.clone();
        let cookie = cookie.clone();
        async move {
            f.req(
                "POST",
                "/api/account/keys",
                Some(&cookie),
                None,
                true,
                None,
                json!({"name":format!("parallel-{i}")}),
            )
            .await
            .0
        }
    });
    let results = futures_util::future::join_all(futures).await;
    assert_eq!(results.iter().filter(|s| **s == StatusCode::OK).count(), 1);
    assert_eq!(db::identity::keys(&f.state.conn(), user).unwrap().len(), 32);
    // Imported accounts may have more than the current cap; metadata reads preserve all of them.
    db::identity::create_key(
        &f.state.conn(),
        user,
        "legacy-extra",
        &Default::default(),
        None,
        None,
    )
    .unwrap();
    let (s, v, _) = f
        .req(
            "GET",
            "/api/account/keys",
            Some(&cookie),
            None,
            false,
            None,
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["keys"].as_array().unwrap().len(), 33);
    assert_eq!(
        f.req(
            "POST",
            "/api/account/keys",
            Some(&cookie),
            None,
            true,
            None,
            json!({"name":"blocked"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn credential_updates_revoke_sessions_keep_identity_and_legacy_duplicate_links() {
    let f = Fixture::new();
    let (s, v, _) = f
        .admin(
            "POST",
            "/api/managed-users",
            json!({"username":"rename","display_name":"Before","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let user = v["user"]["id"].as_i64().unwrap();
    let account = v["user"]["accounts"][0]["id"].as_i64().unwrap();
    let (_, cookie) = f.login("rename", PASSWORD).await;
    let (s, v, _) = f
        .admin(
            "PATCH",
            &format!("/api/managed-users/{user}"),
            json!({"display_name":"After","password":"another-safe-password-123","limit_micro":0}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["user"]["accounts"][0]["display_name"], "After");
    assert_eq!(v["user"]["accounts"][0]["id"], account);
    assert_eq!(v["user"]["budget"]["limit_micro"], 0);
    assert_eq!(
        f.req(
            "GET",
            "/api/account/keys",
            Some(&cookie),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.login("rename", "another-safe-password-123").await;
    let hash = db::sessions::hash_password(PASSWORD).unwrap();
    db::accounts::create(&f.state.conn(), "oldshared", "Shared", user, &hash).unwrap();
    let (s, v, _) = f.admin("GET", "/api/managed-users", json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let row = v["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == user)
        .unwrap();
    assert_eq!(row["accounts"].as_array().unwrap().len(), 2);
    assert_eq!(
        f.admin(
            "PATCH",
            &format!("/api/managed-users/{user}"),
            json!({"password":PASSWORD})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (s, v, _) = f
        .admin(
            "PATCH",
            &format!("/api/managed-users/{user}"),
            json!({"enabled":false}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        v["user"]["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["enabled"] == false)
    );
}

#[tokio::test]
async fn self_service_catalog_is_authorized_visible_and_contains_no_connection_secrets() {
    let f = Fixture::new();
    for (name, model) in [
        ("first", "allowed"),
        ("second", "foreign"),
        ("compat", "old-hidden"),
    ] {
        let (s,v,_)=f.admin("POST","/api/channels",json!({"name":name,"base_url":"http://127.0.0.1:1/v1","key":"never-expose-test-secret","models":[{"model":model}]})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    db::model_routes::set_visible(&f.state.conn(), "old-hidden", false).unwrap();
    let (s,v,_)=f.admin("POST","/api/managed-users",json!({"username":"catalog","display_name":"Catalog","password":PASSWORD,"policy":{"allowed_models":["allowed","old-hidden"]}})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, cookie) = f.login("catalog", PASSWORD).await;
    let (s, v, _) = f
        .req(
            "GET",
            "/api/account/keys",
            Some(&cookie),
            None,
            false,
            None,
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["models"], json!(["allowed"]));
    assert!(!v.to_string().contains("never-expose-test-secret"));
    assert!(!v.to_string().contains("127.0.0.1"));
    assert_eq!(
        v["policy"]["allowed_models"],
        json!(["allowed", "old-hidden"])
    );
    assert_eq!(
        f.state
            .conn()
            .query_row("SELECT count(*) FROM billing_ledger", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn busy_hashing_or_revoked_root_cannot_leave_half_created_users() {
    let f = Fixture::new();
    let permits = f
        .state
        .auth_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    // A busy hashing pool rejects without changing either identity table.
    let before = db::identity::users(&f.state.conn()).unwrap().len();
    let (s, _, _) = f
        .admin(
            "POST",
            "/api/managed-users",
            json!({"username":"blocked","display_name":"Blocked","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(db::identity::users(&f.state.conn()).unwrap().len(), before);
    drop(permits);
    db::sessions::revoke_all(&f.state.conn()).unwrap();
    assert_eq!(
        f.admin(
            "POST",
            "/api/managed-users",
            json!({"username":"blocked","display_name":"Blocked","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
