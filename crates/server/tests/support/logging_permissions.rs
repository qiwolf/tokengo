//! Preserve machine authorization and durable audit around runtime-only log changes.
//! These contract tests exercise the real handler; they do not wait for expiry timers.
use super::*;

fn log_state(f: &Fixture) -> (String, Option<i64>) {
    let control = f.state.log_control();
    (control.level(), control.revert_at_ms())
}

#[tokio::test]
async fn active_machine_log_level_change_records_the_machine_actor() {
    let f = Fixture::new();
    let cookie = f.login("admin", PASSWORD).await;
    let minted = f.mint(&cookie, "runtime-log-manager").await;
    let secret = minted["secret"].as_str().unwrap();
    let (status, body, headers) = f
        .req(
            "PUT",
            "/api/logs/level",
            None,
            Some(secret),
            json!({"level":"debug","minutes":0}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers["cache-control"], "no-store");
    assert!(body["level"].as_str().unwrap().contains("debug"));
    assert_eq!(body["revert_at_ms"], Value::Null);
    let (level, revert_at) = log_state(&f);
    assert!(level.contains("debug"));
    assert_eq!(revert_at, None);

    let c = f.state.conn();
    let mut query = c
        .prepare("SELECT actor_user_id,actor_key_id,target_type,metadata_json FROM audit_events WHERE action='operations.logs.level'")
        .unwrap();
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one runtime mutation audit");
    let (user, key, target, raw) = &rows[0];
    assert_eq!((*user, *key), (None, None));
    assert_eq!(target, "runtime");
    assert!(!raw.contains(secret), "audit must never contain the credential");
    let metadata: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(metadata["actor_type"], "operations_token");
    assert_eq!(metadata["actor_id"], minted["token"]["id"]);
    assert_eq!(metadata["success"], true);
}

#[tokio::test]
async fn revoked_machine_cannot_change_log_level_or_expiry() {
    let f = Fixture::new();
    let cookie = f.login("admin", PASSWORD).await;
    let minted = f.mint(&cookie, "revoked-log-manager").await;
    let secret = minted["secret"].as_str().unwrap();
    let id = minted["token"]["id"].as_i64().unwrap();
    let before = log_state(&f);
    assert_eq!(
        f.req(
            "DELETE",
            &format!("/api/ops-tokens/{id}"),
            Some(&cookie),
            None,
            json!({}),
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, body, _) = f
        .req(
            "PUT",
            "/api/logs/level",
            None,
            Some(secret),
            json!({"level":"debug","minutes":1}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(log_state(&f), before, "revocation must preserve runtime state");
    let audits: i64 = f
        .state
        .conn()
        .query_row("SELECT COUNT(*) FROM audit_events WHERE action='operations.logs.level'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(audits, 0, "rejected writes have no successful mutation audit");
}

#[tokio::test]
async fn failed_runtime_audit_prevents_log_level_and_expiry_mutation() {
    let f = Fixture::new();
    let cookie = f.login("admin", PASSWORD).await;
    let minted = f.mint(&cookie, "audit-required-log-manager").await;
    let secret = minted["secret"].as_str().unwrap();
    let before = log_state(&f);
    // Allow middleware's intent record, then fail the mutation-specific audit.
    // This proves the handler fails closed before touching its in-memory control.
    f.state
        .conn()
        .execute_batch("CREATE TEMP TRIGGER deny_runtime_log_audit BEFORE INSERT ON audit_events WHEN NEW.action='operations.logs.level' BEGIN SELECT RAISE(ABORT,'runtime audit unavailable'); END;")
        .unwrap();
    let (status, body, _) = f
        .req(
            "PUT",
            "/api/logs/level",
            None,
            Some(secret),
            json!({"level":"debug","minutes":1}),
        )
        .await;
    assert!(status.is_server_error(), "audit failure must reject mutation: {status} {body}");
    assert_eq!(log_state(&f), before, "failed durable audit must preserve runtime state");
    let c = f.state.conn();
    let audits: i64 = c
        .query_row("SELECT COUNT(*) FROM audit_events WHERE action='operations.logs.level'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(audits, 0);
    let intents: i64 = c
        .query_row("SELECT COUNT(*) FROM audit_events WHERE action='operations.intent'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(intents, 1, "request must reach the handler after its durable intent");
}
