//! 数据库层：连接、迁移、以及各仓储模块。
//!
//! SQLite（rusqlite，bundled 编译，不依赖系统库），与现网 FreeLLMAPI 同一种库，
//! 将来搬数据和对拍都方便。

pub mod groups;
pub mod legs;
pub mod requests;
pub mod routing;
pub mod schema;
pub mod sessions;
pub mod settings;

use std::path::Path;

use rusqlite::Connection;

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("数据库错误: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Other(String),
}

/// 打开（或新建）库并跑迁移。
pub fn open(path: impl AsRef<Path>) -> Result<Connection> {
    let conn = Connection::open(path)?;
    prepare(&conn)?;
    Ok(conn)
}

/// 内存库，测试用。
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    prepare(&conn)?;
    Ok(conn)
}

fn prepare(conn: &Connection) -> Result<()> {
    // WAL 让读不阻塞写；外键约束必须显式打开，SQLite 默认是关的
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    migrate(conn)?;
    Ok(())
}

/// 按版本号顺序执行未执行的迁移。
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version    INTEGER PRIMARY KEY,
             applied_at TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )?;

    for (version, sql) in schema::MIGRATIONS {
        let done: i64 = conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
            [version],
            |r| r.get(0),
        )?;
        if done > 0 {
            continue;
        }
        conn.execute_batch("BEGIN")?;
        match conn.execute_batch(sql) {
            Ok(()) => {
                conn.execute(
                    "INSERT INTO schema_migrations (version) VALUES (?1)",
                    [version],
                )?;
                conn.execute_batch("COMMIT")?;
                tracing::info!("已应用迁移 v{version}");
            }
            Err(e) => {
                conn.execute_batch("ROLLBACK").ok();
                return Err(DbError::Other(format!("迁移 v{version} 失败: {e}")));
            }
        }
    }
    Ok(())
}

/// 把一段操作放进一个事务。中途失败整体回滚。
///
/// 为什么必须有这个：一条腿的保存是四步（主行 + 密钥 + 请求头 + 模型名）。
/// 不做事务的话，第 3 步失败会留下一条"名字在、请求头丢了"的半截腿，
/// 用户看到的是保存失败，磁盘上却已经改了东西。
pub fn in_tx<T>(conn: &Connection, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let tx = conn.unchecked_transaction()?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

/// 当前时间，unix 毫秒。全项目统一用它，别各写各的。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                // sqlite_sequence 是 AUTOINCREMENT 自动建的内务表，不是我们的表
                "SELECT name FROM sqlite_master WHERE type='table'
                   AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        let v = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        v
    }

    #[test]
    fn migrations_create_the_expected_tables() {
        let conn = open_in_memory().unwrap();
        let got = table_names(&conn);
        for want in [
            "attempts",
            "leg_headers",
            "leg_models",
            "leg_secrets",
            "legs",
            "model_group_legs",
            "model_groups",
            "panel_sessions",
            "requests",
            "routing_state",
            "schema_migrations",
            "settings",
        ] {
            assert!(got.contains(&want.to_string()), "缺表 {want}，实际有 {got:?}");
        }
        assert_eq!(got.len(), 12, "表数与设计不符：{got:?}");
    }

    /// 关键反证：**免费模型目录相关的表一张都不该有。**
    /// 用户明确不要目录同步，那些表也是库持续膨胀的来源。
    #[test]
    fn no_catalog_tables_leak_in() {
        let conn = open_in_memory().unwrap();
        let got = table_names(&conn);
        for forbidden in [
            "models",
            "embedding_models",
            "media_models",
            "quirks",
            "quirk_targets",
            "fallback_config",
            "catalog_model_tombstones",
            "provider_quota_state",
            "response_cache",
            "idempotency_claims",
            "url_tokens",
            "profiles",
            "users",
        ] {
            assert!(
                !got.contains(&forbidden.to_string()),
                "不该出现的表出现了：{forbidden}"
            );
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, schema::MIGRATIONS.len() as i64);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let conn = open_in_memory().unwrap();
        let r = conn.execute(
            "INSERT INTO leg_headers (leg_id, name, value) VALUES (999, 'X', 'y')",
            [],
        );
        assert!(r.is_err(), "外键没生效，插了个不存在的腿也不报错");
    }

    #[test]
    fn cascade_delete_cleans_children() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO legs (id, name, base_url) VALUES (1, '硅基流动', 'https://api.siliconflow.cn/v1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO leg_headers (leg_id, name, value) VALUES (1, 'X-Test', '{{uuid}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO leg_models (leg_id, model_name, upstream_model) VALUES (1, 'auto', 'BAAI/bge-m3')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM legs WHERE id = 1", []).unwrap();
        for t in ["leg_headers", "leg_models"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{t} 里的子行没跟着删");
        }
    }
}
