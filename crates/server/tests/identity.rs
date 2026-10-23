use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use tokengo_server::{app, AppState, Config, SharedState};
use tower::ServiceExt;
struct Fixture {
    state: SharedState,
    token: String,
    cfg: Config,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-identity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("identity.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        };
        let state = AppState::init(cfg.clone()).unwrap();
        let token = tokengo_server::db::sessions::create_session(&state.conn(), None).unwrap();
        Self { state, token, cfg }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        credential: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(key) = credential {
            req = req.header("authorization", format!("Bearer {key}"));
        }
        let res = app(self.state.clone())
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    async fn admin(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        self.request(method, path, Some(&self.token), body).await
    }
}
#[tokio::test]
async fn identity_routes_exist_and_require_the_correct_credential_kind() {
    let f = Fixture::new();
    let (status, body) = f.admin("GET", "/api/users", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        f.request("GET", "/api/users", None, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request("GET", "/v1/me", None, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request("GET", "/v1/me", Some(&f.token), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

impl Fixture {
    async fn user(&self, name: &str, policy: Value) -> i64 {
        let (status, body) = self
            .admin("POST", "/api/users", json!({"name":name,"policy":policy}))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["user"]["id"].as_i64().unwrap()
    }
    async fn key(
        &self,
        user: i64,
        name: &str,
        policy: Value,
        expires_at: Option<i64>,
    ) -> (i64, String) {
        let (status, body) = self
            .admin(
                "POST",
                &format!("/api/users/{user}/keys"),
                json!({"name":name,"policy":policy,"expires_at":expires_at}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        (
            body["key"]["id"].as_i64().unwrap(),
            body["secret"].as_str().unwrap().into(),
        )
    }
}
#[tokio::test]
async fn encrypted_recoverable_secrets_owner_scope_and_child_policy_are_enforced() {
    let f = Fixture::new();
    let a = f.user("alice", json!({})).await;
    let b = f.user("bob", json!({})).await;
    let restriction = json!({"allowed_models":["logical-a"],"allowed_channels":[3],"allow_paid":false,"rpm":5,"tpm":200,"max_concurrent":2});
    let (aid, akey) = f.key(a, "alice-parent", restriction, None).await;
    let (bid, bkey) = f.key(b, "bob-key", json!({}), None).await;
    let own = f.request("GET", "/v1/keys", Some(&akey), json!({})).await;
    assert_eq!(own.0, StatusCode::OK);
    assert_eq!(own.1["keys"].as_array().unwrap().len(), 1);
    assert!(!own.1.to_string().contains(&akey));
    assert!(!own.1.to_string().contains("sha256:"));
    assert!(!own.1.to_string().contains("cipher"));
    assert_eq!(
        f.request("GET", "/api/users", Some(&akey), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request(
            "PATCH",
            &format!("/api/vkeys/{aid}"),
            Some(&akey),
            json!({"enabled":true})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request("DELETE", &format!("/v1/keys/{bid}"), Some(&akey), json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert!(f.state.authenticate_context(bkey).await.is_some());
    let attempt = f
        .request(
            "POST",
            "/v1/keys",
            Some(&akey),
            json!({"name":"escape","policy":{}}),
        )
        .await;
    assert_eq!(attempt.0, StatusCode::FORBIDDEN, "{attempt:?}");
    let (status, child) = f
        .request(
            "POST",
            "/v1/keys",
            Some(&akey),
            json!({"name":"alice-child"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{child}");
    let cid = child["key"]["id"].as_i64().unwrap();
    let secret = child["secret"].as_str().unwrap();
    let ctx = f.state.authenticate_context(secret.into()).await.unwrap();
    assert_eq!((ctx.user_id, ctx.key_id), (a, cid));
    assert_eq!(ctx.ancestor_key_ids, vec![aid]);
    assert!(ctx.permits_model("Logical_A"));
    assert!(!ctx.permits_model("other"));
    assert!(ctx.permits_channel(3, false));
    assert!(!ctx.permits_channel(3, true));
    assert!(!ctx.permits_channel(4, false));
    let raw: (String, Option<Vec<u8>>, Option<Vec<u8>>) = f
        .state
        .conn()
        .query_row(
            "SELECT key_hash,nonce,cipher FROM gateway_keys WHERE id=?1",
            [cid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!(raw.0.starts_with("sha256:"));
    assert_eq!(raw.0.len(), 71);
    assert!(!raw.0.contains(secret));
    let nonce=raw.1.as_ref().expect("production-created child stores recovery nonce");
    let cipher=raw.2.as_ref().expect("production-created child stores encrypted recovery material");
    assert!(!nonce.is_empty() && !cipher.is_empty());
    assert_eq!(f.state.secrets.open(nonce,cipher).unwrap(),secret);
    assert!(!cipher.windows(secret.len()).any(|part|part==secret.as_bytes()));
    let (reveal_status,revealed)=f.admin("GET", &format!("/api/gateway-keys/{cid}/reveal"), json!({})).await;
    assert_eq!(reveal_status,StatusCode::OK);
    assert_eq!(revealed["gateway_key"],secret);
    let audit: String = f
        .state
        .conn()
        .query_row(
            "SELECT group_concat(metadata_json,' ') FROM audit_events",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!audit.contains(secret));
    assert!(!audit.contains(&akey));
    assert!(
        f.state
            .conn()
            .query_row("SELECT count(*) FROM audit_events", [], |r| r
                .get::<_, i64>(0))
            .unwrap()
            >= 5
    );
}

#[tokio::test]
async fn cached_identity_revalidates_user_policy_role_expiry_and_parent_revocation() {
    let f = Fixture::new();
    let uid = f.user("cached", json!({})).await;
    let (pid, parent) = f.key(uid, "parent", json!({}), None).await;
    let (_, child) = f
        .request("POST", "/v1/keys", Some(&parent), json!({"name":"child"}))
        .await;
    let cid = child["key"]["id"].as_i64().unwrap();
    let secret = child["secret"].as_str().unwrap().to_string();
    assert!(f.state.authenticate_context(secret.clone()).await.is_some());
    assert_eq!(
        f.admin(
            "PATCH",
            &format!("/api/users/{uid}"),
            json!({"role":"admin","policy":{"allowed_models":["only"]}})
        )
        .await
        .0,
        StatusCode::OK
    );
    let context = f.state.authenticate_context(secret.clone()).await.unwrap();
    assert_eq!(context.role, "admin");
    assert!(!context.permits_model("else"));
    assert_eq!(
        f.admin("DELETE", &format!("/api/users/{uid}"), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    assert!(f.state.authenticate_context(secret.clone()).await.is_none());
    f.admin(
        "PATCH",
        &format!("/api/users/{uid}"),
        json!({"enabled":true}),
    )
    .await;
    assert!(f.state.authenticate_context(secret.clone()).await.is_some());
    f.admin(
        "PATCH",
        &format!("/api/vkeys/{pid}"),
        json!({"expires_at":tokengo_server::db::now_ms()-1}),
    )
    .await;
    assert!(f.state.authenticate_context(secret.clone()).await.is_none());
    f.admin(
        "PATCH",
        &format!("/api/vkeys/{pid}"),
        json!({"expires_at":null}),
    )
    .await;
    assert!(f.state.authenticate_context(secret.clone()).await.is_some());
    f.admin(
        "PATCH",
        &format!("/api/vkeys/{pid}"),
        json!({"policy":{"allowed_models":[]}}),
    )
    .await;
    assert!(!f
        .state
        .authenticate_context(secret.clone())
        .await
        .unwrap()
        .permits_model("only"));
    assert_eq!(
        f.request(
            "DELETE",
            &format!("/v1/keys/{pid}"),
            Some(&parent),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(f.state.authenticate_context(parent).await.is_none());
    assert!(f.state.authenticate_context(secret).await.is_none());
    assert!(tokengo_server::db::identity::key(&f.state.conn(), pid)
        .unwrap()
        .is_some());
    assert!(tokengo_server::db::identity::key(&f.state.conn(), cid)
        .unwrap()
        .is_some());
    assert_eq!(
        f.admin(
            "PATCH",
            &format!("/api/vkeys/{pid}"),
            json!({"enabled":true})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn child_expiry_and_limits_cannot_escape_parent_and_restart_preserves_ownership() {
    let f = Fixture::new();
    let uid = f.user("persistent", json!({})).await;
    let expires = tokengo_server::db::now_ms() + 60_000;
    let (pid, parent) = f
        .key(
            uid,
            "parent",
            json!({"rpm":8,"tpm":120,"max_concurrent":2}),
            Some(expires),
        )
        .await;
    assert_eq!(
        f.request(
            "POST",
            "/v1/keys",
            Some(&parent),
            json!({"name":"long-child","expires_at":expires+1})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request(
            "POST",
            "/v1/keys",
            Some(&parent),
            json!({"name":"fast-child","policy":{"rpm":9,"tpm":120,"max_concurrent":2}})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, child) = f
        .request(
            "POST",
            "/v1/keys",
            Some(&parent),
            json!({"name":"ok-child"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(child["key"]["expires_at"], expires);
    let id = child["key"]["id"].as_i64().unwrap();
    let key = child["secret"].as_str().unwrap().to_string();
    let restarted = AppState::init(f.cfg.clone()).unwrap();
    let ctx = restarted.authenticate_context(key).await.unwrap();
    assert_eq!((ctx.user_id, ctx.key_id), (uid, id));
    assert_eq!(ctx.ancestor_key_ids, vec![pid]);
    assert_eq!(ctx.key_policy.rpm, Some(8));
}

#[tokio::test]
async fn legacy_argon_credentials_migrate_to_admin_and_late_verification_cannot_resurrect_them() {
    use tokengo_server::db::{gateway_keys, identity, sessions};
    let f = Fixture::new();
    let legacy = "legacy-identity-test-secret";
    sessions::set_gateway_key(&f.state.conn(), &f.state.secrets, legacy).unwrap();
    let id = gateway_keys::default_id(&f.state.conn()).unwrap().unwrap();
    let context = f.state.authenticate_context(legacy.into()).await.unwrap();
    assert_eq!(context.user_id, 1);
    assert_eq!(context.role, "admin");
    assert_eq!(
        gateway_keys::reveal(&f.state.conn(), &f.state.secrets, id)
            .unwrap()
            .as_deref(),
        Some(legacy)
    );
    f.state.forget_gateway_key();
    let slots = f
        .state
        .auth_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let pending = tokio::spawn({
        let state = f.state.clone();
        async move { state.authenticate_context(legacy.into()).await }
    });
    tokio::task::yield_now().await;
    identity::revoke(&f.state.conn(), id).unwrap();
    drop(slots);
    assert!(pending.await.unwrap().is_none());
    assert!(!f.state.check_gateway_key(legacy));
    let restarted = AppState::init(f.cfg.clone()).unwrap();
    assert!(restarted
        .authenticate_context(legacy.into())
        .await
        .is_none());
}

#[test]
fn version_24_database_migrates_existing_keys_without_rekeying() {
    use tokengo_server::db::{self, gateway_keys, schema, sessions};
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT DEFAULT(datetime('now')));").unwrap();
    for (version, sql) in schema::MIGRATIONS.iter().filter(|(v, _)| *v <= 24) {
        conn.execute_batch(sql).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations(version)VALUES(?1)",
            [version],
        )
        .unwrap();
    }
    let hash = sessions::hash_password("legacy-migration-secret").unwrap();
    conn.execute("INSERT INTO gateway_keys(name,hint,created_at_ms,key_hash,is_default)VALUES('old','old hint',1,?1,1)",[&hash]).unwrap();
    let id = conn.last_insert_rowid();
    db::migrate(&conn).unwrap();
    db::migrate(&conn).unwrap();
    let ctx = db::identity::context(&conn, id).unwrap().unwrap();
    assert_eq!(
        (ctx.user_id, ctx.key_id, ctx.role.as_str()),
        (1, id, "admin")
    );
    assert!(gateway_keys::check(&conn, "legacy-migration-secret").unwrap());
    assert_eq!(
        conn.query_row("SELECT key_hash FROM gateway_keys WHERE id=?1", [id], |r| r
            .get::<_, String>(0))
            .unwrap(),
        hash
    );
}

#[tokio::test]
async fn credential_cache_is_bound_to_live_hash_and_key_status() {
    let f = Fixture::new();
    let uid = f.user("hash-binding", json!({})).await;
    let (id, old) = f.key(uid, "rotating-test", json!({}), None).await;
    assert!(f.state.authenticate_context(old.clone()).await.is_some());
    let prefix = old.split_once('.').unwrap().0;
    let replacement = format!("{prefix}.{}", uuid::Uuid::new_v4().simple());
    f.state
        .conn()
        .execute(
            "UPDATE gateway_keys SET key_hash=?2 WHERE id=?1",
            rusqlite::params![id, tokengo_server::db::gateway_keys::digest(&replacement)],
        )
        .unwrap();
    assert!(f.state.authenticate_context(old).await.is_none());
    assert!(f
        .state
        .authenticate_context(replacement.clone())
        .await
        .is_some());
    assert_eq!(
        f.admin(
            "PATCH",
            &format!("/api/vkeys/{id}"),
            json!({"enabled":false})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(f
        .state
        .authenticate_context(replacement.clone())
        .await
        .is_none());
    f.admin(
        "PATCH",
        &format!("/api/vkeys/{id}"),
        json!({"enabled":true}),
    )
    .await;
    assert!(f
        .state
        .authenticate_context(replacement.clone())
        .await
        .is_some());
    f.admin(
        "PATCH",
        &format!("/api/vkeys/{id}"),
        json!({"expires_at":tokengo_server::db::now_ms()-1}),
    )
    .await;
    assert!(f.state.authenticate_context(replacement).await.is_none());
}

#[tokio::test]
async fn identity_mutation_rolls_back_when_durable_audit_cannot_be_written() {
    let f = Fixture::new();
    let uid = f.user("audit-atomic", json!({})).await;
    let before = tokengo_server::db::identity::users(&f.state.conn())
        .unwrap()
        .len();
    f.state.conn().execute_batch("CREATE TRIGGER reject_test_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'synthetic audit write failure'); END;").unwrap();
    assert_eq!(
        f.admin("POST", "/api/users", json!({"name":"must-rollback"}))
            .await
            .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        tokengo_server::db::identity::users(&f.state.conn())
            .unwrap()
            .len(),
        before
    );
    assert_eq!(
        f.admin(
            "PATCH",
            &format!("/api/users/{uid}"),
            json!({"enabled":false})
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(
        tokengo_server::db::identity::user(&f.state.conn(), uid)
            .unwrap()
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn scoped_admin_vkeys_manage_route_b_without_access_to_legacy_secrets() {
    let f = Fixture::new();
    let (status, user) = f
        .admin(
            "POST",
            "/api/users",
            json!({"name":"scoped-admin","role":"admin","policy":{"admin_access":true}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{user}");
    let uid = user["user"]["id"].as_i64().unwrap();
    let (id, key) = f
        .key(uid, "admin-capability", json!({"admin_access":true}), None)
        .await;
    for path in ["/api/users", "/api/pricing", "/api/budgets", "/api/usage"] {
        let (status, body) = f.request("GET", path, Some(&key), json!({})).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
    // Delegated Route B administration does not expose channels, upstream or legacy key secrets.
    assert_eq!(
        f.request("GET", "/api/channels", Some(&key), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request("GET", "/api/settings/gateway-key", Some(&key), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, created) = f
        .request(
            "POST",
            "/api/users",
            Some(&key),
            json!({"name":"created-by-scoped-admin"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let target = created["user"]["id"].as_i64().unwrap();
    let(actor_user,actor_key):(i64,i64)=f.state.conn().query_row("SELECT actor_user_id,actor_key_id FROM audit_events WHERE action='user.create' AND target_id=?1 ORDER BY id DESC LIMIT 1",[target],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!((actor_user, actor_key), (uid, id));
    assert_eq!(
        f.request(
            "PUT",
            &format!("/api/budgets?scope=user&scope_id={target}"),
            Some(&key),
            json!({"limit_micro":1000})
        )
        .await
        .0,
        StatusCode::OK
    );
    let actor: String = f
        .state
        .conn()
        .query_row(
            "SELECT actor FROM billing_audit ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(actor, format!("user:{uid}/key:{id}"));
    f.admin("PATCH", &format!("/api/vkeys/{id}"), json!({"revoke":true}))
        .await;
    assert_eq!(
        f.request("GET", "/api/users", Some(&key), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn admin_role_and_both_explicit_capabilities_are_required_and_revalidated() {
    let f = Fixture::new();
    tokengo_server::db::sessions::set_gateway_key(
        &f.state.conn(),
        &f.state.secrets,
        "public-legacy-key",
    )
    .unwrap();
    assert_eq!(
        f.request("GET", "/api/users", Some("public-legacy-key"), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let ordinary = f
        .user("ordinary-with-flag", json!({"admin_access":true}))
        .await;
    let (_, ordinary_key) = f
        .key(ordinary, "not-an-admin", json!({"admin_access":true}), None)
        .await;
    assert_eq!(
        f.request("GET", "/api/users", Some(&ordinary_key), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, row) = f
        .admin(
            "POST",
            "/api/users",
            json!({"name":"admin-without-user-flag","role":"admin"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let uid = row["user"]["id"].as_i64().unwrap();
    let (id, key) = f.key(uid, "admin-without-key-flag", json!({}), None).await;
    assert_eq!(
        f.request("GET", "/api/users", Some(&key), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.admin(
        "PATCH",
        &format!("/api/users/{uid}"),
        json!({"policy":{"admin_access":true}}),
    )
    .await;
    assert_eq!(
        f.request("GET", "/api/users", Some(&key), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.admin(
        "PATCH",
        &format!("/api/vkeys/{id}"),
        json!({"policy":{"admin_access":true}}),
    )
    .await;
    assert_eq!(
        f.request("GET", "/api/users", Some(&key), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    f.admin(
        "PATCH",
        &format!("/api/users/{uid}"),
        json!({"policy":{"admin_access":false}}),
    )
    .await;
    assert_eq!(
        f.request("GET", "/api/users", Some(&key), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let response = f
        .request(
            "POST",
            "/v1/keys",
            Some(&key),
            json!({"name":"escalation-child","policy":{"admin_access":true}}),
        )
        .await;
    assert_eq!(response.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn child_key_self_service_is_limited_to_its_own_subtree() {
    let f = Fixture::new();
    let uid = f.user("subtree-owner", json!({})).await;
    let (pid, parent) = f.key(uid, "parent", json!({}), None).await;
    let (status, child) = f
        .request(
            "POST",
            "/v1/keys",
            Some(&parent),
            json!({"name":"application-child"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let cid = child["key"]["id"].as_i64().unwrap();
    let child_secret = child["secret"].as_str().unwrap();
    let (status, sibling) = f
        .request(
            "POST",
            "/v1/keys",
            Some(&parent),
            json!({"name":"sibling-service"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let sid = sibling["key"]["id"].as_i64().unwrap();
    let (status, descendant) = f
        .request(
            "POST",
            "/v1/keys",
            Some(child_secret),
            json!({"name":"child-job"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let did = descendant["key"]["id"].as_i64().unwrap();
    let (status, list) = f
        .request("GET", "/v1/keys", Some(child_secret), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<i64> = list["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["id"].as_i64().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![cid, did],
        "a delegated application key must not enumerate parent/sibling metadata"
    );
    for id in [pid, sid] {
        assert_eq!(
            f.request(
                "DELETE",
                &format!("/v1/keys/{id}"),
                Some(child_secret),
                json!({})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert!(f.state.authenticate_context(parent.clone()).await.is_some());
    assert!(f
        .state
        .authenticate_context(sibling["secret"].as_str().unwrap().into())
        .await
        .is_some());
    assert_eq!(
        f.request(
            "DELETE",
            &format!("/v1/keys/{did}"),
            Some(child_secret),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "DELETE",
            &format!("/v1/keys/{cid}"),
            Some(&parent),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.admin("GET", &format!("/api/users/{uid}/keys"), json!({}))
            .await
            .1["keys"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[tokio::test]
async fn auth_context_policy_version_is_stable_and_tracks_effective_ancestry_and_expiry() {
    let f = Fixture::new();
    let uid = f
        .user(
            "policy-version",
            json!({"allowed_models":["B","A"],"allowed_channels":[4,3]}),
        )
        .await;
    let now = tokengo_server::db::now_ms();
    let (pid, parent) = f
        .key(uid, "version-parent", json!({}), Some(now + 120_000))
        .await;
    let (status, child) = f
        .request(
            "POST",
            "/v1/keys",
            Some(&parent),
            json!({"name":"version-child","expires_at":now+90_000}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let cid = child["key"]["id"].as_i64().unwrap();
    let secret = child["secret"].as_str().unwrap().to_string();
    let initial = f.state.authenticate_context(secret.clone()).await.unwrap();
    assert_eq!(initial.expires_at, Some(now + 90_000));
    assert_eq!(initial.policy_version.len(), 64);
    assert!(initial
        .policy_version
        .bytes()
        .all(|b| b.is_ascii_hexdigit()));
    assert_eq!(
        initial.policy_version,
        f.state
            .authenticate_context(secret.clone())
            .await
            .unwrap()
            .policy_version
    );
    let restarted = AppState::init(f.cfg.clone()).unwrap();
    assert_eq!(
        initial.policy_version,
        restarted
            .authenticate_context(secret.clone())
            .await
            .unwrap()
            .policy_version
    );
    // Set order and equivalent logical-model spelling do not change effective permission sets.
    assert_eq!(
        f.admin(
            "PATCH",
            &format!("/api/users/{uid}"),
            json!({"policy":{"allowed_models":["a","b"],"allowed_channels":[3,4]}})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        initial.policy_version,
        f.state
            .authenticate_context(secret.clone())
            .await
            .unwrap()
            .policy_version
    );
    assert_eq!(
        f.admin(
            "PATCH",
            &format!("/api/vkeys/{pid}"),
            json!({"policy":{"allowed_models":["a"]}})
        )
        .await
        .0,
        StatusCode::OK
    );
    let narrowed = f.state.authenticate_context(secret.clone()).await.unwrap();
    assert_ne!(initial.policy_version, narrowed.policy_version);
    assert!(!narrowed.permits_model("b"));
    assert_eq!(
        f.admin(
            "PATCH",
            &format!("/api/vkeys/{pid}"),
            json!({"expires_at":now+30_000})
        )
        .await
        .0,
        StatusCode::OK
    );
    let shorter = f.state.authenticate_context(secret.clone()).await.unwrap();
    assert_eq!(shorter.expires_at, Some(now + 30_000));
    assert_ne!(narrowed.policy_version, shorter.policy_version);
    assert_eq!(shorter.ancestor_key_ids, vec![pid]);
    assert_eq!(shorter.key_id, cid);
    assert_eq!(
        f.admin(
            "PATCH",
            &format!("/api/vkeys/{pid}"),
            json!({"revoke":true})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(f.state.authenticate_context(secret).await.is_none());
}
