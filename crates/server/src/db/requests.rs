//! 记账：请求级 + 尝试级。
//!
//! 为什么记到"尝试"这一层：现网 FreeLLMAPI 只看得到"这条渠道用了多少次"，
//! 看不到"这条渠道被叫了几次、几次是它自己坏的"。要判断某条渠道是不是真在干活、
//! 值不值得留在列表里，必须把失败也算进分母。

use rusqlite::{params, Connection, OptionalExtension};
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
        "INSERT INTO requests (ts_ms, endpoint, model, status, client_ip, has_image, usage_source)
         VALUES (?1, ?2, ?3, 'running', ?4, ?5, 'unreported')",
        params![now_ms(), endpoint, model, client_ip, has_image as i64],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Outbound progress is queued in order and cannot overwrite a terminal record.
#[derive(Debug, Clone)]
pub struct ProgressArgs {
    pub request_id: i64,
    pub channel_id: i64,
    pub served_model: String,
    pub streamed: bool,
}

pub fn progress_args(conn: &Connection, a: &ProgressArgs) -> Result<()> {
    conn.execute("UPDATE requests SET channel_id=?2, served_model=?3, streamed=?4 WHERE id=?1 AND status='running'",
        params![a.request_id,a.channel_id,a.served_model,a.streamed])?;
    Ok(())
}

/// Only called once at process initialization, before accepting requests.
pub fn recover_interrupted(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("UPDATE requests SET status='failed', error='服务重启，请求被中断', error_code='server_interrupted', latency_ms=MAX(0,?1-ts_ms) WHERE status='running'", [now_ms()])?)
}

/// 一次「请求结束」的落账参数。
///
/// 存在的理由：这个写操作**不能失败请求**。抢不到库锁时请求路径会把它整包丢给
/// 后台落账员（`state::WriteJob`），所以参数得是自持有（owned）的，不能借用请求里的东西。
#[derive(Debug, Clone)]
pub struct FinishArgs {
    pub input_tokens_known: bool,
    pub output_tokens_known: bool,
    pub usage_source: String,

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
    /// 这次真正被叫的模型名。普通模型名时与 `model` 一样；`auto` 时是它挑中的那个
    /// —— 不然面板上只能看到一条「auto 成功了」，看不出到底走了谁。
    pub served_model: Option<String>,
    pub error_code: Option<String>,
}

/// 一次「渠道尝试」的落账参数。同上：要能被丢进后台队列。
#[derive(Debug, Clone)]
pub struct AttemptArgs {
    pub model_name: String,
    pub upstream_model: String,
    pub first_byte_ms: Option<i64>,
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
    )?;
    conn.execute("UPDATE requests SET input_tokens_known=?2,output_tokens_known=?3,usage_source=?4 WHERE id=?1",params![a.request_id,a.input_tokens_known,a.output_tokens_known,a.usage_source])?;
    if let Some(code)=&a.error_code {conn.execute("UPDATE requests SET error_code=?2 WHERE id=?1",params![a.request_id,code])?;}
    // served_model 只在「真正被叫的名字与客户端打的名字不一样」时才有值（auto）。
    // 单独一条小 UPDATE：既不用给 finish 加第 12 个参数（把一圈调用点全改了），
    // 也让「没值」时连这一句都不发。
    if let Some(s) = a.served_model.as_deref().filter(|s| !s.trim().is_empty()) {
        conn.execute(
            "UPDATE requests SET served_model = ?2 WHERE id = ?1",
            params![a.request_id, s],
        )?;
    }
    Ok(())
}

/// `AttemptArgs` 的落库入口（参数化版本）。
pub fn add_attempt_args(conn: &Connection, a: &AttemptArgs) -> Result<()> {
    conn.execute("INSERT INTO attempts(request_id,ordinal,channel_id,outcome,http_status,duration_ms,error,model_name,upstream_model,first_byte_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![a.request_id,a.ordinal,a.channel_id,a.outcome,a.http_status,a.duration_ms,a.error,a.model_name,a.upstream_model,a.first_byte_ms])?;
    Ok(())
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
        "UPDATE requests SET status = ?2, http_status = ?3, channel_id = COALESCE(?4, channel_id), latency_ms = ?5,
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
    pub input_tokens_known: bool,
    pub output_tokens_known: bool,
    pub usage_source: String,

    pub explored:bool,
    pub error_code: Option<String>,
    pub first_byte_ms: Option<i64>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub streamed: bool,
    pub served_model: String,
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

fn request_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RequestRow> {
    Ok(RequestRow {
        input_tokens_known:r.get(19)?,output_tokens_known:r.get(20)?,usage_source:r.get(21)?,
        explored:r.get::<_,i64>(18)?!=0,
        error_code:r.get(17)?,
        served_model: r.get(12)?,
        first_byte_ms:r.get(13)?,
        client_ip:r.get(14)?,
        user_agent:r.get(15)?,
        streamed:r.get::<_,i64>(16)?!=0,
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
}

#[derive(Debug, Clone, Serialize)]
pub struct AttemptRow {
    pub model_name: Option<String>,
    pub upstream_model: Option<String>,
    pub ordinal: i64,
    pub channel_id: i64,
    pub channel_name: Option<String>,
    pub outcome: String,
    pub http_status: Option<i64>,
    pub duration_ms: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestDetail {
    pub request: RequestRow,
    pub attempts: Vec<AttemptRow>,
}

/// Exact lookup is independent of the recent-list limit and retention window.
pub fn detail(conn: &Connection, id: i64) -> Result<Option<RequestDetail>> {
    let request = conn.query_row(
        "SELECT id, ts_ms, endpoint, model, status, http_status, channel_id, latency_ms,
                input_tokens, output_tokens, has_image, error, served_model, first_byte_ms,
                client_ip, user_agent, streamed, error_code, explored, input_tokens_known, output_tokens_known, usage_source FROM requests WHERE id = ?1",
        [id], request_row,
    ).optional()?;
    let Some(request) = request else { return Ok(None); };
    // LEFT JOIN keeps history readable after a channel has been deleted.
    let mut stmt = conn.prepare(
        "SELECT a.ordinal, a.channel_id, c.name, a.outcome, a.http_status, a.duration_ms, a.error,a.model_name,a.upstream_model
         FROM attempts a LEFT JOIN channels c ON c.id = a.channel_id
         WHERE a.request_id = ?1 ORDER BY a.ordinal ASC, a.id ASC",
    )?;
    let attempts = stmt.query_map([id], |r| Ok(AttemptRow {
        model_name:r.get(7)?,upstream_model:r.get(8)?,
        ordinal: r.get(0)?, channel_id: r.get(1)?, channel_name: r.get(2)?,
        outcome: r.get(3)?, http_status: r.get(4)?, duration_ms: r.get(5)?, error: r.get(6)?,
    }))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(RequestDetail { request, attempts }))
}

pub fn recent(conn: &Connection, limit: u32) -> Result<Vec<RequestRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts_ms, endpoint, model, status, http_status, channel_id, latency_ms,
                input_tokens, output_tokens, has_image, error, served_model, first_byte_ms, client_ip, user_agent, streamed, error_code, explored, input_tokens_known, output_tokens_known, usage_source
           FROM requests ORDER BY ts_ms DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |r| {
            request_row(r)
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Coverage is independent of numeric totals: historical zero cannot establish absence or zero use.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct UsageCoverage {
    pub unknown_requests: i64,
    pub unknown_input_requests: i64,
    pub unknown_output_requests: i64,
    pub legacy_requests: i64,
}
fn coverage(conn: &Connection, since: i64, filter: &str, key: Option<&str>) -> Result<UsageCoverage> {
    let sql=format!("SELECT COALESCE(SUM(NOT(input_tokens_known AND output_tokens_known)),0), COALESCE(SUM(NOT input_tokens_known),0), COALESCE(SUM(NOT output_tokens_known),0), COALESCE(SUM(usage_source='legacy'),0) FROM requests WHERE ts_ms>=?1 AND {filter}");
    Ok(conn.query_row(&sql, params![since,key], |r| Ok(UsageCoverage { unknown_requests:r.get(0)?,unknown_input_requests:r.get(1)?,unknown_output_requests:r.get(2)?,legacy_requests:r.get(3)? }))?)
}

/// 按渠道的账。**分母是"被叫了几次"，不是"成功了几次"** —— 这是判断一条渠道
/// 值不值得留的关键。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChannelStat {
    pub usage: UsageCoverage,
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
    pub usage: UsageCoverage,
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
    pub usage: UsageCoverage,
    pub client_ip: Option<String>,
    pub requests: i64,
    pub ok: i64,
    pub failed: i64,
    pub client_errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 按端点归类的账：对话 / 向量 / 重排 / 别的。
///
/// 为什么要它：同一个网关后面挂着三种用法完全不同的模型，混在一起看不出
/// 「向量这条路稳不稳」—— 而现网里向量占了 86% 的量（`docs/12-模型类型.md`）。
///
/// **按 `endpoint` 归类，不按模型组现在的类型**：`endpoint` 是这条请求当时实际打的端点，
/// 既成事实；用途是后来配的、还能随时改，用它会**让历史账跟着变**（今天把某个名字从
/// 对话改成向量，昨天那批对话请求就会被算成向量）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct KindStat {
    pub usage: UsageCoverage,
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
    pub usage: UsageCoverage,
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
                usage: UsageCoverage::default(),
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
               FROM requests WHERE ts_ms >= ?1 AND channel_id = ?2 ",
            params![since_ms, s.channel_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        s.usage=coverage(conn,since_ms,"channel_id=CAST(?2 AS INTEGER)",Some(&s.channel_id.to_string()))?;
        s.input_tokens = i;
        s.output_tokens = o;
    }

    // ── 按模型名 ──
    let mut stmt = conn.prepare(
        "SELECT CASE WHEN TRIM(served_model)<>'' AND LOWER(TRIM(served_model))<>'auto' THEN served_model WHEN channel_id IS NULL THEN '未调用上游' WHEN LOWER(TRIM(model))='auto' THEN '实际模型未记录' ELSE model END AS actual_model,
                COUNT(*),
                COALESCE(SUM(status = 'ok'), 0),
                COALESCE(SUM(status = 'failed'), 0),
                COALESCE(SUM(status = 'client_error'), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0)
           FROM requests WHERE ts_ms >= ?1
          GROUP BY actual_model ORDER BY COUNT(*) DESC LIMIT 50",
    )?;
    let mut by_model: Vec<ModelStat> = stmt
        .query_map([since_ms], |r| {
            Ok(ModelStat {
                usage: UsageCoverage::default(),
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
        usage: UsageCoverage::default(),
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
    let mut by_client: Vec<ClientStat> = stmt
        .query_map([since_ms], |r| {
            Ok(ClientStat {
                usage: UsageCoverage::default(),
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

    for row in &mut by_model {
        row.usage=coverage(conn,since_ms,"(CASE WHEN TRIM(served_model)<>'' AND LOWER(TRIM(served_model))<>'auto' THEN served_model WHEN channel_id IS NULL THEN '未调用上游' WHEN LOWER(TRIM(model))='auto' THEN '实际模型未记录' ELSE model END)=?2",Some(&row.model))?;
    }
    for row in &mut by_client { row.usage=coverage(conn,since_ms,"client_ip IS ?2",row.client_ip.as_deref())?; }
    for row in &mut by_kind {
        row.usage=coverage(conn,since_ms,"(CASE endpoint WHEN '/v1/chat/completions' THEN 'chat' WHEN '/v1/embeddings' THEN 'embeddings' WHEN '/v1/rerank' THEN 'rerank' ELSE '' END)=?2",Some(&row.kind))?;
    }
    Ok(Stats {
        usage:coverage(conn,since_ms,"(?2 IS NULL)",None)?,
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
    fn usage_migration_preserves_legacy_totals_and_marks_ambiguous_zero() {
        let c=Connection::open_in_memory().unwrap();
        for (v,sql) in super::super::schema::MIGRATIONS.iter().filter(|(v,_)|*v<=22) {
            c.execute_batch(sql).unwrap();
            if *v==1 {c.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT);").unwrap();}
            c.execute("INSERT INTO schema_migrations(version) VALUES(?1)",[v]).unwrap();
        }
        c.execute_batch("INSERT INTO requests(ts_ms,endpoint,model,status,input_tokens,output_tokens) VALUES(1,'chat','old','ok',12,0),(2,'chat','old','failed',0,0),(3,'chat','old','canceled',4,2);").unwrap();
        let snapshot=|c:&Connection| c.query_row("SELECT count(*),sum(input_tokens),sum(output_tokens),sum(status='ok'),sum(status='failed'),sum(status='canceled') FROM requests",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?))).unwrap();
        let before=snapshot(&c);super::super::migrate(&c).unwrap();assert_eq!(before,snapshot(&c));
        let rows=recent(&c,3).unwrap();assert!(rows.iter().all(|r|r.usage_source=="legacy"));
        assert!(rows[0].input_tokens_known && rows[0].output_tokens_known);
        assert!(!rows[1].input_tokens_known && !rows[1].output_tokens_known);
        assert!(rows[2].input_tokens_known && !rows[2].output_tokens_known);
        let s=stats_since(&c,0).unwrap();assert_eq!(s.usage.unknown_requests,2);assert_eq!(s.usage.legacy_requests,3);
        assert_eq!((s.input_tokens,s.output_tokens),(16,2));
        super::super::migrate(&c).unwrap();assert_eq!(before,snapshot(&c));
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
        assert_eq!(rows[0].first_byte_ms, Some(200));
        assert_eq!(rows[0].client_ip.as_deref(),Some("10.0.0.1"));
        assert_eq!(rows[0].user_agent,None);
    }

    #[test]
    fn model_statistics_attribute_auto_to_actual_model(){
        let c=open_in_memory().unwrap();let id=start(&c,"chat","auto",None,false).unwrap();
        c.execute("UPDATE requests SET served_model='real',status='ok',input_tokens=12 WHERE id=?1",[id]).unwrap();
        let v=stats_since(&c,0).unwrap();assert_eq!(v.by_model[0].model,"real");assert_eq!(v.by_model[0].input_tokens,12);
    }
    #[test]
    fn recent_calls_expose_actual_model_without_changing_request_name() {
        let conn=open_in_memory().unwrap();
        let id=start(&conn,"/v1/chat/completions","auto",None,false).unwrap();
        conn.execute("UPDATE requests SET served_model='actual-model' WHERE id=?1",[id]).unwrap();
        let row=recent(&conn,1).unwrap().remove(0);
        assert_eq!(row.model,"auto");
        assert_eq!(row.served_model,"actual-model");
    }

    #[test]
    fn new_requests_are_running_until_finished() {
        let conn = open_in_memory().unwrap();
        let id = start(&conn, "/v1/chat/completions", "auto", None, false).unwrap();
        // In-flight requests are not failures.
        let rows = recent(&conn, 1).unwrap();
        assert_eq!(rows[0].status, "running");
        assert_eq!(rows[0].id, id);
    }

    #[test]
    fn interrupted_recovery_and_late_progress_preserve_terminal_records() {
        let c=open_in_memory().unwrap();
        let running=start(&c,"chat","a",None,false).unwrap();
        let ok=start(&c,"chat","b",None,false).unwrap();
        c.execute("UPDATE requests SET status='ok' WHERE id=?1",[ok]).unwrap();
        assert_eq!(recover_interrupted(&c).unwrap(),1);
        assert_eq!(recover_interrupted(&c).unwrap(),0);
        progress_args(&c,&ProgressArgs{request_id:running,channel_id:7,served_model:"late".into(),streamed:false}).unwrap();
        progress_args(&c,&ProgressArgs{request_id:ok,channel_id:7,served_model:"late".into(),streamed:false}).unwrap();
        let rows=recent(&c,2).unwrap();
        assert!(rows.iter().all(|r|r.channel_id.is_none()));
        assert_eq!(rows.iter().find(|r|r.id==ok).unwrap().status,"ok");
        assert_eq!(rows.iter().find(|r|r.id==running).unwrap().error_code.as_deref(),Some("server_interrupted"));
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
        let auto = s.by_model.iter().find(|m| m.model == "实际模型未记录").unwrap();
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
        let rows=recent(&conn,10).unwrap();
        assert!(rows.iter().any(|r|r.error.as_deref().unwrap_or("").contains("连接超时")));
    }

    #[test]
    fn a_failure_without_a_recorded_reason_still_shows_up() {
        let conn = open_in_memory().unwrap();
        record_detail(&conn, "auto", Some("10.0.0.1"), Status::Failed, None);
        let s = stats_since(&conn, 0).unwrap();
        assert_eq!(s.failed,1);
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
