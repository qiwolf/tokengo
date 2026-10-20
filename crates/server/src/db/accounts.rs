//! Panel accounts are separate from API identities; session material is never serialized.
use super::{now_ms, DbError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
pub const SESSION_MS: i64 = 12 * 60 * 60 * 1000;
#[derive(Clone, Serialize)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub user_id: Option<i64>,
    pub enabled: bool,
    pub created_at_ms: i64,
}
#[derive(Clone)]
pub struct Credential {
    pub account: Account,
    pub hash: String,
    pub version: i64,
}
fn bad(s: &str) -> DbError {
    DbError::Other(s.into())
}
pub fn username(s: &str) -> Result<String> {
    if !(3..=64).contains(&s.len())
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(bad("用户名须为3..64个ASCII字母、数字、点、下划线或连字符"));
    }
    Ok(s.to_ascii_lowercase())
}
pub fn password(s: &str) -> Result<()> {
    if !(12..=256).contains(&s.len()) || s.chars().any(char::is_control) {
        return Err(bad("新密码须为12..256字节且不含控制字符"));
    }
    Ok(())
}
pub fn display_name(s: &str) -> Result<&str> {
    super::identity::validate_name(s)
}
pub fn digest(raw: &str) -> String {
    hex::encode(Sha256::digest(raw.as_bytes()))
}
pub fn legacy_digest(raw: &str) -> String {
    format!("sha256:{}", digest(raw))
}
pub fn atomic<T>(c: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    c.execute_batch("SAVEPOINT panel_account_write")?;
    match f() {
        Ok(v) => {
            c.execute_batch("RELEASE panel_account_write")?;
            Ok(v)
        }
        Err(e) => {
            c.execute_batch("ROLLBACK TO panel_account_write; RELEASE panel_account_write")?;
            Err(e)
        }
    }
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Credential> {
    Ok(Credential {
        account: Account {
            id: r.get(0)?,
            username: r.get(1)?,
            display_name: r.get(2)?,
            role: r.get(3)?,
            user_id: r.get(4)?,
            enabled: r.get(5)?,
            created_at_ms: r.get(6)?,
        },
        hash: r.get(7)?,
        version: r.get(8)?,
    })
}
const SELECT:&str="SELECT id,username,display_name,role,user_id,enabled,created_at_ms,password_hash,credential_version FROM panel_accounts";
pub fn by_id(c: &Connection, id: i64) -> Result<Option<Credential>> {
    Ok(c.query_row(&format!("{SELECT} WHERE id=?1"), [id], row)
        .optional()?)
}
pub fn by_username(c: &Connection, name: &str) -> Result<Option<Credential>> {
    Ok(c.query_row(
        &format!("{SELECT} WHERE username=?1 COLLATE NOCASE"),
        [name],
        row,
    )
    .optional()?)
}
pub fn root(c: &Connection) -> Result<Option<Credential>> {
    Ok(c.query_row(&format!("{SELECT} WHERE role='root'"), [], row)
        .optional()?)
}
pub fn active(c: &Connection, a: &Credential) -> Result<bool> {
    if !a.account.enabled {
        return Ok(false);
    }
    if a.account.role == "root" {
        return Ok(a.account.user_id.is_none());
    }
    let Some(id) = a.account.user_id else {
        return Ok(false);
    };
    Ok(super::identity::user(c, id)?.is_some_and(|u| u.enabled))
}
pub fn accounts(c: &Connection) -> Result<Vec<Account>> {
    let mut q = c.prepare(&format!("{SELECT} ORDER BY id"))?;
    let rows = q
        .query_map([], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().map(|r| r.account).collect())
}
pub fn sync_root_hash(c: &Connection, hash: &str) -> Result<()> {
    if let Some(a) = root(c)? {
        c.execute(
            "UPDATE panel_accounts SET password_hash=?2 WHERE id=?1",
            params![a.account.id, hash],
        )?;
    } else {
        c.execute("INSERT INTO panel_accounts(username,display_name,role,password_hash,created_at_ms) VALUES('admin','管理员','root',?1,?2)",params![hash,now_ms()])?;
    }
    c.execute("DELETE FROM account_sessions WHERE account_id IN (SELECT id FROM panel_accounts WHERE role='root')",[])?;
    Ok(())
}
pub fn legacy_root_valid(c: &Connection) -> Result<bool> {
    let configured = super::settings::get(c, super::settings::PANEL_PASSWORD_HASH)?;
    match (root(c)?, configured) {
        (Some(a), Some(hash)) => {
            Ok(a.account.enabled && a.account.user_id.is_none() && a.hash == hash)
        }
        (None, None) => Ok(true),
        _ => Ok(false),
    }
}
pub fn migrate_legacy_sessions(c: &Connection) -> Result<()> {
    let mut q = c.prepare("SELECT token FROM panel_sessions WHERE token NOT LIKE 'sha256:%'")?;
    let rows = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    atomic(c, || {
        for raw in rows {
            c.execute(
                "UPDATE panel_sessions SET token=?2 WHERE token=?1",
                params![raw, legacy_digest(&raw)],
            )?;
        }
        Ok(())
    })
}
pub fn create_session(c: &Connection, a: &Credential) -> Result<String> {
    if !active(c, a)? {
        return Err(bad("账号不可用"));
    }
    let now = now_ms();
    c.execute(
        "DELETE FROM account_sessions WHERE expires_at_ms<=?1",
        [now],
    )?;
    let raw = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    c.execute("INSERT INTO account_sessions(session_digest,account_id,credential_version,created_at_ms,expires_at_ms) VALUES(?1,?2,?3,?4,?5)",params![digest(&raw),a.account.id,a.version,now,now+SESSION_MS])?;
    c.execute("DELETE FROM account_sessions WHERE account_id=?1 AND session_digest NOT IN (SELECT session_digest FROM account_sessions WHERE account_id=?1 ORDER BY created_at_ms DESC,rowid DESC LIMIT 10)",[a.account.id])?;
    Ok(raw)
}
pub fn authenticate(c: &Connection, raw: &str) -> Result<Option<Credential>> {
    if raw.len() != 64 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let session:Option<(i64,i64)>=c.query_row("SELECT account_id,credential_version FROM account_sessions WHERE session_digest=?1 AND expires_at_ms>?2",params![digest(raw),now_ms()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((id, version)) = session else {
        return Ok(None);
    };
    let Some(a) = by_id(c, id)? else {
        return Ok(None);
    };
    Ok((a.version == version && active(c, &a)?).then_some(a))
}
pub fn revoke(c: &Connection, raw: &str) -> Result<()> {
    c.execute(
        "DELETE FROM account_sessions WHERE session_digest=?1",
        [digest(raw)],
    )?;
    Ok(())
}
pub fn create(
    c: &Connection,
    name: &str,
    display: &str,
    user_id: i64,
    hash: &str,
) -> Result<Account> {
    let name = username(name)?;
    let display = display_name(display)?;
    if !super::identity::user(c, user_id)?.is_some_and(|u| u.enabled) {
        return Err(bad("请选择真实且启用的API用户"));
    }
    if by_username(c, &name)?.is_some() {
        return Err(bad("用户名已被使用"));
    }
    c.execute("INSERT INTO panel_accounts(username,display_name,role,user_id,password_hash,created_at_ms) VALUES(?1,?2,'user',?3,?4,?5)",params![name,display,user_id,hash,now_ms()])?;
    Ok(by_id(c, c.last_insert_rowid())?
        .ok_or_else(|| bad("账号不存在"))?
        .account)
}
pub fn audit(c: &Connection, actor: i64, event: &str, target: i64) -> Result<()> {
    c.execute("INSERT INTO audit_events(ts_ms,actor_user_id,actor_key_id,action,target_type,target_id,metadata_json) VALUES(?1,NULL,NULL,?2,'panel_account',?3,?4)",params![now_ms(),event,target,serde_json::json!({"actor_account_id":actor}).to_string()])?;
    Ok(())
}
