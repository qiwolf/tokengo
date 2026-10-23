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
        StatusCode::OK
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

#[tokio::test]
async fn delete_user_revokes_login_and_keys_retains_history_and_cannot_resurrect() {
    let f=Fixture::new();
    let(s,v,_)=f.admin("POST","/api/managed-users",json!({"username":"delete-me","display_name":"Delete Me","password":PASSWORD,"limit_micro":10000})).await;
    assert_eq!(s,StatusCode::OK,"{v}");let id=v["user"]["id"].as_i64().unwrap();
    let(_,cookie)=f.login("delete-me",PASSWORD).await;
    let(s,v,_)=f.req("POST","/api/account/keys",Some(&cookie),None,true,None,json!({"name":"retained-key"})).await;
    assert_eq!(s,StatusCode::OK,"{v}");let key=v["key"]["id"].as_i64().unwrap();let secret=v["secret"].as_str().unwrap().to_owned();
    assert!(f.state.authenticate_context(secret.clone()).await.is_some());
    {
        let c=f.state.conn();c.execute("INSERT INTO requests(ts_ms,endpoint,model,status,user_id,key_id) VALUES(1,'/v1/chat/completions','m','success',?1,?2)",rusqlite::params![id,key]).unwrap();
        c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,debit_micro,status,created_ms) VALUES('kept',?1,?2,1,99,'m',0,0,0,7,'success',1)",rusqlite::params![id,key]).unwrap();
    }
    assert_eq!(f.req("DELETE",&format!("/api/managed-users/{id}"),Some(&cookie),None,true,None,json!(null)).await.0,StatusCode::FORBIDDEN);
    let(s,v,_)=f.admin("DELETE",&format!("/api/managed-users/{id}"),json!(null)).await;assert_eq!(s,StatusCode::OK,"{v}");assert_eq!(v["archived"],true);
    assert!(f.state.authenticate_context(secret).await.is_none());
    assert_eq!(f.req("GET","/api/account/keys",Some(&cookie),None,false,None,json!(null)).await.0,StatusCode::UNAUTHORIZED);
    assert_eq!(f.req("POST","/api/account/login",None,None,true,Some("http://localhost"),json!({"username":"delete-me","password":PASSWORD})).await.0,StatusCode::UNAUTHORIZED);
    let(s,v,_)=f.admin("GET","/api/managed-users",json!(null)).await;assert_eq!(s,StatusCode::OK);assert!(!v["users"].as_array().unwrap().iter().any(|u|u["id"]==id));
    assert_eq!(f.admin("PATCH",&format!("/api/managed-users/{id}"),json!({"enabled":true})).await.0,StatusCode::NOT_FOUND);
    assert_eq!(f.admin("DELETE","/api/managed-users/1",json!(null)).await.0,StatusCode::FORBIDDEN);
    let c=f.state.conn();
    assert_eq!(c.query_row("SELECT name FROM users WHERE id=?1",[id],|r|r.get::<_,String>(0)).unwrap(),"Delete Me");
    assert_eq!(c.query_row("SELECT debit_micro FROM billing_ledger WHERE user_id=?1",[id],|r|r.get::<_,i64>(0)).unwrap(),7);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM requests WHERE user_id=?1",[id],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM gateway_keys WHERE id=?1 AND enabled=0 AND revoked_at IS NOT NULL",[key],|r|r.get::<_,i64>(0)).unwrap(),1);
}

#[tokio::test]
async fn delete_user_requires_csrf_and_rolls_back_unsettled_user() {
    let f=Fixture::new();let id=f.user("busy-user");
    let(_,root_cookie)=f.login("admin",PASSWORD).await;
    assert_eq!(f.req("DELETE",&format!("/api/managed-users/{id}"),Some(&root_cookie),None,false,None,json!(null)).await.0,StatusCode::FORBIDDEN);
    assert_eq!(f.req("DELETE",&format!("/api/managed-users/{id}"),Some(&root_cookie),None,true,Some("https://foreign.invalid"),json!(null)).await.0,StatusCode::FORBIDDEN);
    f.state.conn().execute("INSERT INTO requests(ts_ms,endpoint,model,status,user_id) VALUES(1,'/v1/chat/completions','m','running',?1)",[id]).unwrap();
    let(s,v,_)=f.admin("DELETE",&format!("/api/managed-users/{id}"),json!(null)).await;assert_eq!(s,StatusCode::CONFLICT,"{v}");
    assert!(db::identity::user(&f.state.conn(),id).unwrap().unwrap().enabled);
    assert!(!db::lifecycle::archived_user(&f.state.conn(),id).unwrap());
    assert_eq!(f.admin("DELETE","/api/managed-users/999999",json!(null)).await.0,StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn self_paused_key_can_resume_but_admin_disabled_expired_and_revoked_keys_cannot() {
    let f=Fixture::new();let(s,v,_)=f.admin("POST","/api/managed-users",json!({"username":"pause-owner","display_name":"Pause owner","password":PASSWORD})).await;assert_eq!(s,StatusCode::OK,"{v}");
    let user=v["user"]["id"].as_i64().unwrap();let(_,cookie)=f.login("pause-owner",PASSWORD).await;
    let(s,v,_)=f.req("POST","/api/account/keys",Some(&cookie),None,true,None,json!({"name":"pause key"})).await;assert_eq!(s,StatusCode::OK,"{v}");let id=v["key"]["id"].as_i64().unwrap();let secret=v["secret"].as_str().unwrap().to_string();let path=format!("/api/account/keys/{id}");let admin_path=format!("/api/vkeys/{id}");
    let(s,v,_)=f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":false})).await;assert_eq!(s,StatusCode::OK,"{v}");assert_eq!(v["key"]["self_paused"],true);assert_eq!(v["key"]["can_enable"],true);assert!(f.state.authenticate_context(secret.clone()).await.is_none());
    // Metadata editing a paused key must retain the owner's ability to resume it.
    assert_eq!(f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"name":"edited while paused"})).await.1["key"]["can_enable"],true);
    let(s,v,_)=f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":true})).await;assert_eq!(s,StatusCode::OK,"{v}");assert_eq!(v["key"]["enabled"],true);assert_eq!(v["key"]["self_paused"],false);assert!(f.state.authenticate_context(secret.clone()).await.is_some());
    f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":false})).await;
    // An explicit administrator pause clears even a pre-existing owner pause.
    assert_eq!(f.admin("PATCH",&admin_path,json!({"enabled":false})).await.0,StatusCode::OK);
    let(s,v,_)=f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":false})).await;assert_eq!(s,StatusCode::OK,"{v}");assert_eq!(v["key"]["can_enable"],false);assert_eq!(v["key"]["self_paused"],false);
    assert_eq!(f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":true})).await.0,StatusCode::FORBIDDEN);
    f.admin("PATCH",&admin_path,json!({"enabled":true})).await;f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":false})).await;
    f.admin("PATCH",&admin_path,json!({"expires_at":1})).await;
    assert_eq!(f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":true})).await.0,StatusCode::FORBIDDEN);
    f.admin("PATCH",&admin_path,json!({"expires_at":null,"enabled":true})).await;f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":false})).await;
    assert_eq!(f.req("DELETE",&path,Some(&cookie),None,true,None,json!({})).await.0,StatusCode::OK);
    assert_eq!(f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":true})).await.0,StatusCode::FORBIDDEN);
    assert!(db::settings::get(&f.state.conn(),&format!("self_paused_key:{id}")).unwrap().is_none());
    // User deletion removes owner-pause markers while keeping key/history IDs.
    let(_,v,_)=f.req("POST","/api/account/keys",Some(&cookie),None,true,None,json!({"name":"deleted user key"})).await;let second=v["key"]["id"].as_i64().unwrap();
    f.req("PATCH",&format!("/api/account/keys/{second}"),Some(&cookie),None,true,None,json!({"enabled":false})).await;
    assert_eq!(f.admin("DELETE",&format!("/api/managed-users/{user}"),json!(null)).await.0,StatusCode::OK);
    assert!(db::settings::get(&f.state.conn(),&format!("self_paused_key:{second}")).unwrap().is_none());
}

#[tokio::test]
async fn resuming_self_paused_key_enforces_parent_and_active_key_capacity() {
    let f=Fixture::new();let(_,v,_)=f.admin("POST","/api/managed-users",json!({"username":"pause-parent","display_name":"Pause parent","password":PASSWORD})).await;let user=v["user"]["id"].as_i64().unwrap();let(_,cookie)=f.login("pause-parent",PASSWORD).await;
    let(parent,child)={let c=f.state.conn();let(parent,_)=db::identity::create_key(&c,user,"parent",&Default::default(),None,None).unwrap();let(child,_)=db::identity::create_key(&c,user,"child",&Default::default(),None,Some(parent.id)).unwrap();(parent.id,child.id)};
    let path=format!("/api/account/keys/{child}");f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":false})).await;
    f.admin("PATCH",&format!("/api/vkeys/{parent}"),json!({"enabled":false})).await;
    assert_eq!(f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":true})).await.0,StatusCode::FORBIDDEN);
    f.admin("PATCH",&format!("/api/vkeys/{parent}"),json!({"enabled":true})).await;
    {let c=f.state.conn();for i in 0..31 {db::identity::create_key(&c,user,&format!("capacity {i}"),&Default::default(),None,None).unwrap();}}
    let(s,v,_)=f.req("GET","/api/account/keys",Some(&cookie),None,false,None,json!(null)).await;assert_eq!(s,StatusCode::OK,"{v}");assert_eq!(v["keys"].as_array().unwrap().iter().find(|k|k["id"]==child).unwrap()["can_enable"],false);
    assert_eq!(f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":true})).await.0,StatusCode::BAD_REQUEST);
    let other=db::identity::keys(&f.state.conn(),user).unwrap().into_iter().find(|k|k.id!=child&&k.id!=parent).unwrap().id;
    f.req("PATCH",&format!("/api/account/keys/{other}"),Some(&cookie),None,true,None,json!({"enabled":false})).await;
    assert_eq!(f.req("PATCH",&path,Some(&cookie),None,true,None,json!({"enabled":true})).await.0,StatusCode::OK);
}

#[tokio::test]
async fn passthrough_owner_directory_and_user_grants_are_explicit_and_scoped() {
    let f=Fixture::new();
    let route=db::passthrough::create(&f.state.conn(),&f.state.secrets,&serde_json::from_value(json!({"name":"Face","prefix":"/face/","upstream":"http://127.0.0.1:8066","enabled":true,"headers":[{"name":"api-key","value":"synthetic-hidden-header"}]})).unwrap()).unwrap();
    let(s,v,_)=f.admin("POST","/api/managed-users",json!({"username":"services","display_name":"Services","password":PASSWORD,"policy":{"allowed_services":[route.id]}})).await;
    assert_eq!(s,StatusCode::OK,"{v}");let uid=v["user"]["id"].as_i64().unwrap();
    let(_,cookie)=f.login("services",PASSWORD).await;
    for path in ["/api/account/services","/api/account/keys"] {
        let(s,v,h)=f.req("GET",path,Some(&cookie),None,false,None,json!({})).await;
        assert_eq!(s,StatusCode::OK,"{v}");assert_eq!(h["cache-control"],"no-store");
        assert_eq!(v["services"].as_array().unwrap().len(),1);assert_eq!(v["services"][0]["id"],route.id);
        assert_eq!(v["services"][0].as_object().unwrap().len(),6);
        assert!(!v.to_string().contains("synthetic-hidden-header"));assert!(v["services"][0].get("upstream").is_none());
    }
    let(s,key,_)=f.req("POST","/api/account/keys",Some(&cookie),None,true,None,json!({"name":"inherited","policy":{"allowed_services":null}})).await;
    assert_eq!(s,StatusCode::OK,"{key}");
    let kid=key["key"]["id"].as_i64().unwrap();
    let(s,v,_)=f.req("PATCH",&format!("/api/account/keys/{kid}"),Some(&cookie),None,true,None,json!({"policy":{"allowed_services":null}})).await;
    assert_eq!(s,StatusCode::OK,"{v}");
    let auth=f.state.authenticate_context(key["secret"].as_str().unwrap().to_owned()).await.unwrap();
    let runtime=db::passthrough::match_route(&f.state.conn(),&f.state.secrets,"/face/check").unwrap().unwrap();
    assert!(db::passthrough::authorize(&f.state.conn(),&runtime,&auth).unwrap());
    let(s,v,_)=f.req("PATCH",&format!("/api/account/keys/{kid}"),Some(&cookie),None,true,None,json!({"policy":{"allowed_services":[route.id]}})).await;
    assert_eq!(s,StatusCode::OK,"null key can narrow to selected owner services: {v}");
    let(s,v,_)=f.req("PATCH",&format!("/api/account/keys/{kid}"),Some(&cookie),None,true,None,json!({"policy":{"allowed_services":[]}})).await;
    assert_eq!(s,StatusCode::OK,"{v}");
    let(s,v,_)=f.req("PATCH",&format!("/api/account/keys/{kid}"),Some(&cookie),None,true,None,json!({"policy":{"allowed_services":null}})).await;
    assert_eq!(s,StatusCode::FORBIDDEN,"inherited scope cannot erase an existing key restriction: {v}");
    let(s,v,_)=f.admin("PATCH",&format!("/api/managed-users/{uid}"),json!({"policy":{"allowed_services":[]}})).await;assert_eq!(s,StatusCode::OK,"{v}");
    assert!(!db::passthrough::authorize(&f.state.conn(),&runtime,&auth).unwrap());
    let(s,v,_)=f.admin("PATCH",&format!("/api/managed-users/{uid}"),json!({"policy":{"allowed_services":[999999]}})).await;assert_eq!(s,StatusCode::BAD_REQUEST,"{v}");
    let(s,v,_)=f.admin("PATCH",&format!("/api/managed-users/{uid}"),json!({"policy":{"allowed_services":null}})).await;assert_eq!(s,StatusCode::OK,"{v}");
    let(s,v,_)=f.req("POST","/api/account/keys",Some(&cookie),None,true,None,json!({"name":"escalate","policy":{"allowed_services":[route.id]}})).await;
    assert_eq!(s,StatusCode::FORBIDDEN,"{v}");
    let(s,v,_)=f.req("GET","/api/account/services",None,None,false,None,json!({})).await;assert_eq!(s,StatusCode::UNAUTHORIZED,"{v}");
}

#[tokio::test]
async fn service_key_default_inherits_and_legacy_restrictions_remain_editable() {
    let f=Fixture::new();
    let a=db::passthrough::create(&f.state.conn(),&f.state.secrets,&serde_json::from_value(json!({"name":"A","prefix":"/service-a/","upstream":"http://127.0.0.1:8066","enabled":true})).unwrap()).unwrap();
    let b=db::passthrough::create(&f.state.conn(),&f.state.secrets,&serde_json::from_value(json!({"name":"B","prefix":"/service-b/","upstream":"http://127.0.0.1:8066","enabled":true})).unwrap()).unwrap();
    let(s,v,_)=f.admin("POST","/api/managed-users",json!({"username":"servicekeys","display_name":"Services","password":PASSWORD,"policy":{"allowed_services":[a.id]}})).await;
    assert_eq!(s,StatusCode::OK,"{v}");let uid=v["user"]["id"].as_i64().unwrap();let(_,cookie)=f.login("servicekeys",PASSWORD).await;
    let(s,v,_)=f.req("POST","/api/account/keys",Some(&cookie),None,true,None,json!({"name":"default"})).await;
    assert_eq!(s,StatusCode::OK,"{v}");assert!(v["key"]["policy"]["allowed_services"].is_null());
    let key_id=v["key"]["id"].as_i64().unwrap();let secret=v["secret"].as_str().unwrap().to_owned();
    // An explicit cap must survive even when the current owner scope is narrower.
    let(s,v,_)=f.admin("PATCH",&format!("/api/vkeys/{key_id}"),json!({"policy":{"allowed_services":[a.id,b.id]}})).await;assert_eq!(s,StatusCode::OK,"{v}");
    let(s,v,_)=f.req("PATCH",&format!("/api/account/keys/{key_id}"),Some(&cookie),None,true,None,json!({"policy":{"allowed_services":null}})).await;
    assert_eq!(s,StatusCode::FORBIDDEN,"must preserve explicit cap for future owner grants: {v}");
    let(s,v,_)=f.admin("PATCH",&format!("/api/managed-users/{uid}"),json!({"policy":{"allowed_services":null}})).await;assert_eq!(s,StatusCode::OK,"{v}");
    for services in [json!([a.id,b.id]),json!([a.id])] {
        let(s,v,_)=f.req("PATCH",&format!("/api/account/keys/{key_id}"),Some(&cookie),None,true,None,json!({"name":"renamed","policy":{"allowed_services":services}})).await;
        assert_eq!(s,StatusCode::OK,"legacy key cap must remain editable: {v}");
    }
    let(s,v,_)=f.req("PATCH",&format!("/api/account/keys/{key_id}"),Some(&cookie),None,true,None,json!({"policy":{"allowed_services":[b.id]}})).await;assert_eq!(s,StatusCode::FORBIDDEN,"{v}");
    let(s,v,_)=f.req("POST","/v1/keys",None,Some(&secret),false,None,json!({"name":"outside","policy":{"allowed_services":[b.id]}})).await;assert_eq!(s,StatusCode::FORBIDDEN,"{v}");
    let(s,v,_)=f.req("POST","/v1/keys",None,Some(&secret),false,None,json!({"name":"subset","policy":{"allowed_services":[a.id]}})).await;assert_eq!(s,StatusCode::OK,"{v}");
    for path in [format!("/api/users/{uid}"),format!("/api/vkeys/{key_id}")] {
        let(s,v,_)=f.admin("PATCH",&path,json!({"policy":{"allowed_services":[999999]}})).await;assert_eq!(s,StatusCode::BAD_REQUEST,"{path}: {v}");
    }
    let(s,v,_)=f.admin("POST","/api/users",json!({"name":"bad-service","policy":{"allowed_services":[999999]}})).await;assert_eq!(s,StatusCode::BAD_REQUEST,"{v}");
}

async fn deletion_user(f: &Fixture, username: &str) -> (i64, String) {
    let (status, value, _) = f.admin("POST", "/api/managed-users", json!({
        "username":username,"display_name":username,"password":PASSWORD
    })).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let (_, cookie) = f.login(username, PASSWORD).await;
    (value["user"]["id"].as_i64().unwrap(), cookie)
}

#[tokio::test]
async fn deleted_revoked_key_hides_from_lists_but_preserves_ancestry_and_financial_history() {
    let f = Fixture::new();
    let (user, cookie) = deletion_user(&f, "key-delete-owner").await;
    let (parent, parent_secret, child, child_secret) = {
        let c = f.state.conn();
        let (parent, secret) = db::identity::create_key(&c, user, "retained parent", &Default::default(), None, None).unwrap();
        let (child, child_secret) = db::identity::create_key(&c, user, "retained child", &Default::default(), None, Some(parent.id)).unwrap();
        c.execute("INSERT INTO requests(ts_ms,endpoint,model,status,user_id,key_id) VALUES(1,'/v1/chat/completions','kept-model','success',?1,?2)",rusqlite::params![user,parent.id]).unwrap();
        let request = c.last_insert_rowid();
        c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,debit_micro,status,created_ms,settled_ms) VALUES('key-delete-kept',?1,?2,?3,99,'kept-model',0,0,0,7,'success',1,2)",rusqlite::params![user,parent.id,request]).unwrap();
        (parent.id, secret, child.id, child_secret)
    };
    assert!(f.state.authenticate_context(parent_secret.clone()).await.is_some());
    assert!(f.state.authenticate_context(child_secret.clone()).await.is_some());
    let path = format!("/api/account/keys/{parent}");
    assert_eq!(f.req("DELETE", &path, Some(&cookie), None, true, None, json!(null)).await.0, StatusCode::OK);
    let before = {
        let c = f.state.conn();
        let row = db::identity::key(&c, parent).unwrap().unwrap();
        (row.revoked_at, row.name, serde_json::to_value(row.policy).unwrap())
    };
    for _ in 0..2 {
        let (status, value, _) = f.req("POST", &format!("{path}/delete"), Some(&cookie), None, true, None, json!({})).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        assert_eq!(value["deleted"], true);
        assert_eq!(value["id"], parent);
        assert_eq!(value["key"]["id"], parent);
        assert!(value["key"].get("secret").is_none());
    }
    for (path, personal) in [("/api/account/keys".to_string(), true), (format!("/api/users/{user}/keys"), false)] {
        let (status, value, _) = if personal {
            f.req("GET", &path, Some(&cookie), None, false, None, json!(null)).await
        } else { f.admin("GET", &path, json!(null)).await };
        assert_eq!(status, StatusCode::OK, "{value}");
        let keys = value["keys"].as_array().unwrap();
        assert!(!keys.iter().any(|key| key["id"] == parent));
        assert!(keys.iter().any(|key| key["id"] == child), "hiding a parent must not erase its child metadata");
    }
    assert!(f.state.authenticate_context(parent_secret).await.is_none());
    assert!(f.state.authenticate_context(child_secret.clone()).await.is_none());
    assert_eq!(f.req("PATCH", &path, Some(&cookie), None, true, None, json!({"enabled":true})).await.0, StatusCode::NOT_FOUND, "deleted keys are no longer public editing targets");
    let (status, _, _) = f.admin("PATCH", &format!("/api/vkeys/{parent}"), json!({"enabled":true})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A child retaining its own enabled bit cannot bypass a revoked ancestor.
    f.admin("PATCH", &format!("/api/vkeys/{child}"), json!({"enabled":true})).await;
    assert!(f.state.authenticate_context(child_secret).await.is_none());
    let c = f.state.conn();
    assert!(db::settings::get(&c, &format!("deleted_key:{parent}")).unwrap().is_some());
    let row = db::identity::key(&c, parent).unwrap().unwrap();
    assert_eq!((row.revoked_at, row.name, serde_json::to_value(row.policy).unwrap()), before);
    assert!(!row.enabled);
    assert_eq!(db::identity::key(&c, child).unwrap().unwrap().parent_key_id, Some(parent));
    assert_eq!(c.query_row("SELECT count(*) FROM requests WHERE user_id=?1 AND key_id=?2 AND model='kept-model'",rusqlite::params![user,parent],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(c.query_row("SELECT debit_micro FROM billing_ledger WHERE attempt_id='key-delete-kept' AND key_id=?1",[parent],|r|r.get::<_,i64>(0)).unwrap(),7);
    assert!(c.query_row("SELECT count(*) FROM audit_events WHERE target_id=?1 AND action LIKE '%delete%'",[parent],|r|r.get::<_,i64>(0)).unwrap() >= 1);
}

#[tokio::test]
async fn key_delete_requires_revocation_ownership_browser_csrf_or_explicit_administrator() {
    let f = Fixture::new();
    let (user, cookie) = deletion_user(&f, "key-delete-scope").await;
    let (_, other_cookie) = deletion_user(&f, "key-delete-other").await;
    let (id, secret) = {
        let c = f.state.conn();
        let (key, secret) = db::identity::create_key(&c, user, "scoped deletion", &Default::default(), None, None).unwrap();
        (key.id, secret)
    };
    let own = format!("/api/account/keys/{id}");
    let delete = format!("{own}/delete");
    let admin = format!("/api/vkeys/{id}/delete");
    for path in [&delete, &admin] {
        let (status, _, _) = if path == &delete {
            f.req("POST", path, Some(&cookie), None, true, None, json!({})).await
        } else { f.admin("POST", path, json!({})).await };
        assert_eq!(status, StatusCode::CONFLICT, "active key cannot be deleted: {path}");
    }
    assert_eq!(f.req("PATCH", &own, Some(&cookie), None, true, None, json!({"enabled":false})).await.0, StatusCode::OK);
    for path in [&delete, &admin] {
        let (status, _, _) = if path == &delete {
            f.req("POST", path, Some(&cookie), None, true, None, json!({})).await
        } else { f.admin("POST", path, json!({})).await };
        assert_eq!(status, StatusCode::CONFLICT, "a reversible pause is not permanent revocation: {path}");
    }
    assert_eq!(f.req("DELETE", &own, Some(&cookie), None, true, None, json!({})).await.0, StatusCode::OK);
    assert_eq!(f.req("POST", &delete, Some(&other_cookie), None, true, None, json!({})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(f.req("POST", &delete, None, None, true, None, json!({})).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(f.req("POST", &delete, Some(&cookie), None, false, None, json!({})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(f.req("POST", &delete, None, Some(&secret), true, None, json!({})).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(f.req("POST", &admin, Some(&cookie), None, true, None, json!({})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(f.req("POST", &admin, None, Some(&secret), true, None, json!({})).await.0, StatusCode::UNAUTHORIZED);
    assert!(db::settings::get(&f.state.conn(), &format!("deleted_key:{id}")).unwrap().is_none(), "rejected calls must not hide the key");
    let (status, value, _) = f.admin("POST", &admin, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["deleted"], true);assert_eq!(value["id"], id);assert_eq!(value["key"]["id"], id);
    // A subsequent owner deletion is idempotent across the two management surfaces.
    assert_eq!(f.req("POST", &delete, Some(&cookie), None, true, None, json!({})).await.0, StatusCode::OK);
    for path in [format!("/api/users/{user}/keys"), "/api/vkeys/999999/delete".into()] {
        if path.ends_with("/keys") {
            assert!(!f.admin("GET", &path, json!(null)).await.1["keys"].as_array().unwrap().iter().any(|key|key["id"]==id));
        } else {assert_eq!(f.admin("POST", &path, json!({})).await.0,StatusCode::NOT_FOUND);}
    }
}

#[tokio::test]
async fn deleting_revoked_key_rolls_back_when_audit_cannot_be_persisted() {
    let f = Fixture::new();
    let (user, cookie) = deletion_user(&f, "key-delete-audit").await;
    let id = {
        let c = f.state.conn();
        let (key, _) = db::identity::create_key(&c, user, "audit retained", &Default::default(), None, None).unwrap();
        db::identity::revoke(&c, key.id).unwrap();
        c.execute_batch("CREATE TRIGGER reject_key_delete_audit BEFORE INSERT ON audit_events WHEN NEW.action LIKE '%delete%' BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;").unwrap();
        key.id
    };
    for personal in [true,false] {
        let path = if personal {format!("/api/account/keys/{id}/delete")} else {format!("/api/vkeys/{id}/delete")};
        let (status, _, _) = if personal {f.req("POST", &path, Some(&cookie), None, true, None, json!({})).await} else {f.admin("POST", &path, json!({})).await};
        assert!(!status.is_success(), "deletion must not succeed without its audit event");
        let c = f.state.conn();
        assert!(db::settings::get(&c, &format!("deleted_key:{id}")).unwrap().is_none());
        let key = db::identity::key(&c, id).unwrap().unwrap();
        assert!(key.revoked_at.is_some());assert!(!key.enabled);
    }
    let (status, value, _) = f.req("GET", "/api/account/keys", Some(&cookie), None, false, None, json!(null)).await;
    assert_eq!(status,StatusCode::OK,"{value}");
    assert!(value["keys"].as_array().unwrap().iter().any(|key|key["id"]==id));
}
