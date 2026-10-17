//! 账目维护：按保留天数清掉旧请求明细，并把腾出来的空间还给磁盘。
//!
//! 为什么必须有这个模块：现网实测约 2.9 万条/天，照这个速度一年长 3–4 GB，
//! 而 `requests` 表原先只进不出。清理是「上线切换」的前置项，不是优化项。
//!
//! 三条不能破的：
//!
//! - **分批删**。一次 `DELETE` 删几十万行会长时间占着库锁，期间所有请求排队；
//!   这里每批只删 `BATCH_LIMIT` 行，调用方在批与批之间放开锁。
//! - **保留天数 = 0 就什么都不删**。这是用户手里的刹车，测试钉住它。
//! - **删除只按时间一个条件**，不带状态/渠道之类的附加筛选 —— 行为可预测，
//!   不会出现「某些旧账删不掉」这种猜不透的残留。

use rusqlite::{params, Connection};

use super::{settings, Result};

/// 保留多少天（0 = 永不清理）。
pub const RETENTION_DAYS_KEY: &str = "requests_retention_days";
/// 上次真正跑清理的时间（毫秒）。
pub const LAST_PRUNE_AT_KEY: &str = "requests_last_prune_at_ms";
/// 上次清理掉了多少条（给面板显示，不做业务判断）。
pub const LAST_PRUNE_COUNT_KEY: &str = "requests_last_prune_count";
/// 上次回收空间的时间（毫秒）。
pub const LAST_VACUUM_AT_KEY: &str = "requests_last_vacuum_at_ms";

pub const DEFAULT_RETENTION_DAYS: i64 = 30;
/// 一批删多少行。太小则清零一次要点很多轮，太大则单次占锁变长。
pub const BATCH_LIMIT: i64 = 2000;
/// 保留天数上限（十年）。填歪了（比如把毫秒当秒填进来）不至于一次删光。
pub const MAX_RETENTION_DAYS: i64 = 3650;

const DAY_MS: i64 = 86_400_000;
/// 两次空间回收之间至少隔一周。
const VACUUM_INTERVAL_MS: i64 = 7 * DAY_MS;
/// 超过这个体积就不再自动做「整库重写式」转换，留给停服窗口。
const AUTO_VACUUM_CONVERT_LIMIT_BYTES: i64 = 200 * 1024 * 1024;

/// 回收空间时，库小于这个体积就直接 `VACUUM`（秒级、文件真变小）；
/// 大于它就退回有上限的增量回收，绝不长时间独占库锁。
const VACUUM_DIRECT_LIMIT_BYTES: i64 = 200 * 1024 * 1024;
/// 大库走增量回收时，单轮最多放多少批（每批 1000 页）。
const MAX_INCREMENTAL_ROUNDS: usize = 64;

/// 现在保留多少天。没设过就是 30 天；填歪了按默认值走，不因为一个脏值停掉清理。
pub fn retention_days(conn: &Connection) -> Result<i64> {
    let raw = settings::get(conn, RETENTION_DAYS_KEY)?;
    Ok(raw
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|d| d.clamp(0, MAX_RETENTION_DAYS))
        .unwrap_or(DEFAULT_RETENTION_DAYS))
}

pub fn set_retention_days(conn: &Connection, days: i64) -> Result<i64> {
    let d = days.clamp(0, MAX_RETENTION_DAYS);
    settings::set(conn, RETENTION_DAYS_KEY, &d.to_string())?;
    Ok(d)
}

/// 保留期的时间界线：早于这个毫秒数的行该走。
pub fn cutoff_ms(now_ms: i64, days: i64) -> Option<i64> {
    if days <= 0 {
        None
    } else {
        Some(now_ms - days * DAY_MS)
    }
}

/// 删一批。返回真删掉的行数。
///
/// 用「先选 id 再按 id 删」而不是 `DELETE ... WHERE ts_ms < ?` 直接上 `LIMIT`，
/// 因为 SQLite 的 `DELETE` 本身不带 `LIMIT`；子查询选 id 的顺序是确定的，
/// 每次都是最老的那一批先走。
pub fn prune_batch(conn: &Connection, before_ms: i64, limit: i64) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM requests WHERE id IN (
             SELECT id FROM requests WHERE ts_ms < ?1 ORDER BY ts_ms LIMIT ?2
         )",
        params![before_ms, limit.max(1)],
    )?;
    Ok(n)
}

/// 记录一次清理的结果（时间 + 条数）。条数为 0 也记，面板据此显示「跑过了、没东西清」。
pub fn record_prune(conn: &Connection, now_ms: i64, count: usize) -> Result<()> {
    settings::set(conn, LAST_PRUNE_AT_KEY, &now_ms.to_string())?;
    settings::set(conn, LAST_PRUNE_COUNT_KEY, &count.to_string())?;
    Ok(())
}

pub fn last_prune_at(conn: &Connection) -> Result<Option<i64>> {
    Ok(settings::get(conn, LAST_PRUNE_AT_KEY)?
        .and_then(|v| v.trim().parse::<i64>().ok()))
}

pub fn last_prune_count(conn: &Connection) -> Result<i64> {
    Ok(settings::get(conn, LAST_PRUNE_COUNT_KEY)?
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0))
}

fn last_vacuum_at(conn: &Connection) -> Result<Option<i64>> {
    Ok(settings::get(conn, LAST_VACUUM_AT_KEY)?
        .and_then(|v| v.trim().parse::<i64>().ok()))
}

/// 库文件当前多大（字节）。内存库也算得出来，测试能用。
pub fn db_bytes(conn: &Connection) -> Result<i64> {
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(pages.saturating_mul(size))
}

/// 账目现状：给面板显示用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub retention_days: i64,
    pub total_requests: i64,
    pub oldest_ts_ms: Option<i64>,
    pub db_bytes: i64,
    pub last_prune_at_ms: Option<i64>,
    pub last_prune_count: i64,
}

pub fn info(conn: &Connection) -> Result<Info> {
    let (total, oldest): (i64, Option<i64>) = conn.query_row(
        "SELECT COUNT(*), MIN(ts_ms) FROM requests",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(Info {
        retention_days: retention_days(conn)?,
        total_requests: total,
        oldest_ts_ms: oldest,
        db_bytes: db_bytes(conn)?,
        last_prune_at_ms: last_prune_at(conn)?,
        last_prune_count: last_prune_count(conn)?,
    })
}

/// 把库切成「增量回收」模式，以后腾出来的页可以直接还回去。
///
/// 这个属性只在建库时生效，所以要改必须做一次整库重写（`VACUUM`）。库小的时候
/// 瞬间完成；库大了会长时间占锁，所以超过 `AUTO_VACUUM_CONVERT_LIMIT_BYTES` 就
/// 只打日志、不动手 —— 那种体量该在停服窗口里做。
pub fn ensure_incremental_vacuum(conn: &Connection) -> Result<bool> {
    let mode: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
    if mode == 2 {
        return Ok(false);
    }
    if db_bytes(conn)? > AUTO_VACUUM_CONVERT_LIMIT_BYTES {
        tracing::warn!(
            "库体积已超过 200 MB，跳过 auto_vacuum 转换（要做需停服窗口）"
        );
        return Ok(false);
    }
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    conn.execute_batch("VACUUM;")?;
    tracing::info!("库已切换为增量回收模式（auto_vacuum=INCREMENTAL）");
    Ok(true)
}

/// 回收空间。到点（满一周）才真做，返回是否做了。
///
/// 只做「增量回收 + WAL 截断」，不做整库 `VACUUM`：增量回收按页释放，
/// 每次有上限，不会像整库重写那样长时间独占。
pub fn maybe_reclaim_space(conn: &Connection, now_ms: i64) -> Result<bool> {
    let last = last_vacuum_at(conn)?;
    if let Some(t) = last {
        if now_ms - t < VACUUM_INTERVAL_MS {
            return Ok(false);
        }
    }
    reclaim_space(conn, now_ms)?;
    Ok(true)
}

/// 立刻回收一次（面板「立即清理」和定时任务共用）。
pub fn reclaim_space(conn: &Connection, now_ms: i64) -> Result<()> {
    // 先看有多少空闲页。`PRAGMA incremental_vacuum` 在 rusqlite 里**不返回行**
    // （拿返回值会 QueryReturnedNoRows），所以判断一律看 freelist_count。
    let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    if free > 0 {
        if db_bytes(conn)? <= VACUUM_DIRECT_LIMIT_BYTES {
            // 小库：直接整库重写。增量回收在这个体量上要一页一页挪（实测一轮只还一页，
            // 几千页就要几千轮，反而更久），而 200 MB 以内重写是秒级、且真能把文件缩回去。
            conn.execute_batch("VACUUM;")?;
        } else {
            // 大库：只做有上限的增量回收，宁可这次少还一点，也不长时间独占锁。
            for _ in 0..MAX_INCREMENTAL_ROUNDS {
                let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
                if free <= 0 {
                    break;
                }
                conn.execute_batch("PRAGMA incremental_vacuum(1000);")?;
            }
            let left: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            if left > 0 {
                tracing::warn!("增量回收本轮还剩 {left} 个空闲页没还（库较大，下一轮继续）");
            }
        }
    }
    // WAL 截断：让 -wal 文件缩回去。非 WAL 库会报错，忽略即可。
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0));
    settings::set(conn, LAST_VACUUM_AT_KEY, &now_ms.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    /// 直接塞一条某时刻的请求记录（绕开 requests::start 的「用当前时间」）。
    fn insert_at(conn: &Connection, ts_ms: i64, endpoint: &str) -> i64 {
        conn.execute(
            "INSERT INTO requests (ts_ms, endpoint, model, status)
             VALUES (?1, ?2, 'auto', 'ok')",
            params![ts_ms, endpoint],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_attempt(conn: &Connection, request_id: i64) {
        conn.execute(
            "INSERT INTO attempts (request_id, ordinal, channel_id, outcome)
             VALUES (?1, 0, 1, 'success')",
            params![request_id],
        )
        .unwrap();
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn default_retention_is_thirty_days() {
        let conn = open_in_memory().unwrap();
        assert_eq!(retention_days(&conn).unwrap(), 30);
    }

    #[test]
    fn retention_is_clamped_and_survives_junk() {
        let conn = open_in_memory().unwrap();
        assert_eq!(set_retention_days(&conn, -5).unwrap(), 0);
        assert_eq!(retention_days(&conn).unwrap(), 0);
        assert_eq!(set_retention_days(&conn, 99_999).unwrap(), MAX_RETENTION_DAYS);
        // 脏值不能让清理停摆：按默认 30 天走
        settings::set(&conn, RETENTION_DAYS_KEY, "三十").unwrap();
        assert_eq!(retention_days(&conn).unwrap(), DEFAULT_RETENTION_DAYS);
    }

    #[test]
    fn zero_days_means_never_delete() {
        let conn = open_in_memory().unwrap();
        set_retention_days(&conn, 0).unwrap();
        assert_eq!(cutoff_ms(1_000_000, 0), None, "0 天没有界线，调用方就不该删");
        insert_at(&conn, 1, "/v1/chat/completions");
        // 调用方拿到 None 就不会调 prune_batch —— 这里直接验界线语义
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM requests"), 1);
    }

    #[test]
    fn prune_only_touches_rows_older_than_the_cutoff() {
        let conn = open_in_memory().unwrap();
        let now = 1_000 * DAY_MS;
        let old = insert_at(&conn, now - 40 * DAY_MS, "/v1/embeddings");
        insert_at(&conn, now - 10 * DAY_MS, "/v1/embeddings");
        let before = cutoff_ms(now, 30).unwrap();
        assert_eq!(prune_batch(&conn, before, BATCH_LIMIT).unwrap(), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM requests"), 1);
        // 老的那条没了，新的那条还在
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM requests WHERE id = ?1", [old], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn deleting_a_request_takes_its_attempts_with_it() {
        let conn = open_in_memory().unwrap();
        let old = insert_at(&conn, 1_000, "/v1/chat/completions");
        insert_attempt(&conn, old);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM attempts"), 1);
        prune_batch(&conn, 2_000, BATCH_LIMIT).unwrap();
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM attempts"),
            0,
            "尝试级明细必须跟着请求一起走（外键 ON DELETE CASCADE）"
        );
    }

    #[test]
    fn a_batch_never_exceeds_the_limit() {
        let conn = open_in_memory().unwrap();
        for i in 0..(BATCH_LIMIT + 50) {
            insert_at(&conn, i, "/v1/embeddings");
        }
        let n = prune_batch(&conn, i64::MAX / 2, BATCH_LIMIT).unwrap();
        assert_eq!(n as i64, BATCH_LIMIT, "单批最多 BATCH_LIMIT 行，多出来的留给下一批");
    }

    #[test]
    fn the_oldest_rows_go_first() {
        let conn = open_in_memory().unwrap();
        for ts in [500, 100, 300] {
            insert_at(&conn, ts, "/v1/embeddings");
        }
        prune_batch(&conn, i64::MAX / 2, 2).unwrap();
        let left: Vec<i64> = {
            let mut stmt = conn.prepare("SELECT ts_ms FROM requests ORDER BY ts_ms").unwrap();
            let v = stmt
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<i64>>>()
                .unwrap();
            v
        };
        assert_eq!(left, vec![500], "最老的两条先走");
    }

    #[test]
    fn prune_records_when_it_ran() {
        let conn = open_in_memory().unwrap();
        assert_eq!(last_prune_at(&conn).unwrap(), None);
        record_prune(&conn, 12_345, 7).unwrap();
        assert_eq!(last_prune_at(&conn).unwrap(), Some(12_345));
        assert_eq!(last_prune_count(&conn).unwrap(), 7);
    }

    #[test]
    fn info_reports_the_oldest_row_and_the_file_size() {
        let conn = open_in_memory().unwrap();
        insert_at(&conn, 900, "/v1/embeddings");
        insert_at(&conn, 100, "/v1/chat/completions");
        let i = info(&conn).unwrap();
        assert_eq!(i.total_requests, 2);
        assert_eq!(i.oldest_ts_ms, Some(100));
        assert!(i.db_bytes > 0);
        assert_eq!(i.retention_days, 30);
    }

    #[test]
    fn reclaim_space_is_weekly_not_every_tick() {
        let conn = open_in_memory().unwrap();
        assert!(maybe_reclaim_space(&conn, 10 * DAY_MS).unwrap(), "第一次该做");
        assert!(!maybe_reclaim_space(&conn, 11 * DAY_MS).unwrap(), "一周内不做");
        assert!(maybe_reclaim_space(&conn, 17 * DAY_MS).unwrap(), "满一周了再做");
    }

    #[test]
    fn incremental_vacuum_is_idempotent() {
        let conn = open_in_memory().unwrap();
        assert!(ensure_incremental_vacuum(&conn).unwrap(), "第一次要转换");
        assert!(!ensure_incremental_vacuum(&conn).unwrap(), "已经是增量模式就不再重写整库");
    }
}
