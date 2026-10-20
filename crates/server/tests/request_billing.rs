use tokengo_server::db::{
    self,
    billing::{self, RequestPrice, UsageQuery},
    identity::{self, Policy},
};
fn key(c: &rusqlite::Connection, name: &str, parent: Option<i64>) -> i64 {
    identity::create_key(c, 1, name, &Policy::default(), None, parent)
        .unwrap()
        .0
        .id
}
fn price(c: &rusqlite::Connection) -> RequestPrice {
    billing::create_request_price(
        c,
        &RequestPrice {
            id: 0,
            channel_id: 1,
            model: "deepseek-search".into(),
            micro_per_call: 750,
        },
        "test",
    )
    .unwrap()
}
fn row(c: &rusqlite::Connection) -> billing::Ledger {
    billing::usage(c, &UsageQuery::default()).unwrap().remove(0)
}
#[test]
fn flat_units_not_tokens_and_each_ancestor_budget() {
    let c = db::open_in_memory().unwrap();
    let parent = key(&c, "p", None);
    let child = key(&c, "c", Some(parent));
    let p = price(&c);
    for (s, i) in [("user", 1), ("key", parent), ("key", child)] {
        billing::set_budget(&c, s, i, Some(1000), "t").unwrap();
    }
    let id = billing::reserve_request(&c, 1, child, 1, "request", &p).unwrap();
    assert!(billing::reserve_request(&c, 1, child, 2, "denied", &p).is_err());
    billing::settle_request(&c, id, Some(1), "ok").unwrap();
    let r = row(&c);
    assert_eq!(r.billing_unit, "request");
    assert_eq!(r.request_units, Some(1));
    assert_eq!(r.input_tokens, None);
    assert_eq!(r.output_tokens, None);
    assert_eq!(r.debit_micro, Some(750));
    assert_eq!(r.usage_certainty, "reported");
    for (s, i) in [("user", 1), ("key", parent), ("key", child)] {
        let b = billing::budgets(&c, Some(s), Some(i)).unwrap().remove(0);
        assert_eq!(b.spent_micro, 750);
        assert_eq!(b.reserved_micro, 0);
    }
}
#[test]
fn empty_failure_knownzero_but_cancel_unknown_not_reportedzero() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "k", None);
    let p = price(&c);
    let id = billing::reserve_request(&c, 1, k, 1, "empty", &p).unwrap();
    billing::settle_request(&c, id, Some(0), "ok").unwrap();
    assert_eq!(row(&c).debit_micro, Some(0));
    let id = billing::reserve_request(&c, 1, k, 2, "canceled", &p).unwrap();
    billing::settle_request(&c, id, None, "canceled").unwrap();
    let r = row(&c);
    assert_eq!(r.request_units, None);
    assert_eq!(r.input_tokens, None);
    assert_eq!(r.debit_micro, Some(750));
    assert_eq!(r.usage_certainty, "estimated");
    assert_eq!(r.observed_cost_micro, None);
}
#[test]
fn flat_settle_idempotent_and_cannot_use_token_settlement() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "k", None);
    let p = price(&c);
    let id = billing::reserve_request(&c, 1, k, 1, "idempotent", &p).unwrap();
    assert!(billing::settle(&c, id, Some(1), Some(0), "ok").is_err());
    assert!(billing::settle_request(&c, id, Some(2), "ok").is_err());
    billing::settle_request(&c, id, Some(1), "ok").unwrap();
    billing::settle_request(&c, id, Some(0), "ok").unwrap();
    assert_eq!(row(&c).debit_micro, Some(750));
}
#[test]
fn immutable_request_price_identity_and_frozen_user() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "k", None);
    let mut p = price(&c);
    assert!(c
        .execute(
            "UPDATE billing_request_prices SET micro_per_call=0 WHERE id=?1",
            [p.id]
        )
        .is_err());
    p.micro_per_call = 0;
    assert!(billing::reserve_request(&c, 1, k, 1, "forged", &p).is_err());
    let p = price(&c);
    billing::set_budget(&c, "user", 1, None, "t").unwrap();
    c.execute(
        "UPDATE billing_budgets SET frozen=1 WHERE scope='user' AND scope_id=1",
        [],
    )
    .unwrap();
    assert!(billing::reserve_request(&c, 1, k, 2, "frozen", &p).is_err());
}
#[test]
fn recovery_preserves_unknown_flat_unit() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "k", None);
    let p = price(&c);
    billing::reserve_request(&c, 1, k, 1, "restart", &p).unwrap();
    assert_eq!(billing::recover(&c).unwrap(), 1);
    let r = row(&c);
    assert_eq!(r.request_units, None);
    assert_eq!(r.debit_micro, Some(750));
    assert_eq!(r.usage_certainty, "estimated");
}
#[test]
fn durable_request_settlement_replays_known_and_unknown_without_token_aliasing() {
    use tokengo_server::{billing_writer, AppState, Config};
    for units in [Some(0), Some(1), None] {
        let dir = std::env::temp_dir().join(format!("request-writer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        let id = {
            let c = state.conn();
            let k = key(&c, "k", None);
            let p = price(&c);
            billing::reserve_request(&c, 1, k, 1, "durable-flat", &p).unwrap()
        };
        state.conn().execute_batch("CREATE TRIGGER reject_settle BEFORE UPDATE OF status ON billing_ledger BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(billing_writer::settle_request(&state, id, units, "canceled").is_err());
        let path = dir
            .join("test.db.billing-outbox")
            .join(format!("{id}.json"));
        let intent: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(intent["kind"], "request");
        assert!(intent["input"].is_null());
        assert!(intent["output"].is_null());
        assert!(billing_writer::settle(&state, id, Some(0), Some(0), "canceled").is_err());
        state
            .conn()
            .execute_batch("DROP TRIGGER reject_settle;")
            .unwrap();
        let c = db::open(dir.join("test.db")).unwrap();
        assert_eq!(billing_writer::replay(&c, &state.cfg.db_path).unwrap(), 1);
        assert_eq!(billing::recover(&c).unwrap(), 0);
        let r = row(&c);
        assert_eq!(r.request_units, units);
        assert_eq!(r.input_tokens, None);
        assert_eq!(r.debit_micro, Some(if units == Some(0) { 0 } else { 750 }));
        assert_eq!(
            r.usage_certainty,
            if units.is_some() {
                "reported"
            } else {
                "estimated"
            }
        );
        assert_eq!(billing_writer::replay(&c, &state.cfg.db_path).unwrap(), 0);
        drop(c);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn flat_budget_arithmetic_overflow_rolls_back() {
    let c = db::open_in_memory().unwrap();
    let k = key(&c, "k", None);
    let p = billing::create_request_price(
        &c,
        &RequestPrice {
            id: 0,
            channel_id: 1,
            model: "m".into(),
            micro_per_call: i64::MAX,
        },
        "test",
    )
    .unwrap();
    billing::set_budget(&c, "user", 1, None, "t").unwrap();
    c.execute(
        "UPDATE billing_budgets SET spent_micro=1 WHERE scope='user'",
        [],
    )
    .unwrap();
    assert!(billing::reserve_request(&c, 1, k, 1, "overflow", &p).is_err());
    assert!(billing::usage(&c, &UsageQuery::default())
        .unwrap()
        .is_empty());
    assert_eq!(
        billing::budgets(&c, Some("user"), Some(1)).unwrap()[0].reserved_micro,
        0
    );
}
