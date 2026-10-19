//! Explicit account/model faults, separate from transient model cooldowns.
use super::Result;
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fault {
    Key,
    Balance,
    Quota,
    Model,
}
impl Fault {
    pub fn label(self) -> &'static str {
        match self {
            Self::Key => "密钥无效",
            Self::Balance => "余额不足",
            Self::Quota => "额度耗尽",
            Self::Model => "模型无权限",
        }
    }
    fn kind(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Balance => "balance",
            Self::Quota => "quota",
            Self::Model => "model",
        }
    }
}
/// Only structured, explicit codes are trusted. HTTP codes and vague text cannot suspend an account.
pub fn classify(status: u16, body: &[u8]) -> Option<Fault> {
    if !(400..600).contains(&status) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let e = v.get("error").unwrap_or(&v);
    let code = e
        .get("code")
        .and_then(|x| x.as_str())
        .or_else(|| e.get("type").and_then(|x| x.as_str()))?;
    match code.to_ascii_lowercase().as_str() {
        "invalid_api_key" | "api_key_invalid" | "key_expired" | "api_key_expired"
        | "key_revoked" | "missing_api_key" => Some(Fault::Key),
        "insufficient_balance" | "balance_not_enough" | "account_balance_insufficient" => {
            Some(Fault::Balance)
        }
        "billing_hard_limit_reached" | "account_quota_exhausted" => Some(Fault::Quota),
        "model_access_denied" | "model_permission_denied" => Some(Fault::Model),
        _ => None,
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub channel_id: i64,
    pub model: String,
    pub kind: String,
    pub reason: String,
    pub occurred_ms: i64,
    pub retry_at: i64,
    pub ready: bool,
    pub lease_until: i64,
    pub generation: i64,
}
pub fn revision(c: &Connection, id: i64) -> Result<i64> {
    Ok(c.query_row(
        "SELECT fault_revision FROM channels WHERE id=?1",
        [id],
        |r| r.get(0),
    )?)
}
pub fn rows(c: &Connection) -> Result<Vec<Row>> {
    let mut q=c.prepare("SELECT f.channel_id,f.model,f.kind,f.reason,f.occurred_ms,f.retry_at,f.ready,f.lease_until,f.generation FROM channel_faults f JOIN channels c ON c.id=f.channel_id AND c.fault_revision=f.revision ORDER BY f.occurred_ms DESC")?;
    let out = q
        .query_map([], |r| {
            Ok(Row {
                channel_id: r.get(0)?,
                model: r.get(1)?,
                kind: r.get(2)?,
                reason: r.get(3)?,
                occurred_ms: r.get(4)?,
                retry_at: r.get(5)?,
                ready: r.get(6)?,
                lease_until: r.get(7)?,
                generation: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(out)
}
pub fn has_fault(c: &Connection, id: i64, model: Option<&str>) -> Result<bool> {
    Ok(rows(c)?.iter().any(|r| {
        r.channel_id == id
            && model
                .map(|m| r.model.is_empty() || r.model == tokengo_shared::normalize_model_name(m))
                .unwrap_or(true)
    }))
}
pub fn blocked(c: &Connection, id: i64, model: &str, now: i64) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM channel_faults f JOIN channels c ON c.id=f.channel_id AND c.fault_revision=f.revision WHERE f.channel_id=?1 AND (model='' OR model=?2) AND (lease_until>?3 OR (ready=0 AND (retry_at=0 OR retry_at>?3))))",params![id,tokengo_shared::normalize_model_name(model),now],|r|r.get(0))?)
}
#[derive(Debug)]
pub struct Permit {
    pub id: i64,
    pub revision: i64,
    leases: Vec<(String, i64)>,
}
impl Permit {
    pub fn recovering(&self) -> bool {
        !self.leases.is_empty()
    }
}
pub fn current(c: &Connection, p: &Permit) -> Result<bool> {
    Ok(revision(c, p.id)? == p.revision)
}
pub fn defer_quota(c: &Connection, id: i64, now: i64, delay: u64) -> Result<()> {
    let until = now.saturating_add(i64::try_from(delay).unwrap_or(i64::MAX));
    c.execute(
        "UPDATE channel_faults SET retry_at=?2 WHERE channel_id=?1 AND kind='quota'",
        params![id, until],
    )?;
    Ok(())
}
pub fn claim(c: &Connection, id: i64, model: &str, rev: i64, now: i64) -> Result<Option<Permit>> {
    if revision(c, id)? != rev || blocked(c, id, model, now)? {
        return Ok(None);
    }
    let mut p = Permit {
        id,
        revision: rev,
        leases: Vec::new(),
    };
    for r in rows(c)?.into_iter().filter(|r| {
        r.channel_id == id
            && (r.model.is_empty() || r.model == tokengo_shared::normalize_model_name(model))
    }) {
        c.execute("UPDATE channel_faults SET lease_until=?3,generation=generation+1 WHERE channel_id=?1 AND model=?2",params![id,r.model,now+660000])?;
        p.leases.push((r.model, r.generation + 1));
    }
    Ok(Some(p))
}
pub fn suspend(
    c: &Connection,
    p: &Permit,
    model: &str,
    f: Fault,
    reason: &str,
    now: i64,
) -> Result<()> {
    if revision(c, p.id)? != p.revision {
        return Ok(());
    }
    let scope = if f == Fault::Model {
        tokengo_shared::normalize_model_name(model)
    } else {
        String::new()
    };
    c.execute(
        "UPDATE channels SET fault_revision=fault_revision+1 WHERE id=?1",
        [p.id],
    )?;
    let next = revision(c, p.id)?;
    c.execute(
        "UPDATE channel_faults SET revision=?2 WHERE channel_id=?1",
        params![p.id, next],
    )?;
    c.execute("INSERT INTO channel_faults(channel_id,model,kind,reason,occurred_ms,revision) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(channel_id,model) DO UPDATE SET kind=excluded.kind,reason=excluded.reason,occurred_ms=excluded.occurred_ms,ready=0,retry_at=0,lease_until=0,generation=channel_faults.generation+1",params![p.id,scope,f.kind(),reason,now,next])?;
    Ok(())
}
pub fn complete(c: &Connection, p: &Permit, success: bool) -> Result<()> {
    for (model, g) in &p.leases {
        if success {
            c.execute("DELETE FROM channel_faults WHERE channel_id=?1 AND model=?2 AND revision=?3 AND generation=?4",params![p.id,model,p.revision,g])?;
        } else {
            c.execute("UPDATE channel_faults SET ready=0,retry_at=0,lease_until=0,generation=generation+1 WHERE channel_id=?1 AND model=?2 AND revision=?3 AND generation=?4",params![p.id,model,p.revision,g])?;
        }
    }
    Ok(())
}
pub fn release(c: &Connection, p: &Permit) -> Result<()> {
    for (model, g) in &p.leases {
        c.execute("UPDATE channel_faults SET lease_until=0 WHERE channel_id=?1 AND model=?2 AND revision=?3 AND generation=?4",params![p.id,model,p.revision,g])?;
    }
    Ok(())
}
pub fn retry(c: &Connection, id: i64, model: &str) -> Result<()> {
    c.execute(
        "UPDATE channels SET fault_revision=fault_revision+1 WHERE id=?1",
        [id],
    )?;
    c.execute("UPDATE channel_faults SET revision=(SELECT fault_revision FROM channels WHERE id=?1) WHERE channel_id=?1",[id])?;
    c.execute("UPDATE channel_faults SET ready=1,lease_until=0,generation=generation+1 WHERE channel_id=?1 AND model=?2",params![id,model])?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Connection {
        let c = super::super::open_in_memory().unwrap();
        c.execute(
            "INSERT INTO channels(name,base_url) VALUES('a','http://a')",
            [],
        )
        .unwrap();
        c
    }
    #[test]
    fn conservative_classifier() {
        for (code, f) in [
            ("invalid_api_key", Fault::Key),
            ("insufficient_balance", Fault::Balance),
            ("account_quota_exhausted", Fault::Quota),
            ("model_access_denied", Fault::Model),
        ] {
            for s in [400, 401, 402, 403, 429, 503] {
                assert_eq!(
                    classify(
                        s,
                        serde_json::json!({"error":{"code":code}})
                            .to_string()
                            .as_bytes()
                    ),
                    Some(f)
                );
            }
        }
        for body in [
            r#"{"error":{"code":"insufficient_quota"}}"#,
            r#"{"error":{"message":"余额不足"}}"#,
            r#"{"error":{"code":"invalid_request_error"}}"#,
            "余额不足",
        ] {
            assert_eq!(classify(402, body.as_bytes()), None);
        }
    }
    #[test]
    fn single_recovery_and_scopes() {
        let c = fixture();
        let rev = revision(&c, 1).unwrap();
        let p = claim(&c, 1, "a", rev, 0).unwrap().unwrap();
        suspend(&c, &p, "a", Fault::Balance, "balance", 0).unwrap();
        assert!(blocked(&c, 1, "b", 1).unwrap());
        retry(&c, 1, "").unwrap();
        let p = claim(&c, 1, "a", revision(&c, 1).unwrap(), 2)
            .unwrap()
            .unwrap();
        assert!(claim(&c, 1, "b", revision(&c, 1).unwrap(), 3)
            .unwrap()
            .is_none());
        complete(&c, &p, false).unwrap();
        assert!(blocked(&c, 1, "b", 4).unwrap());
        retry(&c, 1, "").unwrap();
        let p = claim(&c, 1, "b", revision(&c, 1).unwrap(), 5)
            .unwrap()
            .unwrap();
        complete(&c, &p, true).unwrap();
        assert!(rows(&c).unwrap().is_empty());
        let p = claim(&c, 1, "a", revision(&c, 1).unwrap(), 6)
            .unwrap()
            .unwrap();
        suspend(&c, &p, "a", Fault::Model, "model", 6).unwrap();
        assert!(!blocked(&c, 1, "b", 7).unwrap());
    }
    #[test]
    fn old_config_cannot_suspend_new_key() {
        let c = fixture();
        let rev = revision(&c, 1).unwrap();
        let p = claim(&c, 1, "a", rev, 0).unwrap().unwrap();
        c.execute("UPDATE channels SET base_url='http://new' WHERE id=1", [])
            .unwrap();
        suspend(&c, &p, "a", Fault::Key, "key", 1).unwrap();
        assert!(rows(&c).unwrap().is_empty());
        assert!(claim(&c, 1, "a", rev, 1).unwrap().is_none());
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    fn fixture() -> Connection {
        let c = crate::db::open_in_memory().unwrap();
        c.execute(
            "INSERT INTO channels(name,base_url) VALUES('a','http://a')",
            [],
        )
        .unwrap();
        c
    }
    #[test]
    fn late_failure_cannot_overwrite_retry_or_success() {
        let c = fixture();
        let old = claim(&c, 1, "a", 0, 0).unwrap().unwrap();
        suspend(&c, &old, "a", Fault::Key, "key", 1).unwrap();
        suspend(&c, &old, "a", Fault::Balance, "late", 2).unwrap();
        assert_eq!(rows(&c).unwrap()[0].kind, "key");
        retry(&c, 1, "").unwrap();
        let p = claim(&c, 1, "a", revision(&c, 1).unwrap(), 3)
            .unwrap()
            .unwrap();
        complete(&c, &p, true).unwrap();
        suspend(&c, &old, "a", Fault::Key, "late", 4).unwrap();
        assert!(rows(&c).unwrap().is_empty());
    }
    #[test]
    fn rename_preserves_pause_headers_allow_only_one_trial() {
        let c = fixture();
        let p = claim(&c, 1, "a", 0, 0).unwrap().unwrap();
        suspend(&c, &p, "a", Fault::Key, "key", 1).unwrap();
        let rev = revision(&c, 1).unwrap();
        c.execute(
            "UPDATE channels SET name='new name',priority=5 WHERE id=1",
            [],
        )
        .unwrap();
        assert_eq!(revision(&c, 1).unwrap(), rev);
        assert!(blocked(&c, 1, "a", 2).unwrap());
        c.execute(
            "INSERT INTO channel_headers(channel_id,name,value) VALUES(1,'Authorization','test')",
            [],
        )
        .unwrap();
        assert!(rows(&c).unwrap()[0].ready);
        let p = claim(&c, 1, "a", revision(&c, 1).unwrap(), 3)
            .unwrap()
            .unwrap();
        assert!(claim(&c, 1, "a", revision(&c, 1).unwrap(), 4)
            .unwrap()
            .is_none());
        release(&c, &p).unwrap();
        let p = claim(&c, 1, "a", revision(&c, 1).unwrap(), 5)
            .unwrap()
            .unwrap();
        complete(&c, &p, false).unwrap();
        assert!(blocked(&c, 1, "a", 6).unwrap());
    }
    #[test]
    fn quota_deadline_releases_exactly_one_trial() {
        let c = fixture();
        let p = claim(&c, 1, "a", 0, 0).unwrap().unwrap();
        suspend(&c, &p, "a", Fault::Quota, "quota", 1).unwrap();
        defer_quota(&c, 1, 1, 100).unwrap();
        let rev = revision(&c, 1).unwrap();
        assert!(claim(&c, 1, "a", rev, 100).unwrap().is_none());
        let p = claim(&c, 1, "a", rev, 101).unwrap().unwrap();
        assert!(p.recovering());
        assert!(claim(&c, 1, "a", rev, 102).unwrap().is_none());
    }
    #[test]
    fn pause_survives_connection_reopen() {
        let dir = std::env::temp_dir().join(format!("fault-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        {
            let c = crate::db::open(&path).unwrap();
            c.execute(
                "INSERT INTO channels(name,base_url) VALUES('a','http://a')",
                [],
            )
            .unwrap();
            let p = claim(&c, 1, "a", 0, 0).unwrap().unwrap();
            suspend(&c, &p, "a", Fault::Balance, "balance", 1).unwrap();
        }
        let c = crate::db::open(&path).unwrap();
        assert!(blocked(&c, 1, "a", i64::MAX - 1).unwrap());
        assert_eq!(rows(&c).unwrap()[0].kind, "balance");
        drop(c);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
