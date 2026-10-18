//! 设置仓储：路由参数、面板口令、对外统一 key 等，都存在这张键值表里。

use rusqlite::{params, Connection, OptionalExtension};

use super::Result;

/// 面板口令的 argon2 哈希。**明文绝不入库。**
pub const PANEL_PASSWORD_HASH: &str = "panel_password_hash";
/// 对外统一 key 的哈希（客户端拿这个 key 连网关）。
pub const GATEWAY_KEY_HASH: &str = "gateway_key_hash";

/// 对外 key 的明文（用主密钥加密后存这里）。
///
/// 为什么留一份可解密的：用户要求「生成一次之后就一直能用，点一下就能复制」——
/// 只存哈希的话，丢了就只能重新生成（旧的当场失效），每次都要重配客户端。
/// 存的是密文（`nonce_hex:cipher_hex`），主密钥不在库里，泄库不等于泄 key。
pub const GATEWAY_KEY_SEALED: &str = "gateway_key_sealed";

pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

pub fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}

pub fn all(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
    let v = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

pub fn get_bool(conn: &Connection, key: &str, default: bool) -> Result<bool> {
    Ok(get(conn, key)?.map(|v| v == "1" || v == "true").unwrap_or(default))
}

pub fn set_bool(conn: &Connection, key: &str, v: bool) -> Result<()> {
    set(conn, key, if v { "1" } else { "0" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn set_get_overwrite_delete() {
        let conn = open_in_memory().unwrap();
        assert_eq!(get(&conn, "x").unwrap(), None);
        set(&conn, "x", "1").unwrap();
        assert_eq!(get(&conn, "x").unwrap().unwrap(), "1");
        set(&conn, "x", "2").unwrap();
        assert_eq!(get(&conn, "x").unwrap().unwrap(), "2");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        delete(&conn, "x").unwrap();
        assert_eq!(get(&conn, "x").unwrap(), None);
    }

    #[test]
    fn bools_are_parsed_leniently() {
        let conn = open_in_memory().unwrap();
        assert!(get_bool(&conn, "b", true).unwrap(), "缺省走默认值");
        set_bool(&conn, "b", false).unwrap();
        assert!(!get_bool(&conn, "b", true).unwrap());
        set(&conn, "b", "true").unwrap();
        assert!(get_bool(&conn, "b", false).unwrap());
    }
}
