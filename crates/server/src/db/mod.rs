//! 数据库层：连接、迁移、以及各仓储模块。
//!
//! SQLite（rusqlite，bundled 编译，不依赖系统库），与现网 FreeLLMAPI 同一种库，
//! 将来搬数据和对拍都方便。

pub mod passthrough;
pub mod provider_channels;
pub mod provider_pool;
pub mod accounts;
pub mod operations_tokens;
pub mod identity;
pub mod lifecycle;
pub mod billing;
pub mod billing_management;
pub mod faults;
pub mod catalog;
pub mod gateway_keys;
pub mod kinds;
pub mod channels;
pub mod model_routes;
pub mod scores;
pub mod policy;
pub mod maintenance;
pub mod requests;
pub mod routing;
pub mod route_policy;
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
/// 为什么必须有这个：一条渠道的保存是四步（主行 + 密钥 + 请求头 + 模型名）。
/// 不做事务的话，第 3 步失败会留下一条"名字在、请求头丢了"的半截渠道，
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
        let expected = [
            "passthrough_routes",
            "passthrough_headers",
            "passthrough_acl",
            "attempts",
            "gateway_keys",
            "channel_catalog",
            "channel_headers",
            "channel_models",
            "channel_secrets",
            "channels",
            "model_group_channels",
            "model_groups",
            "model_kinds",
            "model_type_checks",
            "model_vision_checks",
            "model_cooldowns",
            "model_invocations",
            "panel_sessions",
            "panel_accounts",
            "provider_channels",
            "provider_connections",
            "provider_credentials",
            "source_credentials",
            "credential_affinity",
            "user_virtual_models",
            "shared_virtual_models",
            "user_virtual_targets",
            "user_virtual_windows",
            "channel_reasoning_catalog",
            "channel_reasoning_settings",
            "account_sessions",
            "channel_context_overrides",
            "registry_associations",
            "registry_context",
            "registry_context_status",
            "requests",
            "routing_state",
            "schema_migrations",
            "settings",
            "channel_faults",
            // Route B adds identity, immutable pricing/ledger/budget and quota state.
            "users",
            "audit_events",
            "billing_prices",
            "billing_request_prices",
            "billing_budgets",
            "billing_ledger",
            "billing_audit",
            "billing_allocations",
            "quota_events",
            "protocol_sessions",
            "protocol_artifacts",
            "operations_tokens",
            "operations_instance",
            "oauth_hosts",
            "oauth_accounts",
            "oauth_pending",
            "oauth_channel_bindings",
        ];
        let actual: std::collections::BTreeSet<_> = got.iter().map(String::as_str).collect();
        let expected: std::collections::BTreeSet<_> = expected.into_iter().collect();
        assert_eq!(actual, expected, "库表集合与当前设计不符");
    }

    /// 关键反证：**免费模型目录相关的表一张都不该有。**
    /// 用户明确不要无关目录同步；Route B 的 users 属于已授权的身份体系。
    ///
    /// `channel_catalog` 不在禁列里，因为它是**另一回事**：不是随包发的免费模型目录，
    /// 而是用户按一次「问一次上游」之后，上游当场报回来的清单存下来的一行（v0.7.0）。
    /// 一张渠道一行清单，量随渠道数走，不随上游目录膨胀。
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
            "INSERT INTO channel_headers (channel_id, name, value) VALUES (999, 'X', 'y')",
            [],
        );
        assert!(r.is_err(), "外键没生效，插了个不存在的渠道也不报错");
    }

    #[test]
    fn cascade_delete_cleans_children() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO channels (id, name, base_url) VALUES (1, '硅基流动', 'https://api.siliconflow.cn/v1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channel_headers (channel_id, name, value) VALUES (1, 'X-Test', '{{uuid}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channel_models (channel_id, model_name, upstream_model) VALUES (1, 'auto', 'BAAI/bge-m3')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM channels WHERE id = 1", []).unwrap();
        for t in ["channel_headers", "channel_models"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{t} 里的子行没跟着删");
        }
    }

    /// 旧库升级：CT112 上的库是「腿」时代建的，改名迁移必须在真库上也能过。
    ///
    /// 这段**故意写旧名字**（legs / leg_id / active_leg_id）—— 别用
    /// `scratch/rename_leg_to_channel.py` 批量改这一段，改了这条测试就没意义了。
    #[test]
    fn upgrading_a_leg_era_database_renames_things_and_keeps_the_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                 version    INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        // 只应用改名之前的两个版本，装成一个老库
        for (version, sql) in schema::MIGRATIONS.iter().take(2) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                [version],
            )
            .unwrap();
        }

        // 按旧名字塞进数据：一条渠道 + 一个组 + 一条请求 + 一次尝试 + 一条路由状态
        conn.execute(
            "INSERT INTO legs (id, name, base_url) VALUES (1, '硅基流动', 'https://api.siliconflow.cn/v1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO leg_models (leg_id, model_name, upstream_model) VALUES (1, 'auto', 'BAAI/bge-m3')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO model_groups (name) VALUES ('auto')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO model_group_legs (group_name, leg_id, ordinal) VALUES ('auto', 1, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO requests (id, ts_ms, endpoint, model, status, leg_id)
             VALUES (1, 1, '/v1/chat/completions', 'auto', 'ok', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attempts (request_id, ordinal, leg_id, outcome) VALUES (1, 0, 1, 'ok')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO routing_state (group_name, active_leg_id) VALUES ('auto', 1)",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        // 表名换成渠道，旧名一个都不许留
        let got = table_names(&conn);
        for want in [
            "channels",
            "channel_secrets",
            "channel_headers",
            "channel_models",
            "model_group_channels",
        ] {
            assert!(got.contains(&want.to_string()), "升级后缺表 {want}：{got:?}");
        }
        for gone in [
            "legs",
            "leg_secrets",
            "leg_headers",
            "leg_models",
            "model_group_legs",
        ] {
            assert!(!got.contains(&gone.to_string()), "旧表 {gone} 还在：{got:?}");
        }

        // 数据一条不少，列名也换了
        let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(one("SELECT COUNT(*) FROM channels"), 1);
        assert_eq!(one("SELECT COUNT(*) FROM channel_models"), 0, "废弃auto映射在v19清理，历史账保留");
        assert_eq!(one("SELECT COUNT(*) FROM model_group_channels"), 1);
        assert_eq!(one("SELECT channel_id FROM requests WHERE id = 1"), 1);
        assert_eq!(one("SELECT channel_id FROM attempts"), 1);
        assert_eq!(one("SELECT active_channel_id FROM routing_state"), 1);

        // 外键指向的是新表名：插个不存在的渠道必须报错
        let r = conn.execute(
            "INSERT INTO channel_headers (channel_id, name, value) VALUES (999, 'X', 'y')",
            [],
        );
        assert!(r.is_err(), "改名后外键没跟过来");

        // 级联删除也还认新表名
        conn.execute("DELETE FROM channels WHERE id = 1", []).unwrap();
        assert_eq!(
            one("SELECT COUNT(*) FROM model_group_channels"),
            0,
            "改名后级联删除失效了"
        );
    }
}

pub mod user_virtual_routes;

pub mod shared_virtual_routes;
pub mod model_access;
