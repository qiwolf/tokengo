//! 记账：请求级 + 尝试级。
//!
//! 为什么记到"尝试"这一层：现网 FreeLLMAPI 只看得到"这条渠道用了多少次"，
//! 看不到"这条渠道被叫了几次、几次是它自己坏的"。要判断某条渠道是不是真在干活、
//! 值不值得留在列表里，必须把失败也算进分母。

use rusqlite::{params, Connection};
use serde::Serialize;

use super::{now_ms, Result};

/// 请求终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// 打通了
    Ok,
    /// 所有渠道都试完了还是失败
    Failed,
    /// 客户端自己中途断开（不算渠道的账）
    Canceled,
    /// 客户端自己的错（400/404/422，渠道没毛病，原样返回了）
    ClientError,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Failed => "failed",
            Status::Canceled => "canceled",
            Status::ClientError => "client_error",
        }
    }
}

/// 开一条请求记录，返回它的 id。
pub fn start(conn: &Connection, endpoint: &str, model: &str, client_ip: Option<&str>) -> Result<i64> {
    conn.execute(
        "INSERT INTO requests (ts_ms, endpoint, model, status, client_ip)
         VALUES (?1, ?2, ?3, 'failed', ?4)",
        params![now_ms(), endpoint, model, client_ip],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 一条请求收尾。
#[allow(clippy::too_many_arguments)]
pub fn finish(
    conn: &Connection,
    request_id: i64,
    status: Status,
    http_status: Option<u16>,
    channel_id: Option<i64>,
    latency_ms: i64,
    first_byte_ms: Option<i64>,
    input_tokens: i64,
    output_tokens: i64,
    streamed: bool,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE requests SET status = ?2, http_status = ?3, channel_id = ?4, latency_ms = ?5,
                first_byte_ms = ?6, input_tokens = ?7, output_tokens = ?8,
                streamed = ?9, error = ?10
          WHERE id = ?1",
        params![
            request_id,
            status.as_str(),
            http_status.map(|s| s as i64),
            channel_id,
            latency_ms,
            first_byte_ms,
            input_tokens,
            output_tokens,
            streamed as i64,
            error
        ],
    )?;
    Ok(())
}

/// 记一次尝试。`ordinal` 从 0 起。
pub fn add_attempt(
    conn: &Connection,
    request_id: i64,
    ordinal: u32,
    channel_id: i64,
    outcome: &str,
    http_status: Option<u16>,
    duration_ms: i64,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO attempts (request_id, ordinal, channel_id, outcome, http_status, duration_ms, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            request_id,
            ordinal as i64,
            channel_id,
            outcome,
            http_status.map(|s| s as i64),
            duration_ms,
            error
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestRow {
    pub id: i64,
    pub ts_ms: i64,
    pub endpoint: String,
    pub model: String,
    pub status: String,
    pub http_status: Option<i64>,
    pub channel_id: Option<i64>,
    pub latency_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub error: Option<String>,
}

pub fn recent(conn: &Connection, limit: u32) -> Result<Vec<RequestRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts_ms, endpoint, model, status, http_status, channel_id, latency_ms,
                input_tokens, output_tokens, error
           FROM requests ORDER BY ts_ms DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(RequestRow {
                id: r.get(0)?,
                ts_ms: r.get(1)?,
                endpoint: r.get(2)?,
                model: r.get(3)?,
                status: r.get(4)?,
                http_status: r.get(5)?,
                channel_id: r.get(6)?,
                latency_ms: r.get(7)?,
                input_tokens: r.get(8)?,
                output_tokens: r.get(9)?,
                error: r.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// 按渠道的账。**分母是"被叫了几次"，不是"成功了几次"** —— 这是判断一条渠道
/// 值不值得留的关键。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChannelStat {
    pub channel_id: i64,
    pub attempts: i64,
    pub successes: i64,
    pub channel_faults: i64,
    pub avg_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Stats {
    pub since_ms: i64,
    pub total: i64,
    pub ok: i64,
    pub failed: i64,
    pub canceled: i64,
    /// 客户端自己写错的请求（模型名写错之类）。**不算渠道的锅**，单独一列看。
    pub client_errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub by_channel: Vec<ChannelStat>,
}

/// 汇总自 `since_ms` 起的账。
pub fn stats_since(conn: &Connection, since_ms: i64) -> Result<Stats> {
    let (total, ok, failed, canceled, client_errors, inp, out): (i64, i64, i64, i64, i64, i64, i64) =
        conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(status = 'ok'), 0),
                COALESCE(SUM(status = 'failed'), 0),
                COALESCE(SUM(status = 'canceled'), 0),
                COALESCE(SUM(status = 'client_error'), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0)
           FROM requests WHERE ts_ms >= ?1",
        [since_ms],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        },
    )?;

    let mut stmt = conn.prepare(
        "SELECT a.channel_id,
                COUNT(*),
                COALESCE(SUM(a.outcome = 'success'), 0),
                COALESCE(SUM(a.outcome = 'channel_fault'), 0),
                COALESCE(CAST(AVG(a.duration_ms) AS INTEGER), 0)
           FROM attempts a JOIN requests rq ON rq.id = a.request_id
          WHERE rq.ts_ms >= ?1
          GROUP BY a.channel_id ORDER BY COUNT(*) DESC",
    )?;
    let mut by_channel: Vec<ChannelStat> = stmt
        .query_map([since_ms], |r| {
            Ok(ChannelStat {
                channel_id: r.get(0)?,
                attempts: r.get(1)?,
                successes: r.get(2)?,
                channel_faults: r.get(3)?,
                avg_ms: r.get(4)?,
                input_tokens: 0,
                output_tokens: 0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    // token 账在请求级记的，按"最终成功的渠道"归给对应渠道
    for s in &mut by_channel {
        let (i, o): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0)
               FROM requests WHERE ts_ms >= ?1 AND channel_id = ?2 AND status = 'ok'",
            params![since_ms, s.channel_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        s.input_tokens = i;
        s.output_tokens = o;
    }

    Ok(Stats {
        since_ms,
        total,
        ok,
        failed,
        canceled,
        client_errors,
        input_tokens: inp,
        output_tokens: out,
        by_channel,
    })
}

/// 清掉早于 `before_ms` 的明细。定时任务会用。
pub fn prune_before(conn: &Connection, before_ms: i64) -> Result<usize> {
    Ok(conn.execute("DELETE FROM requests WHERE ts_ms < ?1", [before_ms])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn record(conn: &Connection, channels: &[(i64, &str)], status: Status) -> i64 {
        let id = start(conn, "/v1/chat/completions", "auto", Some("192.0.2.9")).unwrap();
        for (ord, (channel, outcome)) in channels.iter().enumerate() {
            add_attempt(conn, id, ord as u32, *channel, outcome, None, 10, None).unwrap();
        }
        let final_channel = channels.last().map(|(l, _)| *l);
        finish(
            conn,
            id,
            status,
            Some(200),
            final_channel,
            30,
            Some(5),
            100,
            50,
            false,
            None,
        )
        .unwrap();
        id
    }

    #[test]
    fn a_request_survives_the_whole_lifecycle() {
        let conn = open_in_memory().unwrap();
        let id = start(&conn, "/v1/embeddings", "bge-m3", Some("10.0.0.1")).unwrap();
        finish(
            &conn,
            id,
            Status::Ok,
            Some(200),
            Some(44),
            1234,
            Some(200),
            12,
            0,
            false,
            None,
        )
        .unwrap();
        let rows = recent(&conn, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "ok");
        assert_eq!(rows[0].channel_id, Some(44));
        assert_eq!(rows[0].latency_ms, 1234);
        assert_eq!(rows[0].input_tokens, 12);
    }

    #[test]
    fn default_status_is_failed_so_a_crash_still_counts_as_a_failure() {
        let conn = open_in_memory().unwrap();
        let id = start(&conn, "/v1/chat/completions", "auto", None).unwrap();
        // 进程崩了没来得及调 finish —— 记录不能显示成"成功"
        let rows = recent(&conn, 1).unwrap();
        assert_eq!(rows[0].status, "failed");
        assert_eq!(rows[0].id, id);
    }

    #[test]
    fn attempts_keep_order_and_count_failures() {
        let conn = open_in_memory().unwrap();
        let id = record(
            &conn,
            &[(44, "channel_fault"), (47, "channel_fault"), (51, "success")],
            Status::Ok,
        );
        let rows = recent(&conn, 1).unwrap();
        assert_eq!(rows[0].id, id);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM attempts WHERE request_id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 3, "三次尝试都要留痕");
    }

    #[test]
    fn stats_count_attempts_as_the_denominator() {
        let conn = open_in_memory().unwrap();
        // 渠道 44 被叫 2 次、坏 2 次；渠道 51 被叫 1 次、好 1 次
        record(&conn, &[(44, "channel_fault"), (51, "success")], Status::Ok);
        record(&conn, &[(44, "channel_fault")], Status::Failed);

        let s = stats_since(&conn, 0).unwrap();
        assert_eq!(s.total, 2);
        assert_eq!(s.ok, 1);
        assert_eq!(s.failed, 1);

        let sf = s.by_channel.iter().find(|l| l.channel_id == 44).unwrap();
        assert_eq!(sf.attempts, 2);
        assert_eq!(sf.successes, 0);
        assert_eq!(sf.channel_faults, 2, "这条渠道一次都没成过");

        let cg = s.by_channel.iter().find(|l| l.channel_id == 51).unwrap();
        assert_eq!(cg.attempts, 1);
        assert_eq!(cg.successes, 1);
        assert_eq!(cg.output_tokens, 50, "token 归给最终接住请求的渠道");
    }

    #[test]
    fn time_window_filters_old_rows() {
        let conn = open_in_memory().unwrap();
        let id = record(&conn, &[(44, "success")], Status::Ok);
        let now = now_ms();
        conn.execute(
            "UPDATE requests SET ts_ms = ?1 WHERE id = ?2",
            params![now - 10_000_000, id],
        )
        .unwrap();
        assert_eq!(stats_since(&conn, now - 1000).unwrap().total, 0);
        assert_eq!(stats_since(&conn, 0).unwrap().total, 1);
    }

    #[test]
    fn deleting_a_request_takes_its_attempts_with_it() {
        let conn = open_in_memory().unwrap();
        let id = record(&conn, &[(44, "success")], Status::Ok);
        conn.execute("DELETE FROM requests WHERE id = ?1", [id]).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn prune_keeps_recent_rows() {
        let conn = open_in_memory().unwrap();
        record(&conn, &[(44, "success")], Status::Ok);
        let now = now_ms();
        assert_eq!(prune_before(&conn, now - 86_400_000).unwrap(), 0);
        assert_eq!(prune_before(&conn, now + 1).unwrap(), 1);
        assert_eq!(recent(&conn, 10).unwrap().len(), 0);
    }
}
