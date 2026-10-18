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
pub fn start(
    conn: &Connection,
    endpoint: &str,
    model: &str,
    client_ip: Option<&str>,
    has_image: bool,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO requests (ts_ms, endpoint, model, status, client_ip, has_image)
         VALUES (?1, ?2, ?3, 'failed', ?4, ?5)",
        params![now_ms(), endpoint, model, client_ip, has_image as i64],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 一次「请求结束」的落账参数。
///
/// 存在的理由：这个写操作**不能失败请求**。抢不到库锁时请求路径会把它整包丢给
/// 后台落账员（`state::WriteJob`），所以参数得是自持有（owned）的，不能借用请求里的东西。
#[derive(Debug, Clone)]
pub struct FinishArgs {
    pub request_id: i64,
    pub status: Status,
    pub http_status: Option<u16>,
    pub channel_id: Option<i64>,
    pub latency_ms: i64,
    pub first_byte_ms: Option<i64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub streamed: bool,
    pub error: Option<String>,
}

/// 一次「渠道尝试」的落账参数。同上：要能被丢进后台队列。
#[derive(Debug, Clone)]
pub struct AttemptArgs {
    pub request_id: i64,
    pub ordinal: u32,
    pub channel_id: i64,
    pub outcome: String,
    pub http_status: Option<u16>,
    pub duration_ms: i64,
    pub error: Option<String>,
}

/// `FinishArgs` 的落库入口（参数化版本，供后台落账员调用）。
pub fn finish_args(conn: &Connection, a: &FinishArgs) -> Result<()> {
    finish(
        conn,
        a.request_id,
        a.status,
        a.http_status,
        a.channel_id,
        a.latency_ms,
        a.first_byte_ms,
        a.input_tokens,
        a.output_tokens,
        a.streamed,
        a.error.as_deref(),
    )
}

/// `AttemptArgs` 的落库入口（参数化版本）。
pub fn add_attempt_args(conn: &Connection, a: &AttemptArgs) -> Result<()> {
    add_attempt(
        conn,
        a.request_id,
        a.ordinal,
        a.channel_id,
        &a.outcome,
        a.http_status,
        a.duration_ms,
        a.error.as_deref(),
    )
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
    /// 这条请求是不是在让模型看图（口径与路由用的那个判断完全一致）。
    pub has_image: bool,
    pub error: Option<String>,
}

pub fn recent(conn: &Connection, limit: u32) -> Result<Vec<RequestRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts_ms, endpoint, model, status, http_status, channel_id, latency_ms,
                input_tokens, output_tokens, has_image, error
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
                has_image: r.get::<_, i64>(10)? != 0,
                error: r.get(11)?,
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

/// 按模型名的账。看的是**客户端打了什么名字**，与最终落哪条渠道无关。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModelStat {
    pub model: String,
    pub requests: i64,
    pub ok: i64,
    pub failed: i64,
    pub client_errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 按客户端的账。`client_ip` 为 `None` 表示没拿到来源地址。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClientStat {
    pub client_ip: Option<String>,
    pub requests: i64,
    pub ok: i64,
    pub failed: i64,
    pub client_errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 错误分布：同一条错误话术被记了多少次。**只数失败与客户端错**。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ErrorStat {
    pub error: String,
    pub count: i64,
}

/// 按端点归类的账：对话 / 向量 / 重排 / 别的。
///
/// 为什么要它：同一个网关后面挂着三种用法完全不同的模型，混在一起看不出
/// 「向量这条路稳不稳」—— 而现网里向量占了 86% 的量（`docs/12-模型类型.md`）。
///
/// **按 `endpoint` 归类，不按模型组现在的类型**：`endpoint` 是这条请求当时实际打的端点，
/// 既成事实；组类型是后来配的、还能随时改，用它会**让历史账跟着变**（今天把某组从
/// 对话改成向量，昨天那批对话请求就会被算成向量）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct KindStat {
    /// `chat` / `embeddings` / `rerank` / `""`（认不出来的端点）
    pub kind: String,
    /// 给人看的中文名。
    pub label: String,
    pub requests: i64,
    pub ok: i64,
    pub failed: i64,
    pub client_errors: i64,
    pub canceled: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// **成功请求**的平均耗时。把失败的一起算进去会让这个数字毫无意义
    /// （失败请求把重试和超时都算进去了）。
    pub avg_ms: i64,
}

impl KindStat {
    /// 端点字面值 → 类型。不认识的端点归到「别的」（**不猜**）。
    pub fn kind_of_endpoint(endpoint: &str) -> (&'static str, &'static str) {
        match endpoint.trim() {
            "/v1/chat/completions" => ("chat", "对话"),
            "/v1/embeddings" => ("embeddings", "向量"),
            "/v1/rerank" => ("rerank", "重排"),
            _ => ("", "别的"),
        }
    }
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
    /// 按端点类型：对话 / 向量 / 重排 / 别的。**固定按这个顺序**，缺的也补 0 行。
    pub by_kind: Vec<KindStat>,
    pub by_model: Vec<ModelStat>,
    pub by_client: Vec<ClientStat>,
    pub errors: Vec<ErrorStat>,
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

    // ── 按模型名 ──
    let mut stmt = conn.prepare(
        "SELECT model,
                COUNT(*),
                COALESCE(SUM(status = 'ok'), 0),
                COALESCE(SUM(status = 'failed'), 0),
                COALESCE(SUM(status = 'client_error'), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0)
           FROM requests WHERE ts_ms >= ?1
          GROUP BY model ORDER BY COUNT(*) DESC LIMIT 50",
    )?;
    let by_model: Vec<ModelStat> = stmt
        .query_map([since_ms], |r| {
            Ok(ModelStat {
                model: r.get(0)?,
                requests: r.get(1)?,
                ok: r.get(2)?,
                failed: r.get(3)?,
                client_errors: r.get(4)?,
                input_tokens: r.get(5)?,
                output_tokens: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    // ── 按端点类型（对话 / 向量 / 重排 / 别的）──
    // 先按端点字面值分组查，再在 Rust 里按固定顺序摆好、缺的补 0 行 ——
    // 「今天没有向量请求」也要看得见「向量 0 次」，而不是这一块整个消失。
    let mut stmt = conn.prepare(
        "SELECT endpoint,
                COUNT(*),
                COALESCE(SUM(status = 'ok'), 0),
                COALESCE(SUM(status = 'failed'), 0),
                COALESCE(SUM(status = 'client_error'), 0),
                COALESCE(SUM(status = 'canceled'), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(CAST(AVG(CASE WHEN status = 'ok' THEN latency_ms END) AS INTEGER), 0)
           FROM requests WHERE ts_ms >= ?1
          GROUP BY endpoint",
    )?;
    let raw: Vec<(String, i64, i64, i64, i64, i64, i64, i64, i64)> = stmt
        .query_map([since_ms], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let zero = |kind: &str, label: &str| KindStat {
        kind: kind.to_string(),
        label: label.to_string(),
        requests: 0,
        ok: 0,
        failed: 0,
        client_errors: 0,
        canceled: 0,
        input_tokens: 0,
        output_tokens: 0,
        avg_ms: 0,
    };
    let mut by_kind = vec![
        zero("chat", "对话"),
        zero("embeddings", "向量"),
        zero("rerank", "重排"),
    ];
    let mut other: Option<KindStat> = None;
    for (endpoint, n, ok, failed, ce, canceled, inp, out, avg) in raw {
        let kind = KindStat::kind_of_endpoint(&endpoint).0;
        let row = if kind.is_empty() {
            other.get_or_insert_with(|| zero("", "别的"))
        } else {
            by_kind.iter_mut().find(|k| k.kind == kind).unwrap()
        };
        row.requests += n;
        row.ok += ok;
        row.failed += failed;
        row.client_errors += ce;
        row.canceled += canceled;
        row.input_tokens += inp;
        row.output_tokens += out;
        // 平均耗时按成功次数加权摊回来（只算成功请求，见 KindStat::avg_ms 的注释）。
        if avg > 0 && ok > 0 {
            row.avg_ms = ((row.avg_ms * (row.ok - ok) + avg * ok) / row.ok).max(0);
        }
    }
    if let Some(o) = other {
        by_kind.push(o);
    }

    // ── 按客户端 ──
    // client_ip 可能是 NULL（内网直连或没取到），也要单独占一行，不能悄悄丢掉。
    let mut stmt = conn.prepare(
        "SELECT client_ip,
                COUNT(*),
                COALESCE(SUM(status = 'ok'), 0),
                COALESCE(SUM(status = 'failed'), 0),
                COALESCE(SUM(status = 'client_error'), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0)
           FROM requests WHERE ts_ms >= ?1
          GROUP BY client_ip ORDER BY COUNT(*) DESC LIMIT 50",
    )?;
    let by_client: Vec<ClientStat> = stmt
        .query_map([since_ms], |r| {
            Ok(ClientStat {
                client_ip: r.get(0)?,
                requests: r.get(1)?,
                ok: r.get(2)?,
                failed: r.get(3)?,
                client_errors: r.get(4)?,
                input_tokens: r.get(5)?,
                output_tokens: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    // ── 错误分布 ──
    // 只数失败/客户端错；没写 error 的用「（没记原因）」占一行，
    // 否则「失败 12 次但一条原因都看不到」会让人以为面板坏了。
    let mut stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(TRIM(error), ''), '（没记原因）') AS e, COUNT(*)
           FROM requests
          WHERE ts_ms >= ?1 AND status IN ('failed', 'client_error')
          GROUP BY e ORDER BY COUNT(*) DESC LIMIT 20",
    )?;
    let errors: Vec<ErrorStat> = stmt
        .query_map([since_ms], |r| {
            Ok(ErrorStat {
                error: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

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
        by_kind,
        by_model,
        by_client,
        errors,
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
        let id = start(conn, "/v1/chat/completions", "auto", Some("192.0.2.9"), false).unwrap();
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
        let id = start(&conn, "/v1/embeddings", "bge-m3", Some("10.0.0.1"), false).unwrap();
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
        let id = start(&conn, "/v1/chat/completions", "auto", None, false).unwrap();
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

    /// 按类型分开的账：**按请求实际打的端点**归类；三行始终在（没有的补 0，不是整块消失）。
    #[test]
    fn stats_split_requests_by_the_endpoint_they_really_used() {
        let conn = open_in_memory().unwrap();
        // 1 个对话（成功，100/50 token，耗时 900），1 个向量（成功，120/0，耗时 30）
        record(&conn, &[(1, "success")], Status::Ok);
        let vec_id = start(&conn, "/v1/embeddings", "BAAI/bge-m3", Some("192.0.2.139"), false).unwrap();
        finish(&conn, vec_id, Status::Ok, Some(200), Some(1), 30, None, 120, 0, false, None).unwrap();
        // 一个认不出的端点写成失败 → 归到「别的」，不许悄悄算进任何一种
        let odd = start(&conn, "/v1/unknown", "x", None, false).unwrap();
        finish(&conn, odd, Status::Failed, Some(502), Some(1), 5000, None, 0, 0, false, Some("炸了")).unwrap();

        let s = stats_since(&conn, 0).unwrap();
        let kinds: Vec<(&str, i64)> = s.by_kind.iter().map(|k| (k.kind.as_str(), k.requests)).collect();
        assert_eq!(
            kinds,
            vec![("chat", 1), ("embeddings", 1), ("rerank", 0), ("", 1)],
            "固定顺序、缺的补 0、认不出的进「别的」：{kinds:?}"
        );
        let chat = &s.by_kind[0];
        assert_eq!((chat.ok, chat.failed, chat.input_tokens, chat.output_tokens), (1, 0, 100, 50));
        assert!(chat.avg_ms > 0, "成功请求的平均耗时要有值：{chat:?}");
        let vec = s.by_kind.iter().find(|k| k.kind == "embeddings").unwrap();
        assert_eq!((vec.ok, vec.input_tokens, vec.output_tokens), (1, 120, 0));
        let other = s.by_kind.iter().find(|k| k.kind.is_empty()).unwrap();
        assert_eq!((other.requests, other.failed), (1, 1), "认不出的端点要单独一行：{other:?}");
        assert_eq!(other.avg_ms, 0, "这条只有失败，没有「成功请求的平均耗时」可言");
        assert_eq!(s.by_kind[2].label, "重排");
        // 总数要对得上：三块 + 别的 = 全部请求
        let sum: i64 = s.by_kind.iter().map(|k| k.requests).sum();
        assert_eq!(sum, s.total, "分块的请求数加起来必须等于总数");
    }

    /// 反证：端点表认不出来就是认不出来 —— 不许把陌生端点猜成对话。
    #[test]
    fn an_unknown_endpoint_is_never_guessed_as_chat() {
        assert_eq!(KindStat::kind_of_endpoint("/v1/chat/completions").0, "chat");
        assert_eq!(KindStat::kind_of_endpoint("/v1/embeddings").0, "embeddings");
        assert_eq!(KindStat::kind_of_endpoint("/v1/rerank").0, "rerank");
        for e in ["", "/v1/chat", "/v1/embeddings/", "/v1/completions", "随便"] {
            assert_eq!(KindStat::kind_of_endpoint(e).0, "", "{e} 不该被猜成某种类型");
        }
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

    /// 打一条指定模型名 / 来源 / 错误话术的账，用来看三个新维度。
    fn record_detail(
        conn: &Connection,
        model: &str,
        ip: Option<&str>,
        status: Status,
        error: Option<&str>,
    ) -> i64 {
        let id = start(conn, "/v1/chat/completions", model, ip, false).unwrap();
        add_attempt(conn, id, 0, 44, "success", Some(200), 10, None).unwrap();
        finish(
            conn,
            id,
            status,
            Some(200),
            Some(44),
            30,
            Some(5),
            100,
            50,
            false,
            error,
        )
        .unwrap();
        id
    }

    #[test]
    fn stats_break_down_by_model_client_and_error() {
        let conn = open_in_memory().unwrap();
        record_detail(&conn, "auto", Some("192.0.2.139"), Status::Ok, None);
        record_detail(&conn, "auto", Some("192.0.2.139"), Status::Ok, None);
        record_detail(&conn, "auto", Some("192.0.2.101"), Status::Failed, Some("硅基流动：连接超时"));
        record_detail(&conn, "bge-m3", Some("192.0.2.139"), Status::Ok, None);
        record_detail(&conn, "写错的模型", None, Status::ClientError, Some("模型 写错的模型 没有配置"));

        let s = stats_since(&conn, 0).unwrap();

        // 按模型：auto 3 次（2 成 1 败）、bge-m3 1 次、写错的模型 1 次
        assert_eq!(s.by_model.len(), 3);
        let auto = s.by_model.iter().find(|m| m.model == "auto").unwrap();
        assert_eq!((auto.requests, auto.ok, auto.failed), (3, 2, 1));
        let bad = s.by_model.iter().find(|m| m.model == "写错的模型").unwrap();
        assert_eq!(bad.client_errors, 1, "客户端写错要单独列出来，不算渠道的锅");

        // 按客户端：两条内网地址 + 一条没有来源
        let a = s.by_client.iter().find(|c| c.client_ip.as_deref() == Some("192.0.2.139")).unwrap();
        assert_eq!(a.requests, 3);
        let b = s.by_client.iter().find(|c| c.client_ip.as_deref() == Some("192.0.2.101")).unwrap();
        assert_eq!(b.failed, 1);
        let none = s.by_client.iter().find(|c| c.client_ip.is_none()).unwrap();
        assert_eq!(none.requests, 1, "没拿到来源地址的请求也要占一行，不能丢");

        // 错误分布：两条不同原因各 1 次，成功的不进这个榜
        assert_eq!(s.errors.len(), 2);
        assert_eq!(s.errors.iter().map(|e| e.count).sum::<i64>(), 2);
        assert!(s.errors.iter().any(|e| e.error.contains("连接超时")));
    }

    #[test]
    fn a_failure_without_a_recorded_reason_still_shows_up() {
        let conn = open_in_memory().unwrap();
        record_detail(&conn, "auto", Some("10.0.0.1"), Status::Failed, None);
        let s = stats_since(&conn, 0).unwrap();
        assert_eq!(s.errors.len(), 1);
        assert_eq!(s.errors[0].error, "（没记原因）");
        assert_eq!(s.errors[0].count, 1);
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
