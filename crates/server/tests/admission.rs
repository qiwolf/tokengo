//! Durable admission uses live DB identity, never an old authentication snapshot.
use std::sync::Arc;
use tokengo_server::{
    admission,
    db::{
        self,
        identity::{AuthContext, Policy},
    },
    AppState, Config,
};
struct Fixture {
    state: Arc<AppState>,
    cfg: Config,
    dir: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-admission-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        };
        Self {
            state: AppState::init(cfg.clone()).unwrap(),
            cfg,
            dir,
        }
    }
    fn events(&self) -> i64 {
        self.state
            .conn()
            .query_row("SELECT COUNT(*) FROM quota_events", [], |r| r.get(0))
            .unwrap()
    }
    fn auth(&self, user_policy: Policy, key_policy: Policy) -> AuthContext {
        let c = self.state.conn();
        let user = db::identity::create_user(&c, "limiter-user", "user", &user_policy).unwrap();
        let (key, _) =
            db::identity::create_key(&c, user.id, "key", &key_policy, None, None).unwrap();
        db::identity::context(&c, key.id).unwrap().unwrap()
    }
    fn sibling(&self, a: &AuthContext) -> AuthContext {
        let c = self.state.conn();
        let (key, _) =
            db::identity::create_key(&c, a.user_id, "sibling", &a.key_policy, None, None).unwrap();
        db::identity::context(&c, key.id).unwrap().unwrap()
    }
    fn user_policy(&self, a: &AuthContext, policy: Policy) {
        let c = self.state.conn();
        let mut u = db::identity::user(&c, a.user_id).unwrap().unwrap();
        u.policy = policy;
        db::identity::update_user(&c, &u).unwrap();
    }
    fn key_policy(&self, a: &AuthContext, policy: Policy) {
        let c = self.state.conn();
        let mut k = db::identity::key(&c, a.key_id).unwrap().unwrap();
        k.policy = policy;
        db::identity::update_key(&c, &k).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn rejected(f: &Fixture, a: &AuthContext, input: u64, output: u64) {
    let e = admission::acquire(f.state.clone(), a, input, output).unwrap_err();
    assert_eq!(e.status.as_u16(), 429, "{e:?}");
    assert_eq!(e.kind, "rate_limit_exceeded");
}
#[test]
fn user_rpm_aggregates_keys_while_key_rpm_is_independent() {
    let f = Fixture::new();
    let a = f.auth(
        Policy {
            rpm: Some(2),
            ..Policy::default()
        },
        Policy {
            rpm: Some(1),
            ..Policy::default()
        },
    );
    drop(admission::acquire(f.state.clone(), &a, 0, 0).unwrap());
    rejected(&f, &a, 0, 0);
    let b = f.sibling(&a);
    drop(admission::acquire(f.state.clone(), &b, 0, 0).unwrap());
    let c = f.sibling(&a);
    rejected(&f, &c, 0, 0);
    assert_eq!(f.events(), 4);
}
#[test]
fn tpm_reserves_input_and_maximum_output_at_both_scopes() {
    let f = Fixture::new();
    let a = f.auth(
        Policy {
            tpm: Some(10),
            ..Policy::default()
        },
        Policy {
            tpm: Some(9),
            ..Policy::default()
        },
    );
    drop(admission::acquire(f.state.clone(), &a, 4, 5).unwrap());
    rejected(&f, &a, 1, 0);
    rejected(&f, &f.sibling(&a), 2, 0);
    assert_eq!(f.events(), 2);
}
#[test]
fn rejected_key_cannot_consume_another_keys_user_allowance() {
    let f = Fixture::new();
    let a = f.auth(
        Policy {
            rpm: Some(1),
            ..Policy::default()
        },
        Policy {
            tpm: Some(1),
            ..Policy::default()
        },
    );
    rejected(&f, &a, 5, 5);
    assert_eq!(f.events(), 0);
    let b = f.sibling(&a);
    drop(admission::acquire(f.state.clone(), &b, 1, 0).unwrap());
    assert_eq!(f.events(), 2);
}
#[test]
fn slots_intersect_and_drop_releases_both_scopes_without_refunding_rate_window() {
    let f = Fixture::new();
    let a = f.auth(
        Policy {
            max_concurrent: Some(1),
            ..Policy::default()
        },
        Policy {
            max_concurrent: Some(1),
            ..Policy::default()
        },
    );
    let b = f.sibling(&a);
    let p = admission::acquire(f.state.clone(), &a, 1, 1).unwrap();
    rejected(&f, &b, 1, 1);
    rejected(&f, &a, 1, 1);
    drop(p);
    drop(admission::acquire(f.state.clone(), &b, 1, 1).unwrap());
    f.user_policy(&a, Policy::default());
    let one = admission::acquire(f.state.clone(), &a, 1, 1).unwrap();
    rejected(&f, &a, 1, 1);
    let two = admission::acquire(f.state.clone(), &b, 1, 1).unwrap();
    drop((one, two));
    assert_eq!(f.events(), 8);
}
#[test]
fn window_survives_restart_and_expiration_restores_capacity() {
    let f = Fixture::new();
    let a = f.auth(
        Policy {
            rpm: Some(1),
            ..Policy::default()
        },
        Policy::default(),
    );
    drop(admission::acquire(f.state.clone(), &a, 2, 3).unwrap());
    let restarted = AppState::init(f.cfg.clone()).unwrap();
    assert!(admission::acquire(restarted.clone(), &a, 2, 3).is_err());
    restarted
        .conn()
        .execute("UPDATE quota_events SET at_ms=?1", [db::now_ms() - 59_000])
        .unwrap();
    assert!(admission::acquire(restarted.clone(), &a, 2, 3).is_err());
    restarted
        .conn()
        .execute("UPDATE quota_events SET at_ms=?1", [db::now_ms() - 61_000])
        .unwrap();
    drop(admission::acquire(restarted, &a, 2, 3).unwrap());
    assert_eq!(f.events(), 2);
}
#[test]
fn zero_limits_and_overflow_deny_without_reservations() {
    let f = Fixture::new();
    let a = f.auth(Policy::default(), Policy::default());
    // Policy APIs reject zero. Corrupt persisted zero must also fail closed;
    // caller-supplied snapshots cannot silently replace this invalid DB policy.
    for column in ["rpm", "tpm", "max_concurrent"] {
        for table in ["users", "gateway_keys"] {
            let id = if table == "users" {
                a.user_id
            } else {
                a.key_id
            };
            let raw = serde_json::json!({column:0}).to_string();
            f.state
                .conn()
                .execute(
                    &format!("UPDATE {table} SET policy_json=?1 WHERE id=?2"),
                    rusqlite::params![raw, id],
                )
                .unwrap();
            assert!(admission::acquire(f.state.clone(), &a, 0, 0).is_err());
            f.state
                .conn()
                .execute(
                    &format!("UPDATE {table} SET policy_json='{{}}' WHERE id=?1"),
                    [id],
                )
                .unwrap();
        }
    }
    rejected(&f, &a, u64::MAX, 1);
    rejected(&f, &a, i64::MAX as u64, 1);
    assert_eq!(f.events(), 0);
}
#[test]
fn simultaneous_admissions_cannot_oversubscribe_a_window() {
    let f = Fixture::new();
    let a = f.auth(
        Policy {
            rpm: Some(3),
            ..Policy::default()
        },
        Policy::default(),
    );
    let threads: Vec<_> = (0..12)
        .map(|_| {
            let a = a.clone();
            let state = f.state.clone();
            std::thread::spawn(move || admission::acquire(state, &a, 1, 1).is_ok())
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|ok| *ok)
            .count(),
        3
    );
    assert_eq!(f.events(), 6);
}
fn children(f: &Fixture, p: Policy) -> (i64, AuthContext, AuthContext) {
    let c = f.state.conn();
    let u = db::identity::create_user(&c, "parent-user", "user", &Policy::default()).unwrap();
    let (root, _) = db::identity::create_key(&c, u.id, "root", &p, None, None).unwrap();
    let (a, _) = db::identity::create_key(&c, u.id, "a", &p, None, Some(root.id)).unwrap();
    let (b, _) = db::identity::create_key(&c, u.id, "b", &p, None, Some(root.id)).unwrap();
    (
        root.id,
        db::identity::context(&c, a.id).unwrap().unwrap(),
        db::identity::context(&c, b.id).unwrap().unwrap(),
    )
}
#[test]
fn sibling_keys_share_parent_rpm_and_concurrency() {
    let f = Fixture::new();
    let (root, a, b) = children(
        &f,
        Policy {
            rpm: Some(2),
            max_concurrent: Some(1),
            ..Policy::default()
        },
    );
    let p = admission::acquire(f.state.clone(), &a, 1, 1).unwrap();
    rejected(&f, &b, 1, 1);
    drop(p);
    drop(admission::acquire(f.state.clone(), &b, 1, 1).unwrap());
    rejected(&f, &a, 1, 1);
    let n: i64 = f
        .state
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM quota_events WHERE scope='key' AND scope_id=?1",
            [root],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(f.events(), 6);
}
#[test]
fn sibling_reservations_also_charge_parent_tpm_and_survive_restart() {
    let f = Fixture::new();
    let (root, a, b) = children(
        &f,
        Policy {
            tpm: Some(5),
            ..Policy::default()
        },
    );
    drop(admission::acquire(f.state.clone(), &a, 1, 1).unwrap());
    drop(admission::acquire(f.state.clone(), &b, 1, 2).unwrap());
    let restarted = AppState::init(f.cfg.clone()).unwrap();
    let root = db::identity::context(&restarted.conn(), root)
        .unwrap()
        .unwrap();
    assert_eq!(
        admission::acquire(restarted, &root, 1, 0)
            .unwrap_err()
            .status
            .as_u16(),
        429
    );
    assert_eq!(f.events(), 6);
}
#[test]
fn parent_revocation_between_authentication_and_admission_fails_closed() {
    let f = Fixture::new();
    let (root, a, _) = children(&f, Policy::default());
    db::identity::revoke(&f.state.conn(), root).unwrap();
    assert_eq!(
        admission::acquire(f.state.clone(), &a, 0, 0)
            .unwrap_err()
            .status
            .as_u16(),
        401
    );
    assert_eq!(f.events(), 0);
}
#[test]
fn own_key_revocation_after_authentication_is_enforced() {
    let f = Fixture::new();
    let a = f.auth(Policy::default(), Policy::default());
    db::identity::revoke(&f.state.conn(), a.key_id).unwrap();
    assert_eq!(
        admission::acquire(f.state.clone(), &a, 1, 1)
            .unwrap_err()
            .status
            .as_u16(),
        401
    );
    assert_eq!(f.events(), 0);
}
#[test]
fn own_key_limits_tightened_after_authentication_are_enforced() {
    let f = Fixture::new();
    let a = f.auth(Policy::default(), Policy::default());
    f.key_policy(
        &a,
        Policy {
            tpm: Some(1),
            ..Policy::default()
        },
    );
    rejected(&f, &a, 1, 1);
    assert_eq!(f.events(), 0);
}
#[test]
fn user_limits_tightened_after_authentication_are_enforced() {
    let f = Fixture::new();
    let a = f.auth(Policy::default(), Policy::default());
    drop(admission::acquire(f.state.clone(), &a, 0, 0).unwrap());
    f.user_policy(
        &a,
        Policy {
            rpm: Some(1),
            ..Policy::default()
        },
    );
    rejected(&f, &a, 0, 0);
    assert_eq!(f.events(), 2);
}
#[test]
fn stale_snapshot_cannot_change_reservation_owner() {
    let f = Fixture::new();
    let mut a = f.auth(Policy::default(), Policy::default());
    a.user_id += 100;
    assert_eq!(
        admission::acquire(f.state.clone(), &a, 0, 0)
            .unwrap_err()
            .status
            .as_u16(),
        401
    );
    assert_eq!(f.events(), 0);
}
