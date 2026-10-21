//! Independent site-operations credentials: no plaintext credential is persisted.
use super::{now_ms, DbError, Result};
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Clone, Debug, Serialize)]
pub struct Token {
    pub id: i64,
    pub public_id: String,
    pub name: String,
    pub created_by_account_id: i64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
    pub last_used_at_ms: Option<i64>,
    pub status: String,
}
#[derive(Serialize)]
pub struct Created {
    pub token: Token,
    pub secret: String,
}
const SELECT:&str="SELECT id,public_id,name,created_by_account_id,created_at_ms,expires_at_ms,revoked_at_ms,last_used_at_ms FROM operations_tokens";
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Token> {
    let expiry: i64 = r.get(5)?;
    let revoked: Option<i64> = r.get(6)?;
    Ok(Token {
        id: r.get(0)?,
        public_id: r.get(1)?,
        name: r.get(2)?,
        created_by_account_id: r.get(3)?,
        created_at_ms: r.get(4)?,
        expires_at_ms: expiry,
        revoked_at_ms: revoked,
        last_used_at_ms: r.get(7)?,
        status: if revoked.is_some() {
            "revoked"
        } else if expiry <= now_ms() {
            "expired"
        } else {
            "active"
        }
        .into(),
    })
}
fn bad(s: &str) -> DbError {
    DbError::Other(s.into())
}
pub fn validate(name: &str, days: i64) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(bad("令牌名称须为1–64字符且不含控制字符"));
    }
    if !(1..=365).contains(&days) {
        return Err(bad("有效期须为1–365天"));
    }
    Ok(())
}
fn digest(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"tokengo:operations-token:v1\0");
    h.update(raw.as_bytes());
    hex::encode(h.finalize())
}
fn atomic<T>(c: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    c.execute_batch("SAVEPOINT operations_token_write")?;
    match f() {
        Ok(v) => {
            c.execute_batch("RELEASE operations_token_write")?;
            Ok(v)
        }
        Err(e) => {
            c.execute_batch("ROLLBACK TO operations_token_write; RELEASE operations_token_write")?;
            Err(e)
        }
    }
}
pub fn audit(
    c: &Connection,
    actor_type: &str,
    actor_id: i64,
    action: &str,
    target_type: &str,
    target_id: Option<i64>,
    request_id: Option<&str>,
    success: bool,
) -> Result<()> {
    c.execute("INSERT INTO audit_events(ts_ms,actor_user_id,actor_key_id,action,target_type,target_id,metadata_json) VALUES(?1,NULL,NULL,?2,?3,?4,?5)",params![now_ms(),action,target_type,target_id,serde_json::json!({"actor_type":actor_type,"actor_id":actor_id,"request_id":request_id,"success":success}).to_string()])?;
    Ok(())
}
pub fn create(c: &Connection, name: &str, days: i64, root_id: i64) -> Result<Created> {
    validate(name, days)?;
    atomic(c, || {
        // This count and insert share the SQLite write transaction, including across processes.
        c.execute("UPDATE operations_tokens SET id=id WHERE 0", [])?;
        if !super::accounts::by_id(c, root_id)?.is_some_and(|a| {
            a.account.role == "root" && a.account.enabled && a.account.user_id.is_none()
        }) {
            return Err(bad("签发者不是有效root账号"));
        }
        let n:i64=c.query_row("SELECT COUNT(*) FROM operations_tokens WHERE revoked_at_ms IS NULL AND expires_at_ms>?1",[now_ms()],|r|r.get(0))?;
        if n >= 20 {
            return Err(bad("最多允许20把有效运维令牌"));
        }
        let mut bytes = [0u8; 32];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| bad("安全随机源不可用"))?;
        let public_id = uuid::Uuid::new_v4().simple().to_string();
        let secret = format!("tgo_ops_{}.{}", public_id, hex::encode(bytes));
        let now = now_ms();
        c.execute("INSERT INTO operations_tokens(public_id,name,token_hash,created_by_account_id,created_at_ms,expires_at_ms) VALUES(?1,?2,?3,?4,?5,?6)",params![public_id,name.trim(),digest(&secret),root_id,now,now+days*86_400_000])?;
        let id = c.last_insert_rowid();
        let token = c.query_row(&format!("{SELECT} WHERE id=?1"), [id], row)?;
        audit(
            c,
            "root_account",
            root_id,
            "operations_token.create",
            "operations_token",
            Some(id),
            None,
            true,
        )?;
        Ok(Created { token, secret })
    })
}
pub fn by_id(c: &Connection, id: i64) -> Result<Option<Token>> {
    Ok(c.query_row(&format!("{SELECT} WHERE id=?1"), [id], row)
        .optional()?)
}
pub fn active(c: &Connection, id: i64) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM operations_tokens WHERE id=?1 AND revoked_at_ms IS NULL AND expires_at_ms>?2)",params![id,now_ms()],|r|r.get(0))?)
}
pub fn authenticate(c: &Connection, raw: &str) -> Result<Option<Token>> {
    // Fixed-size public locator and 256-bit random secret. Never fall back to another identity.
    if raw.len() != 105 {
        return Ok(None);
    }
    let Some(rest) = raw.strip_prefix("tgo_ops_") else {
        return Ok(None);
    };
    let Some((public, secret)) = rest.split_once('.') else {
        return Ok(None);
    };
    if public.len() != 32
        || secret.len() != 64
        || !public
            .bytes()
            .chain(secret.bytes())
            .all(|b| b.is_ascii_hexdigit())
    {
        return Ok(None);
    }
    let expected: Option<String> = c
        .query_row(
            "SELECT token_hash FROM operations_tokens WHERE public_id=?1",
            [public],
            |r| r.get(0),
        )
        .optional()?;
    let actual = digest(raw);
    let expected = expected.unwrap_or_else(|| "0".repeat(64));
    if !bool::from(actual.as_bytes().ct_eq(expected.as_bytes())) {
        return Ok(None);
    }
    let found = c
        .query_row(
            &format!("{SELECT} WHERE public_id=?1 AND revoked_at_ms IS NULL AND expires_at_ms>?2"),
            params![public, now_ms()],
            row,
        )
        .optional()?;
    if let Some(t) = &found {
        c.execute("UPDATE operations_tokens SET last_used_at_ms=?2 WHERE id=?1 AND (last_used_at_ms IS NULL OR last_used_at_ms<=?2-60000)",params![t.id,now_ms()])?;
    }
    Ok(found)
}
pub fn list(c: &Connection, before_id: Option<i64>, limit: i64) -> Result<Vec<Token>> {
    let mut q = c.prepare(&format!(
        "{SELECT} WHERE (?1 IS NULL OR id<?1) ORDER BY id DESC LIMIT ?2"
    ))?;
    let rows = q
        .query_map(params![before_id, limit.clamp(1, 101)], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn revoke(c: &Connection, id: i64, actor_root_id: i64) -> Result<bool> {
    atomic(c, || {
        let n = c.execute(
            "UPDATE operations_tokens SET revoked_at_ms=COALESCE(revoked_at_ms,?2) WHERE id=?1",
            params![id, now_ms()],
        )?;
        if n > 0 {
            audit(
                c,
                "root_account",
                actor_root_id,
                "operations_token.revoke",
                "operations_token",
                Some(id),
                None,
                true,
            )?;
        }
        Ok(n > 0)
    })
}
pub fn revoke_all(c: &Connection, actor_root_id: i64) -> Result<usize> {
    atomic(c, || {
        let n=c.execute("UPDATE operations_tokens SET revoked_at_ms=?1 WHERE revoked_at_ms IS NULL AND expires_at_ms>?1",[now_ms()])?;
        audit(
            c,
            "root_account",
            actor_root_id,
            "operations_token.revoke_all",
            "operations_token",
            None,
            None,
            true,
        )?;
        Ok(n)
    })
}
pub fn instance_id(c: &Connection) -> Result<String> {
    if let Some(id) = c
        .query_row(
            "SELECT instance_id FROM operations_instance WHERE id=1",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    c.execute(
        "INSERT OR IGNORE INTO operations_instance(id,instance_id) VALUES(1,?1)",
        [uuid::Uuid::new_v4().to_string()],
    )?;
    Ok(c.query_row(
        "SELECT instance_id FROM operations_instance WHERE id=1",
        [],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root(c: &Connection) -> i64 {
        crate::db::accounts::sync_root_hash(c, "test-hash").unwrap();
        crate::db::accounts::root(c).unwrap().unwrap().account.id
    }
    #[test]
    fn lifecycle_hash_only_and_atomic_audit() {
        let c = crate::db::open_in_memory().unwrap();
        let id = root(&c);
        let issued = create(&c, "维护智能体", 90, id).unwrap();
        assert!(issued.secret.starts_with("tgo_ops_"));
        assert_eq!(
            authenticate(&c, &issued.secret).unwrap().unwrap().id,
            issued.token.id
        );
        let stored: String = c
            .query_row("SELECT token_hash FROM operations_tokens", [], |r| r.get(0))
            .unwrap();
        assert!(!stored.contains(&issued.secret));
        assert_eq!(stored.len(), 64);
        assert!(revoke(&c, issued.token.id, id).unwrap());
        assert!(authenticate(&c, &issued.secret).unwrap().is_none());
        assert!(revoke(&c, issued.token.id, id).unwrap());
        assert_eq!(list(&c, None, 50).unwrap()[0].status, "revoked");
        assert_eq!(
            by_id(&c, issued.token.id).unwrap().unwrap().id,
            issued.token.id
        );
        assert!(by_id(&c, i64::MAX).unwrap().is_none());
    }
    #[test]
    fn issuance_rolls_back_when_audit_fails() {
        let c = crate::db::open_in_memory().unwrap();
        let id = root(&c);
        c.execute_batch("CREATE TRIGGER deny_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;").unwrap();
        assert!(create(&c, "x", 90, id).is_err());
        assert!(list(&c, None, 50).unwrap().is_empty());
    }
    #[test]
    fn root_rotation_does_not_revoke_and_audit_has_no_secret() {
        let c = crate::db::open_in_memory().unwrap();
        let id = root(&c);
        let issued = create(&c, "x", 90, id).unwrap();
        crate::db::accounts::sync_root_hash(&c, "new-test-hash").unwrap();
        assert!(authenticate(&c, &issued.secret).unwrap().is_some());
        let audit: String = c
            .query_row(
                "SELECT metadata_json FROM audit_events ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(audit.contains("root_account"));
        assert!(!audit.contains(&issued.secret));
        let mut forged = issued.secret.clone();
        let last = forged.pop().unwrap();
        forged.push(if last == 'a' { 'b' } else { 'a' });
        assert!(authenticate(&c, &forged).unwrap().is_none());
    }
    #[test]
    fn revoked_tokens_and_instance_survive_reopen() {
        let path =
            std::env::temp_dir().join(format!("tokengo-ops-{}.sqlite", uuid::Uuid::new_v4()));
        let (raw, instance) = {
            let c = crate::db::open(&path).unwrap();
            let id = root(&c);
            let issued = create(&c, "x", 1, id).unwrap();
            let instance = instance_id(&c).unwrap();
            revoke(&c, issued.token.id, id).unwrap();
            (issued.secret, instance)
        };
        {
            let c = crate::db::open(&path).unwrap();
            assert!(authenticate(&c, &raw).unwrap().is_none());
            assert_eq!(instance_id(&c).unwrap(), instance);
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn revocation_rolls_back_when_audit_fails() {
        let c = crate::db::open_in_memory().unwrap();
        let id = root(&c);
        let issued = create(&c, "x", 90, id).unwrap();
        c.execute_batch("CREATE TRIGGER deny_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;").unwrap();
        assert!(revoke(&c, issued.token.id, id).is_err());
        assert!(active(&c, issued.token.id).unwrap());
        assert!(revoke_all(&c, id).is_err());
        assert!(active(&c, issued.token.id).unwrap());
    }
    #[test]
    fn validation_limit_expiry_and_instance() {
        let c = crate::db::open_in_memory().unwrap();
        let id = root(&c);
        for days in [0, 366] {
            assert!(create(&c, "x", days, id).is_err());
        }
        assert!(create(&c, " ", 90, id).is_err());
        for _ in 0..20 {
            create(&c, "x", 1, id).unwrap();
        }
        assert!(create(&c, "x", 1, id).is_err());
        c.execute("UPDATE operations_tokens SET expires_at_ms=0", [])
            .unwrap();
        let one = create(&c, "new", 1, id).unwrap();
        assert!(active(&c, one.token.id).unwrap());
        assert_eq!(revoke_all(&c, id).unwrap(), 1);
        assert!(!active(&c, one.token.id).unwrap());
        let instance = instance_id(&c).unwrap();
        c.execute_batch("CREATE TRIGGER deny_instance_write BEFORE INSERT ON operations_instance BEGIN SELECT RAISE(ABORT,'read-only actor'); END;").unwrap();
        assert_eq!(instance_id(&c).unwrap(), instance);
        for raw in ["", "tgo_ops_bad", "tgo_ops_abc.secret"] {
            assert!(authenticate(&c, raw).unwrap().is_none());
        }
    }
}
