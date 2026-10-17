//! 后台清扫员：按保留天数清掉旧账，并定期把腾出来的空间还给磁盘。
//!
//! 为什么单独一个后台任务而不是「写请求时顺手删」：清理要成批做，混在请求路径里
//! 会让某一次请求莫名其妙多花几百毫秒。这里单独跑，用户感觉不到。
//!
//! 三条纪律：
//!
//! - **每批之间放开库锁**。一批 2000 行删完就 drop 掉守卫，睡一会儿再来下一批，
//!   否则一次清几十万行会让所有请求排队。
//! - **一天最多清一次**。靠 `LAST_PRUNE_AT_KEY` 判断，重启不会把节奏打乱
//!   （重启后立刻又清一遍纯属浪费）。
//! - **保留天数 0 就不删**，只打一行日志说明当前是关着的。
//!
//! 注意：这里用的是 `std::thread::sleep` 之外的方式 —— 批间用 `tokio::time::sleep`
//! 让出执行权，别把整个运行时堵住。锁守卫在 await 之前必须已经 drop。

use std::time::Duration;

use crate::db::maintenance::{self, BATCH_LIMIT};
use crate::db::now_ms;
use crate::SharedState;

/// 多久醒一次看看该不该干活。清理本身一天一次，醒得勤是为了「重启后尽快补上」。
const TICK: Duration = Duration::from_secs(3600);
/// 两次真正清理之间至少隔多久。
const PRUNE_INTERVAL_MS: i64 = 86_400_000;
/// 单次清理最多跑多少批 —— 防止一个巨大的旧库让后台任务跑一整天。
/// 2000 × 500 = 100 万行/次，够把一年的账清完了。
const MAX_BATCHES_PER_RUN: usize = 500;

/// 一次清理的结果，日志与测试共用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneRun {
    pub deleted: usize,
    pub batches: usize,
    /// 保留天数为 0（用户关了清理）时为 true
    pub disabled: bool,
}

/// 起后台任务。main 里调一次。
pub fn spawn(state: SharedState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // 启动时先做一次「该不该清」的判断 —— 但别在起服务的那一瞬间抢库锁，
        // 先等一小会儿，让探活与第一批请求先跑起来。
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut t = tokio::time::interval(TICK);
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            match run_once(&state).await {
                Ok(run) if run.deleted > 0 => {
                    tracing::info!(
                        "账目清理：删掉 {} 条过期请求（{} 批），保留期 {} 天",
                        run.deleted,
                        run.batches,
                        retention_days_of(&state)
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::error!("账目清理失败：{e}"),
            }
            t.tick().await;
        }
    })
}

fn retention_days_of(state: &SharedState) -> i64 {
    let conn = state.conn();
    maintenance::retention_days(&conn).unwrap_or(maintenance::DEFAULT_RETENTION_DAYS)
}

/// 到点就清一次。单独拆出来是为了能测（不用等真定时器）。
///
/// 返回的是本次真删掉多少条；没到点、或者用户关了清理，就返回 0。
pub async fn run_once(state: &SharedState) -> anyhow::Result<PruneRun> {
    run(state, false).await
}

/// 面板「立即清理一次」：不等一天间隔，马上跑。
pub async fn prune_now(state: &SharedState) -> anyhow::Result<PruneRun> {
    run(state, true).await
}

async fn run(state: &SharedState, force: bool) -> anyhow::Result<PruneRun> {
    let now = now_ms();

    // —— 第 1 段：只持锁读判断，读完立刻放开 ——
    let (days, due, cutoff) = {
        let conn = state.conn();
        let days = maintenance::retention_days(&conn)?;
        let last = maintenance::last_prune_at(&conn)?;
        let due = last.map(|t| now - t >= PRUNE_INTERVAL_MS).unwrap_or(true);
        (days, due, maintenance::cutoff_ms(now, days))
    };

    let Some(cutoff) = cutoff else {
        return Ok(PruneRun { deleted: 0, batches: 0, disabled: true });
    };
    if !due && !force {
        return Ok(PruneRun { deleted: 0, batches: 0, disabled: false });
    }

    // —— 第 2 段：分批删，每批之间放开锁并让出执行权 ——
    let mut deleted = 0usize;
    let mut batches = 0usize;
    loop {
        let n = {
            let conn = state.conn();
            maintenance::prune_batch(&conn, cutoff, BATCH_LIMIT)?
        };
        deleted += n;
        batches += 1;
        if n == 0 {
            break;
        }
        if batches >= MAX_BATCHES_PER_RUN {
            tracing::warn!("账目清理达到单次批次上限（{} 批），剩下的下一轮继续", batches);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // —— 第 3 段：记账 + 每周空间回收 ——
    {
        let conn = state.conn();
        maintenance::record_prune(&conn, now, deleted)?;
        maintenance::maybe_reclaim_space(&conn, now)?;
    }

    // 保留期天数留一份给日志（上面持锁已经读过了，这里用同一个值）
    let _ = days;
    Ok(PruneRun { deleted, batches, disabled: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interval_is_daily() {
        assert_eq!(PRUNE_INTERVAL_MS, 86_400_000, "一天一次，不是一小时一次");
    }

    #[test]
    fn a_single_run_is_bounded() {
        // 批次上限别被改成 u64::MAX 之类，否则一个巨型旧库会让后台任务跑一整天
        assert!(MAX_BATCHES_PER_RUN <= 1000);
        assert!(MAX_BATCHES_PER_RUN * BATCH_LIMIT as usize >= 1_000_000);
    }
}