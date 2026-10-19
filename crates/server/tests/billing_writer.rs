use tokengo_server::{
    billing_writer,
    db::{
        self,
        billing::{self, Price, UsageQuery},
        identity::{self, Policy},
    },
    AppState, Config, SharedState,
};
struct Fixture {
    state: SharedState,
    dir: std::path::PathBuf,
    id: i64,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("billing-writer-{}", uuid::Uuid::new_v4()));
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
            let conn = state.conn();
            let k = identity::create_key(&conn, 1, "key", &Policy::default(), None, None)
                .unwrap()
                .0
                .id;
            let p = billing::create_price(
                &conn,
                &Price {
                    id: 0,
                    channel_id: 1,
                    model: "test".into(),
                    input_micro_per_million: 1_000_000,
                    output_micro_per_million: 1_000_000,
                    max_input_tokens: 100,
                    max_output_tokens: 100,
                },
                "test",
            )
            .unwrap();
            billing::reserve(&conn, 1, k, 777, "durable-test-attempt", &p, 50, 50).unwrap()
        };
        Self { state, dir, id }
    }
    fn outbox(&self) -> std::path::PathBuf {
        self.dir.join("test.db.billing-outbox")
    }
    fn fail(&self) {
        self.state.conn().execute_batch("CREATE TRIGGER reject_billing_settle BEFORE UPDATE OF status ON billing_ledger BEGIN SELECT RAISE(ABORT,'injected settlement failure'); END;").unwrap();
    }
    fn heal(&self) {
        self.state
            .conn()
            .execute_batch("DROP TRIGGER reject_billing_settle;")
            .unwrap();
    }
    fn row(&self) -> billing::Ledger {
        billing::usage(&self.state.conn(), &UsageQuery::default())
            .unwrap()
            .remove(0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
#[test]
fn failed_known_settlement_is_durable_and_replayed_before_unknown_recovery() {
    let f = Fixture::new();
    f.fail();
    assert!(billing_writer::settle(&f.state, f.id, Some(3), Some(4), "ok").is_err());
    assert!(
        f.outbox().join(format!("{}.json", f.id)).is_file(),
        "known usage intent must survive failed SQLite settlement"
    );
    assert_eq!(f.row().status, "reserved");
    f.heal();
    let conn = db::open(f.dir.join("test.db")).unwrap();
    assert_eq!(
        billing_writer::replay(&conn, &f.state.cfg.db_path).unwrap(),
        1
    );
    assert_eq!(billing::recover(&conn).unwrap(), 0);
    let row = f.row();
    assert_eq!(row.debit_micro, Some(7));
    assert_eq!(row.input_tokens, Some(3));
    assert_eq!(row.output_tokens, Some(4));
    assert_eq!(row.usage_certainty, "reported");
    assert!(!f.outbox().join(format!("{}.json", f.id)).exists());
    assert_eq!(
        billing_writer::replay(&conn, &f.state.cfg.db_path).unwrap(),
        0
    );
    billing_writer::settle(&f.state, f.id, Some(3), Some(4), "ok").unwrap();
    assert_eq!(f.row().debit_micro, Some(7));
}
#[test]
fn bad_json_is_quarantined_and_never_allows_automatic_unknown_recovery() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.outbox()).unwrap();
    std::fs::write(f.outbox().join("broken.json"), b"{bad").unwrap();
    assert!(billing_writer::replay(&f.state.conn(), &f.state.cfg.db_path).is_err());
    assert!(!f.outbox().join("broken.json").exists());
    assert!(std::fs::read_dir(f.outbox().join("dead-letter"))
        .unwrap()
        .next()
        .is_some());
    assert!(billing_writer::replay(&f.state.conn(), &f.state.cfg.db_path).is_err());
    assert_eq!(f.row().status, "reserved");
}
#[test]
fn different_database_spool_is_rejected_instead_of_charging_a_matching_reservation_id() {
    let first = Fixture::new();
    let second = Fixture::new();
    first.fail();
    assert!(billing_writer::settle(&first.state, first.id, Some(1), Some(2), "ok").is_err());
    std::fs::create_dir_all(second.outbox()).unwrap();
    std::fs::copy(
        first.outbox().join(format!("{}.json", first.id)),
        second.outbox().join(format!("{}.json", second.id)),
    )
    .unwrap();
    assert!(billing_writer::replay(&second.state.conn(), &second.state.cfg.db_path).is_err());
    assert_eq!(second.row().status, "reserved");
}
#[test]
fn spool_contains_only_accounting_intent_and_cannot_replace_pending_known_usage() {
    let f = Fixture::new();
    f.fail();
    assert!(billing_writer::settle(&f.state, f.id, Some(3), Some(4), "ok").is_err());
    let file = f.outbox().join(format!("{}.json", f.id));
    let raw = std::fs::read_to_string(&file).unwrap();
    assert!(!raw.contains("cipher"));
    assert!(!raw.contains("secret"));
    assert!(!raw.contains("Authorization"));
    assert!(!raw.contains("Bearer"));
    assert!(billing_writer::settle(&f.state, f.id, None, None, "failed").is_err());
    let intent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert_eq!(intent["input"], 3);
    assert_eq!(intent["output"], 4);
    f.heal();
    billing_writer::replay(&f.state.conn(), &f.state.cfg.db_path).unwrap();
    assert_eq!(f.row().debit_micro, Some(7));
}
#[tokio::test]
async fn runtime_retry_flushes_known_usage_without_process_restart() {
    let f = Fixture::new();
    f.fail();
    assert!(billing_writer::settle(&f.state, f.id, Some(3), Some(4), "ok").is_err());
    f.heal();
    let task = billing_writer::spawn_retry(std::sync::Arc::downgrade(&f.state));
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            if f.row().status != "reserved" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert_eq!(f.row().debit_micro, Some(7));
    assert_eq!(f.row().usage_certainty, "reported");
}

#[test]
fn readonly_sqlite_still_gets_a_durable_known_usage_intent() {
    let f = Fixture::new();
    billing_writer::replay(&f.state.conn(), &f.state.cfg.db_path).unwrap();
    f.state
        .conn()
        .pragma_update(None, "query_only", true)
        .unwrap();
    assert!(billing_writer::settle(&f.state, f.id, Some(3), Some(4), "ok").is_err());
    let file = f.outbox().join(format!("{}.json", f.id));
    assert!(file.is_file());
    let intent: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert!(intent["attempts"].as_u64().unwrap() >= 1);
    f.state
        .conn()
        .pragma_update(None, "query_only", false)
        .unwrap();
    billing_writer::replay(&f.state.conn(), &f.state.cfg.db_path).unwrap();
    assert_eq!(f.row().debit_micro, Some(7));
    assert_eq!(f.row().usage_certainty, "reported");
}
#[test]
fn committed_sqlite_with_leftover_intent_replay_does_not_debit_twice() {
    let f = Fixture::new();
    f.fail();
    assert!(billing_writer::settle(&f.state, f.id, Some(3), Some(4), "ok").is_err());
    f.heal();
    billing::settle(&f.state.conn(), f.id, Some(3), Some(4), "ok").unwrap();
    assert!(f.outbox().join(format!("{}.json", f.id)).is_file());
    billing_writer::replay(&f.state.conn(), &f.state.cfg.db_path).unwrap();
    assert_eq!(
        billing::budgets(&f.state.conn(), Some("user"), Some(1)).unwrap()[0].spent_micro,
        7
    );
    assert!(!f.outbox().join(format!("{}.json", f.id)).exists());
}

#[test]
fn financial_holds_and_replay_use_full_sqlite_durability_before_outbox_deletion() {
    let f = Fixture::new();
    assert_eq!(
        f.state
            .conn()
            .query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2,
        "reservation must be FULL before a provider can be called"
    );
    f.fail();
    assert!(billing_writer::settle(&f.state, f.id, Some(3), Some(4), "ok").is_err());
    f.heal();
    let connection = db::open(&f.state.cfg.db_path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    billing_writer::replay(&connection, &f.state.cfg.db_path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(f.row().debit_micro, Some(7));
}
#[test]
fn incomplete_atomic_write_is_not_silently_recovered_as_unknown_usage() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.outbox()).unwrap();
    std::fs::write(f.outbox().join(".1.interrupted.tmp"), b"{\"input\":3").unwrap();
    assert!(billing_writer::replay(&f.state.conn(), &f.state.cfg.db_path).is_err());
    assert!(!f.outbox().join(".1.interrupted.tmp").exists());
    assert_eq!(f.row().status, "reserved");
}
