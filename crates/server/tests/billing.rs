use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tokengo_server::{app, db, AppState, Config};
use tower::ServiceExt;
struct Fixture {
    state: tokengo_server::SharedState,
    dir: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("billing-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        Self { state, dir }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
#[tokio::test]
async fn billing_http_pricing_exists_and_requires_panel_auth() {
    let f = Fixture::new();
    let response = app(f.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/pricing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let token = db::sessions::create_session(&f.state.conn(), None).unwrap();
    let response = app(f.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/pricing")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

use db::billing::{self, Price, UsageQuery};
use db::identity::{self, Policy};
fn key(c: &rusqlite::Connection, name: &str, parent: Option<i64>) -> i64 {
    identity::create_key(c, 1, name, &Policy::default(), None, parent)
        .unwrap()
        .0
        .id
}
fn price(c: &rusqlite::Connection) -> Price {
    billing::create_price(
        c,
        &Price {
            id: 0,
            channel_id: 1,
            model: "Model-A".into(),
            input_micro_per_million: 1_000_000,
            output_micro_per_million: 2_000_000,
            max_input_tokens: 100,
            max_output_tokens: 100,
        },
        "test",
    )
    .unwrap()
}
fn budget(c: &rusqlite::Connection, scope: &str, id: i64) -> billing::Budget {
    billing::budgets(c, Some(scope), Some(id))
        .unwrap()
        .remove(0)
}
fn rows(c: &rusqlite::Connection) -> Vec<billing::Ledger> {
    billing::usage(c, &UsageQuery::default()).unwrap()
}

#[test]
fn exact_cost_ceil_and_immutable_price_versions() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "key", None);
    let p = price(&c);
    let r = billing::reserve(&c, 1, k, 99, "one", &p, 10, 20).unwrap();
    let mut next = p.clone();
    next.input_micro_per_million = 3_000_000;
    let next = billing::create_price(&c, &next, "test").unwrap();
    assert_ne!(next.id, p.id);
    assert_eq!(
        billing::price(&c, "model_a", 1).unwrap().unwrap().id,
        next.id
    );
    assert!(c
        .execute(
            "UPDATE billing_prices SET input_micro_per_million=0 WHERE id=?1",
            [p.id]
        )
        .is_err());
    billing::settle(&c, r, Some(3), Some(4), "ok").unwrap();
    let row = rows(&c).remove(0);
    assert_eq!(row.price_snapshot, Some(p.clone()));
    assert_eq!(row.debit_micro, Some(11));
    assert_eq!(row.observed_cost_micro, Some(11));
    assert_eq!(budget(&c, "user", 1).spent_micro, 11);
    assert_eq!(budget(&c, "key", k).spent_micro, 11);
    let mut fractional = p;
    fractional.input_micro_per_million = 1;
    fractional.output_micro_per_million = 1;
    assert_eq!(billing::cost(&fractional, 1, 1).unwrap(), 1);
}
#[test]
fn both_budget_layers_roll_back_atomically_and_lowering_blocks_new_holds() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "key", None);
    let p = price(&c);
    billing::set_budget(&c, "user", 1, Some(100), "test").unwrap();
    billing::set_budget(&c, "key", k, Some(5), "test").unwrap();
    assert!(billing::reserve(&c, 1, k, 1, "denied", &p, 10, 0).is_err());
    assert_eq!(budget(&c, "user", 1).reserved_micro, 0);
    assert!(rows(&c).is_empty());
    billing::set_budget(&c, "key", k, Some(100), "test").unwrap();
    let r = billing::reserve(&c, 1, k, 1, "hold", &p, 50, 0).unwrap();
    billing::set_budget(&c, "user", 1, Some(20), "test").unwrap();
    assert!(billing::reserve(&c, 1, k, 1, "new", &p, 1, 0).is_err());
    billing::settle(&c, r, Some(15), Some(0), "ok").unwrap();
    assert_eq!(budget(&c, "user", 1).spent_micro, 15);
    assert_eq!(budget(&c, "user", 1).reserved_micro, 0);
    assert!(billing::reserve(&c, 1, k, 1, "too-much", &p, 6, 0).is_err());
    assert!(billing::reserve(&c, 1, k, 1, "fits", &p, 5, 0).is_ok());
}
#[test]
fn partial_missing_usage_is_estimated_but_reported_zero_is_zero() {
    for (i, o, expected, certainty) in [
        (None, None, 50, "estimated"),
        (Some(2), None, 50, "estimated"),
        (Some(0), Some(0), 0, "reported"),
    ] {
        let c = db::open_in_memory().unwrap();
        let k = key(&c, "key", None);
        let p = price(&c);
        let r = billing::reserve(&c, 1, k, 1, "hold", &p, 10, 20).unwrap();
        billing::settle(&c, r, i, o, "canceled").unwrap();
        let row = rows(&c).remove(0);
        assert_eq!(row.debit_micro, Some(expected));
        assert_eq!(row.usage_certainty, certainty);
        assert_eq!(row.input_tokens, i);
        assert_eq!(row.output_tokens, o);
    }
}
#[test]
fn idempotent_reservation_settlement_release_and_retention_are_independent() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "key", None);
    let p = price(&c);
    let request = db::requests::start(&c, "chat", "Model-A", None, false).unwrap();
    let r = billing::reserve(&c, 1, k, request, "unique", &p, 10, 0).unwrap();
    assert_eq!(
        billing::reserve(&c, 1, k, request, "unique", &p, 10, 0).unwrap(),
        r
    );
    assert_eq!(budget(&c, "user", 1).reserved_micro, 10);
    assert!(billing::reserve(&c, 1, k, request, "unique", &p, 11, 0).is_err());
    c.execute("DELETE FROM requests", []).unwrap();
    assert_eq!(rows(&c).len(), 1);
    billing::settle(&c, r, Some(3), Some(0), "ok").unwrap();
    billing::settle(&c, r, None, None, "failed").unwrap();
    billing::release(&c, r, "not_sent").unwrap();
    assert_eq!(budget(&c, "user", 1).spent_micro, 3);
    assert_eq!(rows(&c)[0].status, "ok");
    let r = billing::reserve(&c, 1, k, request, "unsent", &p, 10, 0).unwrap();
    billing::release(&c, r, "before_send_failed").unwrap();
    billing::release(&c, r, "again").unwrap();
    assert_eq!(budget(&c, "key", k).spent_micro, 3);
    assert_eq!(budget(&c, "key", k).reserved_micro, 0);
}
#[test]
fn overrun_keeps_provider_cost_visible_caps_debit_and_freezes_future_budgeted_attempts() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "key", None);
    let p = price(&c);
    billing::set_budget(&c, "user", 1, Some(1000), "test").unwrap();
    let r = billing::reserve(&c, 1, k, 1, "small", &p, 10, 0).unwrap();
    billing::settle(&c, r, Some(40), Some(0), "ok").unwrap();
    let row = rows(&c).remove(0);
    assert_eq!(row.debit_micro, Some(10));
    assert_eq!(row.observed_cost_micro, Some(40));
    assert!(row.overrun);
    assert!(budget(&c, "user", 1).frozen);
    assert!(billing::reserve(&c, 1, k, 1, "frozen", &p, 1, 0).is_err());
}
#[test]
fn restart_conservatively_charges_outstanding_once_without_diagnostic_rows() {
    let f = Fixture::new();
    let k = key(&f.state.conn(), "key", None);
    let p = price(&f.state.conn());
    let id = billing::reserve(&f.state.conn(), 1, k, 888, "crash", &p, 10, 0).unwrap();
    let other = db::open(f.dir.join("test.db")).unwrap();
    assert_eq!(billing::recover(&other).unwrap(), 1);
    assert_eq!(billing::recover(&other).unwrap(), 0);
    billing::settle(&other, id, Some(1), Some(0), "late").unwrap();
    assert_eq!(budget(&other, "user", 1).spent_micro, 10);
    assert_eq!(rows(&other)[0].status, "restart_unknown");
    assert_eq!(rows(&other)[0].usage_certainty, "estimated");
}
#[test]
fn two_connections_cannot_overreserve_the_same_budget() {
    let f = Fixture::new();
    let k = key(&f.state.conn(), "key", None);
    let p = price(&f.state.conn());
    billing::set_budget(&f.state.conn(), "user", 1, Some(10), "test").unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let path = f.dir.join("test.db");
    let workers: Vec<_> = (0..2)
        .map(|n| {
            let barrier = barrier.clone();
            let path = path.clone();
            let p = p.clone();
            std::thread::spawn(move || {
                let c = db::open(path).unwrap();
                barrier.wait();
                billing::reserve(&c, 1, k, 1, &format!("race-{n}"), &p, 10, 0).is_ok()
            })
        })
        .collect();
    let count = workers
        .into_iter()
        .map(|t| usize::from(t.join().unwrap()))
        .sum::<usize>();
    assert_eq!(count, 1);
    assert_eq!(budget(&f.state.conn(), "user", 1).reserved_micro, 10);
    assert_eq!(rows(&f.state.conn()).len(), 1);
}
#[test]
fn ancestor_budget_is_shared_and_snapshot_survives_parent_deletion() {
    let c = db::open_in_memory().unwrap();
    let parent = key(&c, "parent", None);
    let child = key(&c, "child", Some(parent));
    let p = price(&c);
    billing::set_budget(&c, "key", parent, Some(10), "test").unwrap();
    let r = billing::reserve(&c, 1, child, 1, "child", &p, 10, 0).unwrap();
    assert_eq!(budget(&c, "key", parent).reserved_micro, 10);
    assert!(billing::reserve(&c, 1, parent, 1, "parent", &p, 1, 0).is_err());
    c.execute("DELETE FROM gateway_keys WHERE id=?1", [parent])
        .unwrap();
    billing::settle(&c, r, Some(3), Some(0), "ok").unwrap();
    assert_eq!(budget(&c, "key", parent).spent_micro, 3);
    assert_eq!(budget(&c, "key", child).spent_micro, 3);
}
#[test]
fn unpriced_ledger_stays_unknown_and_any_finite_ancestor_budget_requires_configuration() {
    let c = db::open_in_memory().unwrap();
    let parent = key(&c, "parent", None);
    let child = key(&c, "child", Some(parent));
    let r = billing::reserve_unpriced(&c, 1, child, 1, "unknown", 1, "Model-A").unwrap();
    billing::settle(&c, r, Some(8), Some(0), "ok").unwrap();
    let row = rows(&c).remove(0);
    assert_eq!(row.price_id, None);
    assert_eq!(row.price_snapshot, None);
    assert_eq!(row.debit_micro, None);
    assert_eq!(row.observed_cost_micro, None);
    assert_eq!(row.input_tokens, Some(8));
    assert_eq!(row.status, "unpriced");
    assert_eq!(row.outcome.as_deref(), Some("ok"));
    billing::set_budget(&c, "key", parent, Some(0), "test").unwrap();
    assert!(
        billing::reserve_unpriced(&c, 1, child, 1, "denied", 1, "Model-A")
            .unwrap_err()
            .to_string()
            .contains("config_required")
    );
    assert_eq!(rows(&c).len(), 1);
}
#[test]
fn negatives_overflows_and_price_snapshot_tampering_are_rejected() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "key", None);
    let p = price(&c);
    let mut huge = p.clone();
    huge.input_micro_per_million = i64::MAX;
    huge.max_input_tokens = i64::MAX;
    assert!(billing::create_price(&c, &huge, "test").is_err());
    assert!(billing::reserve(&c, 1, k, 1, "negative", &p, -1, 0).is_err());
    assert!(billing::set_budget(&c, "user", 1, Some(-1), "test").is_err());
    assert!(billing::reserve(&c, 2, k, 1, "wrong-user", &p, 1, 0).is_err());
    let mut fake = p.clone();
    fake.input_micro_per_million = 0;
    assert!(billing::reserve(&c, 1, k, 1, "fake", &fake, 1, 0).is_err());
    let r = billing::reserve(&c, 1, k, 1, "valid", &p, 1, 0).unwrap();
    assert!(billing::settle(&c, r, Some(-1), Some(0), "ok").is_err());
    assert_eq!(budget(&c, "user", 1).reserved_micro, 1);
    c.execute(
        "UPDATE billing_budgets SET spent_micro=?1 WHERE scope='user'",
        [i64::MAX],
    )
    .unwrap();
    assert!(billing::reserve(&c, 1, k, 1, "overflow", &p, 1, 0).is_err());
    assert_eq!(rows(&c).len(), 1);
}

async fn http(
    f: &Fixture,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}{path}", listener.local_addr().unwrap());
    let app = app(f.state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let mut req = client.request(method.parse().unwrap(), url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().await.unwrap();
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap();
    task.abort();
    (status, serde_json::from_slice(&raw).unwrap())
}
#[tokio::test]
async fn real_http_pricing_budgets_audit_and_bounded_usage() {
    let f = Fixture::new();
    let token = db::sessions::create_session(&f.state.conn(), None).unwrap();
    assert_eq!(http(&f, "GET", "/api/pricing", None, None).await.0, 401);
    let body = serde_json::json!({"channel_id":1,"model":"test","input_micro_per_million":1,"output_micro_per_million":2,"max_input_tokens":100,"max_output_tokens":100});
    let (s, p) = http(&f, "POST", "/api/pricing", Some(&token), Some(body.clone())).await;
    assert_eq!(s, 200);
    let id = p["price"]["id"].as_i64().unwrap();
    let (_, p2) = http(&f, "POST", "/api/pricing", Some(&token), Some(body)).await;
    assert!(p2["price"]["id"].as_i64().unwrap() > id);
    let (s, b) = http(
        &f,
        "PUT",
        "/api/budgets?scope=user&scope_id=1",
        Some(&token),
        Some(serde_json::json!({"limit_micro":100})),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(b["budgets"][0]["limit_micro"], 100);
    assert_eq!(
        http(
            &f,
            "PUT",
            "/api/budgets?scope=user&scope_id=1",
            Some(&token),
            Some(serde_json::json!({"limit_micro":-1}))
        )
        .await
        .0,
        400
    );
    assert_eq!(
        http(
            &f,
            "PUT",
            "/api/budgets?scope=user&scope_id=1",
            Some(&token),
            Some(serde_json::json!({}))
        )
        .await
        .0,
        400
    );
    assert_eq!(budget(&f.state.conn(), "user", 1).limit_micro, Some(100));
    assert_eq!(
        f.state
            .conn()
            .query_row("SELECT COUNT(*) FROM billing_audit", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        http(&f, "GET", "/api/usage?limit=999999", Some(&token), None)
            .await
            .0,
        200
    );
}
#[tokio::test]
async fn real_http_self_usage_cannot_read_another_key_and_paginates_without_secrets() {
    let f = Fixture::new();
    let (own, secret) =
        identity::create_key(&f.state.conn(), 1, "own", &Policy::default(), None, None).unwrap();
    let other = key(&f.state.conn(), "other", None);
    let p = price(&f.state.conn());
    for (n, k) in [own.id, other, own.id].into_iter().enumerate() {
        let id =
            billing::reserve(&f.state.conn(), 1, k, 1, &format!("http-{n}"), &p, 10, 0).unwrap();
        billing::settle(&f.state.conn(), id, Some(2), Some(0), "ok").unwrap();
    }
    assert_eq!(http(&f, "GET", "/v1/usage", None, None).await.0, 401);
    let (s, page) = http(
        &f,
        "GET",
        &format!("/v1/usage?key_id={other}&user_id=999&limit=1"),
        Some(&secret),
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["entries"][0]["key_id"], own.id);
    let before = page["next_before_id"].as_i64().unwrap();
    let (_, next) = http(
        &f,
        "GET",
        &format!("/v1/usage?before_id={before}&limit=1"),
        Some(&secret),
        None,
    )
    .await;
    assert_eq!(next["entries"][0]["key_id"], own.id);
    assert!(next["entries"][0]["id"].as_i64().unwrap() < before);
    let raw = page.to_string();
    assert!(!raw.contains(&secret));
    assert!(!raw.contains("key_hash"));
    assert!(!raw.contains("cipher"));
    let token = db::sessions::create_session(&f.state.conn(), None).unwrap();
    let (_, filtered) = http(
        &f,
        "GET",
        &format!("/api/usage?key_id={other}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(filtered["entries"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["entries"][0]["key_id"], other);
}

#[test]
fn partial_usage_can_prove_overrun_without_claiming_a_complete_provider_cost() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "key", None);
    let p = price(&c);
    billing::set_budget(&c, "key", k, Some(1000), "test").unwrap();
    let r = billing::reserve(&c, 1, k, 1, "partial-overrun", &p, 10, 0).unwrap();
    billing::settle(&c, r, Some(40), None, "failed").unwrap();
    let row = rows(&c).remove(0);
    assert!(row.overrun);
    assert_eq!(row.debit_micro, Some(10));
    assert_eq!(row.observed_cost_micro, None);
    assert_eq!(row.usage_certainty, "estimated");
    assert!(budget(&c, "key", k).frozen);
    assert!(billing::reserve(&c, 1, k, 1, "blocked", &p, 1, 0).is_err());
}

#[test]
fn unlimited_overrun_freezes_new_priced_and_zero_cost_attempts() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "key", None);
    let p = price(&c);
    let id = billing::reserve(&c, 1, k, 1, "overrun-unlimited", &p, 10, 0).unwrap();
    billing::settle(&c, id, Some(40), Some(0), "ok").unwrap();
    assert!(budget(&c, "user", 1).frozen);
    assert!(budget(&c, "key", k).frozen);
    assert_eq!(budget(&c, "user", 1).limit_micro, None);
    let mut free = p.clone();
    free.input_micro_per_million = 0;
    free.output_micro_per_million = 0;
    let free = billing::create_price(&c, &free, "test").unwrap();
    for (attempt, p) in [("new-priced", &p), ("new-free", &free)] {
        let error = billing::reserve(&c, 1, k, 1, attempt, p, 1, 0)
            .expect_err("frozen scope must reject even without a finite cap");
        assert!(error.to_string().contains("budget"));
    }
    assert_eq!(rows(&c).len(), 1);
}
#[test]
fn removing_a_finite_limit_never_implicitly_unfreezes_after_overrun() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "key", None);
    let p = price(&c);
    billing::set_budget(&c, "user", 1, Some(100), "test").unwrap();
    billing::set_budget(&c, "key", k, Some(100), "test").unwrap();
    let id = billing::reserve(&c, 1, k, 1, "overrun-finite", &p, 10, 0).unwrap();
    billing::settle(&c, id, Some(40), Some(0), "ok").unwrap();
    billing::set_budget(&c, "user", 1, None, "test").unwrap();
    billing::set_budget(&c, "key", k, None, "test").unwrap();
    assert!(budget(&c, "user", 1).frozen);
    assert!(budget(&c, "key", k).frozen);
    assert!(
        billing::reserve(&c, 1, k, 1, "null-cannot-unfreeze", &p, 1, 0)
            .unwrap_err()
            .to_string()
            .contains("budget")
    );
    assert_eq!(rows(&c).len(), 1);
}
#[test]
fn unpriced_does_not_bypass_any_frozen_user_key_or_ancestor_allocation() {
    for scope_to_freeze in ["user", "own-key", "ancestor"] {
        let c = db::open_in_memory().unwrap();
        let parent = key(&c, "parent", None);
        let child = key(&c, "child", Some(parent));
        let (scope, id) = match scope_to_freeze {
            "user" => ("user", 1),
            "own-key" => ("key", child),
            _ => ("key", parent),
        };
        // Isolate each scope so a frozen user cannot hide a missing child/parent check.
        billing::set_budget(&c, scope, id, None, "test").unwrap();
        c.execute(
            "UPDATE billing_budgets SET frozen=1 WHERE scope=?1 AND scope_id=?2",
            rusqlite::params![scope, id],
        )
        .unwrap();
        let error =
            billing::reserve_unpriced(&c, 1, child, 1, "unpriced-frozen", 1, "unpriced-model")
                .expect_err("removing a price cannot bypass a freeze");
        assert!(
            error.to_string().contains("budget"),
            "{scope_to_freeze}: {error}"
        );
        assert!(rows(&c).is_empty());
        assert_eq!(budget(&c, scope, id).reserved_micro, 0);
    }
}
#[test]
fn isolated_frozen_ancestor_blocks_child_priced_and_free_reservations() {
    let c = db::open_in_memory().unwrap();
    let parent = key(&c, "parent", None);
    let child = key(&c, "child", Some(parent));
    let p = price(&c);
    billing::set_budget(&c, "key", parent, None, "test").unwrap();
    c.execute(
        "UPDATE billing_budgets SET frozen=1 WHERE scope='key' AND scope_id=?1",
        [parent],
    )
    .unwrap();
    let mut free = p.clone();
    free.input_micro_per_million = 0;
    free.output_micro_per_million = 0;
    let free = billing::create_price(&c, &free, "test").unwrap();
    for (attempt, p) in [("parent-blocks-priced", &p), ("parent-blocks-free", &free)] {
        assert!(billing::reserve(&c, 1, child, 1, attempt, p, 1, 0)
            .unwrap_err()
            .to_string()
            .contains("budget"));
    }
    assert!(rows(&c).is_empty());
    assert_eq!(budget(&c, "key", parent).reserved_micro, 0);
}
