//! Named gateway credentials. Settings compatibility targets only the default row.
use super::{now_ms, sessions, DbError, Result};
use crate::crypto::SecretBox;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Serialize)]
pub struct GatewayKey {
    pub id: i64,
    pub name: String,
    pub hint: String,
    pub created_at_ms: i64,
}

pub fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(DbError::Other(
            "Key name must contain 1 to 80 characters without control characters".into(),
        ));
    }
    Ok(name)
}

pub fn generate() -> String {
    format!(
        "tk-{}.{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn selector(key: &str) -> Option<&str> {
    let (prefix, secret) = key.split_once('.')?;
    (prefix.len() == 35
        && prefix.starts_with("tk-")
        && secret.len() == 32
        && prefix[3..].bytes().all(|c| c.is_ascii_hexdigit())
        && secret.bytes().all(|c| c.is_ascii_hexdigit()))
    .then_some(prefix)
}

pub fn list(conn: &Connection) -> Result<Vec<GatewayKey>> {
    let mut stmt =
        conn.prepare("SELECT id,name,hint,created_at_ms FROM gateway_keys ORDER BY id")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(GatewayKey {
                id: r.get(0)?,
                name: r.get(1)?,
                hint: r.get(2)?,
                created_at_ms: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn any(conn: &Connection) -> Result<bool> {
    Ok(
        conn.query_row("SELECT EXISTS(SELECT 1 FROM gateway_keys)", [], |r| {
            r.get(0)
        })?,
    )
}

pub fn default_id(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT id FROM gateway_keys WHERE is_default=1", [], |r| {
            r.get(0)
        })
        .optional()?)
}

pub fn create(
    conn: &Connection,
    sb: &SecretBox,
    name: &str,
    key: &str,
    default: bool,
) -> Result<GatewayKey> {
    let name = validate_name(name)?;
    if key.trim().is_empty() {
        return Err(DbError::Other("Gateway key cannot be empty".into()));
    }
    let hash = sessions::hash_password(key)?;
    let (nonce, cipher) = sb
        .seal(key)
        .map_err(|_| DbError::Other("Cannot encrypt gateway key".into()))?;
    conn.execute("INSERT INTO gateway_keys(name,hint,created_at_ms,key_hash,nonce,cipher,selector,is_default) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![name,sessions::mask_key(key),now_ms(),hash,nonce,cipher,selector(key),default])?;
    let id = conn.last_insert_rowid();
    Ok(GatewayKey {
        id,
        name: name.into(),
        hint: sessions::mask_key(key),
        created_at_ms: conn.query_row(
            "SELECT created_at_ms FROM gateway_keys WHERE id=?1",
            [id],
            |r| r.get(0),
        )?,
    })
}

pub fn set_default(conn: &Connection, sb: &SecretBox, key: &str) -> Result<()> {
    super::in_tx(conn, |c| {
        if let Some(id) = default_id(c)? {
            delete(c, id)?;
        }
        create(c, sb, "Default", key, true)?;
        Ok(())
    })
}

pub fn reveal(conn: &Connection, sb: &SecretBox, id: i64) -> Result<Option<String>> {
    let row: Option<(Option<Vec<u8>>, Option<Vec<u8>>)> = conn
        .query_row(
            "SELECT nonce,cipher FROM gateway_keys WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match row {
        Some((Some(n), Some(c))) => sb
            .open(&n, &c)
            .map(Some)
            .map_err(|_| DbError::Other("Cannot decrypt gateway key".into())),
        _ => Ok(None),
    }
}

pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.execute("DELETE FROM gateway_keys WHERE id=?1", [id])? > 0)
}

pub fn check(conn: &Connection, key: &str) -> Result<bool> {
    if key.trim().is_empty() {
        return Ok(false);
    }
    // The random public selector makes cold authentication one indexed lookup, not N Argon2 verifications.
    let hash: Option<String> = if let Some(sel) = selector(key) {
        conn.query_row(
            "SELECT key_hash FROM gateway_keys WHERE selector=?1",
            [sel],
            |r| r.get(0),
        )
        .optional()?
    } else {
        None
    };
    if let Some(hash) = hash {
        return Ok(sessions::verify_password(key, &hash));
    }
    let legacy: Option<String> = conn
        .query_row(
            "SELECT key_hash FROM gateway_keys WHERE is_default=1 AND selector IS NULL",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(legacy.is_some_and(|hash| sessions::verify_password(key, &hash)))
}

/// SQL migration consumes old settings once, including hash-only credentials. Enrich encrypted legacy metadata at startup.
pub fn hydrate_legacy(conn: &Connection, sb: &SecretBox) -> Result<()> {
    let Some(id) = default_id(conn)? else {
        return Ok(());
    };
    let raw: Option<String> = conn.query_row(
        "SELECT legacy_sealed FROM gateway_keys WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    let Some(raw) = raw else {
        return Ok(());
    };
    let Some((n, c)) = raw.split_once(':') else {
        return Err(DbError::Other("Invalid legacy gateway ciphertext".into()));
    };
    let n = hex::decode(n).map_err(|_| DbError::Other("Invalid legacy nonce".into()))?;
    let c = hex::decode(c).map_err(|_| DbError::Other("Invalid legacy ciphertext".into()))?;
    let key = sb
        .open(&n, &c)
        .map_err(|_| DbError::Other("Cannot decrypt legacy gateway key".into()))?;
    conn.execute("UPDATE gateway_keys SET nonce=?1,cipher=?2,hint=?3,selector=?4,legacy_sealed=NULL WHERE id=?5",params![n,c,sessions::mask_key(&key),selector(&key),id])?;
    Ok(())
}

pub fn default_hint(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT hint FROM gateway_keys WHERE is_default=1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

/// Snapshot authentication work without retaining a database guard across Argon2.
pub fn auth_candidate(conn: &Connection, key: &str) -> Result<Option<(i64, String)>> {
    if let Some(sel) = selector(key) {
        let row = conn.query_row("SELECT id,key_hash FROM gateway_keys WHERE selector=?1",[sel],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if row.is_some() { return Ok(row); }
    }
    Ok(conn.query_row("SELECT id,key_hash FROM gateway_keys WHERE is_default=1 AND selector IS NULL",[],|r|Ok((r.get(0)?,r.get(1)?))).optional()?)
}
