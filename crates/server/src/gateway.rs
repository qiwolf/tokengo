//! 对外代理面：真转发。全项目最核心的一块。
//!
//! 一段请求的一生（严格按 `docs/03-路由策略.md`）：
//!
//! ```text
//!   名字 → 勾了它的渠道（auto 时先现场挑一串模型名）→（带图的请求：再筛出标了
//!   「能看图」的候选）
//!   → 普通模型名问路由状态机「这次按什么顺序试」；auto 用现场算出来的顺序
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
use std::sync::Arc;
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
use crate::db::{model_routes, channels, requests, now_ms};
use crate::routes::ApiError;
use crate::SharedState;

/// 对外端点。只有三个（依据：3 天 10 万次请求里向量 86%、对话 14%，其余 41 个端点 0 次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    Chat,
    Embeddings,
    Rerank,
}

/// 上游在 `Retry-After` 里要求等的时长超过这个数（毫秒），就认定在这条渠道上耗下去
/// 不划算 —— 直接放弃剩下的重试、换下一条渠道。
///
/// 依据：现网实测的 429 有两类 —— 「Go 用量超了」与「周额度用完」，它们说的恢复时间
/// 以小时计。这时候"等 1 秒再试 3 次"纯属白等，还会把请求预算烧光。
const LONG_RETRY_AFTER_MS: u64 = 2000;
/// 同一「渠道＋模型」连续失败到这个数才进冷却（用户口径：先重试，三次都失败才冷却）。
pub const COOLDOWN_AFTER_FAILURES: i64 = 3;

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

    /// 这个端点对应的模型类型（写进模型组、回给客户端的那种）。
    pub fn as_model_kind(self) -> tokengo_shared::ModelKind {
        match self {
            EndpointKind::Chat => tokengo_shared::ModelKind::Chat,
            EndpointKind::Embeddings => tokengo_shared::ModelKind::Embeddings,
            EndpointKind::Rerank => tokengo_shared::ModelKind::Rerank,
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
    pub user_agent: Option<String>,
}

/// 一次请求要试的一个候选：**用哪个模型名** + 走哪条渠道。
///
/// 普通模型名下，所有候选共用一个名字；`auto` 时每个候选是它现场挑中的一个模型名 ——
/// 别名 / 能看图 / 丢字段都是按**这个候选名**在这条渠道上查的。
struct Candidate {
    name: String,
    channel: Channel,
}

/// 这次请求要试的候选链（已按顺序排好）+ 各自的密钥。
struct Plan {
    /// 这一链挂在哪个名字下（普通模型名 = 归一后的那个名字；auto 时是保留名）。
    route: String,
    /// 是不是保留名 `auto`：auto 现场挑、不写路由状态、不做回切。
    auto: bool,
    entries: Vec<Candidate>,
    secrets: HashMap<i64, String>,
    /// 每条渠道「同时最多几发」（渠道 id → 上限；0 或缺席 = 不限）。
    caps: HashMap<i64, u32>,
    explored:Option<String>,
    load_ticket:Option<(String,crate::state::ModelLoadTicket)>,
}

impl Plan {
    fn ids(&self) -> Vec<ChannelId> {
        self.entries.iter().map(|e| e.channel.id).collect()
    }

    fn secret(&self, id: ChannelId) -> Option<&str> {
        self.secrets.get(&id.0).map(String::as_str)
    }

    /// 这条渠道同时最多几发（0 = 不限）。
    fn cap(&self, id: ChannelId) -> u32 {
        self.caps.get(&id.0).copied().unwrap_or(0)
    }
}

/// 把一条候选挂进链子（顺手把它的密钥与并发上限捞出来）。
fn push_entry(
    state: &SharedState,
    conn: &rusqlite::Connection,
    plan: &mut Plan,
    name: String,
    channel: Channel,
) -> Result<(), ApiError> {
    if let Some(s) = channels::secret(conn, &state.secrets, channel.id)? {
        plan.secrets.insert(channel.id.0, s);
    }
    if channel.max_concurrent > 0 {
        plan.caps.insert(channel.id.0, channel.max_concurrent);
    }
    plan.entries.push(Candidate { name, channel });
    Ok(())
}

/// 从库里把这次请求要用的候选链一次捞齐（同步、短暂持锁；返回后就不许再碰库守卫）。
///
/// 两种情形：
/// - **普通模型名**：勾了它的渠道，按渠道顺序（面板「渠道」页从上到下 = 先试）。
/// - **保留名 `auto`**：现场挑一串最优的模型名（见 `db::model_routes::auto_candidates`），
///   逐个摊成链 —— 每条候选都带着「这一发要发给上游的名字」。
fn load_plan(state: &SharedState, model: &str, endpoint: EndpointKind, wants_vision:bool) -> Result<Plan, ApiError> {
    let conn = state.conn();
    if model_routes::is_auto(model) {
        return load_auto_plan(state, &conn, model, endpoint, wants_vision);
    }
    let Some(route) = model_routes::routes_for(&conn, model)? else {
        // 报错别教训人，说清「去哪儿勾上」就够了。
        return Err(ApiError::not_found(format!(
            "模型「{model}」这里没有：去面板「渠道」页把这个模型名勾上 —— 勾上才认（勾选就是白名单）"
        )));
    };
    if !route.enabled {return Err(ApiError::new(StatusCode::FORBIDDEN,"model_disabled",format!("模型「{model}」已停用")));}
    let mut plan = Plan {
        route: route.name.clone(),
        auto: false,
        entries: Vec::new(),
        secrets: HashMap::new(),
        caps: HashMap::new(),
        explored:None,
        load_ticket:None,
    };
    for channel in model_routes::channels_for(&conn, &route.name)? {
        if !channel.routable() {
            continue;
        }
        push_entry(state, &conn, &mut plan, route.name.clone(), channel)?;
    }
    if plan.entries.is_empty() {
        // 有渠道勾过这个名字，但一条都用不了：把原因指明白（停用了 / 没填 base_url）
        let mut names: Vec<String> = Vec::new();
        for id in &route.declared_by {
            if let Some(l) = channels::get(&conn, *id)? {
                names.push(l.name);
            }
        }
        return Err(ApiError::not_found(format!(
            "模型「{model}」现在一条可用渠道都没有：勾它的渠道（{}）要么停用了，要么没填 base_url",
            if names.is_empty() {
                "无".to_string()
            } else {
                names.join("、")
            }
        )));
    }
    Ok(plan)
}

/// `auto` 的链：在用户勾选过的模型里挑一串（最优在前），逐个摊成候选。
///
/// **不做兜底**（用户口径）：候选只来自用户勾选过的模型。一个都没有就 404 说清楚，
/// 对外清单里也不显示 auto（`/v1/models` 那边用 `auto_available` 判断）。
fn load_auto_plan(
    state: &SharedState,
    conn: &rusqlite::Connection,
    model: &str,
    endpoint: EndpointKind,
    wants_vision:bool,
) -> Result<Plan, ApiError> {
    let mut plan = Plan {
        route: model_routes::AUTO_NAME.to_string(),
        auto: true,
        entries: Vec::new(),
        secrets: HashMap::new(),
        caps: HashMap::new(),
        explored:None,
        load_ticket:None,
    };
    let mut candidates =
        model_routes::auto_candidates(conn, endpoint.as_model_kind(), wants_vision, now_ms())?;
    if candidates.is_empty() && wants_vision && !model_routes::auto_candidates(conn,endpoint.as_model_kind(),false,now_ms())?.is_empty() {
        return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY,"no_vision_model","这条请求带了图片，但没有符合视觉能力的候选模型"));
    }
    if candidates.is_empty() {
        let msg = if model_routes::auto_available(conn)? {
            format!(
                "「{model}」现在没有可挑的模型：你勾选过的模型里没有「{}」这一类的。\
                 去面板「模型」页看看它们的用途定得对不对（用途不对就挑不到），\
                 或者在「渠道」页勾上合适的模型名",
                endpoint.as_model_kind().label()
            )
        } else {
            format!(
                "「{model}」现在没有可挑的模型：你还没有勾选任何模型。\
                 去面板「渠道」页把要用的模型名勾上 —— 只有你勾选过的模型才会被挑（勾选就是白名单）"
            )
        };
        return Err(ApiError::not_found(msg));
    }
    let sequence=state.auto_sequence.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
    let config=crate::db::policy::get(conn)?;
    use rand::Rng;
    let draw=rand::thread_rng().gen_range(0..10000u64);
    // Exploration must never select a model whose every matching channel is cooling.
    candidates.retain(|name|model_routes::channels_for(conn,name).unwrap_or_default().iter().any(|c|crate::db::policy::peek(conn,c.id.0,name,c.upstream_model(name),now_ms()).ok().flatten().is_none()));
    plan.explored=crate::db::policy::rank(conn,&mut candidates,&config,sequence,draw)?;
    if config.strategy=="idle" {if let Some(t)=state.model_loads.select(&mut candidates,plan.explored.is_some()){plan.load_ticket=Some((candidates[0].clone(),t));}}
    for name in candidates {
        for channel in model_routes::channels_for(conn, &name)? {
            if !channel.routable() || (wants_vision && !channel.vision) {
                continue;
            }
            push_entry(state, conn, &mut plan, name.clone(), channel)?;
        }
    }
    if plan.entries.is_empty() {
        let until=model_routes::list(conn)?.iter().flat_map(|r|crate::db::policy::rows(conn,&r.name).unwrap_or_default()).map(|r|r.until_ms.max(r.lease_until)).filter(|t|*t>now_ms()).min();
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"models_cooling",format!("暂无可用模型，候选均在冷却；最早可重试时间：{}",until.map(|t|t.to_string()).unwrap_or_else(||"稍后".into()))));
    }
    Ok(plan)
}

/// 转发。成功/失败都返回一个**能直接回给客户端**的响应。
pub async fn forward(state: SharedState, req: ForwardRequest) -> Result<Response, ApiError> {
    let started_ms = now_ms();
    // 路由不出去（模型名没配置 / 组停用了 / 组里没有可用渠道）时，**也要留一条账**。
    //
    // 为什么：面板「请求」页是用户排障的第一站（他就是从那里问出「为什么客户端连不上」的）。
    // 以前这三类 404 在 `load_plan` 里就直接 return 了，一条账都不记 —— 用户看到的是
    // 「客户端报 404、面板上干干净净」，只能靠猜。这与带图被拦（422）的处理也不一致：
    // 那条是记的。**记账绝不能反过来影响请求结果**：这里只记，记账本身错了就吞掉。
    let plan_all = match load_plan(&state, &req.model, req.endpoint,req.endpoint==EndpointKind::Chat && tokengo_shared::request_has_image(&req.body)) {
        Ok(p) => p,
        Err(e) => {
            if e.status.is_client_error() || e.status==StatusCode::SERVICE_UNAVAILABLE {
                record_routing_refusal(&state, &req, started_ms, e.status.as_u16(), &e.message);
            }
            return Err(e);
        }
    };
    // 这一链的全量候选（状态机与「记账记的是谁」都看这一份）
    let all_ids = plan_all.ids();

    // 这条请求是不是在让模型看图？（只有对话端点会有图）
    let wants_vision =
        req.endpoint == EndpointKind::Chat && tokengo_shared::request_has_image(&req.body);

    // 带图就只走「标了能看图」的候选。**没标 = 不敢保证**：把图发给一条纯文本
    // 上游，它要么回一个含糊的 400、要么干脆把图忽略掉装没事，两种都让用户
    // 看不出来发生了什么（上游 FreeLLMAPI 也是这个规矩，它连"一个能看图的
    // 模型都没启用"都要单独回一个 422，而不是等上游报错）。
    // `auto` 的候选在挑的时候就只挑能看图的模型，这里再按渠道筛一遍。
    let mut plan = if wants_vision {
        let mut p = plan_all;
        p.entries.retain(|e| e.channel.vision);
        p
    } else {
        plan_all
    };

    if wants_vision && plan.entries.is_empty() {
        // 记一条账：面板的「请求流水」里要能看见这次被拦下的带图请求
        // （这不是渠道的毛病，是配置里没有能看图的渠道 → client_error）。
        let msg = if plan.auto {
            format!(
                "这条请求带了图片，但你勾选过的模型里没有一条渠道标了「能看图」。\
                 「{}」不会把图悄悄丢掉 —— 去面板「渠道」页给能看图的模型名勾上「能看图」\
                 （拿不准就先点它旁边的「测视觉」实测一下）。",
                req.model
            )
        } else {
            format!(
                "这条请求带了图片，但模型「{}」后面没有一条渠道标了「能看图」。\
                 去面板『渠道管理』给这条渠道的模型名勾上「能看图」（拿不准就先点它旁边的『测视觉』实测一下），\
                 或者把图片请求改成打一个确实能看图的模型名。",
                req.model
            )
        };
        let request_id = {
            let conn = state.conn();
            requests::start(
                &conn,
                req.endpoint.as_str(),
                &req.model,
                req.client_ip.as_deref(),
                wants_vision,
            )?
        };
        let mut guard = RequestGuard::new(state.clone(), request_id, started_ms);
    record_client_app(&state,request_id,req.user_agent.as_deref());
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


    // 顺序：
    // · 普通模型名 → 问状态机（首选 / 回切 / 上一条走的谁），带图时再按能看图筛一遍；
    // · `auto` → 现场算出来的那条链就是顺序（不读也不写状态机）。
    let order: Vec<usize> = if plan.auto {
        (0..plan.entries.len())
            .filter(|i| !wants_vision || plan.entries[*i].channel.vision)
            .collect()
    } else {
        let mut ord = state.routing().attempt_order(&plan.route, &all_ids, started_ms);
        if ord.is_empty() {
            ord = all_ids.clone();
        }
        let index_of: HashMap<i64, usize> = plan
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.channel.id.0, i))
            .collect();
        ord.iter()
            .filter_map(|id| index_of.get(&id.0).copied())
            .filter(|i| !wants_vision || plan.entries[*i].channel.vision)
            .collect()
    };
    // 带图请求与 `auto` **只读状态机、不写**：
    // · 带图请求本来就只在「能看图」这个小圈子里挑渠道，让它的结果去改整个模型名的口味，
    //   等于一条带图请求把后面十分钟的纯文字请求一起改道。
    // · `auto` 每一发都是现场挑的，没有「首选/回切」可言；让它去写状态，
    //   那个名字下的账就没人解释得清了。
    // 渠道自身的健康照旧由每分钟探活和普通请求看着。
    let touch_state = !wants_vision && !plan.auto;

    let request_id = {
        let conn = state.conn();
        requests::start(
                &conn,
                req.endpoint.as_str(),
                &req.model,
                req.client_ip.as_deref(),
                wants_vision,
            )?
    };
    let mut guard = RequestGuard::new(state.clone(), request_id, started_ms);
    record_client_app(&state,request_id,req.user_agent.as_deref());

    let cfg = state.routing().cfg.clone();
    let attempt_timeout = Duration::from_millis(cfg.attempt_timeout_ms.max(1));

    // 同一段对话稳定不变的会话 ID（自定义头 `{{session_id}}` 用）。
    // **按客户端原样的请求体算一次**：改成上游的模型名之后再算，同一个客户端模型名
    // 落到不同渠道会得出不同的会话 ID，上游看到的就不再是"同一段对话"了。
    let session = tokengo_shared::session_id_from_body(&req.body);

    let mut ordinal: u32 = 0;
    let mut last_fault: Option<String> = None;
    let mut last_http: Option<UpstreamError> = None;
    let mut last_channel = ChannelId(0);
    // 上游说的「过多久再来」；下一次重试用它顶掉默认节拍。


    // 每条渠道的「门口闸机」：同时最多几发（面板上按渠道配，0 = 不限）。
    // 满了**不硬压上去**（现网 503「bridge busy」151 次就是这么来的）：先换别的渠道；
    // 所有渠道都满，才在门口排队等一个名额，等到再重跑一遍。排队有预算上限，不会挂死。
    let mut pre_permits: HashMap<i64, tokio::sync::OwnedSemaphorePermit> = HashMap::new();
    'pass: loop {
        let mut tried_any = false;
        let mut busy: Vec<(i64, Arc<tokio::sync::Semaphore>)> = Vec::new();

    'channels: for entry_idx in order.iter().copied() {
        let Some(Candidate { name, channel }) = plan.entries.get(entry_idx) else {
            continue;
        };
        // 门口闸机：拿到名额才发；这条满了就先跳过（换下一条）
        let permit = match pre_permits.remove(&channel.id.0) {
            Some(p) => Some(p),
            None => match state.channel_gate(channel.id.0, plan.cap(channel.id)) {
                None => None,
                Some(sem) => match sem.clone().try_acquire_owned() {
                    Ok(p) => Some(p),
                    Err(_) => {
                        busy.push((channel.id.0, sem));
                        continue;
                    }
                },
            },
        };
        let sent=channel.upstream_model(name);
        let generation={let c=state.conn();crate::db::policy::claim(&c,channel.id.0,name,sent,now_ms())?};
        let Some(generation)=generation else {continue;};
        let load_ticket=match plan.load_ticket.take(){Some((n,t)) if tokengo_shared::same_model_name(&n,name)=>t,other=>{drop(other);state.model_loads.acquire(name)}};
        // 冷到期的恢复试用只放一条：失败就进下一级冷却，不再连试三次。
        let recovering={let c=state.conn();crate::db::policy::rows(&c,name)?.iter().any(|r|r.channel_id==channel.id.0 && r.upstream==sent && r.lease_until>0)};
        let max_attempts=if recovering {1} else {cfg.max_attempts().min(COOLDOWN_AFTER_FAILURES as u32).max(1)};
        let mut cooldown=CooldownTicket{state:state.clone(),id:channel.id.0,name:name.clone(),upstream:sent.to_string(),generation,done:false};
        tried_any = true;
        last_channel = channel.id;
        // 这一发送给上游的名字：按**这个候选名**在这条渠道上查别名
        // （auto 时每个候选各不相同 —— auto 挑中的是谁，上游看到的就是谁的名字）
        let sent_model = channel.upstream_model(name).to_string();
        // 账上记下「真正被叫的是谁」：面板上看得见 auto 这一发走了哪个模型
        if plan.explored.as_deref()==Some(name.as_str()) {let c=state.conn();let _=c.execute("UPDATE requests SET explored=1 WHERE id=?1",[request_id]);}

        // 先按这条渠道「这个模型名」的清单把不认的字段丢掉，再换模型名。
        // 顺序无所谓（丢的是顶层字段、换的也是顶层字段），但别把 `model` 写进清单里 ——
        // 那样等于把模型名连同别的字段一起删掉，上游会以为没带模型。
        let adapted = tokengo_shared::strip_fields(&req.body, &channel.drop_fields);
        let mut body = rewrite_model(&adapted, &sent_model);
        let cap_key = output_cap_key(channel, &sent_model, plan.secret(channel.id));
        let cached_cap = crate::db::settings::get(&state.conn(), &cap_key).ok().flatten()
            .and_then(|s| s.parse::<u64>().ok());
        let mut adaptation_note = None::<String>;
        if let Some(cap) = cached_cap {
            if clamp_output(&mut body, cap) {
                adaptation_note = Some(format!("按此渠道已知输出上限收窄至 {cap} tokens"));
            }
        }
        let mut body_bytes = Bytes::from(serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?);
        let mut learned_this_request = false;

        let mut retry_after_override:Option<u64>=None;
        // 上一次是「快失败」还是「慢失败」。连接被拒 / 5xx / 429 这类上游马上就回了，
        // 重试有意义；耗时顶到超时的失败多半是上游在挂，再等一轮只会把整个请求预算吃掉、
        // 让客户端白等，所以那种直接换下一候选（冷却计数照记）。
        let mut slow_previous=false;
        // 这一候选上见过的最大 Retry-After：冷却时当下限用（上游自己说的等多久最准）。
        let mut pending_retry_ms:i64=0;
        for attempt_no in 1..=max_attempts {
            if attempt_no > 1 {
                let left = cfg.request_budget_ms as i64 - (now_ms() - started_ms);
                if slow_previous || left < cfg.attempt_timeout_ms.max(1) as i64 {
                    break;
                }
                // 上游在 Retry-After 里说了等多久就等多久（它比我们的默认节拍更懂自己）
                let d = retry_after_override.take().unwrap_or_else(|| retry_delay_ms(attempt_no));
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

            guard.note_served(name);
            guard.channel_id = Some(channel.id.0);
            state.enqueue_write(crate::state::WriteJob::Progress(requests::ProgressArgs {
                request_id, channel_id: channel.id.0, served_model: name.clone(), streamed: req.stream,
            }));
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
                    session.as_deref(),
                )
                .await;

            let resp = match outcome {
                Err(fault) => {
                    let dur = now_ms() - t0;
                    slow_previous = dur >= cfg.attempt_timeout_ms.max(1) as i64 * 3 / 4;
                    record_attempt(&state, request_id, ordinal, channel, name, &sent_model, "channel_fault", None, dur, Some(&fault.message));
                    ordinal += 1;
                    last_fault = Some(format!("{}：{}", channel.name, fault.message));
                    if cooldown.failure(&fault.message,pending_retry_ms) {break;}
                    continue;
                }
                Ok(r) => r,
            };

            let status = resp.status().as_u16();
            let dur = now_ms() - t0;
            slow_previous = dur >= cfg.attempt_timeout_ms.max(1) as i64 * 3 / 4;
            match classify_status(status) {
                AttemptOutcome::Success => {
                    // 流式：第一个字节发出去之后就不能换渠道了，所以先记账、更新路由状态，
                    // 再直接透传（空不空只能等流完再判断，那时已经改不了什么，见 StreamState）。
                    if req.stream {

                        if touch_state {
                            let mut r = state.routing();
                            r.on_success(&plan.route, channel.id, &all_ids, now_ms());
                            drop(r);
                            state.persist_state(&plan.route)?;
                        }
                        return Ok(stream_passthrough(
                            state.clone(),
                            resp,
                            guard,
                            request_id,
                            started_ms,
                            channel.id.0,
                            permit,
                            req.endpoint,
                            ordinal, name.clone(), sent_model.clone(), t0, adaptation_note.clone(), cooldown, load_ticket, attempt_timeout,
                        ));
                    }

                    // 非流式：**先把正文读下来再算成功**。有的上游回 200 却一句话不说
                    // （现网 4 天 9 次），那种必须当这条渠道的毛病去重试/换渠道 —— 不能先把
                    // 路由状态记成「这条渠道好了」，更不能把空泡泡原样丢给用户。
                    let http = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
                    let (resp, input, output, empty) = if req.endpoint == EndpointKind::Rerank {
                        buffer_rerank_passthrough(resp, http, &req.body).await
                    } else {
                        buffer_passthrough(
                            resp,
                            http,
                            channel.unwrap_envelope,
                            req.endpoint == EndpointKind::Chat,
                        )
                        .await
                    };
                    if let Some(note) = empty {
                        record_attempt(
                            &state,
                            request_id,
                            ordinal,
                            channel,
                            name, &sent_model,
                            "empty_reply",
                            Some(status),
                            dur,
                            Some(&note),
                        );
                        ordinal += 1;
                        tracing::warn!("渠道 {} 回了 200 但一句话没说，按这条渠道的毛病处理", channel.name);
                        last_fault = Some(format!("{}：{note}", channel.name));
                        if cooldown.failure(&note,pending_retry_ms) {break;}
                        continue;
                    }
                    record_attempt(&state, request_id, ordinal, channel, name, &sent_model, "success", Some(status), now_ms()-t0, adaptation_note.as_deref());
                    cooldown.finish(true,"",0);
                    state.note_model_endpoint(name, req.endpoint);
                    // 命中的是首选就视为已回切；不是就记下当前渠道、重设检查点
                    if touch_state {
                        let mut r = state.routing();
                        r.on_success(&plan.route, channel.id, &all_ids, now_ms());
                        drop(r);
                        state.persist_state(&plan.route)?;
                    }
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
                    return Ok(resp);
                }
                AttemptOutcome::ClientFault => {
                    // 客户端自己的错：原样返回，**不重试不换渠道**（换个说法：一次写错的请求
                    // 不许把整条链路打断）
                    let err = read_error(resp).await;
                    if matches!(status, 400 | 413 | 422) && context_rejection(&err.text) {
                        record_attempt(&state, request_id, ordinal, channel, name, &sent_model,
                            "context_limit", Some(status), dur, Some(&err.text));
                        ordinal += 1;
                        last_fault = Some(format!("渠道 {} 上下文不足：{}", channel.name, err.text));
                        last_http = Some(err);
                        continue 'channels;
                    }
                    if req.endpoint == EndpointKind::Chat && matches!(status, 400 | 422) && !learned_this_request {
                        if let Some(cap) = output_ceiling(&err.text) {
                            if clamp_output(&mut body, cap) {
                                learned_this_request = true;
                                let _ = crate::db::settings::set(&state.conn(), &cap_key, &cap.to_string());
                                adaptation_note = Some(format!("上游明确输出上限为 {cap} tokens，已收窄重试"));
                                record_attempt(&state, request_id, ordinal, channel, name, &sent_model,
                                    "output_limit", Some(status), dur, adaptation_note.as_deref());
                                ordinal += 1;
                                body_bytes = Bytes::from(serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?);
                                if attempt_no < max_attempts { continue; }
                                last_http = Some(err);
                                continue 'channels;
                            }
                        }
                    }
                    record_attempt(&state, request_id, ordinal, channel, name, &sent_model, "client_fault", Some(status), dur, Some(&err.text));
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
                    let after = err
                        .retry_after
                        .as_ref()
                        .and_then(|v| v.to_str().ok())
                        .and_then(crate::upstream::retry_after_ms);
                    record_attempt(
                        &state,
                        request_id,
                        ordinal,
                        channel,
                        name, &sent_model,
                        "channel_fault",
                        Some(status),
                        dur,
                        Some(&err.text),
                    );
                    ordinal += 1;
                    last_fault = Some(format!("{}：HTTP {status} {}", channel.name, err.text));
                    pending_retry_ms=pending_retry_ms.max(after.unwrap_or(0) as i64);
                    let cooling=cooldown.failure(&format!("HTTP {status}: {}",err.text),pending_retry_ms);
                    last_http = Some(err);
                    if cooling {break;}
                    if let Some(ms) = after {
                        // Long Retry-After sets the eventual cooldown, not an unbounded request sleep.
                        let ms=ms.min(LONG_RETRY_AFTER_MS);
                        retry_after_override = Some(ms);
                    }
                    continue;
                }
                AttemptOutcome::ClientGone => {
                    record_attempt(&state, request_id, ordinal, channel, name, &sent_model, "client_gone", Some(status), dur, None);
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
            r.on_channel_exhausted(&plan.route, channel.id, &all_ids, now_ms());
            drop(r);
            state.persist_state(&plan.route)?;
            tracing::warn!("渠道 {} 连续 {} 次失败，切换", channel.name, max_attempts);
        } else {
            tracing::warn!(
                "（带图请求，不动路由状态）渠道 {} 连续 {} 次失败",
                channel.name,
                max_attempts
            );
        }
    }

        if tried_any || busy.is_empty() {
            break 'pass;
        }
        // 所有渠道都到并发上限了：在门口排队等一个名额，等到再重跑一遍。
        // 预算用「这次请求的剩余重试预算」，排到超时就按「上游太忙」如实回，不挂死。
        let (id, sem) = busy.remove(0);
        let left = cfg.request_budget_ms as i64 - (now_ms() - started_ms);
        if left <= 0 {
            last_fault = Some("所有渠道都到并发上限了，排队的预算也用完了".to_string());
            break 'pass;
        }
        match tokio::time::timeout(Duration::from_millis(left as u64), sem.acquire_owned()).await {
            Ok(Ok(p)) => {
                pre_permits.insert(id, p);
            }
            _ => {
                last_fault = Some("所有渠道都到并发上限了，排队等到超时".to_string());
                break 'pass;
            }
        }
    }

    // 一个候选都没敢碰（全在冷却或恢复试用里）：如实回 503 并说清最早什么时候能再试 ——
    // 别拿「所有渠道都失败了」去冤枉渠道（渠道根本没被调过）。普通模型名和 auto 一视同仁。
    if last_channel.0==0 {
        let c=state.conn();
        let until=model_routes::list(&c)?.iter().flat_map(|r|crate::db::policy::rows(&c,&r.name).unwrap_or_default()).map(|r|r.until_ms.max(r.lease_until)).filter(|t|*t>now_ms()).min();
        drop(c);
        let msg=format!("候选冷却或恢复试用中，请稍后重试{}",until.map(|t|format!("（最早可重试时间：{t}）")).unwrap_or_default());
        guard.error_code=Some("models_cooling".into());
        guard.finish(&state,Status::Failed,Some(503),None,started_ms,None,0,0,false,Some(&msg));
        return Ok(ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"models_cooling",msg).into_response());
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

// Only explicit output-parameter errors qualify; context/input numbers are not output caps.
fn output_ceiling(text: &str) -> Option<u64> {
    let text = text.to_ascii_lowercase();
    if context_rejection(&text) { return None; }
    let start = ["max_completion_tokens", "max_output_tokens", "max_tokens"]
        .iter().filter_map(|p| text.find(p)).min()?;
    let tail = &text[start..];
    for marker in ["must be less than or equal to", "must be less than", "at most", "maximum allowed value of", "maximum output tokens", "cannot exceed", "must not exceed"] {
        if let Some(pos) = tail.find(marker) {
            let rest = tail[pos + marker.len()..].trim_start_matches(|c: char| !c.is_ascii_digit());
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let n = digits.parse::<u64>().ok()?;
            let cap = if marker == "must be less than" { n.checked_sub(1)? } else { n };
            if (1..=16_777_216).contains(&cap) { return Some(cap); }
        }
    }
    None
}

fn context_rejection(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    ["context_length_exceeded", "contextwindowexceeded", "maximum context length", "context length exceeded", "context window exceeded", "input token count exceeds", "input is too long"].iter().any(|m| text.contains(m))
}

fn clamp_output(body: &mut Value, cap: u64) -> bool {
    let mut changed = false;
    for field in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        if body.get(field).and_then(Value::as_u64).is_some_and(|n| n > cap) {
            body[field] = Value::from(cap);
            changed = true;
        }
    }
    changed
}

fn output_cap_key(channel: &Channel, model: &str, secret: Option<&str>) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (channel.id.0, &channel.base_url, model, secret).hash(&mut h);
    format!("learned_output_cap:{:016x}", h.finish())
}

fn gateway_timeout(plan: &Plan, model: &str) -> Response {
    ApiError::new(
        StatusCode::GATEWAY_TIMEOUT,
        "upstream_timeout",
        format!(
            "重试预算用尽（模型「{}」，请求的模型「{model}」）",
            plan.route
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

/// 记一次尝试。**记账失败不许把请求带崩**：抢不到库锁就把这一件丢给后台落账员，
/// 日志里有痕迹就行。
struct CooldownTicket {state:SharedState,id:i64,name:String,upstream:String,generation:i64,done:bool}
impl CooldownTicket {
    fn failure(&mut self,reason:&str,retry:i64)->bool {
        if self.done {return true;}
        let c=self.state.conn();
        let min=if reason.contains("HTTP 401:")||reason.contains("HTTP 403:"){1800000}else{retry};
        if let Err(e)=crate::db::policy::complete(&c,self.id,&self.name,&self.upstream,self.generation,false,reason,min,now_ms()){tracing::warn!("冷却记录失败：{e}");}
        let cooling=crate::db::policy::peek(&c,self.id,&self.name,&self.upstream,now_ms()).ok().flatten().is_some();
        self.done=cooling;
        cooling
    }
    fn finish(&mut self,success:bool,reason:&str,retry:i64){
        if !success {self.failure(reason,retry);return;}
        if self.done{return;}
        self.done=true;
        let c=self.state.conn();
        if let Err(e)=crate::db::policy::complete(&c,self.id,&self.name,&self.upstream,self.generation,true,"",0,now_ms()){tracing::warn!("冷却记录失败：{e}");}
    }
}
impl Drop for CooldownTicket {fn drop(&mut self){if !self.done{let c=self.state.conn();let _=crate::db::policy::release(&c,self.id,&self.name,&self.upstream,self.generation);}}}

fn record_attempt(
    state: &SharedState,
    request_id: i64,
    ordinal: u32,
    channel: &Channel,
    model_name: &str,
    upstream_model: &str,
    outcome: &str,
    status: Option<u16>,
    duration_ms: i64,
    error: Option<&str>,
) {
    let args = requests::AttemptArgs {
        model_name:model_name.to_string(),upstream_model:upstream_model.to_string(),first_byte_ms:None,
        request_id,
        ordinal,
        channel_id: channel.id.0,
        outcome: outcome.to_string(),
        http_status: status,
        duration_ms,
        error: error.map(str::to_string),
    };
    match state.db.try_lock() {
        Ok(conn) => {
            if let Err(e) = requests::add_attempt_args(&conn, &args) {
                tracing::error!("记账失败（尝试级）：{e}；交给后台补写");
                state.enqueue_write(crate::state::WriteJob::Attempt(args));
            }
        }
        Err(_) => state.enqueue_write(crate::state::WriteJob::Attempt(args)),
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
///
/// `unwrap_envelope`：这条渠道勾了「响应解包」时，先把外面那层壳剥掉再回客户端与记账
/// （cline.bot 的形状：`{"data": {<标准响应>}, "success": true}`，不剥客户端读顶层
/// `choices` 只能读到空）。**只在真认出那种形状时才剥**，其余一律原样透传。
///
/// `check_empty`：对话端点要顺手看一眼「它到底说没说话」。回了 200 但一句话没有的，
/// 第四个返回值给出说明 —— 调用方拿它当**这条渠道的毛病**去重试/换渠道，而不是把空泡泡
/// 原样丢给用户。
#[allow(clippy::type_complexity)]
async fn buffer_passthrough(
    resp: reqwest::Response,
    status: StatusCode,
    unwrap_envelope: bool,
    check_empty: bool,
) -> (Response, i64, i64, Option<String>) {
    let ctype = content_type(&resp);
    let bytes = match resp.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => return invalid_buffer(format!("upstream body read failed: {e}")),
    };
    let mut out = bytes.clone();
    let mut empty: Option<String> = None;
    let (input, output) = match serde_json::from_slice::<Value>(&bytes) {
        Ok(mut v) => {
            if unwrap_envelope && tokengo_shared::unwrap_envelope(&mut v) {
                match serde_json::to_vec(&v) {
                    Ok(b) => out = Bytes::from(b),
                    Err(e) => tracing::warn!("剥壳后重新序列化失败，原样回：{e}"),
                }
            }
            let mut validated = v.clone();
            // A configured raw envelope remains byte-for-byte compatible, but its payload must be valid.
            tokengo_shared::unwrap_envelope(&mut validated);
            if let Err(note) = crate::upstream::validate_response(&validated, if check_empty { "chat/completions" } else { "embeddings" }) {
                return invalid_buffer(note);
            }
            if check_empty && tokengo_shared::empty_chat_reply(&v) {
                let head: String = String::from_utf8_lossy(&out).chars().take(200).collect();
                empty = Some(format!(
                    "上游回了 200 但一句话都没说（没 content、没 tool_calls、没推理内容）：{head}"
                ));
            }
            usage_from_value(&v)
        }
        Err(e) => return invalid_buffer(format!("invalid upstream JSON: {e}")),
    };
    let mut b = Response::builder().status(status);
    if let Some(ct) = ctype {
        b = b.header(header::CONTENT_TYPE, ct);
    }
    (
        b.body(Body::from(out))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()),
        input,
        output,
        empty,
    )
}

fn invalid_buffer(note: String) -> (Response, i64, i64, Option<String>) {
    (StatusCode::BAD_GATEWAY.into_response(), 0, 0, Some(note))
}

/// 重排：整段读下来再回给客户端；上游 `document` 为 null 时按 index 回填请求里的原文。
async fn buffer_rerank_passthrough(
    resp: reqwest::Response,
    status: StatusCode,
    req_body: &Value,
) -> (Response, i64, i64, Option<String>) {
    let ctype = content_type(&resp);
    let bytes = match resp.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => return invalid_buffer(format!("upstream body read failed: {e}")),
    };
    let docs = req_body
        .get("documents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut parsed: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return invalid_buffer(format!("invalid upstream JSON: {e}")),
    };
    if let Err(note) = crate::upstream::validate_response(&parsed, "rerank") {
        return invalid_buffer(note);
    }
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
        None,
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
    request_id: i64,
    started_ms: i64,
    channel_id: i64,
    gate: Option<tokio::sync::OwnedSemaphorePermit>,
    endpoint: EndpointKind,
    ordinal:u32, model_name:String, upstream_model:String, attempt_started:i64, adaptation_note:Option<String>, cooldown:CooldownTicket, load_ticket:crate::state::ModelLoadTicket, idle_timeout:Duration,
) -> Response {
    let status = resp.status();
    let ctype = content_type(&resp).unwrap_or_else(|| "text/event-stream".to_string());
    let st = StreamState {
        request_id,ordinal,model_name,upstream_model,attempt_started,adaptation_note,cooldown,_model_load:load_ticket,
        resp,
        scanner: SseScanner::default(),
        endpoint,
        first_byte_ms: None,
        guard: Some(guard),
        started_ms,
        channel_id,
        state,
        delivered: 0,
        idle_timeout,
        _gate: gate,
    };
    let stream = unfold(st, |mut st| async move {
        match tokio::time::timeout(st.idle_timeout, st.resp.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                if st.first_byte_ms.is_none() && !chunk.is_empty() { st.first_byte_ms=Some(now_ms()-st.started_ms); }
                st.scanner.feed(&chunk);
                st.delivered += chunk.len();
                Some((Ok::<Bytes, std::io::Error>(chunk), st))
            }
            Ok(Ok(None)) => {
                st.finish(None);
                None
            }
            Ok(Err(e)) => {
                st.finish(Some("上游流式连接异常中断；已输出内容不重试"));
                tracing::warn!("流式响应中途断了：{e}");
                Some((
                    Err(std::io::Error::other(format!("上游断流：{e}"))),
                    st,
                ))
            }
            Err(_) => {
                let note = format!("上游流式空闲超时：连续 {} 毫秒未收到数据；已输出内容不重试", st.idle_timeout.as_millis());
                st.finish(Some(&note));
                Some((Err(std::io::Error::new(std::io::ErrorKind::TimedOut, note)), st))
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
    _model_load:crate::state::ModelLoadTicket,
    cooldown:CooldownTicket,
    request_id:i64,ordinal:u32,model_name:String,upstream_model:String,attempt_started:i64,
    adaptation_note:Option<String>,
    first_byte_ms: Option<i64>,
    endpoint: EndpointKind,
    resp: reqwest::Response,
    scanner: SseScanner,
    guard: Option<RequestGuard>,
    started_ms: i64,
    channel_id: i64,
    state: SharedState,
    delivered: usize,
    idle_timeout: Duration,
    /// 这条渠道的并发名额。**流式期间一直握着**：不然名额一还，
    /// 明明还在往外吐字的那条流就不算数了，上游照样被打爆。
    _gate: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl StreamState {
    /// `transport_error` distinguishes idle/transport failure from a clean EOF.
    ///
    /// 流完了但**一块真内容都没有**也算失败：客户端刚才收到的是一个空泡泡，
    /// 我们虽然改不了已经发出去的字节，但账要记对、原因要写清（不然只能看到一条"成功"）。
    fn finish(&mut self, transport_error: Option<&str>) {
        let Some(mut guard) = self.guard.take() else {
            return;
        };
        if transport_error.is_none() { self.scanner.finish_eof(); }
        let (input, output) = self.scanner.totals();
        let ended = self.scanner.terminal();
        let complete = transport_error.is_none() && ended && !self.scanner.error;
        let empty = complete && !self.scanner.saw_payload();
        let note = if let Some(error) = transport_error {
            Some(error)
        } else if self.scanner.error {
            Some("上游流式返回错误事件，已发出去的内容收不回来")
        } else if !ended {
            Some("上游流式缺少结束标记，回答可能未完成；已输出内容不重试")
        } else if empty {
            Some("上游流里一个字都没说（没 content、没 tool_calls、没推理内容）")
        } else {
            None
        };
        self.cooldown.finish(complete && !empty,note.unwrap_or(""),0);
        let args=requests::AttemptArgs{request_id:self.request_id,ordinal:self.ordinal,channel_id:self.channel_id,model_name:self.model_name.clone(),upstream_model:self.upstream_model.clone(),outcome:if complete && !empty {"success"}else{"channel_fault"}.into(),http_status:None,duration_ms:now_ms()-self.attempt_started,error:note.map(str::to_string).or_else(||self.adaptation_note.clone()),first_byte_ms:self.first_byte_ms.map(|ms| (ms+self.started_ms-self.attempt_started).max(0))};
        match self.state.db.try_lock(){Ok(c)=>{if requests::add_attempt_args(&c,&args).is_err(){self.state.enqueue_write(crate::state::WriteJob::Attempt(args));}},Err(_)=>self.state.enqueue_write(crate::state::WriteJob::Attempt(args))}

        if complete && !empty {
            if let Some(name) = &guard.served_model {
                self.state.note_model_endpoint(name, self.endpoint);
            }
        }
        guard.finish(
            &self.state,
            if complete && !empty { Status::Ok } else { Status::Failed },
            None,
            Some(self.channel_id),
            self.started_ms,
            self.first_byte_ms,
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
    /// 整条流里到底有没有真内容（正文 / 工具调用 / 推理内容）——
    /// 流式不能等读完再判断（字节早发出去了），只能边转边记。
    payload: bool,
    done_event: bool,
    finished_choices: std::collections::HashSet<u64>,
    seen_choices: std::collections::HashSet<u64>,
    error: bool,
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
        if data == "[DONE]" {
            self.done_event = true;
            return;
        }
        if data.is_empty() {
            return;
        }
        if let Ok(v) = serde_json::from_str::<Value>(data) {
            if v.get("error").is_some_and(|e| !e.is_null()) {
                self.error = true;
            }
            if let Some(choices) = v.get("choices").and_then(Value::as_array) {
                for choice in choices {
                    let index = choice.get("index").and_then(Value::as_u64).unwrap_or(0);
                    // Bound metadata even for malformed upstreams with ever-changing indices.
                    if self.seen_choices.len() < 128 || self.seen_choices.contains(&index) {
                        self.seen_choices.insert(index);
                        if choice.get("finish_reason").and_then(Value::as_str).is_some_and(|r| !r.is_empty()) {
                            self.finished_choices.insert(index);
                        }
                    } else {
                        self.error = true;
                    }
                }
            }
            if tokengo_shared::chunk_has_payload(&v) {
                self.payload = true;
            }
            let (i, o) = usage_from_value(&v);
            if i > 0 || o > 0 {
                self.input = i;
                self.output = o;
            }
        }
    }

    fn finish_eof(&mut self) {
        // Accept a final SSE data line even when its trailing newline is absent.
        let tail = std::mem::take(&mut self.pending);
        if !tail.is_empty() { self.line(&tail); }
    }

    fn terminal(&self) -> bool {
        self.done_event || (!self.seen_choices.is_empty()
            && self.seen_choices.is_subset(&self.finished_choices))
    }

    pub fn totals(&self) -> (i64, i64) {
        (self.input, self.output)
    }

    /// 整条流里有没有出现过真内容（判断「上游一个字都没说」用的）。
    pub fn saw_payload(&self) -> bool {
        self.payload
    }
}

/// 把「这条请求压根没能路由出去」记成一条 `client_error` 账（含 HTTP 状态与原因）。
///
/// 不写 attempt（一次上游都没打过）、不改路由状态（不是渠道的毛病）。
/// **失败一律吞掉**：记账是为排障，绝不能因为它把请求结果改了。
fn record_routing_refusal(
    state: &SharedState,
    req: &ForwardRequest,
    started_ms: i64,
    http: u16,
    msg: &str,
) {
    let has_image =
        req.endpoint == EndpointKind::Chat && tokengo_shared::request_has_image(&req.body);
    let request_id = {
        let conn = state.conn();
        match crate::db::requests::start(
            &conn,
            req.endpoint.as_str(),
            &req.model,
            req.client_ip.as_deref(),
            has_image,
        ) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("路由不出去这一发没能开账（不影响请求结果）：{e}");
                return;
            }
        }
    };
    let mut guard = RequestGuard::new(state.clone(), request_id, started_ms);
    record_client_app(&state,request_id,req.user_agent.as_deref());
    guard.error_code=Some(if http==503 {"models_cooling"} else if http==422 {"vision_not_ready"} else if model_routes::is_auto(&req.model) {"auto_no_candidate"} else {"model_not_configured"}.into());

    guard.finish(
        state,
        if http==503 {Status::Failed} else {Status::ClientError},
        Some(http),
        None,
        started_ms,
        None,
        0,
        0,
        false,
        Some(msg),
    );
}

/// 请求收尾器：谁先到谁落账，只落一次。
///
/// 正常路径显式调 `finish`；如果这个结构是被 **Drop** 掉的（客户端中途断开时，
/// axum/hyper 会把整个处理函数连同它一起丢掉），就记成 `canceled` ——
/// 客户端自己断开不算渠道的毛病，不该进渠道的账。
pub fn record_client_app(state:&SharedState,id:i64,agent:Option<&str>) {
    if let Some(agent)=agent { let conn=state.conn();
        if let Err(e)=conn.execute("UPDATE requests SET user_agent=?2 WHERE id=?1",rusqlite::params![id,agent.chars().take(512).collect::<String>()]) {
            tracing::warn!("记录请求应用失败：{e}");
        }
    }
}

struct RequestGuard {
    channel_id: Option<i64>,
    state: SharedState,
    request_id: i64,
    started_ms: i64,
    done: bool,
    /// 这一发真正发给上游的模型名（`auto` 时是它挑中的那个）。
    /// 收尾时落进 `requests.served_model` —— 没有这一列，面板上只能看到
    /// 一条「auto 成功了」，看不出到底走了谁。
    served_model: Option<String>,
    error_code:Option<String>,
}

impl RequestGuard {
    pub fn new(state: SharedState, request_id: i64, started_ms: i64) -> Self {
        Self {
            state,
            request_id,
            started_ms,
            done: false,
            channel_id: None,
            served_model: None,
            error_code:None,
        }
    }

    /// 记下这一发真正发给上游的模型名。
    pub fn note_served(&mut self, name: &str) {
        self.served_model = Some(name.to_string());
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
        if matches!(status, Status::ClientError) && self.error_code.is_none() {
            self.error_code=Some(if http_status==Some(422) && error.map(|s|s.contains("图片")).unwrap_or(false) {"vision_not_ready"} else {"request_invalid"}.into());
        }
        // 记账不许把请求带崩：抢不到库锁就丢给后台落账员（2026-10-18 压测踩出来的规矩）
        let args = requests::FinishArgs {
            request_id: self.request_id,
            status,
            http_status,
            channel_id: channel_id.or(self.channel_id),
            latency_ms: now_ms() - started_ms,
            first_byte_ms,
            input_tokens,
            output_tokens,
            streamed,
            error: error.map(str::to_string),
            served_model: self.served_model.clone(),
            error_code:self.error_code.clone(),
        };
        match state.db.try_lock() {
            Ok(conn) => {
                if let Err(e) = requests::finish_args(&conn, &args) {
                    tracing::error!("记账失败（请求级）：{e}；交给后台补写");
                    state.enqueue_write(crate::state::WriteJob::Finish(args));
                }
            }
            Err(_) => state.enqueue_write(crate::state::WriteJob::Finish(args)),
        }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        // 请求被中断（客户端断开等）也要记上一条 canceled —— 以前这里抢不到库锁就整条丢掉，
        // 面板上那一发就永远停在「进行中」。现在抢不到就丢给后台落账员补写。
        let args = requests::FinishArgs {
            request_id: self.request_id,
            status: Status::Canceled,
            http_status: None,
            channel_id: self.channel_id,
            latency_ms: now_ms() - self.started_ms,
            first_byte_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            streamed: false,
            error: Some("客户端断开（处理函数被丢弃）".to_string()),
            served_model: self.served_model.clone(),
            error_code:self.error_code.clone(),
        };
        match self.state.db.try_lock() {
            Ok(conn) => {
                if let Err(e) = requests::finish_args(&conn, &args) {
                    tracing::warn!("请求 {} 记 canceled 失败：{e}；交给后台补写", self.request_id);
                    self.state.enqueue_write(crate::state::WriteJob::Finish(args));
                }
            }
            Err(_) => self.state.enqueue_write(crate::state::WriteJob::Finish(args)),
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
    fn sse_completion_requires_done_or_all_choices_finished_and_no_error() {
        let mut s = SseScanner::default();
        s.feed(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"half\"}},{\"index\":1,\"delta\":{\"content\":\"half\"}}]}\n\n");
        assert!(!s.terminal());
        s.feed(b"data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}]}\n\n");
        assert!(!s.terminal(), "another choice is still incomplete");
        s.feed(b"data: {\"choices\":[{\"index\":1,\"finish_reason\":\"tool_calls\"}]}\n\n");
        assert!(s.terminal(), "explicit finish reasons work without DONE");
        let mut done = SseScanner::default();
        done.feed(b"data: [DO");
        done.feed(b"NE]\r\n\r\n");
        assert!(done.terminal());
        done.feed(b"data: {\"error\":{\"message\":\"broken\"}}\n\n");
        assert!(done.error, "DONE must not hide an upstream error event");
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
    fn limits_only_learn_explicit_output_caps() {
        assert_eq!(output_ceiling("max_tokens must be less than or equal to 8192"), Some(8192));
        assert_eq!(output_ceiling("max_completion_tokens must be less than 100"), Some(99));
        assert_eq!(output_ceiling("maximum context length 8192, max_tokens 4000"), None);
        assert_eq!(output_ceiling("429 retry after 10"), None);
        let mut v = json!({"max_tokens":32000,"temperature":0.5});
        assert!(clamp_output(&mut v, 8192));
        assert_eq!(v["max_tokens"], 8192);
        assert_eq!(v["temperature"], 0.5);
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
