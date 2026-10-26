//! Named gateway credentials. Settings compatibility targets only the default row.
use super::{now_ms, sessions, DbError, Result};
use crate::crypto::SecretBox;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Serialize)]
pub struct GatewayKey {
    pub id: i64,
    pub name: String,
    pub display_name: String,
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
        conn.prepare("SELECT g.id,g.name,g.hint,g.created_at_ms,CASE WHEN u.name IS NOT NULL THEN g.name || ' - ' || u.name ELSE g.name END FROM gateway_keys g LEFT JOIN users u ON u.id=g.user_id WHERE NOT EXISTS(SELECT 1 FROM settings s WHERE s.key='deleted_key:'||CAST(g.id AS TEXT)) ORDER BY g.id")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(GatewayKey {
                id: r.get(0)?,
                name: r.get(1)?,
                display_name: r.get(4)?,
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
        display_name: conn.query_row("SELECT g.name || ' - ' || u.name FROM gateway_keys g JOIN users u ON u.id=g.user_id WHERE g.id=?1", [id], |r| r.get(0))?,
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

/// Persist only encrypted recovery material for an existing identity credential.
/// The caller owns the create/audit transaction; the public lists never select it.
pub fn retain_secret(conn:&Connection,sb:&SecretBox,id:i64,key:&str)->Result<()> {
    let hash:Option<String>=conn.query_row("SELECT key_hash FROM gateway_keys WHERE id=?1",[id],|r|r.get(0)).optional()?;
    if !hash.is_some_and(|hash|verify_credential(key,&hash)) { return Err(DbError::Other("Credential recovery material does not match key".into())); }
    let (nonce,cipher)=sb.seal(key).map_err(|_|DbError::Other("Cannot encrypt gateway key".into()))?;
    conn.execute("UPDATE gateway_keys SET nonce=?2,cipher=?3 WHERE id=?1",params![id,nonce,cipher])?;
    Ok(())
}
pub fn can_reveal(conn:&Connection,id:i64)->Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM gateway_keys g WHERE g.id=?1 AND g.nonce IS NOT NULL AND length(g.nonce)>0 AND g.cipher IS NOT NULL AND length(g.cipher)>0 AND NOT EXISTS(SELECT 1 FROM settings s WHERE s.key='deleted_key:'||CAST(g.id AS TEXT)))",[id],|r|r.get(0))?)
}
pub fn reveal(conn: &Connection, sb: &SecretBox, id: i64) -> Result<Option<String>> {
    let row: Option<(Option<Vec<u8>>, Option<Vec<u8>>)> = conn
        .query_row(
            "SELECT nonce,cipher FROM gateway_keys g WHERE g.id=?1 AND NOT EXISTS(SELECT 1 FROM settings s WHERE s.key='deleted_key:'||CAST(g.id AS TEXT))",
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
    let remove=|conn:&Connection| -> Result<bool> {
        let removed=conn.execute("DELETE FROM gateway_keys WHERE id=?1",[id])?>0;
        super::identity::clear_self_pause(conn,id)?;
        Ok(removed)
    };
    if conn.is_autocommit() { super::in_tx(conn,remove) } else { remove(conn) }
}

/// SHA-256 is appropriate for a newly generated 244-bit random credential; legacy
/// human-chosen credentials retain their existing Argon2 verifier.
pub fn digest(key: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(key.as_bytes())))
}

pub fn verify_credential(key: &str, hash: &str) -> bool {
    if hash.starts_with("sha256:") {
        let expected = digest(key);
        if expected.len() != hash.len() { return false; }
        expected.bytes().zip(hash.bytes()).fold(0u8, |diff,(a,b)| diff | (a ^ b)) == 0
    } else {
        sessions::verify_password(key, hash)
    }
}

pub fn check(conn: &Connection, key: &str) -> Result<bool> {
    if key.trim().is_empty() { return Ok(false); }
    let Some((id, hash)) = auth_candidate(conn,key)? else { return Ok(false); };
    Ok(verify_credential(key,&hash) && super::identity::context(conn,id)?.is_some())
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
