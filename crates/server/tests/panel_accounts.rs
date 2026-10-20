use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use serde_json::{json, Value};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tower::ServiceExt;
const PASSWORD: &str = "legacy-password";
const NEW: &str = "new-safe-password-42";
#[derive(Clone)]
struct Fixture {
    state: SharedState,
    cfg: Config,
    legacy: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-accounts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into_owned(),
            ..Config::default()
        };
        let state = AppState::init(cfg.clone()).unwrap();
        db::sessions::set_panel_password(&state.conn(), PASSWORD).unwrap();
        let legacy = db::sessions::create_session(&state.conn(), None).unwrap();
        Self { state, cfg, legacy }
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
    async fn account(&self, name: &str, user: i64) -> i64 {
        let (s, v, _) = self
            .admin(
                "POST",
                "/api/accounts",
                json!({"username":name,"display_name":name,"password":PASSWORD,"user_id":user}),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["account"]["id"].as_i64().unwrap()
    }
}
#[tokio::test]
async fn migrated_root_cookie_restores_and_csrf_covers_management() {
    let f = Fixture::new();
    let (v, c) = f.login("admin", PASSWORD).await;
    assert_eq!(v["account"]["role"], "root");
    assert!(v["account"]["user_id"].is_null());
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req(
            "POST",
            "/api/users",
            Some(&c),
            None,
            false,
            None,
            json!({"name":"bad"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.req(
            "POST",
            "/api/users",
            Some(&c),
            None,
            true,
            Some("https://evil.test"),
            json!({"name":"bad"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.req(
            "POST",
            "/api/users",
            Some(&c),
            None,
            true,
            Some("http://localhost"),
            json!({"name":"good"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req(
            "GET",
            "/api/channels",
            Some(&c),
            Some(&f.legacy),
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.admin("GET", "/api/channels", json!({})).await.0,
        StatusCode::OK
    );
}
#[tokio::test]
async fn wrong_disabled_and_unknown_accounts_are_indistinguishable() {
    let f = Fixture::new();
    let u = f.user("same-owner");
    let id = f.account("alice", u).await;
    let (s, _, _) = f
        .admin(
            "PUT",
            &format!("/api/accounts/{id}"),
            json!({"enabled":false}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let mut results = vec![];
    for (name, pass) in [
        ("admin", "wrong"),
        ("unknown", PASSWORD),
        ("alice", PASSWORD),
    ] {
        let (s, v, _) = f
            .req(
                "POST",
                "/api/account/login",
                None,
                None,
                true,
                None,
                json!({"username":name,"password":pass}),
            )
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        results.push(v);
    }
    assert!(results.windows(2).all(|p| p[0] == p[1]));
}
#[tokio::test]
async fn ordinary_cookie_cannot_manage_and_portal_is_explicit_owner_projection() {
    let f = Fixture::new();
    let a = f.user("a");
    let b = f.user("b");
    f.account("alice", a).await;
    let (ka, secret) =
        db::identity::create_key(&f.state.conn(), a, "owned", &Default::default(), None, None)
            .unwrap();
    db::identity::create_key(
        &f.state.conn(),
        b,
        "foreign",
        &Default::default(),
        None,
        None,
    )
    .unwrap();
    let (_, c) = f.login("alice", PASSWORD).await;
    for p in [
        "/api/channels",
        "/api/users",
        "/api/accounts",
        "/api/pricing",
        "/api/usage",
    ] {
        assert_eq!(
            f.req("GET", p, Some(&c), None, false, None, json!({}))
                .await
                .0,
            StatusCode::FORBIDDEN,
            "{p}"
        );
    }
    let (s, v, _) = f
        .req(
            "GET",
            "/api/account/portal",
            Some(&c),
            None,
            false,
            None,
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["keys"].as_array().unwrap().len(), 1);
    assert_eq!(v["keys"][0]["id"], ka.id);
    let raw = v.to_string();
    for forbidden in [
        secret.as_str(),
        "foreign",
        "password_hash",
        "auth_snapshot",
        "source_snapshot",
        "price_snapshot",
        "channel_id",
    ] {
        assert!(!raw.contains(forbidden), "{forbidden}");
    }
    assert_eq!(
        f.req(
            "GET",
            "/api/account/portal?user_id=1",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn linked_user_disable_and_reenable_never_revives_old_cookie() {
    let f = Fixture::new();
    let u = f.user("a");
    f.account("alice", u).await;
    let (_, c) = f.login("alice", PASSWORD).await;
    f.state
        .conn()
        .execute("UPDATE users SET enabled=0 WHERE id=?", [u])
        .unwrap();
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.state
        .conn()
        .execute("UPDATE users SET enabled=1 WHERE id=?", [u])
        .unwrap();
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(f.login("alice", PASSWORD).await.0["account"]["id"].is_i64());
}
#[tokio::test]
async fn root_password_changes_sync_legacy_and_revoke_both_session_kinds() {
    let f = Fixture::new();
    let (_, c) = f.login("admin", PASSWORD).await;
    let (s, v, _) = f
        .req(
            "PUT",
            "/api/account/password",
            Some(&c),
            None,
            true,
            None,
            json!({"old_password":PASSWORD,"new_password":NEW}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["reauthenticate"], true);
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.admin("GET", "/api/channels", json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(db::sessions::check_panel_password(&f.state.conn(), NEW).unwrap());
    assert!(!db::sessions::check_panel_password(&f.state.conn(), PASSWORD).unwrap());
    let (_, newcookie) = f.login("admin", NEW).await;
    db::sessions::set_panel_password(&f.state.conn(), PASSWORD).unwrap();
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&newcookie),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.login("admin", PASSWORD).await;
}
#[tokio::test]
async fn root_username_change_requires_password_and_closes_legacy_password_only_backdoor() {
    let f = Fixture::new();
    let (_, c) = f.login("admin", PASSWORD).await;
    let (s, _, _) = f
        .req(
            "PUT",
            "/api/account/profile",
            Some(&c),
            None,
            true,
            None,
            json!({"username":"rootnew","old_password":"wrong"}),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = f
        .req(
            "PUT",
            "/api/account/profile",
            Some(&c),
            None,
            true,
            None,
            json!({"username":"RootNew","old_password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        f.admin("GET", "/api/channels", json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.req(
            "POST",
            "/api/auth/login",
            None,
            None,
            false,
            None,
            json!({"password":PASSWORD})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.req(
            "POST",
            "/api/auth/login",
            None,
            None,
            false,
            None,
            json!({"username":"admin","password":PASSWORD})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (s, v, _) = f
        .req(
            "POST",
            "/api/auth/login",
            None,
            None,
            false,
            None,
            json!({"username":"rootnew","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (t, _) = f.login("rootnew", PASSWORD).await;
    assert_eq!(t["account"]["username"], "rootnew");
}
#[tokio::test]
async fn account_disable_reset_and_restart_preserve_revocation_and_hash_only_sessions() {
    let f = Fixture::new();
    let u = f.user("a");
    let id = f.account("alice", u).await;
    let (_, c) = f.login("alice", PASSWORD).await;
    let raw = c.split_once('=').unwrap().1;
    let stored: String = f
        .state
        .conn()
        .query_row(
            "SELECT session_digest FROM account_sessions LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(stored, raw);
    assert!(!stored.contains(raw));
    let forged = format!("tokengo_account_session={stored}");
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&forged),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (s, _, _) = f
        .admin(
            "PUT",
            &format!("/api/accounts/{id}"),
            json!({"password":NEW}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let restarted = AppState::init(f.cfg.clone()).unwrap();
    let ff = Fixture {
        state: restarted,
        cfg: f.cfg.clone(),
        legacy: f.legacy.clone(),
    };
    assert_eq!(
        ff.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, c) = ff.login("alice", NEW).await;
    assert_eq!(
        ff.admin(
            "PUT",
            &format!("/api/accounts/{id}"),
            json!({"enabled":false})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        ff.admin(
            "PUT",
            &format!("/api/accounts/{id}"),
            json!({"enabled":true})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        ff.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn legacy_session_digest_is_not_a_bearer_and_old_raw_sessions_migrate() {
    let f = Fixture::new();
    let raw = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let expiry = db::now_ms() + 60000;
    f.state
        .conn()
        .execute(
            "INSERT INTO panel_sessions(token,created_at_ms,expires_at_ms) VALUES(?1,?2,?3)",
            rusqlite::params![raw, db::now_ms(), expiry],
        )
        .unwrap();
    let state = AppState::init(f.cfg.clone()).unwrap();
    let c = state.conn();
    let stored = db::accounts::legacy_digest(&raw);
    assert!(db::sessions::check_session(&c, &raw).unwrap());
    assert!(!db::sessions::check_session(&c, &stored).unwrap());
    let count: i64 = c
        .query_row(
            "SELECT count(*) FROM panel_sessions WHERE token=?1",
            [&stored],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    db::sessions::revoke_session(&c, &stored).unwrap();
    assert!(db::sessions::check_session(&c, &raw).unwrap());
    drop(c);
    let f = Fixture {
        state,
        cfg: f.cfg.clone(),
        legacy: stored,
    };
    assert_eq!(
        f.admin("GET", "/api/accounts", json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    db::sessions::revoke_session(&f.state.conn(), &raw).unwrap();
    assert!(!db::sessions::check_session(&f.state.conn(), &raw).unwrap());
}
#[tokio::test]
async fn root_management_cannot_reset_root_without_current_password_or_disable_it() {
    let f = Fixture::new();
    let (v, c) = f.login("admin", PASSWORD).await;
    let id = v["account"]["id"].as_i64().unwrap();
    let before = db::accounts::root(&f.state.conn()).unwrap().unwrap();
    for body in [
        json!({"password":NEW}),
        json!({"enabled":false}),
        json!({"role":"user"}),
        json!({"user_id":1}),
        json!({"username":"bypass"}),
    ] {
        let (s, _, _) = f.admin("PUT", &format!("/api/accounts/{id}"), body).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
    let after = db::accounts::root(&f.state.conn()).unwrap().unwrap();
    assert_eq!(before.hash, after.hash);
    assert_eq!(before.version, after.version);
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
}
#[tokio::test]
async fn portal_ledger_scope_and_unknown_meter_are_not_replaced_by_foreign_values() {
    let f = Fixture::new();
    let a = f.user("a");
    let b = f.user("b");
    f.account("alice", a).await;
    let (ka, _) =
        db::identity::create_key(&f.state.conn(), a, "owned", &Default::default(), None, None)
            .unwrap();
    let (kb, _) = db::identity::create_key(
        &f.state.conn(),
        b,
        "foreign",
        &Default::default(),
        None,
        None,
    )
    .unwrap();
    for (u, k, model, input, output) in [
        (a, ka.id, "owned-model", None, Some(5)),
        (b, kb.id, "foreign-model", Some(999), Some(999)),
    ] {
        f.state.conn().execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,settled_ms,status,input_tokens,output_tokens) VALUES(?1,?2,?3,?4,77,?5,100,100,0,1,2,'unpriced',?6,?7)",rusqlite::params![format!("fixture-{u}"),u,k,u,model,input,output]).unwrap();
    }
    let (_, c) = f.login("alice", PASSWORD).await;
    let (s, v, h) = f
        .req(
            "GET",
            "/api/account/portal?limit=1",
            Some(&c),
            None,
            false,
            None,
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["cache-control"], "no-store");
    assert_eq!(v["usage"]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(v["usage"]["entries"][0]["key_id"], ka.id);
    assert!(v["usage"]["entries"][0]["input_tokens"].is_null());
    assert_eq!(v["usage"]["summary"]["unknown_input_entries"], 1);
    assert_eq!(v["usage"]["summary"]["output_tokens"], 5);
    assert!(!v.to_string().contains("foreign-model"));
}
#[tokio::test]
async fn fresh_cookie_setup_is_atomic_and_echoes_gateway_key_only_once() {
    let mut f = Fixture::new();
    {
        let c = f.state.conn();
        c.execute("DELETE FROM panel_accounts", []).unwrap();
        c.execute("DELETE FROM settings WHERE key='panel_password_hash'", [])
            .unwrap();
    }
    f.legacy.clear();
    let a = f.clone();
    let b = f.clone();
    let x = tokio::spawn(async move {
        a.req(
            "POST",
            "/api/account/setup",
            None,
            None,
            true,
            Some("https://localhost"),
            json!({"password":NEW}),
        )
        .await
    });
    let y = tokio::spawn(async move {
        b.req(
            "POST",
            "/api/account/setup",
            None,
            None,
            true,
            Some("https://localhost"),
            json!({"password":NEW}),
        )
        .await
    });
    let mut replies = vec![x.await.unwrap(), y.await.unwrap()];
    replies.sort_by_key(|r| r.0.as_u16());
    assert_eq!(
        replies[0].0,
        StatusCode::OK,
        "setup errors {:?}",
        replies.iter().map(|r| r.1.get("error")).collect::<Vec<_>>()
    );
    assert_eq!(replies[1].0, StatusCode::BAD_REQUEST);
    let body = &replies[0].1;
    assert!(body.get("token").is_none());
    assert_eq!(body["account"]["username"], "admin");
    assert_eq!(body["gateway_key_generated"], true);
    let key = body["gateway_key"].as_str().unwrap();
    assert!(f.state.authenticate_context(key.into()).await.is_some());
    assert!(replies[0].2["set-cookie"]
        .to_str()
        .unwrap()
        .contains("; Secure"));
    let (login, c) = f.login("admin", NEW).await;
    assert!(login.get("gateway_key").is_none());
    let (s, _, h) = f
        .req(
            "POST",
            "/api/account/logout",
            Some(&c),
            None,
            true,
            None,
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(h["set-cookie"].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn login_throttle_and_expiry_are_bounded_without_account_enumeration() {
    let f = Fixture::new();
    for _ in 0..10 {
        assert_eq!(
            f.req(
                "POST",
                "/api/account/login",
                None,
                None,
                true,
                None,
                json!({"username":"unknown","password":"wrong"})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        f.req(
            "POST",
            "/api/account/login",
            None,
            None,
            true,
            None,
            json!({"username":"unknown","password":"wrong"})
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let (_, c) = f.login("admin", PASSWORD).await;
    f.state
        .conn()
        .execute("UPDATE account_sessions SET expires_at_ms=0", [])
        .unwrap();
    assert_eq!(
        f.req(
            "GET",
            "/api/account/me",
            Some(&c),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn root_actor_revocation_during_hash_cannot_create_or_reset_accounts() {
    for cookie_actor in [false, true] {
        let f = Fixture::new();
        let user = f.user("target");
        let target = f.account("targetlogin", user).await;
        let (_, cookie) = f.login("admin", PASSWORD).await;
        let snapshot = db::accounts::by_id(&f.state.conn(), target)
            .unwrap()
            .unwrap();
        let worker = f.clone();
        let actor_cookie = cookie.clone();
        let task = tokio::spawn(async move {
            if cookie_actor {
                worker
                    .req(
                        "PUT",
                        &format!("/api/accounts/{target}"),
                        Some(&actor_cookie),
                        None,
                        true,
                        None,
                        json!({"password":NEW}),
                    )
                    .await
            } else {
                worker.admin("POST","/api/accounts",json!({"username":"staleactor","display_name":"blocked","password":NEW,"user_id":user})).await
            }
        });
        let mut hashing = false;
        for _ in 0..1000 {
            if f.state.auth_slots.available_permits() < 2 {
                hashing = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(
            hashing,
            "request must have reached bounded hashing before revocation"
        );
        if cookie_actor {
            db::accounts::revoke(&f.state.conn(), cookie.split_once('=').unwrap().1).unwrap();
        } else {
            db::sessions::revoke_session(&f.state.conn(), &f.legacy).unwrap();
        }
        let (s, _, _) = task.await.unwrap();
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert!(db::accounts::by_username(&f.state.conn(), "staleactor")
            .unwrap()
            .is_none());
        assert_eq!(
            db::accounts::by_id(&f.state.conn(), target)
                .unwrap()
                .unwrap()
                .hash,
            snapshot.hash
        );
    }
}
#[tokio::test]
async fn migration32_copies_the_original_short_password_hash_and_legacy_session_without_reset() {
    let f = Fixture::new();
    let dir = f.cfg.db_path.parent().unwrap().join("old31");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = Config {
        db_path: dir.join("db"),
        master_key_path: dir.join("master").to_string_lossy().into_owned(),
        ..Config::default()
    };
    let hash = db::sessions::hash_password("old4").unwrap();
    let raw = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    {
        let c = rusqlite::Connection::open(&cfg.db_path).unwrap();
        c.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY)")
            .unwrap();
        for (version, sql) in db::schema::MIGRATIONS.iter().filter(|(v, _)| *v < 32) {
            c.execute_batch(sql).unwrap();
            c.execute(
                "INSERT INTO schema_migrations(version) VALUES(?1)",
                [version],
            )
            .unwrap();
        }
        db::settings::set(&c, db::settings::PANEL_PASSWORD_HASH, &hash).unwrap();
        c.execute(
            "INSERT INTO panel_sessions(token,created_at_ms,expires_at_ms) VALUES(?1,1,?2)",
            rusqlite::params![raw, db::now_ms() + 60000],
        )
        .unwrap();
    }
    let state = AppState::init(cfg.clone()).unwrap();
    let account = db::accounts::root(&state.conn()).unwrap().unwrap();
    assert_eq!(account.hash, hash);
    assert_eq!(account.account.username, "admin");
    assert_eq!(account.account.user_id, None);
    assert!(db::sessions::check_session(&state.conn(), &raw).unwrap());
    let ff = Fixture {
        state,
        cfg: cfg.clone(),
        legacy: raw,
    };
    ff.login("admin", "old4").await;
    let restart = AppState::init(cfg).unwrap();
    assert_eq!(db::accounts::accounts(&restart.conn()).unwrap().len(), 1);
    assert_eq!(
        db::accounts::root(&restart.conn()).unwrap().unwrap().hash,
        hash
    );
}
#[tokio::test]
async fn legacy_cookie_csrf_malformed_credentials_and_api_admin_never_grant_panel_root() {
    let f = Fixture::new();
    let oldcookie = format!("tokengo_session={}", f.legacy);
    assert_eq!(
        f.req(
            "GET",
            "/api/channels",
            Some(&oldcookie),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req(
            "POST",
            "/api/users",
            Some(&oldcookie),
            None,
            false,
            None,
            json!({"name":"blocked"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.req(
            "POST",
            "/api/users",
            Some(&oldcookie),
            None,
            true,
            None,
            json!({"name":"allowed"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let ambiguous_cookie = format!("tokengo_account_session; {oldcookie}");
    assert_eq!(
        f.req(
            "GET",
            "/api/channels",
            Some(&ambiguous_cookie),
            None,
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let u = f.user("apiadmin");
    let (_, key) = db::identity::create_key(
        &f.state.conn(),
        u,
        "admin-api",
        &db::identity::Policy {
            admin_access: true,
            ..Default::default()
        },
        None,
        None,
    )
    .unwrap();
    let(s,_,_)=f.req("POST","/api/accounts",None,Some(&key),false,None,json!({"username":"noroot","password":NEW,"display_name":"test","user_id":u,"role":"root"})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(
        f.req(
            "GET",
            "/api/users",
            Some("tokengo_account_session=malformed"),
            Some(&key),
            false,
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (s, v, _) = f
        .req(
            "POST",
            "/api/auth/login",
            None,
            None,
            false,
            None,
            json!({"username":"admin","password":777777777}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(!v.to_string().contains("777777777"));
}
#[tokio::test]
async fn in_flight_login_cannot_issue_session_after_root_rename_or_password_reset() {
    for legacy in [false, true] {
        let f = Fixture::new();
        let worker = f.clone();
        let task = tokio::spawn(async move {
            worker
                .req(
                    "POST",
                    if legacy {
                        "/api/auth/login"
                    } else {
                        "/api/account/login"
                    },
                    None,
                    None,
                    !legacy,
                    None,
                    json!({"username":"admin","password":PASSWORD}),
                )
                .await
        });
        let mut started = false;
        for _ in 0..1000 {
            if f.state.auth_slots.available_permits() < 2 {
                started = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(started);
        if legacy {
            f.state
                .conn()
                .execute(
                    "UPDATE panel_accounts SET username='changed' WHERE role='root'",
                    [],
                )
                .unwrap();
        } else {
            db::sessions::set_panel_password(&f.state.conn(), NEW).unwrap();
        }
        let (s, _, h) = task.await.unwrap();
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert!(h.get("set-cookie").is_none());
        let count: i64 = f
            .state
            .conn()
            .query_row("SELECT count(*) FROM account_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
#[tokio::test]
async fn session_logout_during_stepup_hash_cannot_change_password_or_profile() {
    for rename in [false, true] {
        let f = Fixture::new();
        let (_, cookie) = f.login("admin", PASSWORD).await;
        let before = db::accounts::root(&f.state.conn()).unwrap().unwrap();
        let worker = f.clone();
        let actor = cookie.clone();
        let task = tokio::spawn(async move {
            worker
                .req(
                    "PUT",
                    if rename {
                        "/api/account/profile"
                    } else {
                        "/api/account/password"
                    },
                    Some(&actor),
                    None,
                    true,
                    None,
                    if rename {
                        json!({"username":"not-applied","old_password":PASSWORD})
                    } else {
                        json!({"old_password":PASSWORD,"new_password":NEW})
                    },
                )
                .await
        });
        let mut hashing = false;
        for _ in 0..1000 {
            if f.state.auth_slots.available_permits() < 2 {
                hashing = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(hashing);
        db::accounts::revoke(&f.state.conn(), cookie.split_once('=').unwrap().1).unwrap();
        assert_eq!(task.await.unwrap().0, StatusCode::UNAUTHORIZED);
        let after = db::accounts::root(&f.state.conn()).unwrap().unwrap();
        assert_eq!(before.hash, after.hash);
        assert_eq!(before.account.username, after.account.username);
    }
}

#[tokio::test]
async fn configured_setup_rejects_before_consuming_any_hash_slot() {
    let f = Fixture::new();
    let _slots = f
        .state
        .auth_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let (s, v, _) = f
        .req(
            "POST",
            "/api/account/setup",
            None,
            None,
            true,
            None,
            json!({"password":NEW}),
        )
        .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "configured setup must fail before bounded hash worker, not429: {v}"
    );
}
