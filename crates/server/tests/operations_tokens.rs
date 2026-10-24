//! Independent HTTP contract tests: isolated databases, no real upstreams or secrets.
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use serde_json::{json, Value};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tower::ServiceExt;
const PASSWORD: &str = "operations-test-password";
struct Fixture {
    state: SharedState,
    cfg: Config,
    legacy: String,
}
impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("tokengo-ops-contract-{}", uuid::Uuid::new_v4()));
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
        body: Value,
    ) -> (StatusCode, Value, HeaderMap) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json");
        if path == "/api/account/login" {
            b = b
                .header("x-tokengo-panel", "1")
                .header("origin", "http://localhost");
        }
        if let Some(c) = cookie {
            b = b
                .header("cookie", c)
                .header("x-tokengo-panel", "1")
                .header("origin", "http://localhost");
        }
        if let Some(t) = bearer {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let r = app(self.state.clone())
            .oneshot(b.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let s = r.status();
        let h = r.headers().clone();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
        (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null), h)
    }
    async fn login(&self, name: &str, password: &str) -> String {
        let (s, v, h) = self
            .req(
                "POST",
                "/api/account/login",
                None,
                None,
                json!({"username":name,"password":password}),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        h["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }
    async fn mint(&self, c: &str, name: &str) -> Value {
        let (s, v, h) = self
            .req(
                "POST",
                "/api/ops-tokens",
                Some(c),
                None,
                json!({"name":name,"password":PASSWORD}),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(h["cache-control"], "no-store");
        assert!(v["secret"].as_str().unwrap().starts_with("tgo_ops_"));
        v
    }
}
#[tokio::test]
async fn browser_stepup_single_display_and_default_expiry() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let (s, v, _) = f
        .req(
            "POST",
            "/api/ops-tokens",
            None,
            Some(&f.legacy),
            json!({"name":"no","password":PASSWORD}),
        )
        .await;
    assert_eq!(
        s,
        StatusCode::UNAUTHORIZED,
        "legacy root is not browser root: {v}"
    );
    for (path, body) in [
        ("/api/ops-tokens", json!({"name":"no","password":"wrong"})),
        ("/api/ops-tokens/revoke-all", json!({"password":"wrong"})),
        ("/api/account/security-reset", json!({"password":"wrong"})),
    ] {
        let (s, v, _) = f.req("POST", path, Some(&c), None, body).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{path}: {v}");
        assert_eq!(v["error"]["type"], "step_up_failed");
        assert_eq!(
            f.req("GET", "/api/account/me", Some(&c), None, json!({}))
                .await
                .0,
            StatusCode::OK,
            "wrong confirmation must not log out a valid browser session"
        );
        let count: i64 = f
            .state
            .conn()
            .query_row("SELECT COUNT(*) FROM operations_tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    let v = f.mint(&c, "agent").await;
    let secret = v["secret"].as_str().unwrap();
    let t = &v["token"];
    let duration = t["expires_at_ms"].as_i64().unwrap() - t["created_at_ms"].as_i64().unwrap();
    assert_eq!(duration, 90 * 86400000);
    let (s, list, h) = f
        .req("GET", "/api/ops-tokens", Some(&c), None, json!({}))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["cache-control"], "no-store");
    assert_eq!(list["tokens"].as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(secret));
    assert!(!list.to_string().contains("token_hash"));
    let conn = f.state.conn();
    let stored: String = conn
        .query_row("SELECT token_hash FROM operations_tokens", [], |r| r.get(0))
        .unwrap();
    assert_ne!(stored, secret);
    assert!(!stored.contains("tgo_ops_"));
}
#[tokio::test]
async fn lifetime_bounds_active_limit_and_expired_slots() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    for days in [json!(0), json!(366), json!(-1), json!(1.5)] {
        assert_eq!(
            f.req(
                "POST",
                "/api/ops-tokens",
                Some(&c),
                None,
                json!({"name":"bad","password":PASSWORD,"expires_in_days":days})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    for name in ["".to_owned(), "x".repeat(65)] {
        assert_eq!(
            f.req(
                "POST",
                "/api/ops-tokens",
                Some(&c),
                None,
                json!({"name":name,"password":PASSWORD})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    for i in 0..20 {
        f.mint(&c, &format!("agent-{i}")).await;
    }
    let (s, _, _) = f
        .req(
            "POST",
            "/api/ops-tokens",
            Some(&c),
            None,
            json!({"name":"overflow","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    f.state
        .conn()
        .execute("UPDATE operations_tokens SET expires_at_ms=1", [])
        .unwrap();
    f.mint(&c, "reuses-expired-slot").await;
}
#[tokio::test]
async fn machine_self_check_channel_roundtrip_and_actor_audit() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let minted = f.mint(&c, "daily-agent").await;
    let token = minted["secret"].as_str().unwrap();
    let (s, me, h) = f
        .req("GET", "/api/ops/me", None, Some(token), json!({}))
        .await;
    assert_eq!(s, StatusCode::OK, "{me}");
    assert_eq!(me["permission"], "site_operations");
    assert_eq!(me["contract_version"], 1);
    assert!(!me["instance_id"].as_str().unwrap().is_empty());
    assert_eq!(h["cache-control"], "no-store");
    let (s, caps, _) = f
        .req("GET", "/api/ops/capabilities", None, Some(token), json!({}))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(caps["contract_version"], 1);
    assert!(caps["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["method"] == "POST" && a["path"] == "/api/channels"));
    let(s,ch,_)=f.req("POST","/api/channels",None,Some(token),json!({"name":"isolated-test","base_url":"https://example.invalid/v1","key":"upstream-test-secret","models":[]})).await;
    assert_eq!(s, StatusCode::OK, "{ch}");
    let id = ch["channel"]["id"]
        .as_i64()
        .or_else(|| ch["id"].as_i64())
        .unwrap();
    for path in [
        "/api/channels",
        "/api/model-routes",
        "/api/routing",
        "/api/users",
        "/api/budgets",
        "/api/pricing",
        "/api/logs",
        "/api/logs/live",
    ] {
        let (s, v, _) = f.req("GET", path, None, Some(token), json!({})).await;
        assert_eq!(s, StatusCode::OK, "{path}: {v}");
        assert!(!v.to_string().contains("upstream-test-secret"));
        assert!(!v.to_string().contains(token));
    }
    assert_eq!(
        f.req(
            "PUT",
            &format!("/api/channels/{id}/enabled"),
            None,
            Some(token),
            json!({"enabled":false})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req(
            "DELETE",
            &format!("/api/channels/{id}"),
            None,
            Some(token),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (s, routes, _) = f
        .req("GET", "/api/routing", None, Some(token), json!({}))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        f.req(
            "PUT",
            "/api/routing/config",
            None,
            Some(token),
            routes["config"].clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    let (s, u, _) = f
        .req(
            "POST",
            "/api/users",
            None,
            Some(token),
            json!({"name":"ops-test-user"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{u}");
    let uid = u["user"]["id"].as_i64().unwrap();
    let (s, b, _) = f
        .req(
            "PUT",
            &format!("/api/budgets?scope=user&scope_id={uid}"),
            None,
            Some(token),
            json!({"limit_micro":1000}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["budgets"][0]["limit_micro"], 1000);
    let (s, a, _) = f
        .req("GET", "/api/ops/audit", None, Some(token), json!({}))
        .await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert!(a["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["actor_type"] == "operations_token" && e["actor_id"] == minted["token"]["id"]));
    // Audit must identify the operations token without recording raw credentials.
    let conn = f.state.conn();
    let mut st = conn
        .prepare("SELECT actor_user_id,actor_key_id,metadata_json FROM audit_events")
        .unwrap();
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut machines = 0;
    for (user, key, text) in rows {
        assert!(!text.contains(token));
        assert!(!text.contains("upstream-test-secret"));
        let meta: Value = serde_json::from_str(&text).unwrap();
        if meta["actor_type"] == "operations_token" {
            machines += 1;
            assert_eq!(meta["actor_id"], minted["token"]["id"]);
            assert!(user.is_none() && key.is_none());
            assert!(meta.get("actor_account_id").is_none());
            assert!(meta["request_id"].as_str().is_some_and(|s| !s.is_empty()));
        }
    }
    assert!(machines > 0, "No machine audit events");
}
#[tokio::test]
async fn forbidden_actions_and_mixed_auth_fail_closed() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "bounded").await;
    let t = v["secret"].as_str().unwrap();
    for (method, path) in [
        ("GET", "/api/ops-tokens"),
        ("POST", "/api/ops-tokens"),
        ("POST", "/api/ops-tokens/revoke-all"),
        ("PUT", "/api/account/password"),
        ("PUT", "/api/account/profile"),
        ("POST", "/api/account/security-reset"),
        ("GET", "/api/settings/gateway-key"),
        ("GET", "/api/gateway-keys/1/reveal"),
        ("POST", "/api/maintenance/prune"),
        ("GET", "/api/oauth/accounts"),
        ("POST", "/api/providers/1/credentials/1/reveal"),
    ] {
        let (s, v, _) = f
            .req(method, path, None, Some(t), json!({"password":PASSWORD}))
            .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{method} {path}: {v}");
        assert_eq!(v["error"]["type"], "operations_action_forbidden");
    }
    for path in ["/api/channels", "/api/ops/me"] {
        assert_eq!(
            f.req("GET", path, Some(&c), Some(t), json!({})).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        f.req("GET", "/v1/models", None, Some(t), json!({})).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.req(
            "PUT",
            "/api/settings",
            None,
            Some(t),
            json!({"panel_password_hash":"injected"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let r = app(f.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/channels")
                .header("authorization", format!("Bearer {t}"))
                .header("authorization", format!("Bearer {}", f.legacy))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        f.req(
            "GET",
            "/api/channels",
            None,
            Some("tgo_ops_invalid.invalid"),
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.req(
            "GET",
            "/api/channels?token=forbidden",
            None,
            Some(t),
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.req(
            "GET",
            "/api/route-that-does-not-exist",
            None,
            Some(t),
            json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn revoke_expire_restart_and_security_reset() {
    let mut f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "revoked").await;
    let t = v["secret"].as_str().unwrap();
    let id = v["token"]["id"].as_i64().unwrap();
    for _ in 0..2 {
        assert_eq!(
            f.req(
                "DELETE",
                &format!("/api/ops-tokens/{id}"),
                Some(&c),
                None,
                json!({})
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    f.state = AppState::init(f.cfg.clone()).unwrap();
    let (s, v, _) = f.req("GET", "/api/ops/me", None, Some(t), json!({})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["type"], "invalid_operations_token");
    let v = f.mint(&c, "expires").await;
    let t = v["secret"].as_str().unwrap();
    f.state
        .conn()
        .execute("UPDATE operations_tokens SET expires_at_ms=1", [])
        .unwrap();
    assert_eq!(
        f.req("GET", "/api/ops/me", None, Some(t), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let v = f.mint(&c, "security-reset").await;
    let t = v["secret"].as_str().unwrap();
    assert_eq!(
        f.req(
            "POST",
            "/api/account/security-reset",
            Some(&c),
            None,
            json!({"password":PASSWORD})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req("GET", "/api/ops/me", None, Some(t), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.req("GET", "/api/account/me", Some(&c), None, json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn ordinary_root_rename_and_password_rotation_preserve_independent_token() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "stable-agent").await;
    let t = v["secret"].as_str().unwrap();
    assert_eq!(
        f.req(
            "PUT",
            "/api/account/profile",
            Some(&c),
            None,
            json!({"username":"operator","old_password":PASSWORD})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req("GET", "/api/ops/me", None, Some(t), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let c = f.login("operator", PASSWORD).await;
    assert_eq!(
        f.req(
            "PUT",
            "/api/account/password",
            Some(&c),
            None,
            json!({"old_password":PASSWORD,"new_password":"rotated-safe-password"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req("GET", "/api/ops/me", None, Some(t), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let c = f.login("operator", "rotated-safe-password").await;
    assert_eq!(
        f.req(
            "POST",
            "/api/ops-tokens/revoke-all",
            Some(&c),
            None,
            json!({"password":"rotated-safe-password"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.req("GET", "/api/ops/me", None, Some(t), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn long_token_history_does_not_hide_active_identity() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "older-active-agent").await;
    let secret = v["secret"].as_str().unwrap();
    for i in 0..110 {
        f.state.conn().execute("INSERT INTO operations_tokens(public_id,name,token_hash,created_by_account_id,created_at_ms,expires_at_ms,revoked_at_ms) SELECT ?1,'historical','not-a-real-digest',created_by_account_id,1,2,3 FROM operations_tokens WHERE id=?2", rusqlite::params![format!("history-{i}"),v["token"]["id"].as_i64().unwrap()]).unwrap();
    }
    let (s, me, _) = f
        .req("GET", "/api/ops/me", None, Some(secret), json!({}))
        .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "older active token must survive historical pagination: {me}"
    );
    assert_eq!(me["token"]["id"], v["token"]["id"]);
}

#[tokio::test]
async fn diagnostic_projection_excludes_raw_error_and_runtime_text() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "diagnostics").await;
    let t = v["secret"].as_str().unwrap();
    let id = db::requests::start(
        &f.state.conn(),
        "chat/completions",
        "model",
        Some("192.0.2.9"),
        false,
    )
    .unwrap();
    f.state
        .conn()
        .execute(
            "UPDATE requests SET status='failed',error='RAW-ERROR-SECRET-SENTINEL' WHERE id=?1",
            [id],
        )
        .unwrap();
    f.state.logs().push("RAW-RUNTIME-SECRET-SENTINEL".into());
    for path in [
        "/api/logs".to_owned(),
        format!("/api/requests/{id}"),
        "/api/logs/live".to_owned(),
    ] {
        let (s, v, _) = f.req("GET", &path, None, Some(t), json!({})).await;
        assert_eq!(s, StatusCode::OK, "{path}: {v}");
        let raw = v.to_string();
        for denied in [
            "RAW-ERROR-SECRET-SENTINEL",
            "RAW-RUNTIME-SECRET-SENTINEL",
            "192.0.2.9",
        ] {
            assert!(!raw.contains(denied), "{path} leaked {denied}");
        }
        assert!(v.get("lines").is_none());
    }
    // Root console diagnostics keep their existing contract.
    let (_, v, _) = f.req("GET", "/api/logs", Some(&c), None, json!({})).await;
    assert!(v.to_string().contains("RAW-ERROR-SECRET-SENTINEL"));
}

#[tokio::test]
async fn revoked_in_flight_machine_context_cannot_commit_late_write() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "in-flight").await;
    let id = v["token"]["id"].as_i64().unwrap();
    let state = f.state.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        tokengo_server::routes::operations::CONTEXT.scope(tokengo_server::routes::operations::OperationsContext{
            token_id:id,request_id:"late-write-test".into(),method:"PUT".into(),path:"/api/routing/config".into()
        },async move {
            ready_tx.send(()).unwrap(); resume_rx.await.unwrap();
            state.conn().execute("INSERT INTO settings(key,value) VALUES('ops_late_write_test','blocked')",[]).map_err(|e|e.to_string())
        }).await
    });
    ready_rx.await.unwrap();
    assert_eq!(
        f.req(
            "DELETE",
            &format!("/api/ops-tokens/{id}"),
            Some(&c),
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    resume_tx.send(()).unwrap();
    assert!(worker
        .await
        .unwrap()
        .unwrap_err()
        .contains("operations_token_inactive"));
    let count: i64 = f
        .state
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key='ops_late_write_test'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    // Context cleanup must not leak stale machine authority into the next root write.
    f.state
        .conn()
        .execute(
            "INSERT INTO settings(key,value) VALUES('ops_root_after_context','ok')",
            [],
        )
        .unwrap();
}

#[tokio::test]
async fn durable_audit_failure_rolls_back_business_write() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "audit-required").await;
    let t = v["secret"].as_str().unwrap();
    f.state.conn().execute_batch("CREATE TEMP TRIGGER reject_ops_business_audit BEFORE INSERT ON audit_events WHEN NEW.action='operations.insert' BEGIN SELECT RAISE(ABORT,'test audit unavailable'); END;").unwrap();
    let(s,_,_)=f.req("POST","/api/channels",None,Some(t),json!({"name":"must-rollback","base_url":"https://example.invalid/v1","key":"not-persisted-secret","models":[]})).await;
    assert!(
        s.is_server_error(),
        "audit failure must fail operation, got {s}"
    );
    let count: i64 = f
        .state
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM channels WHERE name='must-rollback'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "business record cannot outlive its failed audit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn documented_python_runs_unchanged_against_isolated_http_and_revocation() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "python-doc-agent").await;
    let t = v["secret"].as_str().unwrap().to_owned();
    let id = v["token"]["id"].as_i64().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = f.state.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app(state)).await.unwrap();
    });
    let source = include_str!("../../../docs/operations-tokens.md")
        .split("```python\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap()
        .to_owned();
    let run = |secret: String, source: String| async move {
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let mut child = std::process::Command::new("python3")
                .args(["-c", &source])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(format!("http://{address}\n\n{secret}\n").as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        })
        .await
        .unwrap()
    };
    let first = run(t.clone(), source.clone()).await;
    assert!(
        first.status.success(),
        "Python failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("只读自检完成"));
    assert!(!String::from_utf8_lossy(&first.stdout).contains(&t));
    assert!(!String::from_utf8_lossy(&first.stderr).contains(&t));
    assert_eq!(
        f.req(
            "DELETE",
            &format!("/api/ops-tokens/{id}"),
            Some(&c),
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let after = run(t.clone(), source).await;
    assert!(!after.status.success());
    assert!(String::from_utf8_lossy(&after.stderr).contains("HTTP 401"));
    assert!(!String::from_utf8_lossy(&after.stdout).contains(&t));
    assert!(!String::from_utf8_lossy(&after.stderr).contains(&t));
    server.abort();
}

#[tokio::test]
async fn request_body_credential_aliases_are_rejected_before_business_write() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "body-contract").await;
    let t = v["secret"].as_str().unwrap();
    for alias in ["token", "access_token", "ops_token", "operations_token"] {
        let mut body =
            json!({"name":"must-not-exist","base_url":"https://example.invalid/v1","models":[]});
        body[alias] = json!(t);
        let (s, v, _) = f.req("POST", "/api/channels", None, Some(t), body).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "body alias {alias}: {v}");
        assert_eq!(v["error"]["type"], "invalid_operations_token");
    }
    let count: i64 = f
        .state
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM channels WHERE name='must-not-exist'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn every_registered_action_reaches_its_real_handler_without_machine_denial() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "all-actions").await;
    let t = v["secret"].as_str().unwrap();
    let (s, caps, _) = f
        .req("GET", "/api/ops/capabilities", None, Some(t), json!({}))
        .await;
    assert_eq!(s, StatusCode::OK);
    let actions = caps["actions"].as_array().unwrap();
    assert!(actions.len() >= 90);
    let doc = include_str!("../../../docs/operations-tokens.md");
    // Tools configuration requires an existing source; other source actions use a missing ID
    // so discovery and probes never contact a live or external endpoint.
    let tools_channel = db::channels::create(
        &f.state.conn(),
        &db::channels::NewChannel {
            name: "action-tools-fixture".into(),
            base_url: "https://example.invalid/v1".into(),
            enabled: false,
            unwrap_envelope: false,
            max_concurrent: 0,
        },
    )
    .unwrap()
    .0
    .to_string();
    let mut failures = Vec::new();
    for action in actions {
        let method = action["method"].as_str().unwrap();
        let template = action["path"].as_str().unwrap();
        assert!(
            doc.contains(&format!("| {method} | `{template}` |")),
            "Action absent from documented inventory: {method} {template}"
        );
        // IDs deliberately do not exist: active probes/discovery cannot contact an upstream.
        // Empty JSON exercises handler deserialization, business validation or safe defaults.
        let path = template
            .replace(
                "{id}",
                if template.ends_with("/tools") {
                    &tools_channel
                } else {
                    "999999"
                },
            )
            .replace("{credential}", "999999")
            .replace("{model}", "ops-nonexistent-model")
            .replace("{name}", "ops-nonexistent-model");
        let (s, body, _) = f.req(method, &path, None, Some(t), json!({})).await;
        if [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::METHOD_NOT_ALLOWED,
        ]
        .contains(&s)
        {
            failures.push(format!(
                "Declared action rejected before handler: {method} {path}: {s} {body}"
            ));
        }
        if s.is_server_error() && !(path == "/readyz" && s == StatusCode::SERVICE_UNAVAILABLE) {
            failures.push(format!(
                "Registered action internal failure or SPA response: {method} {path}: {s} {body}"
            ));
        }
        if s == StatusCode::NOT_FOUND && method == "GET" {
            let (root_status, _, _) = f.req(method, &path, Some(&c), None, json!({})).await;
            assert_eq!(
                root_status,
                StatusCode::NOT_FOUND,
                "Machine 404 must be a real domain miss, not missing route: {method} {path}"
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn oauth_provider_binding_is_forbidden_without_side_effects() {
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "no-oauth").await;
    let t = v["secret"].as_str().unwrap();
    let before: i64 = f
        .state
        .conn()
        .query_row("SELECT COUNT(*) FROM oauth_channel_bindings", [], |r| {
            r.get(0)
        })
        .unwrap();
    let (s, body, _) = f
        .req(
            "POST",
            "/api/providers/999999/credentials",
            None,
            Some(t),
            json!({"label":"not-created","kind":"oauth","oauth_account_id":1}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["type"], "operations_action_forbidden");
    let after: i64 = f
        .state
        .conn()
        .query_row("SELECT COUNT(*) FROM oauth_channel_bindings", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(before, after);
    let count: i64 = f
        .state
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM channels WHERE name='not-created'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_upstream_error_body_never_appears_in_machine_probe_diagnostics() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let upstream=axum::Router::new().fallback(move || {let seen=seen.clone();async move {
        seen.fetch_add(1,Ordering::SeqCst);
        (StatusCode::UNAUTHORIZED,axum::Json(json!({"error":{"message":"MOCK-UPSTREAM-CREDENTIAL-SECRET-SENTINEL","api_key":"MOCK-KEY-SENTINEL"}})))
    }});
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let f = Fixture::new();
    let c = f.login("admin", PASSWORD).await;
    let v = f.mint(&c, "mock-probe").await;
    let t = v["secret"].as_str().unwrap();
    let(s,ch,_)=f.req("POST","/api/channels",None,Some(t),json!({"name":"mock-error-source","base_url":format!("http://{addr}/v1"),"key":"mock-fixture-key","models":[{"model":"mock-model","enabled":true}]})).await;
    assert_eq!(s, StatusCode::OK, "{ch}");
    let id = ch["id"].as_i64().unwrap();
    let (s, v, _) = f
        .req(
            "POST",
            &format!("/api/channels/{id}/probe"),
            None,
            Some(t),
            json!({}),
        )
        .await;
    assert!(s.is_success(), "probe report request failed: {s} {v}");
    assert!(
        calls.load(Ordering::SeqCst) > 0,
        "probe must actually hit controlled mock upstream"
    );
    let body = v.to_string();
    for denied in [
        "MOCK-UPSTREAM-CREDENTIAL-SECRET-SENTINEL",
        "MOCK-KEY-SENTINEL",
        "mock-fixture-key",
    ] {
        assert!(
            !body.contains(denied),
            "machine probe leaked upstream error marker"
        );
    }
    assert!(v.get("note").is_none() && v.get("error").is_none() && v.get("reply").is_none());
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_catalog_credential_echo_is_rejected_before_persist_and_on_historical_reads() {
    use std::sync::{Arc, Mutex};
    const SOURCE_SECRET: &str = "fixture-model-catalog-bearer-secret-9731";
    const HEADER_SECRET: &str = "fixture-model-catalog-header-secret-8520";
    let received = Arc::new(Mutex::new(None::<(String, String)>));
    let seen = received.clone();
    let upstream = axum::Router::new().fallback(move |headers: HeaderMap| {
        let seen = seen.clone();
        async move {
            let bearer = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .unwrap_or("")
                .to_owned();
            let custom = headers
                .get("x-fixture-catalog-credential")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            *seen.lock().unwrap() = Some((bearer.clone(), custom.clone()));
            // Valid 2xx model metadata can be malicious, even under ordinary field names.
            axum::Json(json!({"object":"list","data":[{
                "id":format!("echo-model-{bearer}"),"object":"model","owned_by":custom
            }]}))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let f = Fixture::new();
    let cookie = f.login("admin", PASSWORD).await;
    let minted = f.mint(&cookie, "catalog-echo-regression").await;
    let token = minted["secret"].as_str().unwrap();
    let (status, channel, _) = f
        .req("POST", "/api/channels", None, Some(token), json!({
            "name":"catalog-echo-source", "base_url":format!("http://{address}/v1"),
            "key":SOURCE_SECRET,"headers":[{"name":"x-fixture-catalog-credential","value":HEADER_SECRET}],
            "models":[]
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{channel}");
    let id = channel["id"].as_i64().unwrap();
    let (status, body, _) = f
        .req(
            "POST",
            &format!("/api/channels/{id}/models"),
            None,
            Some(token),
            json!({}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "credential echo must reject the catalog"
    );
    for secret in [SOURCE_SECRET, HEADER_SECRET] {
        assert!(
            !body.to_string().contains(secret),
            "refusal response disclosed credential"
        );
    }
    assert_eq!(
        received.lock().unwrap().as_ref(),
        Some(&(SOURCE_SECRET.into(), HEADER_SECRET.into())),
        "controlled mock must echo the actual credentials it received"
    );
    let count: i64 = f
        .state
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM channel_catalog WHERE channel_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "unsafe model ID/owner must never persist");
    // Simulate old data predating the write check. Success-response protection must
    // still fail closed when a historical catalog string contains a current secret.
    f.state
        .conn()
        .execute(
            "INSERT INTO channel_catalog(channel_id,model_id,owned_by) VALUES(?1,?2,?3)",
            rusqlite::params![id, format!("historical-{SOURCE_SECRET}"), HEADER_SECRET],
        )
        .unwrap();
    let (status, body, _) = f
        .req(
            "GET",
            &format!("/api/channels/{id}/catalog"),
            None,
            Some(token),
            json!({}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "historical credential echoes must fail closed"
    );
    for secret in [SOURCE_SECRET, HEADER_SECRET] {
        assert!(
            !body.to_string().contains(secret),
            "historical catalog refusal disclosed credential"
        );
    }
    server.abort();
}

#[tokio::test]
async fn browser_token_list_without_write_csrf_header_and_mutation_csrf_enforcement() {
    let f = Fixture::new();
    let cookie = f.login("admin", PASSWORD).await;
    // Match the real browser client: safe reads carry the cookie, no write-only header.
    let response = app(f.state.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/ops-tokens")
                .header("host", "localhost")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let listed: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(listed["tokens"].as_array().unwrap().is_empty());
    let response = app(f.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ops-tokens")
                .header("host", "localhost")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"name":"no-csrf","password":PASSWORD}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let count: i64 = f
        .state
        .conn()
        .query_row("SELECT COUNT(*) FROM operations_tokens", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "missing CSRF header must prevent token issuance");
    assert_eq!(
        f.req("GET", "/api/ops-tokens", None, None, json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.req("GET", "/api/ops-tokens", None, Some(&f.legacy), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[path = "support/logging_permissions.rs"]
mod logging_permissions;
