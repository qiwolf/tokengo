//! 面板登录凭据。
//!
//! 口令用 argon2 慢哈希存库（明文绝不落库）；登录成功后发一个随机 token 存
//! `panel_sessions`，面板请求带这个 token。token 有有效期，过期即失效。

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rusqlite::{params, Connection, OptionalExtension};

use crate::crypto::mask;

use super::{now_ms, DbError, Result};

/// 会话有效期：30 天。自家面板，别天天登。
pub const SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| DbError::Other(format!("口令哈希失败: {e}")))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// 改面板口令。
pub fn set_panel_password(conn: &Connection, password: &str) -> Result<()> {
    if password.chars().count() < 4 {
        return Err(DbError::Other("口令太短（至少 4 位）".into()));
    }
    let h = hash_password(password)?;
    super::settings::set(conn, super::settings::PANEL_PASSWORD_HASH, &h)?;
    // 改口令 = 踢掉所有已登录的会话
    revoke_all(conn)?;
    Ok(())
}

/// 校验面板口令。没设过口令时返回 false（拒绝登录，不是放行）。
pub fn check_panel_password(conn: &Connection, password: &str) -> Result<bool> {
    match super::settings::get(conn, super::settings::PANEL_PASSWORD_HASH)? {
        None => Ok(false),
        Some(h) => Ok(verify_password(password, &h)),
    }
}

pub fn has_panel_password(conn: &Connection) -> Result<bool> {
    Ok(super::settings::get(conn, super::settings::PANEL_PASSWORD_HASH)?.is_some())
}

/// 登录成功 → 发一个 token。
pub fn create_session(conn: &Connection, client_ip: Option<&str>) -> Result<String> {
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let now = now_ms();
    conn.execute(
        "INSERT INTO panel_sessions (token, created_at_ms, expires_at_ms, client_ip)
         VALUES (?1, ?2, ?3, ?4)",
        params![token, now, now + SESSION_TTL_MS, client_ip],
    )?;
    Ok(token)
}

/// 校验 token 有效且未过期。
pub fn check_session(conn: &Connection, token: &str) -> Result<bool> {
    if token.trim().is_empty() {
        return Ok(false);
    }
    let exp: Option<i64> = conn
        .query_row(
            "SELECT expires_at_ms FROM panel_sessions WHERE token = ?1",
            [token],
            |r| r.get(0),
        )
        .optional()?;
    Ok(matches!(exp, Some(e) if e > now_ms()))
}

pub fn revoke_session(conn: &Connection, token: &str) -> Result<()> {
    conn.execute("DELETE FROM panel_sessions WHERE token = ?1", [token])?;
    Ok(())
}

pub fn revoke_all(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM panel_sessions", [])?;
    Ok(())
}

/// 清掉过期会话。启动时和每次新建会话时顺手调一下。
pub fn purge_expired(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM panel_sessions WHERE expires_at_ms <= ?1",
        [now_ms()],
    )?)
}

/// 面板上展示用：在线会话数。
pub fn active_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM panel_sessions WHERE expires_at_ms > ?1",
        [now_ms()],
        |r| r.get(0),
    )?)
}

/// 对外统一 key：客户端拿它连网关。只存哈希，库里拿不回明文。
pub fn set_gateway_key(conn: &Connection, key: &str) -> Result<()> {
    if key.trim().is_empty() {
        return Err(DbError::Other("网关 key 不能为空".into()));
    }
    let h = hash_password(key)?;
    super::settings::set(conn, super::settings::GATEWAY_KEY_HASH, &h)
}

pub fn check_gateway_key(conn: &Connection, key: &str) -> Result<bool> {
    if key.trim().is_empty() {
        return Ok(false);
    }
    match super::settings::get(conn, super::settings::GATEWAY_KEY_HASH)? {
        None => Ok(false),
        Some(h) => Ok(verify_password(key, &h)),
    }
}

/// 日志里展示用：库里那把网关 key 的掩码。存的是哈希，所以只能这样表示。
pub fn gateway_key_hint(conn: &Connection) -> Result<Option<String>> {
    Ok(super::settings::get(conn, super::settings::GATEWAY_KEY_HASH)?
        .map(|h| format!("{}（只存了哈希，不可回显）", mask(&h[..16.min(h.len())]))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn password_hash_is_not_the_password_and_verifies() {
        let h = hash_password("test-password").unwrap();
        assert!(!h.contains("test-password"), "哈希里不能出现明文");
        assert!(verify_password("test-password", &h));
        assert!(!verify_password("test-password1", &h));
    }

    #[test]
    fn same_password_hashes_differently() {
        let a = hash_password("test-password").unwrap();
        let b = hash_password("test-password").unwrap();
        assert_ne!(a, b, "每次都要新盐");
    }

    #[test]
    fn no_password_set_means_login_is_refused() {
        let conn = open_in_memory().unwrap();
        assert!(!has_panel_password(&conn).unwrap());
        assert!(
            !check_panel_password(&conn, "").unwrap(),
            "没设口令时不能空口令就放行"
        );
        assert!(!check_panel_password(&conn, "anything").unwrap());
    }

    #[test]
    fn set_then_login() {
        let conn = open_in_memory().unwrap();
        set_panel_password(&conn, "test-password").unwrap();
        assert!(has_panel_password(&conn).unwrap());
        assert!(check_panel_password(&conn, "test-password").unwrap());
        assert!(!check_panel_password(&conn, "wrong").unwrap());
    }

    #[test]
    fn changing_password_kicks_everyone_out() {
        let conn = open_in_memory().unwrap();
        set_panel_password(&conn, "test-password").unwrap();
        let t = create_session(&conn, Some("192.0.2.9")).unwrap();
        assert!(check_session(&conn, &t).unwrap());
        set_panel_password(&conn, "test-password2").unwrap();
        assert!(!check_session(&conn, &t).unwrap(), "改口令必须踢掉旧会话");
    }

    #[test]
    fn session_lifecycle() {
        let conn = open_in_memory().unwrap();
        let t = create_session(&conn, None).unwrap();
        assert!(check_session(&conn, &t).unwrap());
        assert!(!check_session(&conn, "made-up").unwrap());
        assert!(!check_session(&conn, "").unwrap());

        revoke_session(&conn, &t).unwrap();
        assert!(!check_session(&conn, &t).unwrap());
    }

    #[test]
    fn expired_sessions_stop_working_and_get_purged() {
        let conn = open_in_memory().unwrap();
        let t = create_session(&conn, None).unwrap();
        // 手动把它改成已过期
        conn.execute(
            "UPDATE panel_sessions SET expires_at_ms = ?1 WHERE token = ?2",
            params![now_ms() - 1, t],
        )
        .unwrap();
        assert!(!check_session(&conn, &t).unwrap());
        assert_eq!(active_count(&conn).unwrap(), 0);
        assert_eq!(purge_expired(&conn).unwrap(), 1);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM panel_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn gateway_key_is_hashed_not_stored_plain() {
        let conn = open_in_memory().unwrap();
        assert!(!check_gateway_key(&conn, "anything").unwrap(), "没配就不放行");
        set_gateway_key(&conn, "test-password").unwrap();
        assert!(check_gateway_key(&conn, "test-password").unwrap());
        assert!(!check_gateway_key(&conn, "nope").unwrap());

        // 反证：整张 settings 表里搜不到明文
        let all = super::super::settings::all(&conn).unwrap();
        for (_, v) in all {
            assert!(!v.contains("test-password"), "库里出现了网关 key 明文");
        }
    }

    #[test]
    fn short_panel_password_is_rejected() {
        let conn = open_in_memory().unwrap();
        assert!(set_panel_password(&conn, "ab").is_err());
    }
}
