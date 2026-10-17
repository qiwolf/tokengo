//! 对外代理面：真转发。全项目最核心的一块。
//!
//! 一段请求的一生（严格按 `docs/03-路由策略.md`）：
//!
//! ```text
//!   取模型组 → 过滤出可用渠道 →（带图的请求：再筛出标了「能看图」的渠道）
//!   → 问路由状态机「这次按什么顺序试」
//!   → 开一条记账（请求级）
//!   → 逐条渠道、每条渠道最多 1+retry_times 次尝试
//!        渠道的毛病（网络/5xx/429/401/403）→ 重试；重试耗尽 → 切换下一条渠道
//!        客户端自己的错（400/404/422）→ 原样返回，不重试不切换
//!        成功 → 更新路由状态（该回切就回切）→ 原样回给客户端
//!   → 每次尝试都记一条（尝试级），分母是"被叫了几次"，不是"成功了几次"
//! ```
//!
//! 两条纪律，代码里反复出现：
//!
//! 1. **锁不能跨 await**。`state.conn()` / `state.routing()` 拿到的守卫在任何
//!    `.await` 之前必须放掉（数据库守卫是 std::Mutex，跨 await 一卡就是全机排队）。
//! 2. **记账只落一次**。客户端中途断开时，处理函数会被整个丢掉 —— 靠
//!    `RequestGuard` 的 Drop 兜底记成 canceled，绝不让一条请求永远停在
//!    "failed"（默认值）上骗统计。

use std::collections::HashMap;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_util::stream::unfold;
use serde_json::Value;
use tokengo_shared::channel::Channel;
use tokengo_shared::routing::{classify_status, retry_delay_ms, AttemptOutcome};
use tokengo_shared::ChannelId;

use crate::db::requests::Status;
use crate::db::{groups, channels, requests, now_ms};
use crate::routes::ApiError;
use crate::SharedState;

/// 对外端点。只有三个（依据：3 天 10 万次请求里向量 86%、对话 14%，其余 41 个端点 0 次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    Chat,
    Embeddings,
    Rerank,
}

impl EndpointKind {
    /// 打上游时接在 base_url 后面的路径。
    pub fn path(self) -> &'static str {
        match self {
            EndpointKind::Chat => "chat/completions",
            EndpointKind::Embeddings => "embeddings",
            EndpointKind::Rerank => "rerank",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EndpointKind::Chat => "/v1/chat/completions",
            EndpointKind::Embeddings => "/v1/embeddings",
            EndpointKind::Rerank => "/v1/rerank",
        }
    }
}

/// 一次转发的输入。
pub struct ForwardRequest {
    pub endpoint: EndpointKind,
    pub model: String,
    /// 原样的请求体（只改 `model` 一处，别的一个字节都不动）。
    pub body: Value,
    pub stream: bool,
    pub client_ip: Option<String>,
}

/// 本次请求要用的渠道（已按组内优先级排好，且都是可用渠道）+ 各自的密钥。
struct Plan {
    group: String,
    channels: Vec<Channel>,
    secrets: HashMap<i64, String>,
}

impl Plan {
    fn ids(&self) -> Vec<ChannelId> {
        self.channels.iter().map(|l| l.id).collect()
    }

    fn channel(&self, id: ChannelId) -> Option<&Channel> {
        self.channels.iter().find(|l| l.id == id)
    }

    fn secret(&self, id: ChannelId) -> Option<&str> {
        self.secrets.get(&id.0).map(String::as_str)
    }
}

/// 从库里把这次请求需要的东西一次捞齐（同步、短暂持锁；返回后就不许再碰库守卫）。
fn load_plan(state: &SharedState, model: &str) -> Result<Plan, ApiError> {
    let conn = state.conn();
    let group_name = model;
    let channels_all = groups::resolve_channels(&conn, group_name)?;
    if channels_all.is_empty() && groups::get(&conn, group_name)?.is_none() {
        return Err(ApiError::not_found(format!(
            "模型 {model} 没有配置：面板里给它建一个模型组，或者打 auto"
        )));
    }
    let mut routable = Vec::new();
    let mut secrets = HashMap::new();
    for channel in channels_all {
        if !channel.routable() {
            continue;
        }
        if let Some(s) = channels::secret(&conn, &state.secrets, channel.id)? {
            secrets.insert(channel.id.0, s);
        }
        routable.push(channel);
    }
    if routable.is_empty() {
        return Err(ApiError::not_found(format!(
            "模型 {model} 现在没有可用的渠道（组里的渠道要么停用了，要么没填 base_url）"
        )));
    }
    Ok(Plan {
        group: group_name.to_string(),
        channels: routable,
        secrets,
    })
}

/// 转发。成功/失败都返回一个**能直接回给客户端**的响应。
pub async fn forward(state: SharedState, req: ForwardRequest) -> Result<Response, ApiError> {
    let started_ms = now_ms();
    let plan_all = load_plan(&state, &req.model)?;
    // 这一组的全量可用渠道（状态机一律看这一份，不看带图筛过的那一份）
    let all_ids = plan_all.ids();

    // 这条请求是不是在让模型看图？（只有对话端点会有图）
    let wants_vision = req.endpoint == EndpointKind::Chat
        && tokengo_shared::request_has_image(&req.body);

    // 带图就只走「标了能看图」的渠道。**没标 = 不敢保证**：把图发给一条纯文本
    // 上游，它要么回一个含糊的 400、要么干脆把图忽略掉装没事，两种都让用户
    // 看不出来发生了什么（上游 FreeLLMAPI 也是这个规矩，它连"一个能看图的
    // 模型都没启用"都要单独回一个 422，而不是等上游报错）。
    let plan = if wants_vision {
        let channels: Vec<Channel> = plan_all
            .channels
            .iter()
            .filter(|l| l.vision)
            .cloned()
            .collect();
        Plan {
            group: plan_all.group.clone(),
            channels,
            secrets: plan_all.secrets.clone(),
        }
    } else {
        plan_all
    };

    if wants_vision && plan.channels.is_empty() {
        // 记一条账：面板的「请求流水」里要能看见这次被拦下的带图请求
        // （这不是渠道的毛病，是配置里没有能看图的渠道 → client_error）。
        let msg = format!(
            "这条请求带了图片，但模型「{}」后面没有一条渠道标了「能看图」。\
             去面板『渠道管理』给这条渠道的模型名勾上「能看图」（拿不准就先点它旁边的『测视觉』实测一下），\
             或者把图片请求改成打一个确实能看图的模型名。",
            req.model
        );
        let request_id = {
            let conn = state.conn();
            requests::start(&conn, req.endpoint.as_str(), &req.model, req.client_ip.as_deref())?
        };
        let mut guard = RequestGuard::new(state.clone(), request_id, started_ms);
        guard.finish(
            &state,
            Status::ClientError,
            Some(422),
            None,
            started_ms,
            None,
            0,
            0,
            false,
            Some(&msg),
        );
        return Ok(no_vision_model_response(msg));
    }

    // 这一组平时走哪个端点，后台探活要知道（否则给向量组发对话探活必然 400）
    state.note_group_endpoint(&plan.group, req.endpoint);

    // 状态机看的一律是**这一组的全量可用渠道**（跟带不带图无关）。带图请求只是在
    // 它给出的顺序里挑出能看图的渠道 —— 一份被筛过的名单喂进去，宁可回不到首选，
    // 也绝不能反过来把整组的「当前渠道」改成那条能看图的渠道。
    let ids = all_ids;
    let order_all = {
        // 微秒级的一段锁，里面没有 await
        state.routing().attempt_order(&plan.group, &ids, started_ms)
    };
    let order_all = if order_all.is_empty() { ids.clone() } else { order_all };
    // plan 带图时已经是筛过的：`plan.channel(id)` 有值就说明这条渠道标了「能看图」
    let order: Vec<ChannelId> = if wants_vision {
        order_all.iter().copied().filter(|id| plan.channel(*id).is_some()).collect()
    } else {
        order_all
    };
    // 带图请求**只读状态机、不写**：它本来就只在「能看图」这个小圈子里挑渠道，
    // 让它的结果去改整组的口味，等于一条带图请求把后面十分钟的纯文字请求一起改道。
    // 渠道自身的健康照旧由每分钟探活和纯文字请求看着。
    let touch_state = !wants_vision;

    let request_id = {
        let conn = state.conn();
        requests::start(&conn, req.endpoint.as_str(), &req.model, req.client_ip.as_deref())?
    };
    let mut guard = RequestGuard::new(state.clone(), request_id, started_ms);

    let cfg = state.routing().cfg.clone();
    let attempt_timeout = Duration::from_millis(cfg.attempt_timeout_ms.max(1));

    let mut ordinal: u32 = 0;
    let mut last_fault: Option<String> = None;
    let mut last_http: Option<UpstreamError> = None;
    let mut last_channel = ChannelId(0);

    for channel_id in order.iter().copied() {
        let Some(channel) = plan.channel(channel_id) else { continue };
        last_channel = channel.id;
        let sent_model = channel.upstream_model(&req.model).to_string();
        let body = rewrite_model(&req.body, &sent_model);
        let body_bytes = Bytes::from(serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?);

        for attempt_no in 1..=cfg.max_attempts() {
            if attempt_no > 1 {
                let d = retry_delay_ms(attempt_no);
                if d > 0 {
                    tokio::time::sleep(Duration::from_millis(d)).await;
                }
            }
            // 重试总预算：超了不再重试（不做无上限的等待）
            if now_ms() - started_ms > cfg.request_budget_ms as i64 {
                guard.finish(
                    &state,
                    Status::Failed,
                    None,
                    Some(last_channel.0),
                    started_ms,
                    None,
                    0,
                    0,
                    false,
                    Some("重试预算用尽，放弃重试"),
                );
                return Ok(gateway_timeout(&plan, &req.model));
            }

            let t0 = now_ms();
            let uuid = crate::upstream::new_uuid();
            let outcome = state
                .upstream
                .call(
                    channel,
                    plan.secret(channel.id),
                    req.endpoint.path(),
                    body_bytes.clone(),
                    req.stream,
                    attempt_timeout,
                    &uuid,
                    now_ms(),
                )
                .await;

            let resp = match outcome {
                Err(fault) => {
                    let dur = now_ms() - t0;
                    record_attempt(&state, request_id, ordinal, channel, "channel_fault", None, dur, Some(&fault.message));
                    ordinal += 1;
                    last_fault = Some(format!("{}：{}", channel.name, fault.message));
                    continue;
                }
                Ok(r) => r,
            };

            let status = resp.status().as_u16();
            let dur = now_ms() - t0;
            match classify_status(status) {
                AttemptOutcome::Success => {
                    record_attempt(&state, request_id, ordinal, channel, "success", Some(status), dur, None);
                    // 命中的是首选就视为已回切；不是就记下当前渠道、重设检查点
                    if touch_state {
                        let mut r = state.routing();
                        r.on_success(&plan.group, channel.id, &ids, now_ms());
                        drop(r);
                        state.persist_state(&plan.group)?;
                    }
                    let http = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
                    return Ok(if req.stream {
                        stream_passthrough(
                            state.clone(),
                            resp,
                            guard,
                            request_id,
                            started_ms,
                            channel.id.0,
                        )
                    } else {
                        let (resp, input, output) = if req.endpoint == EndpointKind::Rerank {
                            buffer_rerank_passthrough(resp, http, &req.body).await
                        } else {
                            buffer_passthrough(resp, http).await
                        };
                        guard.finish(
                            &state,
                            Status::Ok,
                            Some(status),
                            Some(channel.id.0),
                            started_ms,
                            Some(dur),
                            input,
                            output,
                            false,
                            None,
                        );
                        resp
                    });
                }
                AttemptOutcome::ClientFault => {
                    // 客户端自己的错：原样返回，**不重试不换渠道**（换个说法：一次写错的请求
                    // 不许把整条链路打断）
                    record_attempt(&state, request_id, ordinal, channel, "client_fault", Some(status), dur, None);
                    let err = read_error(resp).await;
                    let note = format!(
                        "客户端错误 {status}（渠道 {} 未重试、未切换）：{}",
                        channel.name,
                        err.text.chars().take(200).collect::<String>()
                    );
                    guard.finish(
                        &state,
                        Status::ClientError,
                        Some(status),
                        Some(channel.id.0),
                        started_ms,
                        Some(dur),
                        0,
                        0,
                        false,
                        Some(&note),
                    );
                    return Ok(err.into_response());
                }
                AttemptOutcome::ChannelFault => {
                    let err = read_error(resp).await;
                    record_attempt(
                        &state,
                        request_id,
                        ordinal,
                        channel,
                        "channel_fault",
                        Some(status),
                        dur,
                        Some(&err.text),
                    );
                    ordinal += 1;
                    last_fault = Some(format!("{}：HTTP {status} {}", channel.name, err.text));
                    last_http = Some(err);
                    continue;
                }
                AttemptOutcome::ClientGone => {
                    record_attempt(&state, request_id, ordinal, channel, "client_gone", Some(status), dur, None);
                    guard.finish(
                        &state,
                        Status::Canceled,
                        Some(status),
                        Some(channel.id.0),
                        started_ms,
                        Some(dur),
                        0,
                        0,
                        false,
                        Some("客户端已断开"),
                    );
                    return Ok(StatusCode::REQUEST_TIMEOUT.into_response());
                }
            }
        }

        // 这条渠道把重试次数用完了还是不行 → 切换
        if touch_state {
            let mut r = state.routing();
            r.on_channel_exhausted(&plan.group, channel.id, &ids, now_ms());
            drop(r);
            state.persist_state(&plan.group)?;
            tracing::warn!("渠道 {} 连续 {} 次失败，切换", channel.name, cfg.max_attempts());
        } else {
            tracing::warn!(
                "（带图请求，不动路由状态）渠道 {} 连续 {} 次失败",
                channel.name,
                cfg.max_attempts()
            );
        }
    }

    // 所有渠道都试完了
    let note = last_fault
        .clone()
        .unwrap_or_else(|| "所有渠道都没能提供响应".to_string());
    guard.finish(
        &state,
        Status::Failed,
        last_http.as_ref().map(|e| e.status.as_u16()),
        Some(last_channel.0),
        started_ms,
        None,
        0,
        0,
        false,
        Some(&note),
    );
    tracing::error!("请求 {request_id} 全部渠道失败：{note}");
    Ok(match last_http {
        Some(err) => err.into_response(),
        None => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_unavailable",
            format!("所有渠道都失败了：{note}"),
        )
        .into_response(),
    })
}

fn gateway_timeout(plan: &Plan, model: &str) -> Response {
    ApiError::new(
        StatusCode::GATEWAY_TIMEOUT,
        "upstream_timeout",
        format!(
            "重试预算用尽（模型组 {}，请求的模型 {model}）",
            plan.group
        ),
    )
    .into_response()
}

/// 带图的请求撞上「一组里没有能看图的渠道」时的响应。
///
/// 形状跟上游 FreeLLMAPI 对齐（`code: no_vision_model`）：客户端的处理逻辑可以照抄。
/// 为什么不是把图丢掉照常转发：那样用户会拿到一个"没看图却答了话"的结果，
/// 而且**看不出来**哪里不对 —— 这比报错难查得多。
fn no_vision_model_response(message: String) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": "no_vision_model",
            }
        })),
    )
        .into_response()
}

/// 记一次尝试。记账失败不许把请求带崩 —— 日志里有痕迹就行。
fn record_attempt(
    state: &SharedState,
    request_id: i64,
    ordinal: u32,
    channel: &Channel,
    outcome: &str,
    status: Option<u16>,
    duration_ms: i64,
    error: Option<&str>,
) {
    let conn = state.conn();
    if let Err(e) = requests::add_attempt(
        &conn,
        request_id,
        ordinal,
        channel.id.0,
        outcome,
        status,
        duration_ms,
        error,
    ) {
        tracing::error!("记账失败（尝试级）：{e}");
    }
}

/// 把请求体里的 model 换成这条渠道认的名字，**别的字段一个都不动**。
pub fn rewrite_model(body: &Value, model: &str) -> Value {
    let mut v = body.clone();
    if let Some(obj) = v.as_object_mut() {
        obj.insert("model".to_string(), Value::String(model.to_string()));
    }
    v
}

/// 从响应体里取出 token 用量（OpenAI 形状；向量端点只有输入没有输出）。
pub fn usage_from_value(v: &Value) -> (i64, i64) {
    let u = match v.get("usage") {
        Some(u) => u,
        None => return (0, 0),
    };
    let get = |names: [&str; 2]| -> i64 {
        for n in names {
            if let Some(x) = u.get(n).and_then(|x| x.as_i64()) {
                return x;
            }
        }
        0
    };
    (get(["prompt_tokens", "input_tokens"]), get(["completion_tokens", "output_tokens"]))
}

/// 非流式：整段读下来再回给客户端（顺带把 token 账记上）。
async fn buffer_passthrough(
    resp: reqwest::Response,
    status: StatusCode,
) -> (Response, i64, i64) {
    let ctype = content_type(&resp);
    let bytes = resp.bytes().await.unwrap_or_default();
    let (input, output) = serde_json::from_slice::<Value>(&bytes)
        .map(|v| usage_from_value(&v))
        .unwrap_or((0, 0));
    let mut b = Response::builder().status(status);
    if let Some(ct) = ctype {
        b = b.header(header::CONTENT_TYPE, ct);
    }
    (
        b.body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()),
        input,
        output,
    )
}

/// 重排：整段读下来再回给客户端；上游 `document` 为 null 时按 index 回填请求里的原文。
async fn buffer_rerank_passthrough(
    resp: reqwest::Response,
    status: StatusCode,
    req_body: &Value,
) -> (Response, i64, i64) {
    let ctype = content_type(&resp);
    let bytes = resp.bytes().await.unwrap_or_default();
    let docs = req_body
        .get("documents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(results) = parsed.get_mut("results").and_then(|r| r.as_array_mut()) {
        for r in results.iter_mut() {
            let idx = r.get("index").and_then(|x| x.as_u64()).map(|x| x as usize);
            let doc = r.get("document");
            let need_fill = match doc {
                None => true,
                Some(Value::Null) => true,
                Some(Value::Object(o)) if o.is_empty() => true,
                _ => false,
            };
            if need_fill {
                if let Some(i) = idx {
                    if let Some(text) = docs.get(i).and_then(|d| d.as_str()) {
                        r.as_object_mut().map(|o| {
                            o.insert("document".to_string(), serde_json::json!({"text": text}))
                        });
                    } else if let Some(obj) = docs.get(i) {
                        r.as_object_mut().map(|o| {
                            o.insert("document".to_string(), serde_json::json!({"text": obj.to_string()}))
                        });
                    }
                }
            }
        }
    }
    let out_bytes = serde_json::to_vec(&parsed).unwrap_or_else(|_| bytes.to_vec());
let (input, output) = usage_from_value(&parsed);
    let mut b = Response::builder().status(status);
    if let Some(ct) = ctype {
        b = b.header(header::CONTENT_TYPE, ct);
    }
    (
        b.body(Body::from(out_bytes))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()),
        input,
        output,
    )
}

/// 流式：边收边转发，同时从 SSE 里把最后的 usage 抠出来记账。
///
/// **第一个字节发出去之后就不能再换渠道了**（客户端已经看到内容了），所以流式请求的
/// 重试只发生在"还没拿到响应头"的阶段。这一条也是"客户端断开不计渠道的账"的落点：
/// 流被丢掉时 `RequestGuard` 记 canceled。
fn stream_passthrough(
    state: SharedState,
    resp: reqwest::Response,
    guard: RequestGuard,
    _request_id: i64,
    started_ms: i64,
    channel_id: i64,
) -> Response {
    let status = resp.status();
    let ctype = content_type(&resp).unwrap_or_else(|| "text/event-stream".to_string());
    let st = StreamState {
        resp,
        scanner: SseScanner::default(),
        guard: Some(guard),
        started_ms,
        channel_id,
        state,
        delivered: 0,
    };
    let stream = unfold(st, |mut st| async move {
        match st.resp.chunk().await {
            Ok(Some(chunk)) => {
                st.scanner.feed(&chunk);
                st.delivered += chunk.len();
                Some((Ok::<Bytes, std::io::Error>(chunk), st))
            }
            Ok(None) => {
                st.finish(true);
                None
            }
            Err(e) => {
                st.finish(false);
                tracing::warn!("流式响应中途断了：{e}");
                Some((
                    Err(std::io::Error::other(format!("上游断流：{e}"))),
                    st,
                ))
            }
        }
    });
    Response::builder()
        .status(StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK))
        .header(header::CONTENT_TYPE, ctype)
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

struct StreamState {
    resp: reqwest::Response,
    scanner: SseScanner,
    guard: Option<RequestGuard>,
    started_ms: i64,
    channel_id: i64,
    state: SharedState,
    delivered: usize,
}

impl StreamState {
    /// `complete`：上游把流发完了（true）还是中途断了（false）。
    fn finish(&mut self, complete: bool) {
        let Some(mut guard) = self.guard.take() else {
            return;
        };
        let (input, output) = self.scanner.totals();
        let note = if complete {
            None
        } else {
            Some("上游流式中途断了，已发出去的内容收不回来")
        };
        guard.finish(
            &self.state,
            if complete { Status::Ok } else { Status::Failed },
            None,
            Some(self.channel_id),
            self.started_ms,
            None,
            input,
            output,
            true,
            note,
        );
    }
}

fn content_type(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// 读一眼错误响应的正文（用于记账与日志），并保留能原样回给客户端的响应形状。
async fn read_error(resp: reqwest::Response) -> UpstreamError {
    let status = resp.status();
    let ctype = content_type(&resp);
    let retry_after = resp.headers().get(header::RETRY_AFTER).cloned();
    let body = resp.bytes().await.unwrap_or_default();
    let text = String::from_utf8_lossy(&body)
        .chars()
        .take(400)
        .collect::<String>();
    UpstreamError {
        status,
        ctype,
        retry_after,
        body,
        text,
    }
}

/// 上游的一次错误响应。保留正文与 Content-Type，最后一条渠道失败时原样透给客户端
/// —— 客户端看到的是真实的上游错误（比如 429 的提示），不是网关编的。
pub struct UpstreamError {
    pub status: StatusCode,
    pub ctype: Option<String>,
    pub retry_after: Option<HeaderValue>,
    pub body: Bytes,
    pub text: String,
}

impl UpstreamError {
    pub fn into_response(self) -> Response {
        let mut b = Response::builder().status(self.status);
        if let Some(ct) = self.ctype {
            b = b.header(header::CONTENT_TYPE, ct);
        }
        if let Some(ra) = self.retry_after {
            b = b.header(header::RETRY_AFTER, ra);
        }
        b.body(Body::from(self.body))
            .unwrap_or_else(|_| self.status.into_response())
    }
}

/// SSE 里抠 usage 的小扫描器。
///
/// 为什么不能整段读：流式就是为了别把整段缓存下来。usage 一般出现在最后一个
/// `data:` 行里，逐行解析就够，内存是常数级（只在半行没读完时留一点尾巴）。
#[derive(Default)]
pub struct SseScanner {
    pending: Vec<u8>,
    input: i64,
    output: i64,
}

/// 一行都解析不出来的东西不该把内存吃光（上游万一不回 SSE）。
const MAX_PENDING: usize = 1 << 20;

impl SseScanner {
    pub fn feed(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);
        while let Some(i) = self.pending.iter().position(|b| *b == b'\n') {
            let line = self.pending[..i].to_vec();
            self.pending.drain(..=i);
            self.line(&line);
        }
        if self.pending.len() > MAX_PENDING {
            self.pending.clear();
        }
    }

    fn line(&mut self, raw: &[u8]) {
        let line = String::from_utf8_lossy(raw);
        let Some(data) = line.trim().strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        if let Ok(v) = serde_json::from_str::<Value>(data) {
            let (i, o) = usage_from_value(&v);
            if i > 0 || o > 0 {
                self.input = i;
                self.output = o;
            }
        }
    }

    pub fn totals(&self) -> (i64, i64) {
        (self.input, self.output)
    }
}

/// 请求收尾器：谁先到谁落账，只落一次。
///
/// 正常路径显式调 `finish`；如果这个结构是被 **Drop** 掉的（客户端中途断开时，
/// axum/hyper 会把整个处理函数连同它一起丢掉），就记成 `canceled` ——
/// 客户端自己断开不算渠道的毛病，不该进渠道的账。
pub struct RequestGuard {
    state: SharedState,
    request_id: i64,
    started_ms: i64,
    done: bool,
}

impl RequestGuard {
    pub fn new(state: SharedState, request_id: i64, started_ms: i64) -> Self {
        Self {
            state,
            request_id,
            started_ms,
            done: false,
        }
    }

    /// 记账收尾。`first_byte_ms` 传 None 时用现在的耗时兜底。
    #[allow(clippy::too_many_arguments)]
    pub fn finish(
        &mut self,
        state: &SharedState,
        status: Status,
        http_status: Option<u16>,
        channel_id: Option<i64>,
        started_ms: i64,
        first_byte_ms: Option<i64>,
        input_tokens: i64,
        output_tokens: i64,
        streamed: bool,
        error: Option<&str>,
    ) {
        if self.done {
            return;
        }
        self.done = true;
        let conn = state.conn();
        if let Err(e) = requests::finish(
            &conn,
            self.request_id,
            status,
            http_status,
            channel_id,
            now_ms() - started_ms,
            first_byte_ms,
            input_tokens,
            output_tokens,
            streamed,
            error,
        ) {
            tracing::error!("记账失败（请求级）：{e}");
        }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        // 只 try_lock：丢的时候可能正有别的地方在用库，宁可漏一条账也别卡住进程
        let Ok(conn) = self.state.db.try_lock() else {
            tracing::warn!("请求 {} 被中断，且库正忙，未能记成 canceled", self.request_id);
            return;
        };
        if let Err(e) = requests::finish(
            &conn,
            self.request_id,
            Status::Canceled,
            None,
            None,
            now_ms() - self.started_ms,
            None,
            0,
            0,
            false,
            Some("客户端断开（处理函数被丢弃）"),
        ) {
            tracing::warn!("请求 {} 记 canceled 失败：{e}", self.request_id);
        }
    }
}

/// 从上游响应里读个状态码用（探活用）。
pub fn status_code_of(resp: &reqwest::Response) -> u16 {
    resp.status().as_u16()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_model_field_is_rewritten() {
        let body = json!({
            "model": "auto",
            "messages": [{"role": "user", "content": "你好"}],
            "temperature": 0.7,
            "stream": true
        });
        let out = rewrite_model(&body, "deepseek/deepseek-v4.1-flash");
        assert_eq!(out["model"], "deepseek/deepseek-v4.1-flash");
        assert_eq!(out["messages"][0]["content"], "你好");
        assert_eq!(out["temperature"], 0.7);
        assert_eq!(out["stream"], true);
    }

    #[test]
    fn usage_is_read_from_openai_and_anthropic_shapes() {
        assert_eq!(
            usage_from_value(&json!({"usage": {"prompt_tokens": 12, "completion_tokens": 3}})),
            (12, 3)
        );
        assert_eq!(
            usage_from_value(&json!({"usage": {"input_tokens": 7, "output_tokens": 0}})),
            (7, 0)
        );
        assert_eq!(usage_from_value(&json!({"data": []})), (0, 0));
    }

    #[test]
    fn sse_scanner_survives_split_lines_and_multibyte() {
        let mut s = SseScanner::default();
        // 一行里塞个汉字，并**故意从中间切开** —— 网络分片不照顾 UTF-8 边界
        let cn = "你好".as_bytes();
        s.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"");
        s.feed(&cn[..3]);
        s.feed(&cn[3..]);
        s.feed(b"\"}}]}\n");
        s.feed(b"data: {\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n");
        s.feed(b"data: [DONE]\n");
        assert_eq!(s.totals(), (9, 4));
    }

    #[test]
    fn sse_scanner_keeps_the_last_usage_it_saw() {
        let mut s = SseScanner::default();
        s.feed(b"data: {\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n");
        s.feed(b"data: {\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":6}}\n");
        assert_eq!(s.totals(), (5, 6));
    }

    #[test]
    fn sse_scanner_ignores_junk_and_does_not_grow_forever() {
        let mut s = SseScanner::default();
        s.feed(&vec![b'x'; MAX_PENDING + 10]);
        assert_eq!(s.totals(), (0, 0));
        assert!(s.pending.is_empty(), "没有换行的垃圾流不能把内存吃光");
    }

    #[test]
    fn endpoint_paths_match_the_openai_shape() {
        assert_eq!(EndpointKind::Chat.path(), "chat/completions");
        assert_eq!(EndpointKind::Embeddings.path(), "embeddings");
        assert_eq!(EndpointKind::Rerank.path(), "rerank");
        assert_eq!(EndpointKind::Chat.as_str(), "/v1/chat/completions");
        assert_eq!(EndpointKind::Rerank.as_str(), "/v1/rerank");
    }
}
