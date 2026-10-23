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
#[cfg(test)]
mod oauth_tests;

/// 对外端点。只有三个（依据：3 天 10 万次请求里向量 86%、对话 14%，其余 41 个端点 0 次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    Chat,
    Responses,
    Messages,
    Embeddings,
    Rerank,
    Search,
    Fetch,
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
    pub fn protocol(self) -> Option<crate::protocol::Protocol> {
        use crate::protocol::Protocol;
        match self { Self::Chat=>Some(Protocol::Chat), Self::Responses=>Some(Protocol::Responses), Self::Messages=>Some(Protocol::Messages), _=>None }
    }
    pub fn is_generation(self)->bool {self.protocol().is_some()}
    pub fn tool_kind(self)->Option<crate::search_tools::ToolKind> {match self {Self::Search=>Some(crate::search_tools::ToolKind::Search),Self::Fetch=>Some(crate::search_tools::ToolKind::Fetch),_=>None}}
    fn has_image(self,body:&Value)->bool {self.protocol().is_some_and(|p|crate::protocol::has_image(p,body))}
    /// 打上游时接在 base_url 后面的路径。
    pub fn path(self) -> &'static str {
        match self {
            EndpointKind::Chat => "chat/completions",
            EndpointKind::Responses => "responses",
            EndpointKind::Messages => "messages",
            EndpointKind::Embeddings => "embeddings",
            EndpointKind::Rerank => "rerank",
            EndpointKind::Search => "tools/search",
            EndpointKind::Fetch => "tools/fetch",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EndpointKind::Chat => "/v1/chat/completions",
            EndpointKind::Responses => "/v1/responses",
            EndpointKind::Messages => "/v1/messages",
            EndpointKind::Embeddings => "/v1/embeddings",
            EndpointKind::Rerank => "/v1/rerank",
            EndpointKind::Search => "/v1/tools/search",
            EndpointKind::Fetch => "/v1/tools/fetch",
        }
    }

    /// 这个端点对应的模型类型（写进模型组、回给客户端的那种）。
    pub fn as_model_kind(self) -> tokengo_shared::ModelKind {
        match self {
            EndpointKind::Chat | EndpointKind::Responses | EndpointKind::Messages => tokengo_shared::ModelKind::Chat,
            EndpointKind::Embeddings => tokengo_shared::ModelKind::Embeddings,
            EndpointKind::Rerank => tokengo_shared::ModelKind::Rerank,
            EndpointKind::Search | EndpointKind::Fetch => tokengo_shared::ModelKind::Unknown,
        }
    }
}

/// 一次转发的输入。
pub struct ForwardRequest {
    /// Authenticated identity. None is reserved for trusted in-process diagnostics/tests.
    pub auth: Option<crate::db::identity::AuthContext>,
    pub session_namespace: String,
    /// Validated explicit conversation identity resolved from inbound headers/body.
    pub explicit_session: Option<String>,
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
    revision: i64,
    protocols: crate::protocol_config::Capabilities,
    binding_generation: i64,
    credential: Option<crate::db::provider_pool::CredentialRef>,
    target: Option<crate::db::user_virtual_routes::ResolvedTarget>,
}

/// 这次请求要试的候选链（已按顺序排好）+ 各自的密钥。
struct Plan {
    /// 这一链挂在哪个名字下（普通模型名 = 归一后的那个名字；auto 时是保留名）。
    route: String,
    virtual_route: Option<crate::db::user_virtual_routes::ResolvedVirtualPlan>,
    /// 是不是保留名 `auto`：auto 现场挑、不写路由状态、不做回切。
    auto: bool,
    entries: Vec<Candidate>,
    secrets: HashMap<i64, String>,
    /// 每条渠道「同时最多几发」（渠道 id → 上限；0 或缺席 = 不限）。
    caps: HashMap<i64, u32>,
    explored: Option<String>,
    load_ticket: Option<(String, crate::state::ModelLoadTicket)>,
}

impl Plan {
    fn vector_contract(&self) -> Option<&crate::vector_compat::Contract> {
        self.virtual_route.as_ref().and_then(|route| route.compatibility.as_ref())
    }
    fn automatic_target_failover(&self) -> bool {
        self.vector_contract().is_some() || self.virtual_route.as_ref().is_some_and(|r| r.shared)
    }
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

type GatewayPermits = (
    Option<crate::state::ChannelPermit>,
    Option<crate::state::ChannelPermit>,
);
type PoolGroup = (Option<usize>, i64, String);
#[derive(Default)]
struct PoolControl {
    selected: HashMap<PoolGroup, i64>,
    seen: std::collections::HashSet<(Option<usize>, i64, String)>,
    tokens: HashMap<i64, crate::db::provider_pool::AffinityToken>,
}
fn pool_group(
    target: Option<&crate::db::user_virtual_routes::ResolvedTarget>,
    credential: &crate::db::provider_pool::CredentialRef,
    name: &str,
) -> PoolGroup {
    (
        target.map(|target| target.position),
        credential.provider_id,
        name.to_owned(),
    )
}
fn virtual_route_error(error: crate::db::user_virtual_routes::VirtualError) -> ApiError {
    let status = match error.code {
        "vector_compatibility_renewal_failed" | "vector_compatibility_renewing" => StatusCode::SERVICE_UNAVAILABLE,
        "vector_fallback_unverified" | "vector_fallback_incompatible" | "vector_request_unverified" => StatusCode::BAD_REQUEST,
        "route_time_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        "virtual_route_changed" => StatusCode::CONFLICT,
        _ => StatusCode::FORBIDDEN,
    };
    ApiError::new(status, error.code, error.message)
}
fn order_pool_entries(
    state: &SharedState,
    plan: &mut Plan,
    auth: &crate::db::identity::AuthContext,
    endpoint: EndpointKind,
) -> Result<(), ApiError> {
    let c = state.conn();
    let mut eligible = Vec::new();
    for entry in std::mem::take(&mut plan.entries) {
        let usable = entry.credential.is_none()
            || (!crate::db::faults::blocked(&c, entry.channel.id.0, &entry.name, now_ms())?
                && crate::db::policy::peek(
                    &c,
                    entry.channel.id.0,
                    &entry.name,
                    entry.channel.upstream_model(&entry.name),
                    now_ms(),
                )?
                .is_none());
        if usable {
            eligible.push(entry);
        }
    }
    plan.entries = eligible;
    let mut groups: HashMap<PoolGroup, Vec<usize>> = HashMap::new();
    for (index, entry) in plan.entries.iter().enumerate() {
        if let Some(credential) = &entry.credential {
            // Protocol-ineligible sources cannot establish affinity for an unusable key.
            if endpoint
                .protocol()
                .is_some_and(|protocol| entry.protocols.select(protocol).is_none())
            {
                continue;
            }
            groups
                .entry(pool_group(entry.target.as_ref(), credential, &entry.name))
                .or_default()
                .push(index);
        }
    }
    let mut entries: Vec<Option<Candidate>> = std::mem::take(&mut plan.entries)
        .into_iter()
        .map(Some)
        .collect();
    for ((_, provider, _), indices) in groups {
        let ids: Vec<i64> = indices
            .iter()
            .map(|index| entries[*index].as_ref().unwrap().channel.id.0)
            .collect();
        let loads: HashMap<i64, usize> = indices
            .iter()
            .filter_map(|index| entries[*index].as_ref().unwrap().credential.as_ref())
            .map(|credential| (credential.id, state.credential_load(credential.id)))
            .collect();
        let ordered = crate::db::provider_pool::order_for_user_with_load(
            &c,
            auth.user_id,
            provider,
            &ids,
            &loads,
        )?;
        let mut available: HashMap<i64, Candidate> = indices
            .iter()
            .map(|index| {
                let entry = entries[*index].take().unwrap();
                (entry.channel.id.0, entry)
            })
            .collect();
        for (index, id) in indices.into_iter().zip(ordered) {
            entries[index] = available.remove(&id);
        }
        // Fail closed if the pool omitted disabled/stale sources rather than reinserting them.
    }
    plan.entries = entries.into_iter().flatten().collect();
    Ok(())
}
fn defer_explicit_retry_after(
    state: &SharedState,
    plan: &Plan,
    index: usize,
    error: &UpstreamError,
) -> Result<(), ApiError> {
    let Some(delay) = error
        .retry_after
        .as_ref()
        .and_then(|value| value.to_str().ok())
        .and_then(crate::upstream::retry_after_ms)
        .filter(|delay| *delay > 0)
    else {
        return Ok(());
    };
    let entry = &plan.entries[index];
    let until = now_ms().saturating_add(delay.min(i64::MAX as u64) as i64);
    let c = state.conn();
    for source in plan.entries.iter().filter(|source| {
        source.name == entry.name
            && match (&source.credential, &entry.credential) {
                (Some(a), Some(b)) => a.id == b.id,
                _ => source.channel.id == entry.channel.id,
            }
    }) {
        let generation: i64 = c
            .query_row(
                "SELECT binding_generation FROM channels WHERE id=?1",
                [source.channel.id.0],
                |row| row.get(0),
            )
            .map_err(crate::db::DbError::from)?;
        if generation != source.binding_generation {
            continue;
        }
        c.execute("INSERT OR IGNORE INTO model_cooldowns(channel_id,model_name,upstream_model) VALUES(?1,?2,?3)",rusqlite::params![source.channel.id.0,tokengo_shared::normalize_model_name(&source.name),source.channel.upstream_model(&source.name)]).map_err(crate::db::DbError::from)?;
        c.execute("UPDATE model_cooldowns SET until_ms=MAX(until_ms,?4),reason='HTTP 429 Retry-After',lease_until=0 WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3",rusqlite::params![source.channel.id.0,tokengo_shared::normalize_model_name(&source.name),source.channel.upstream_model(&source.name),until]).map_err(crate::db::DbError::from)?;
    }
    Ok(())
}
fn advance_explicit_route(
    state: &SharedState,
    plan: &Plan,
    index: usize,
    status: u16,
    auth: Option<&crate::db::identity::AuthContext>,
    pinned: bool,
    control: &mut PoolControl,
    active_target: &mut Option<usize>,
) -> Result<bool, ApiError> {
    if pinned {
        return Ok(false);
    }
    let entry = &plan.entries[index];
    if let (Some(credential), Some(auth)) = (&entry.credential, auth) {
        let current =
            crate::db::provider_pool::source_credential(&state.conn(), entry.channel.id.0)?;
        if current.as_ref().is_none_or(|current| {
            !current.enabled
                || current.id != credential.id
                || current.revision != credential.revision
                || current.provider_version != credential.provider_version
        }) {
            return Err(ApiError::new(StatusCode::CONFLICT,"credential_changed","Credential changed while the previous attempt was running; retry with current configuration"));
        }
        let allowed = match status {
            429 => credential.switch_on_429,
            400 => credential.switch_on_400,
            _ => false,
        };
        if allowed {
            let group = pool_group(entry.target.as_ref(), credential, &entry.name);
            if let Some(next) = plan.entries.iter().skip(index + 1).find(|next| {
                next.credential.as_ref().is_some_and(|next_credential| {
                    pool_group(next.target.as_ref(), next_credential, &next.name) == group
                        && next_credential.id != credential.id
                        && !control.seen.contains(&(
                            next.target.as_ref().map(|target| target.position),
                            next_credential.id,
                            next.name.clone(),
                        ))
                })
            }) {
                let next_credential = next.credential.as_ref().unwrap();
                if let Some(token) = control.tokens.get(&credential.provider_id) {
                    if token.credential_id == credential.id
                        && !crate::db::provider_pool::switch_affinity(
                            &state.conn(),
                            auth.user_id,
                            credential.provider_id,
                            token,
                            next_credential.id,
                        )?
                    {
                        return Err(ApiError::new(StatusCode::CONFLICT,"affinity_changed","Concurrent request already changed this user's upstream key; retry with current affinity"));
                    }
                    if let Some(token) = crate::db::provider_pool::affinity_token(
                        &state.conn(),
                        auth.user_id,
                        credential.provider_id,
                    )? {
                        control.tokens.insert(credential.provider_id, token);
                    }
                }
                control.selected.insert(group, next_credential.id);
                return Ok(true);
            }
        }
    }
    if let Some(target) = &entry.target {
        let allowed = match status {
            429 => target.switch_on_429,
            400 => target.switch_on_400,
            _ => false,
        };
        if allowed {
            if let Some(next) = plan
                .entries
                .iter()
                .skip(index + 1)
                .filter_map(|next| next.target.as_ref())
                .find(|next| next.position != target.position)
            {
                *active_target = Some(next.position);
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// 把一条候选挂进链子（顺手把它的密钥与并发上限捞出来）。
fn push_entry(
    state: &SharedState,
    conn: &rusqlite::Connection,
    plan: &mut Plan,
    name: String,
    mut channel: Channel,
) -> Result<(), ApiError> {
    channel.headers = channels::headers(conn, &state.secrets, channel.id)?;
    if let Some(s) =
        crate::db::provider_pool::load_source_secret(conn, &state.secrets, channel.id.0)?
    {
        plan.secrets.insert(channel.id.0, s);
    }
    if channel.max_concurrent > 0 {
        plan.caps.insert(channel.id.0, channel.max_concurrent);
    }
    let revision = crate::db::faults::revision(conn, channel.id.0)?;
    let protocols = crate::protocol_config::load(conn, channel.id.0)?;
    let binding_generation = conn
        .query_row(
            "SELECT binding_generation FROM channels WHERE id=?1",
            [channel.id.0],
            |r| r.get(0),
        )
        .map_err(crate::db::DbError::from)?;
    let credential = crate::db::provider_pool::source_credential(conn, channel.id.0)?;
    if credential
        .as_ref()
        .is_some_and(|credential| !credential.enabled)
        || (credential.is_none()
            && conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM source_credentials WHERE source_id=?1)",
                    [channel.id.0],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(crate::db::DbError::from)?)
    {
        return Ok(());
    }
    plan.entries.push(Candidate {
        name,
        channel,
        revision,
        protocols,
        binding_generation,
        credential,
        target: None,
    });
    Ok(())
}

/// Authorization covers the logical name and the physical source, including unknown prices.
pub fn permitted_candidate(c:&rusqlite::Connection,auth:&crate::db::identity::AuthContext,name:&str,channel:i64)->Result<bool,ApiError> {
    permitted_candidate_for(c,auth,name,channel,EndpointKind::Chat)
}
fn permitted_candidate_for(
    c: &rusqlite::Connection,
    auth: &crate::db::identity::AuthContext,
    name: &str,
    channel: i64,
    endpoint: EndpointKind,
) -> Result<bool, ApiError> {
    permitted_candidate_access(c, auth, name, channel, endpoint, false)
}
fn permitted_plan_candidate(
    c: &rusqlite::Connection,
    auth: &crate::db::identity::AuthContext,
    route: Option<&crate::db::user_virtual_routes::ResolvedVirtualPlan>,
    name: &str, channel: i64, endpoint: EndpointKind,
) -> Result<bool, ApiError> {
    if let Some(route)=route {if let Some(contract)=&route.compatibility {if !crate::vector_renewal::source_allowed(c,if route.shared{0}else{route.user_id},contract,channel,name).map_err(virtual_route_error)?{return Ok(false);}}}
    if let Some(route) = route.filter(|r| r.shared) {
        if !auth.permits_model(&route.name) { return Ok(false); }
        permitted_candidate_access(c, auth, name, channel, endpoint, true)
    } else { permitted_candidate_for(c, auth, name, channel, endpoint) }
}
fn permitted_candidate_access(
    c: &rusqlite::Connection,
    auth: &crate::db::identity::AuthContext,
    name: &str, channel: i64, endpoint: EndpointKind, delegated: bool,
) -> Result<bool, ApiError> {
    if !delegated && crate::db::model_access::internal_only(c, name)? { return Ok(false); }
    let credential = crate::db::provider_pool::source_credential(c, channel)?;
    if credential
        .as_ref()
        .is_some_and(|credential| !credential.enabled)
        || (credential.is_none()
            && c.query_row(
                "SELECT EXISTS(SELECT 1 FROM source_credentials WHERE source_id=?1)",
                [channel],
                |row| row.get::<_, bool>(0),
            )
            .map_err(crate::db::DbError::from)?)
    {
        return Ok(false);
    }
    if !delegated && !auth.permits_model(name) {
        return Ok(false);
    }
    let paid = if endpoint.tool_kind().is_some() {
        crate::db::billing::request_price(c, name, channel)?.is_none_or(|p| p.micro_per_call > 0)
    } else {
        crate::db::billing::price(c, name, channel)?
            .is_none_or(|p| p.input_micro_per_million > 0 || p.output_micro_per_million > 0)
    };
    Ok(auth.permits_channel(channel, paid))
}
pub fn permitted_model(c:&rusqlite::Connection,auth:&crate::db::identity::AuthContext,name:&str)->Result<bool,ApiError> {
    if !auth.permits_model(name) {return Ok(false);}
    for channel in model_routes::channels_for(c,name)? {
        if channel.routable() && permitted_candidate(c,auth,name,channel.id.0)? {return Ok(true);}
    }
    Ok(false)
}
fn choices_count(body:&Value)->Result<i64,ApiError> {
    match body.get("n") {
        None=>Ok(1),
        Some(n)=>n.as_i64().filter(|n|(1..=128).contains(n))
            .ok_or_else(||ApiError::bad_request("n must be an integer between 1 and 128")),
    }
}
fn output_bound(body:&Value,endpoint:EndpointKind)->Result<i64,ApiError> {
    if !endpoint.is_generation() {return Ok(0);}
    let wrong_cap=match endpoint {
        EndpointKind::Responses=>body.get("max_tokens").is_some()||body.get("max_completion_tokens").is_some(),
        EndpointKind::Messages=>body.get("max_output_tokens").is_some()||body.get("max_completion_tokens").is_some(),
        EndpointKind::Chat=>body.get("max_output_tokens").is_some(),_=>false,
    };
    if wrong_cap {return Err(ApiError::bad_request("Output cap field does not belong to the requested protocol"));}
    let mut bound=None;
    let keys:&[&str]=match endpoint {EndpointKind::Responses=>&["max_output_tokens"],EndpointKind::Messages=>&["max_tokens"],_=>&["max_tokens","max_completion_tokens"]};
    for key in keys {
        if let Some(value)=body.get(key) {
            let n=value.as_i64().filter(|n|*n>0).ok_or_else(||ApiError::bad_request("Output token limit must be a positive integer"))?;
            if bound.is_some_and(|old|old!=n) {return Err(ApiError::bad_request("Conflicting output token limits"));}
            bound=Some(n);
        }
    }
    bound.unwrap_or(4096).checked_mul(if endpoint==EndpointKind::Chat {choices_count(body)?} else {1})
        .ok_or_else(||ApiError::bad_request("Total output token bound overflow"))
}
fn admission_bound(body:&Value,endpoint:EndpointKind)->Result<(i64,i64),ApiError> {
    if endpoint.tool_kind().is_some() {return Ok((0,0));}
    let bytes = serde_json::to_vec(body).map_err(|e|ApiError::bad_request(e.to_string()))?.len();
    // Cross encoders process the query once per document. Counting it once can
    // under-reserve both TPM and money even inside a bounded fallback domain.
    let extra = if endpoint == EndpointKind::Rerank {
        body["query"].as_str().map_or(0, str::len)
            .checked_mul(body["documents"].as_array().map_or(0, |docs| docs.len().saturating_sub(1)))
    } else { Some(0) }.ok_or_else(||ApiError::bad_request("Request token bound overflow"))?;
    let input=i64::try_from(bytes.checked_add(extra).ok_or_else(||ApiError::bad_request("Request token bound overflow"))?)
        .ok().and_then(|n|n.checked_add(256)).ok_or_else(||ApiError::bad_request("Request token bound overflow"))?;
    Ok((input,output_bound(body,endpoint)?))
}

#[cfg(test)]
mod rerank_admission_tests {
    use super::*;
    #[test]
    fn rerank_admission_counts_query_for_every_document() {
        let body = serde_json::json!({"query":"q".repeat(512),"documents":vec!["d";128]});
        let (input, output) = admission_bound(&body, EndpointKind::Rerank).unwrap();
        assert_eq!(input, body.to_string().len() as i64 + 512 * 127 + 256);
        assert_eq!(output, 0);
    }
}
fn has_input_audio(v: &Value) -> bool {
    match v {
        Value::Array(items) => items.iter().any(has_input_audio),
        Value::Object(map) => {
            map.get("type").and_then(Value::as_str) == Some("input_audio")
                || ["content", "input", "output", "messages"]
                    .iter()
                    .filter_map(|k| map.get(*k))
                    .any(has_input_audio)
        }
        _ => false,
    }
}
fn validate_input_audio(v: &Value) -> Result<(), ApiError> {
    match v {
        Value::Array(items) => {
            for item in items {
                validate_input_audio(item)?;
            }
        }
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("input_audio") {
                let audio = map
                    .get("input_audio")
                    .and_then(Value::as_object)
                    .ok_or_else(|| ApiError::bad_request("input_audio requires data and format"))?;
                if audio.keys().any(|k| k != "data" && k != "format")
                    || !matches!(
                        audio.get("format").and_then(Value::as_str),
                        Some("wav" | "mp3")
                    )
                {
                    return Err(ApiError::bad_request(
                        "Native input_audio supports inline wav/mp3 only",
                    ));
                }
                use base64::Engine;
                let data = audio
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ApiError::bad_request("input_audio data must be base64"))?;
                if data.is_empty()
                    || base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .is_err()
                {
                    return Err(ApiError::bad_request(
                        "input_audio data must be nonempty standard base64",
                    ));
                }
            }
            for child in ["content", "input", "output", "messages"]
                .iter()
                .filter_map(|k| map.get(*k))
            {
                validate_input_audio(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
fn billing_bound(
    body: &mut Value,
    endpoint: EndpointKind,
    price: Option<&crate::db::billing::Price>,
) -> Result<(i64, i64), ApiError> {
    billing_bound_with_context(body, endpoint, price, None)
}

fn billing_bound_with_context(body:&mut Value,endpoint:EndpointKind,price:Option<&crate::db::billing::Price>,context:Option<(i64,i64)>)->Result<(i64,i64),ApiError> {
    let (mut input,mut output)=admission_bound(body,endpoint)?;
    if has_input_audio(body) || endpoint.protocol().is_some_and(|p|crate::protocol_sessions::opaque_history(p,body)) {
        input=context.map(|v|v.0).or(price.map(|p|p.max_input_tokens)).filter(|n|*n>0)
            .ok_or_else(||ApiError::new(StatusCode::FORBIDDEN,"pricing_required","Opaque history or audio requires a configured maximum input context bound"))?;
    }
    if endpoint.has_image(body) {if let Some((max_input,_))=context {input=max_input;}}
    if price.is_none() && endpoint.is_generation() && body.get("max_tokens").is_none() && body.get("max_completion_tokens").is_none() && body.get("max_output_tokens").is_none() {
        let choices=if endpoint==EndpointKind::Chat {choices_count(body)?}else{1};
        let per_choice=4096.min(context.map(|v|v.1/choices).unwrap_or(4096));
        if per_choice<=0 {return Err(ApiError::bad_request("Source configuration needs a positive output cap"));}
        output=per_choice.checked_mul(choices).ok_or_else(||ApiError::bad_request("Total output token bound overflow"))?;
        body[if endpoint==EndpointKind::Responses{"max_output_tokens"}else{"max_tokens"}]=Value::from(per_choice);
    }
    if let Some(p)=price {
        if endpoint.is_generation() && body.get("max_tokens").is_none() && body.get("max_completion_tokens").is_none() && body.get("max_output_tokens").is_none() {
            let choices=if endpoint==EndpointKind::Chat {choices_count(body)?} else {1};
            let per_choice=4096.min(p.max_output_tokens.min(context.map(|v|v.1).unwrap_or(i64::MAX))/choices);
            if per_choice<=0 {return Err(ApiError::bad_request("Pricing configuration needs a positive output cap"));}
            output=per_choice.checked_mul(choices).ok_or_else(||ApiError::bad_request("Total output token bound overflow"))?;
            body[if endpoint==EndpointKind::Responses {"max_output_tokens"} else {"max_tokens"}]=Value::from(per_choice);
        }
        if endpoint.has_image(body) {input=context.map(|v|v.0).unwrap_or(p.max_input_tokens);}
        if input>p.max_input_tokens || output>p.max_output_tokens {
            return Err(ApiError::new(StatusCode::BAD_REQUEST,"billing_token_limit","Request exceeds configured billing token bounds"));
        }
    }
    if let Some((max_input,max_output))=context {
        if input>max_input || output>max_output {return Err(ApiError::new(StatusCode::BAD_REQUEST,"source_token_limit","Request exceeds configured source token bounds"));}
    }
    Ok((input,output))
}

/// Subscription preview cannot enforce an output cap. Reserve the full verified
/// provider output ceiling and keep unknown monetary prices unknown.
fn subscription_bound(body:&Value,price:Option<&crate::db::billing::Price>,context:Option<(i64,i64)>)->Result<(i64,i64),ApiError> {
    let (max_input,max_output)=context.filter(|(i,o)|*i>0&&*o>0).ok_or_else(||ApiError::new(StatusCode::FORBIDDEN,"source_limits_required","ChatGPT plan requests require verified full provider context and output bounds"))?;
    let (mut input,_)=admission_bound(body,EndpointKind::Responses)?;
    if crate::protocol_sessions::opaque_history(crate::protocol::Protocol::Responses,body) || crate::protocol::has_image(crate::protocol::Protocol::Responses,body) {input=max_input;}
    if input>max_input || price.is_some_and(|p|input>p.max_input_tokens||max_output>p.max_output_tokens) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST,"source_token_limit","ChatGPT plan request exceeds configured source or billing token bounds"));
    }
    Ok((input,max_output))
}
fn validate_native_cost_and_state(endpoint:EndpointKind,body:&Value)->Result<(),ApiError> {
    fn unsupported_media(v:&Value,allow_audio:bool)->bool {
        match v {
            Value::Array(items)=>items.iter().any(|item|unsupported_media(item,allow_audio)),
            Value::Object(map)=>["audio","file_id","file_data"].iter().any(|key|map.get(*key).is_some_and(|v|!v.is_null())) || matches!(map.get("type").and_then(Value::as_str),Some("file"|"input_file"|"document"|"audio"|"video"|"input_video")) || (!allow_audio && map.get("type").and_then(Value::as_str)==Some("input_audio")) || ["content","input","output","messages","source","file"].iter().filter_map(|k|map.get(*k)).any(|child|unsupported_media(child,allow_audio)),
            _=>false,
        }
    }
    if has_input_audio(body) {validate_input_audio(body)?;}
    if body.get("audio").is_some_and(|v|!v.is_null()) || body.get("modalities").and_then(Value::as_array).is_some_and(|m|m.iter().any(|v|v!="text")){return Err(ApiError::bad_request("Only text output is enabled; media output requires separate capabilities and metering"));}
    if unsupported_media(body,endpoint==EndpointKind::Chat) {return Err(ApiError::bad_request("File, document, audio and video blocks require dedicated capability and token accounting; this route currently supports text, images and client tools"));}
    if body.get("container").is_some_and(|v|!v.is_null()) || body.get("web_search_options").is_some_and(|v|!v.is_null()) {
        return Err(ApiError::bad_request("Provider containers and hosted tools require separate metering and are not enabled"));
    }
    if endpoint==EndpointKind::Responses && body.get("input").and_then(Value::as_array).is_some_and(|items|items.iter().any(|item|item["type"]=="item_reference" || (item.get("id").is_some() && item.get("type").is_none() && item.get("content").is_none()))) {
        return Err(ApiError::bad_request("Provider item references are not enabled; supply complete input history"));
    }
    fn client_tool(endpoint:EndpointKind,tool:&Value,depth:usize)->bool {
        if depth>8 {return false;}
        let kind=tool.get("type").and_then(Value::as_str);
        if endpoint==EndpointKind::Responses && kind==Some("namespace") {
            return tool.get("tools").and_then(Value::as_array).is_some_and(|tools|!tools.is_empty() && tools.len()<=128 && tools.iter().all(|t|client_tool(endpoint,t,depth+1)));
        }
        match endpoint {
            EndpointKind::Messages=>kind.is_none_or(|t|t=="custom"||t.starts_with("bash_")||t.starts_with("text_editor_")||t.starts_with("computer_")),
            _=>matches!(kind,Some("function"|"custom"|"local_shell")),
        }
    }
    if let Some(tools)=body.get("tools").and_then(Value::as_array) {
        if tools.len()>128 || tools.iter().any(|t|!client_tool(endpoint,t,0)) {
            return Err(ApiError::bad_request("Hosted or unknown tool types require separate metering; use client function tools and disable hosted web search"));
        }
    }
    if endpoint==EndpointKind::Responses {
        if let Some(input)=body.get("input").and_then(Value::as_array) {
            for item in input {
                if matches!(item["type"].as_str(),Some("tool_search_call"|"tool_search_output")) {return Err(ApiError::bad_request("Provider tool search requires separate metering and is not enabled"));}
                if item["type"]=="additional_tools" {
                    let tools=item.get("tools").and_then(Value::as_array).ok_or_else(||ApiError::bad_request("Expected additional_tools definitions"))?;
                    if tools.len()>128 || tools.iter().any(|tool|!client_tool(endpoint,tool,0)) {return Err(ApiError::bad_request("Hosted or unknown additional_tools require separate metering"));}
                }
            }
        }
    }
    Ok(())
}

fn billing_error(e:crate::db::DbError)->ApiError {
    match e {
        crate::db::DbError::Other(message) if message.contains("config_required") => ApiError::new(StatusCode::FORBIDDEN,"pricing_required",message),
        crate::db::DbError::Other(message) if message.contains("budget") => ApiError::new(StatusCode::TOO_MANY_REQUESTS,"budget_exceeded",message),
        e=>ApiError::from(e),
    }
}

/// 从库里把这次请求要用的候选链一次捞齐（同步、短暂持锁；返回后就不许再碰库守卫）。
///
/// 两种情形：
/// - **普通模型名**：勾了它的渠道，按渠道顺序（面板「渠道」页从上到下 = 先试）。
/// - **保留名 `auto`**：现场挑一串最优的模型名（见 `db::model_routes::auto_candidates`），
///   逐个摊成链 —— 每条候选都带着「这一发要发给上游的名字」。
fn load_plan(
    state: &SharedState,
    model: &str,
    endpoint: EndpointKind,
    wants_vision: bool,
    auth: Option<&crate::db::identity::AuthContext>,
) -> Result<Plan, ApiError> {
    let conn = state.conn();
    if model_routes::is_auto(model) {
        return load_auto_plan(state, &conn, model, endpoint, wants_vision, auth);
    }
    if let Some(auth) = auth {
        let shared = crate::db::shared_virtual_routes::resolve(&conn, auth, model, now_ms())
            .map_err(virtual_route_error)?;
        let resolved = match shared {
            Some(route) => Some(route),
            None => crate::db::user_virtual_routes::resolve(&conn, auth.user_id, model, now_ms()).map_err(virtual_route_error)?,
        };
        if let Some(resolved) = resolved {
            if matches!(endpoint, EndpointKind::Embeddings | EndpointKind::Rerank)
                && resolved.targets.len() > 1 && resolved.compatibility.is_none() {
                return Err(ApiError::new(StatusCode::BAD_REQUEST,"vector_fallback_unverified","向量或重排多目标回退尚未核实，请先确认模型用途并重新保存调用策略"));
            }
            let mut plan = Plan {
                route: resolved.name.clone(),
                virtual_route: Some(resolved.clone()),
                auto: false,
                entries: Vec::new(),
                secrets: HashMap::new(),
                caps: HashMap::new(),
                explored: None,
                load_ticket: None,
            };
            for target in &resolved.targets {
                let Some(route) = model_routes::routes_for(&conn, &target.logical_model)? else {
                    continue;
                };
                if !route.enabled {
                    continue;
                }
                let kind_ok = route.kind == endpoint.as_model_kind()
                    || (route.kind == tokengo_shared::ModelKind::Unknown && (!resolved.shared || endpoint.is_generation()));
                if !kind_ok {
                    continue;
                }
                for channel in model_routes::channels_for(&conn, &route.name)? {
                    let provider =
                        crate::db::provider_pool::source_credential(&conn, channel.id.0)?;
                    if !channel.routable()
                        || provider.as_ref().is_none_or(|credential| {
                            credential.provider_id != target.provider_id || !credential.enabled
                        })
                    {
                        continue;
                    }
                    if !permitted_plan_candidate(&conn, auth, Some(&resolved), &route.name, channel.id.0, endpoint)? {
                        continue;
                    }
                    let before = plan.entries.len();
                    push_entry(state, &conn, &mut plan, route.name.clone(), channel)?;
                    if plan.entries.len() > before {
                        plan.entries.last_mut().unwrap().target = Some(target.clone());
                    }
                }
            }
            if plan.entries.is_empty() {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "source_not_allowed",
                    "No authorized and enabled virtual-route targets",
                ));
            }
            return Ok(plan);
        }
    }
    if crate::db::model_access::internal_only(&conn, model)? {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "model_internal_only", "此模型仅允许管理员共享虚拟入口内部调用"));
    }
    let Some(route) = model_routes::routes_for(&conn, model)? else {
        // 报错别教训人，说清「去哪儿勾上」就够了。
        return Err(ApiError::not_found(format!(
            "模型「{model}」这里没有：去面板「渠道」页把这个模型名勾上 —— 勾上才认（勾选就是白名单）"
        )));
    };
    if !route.enabled {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "model_disabled",
            format!("模型「{model}」已停用"),
        ));
    }
    let mut plan = Plan {
        route: route.name.clone(),
        virtual_route: None,
        auto: false,
        entries: Vec::new(),
        secrets: HashMap::new(),
        caps: HashMap::new(),
        explored: None,
        load_ticket: None,
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
            if let Some(auth) = auth {
                if !permitted_candidate_for(&conn, auth, &route.name, id.0, endpoint)? {
                    continue;
                }
            }
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
    wants_vision: bool,
    auth: Option<&crate::db::identity::AuthContext>,
) -> Result<Plan, ApiError> {
    let mut plan = Plan {
        route: model_routes::AUTO_NAME.to_string(),
        virtual_route: None,
        auto: true,
        entries: Vec::new(),
        secrets: HashMap::new(),
        caps: HashMap::new(),
        explored: None,
        load_ticket: None,
    };
    let mut candidates =
        model_routes::auto_candidates(conn, endpoint.as_model_kind(), wants_vision, now_ms())?;
    let had_candidates = !candidates.is_empty();
    if let Some(auth) = auth {
        candidates.retain(|name| permitted_model(&conn, auth, name).unwrap_or(false));
    }
    if had_candidates && candidates.is_empty() && auth.is_some() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "model_not_allowed",
            "No authorized auto candidates",
        ));
    }
    if candidates.is_empty()
        && wants_vision
        && !model_routes::auto_candidates(conn, endpoint.as_model_kind(), false, now_ms())?
            .is_empty()
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_vision_model",
            "这条请求带了图片，但没有符合视觉能力的候选模型",
        ));
    }
    if candidates.is_empty() {
        if !crate::db::faults::rows(conn)?.is_empty() {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "models_cooling",
                "暂无可用候选，存在账号或权限暂停，请在总览“调用异常”查看",
            ));
        }
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
    let sequence = state
        .auto_sequence
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let config = crate::db::policy::get(conn)?;
    use rand::Rng;
    let draw = rand::thread_rng().gen_range(0..10000u64);
    // Exploration must never select a model whose every matching channel is cooling.
    candidates.retain(|name| {
        model_routes::channels_for(conn, name)
            .unwrap_or_default()
            .iter()
            .any(|c| {
                crate::db::policy::peek(conn, c.id.0, name, c.upstream_model(name), now_ms())
                    .ok()
                    .flatten()
                    .is_none()
            })
    });
    plan.explored = crate::db::policy::rank(conn, &mut candidates, &config, sequence, draw)?;
    if config.strategy == "idle" {
        if let Some(t) = state
            .model_loads
            .select(&mut candidates, plan.explored.is_some())
        {
            plan.load_ticket = Some((candidates[0].clone(), t));
        }
    }
    for name in candidates {
        for channel in model_routes::channels_for(conn, &name)? {
            if !channel.routable() || (wants_vision && !channel.vision) {
                continue;
            }
            push_entry(state, conn, &mut plan, name.clone(), channel)?;
        }
    }
    if plan.entries.is_empty() {
        let until = model_routes::list(conn)?
            .iter()
            .flat_map(|r| crate::db::policy::rows(conn, &r.name).unwrap_or_default())
            .map(|r| r.until_ms.max(r.lease_until))
            .filter(|t| *t > now_ms())
            .min();
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "models_cooling",
            format!(
                "暂无可用模型，候选冷却或账号暂停；最早可重试时间：{}",
                until
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "稍后".into())
            ),
        ));
    }
    Ok(plan)
}

/// 转发。成功/失败都返回一个**能直接回给客户端**的响应。
pub async fn forward(state: SharedState, req: ForwardRequest) -> Result<Response, ApiError> {
    let started_ms = now_ms();
    if let Some(kind) = req.endpoint.tool_kind() {
        if req.stream || model_routes::is_auto(&req.model) {
            return Err(ApiError::bad_request(
                "Search and Fetch require a non-streaming explicit route",
            ));
        }
        crate::search_tools::normalize_request(kind, &req.body).map_err(ApiError::bad_request)?;
    }
    // 路由不出去（模型名没配置 / 组停用了 / 组里没有可用渠道）时，**也要留一条账**。
    //
    // 为什么：面板「请求」页是用户排障的第一站（他就是从那里问出「为什么客户端连不上」的）。
    // 以前这三类 404 在 `load_plan` 里就直接 return 了，一条账都不记 —— 用户看到的是
    // 「客户端报 404、面板上干干净净」，只能靠猜。这与带图被拦（422）的处理也不一致：
    // 那条是记的。**记账绝不能反过来影响请求结果**：这里只记，记账本身错了就吞掉。
    if req.endpoint.is_generation() {
        validate_native_cost_and_state(req.endpoint, &req.body)?;
    }
    if req.endpoint == EndpointKind::Responses
        && ["previous_response_id", "conversation"]
            .iter()
            .any(|f| req.body.get(*f).is_some_and(|v| !v.is_null()))
    {
        return Err(ApiError::bad_request("Server-side continuation is not enabled; supply the complete input history with store=false"));
    }
    let mut plan_all = match load_plan(
        &state,
        &req.model,
        req.endpoint,
        req.endpoint.has_image(&req.body),
        req.auth.as_ref(),
    ) {
        Ok(p) => p,
        Err(e) => {
            if e.status.is_client_error() || e.status == StatusCode::SERVICE_UNAVAILABLE {
                if matches!(req.endpoint, EndpointKind::Embeddings | EndpointKind::Rerank) {
                    record_api_routing_refusal(&state, &req, started_ms, &e);
                } else {
                    record_routing_refusal(&state, &req, started_ms, e.status.as_u16(), &e.message);
                }
            }
            return Err(e);
        }
    };
    if let Some(contract) = plan_all.vector_contract() {
        if let Err(error) = crate::vector_compat::validate_request(
            contract, req.endpoint.as_model_kind(), &req.body,
        ) {
            let error = virtual_route_error(error);
            record_api_routing_refusal(&state, &req, started_ms, &error);
            return Err(error);
        }
    }
    if let Some(auth) = &req.auth {
        if (plan_all.virtual_route.is_none() || plan_all.virtual_route.as_ref().is_some_and(|r| r.shared)) && !auth.permits_model(&req.model) {
            record_routing_refusal(&state, &req, started_ms, 403, "Model not permitted");
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "model_not_allowed",
                "Model not permitted",
            ));
        }
        let conn = state.conn();
        let mut authorized = Vec::new();
        for entry in plan_all.entries.drain(..) {
            if permitted_plan_candidate(&conn, auth, plan_all.virtual_route.as_ref(), &entry.name, entry.channel.id.0, req.endpoint)?
            {
                authorized.push(entry);
            }
        }
        plan_all.entries = authorized;
        if plan_all.entries.is_empty() {
            drop(conn);
            record_routing_refusal(&state, &req, started_ms, 403, "No authorized sources");
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "source_not_allowed",
                "No authorized sources",
            ));
        }
    }
    // 这一链的全量候选（状态机与「记账记的是谁」都看这一份）
    let all_ids = plan_all.ids();

    // 这条请求是不是在让模型看图？（只有对话端点会有图）
    let wants_vision = req.endpoint.has_image(&req.body);

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
        record_client_app(&state, request_id, req.user_agent.as_deref());
        guard.identify(req.auth.as_ref())?;
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

    if let Some(auth) = &req.auth {
        order_pool_entries(&state, &mut plan, auth, req.endpoint)?;
    }
    let pooled = req.auth.is_some() && plan.entries.iter().any(|entry| entry.credential.is_some());
    // 顺序：
    // · 普通模型名 → 问状态机（首选 / 回切 / 上一条走的谁），带图时再按能看图筛一遍；
    // · `auto` → 现场算出来的那条链就是顺序（不读也不写状态机）。
    let mut order: Vec<usize> = if plan.auto || plan.virtual_route.is_some() {
        (0..plan.entries.len())
            .filter(|i| !wants_vision || plan.entries[*i].channel.vision)
            .collect()
    } else {
        let mut ord = state
            .routing()
            .attempt_order(&plan.route, &all_ids, started_ms);
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
    if pooled && !plan.auto && plan.virtual_route.is_none() {
        // Keep the global source/provider slots; only key preference within the same
        // logical-model/provider group belongs to this authenticated user's affinity.
        let mut slots: HashMap<PoolGroup, Vec<usize>> = HashMap::new();
        for (position, index) in order.iter().copied().enumerate() {
            let entry = &plan.entries[index];
            if let Some(credential) = &entry.credential {
                slots
                    .entry(pool_group(entry.target.as_ref(), credential, &entry.name))
                    .or_default()
                    .push(position);
            }
        }
        for positions in slots.into_values() {
            let mut indices: Vec<usize> =
                positions.iter().map(|position| order[*position]).collect();
            indices.sort_unstable();
            for (position, index) in positions.into_iter().zip(indices) {
                order[position] = index;
            }
        }
    }
    // 带图请求与 `auto` **只读状态机、不写**：
    // · 带图请求本来就只在「能看图」这个小圈子里挑渠道，让它的结果去改整个模型名的口味，
    //   等于一条带图请求把后面十分钟的纯文字请求一起改道。
    // · `auto` 每一发都是现场挑的，没有「首选/回切」可言；让它去写状态，
    //   那个名字下的账就没人解释得清了。
    // 渠道自身的健康照旧由每分钟探活和普通请求看着。
    let touch_state = !wants_vision && !plan.auto && plan.virtual_route.is_none();

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
    record_client_app(&state, request_id, req.user_agent.as_deref());
    guard.identify(req.auth.as_ref())?;

    let mut admitted_input = None;
    if let Some(auth) = &req.auth {
        let (mut input, mut output) =
            admission_bound(&req.body, req.endpoint).map_err(|error| {
                guard.reject(&error);
                error
            })?;
        // Predict the per-source default without borrowing mutable price caps.
        // OAuth keeps its full binding ceiling; a missing OAuth bound never uses generic caps.
        let default_output = req.endpoint.is_generation()
            && ["max_tokens", "max_completion_tokens", "max_output_tokens"]
                .iter()
                .all(|key| req.body.get(*key).is_none());
        let choices = if req.endpoint == EndpointKind::Chat {
            choices_count(&req.body)?
        } else {
            1
        };
        let mut planned_output = None;
        for candidate in &plan.entries {
            let c = state.conn();
            let oauth_bound = crate::oauth::context_bounds(&c, candidate.channel.id.0)
                .map_err(|_| ApiError::internal("OAuth source context unavailable"))?;
            let has_oauth: bool = c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings WHERE channel_id=?1)",
                    [candidate.channel.id.0],
                    |r| r.get(0),
                )
                .map_err(crate::db::DbError::from)?;
            let candidate_output = if !req.endpoint.is_generation() {
                0
            } else if has_oauth {
                oauth_bound.map(|v| v.1).unwrap_or(output)
            } else if default_output {
                4096.min(
                    candidate
                        .protocols
                        .context_bounds()
                        .map(|v| v.1 / choices)
                        .unwrap_or(4096),
                )
                .checked_mul(choices)
                .ok_or_else(|| ApiError::bad_request("Total output token bound overflow"))?
            } else {
                output
            };
            planned_output = Some(planned_output.unwrap_or(0).max(candidate_output));
        }
        if let Some(planned) = planned_output {
            output = planned;
        }
        let opaque_input = req
            .endpoint
            .protocol()
            .is_some_and(|p| crate::protocol_sessions::opaque_history(p, &req.body));
        let audio_input = has_input_audio(&req.body);
        if audio_input
            && !plan.entries.iter().any(|entry| {
                entry.protocols.native_input_audio
                    && entry
                        .protocols
                        .native
                        .contains(&crate::protocol::Protocol::Chat)
            })
        {
            let error = ApiError::bad_request("No source declares native Chat input_audio");
            guard.reject(&error);
            return Err(error);
        }
        if opaque_input || audio_input || req.endpoint.has_image(&req.body) {
            let c = state.conn();
            let mut maximum = None;
            for candidate in &plan.entries {
                if audio_input && !candidate.protocols.native_input_audio {
                    continue;
                }
                let oauth_bound = crate::oauth::context_bounds(&c, candidate.channel.id.0)
                    .map_err(|_| ApiError::internal("OAuth source context unavailable"))?;
                let has_oauth: bool = c
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings WHERE channel_id=?1)",
                        [candidate.channel.id.0],
                        |r| r.get(0),
                    )
                    .map_err(crate::db::DbError::from)?;
                let context = if has_oauth {
                    oauth_bound
                } else {
                    candidate.protocols.context_bounds()
                };
                let bound = context.map(|v| v.0).or(crate::db::billing::price(
                    &c,
                    &candidate.name,
                    candidate.channel.id.0,
                )?
                .map(|p| p.max_input_tokens));
                if let Some(bound) = bound.filter(|n| *n > 0) {
                    maximum = Some(maximum.unwrap_or(0).max(bound));
                }
            }
            let tpm_limited = auth.effective_policy().tpm.is_some()
                || auth.ancestor_key_ids.iter().any(|id| {
                    crate::db::identity::context(&c, *id)
                        .ok()
                        .flatten()
                        .is_some_and(|p| p.effective_policy().tpm.is_some())
                });
            if opaque_input || audio_input || tpm_limited {
                input=maximum.ok_or_else(||ApiError::new(StatusCode::FORBIDDEN,"pricing_required","Opaque/audio input or image input with token limits requires configured context bounds")).map_err(|error|{guard.reject(&error);error})?;
            } else if let Some(maximum) = maximum {
                input = maximum;
            }
        }
        guard.admission = Some(
            crate::admission::acquire(state.clone(), auth, input as u64, output as u64).map_err(
                |e| {
                    guard.reject(&e);
                    e
                },
            )?,
        );
        admitted_input = Some(input);
    }
    let global_cfg = state.routing().cfg.clone();
    let cfg = crate::db::route_policy::effective(&state.conn(), &plan.route, global_cfg)?;
    let attempt_ms = if req.stream {
        cfg.attempt_timeout_ms
    } else {
        cfg.nonstream_timeout_ms
    }
    .max(1);
    let attempt_timeout = Duration::from_millis(attempt_ms);

    // 同一段对话稳定不变的会话 ID（自定义头 `{{session_id}}` 用）。
    // **按客户端原样的请求体算一次**：改成上游的模型名之后再算，同一个客户端模型名
    // 落到不同渠道会得出不同的会话 ID，上游看到的就不再是"同一段对话"了。
    let explicit = req.explicit_session.as_deref().or_else(|| {
        ["session_id", "conversation_id"].iter().find_map(|field| {
            req.body
                .get(field)
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
        })
    });
    let has_user = req
        .body
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages
                .iter()
                .any(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        });
    let needs_session = plan.entries.iter().any(|candidate| {
        candidate
            .channel
            .headers
            .iter()
            .any(|(_, value)| value.contains("{{session_id}}"))
    });
    if req.endpoint == EndpointKind::Chat && explicit.is_none() && !has_user && needs_session {
        let note = "该渠道要求稳定会话 ID；没有原始 user 消息的请求必须提供显式会话 ID";
        guard.finish(
            &state,
            Status::ClientError,
            Some(400),
            None,
            started_ms,
            None,
            0,
            0,
            req.stream,
            Some(note),
        );
        return Err(ApiError::bad_request(note));
    }
    let session = explicit
        .map(|id| tokengo_shared::short_hash(&format!("explicit:{id}")))
        .or_else(|| tokengo_shared::session_id_from_body(&req.body))
        .map(|id| tokengo_shared::short_hash(&format!("{}:{id}", req.session_namespace)));

    let opaque = req
        .endpoint
        .protocol()
        .is_some_and(|p| crate::protocol_sessions::opaque_history(p, &req.body));
    let pin_key = req.explicit_session.as_deref().and_then(|id| {
        req.endpoint
            .protocol()
            .filter(|p| *p != crate::protocol::Protocol::Chat)
            .map(|p| crate::protocol_sessions::key(&req.session_namespace, id, &req.model, p))
    });
    let existing_pin = pin_key
        .as_deref()
        .map(|key| crate::protocol_sessions::load(&state.conn(), key))
        .transpose()?
        .flatten();
    let order: Vec<usize> = order
        .into_iter()
        .filter(|index| {
            existing_pin
                .as_ref()
                .is_none_or(|pin| plan.entries[*index].channel.id.0 == pin.channel)
        })
        .collect();
    if existing_pin.is_some() && order.is_empty() {
        let error = ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "native_source_unavailable",
            "Native session source is unavailable; retry later or start a new session",
        );
        guard.reject(&error);
        return Err(error);
    }
    if opaque && existing_pin.is_none() {
        let error=ApiError::bad_request("Opaque reasoning history requires an existing explicit session bound to its original source");
        guard.reject(&error);
        return Err(error);
    }
    let mut ordinal: u32 = 0;
    let mut last_fault: Option<String> = None;
    let mut last_http: Option<UpstreamError> = None;
    let mut last_capacity_http: Option<UpstreamError> = None;
    let mut last_channel = ChannelId(0);
    // 上游说的「过多久再来」；下一次重试用它顶掉默认节拍。

    // 每条渠道的「门口闸机」：同时最多几发（面板上按渠道配，0 = 不限）。
    // 满了**不硬压上去**（现网 503「bridge busy」151 次就是这么来的）：先换别的渠道；
    // 所有渠道都满，才在门口排队等一个名额，等到再重跑一遍。排队有预算上限，不会挂死。
    let mut last_policy_error: Option<ApiError> = None;
    let mut pre_permits: HashMap<i64, crate::state::ChannelPermit> = HashMap::new();
    let mut pool_control = PoolControl::default();
    let mut active_target = plan
        .entries
        .first()
        .and_then(|entry| entry.target.as_ref())
        .map(|target| target.position);
    'pass: loop {
        let mut tried_any = false;
        let mut busy: Vec<(i64, Arc<crate::state::ChannelGate>)> = Vec::new();

        'channels: for entry_idx in order.iter().copied() {
            let Some(Candidate {
                name,
                channel,
                revision,
                protocols,
                binding_generation,
                credential,
                target,
            }) = plan.entries.get(entry_idx)
            else {
                continue;
            };
            // Verified vector chains may walk the remaining ordered candidates
            // on transport, source-health and malformed-output faults. Complete
            // 400/429 responses still stop below when their switches are off.
            if !plan.automatic_target_failover() && target
                .as_ref()
                .is_some_and(|target| Some(target.position) != active_target)
            {
                continue;
            }
            let tool_caps = if let Some(kind) = req.endpoint.tool_kind() {
                let caps = crate::search_tools::load(&state.conn(), channel.id.0)?;
                if !caps.permits(kind) {
                    last_policy_error = Some(ApiError::bad_request(
                        "Source does not explicitly declare this standalone tool capability",
                    ));
                    continue;
                }
                Some(caps)
            } else {
                None
            };

            if has_input_audio(&req.body)
                && (req.endpoint != EndpointKind::Chat
                    || !protocols.native_input_audio
                    || !protocols.native.contains(&crate::protocol::Protocol::Chat))
            {
                last_policy_error = Some(ApiError::bad_request(
                    "Source does not declare native Chat input_audio",
                ));
                continue;
            }
            let upstream_protocol = match req.endpoint.protocol() {
                Some(client) => match protocols.select(client) {
                    Some(p) => Some(p),
                    None => {
                        last_policy_error = Some(ApiError::bad_request(
                            "Source does not declare this protocol or allow conversion",
                        ));
                        continue;
                    }
                },
                None => None,
            };
            let candidate_pin = upstream_protocol.map(|protocol| crate::protocol_sessions::Pin {
                channel: channel.id.0,
                generation: *binding_generation,
                upstream_model: channel.upstream_model(name).into(),
                protocol,
            });
            if existing_pin
                .as_ref()
                .is_some_and(|pin| candidate_pin.as_ref() != Some(pin))
            {
                last_policy_error=Some(ApiError::bad_request("Native session is pinned to a different or changed source; start a new session"));
                continue;
            }
            if pooled {
                if let Some(credential) = credential {
                    let group = pool_group(target.as_ref(), credential, name);
                    let selected = pool_control.selected.entry(group).or_insert(credential.id);
                    if *selected != credential.id {
                        continue;
                    }
                    if pool_control.seen.contains(&(
                        target.as_ref().map(|target| target.position),
                        credential.id,
                        name.clone(),
                    )) {
                        continue;
                    }
                }
            }
            if opaque {
                if let (Some(owner), Some(pin)) = (&pin_key, &candidate_pin) {
                    if !crate::protocol_sessions::verify_artifacts(
                        &state.conn(),
                        owner,
                        pin,
                        &req.body,
                    )? {
                        let error=ApiError::bad_request("Opaque history contains artifacts not issued to this key, session and source");
                        guard.reject(&error);
                        return Err(error);
                    }
                }
            }
            let upstream_path = upstream_protocol
                .map(|p| p.path())
                .unwrap_or(req.endpoint.path());
            let prepared = if let Some(kind) = req.endpoint.tool_kind() {
                crate::search_tools::normalize_request(kind, &req.body)
                    .map_err(ApiError::bad_request)?
            } else {
                match (req.endpoint.protocol(), upstream_protocol) {
                    (Some(from), Some(to)) => {
                        match crate::protocol::convert_request(from, to, &req.body) {
                            Ok(body) => body,
                            Err(note) => {
                                last_policy_error = Some(ApiError::bad_request(note));
                                continue;
                            }
                        }
                    }
                    _ => req.body.clone(),
                }
            };
            // A source returning reasoning does not imply it accepts that dialect
            // in assistant history. Only converted Chat replay needs explicit opt-in.
            if req
                .endpoint
                .protocol()
                .is_some_and(|client| client != crate::protocol::Protocol::Chat)
                && upstream_protocol == Some(crate::protocol::Protocol::Chat)
                && !protocols.chat_reasoning_replay
                && prepared
                    .get("messages")
                    .and_then(Value::as_array)
                    .is_some_and(|messages| {
                        messages.iter().any(|message| {
                            message
                                .get("reasoning_content")
                                .is_some_and(|value| !value.is_null())
                        })
                    })
            {
                last_policy_error = Some(ApiError::bad_request(
                    "Source does not declare Chat reasoning history replay support",
                ));
                continue;
            }
            // 门口闸机：拿到名额才发；这条满了就先跳过（换下一条）
            let permit = match pre_permits.remove(&channel.id.0) {
                Some(p) => Some(p),
                None => match state.channel_gate(channel.id.0, plan.cap(channel.id)) {
                    None => None,
                    Some(sem) => match sem.clone().try_acquire_owned() {
                        Ok(p) => Some(p),
                        Err(_) => {
                            if pooled && credential.is_some() {
                                let left = cfg
                                    .request_budget_ms
                                    .saturating_sub((now_ms() - started_ms).max(0) as u64);
                                match tokio::time::timeout(
                                    Duration::from_millis(left),
                                    sem.acquire_owned(),
                                )
                                .await
                                {
                                    Ok(Ok(permit)) => Some(permit),
                                    _ => {
                                        let error=ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"credential_busy","Selected upstream source remained busy until request deadline");
                                        guard.reject(&error);
                                        return Err(error);
                                    }
                                }
                            } else {
                                busy.push((channel.id.0, sem));
                                continue;
                            }
                        }
                    },
                },
            };
            let credential_permit = if pooled {
                if let Some(credential) = credential {
                    match state.credential_gate(credential.id, credential.max_concurrent) {
                        None => None,
                        Some(gate) => {
                            let left = cfg
                                .request_budget_ms
                                .saturating_sub((now_ms() - started_ms).max(0) as u64);
                            match tokio::time::timeout(
                                Duration::from_millis(left),
                                gate.acquire_owned(),
                            )
                            .await
                            {
                                Ok(Ok(permit)) => Some(permit),
                                _ => {
                                    let error=ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"credential_busy","Selected upstream credential remained busy until request deadline");
                                    guard.reject(&error);
                                    return Err(error);
                                }
                            }
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };
            let permit = (permit, credential_permit);
            let account = {
                let c = state.conn();
                crate::db::faults::claim(&c, channel.id.0, name, *revision, now_ms())?
            };
            let Some(account) = account else {
                continue;
            };
            let sent = channel.upstream_model(name);
            let generation = {
                let c = state.conn();
                crate::db::policy::claim(&c, channel.id.0, name, sent, now_ms())?
            };
            let Some(generation) = generation else {
                let c = state.conn();
                crate::db::faults::release(&c, &account)?;
                continue;
            };
            let load_ticket = match plan.load_ticket.take() {
                Some((n, t)) if tokengo_shared::same_model_name(&n, name) => t,
                other => {
                    drop(other);
                    state.model_loads.acquire(name)
                }
            };
            // 冷到期的恢复试用只放一条：失败就进下一级冷却，不再连试三次。
            let recovering = {
                let c = state.conn();
                crate::db::policy::rows(&c, name)?.iter().any(|r| {
                    r.channel_id == channel.id.0 && r.upstream == sent && r.lease_until > 0
                })
            };
            // Auto chooses a logical candidate, not an exemption from its retry policy.
            let retries = if plan.auto {
                crate::db::route_policy::load(&state.conn(), name)?
                    .retry_times
                    .unwrap_or(cfg.retry_times)
            } else {
                cfg.retry_times
            };
            let max_attempts = if plan.virtual_route.is_some() || recovering || account.recovering()
            {
                1
            } else {
                (1 + retries).min(COOLDOWN_AFTER_FAILURES as u32).max(1)
            };
            let mut cooldown = CooldownTicket {
                state: state.clone(),
                id: channel.id.0,
                name: name.clone(),
                upstream: sent.to_string(),
                generation,
                done: false,
                account,
            };

            // 这一发送给上游的名字：按**这个候选名**在这条渠道上查别名
            // （auto 时每个候选各不相同 —— auto 挑中的是谁，上游看到的就是谁的名字）
            let sent_model = channel.upstream_model(name).to_string();
            // 账上记下「真正被叫的是谁」：面板上看得见 auto 这一发走了哪个模型
            if plan.explored.as_deref() == Some(name.as_str()) {
                let c = state.conn();
                let _ = c.execute("UPDATE requests SET explored=1 WHERE id=?1", [request_id]);
            }

            // 先按这条渠道「这个模型名」的清单把不认的字段丢掉，再换模型名。
            // 顺序无所谓（丢的是顶层字段、换的也是顶层字段），但别把 `model` 写进清单里 ——
            // 那样等于把模型名连同别的字段一起删掉，上游会以为没带模型。
            let adapted = tokengo_shared::strip_fields(&prepared, &channel.drop_fields);
            let mut body = if let Some(kind) = req.endpoint.tool_kind() {
                crate::search_tools::normalize_request(kind, &adapted)
                    .map_err(ApiError::bad_request)?
            } else {
                rewrite_model(&adapted, &sent_model)
            };
            let cap_key = output_cap_key(channel, &sent_model, plan.secret(channel.id));
            let cached_cap = crate::db::settings::get(&state.conn(), &cap_key)
                .ok()
                .flatten()
                .and_then(|s| s.parse::<u64>().ok());
            let notes = match (req.endpoint.protocol(), upstream_protocol) {
                (Some(from), Some(to)) => crate::protocol::normalization_notes(from, to, &req.body),
                _ => vec![],
            };
            let mut adaptation_note = (!notes.is_empty()).then(|| notes.join("; "));
            if let Some(cap) = cached_cap {
                if clamp_output(&mut body, cap) {
                    let cap_note = format!("按此渠道已知输出上限收窄至 {cap} tokens");
                    adaptation_note = Some(match adaptation_note.take() {
                        Some(previous) => format!("{previous}; {cap_note}"),
                        None => cap_note,
                    });
                }
            }
            let mut body_bytes = Bytes::from(
                serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?,
            );
            let mut learned_this_request = false;

            let mut retry_after_override: Option<u64> = None;
            // 上一次是「快失败」还是「慢失败」。连接被拒 / 5xx / 429 这类上游马上就回了，
            // 重试有意义；耗时顶到超时的失败多半是上游在挂，再等一轮只会把整个请求预算吃掉、
            // 让客户端白等，所以那种直接换下一候选（冷却计数照记）。
            let mut slow_previous = false;
            // 这一候选上见过的最大 Retry-After：冷却时当下限用（上游自己说的等多久最准）。
            let mut pending_retry_ms: i64 = 0;
            for attempt_no in 1..=max_attempts {
                if attempt_no > 1 {
                    let left = cfg.request_budget_ms as i64 - (now_ms() - started_ms);
                    if slow_previous || left < attempt_ms as i64 {
                        break;
                    }
                    // 上游在 Retry-After 里说了等多久就等多久（它比我们的默认节拍更懂自己）
                    let d = retry_after_override
                        .take()
                        .unwrap_or_else(|| retry_delay_ms(attempt_no));
                    if d > 0 {
                        tokio::time::sleep(Duration::from_millis(d.min(left.max(0) as u64))).await;
                    }
                }
                // 重试总预算：超了不再重试（不做无上限的等待）
                if now_ms() - started_ms >= cfg.request_budget_ms as i64 {
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

                guard.settle_billing("retry_or_fallback");
                guard.usage = Usage::default();
                guard.request_units = None;
                guard.request_billing = req.endpoint.tool_kind().is_some();
                if !crate::db::faults::current(&state.conn(), &cooldown.account)? {
                    break;
                }
                let oauth_remaining = Duration::from_millis(
                    cfg.request_budget_ms
                        .saturating_sub((now_ms() - started_ms).max(0) as u64)
                        .max(1),
                );
                let oauth = match tokio::time::timeout(
                    oauth_remaining,
                    crate::oauth::credentials(&state, channel.id.0),
                )
                .await
                {
                    Ok(Ok(value)) => value,
                    _ => {
                        last_policy_error=Some(ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"oauth_unavailable","ChatGPT authorization is unavailable; reauthorize this account in administration"));
                        continue 'channels;
                    }
                };
                let mapping_reasoning_defaults = if let Some(native) = upstream_protocol {
                    match crate::reasoning_capabilities::apply_defaults_and_validate(
                        &state.conn(),
                        channel.id.0,
                        name,
                        &sent_model,
                        native,
                        &mut body,
                    ) {
                        Ok(value) => value,
                        Err(note) => {
                            last_policy_error = Some(ApiError::bad_request(note));
                            continue 'channels;
                        }
                    }
                } else {
                    false
                };
                if mapping_reasoning_defaults {
                    body_bytes = Bytes::from(
                        serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?,
                    );
                }
                if let Some(credential) = &oauth {
                    if upstream_protocol != Some(crate::protocol::Protocol::Responses)
                        || credential.binding_generation != *binding_generation
                    {
                        let error = ApiError::bad_request(
                            "OAuth source binding changed; retry with current source configuration",
                        );
                        guard.reject(&error);
                        return Err(error);
                    }
                    crate::protocol::collect::prepare_request(
                        &mut body,
                        if mapping_reasoning_defaults {
                            None
                        } else {
                            credential.reasoning_effort.as_deref()
                        },
                        credential.service_tier.as_deref(),
                    )
                    .map_err(|note| {
                        let error = ApiError::bad_request(note);
                        guard.reject(&error);
                        error
                    })?;
                }
                let live_generation: i64 = state
                    .conn()
                    .query_row(
                        "SELECT binding_generation FROM channels WHERE id=?1",
                        [channel.id.0],
                        |r| r.get(0),
                    )
                    .map_err(crate::db::DbError::from)?;
                if live_generation != *binding_generation {
                    let error=ApiError::bad_request("Source configuration changed during request; retry with current configuration");
                    guard.reject(&error);
                    return Err(error);
                }
                {
                    let c = state.conn();
                    let enabled =
                        model_routes::channels_for(&c, name)?
                            .into_iter()
                            .any(|candidate| {
                                candidate.id == channel.id
                                    && candidate.routable()
                                    && (!wants_vision || candidate.vision)
                            });
                    let kind = model_routes::routes_for(&c, name)?
                        .map(|route| route.kind)
                        .unwrap_or_default();
                    let kind_ok = if plan.virtual_route.as_ref().is_some_and(|r| r.shared) {
                        kind == req.endpoint.as_model_kind() || (kind == tokengo_shared::ModelKind::Unknown && req.endpoint.is_generation())
                    } else if plan.vector_contract().is_some() {
                        kind == req.endpoint.as_model_kind()
                    } else { !plan.auto
                        || kind == req.endpoint.as_model_kind()
                        || (kind == tokengo_shared::ModelKind::Unknown
                            && req.endpoint.is_generation()) };
                    let auto_ok = !plan.auto
                        || (crate::db::model_routes::visible(&c, name)?
                            && crate::db::route_policy::load(&c, name)?.include_in_auto
                                != Some(false));
                    if !enabled || !kind_ok || !auto_ok {
                        last_policy_error = Some(ApiError::new(
                            StatusCode::FORBIDDEN,
                            "source_not_allowed",
                            "Model or source routing permission changed while queued",
                        ));
                        continue 'channels;
                    }
                }
                if let Some(route)=&plan.virtual_route {if let Some(contract)=&route.compatibility {let allowed={let c=state.conn();crate::vector_renewal::source_allowed(&c,if route.shared{0}else{route.user_id},contract,channel.id.0,name).map_err(virtual_route_error)?};if !allowed{last_policy_error=Some(ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"vector_source_isolated","此来源已被续验隔离，将选择其余健康来源"));continue 'channels;}}}
                let live_route_check = (|| -> Result<(), ApiError> {
                    let c = state.conn();
                    if let (Some(route), Some(target), Some(auth)) =
                        (&plan.virtual_route, target, &req.auth)
                    {
                        let current_contract = if route.shared {
                            crate::db::shared_virtual_routes::validate_current(&c, auth, route.id, route.version, target.position, now_ms())
                        } else {
                            crate::db::user_virtual_routes::validate_current(&c, auth.user_id, route.id, route.version, target.position, now_ms())
                        }.map_err(virtual_route_error)?;
                        if !crate::vector_compat::execution_equivalent(current_contract.as_ref(), route.compatibility.as_ref()) {
                            return Err(ApiError::new(StatusCode::CONFLICT,
                                "virtual_route_changed", "兼容检查结果或模型用途已变化，请重试"));
                        }
                    }
                    if pooled {
                        if let Some(snapshot) = credential {
                            let current =
                                crate::db::provider_pool::source_credential(&c, channel.id.0)?;
                            if current.as_ref().is_none_or(|current| {
                                !current.enabled
                                    || current.id != snapshot.id
                                    || current.revision != snapshot.revision
                                    || current.provider_version != snapshot.provider_version
                            }) {
                                return Err(ApiError::new(StatusCode::CONFLICT,"credential_changed","Upstream credential or provider configuration changed; retry with current configuration"));
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = live_route_check {
                    guard.reject(&error);
                    return Err(error);
                }
                if let Some(auth) = &req.auth {
                    // Live policy is re-read at every outbound attempt, including retries.
                    let c = state.conn();
                    let Some(current) = crate::db::identity::context(&c, auth.key_id)? else {
                        drop(c);
                        let error = ApiError::unauthorized("Key revoked, expired or user disabled");
                        guard.reject(&error);
                        return Err(error);
                    };
                    if ((plan.virtual_route.is_none() || plan.virtual_route.as_ref().is_some_and(|r| r.shared)) && !current.permits_model(&req.model))
                        || !permitted_plan_candidate(&c, &current, plan.virtual_route.as_ref(), name, channel.id.0, req.endpoint)?
                    {
                        drop(c);
                        let error = ApiError::new(
                            StatusCode::FORBIDDEN,
                            "source_not_allowed",
                            "Source permission changed",
                        );
                        guard.reject(&error);
                        return Err(error);
                    }
                    if pooled {
                        if let Some(credential) = credential {
                            if existing_pin.is_none() {
                                let live_affinity = crate::db::provider_pool::affinity_token(
                                    &c,
                                    current.user_id,
                                    credential.provider_id,
                                )?;
                                if live_affinity.as_ref().is_some_and(|token| {
                                    token.credential_id != credential.id
                                        && plan.entries.iter().any(|candidate| {
                                            candidate.credential.as_ref().is_some_and(|other| {
                                                other.id == token.credential_id
                                                    && pool_group(
                                                        candidate.target.as_ref(),
                                                        other,
                                                        &candidate.name,
                                                    ) == pool_group(
                                                        target.as_ref(),
                                                        credential,
                                                        name,
                                                    )
                                            })
                                        })
                                }) {
                                    drop(c);
                                    let error=ApiError::new(StatusCode::CONFLICT,"affinity_changed","Upstream key affinity changed while this request was queued; retry with the current identity");
                                    guard.reject(&error);
                                    return Err(error);
                                }
                            }
                            if !pool_control.tokens.contains_key(&credential.provider_id) {
                                let token =
                                    crate::db::provider_pool::ensure_affinity_for_selection(
                                        &c,
                                        current.user_id,
                                        credential.provider_id,
                                        credential.id,
                                    )?;
                                pool_control.tokens.insert(credential.provider_id, token);
                            }
                        }
                    }
                    let price = crate::db::billing::price(&c, name, channel.id.0)?;
                    let context = match oauth.as_ref() {
                        Some(credential) => credential
                            .max_input_tokens
                            .zip(credential.max_output_tokens),
                        None => protocols.context_bounds(),
                    };
                    let image_input = req.endpoint.has_image(&req.body);
                    let tpm_limited = current.effective_policy().tpm.is_some()
                        || current.ancestor_key_ids.iter().any(|id| {
                            crate::db::identity::context(&c, *id)
                                .ok()
                                .flatten()
                                .is_some_and(|parent| parent.effective_policy().tpm.is_some())
                        });
                    // An aggregate admission maximum is not this source's image context.
                    // Preserve unbounded native images only when token limits do not apply.
                    if image_input
                        && tpm_limited
                        && context.is_none()
                        && (oauth.is_some()
                            || !price.as_ref().is_some_and(|p| p.max_input_tokens > 0))
                    {
                        last_policy_error=Some(ApiError::new(StatusCode::FORBIDDEN,"pricing_required","Image input with token limits requires this source's configured context bounds"));
                        continue 'channels;
                    }
                    let full_context_input = has_input_audio(&req.body)
                        || req.endpoint.protocol().is_some_and(|p| {
                            crate::protocol_sessions::opaque_history(p, &req.body)
                        })
                        || (image_input && (tpm_limited || context.is_some() || price.is_some()));
                    let bounds = if req.endpoint.tool_kind().is_some() {
                        Ok((0, 0))
                    } else if oauth.is_some() {
                        subscription_bound(&body, price.as_ref(), context)
                    } else {
                        billing_bound_with_context(
                            &mut body,
                            match upstream_protocol {
                                Some(crate::protocol::Protocol::Responses) => {
                                    EndpointKind::Responses
                                }
                                Some(crate::protocol::Protocol::Messages) => EndpointKind::Messages,
                                Some(crate::protocol::Protocol::Chat) => EndpointKind::Chat,
                                None => req.endpoint,
                            },
                            price.as_ref(),
                            context,
                        )
                    };
                    let (input, output) = bounds.map_err(|error| {
                        guard.reject(&error);
                        error
                    })?;
                    if full_context_input && admitted_input.is_some_and(|reserved| input > reserved)
                    {
                        let error=ApiError::new(StatusCode::BAD_REQUEST,"source_context_changed","Source context exceeds the admitted input reservation; retry with current configuration");
                        guard.reject(&error);
                        return Err(error);
                    }
                    body_bytes = Bytes::from(
                        serde_json::to_vec(&body).map_err(|e| ApiError::internal(e.to_string()))?,
                    );
                    let attempt_id = format!("{}:{}", request_id, ordinal);
                    let reservation = if req.endpoint.tool_kind().is_some() {
                        match crate::db::billing::request_price(&c, name, channel.id.0)? {
                            Some(p) => crate::db::billing::reserve_request(
                                &c,
                                current.user_id,
                                current.key_id,
                                request_id,
                                &attempt_id,
                                &p,
                            ),
                            None => {
                                last_policy_error = Some(ApiError::new(
                                    StatusCode::FORBIDDEN,
                                    "pricing_required",
                                    "Standalone tools require a configured per-request price",
                                ));
                                continue 'channels;
                            }
                        }
                    } else {
                        match price {
                            Some(p) => crate::db::billing::reserve(
                                &c,
                                current.user_id,
                                current.key_id,
                                request_id,
                                &attempt_id,
                                &p,
                                input,
                                output,
                            ),
                            None => crate::db::billing::reserve_unpriced(
                                &c,
                                current.user_id,
                                current.key_id,
                                request_id,
                                &attempt_id,
                                channel.id.0,
                                name,
                            ),
                        }
                    };
                    let id = match reservation {
                        Ok(id) => id,
                        Err(e) => {
                            let error = billing_error(e);
                            if matches!(error.kind, "budget_exceeded" | "pricing_required") {
                                crate::db::identity::audit(
                                    &c,
                                    Some(&current),
                                    "attempt.denied",
                                    "channel",
                                    channel.id.0,
                                    serde_json::json!({"request_id":request_id,"model":name,"reason":error.kind}),
                                )?;
                                last_policy_error = Some(error);
                                continue 'channels;
                            }
                            guard.reject(&error);
                            return Err(error);
                        }
                    };
                    guard.billing_nonchat = !req.endpoint.is_generation();
                    guard.reservation = Some(id);
                    // Capture the actual loaded deployment, independent of later channel edits.
                    // Origin excludes URL credentials, paths, query parameters and fragments.
                    let source = serde_json::json!({"channel_id":channel.id.0,"channel_name":channel.name,
                    "origin":reqwest::Url::parse(&channel.base_url).ok().map(|u|u.origin().ascii_serialization()),
                    "upstream_model":sent_model,"config_revision":revision,"binding_generation":binding_generation,"upstream_protocol":upstream_protocol,"client_protocol":req.endpoint.protocol(),
                    "virtual_model_scope":plan.virtual_route.as_ref().map(|r|if r.shared {"shared"} else {"private"}),"virtual_model_id":plan.virtual_route.as_ref().map(|route|route.id),"virtual_model_version":plan.virtual_route.as_ref().map(|route|route.version),
                    "target_position":target.as_ref().map(|target|target.position),"provider_id":credential.as_ref().map(|credential|credential.provider_id),"credential_id":credential.as_ref().map(|credential|credential.id),"credential_revision":credential.as_ref().map(|credential|credential.revision),
                    "oauth_account_id":oauth.as_ref().map(|c|c.account_id),"oauth_credential_revision":oauth.as_ref().map(|c|c.credential_revision)});
                    if let Err(e)=c.execute("UPDATE billing_ledger SET source_snapshot=?2,input_bound=?3,output_bound=?4 WHERE id=?1",rusqlite::params![id,source.to_string(),input,output]) {
                    if crate::db::billing::release(&c,id,"not_sent").is_ok(){guard.reservation=None;}
                    return Err(ApiError::from(crate::db::DbError::from(e)));
                }
                }
                if let (Some(kind), Some(caps)) = (req.endpoint.tool_kind(), tool_caps.as_ref()) {
                    if !state.tool_rates.admit(&caps.account_scope, kind, caps.qps) {
                        guard.request_units = Some(0);
                        guard.settle_billing("source_rate_limited_before_send");
                        last_policy_error = Some(ApiError::new(
                            StatusCode::TOO_MANY_REQUESTS,
                            "source_rate_limit",
                            "Standalone tool account reached its configured local request rate",
                        ));
                        continue 'channels;
                    }
                }
                // A later real source attempt supersedes an earlier representational mismatch.
                if last_policy_error.as_ref().is_some_and(|error| {
                    matches!(error.kind, "protocol_not_supported" | "oauth_unavailable")
                }) {
                    last_policy_error = None;
                }
                tried_any = true;
                if pooled {
                    if let Some(credential) = credential {
                        pool_control.seen.insert((
                            target.as_ref().map(|target| target.position),
                            credential.id,
                            name.clone(),
                        ));
                    }
                }
                last_channel = channel.id;
                guard.note_served(name);
                guard.channel_id = Some(channel.id.0);
                state.enqueue_write(crate::state::WriteJob::Progress(requests::ProgressArgs {
                    request_id,
                    channel_id: channel.id.0,
                    served_model: name.clone(),
                    streamed: req.stream,
                }));
                let t0 = now_ms();
                let uuid = crate::upstream::new_uuid();
                guard.active_attempt = Some((
                    requests::AttemptArgs {
                        request_id,
                        ordinal,
                        channel_id: channel.id.0,
                        model_name: name.clone(),
                        upstream_model: sent_model.clone(),
                        outcome: "canceled".into(),
                        http_status: None,
                        duration_ms: 0,
                        error: Some("客户端取消当前尝试".into()),
                        first_byte_ms: None,
                    },
                    t0,
                ));
                let mut oauth_channel = channel.clone();
                if let Some(credential) = &oauth {
                    oauth_channel.base_url = credential.api_base.clone();
                    oauth_channel.headers=crate::oauth::inference_headers(credential);
                }
                let outbound_secret = oauth
                    .as_ref()
                    .map(|credential| credential.access_token.as_str())
                    .or_else(|| plan.secret(channel.id));
                let outcome = state
                    .upstream
                    .call_via(
                        &oauth_channel,
                        outbound_secret,
                        upstream_path,
                        body_bytes.clone(),
                        req.stream || oauth.is_some(),
                        attempt_timeout.min(Duration::from_millis(
                            (cfg.request_budget_ms as i64 - (now_ms() - started_ms)).max(1) as u64,
                        )),
                        &uuid,
                        now_ms(),
                        session.as_deref(),
                        oauth.is_some(),
                    )
                    .await;

                let resp = match outcome {
                    Err(fault) => {
                        if plan.automatic_target_failover() { last_http = None; }
                        let dur = now_ms() - t0;
                        slow_previous = dur >= attempt_ms as i64 * 3 / 4;
                        guard.settle_billing("channel_fault");
                        guard.active_attempt = None;
                        record_attempt(
                            &state,
                            request_id,
                            ordinal,
                            channel,
                            name,
                            &sent_model,
                            "channel_fault",
                            None,
                            dur,
                            Some(&fault.message),
                        );
                        ordinal += 1;
                        last_fault = Some(format!("{}：{}", channel.name, fault.message));
                        if cooldown.failure(&fault.message, pending_retry_ms) {
                            break;
                        }
                        continue;
                    }
                    Ok(r) => r,
                };

                let status = resp.status().as_u16();
                if req.endpoint.tool_kind().is_some() && status != 200 {
                    guard.request_units = Some(0);
                }
                let dur = now_ms() - t0;
                slow_previous = dur >= attempt_ms as i64 * 3 / 4;
                let classification = if req.endpoint.tool_kind().is_some() && status == 408 {
                    AttemptOutcome::ChannelFault
                } else {
                    classify_status(status)
                };
                match classification {
                    AttemptOutcome::Success => {
                        // 流式：第一个字节发出去之后就不能换渠道了，所以先记账、更新路由状态，
                        // 再直接透传（空不空只能等流完再判断，那时已经改不了什么，见 StreamState）。
                        if req.stream {
                            let source =
                                upstream_protocol.unwrap_or(crate::protocol::Protocol::Chat);
                            let client = req.endpoint.protocol().unwrap_or(source);
                            let left = attempt_ms
                                .saturating_sub((now_ms() - t0).max(0) as u64)
                                .min(
                                    cfg.request_budget_ms
                                        .saturating_sub((now_ms() - started_ms).max(0) as u64),
                                );
                            let mut resp = resp;
                            let mut prepared = match prepare_stream(
                                &mut resp,
                                source,
                                client,
                                Duration::from_millis(left),
                                attempt_timeout,
                                &mut guard,
                            )
                            .await
                            {
                                Ok(p) => p,
                                Err(note) => {
                                    if is_conversion_fault(&note) {
                                        guard.settle_billing("protocol_fault");
                                        guard.active_attempt = None;
                                        record_attempt(
                                            &state,
                                            request_id,
                                            ordinal,
                                            channel,
                                            name,
                                            &sent_model,
                                            "protocol_fault",
                                            Some(status),
                                            now_ms() - t0,
                                            Some(&note),
                                        );
                                        ordinal += 1;
                                        last_policy_error = Some(ApiError::new(
                                            StatusCode::BAD_REQUEST,
                                            "protocol_not_supported",
                                            note,
                                        ));
                                        continue 'channels;
                                    }
                                    guard.settle_billing("precommit_fault");
                                    guard.active_attempt = None;
                                    record_attempt(
                                        &state,
                                        request_id,
                                        ordinal,
                                        channel,
                                        name,
                                        &sent_model,
                                        "channel_fault",
                                        Some(status),
                                        now_ms() - t0,
                                        Some(&note),
                                    );
                                    ordinal += 1;
                                    last_fault = Some(format!("{}：{note}", channel.name));
                                    slow_previous = now_ms() - t0 >= attempt_ms as i64 * 3 / 4;
                                    if cooldown.failure(&note, pending_retry_ms) {
                                        break;
                                    }
                                    continue;
                                }
                            };
                            if let (Some(key), Some(pin)) = (&pin_key, &candidate_pin) {
                                let claimed = {
                                    let conn = state.conn();
                                    crate::protocol_sessions::claim(&conn, key, pin)
                                };
                                match claimed {
                                    Ok(true) => {}
                                    Ok(false) => {
                                        let error=ApiError::bad_request("Concurrent native session selected a different source; start a new session");
                                        guard.reject(&error);
                                        return Err(error);
                                    }
                                    Err(_) => {
                                        guard.settle_billing("local_state_fault");
                                        guard.active_attempt = None;
                                        record_attempt(
                                            &state,
                                            request_id,
                                            ordinal,
                                            channel,
                                            name,
                                            &sent_model,
                                            "local_fault",
                                            Some(status),
                                            now_ms() - t0,
                                            Some("Native session state unavailable"),
                                        );
                                        guard.error_code = Some("state_unavailable".into());
                                        guard.finish(
                                            &state,
                                            Status::Failed,
                                            Some(503),
                                            Some(channel.id.0),
                                            started_ms,
                                            None,
                                            0,
                                            0,
                                            false,
                                            Some("Native session state unavailable"),
                                        );
                                        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"state_unavailable","Native session state could not be persisted; no fallback was attempted"));
                                    }
                                }
                            }
                            if let (Some(owner), Some(pin)) = (&pin_key, &candidate_pin) {
                                let persisted =
                                    prepared.artifacts.persist(&state.conn(), owner, pin);
                                if persisted.is_err() {
                                    guard.settle_billing("local_state_fault");
                                    guard.active_attempt = None;
                                    record_attempt(
                                        &state,
                                        request_id,
                                        ordinal,
                                        channel,
                                        name,
                                        &sent_model,
                                        "local_fault",
                                        Some(status),
                                        now_ms() - t0,
                                        Some("Opaque artifact state unavailable"),
                                    );
                                    guard.error_code = Some("state_unavailable".into());
                                    guard.finish(
                                        &state,
                                        Status::Failed,
                                        Some(503),
                                        Some(channel.id.0),
                                        started_ms,
                                        None,
                                        0,
                                        0,
                                        false,
                                        Some("Opaque artifact state unavailable"),
                                    );
                                    return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"state_unavailable","Opaque history state could not be persisted; no fallback was attempted"));
                                }
                            }
                            if touch_state {
                                let mut r = state.routing();
                                r.on_success_with_interval(
                                    &plan.route,
                                    channel.id,
                                    &all_ids,
                                    now_ms(),
                                    cfg.reprobe_interval_ms,
                                );
                                drop(r);
                                state.persist_state(&plan.route)?;
                            }
                            return Ok(stream_passthrough(
                                state.clone(),
                                resp,
                                prepared,
                                pin_key.clone().zip(candidate_pin.clone()),
                                guard,
                                request_id,
                                started_ms,
                                channel.id.0,
                                permit,
                                req.endpoint,
                                ordinal,
                                name.clone(),
                                sent_model.clone(),
                                t0,
                                adaptation_note.clone(),
                                cooldown,
                                load_ticket,
                                attempt_timeout,
                            ));
                        }

                        // 非流式：**先把正文读下来再算成功**。有的上游回 200 却一句话不说
                        // （现网 4 天 9 次），那种必须当这条渠道的毛病去重试/换渠道 —— 不能先把
                        // 路由状态记成「这条渠道好了」，更不能把空泡泡原样丢给用户。
                        let http = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
                        let (resp, input, output, empty, usage) = if let Some(kind) =
                            req.endpoint.tool_kind()
                        {
                            let (response, units, error) = buffer_tool_response(resp, kind).await;
                            guard.request_units = units;
                            (response, 0, 0, error, Usage::default())
                        } else if oauth.is_some() {
                            let left = attempt_ms
                                .saturating_sub((now_ms() - t0).max(0) as u64)
                                .min(
                                    cfg.request_budget_ms
                                        .saturating_sub((now_ms() - started_ms).max(0) as u64),
                                );
                            buffer_responses_stream(
                                resp,
                                req.endpoint
                                    .protocol()
                                    .unwrap_or(crate::protocol::Protocol::Responses),
                                pin_key
                                    .as_deref()
                                    .zip(candidate_pin.as_ref())
                                    .map(|(owner, pin)| (&state, owner, pin)),
                                Duration::from_millis(left),
                                attempt_timeout,
                                &mut guard,
                            )
                            .await
                        } else if req.endpoint == EndpointKind::Rerank {
                            buffer_rerank_passthrough(resp, http, &req.body, plan.vector_contract()).await
                        } else {
                            buffer_passthrough(
                                resp,
                                http,
                                channel.unwrap_envelope,
                                req.endpoint == EndpointKind::Chat,
                                upstream_protocol.zip(req.endpoint.protocol()),
                                pin_key
                                    .as_deref()
                                    .zip(candidate_pin.as_ref())
                                    .map(|(owner, pin)| (&state, owner, pin)),
                                plan.vector_contract().map(|contract| (contract, &req.body)),
                            )
                            .await
                        };
                        let body_dur = now_ms() - t0;
                        slow_previous = body_dur >= attempt_ms as i64 * 3 / 4;
                        guard.usage = usage;

                        if let Some(note) = empty
                            .as_deref()
                            .filter(|note| note.starts_with("local_state:"))
                        {
                            guard.settle_billing("local_state_fault");
                            guard.active_attempt = None;
                            record_attempt(
                                &state,
                                request_id,
                                ordinal,
                                channel,
                                name,
                                &sent_model,
                                "local_fault",
                                Some(status),
                                now_ms() - t0,
                                Some(note),
                            );
                            guard.error_code = Some("state_unavailable".into());
                            guard.finish(
                                &state,
                                Status::Failed,
                                Some(503),
                                Some(channel.id.0),
                                started_ms,
                                None,
                                input,
                                output,
                                false,
                                Some(note),
                            );
                            return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"state_unavailable","Opaque history state could not be persisted; no fallback was attempted"));
                        }
                        if let Some(note) =
                            empty.as_deref().filter(|note| is_conversion_fault(note))
                        {
                            guard.settle_billing("protocol_fault");
                            guard.active_attempt = None;
                            record_attempt(
                                &state,
                                request_id,
                                ordinal,
                                channel,
                                name,
                                &sent_model,
                                "protocol_fault",
                                Some(status),
                                now_ms() - t0,
                                Some(note),
                            );
                            ordinal += 1;
                            last_policy_error = Some(ApiError::new(
                                StatusCode::BAD_REQUEST,
                                "protocol_not_supported",
                                note,
                            ));
                            continue 'channels;
                        }
                        if let Some(note) = empty {
                            if plan.automatic_target_failover() { last_http = None; }
                            guard.settle_billing("empty_reply");
                            guard.active_attempt = None;
                            record_attempt(
                                &state,
                                request_id,
                                ordinal,
                                channel,
                                name,
                                &sent_model,
                                "empty_reply",
                                Some(status),
                                dur,
                                Some(&note),
                            );
                            ordinal += 1;
                            tracing::warn!(
                                "渠道 {} 回了 200 但一句话没说，按这条渠道的毛病处理",
                                channel.name
                            );
                            last_fault = Some(format!("{}：{note}", channel.name));
                            if cooldown.failure(&note, pending_retry_ms) {
                                break;
                            }
                            continue;
                        }
                        if let (Some(key), Some(pin)) = (&pin_key, &candidate_pin) {
                            if !crate::protocol_sessions::claim(&state.conn(), key, pin)? {
                                let error=ApiError::bad_request("Concurrent native session selected a different source; start a new session");
                                guard.reject(&error);
                                return Err(error);
                            }
                        }
                        guard.settle_billing("success");
                        guard.active_attempt = None;
                        record_attempt(
                            &state,
                            request_id,
                            ordinal,
                            channel,
                            name,
                            &sent_model,
                            "success",
                            Some(status),
                            now_ms() - t0,
                            adaptation_note.as_deref(),
                        );
                        cooldown.finish(true, "", 0);
                        state.note_model_endpoint(name, req.endpoint);
                        // 命中的是首选就视为已回切；不是就记下当前渠道、重设检查点
                        if touch_state {
                            let mut r = state.routing();
                            r.on_success_with_interval(
                                &plan.route,
                                channel.id,
                                &all_ids,
                                now_ms(),
                                cfg.reprobe_interval_ms,
                            );
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
                        // 完整客户端错误原样返回；错误正文不可读属于渠道故障，可以切换。
                        let remaining = attempt_ms
                            .saturating_sub((now_ms() - t0).max(0) as u64)
                            .min(
                                cfg.request_budget_ms
                                    .saturating_sub((now_ms() - started_ms).max(0) as u64),
                            );
                        let err = match read_error(
                            resp,
                            Duration::from_millis(remaining),
                            attempt_timeout,
                        )
                        .await
                        {
                            Ok(err) => err,
                            Err(note) => {
                                let elapsed = now_ms() - t0;
                                slow_previous = elapsed >= attempt_ms as i64 * 3 / 4;
                                guard.settle_billing("channel_fault");
                                guard.active_attempt = None;
                                record_attempt(
                                    &state,
                                    request_id,
                                    ordinal,
                                    channel,
                                    name,
                                    &sent_model,
                                    "channel_fault",
                                    Some(status),
                                    elapsed,
                                    Some(&note),
                                );
                                ordinal += 1;
                                last_fault = Some(format!("{}：{note}", channel.name));
                                // No complete HTTP error is available to pass through. A previous
                                // candidate's error must not mask this transport/protocol failure.
                                last_http = None;
                                if cooldown.failure(&note, pending_retry_ms) {
                                    break;
                                }
                                continue;
                            }
                        };
                        // A precise backend capacity refusal is request-local, not a broken
                        // account/source. Preserve the whole body and walk authorized candidates.
                        if let Some(capacity) = vector_capacity_rejection(req.endpoint, status, &err.body) {
                            let note = format!("{}; request_shape={}", capacity, vector_request_shape(&req.body));
                            guard.usage = serde_json::from_slice::<Value>(&err.body)
                                .map(|value| Usage::read(&value)).unwrap_or_default();
                            guard.settle_billing("context_limit");
                            guard.active_attempt = None;
                            record_attempt(&state, request_id, ordinal, channel, name, &sent_model,
                                "context_limit", Some(status), now_ms()-t0, Some(&note));
                            ordinal += 1;
                            last_fault = Some(note);
                            last_capacity_http = Some(err);
                            // Dropping the ticket releases recovery leases without changing health.
                            continue 'channels;
                        }
                        if matches!(status, 400 | 429) && (pooled || plan.virtual_route.is_some()) {
                            let switched = match advance_explicit_route(
                                &state,
                                &plan,
                                entry_idx,
                                status,
                                req.auth.as_ref(),
                                existing_pin.is_some(),
                                &mut pool_control,
                                &mut active_target,
                            ) {
                                Ok(switched) => switched,
                                Err(error) => {
                                    guard.usage = serde_json::from_slice::<Value>(&err.body)
                                        .map(|value| Usage::read(&value))
                                        .unwrap_or_default();
                                    guard.settle_billing("affinity_changed");
                                    guard.active_attempt = None;
                                    record_attempt(
                                        &state,
                                        request_id,
                                        ordinal,
                                        channel,
                                        name,
                                        &sent_model,
                                        "affinity_changed",
                                        Some(status),
                                        now_ms() - t0,
                                        Some(&error.message),
                                    );
                                    guard.reject(&error);
                                    return Err(error);
                                }
                            };
                            if switched {
                                if status == 429 {
                                    defer_explicit_retry_after(&state, &plan, entry_idx, &err)?;
                                }
                                guard.usage = serde_json::from_slice::<Value>(&err.body)
                                    .map(|value| Usage::read(&value))
                                    .unwrap_or_default();
                                guard.settle_billing("explicit_route_switch");
                                guard.active_attempt = None;
                                record_attempt(&state,request_id,ordinal,channel,name,&sent_model,"explicit_route_switch",Some(status),now_ms()-t0,Some("Explicit request-local key/target switch; no global key suspension"));
                                ordinal += 1;
                                last_fault = Some(format!("HTTP {status}: explicit route switch"));
                                last_http = Some(err);
                                continue 'channels;
                            }
                            if plan.automatic_target_failover() {
                                guard.usage = serde_json::from_slice::<Value>(&err.body)
                                    .map(|value| Usage::read(&value)).unwrap_or_default();
                                guard.settle_billing("explicit_route_stop");
                                guard.active_attempt = None;
                                record_attempt(&state, request_id, ordinal, channel, name, &sent_model,
                                    "client_fault", Some(status), now_ms()-t0,
                                    Some("Target switch disabled or no authorized next target"));
                                guard.finish(&state, Status::ClientError, Some(status), Some(channel.id.0),
                                    started_ms, Some(now_ms()-t0), 0, 0, false,
                                    Some("Target switch disabled or no authorized next target"));
                                return Ok(err.into_response());
                            }
                        }
                        if req.endpoint.tool_kind().is_some() {
                            guard.request_units = Some(0);
                            guard.settle_billing("tool_client_error");
                            guard.active_attempt = None;
                            record_attempt(&state,request_id,ordinal,channel,name,&sent_model,"client_fault",Some(status),now_ms()-t0,Some("Standalone tool request rejected; source model health unchanged"));
                            guard.finish(
                                &state,
                                Status::ClientError,
                                Some(status),
                                Some(channel.id.0),
                                started_ms,
                                Some(now_ms() - t0),
                                0,
                                0,
                                false,
                                Some("Standalone tool request rejected"),
                            );
                            return Ok(err.into_response());
                        }
                        guard.usage = serde_json::from_slice::<Value>(&err.body)
                            .map(|v| Usage::read(&v))
                            .unwrap_or_default();
                        if let Some(fault) = crate::db::faults::classify(status, &err.body) {
                            let reason = fault.label();
                            cooldown.suspend(
                                fault,
                                reason,
                                err.retry_after
                                    .as_ref()
                                    .and_then(|v| v.to_str().ok())
                                    .and_then(crate::upstream::retry_after_ms),
                            )?;
                            guard.settle_billing("account_fault");
                            guard.active_attempt = None;
                            record_attempt(
                                &state,
                                request_id,
                                ordinal,
                                channel,
                                name,
                                &sent_model,
                                "account_fault",
                                Some(status),
                                dur,
                                Some(reason),
                            );
                            ordinal += 1;
                            last_fault = Some(format!("{}：{}", channel.name, reason));
                            last_http = Some(err);
                            continue 'channels;
                        }

                        guard.usage = serde_json::from_slice::<Value>(&err.body)
                            .map(|v| Usage::read(&v))
                            .unwrap_or_default();
                        if matches!(status, 400 | 413 | 422) && context_rejection(&err.text) {
                            guard.settle_billing("context_limit");
                            guard.active_attempt = None;
                            record_attempt(
                                &state,
                                request_id,
                                ordinal,
                                channel,
                                name,
                                &sent_model,
                                "context_limit",
                                Some(status),
                                dur,
                                Some(&err.text),
                            );
                            ordinal += 1;
                            last_fault =
                                Some(format!("渠道 {} 上下文不足：{}", channel.name, err.text));
                            last_http = Some(err);
                            continue 'channels;
                        }
                        if req.endpoint == EndpointKind::Chat
                            && matches!(status, 400 | 422)
                            && !learned_this_request
                        {
                            if let Some(cap) = output_ceiling(&err.text) {
                                if clamp_output(&mut body, cap) {
                                    learned_this_request = true;
                                    let _ = crate::db::settings::set(
                                        &state.conn(),
                                        &cap_key,
                                        &cap.to_string(),
                                    );
                                    adaptation_note = Some(format!(
                                        "上游明确输出上限为 {cap} tokens，已收窄重试"
                                    ));
                                    guard.settle_billing("output_limit");
                                    guard.active_attempt = None;
                                    record_attempt(
                                        &state,
                                        request_id,
                                        ordinal,
                                        channel,
                                        name,
                                        &sent_model,
                                        "output_limit",
                                        Some(status),
                                        dur,
                                        adaptation_note.as_deref(),
                                    );
                                    ordinal += 1;
                                    body_bytes = Bytes::from(
                                        serde_json::to_vec(&body)
                                            .map_err(|e| ApiError::internal(e.to_string()))?,
                                    );
                                    if attempt_no < max_attempts {
                                        continue;
                                    }
                                    last_http = Some(err);
                                    continue 'channels;
                                }
                            }
                        }
                        guard.settle_billing("client_fault");
                        guard.active_attempt = None;
                        record_attempt(
                            &state,
                            request_id,
                            ordinal,
                            channel,
                            name,
                            &sent_model,
                            "client_fault",
                            Some(status),
                            dur,
                            Some(&err.text),
                        );
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
                        // Tool QPS and URL security errors must not suspend model-account access.
                        if req.endpoint.tool_kind().is_some()
                            && matches!(status, 401 | 402 | 403 | 404 | 429)
                        {
                            let remaining = attempt_ms
                                .saturating_sub((now_ms() - t0).max(0) as u64)
                                .min(
                                    cfg.request_budget_ms
                                        .saturating_sub((now_ms() - started_ms).max(0) as u64),
                                );
                            let err = match read_error(
                                resp,
                                Duration::from_millis(remaining),
                                attempt_timeout,
                            )
                            .await
                            {
                                Ok(err) => err,
                                Err(_) => {
                                    let note = "Standalone tool error body could not be read";
                                    guard.settle_billing("tool_error_unreadable");
                                    guard.active_attempt = None;
                                    record_attempt(
                                        &state,
                                        request_id,
                                        ordinal,
                                        channel,
                                        name,
                                        &sent_model,
                                        "tool_error",
                                        Some(status),
                                        now_ms() - t0,
                                        Some(note),
                                    );
                                    guard.finish(
                                        &state,
                                        Status::Failed,
                                        Some(502),
                                        Some(channel.id.0),
                                        started_ms,
                                        Some(now_ms() - t0),
                                        0,
                                        0,
                                        false,
                                        Some(note),
                                    );
                                    return Err(ApiError::new(
                                        StatusCode::BAD_GATEWAY,
                                        "tool_error_unreadable",
                                        note,
                                    ));
                                }
                            };
                            guard.request_units = Some(0);
                            guard.settle_billing("tool_rejected");
                            guard.active_attempt = None;
                            record_attempt(
                                &state,
                                request_id,
                                ordinal,
                                channel,
                                name,
                                &sent_model,
                                "tool_rejected",
                                Some(status),
                                now_ms() - t0,
                                Some(
                                    "Standalone tool rejected request; no model-account suspension",
                                ),
                            );
                            if matches!(status, 401 | 402 | 429) {
                                ordinal += 1;
                                last_http = Some(err);
                                last_fault =
                                    Some("Standalone tool source rejected the request".into());
                                continue 'channels;
                            }
                            guard.finish(
                                &state,
                                Status::ClientError,
                                Some(status),
                                Some(channel.id.0),
                                started_ms,
                                Some(now_ms() - t0),
                                0,
                                0,
                                false,
                                Some("Standalone tool rejected request"),
                            );
                            return Ok(err.into_response());
                        }
                        let remaining = attempt_ms
                            .saturating_sub((now_ms() - t0).max(0) as u64)
                            .min(
                                cfg.request_budget_ms
                                    .saturating_sub((now_ms() - started_ms).max(0) as u64),
                            );
                        let err = match read_error(
                            resp,
                            Duration::from_millis(remaining),
                            attempt_timeout,
                        )
                        .await
                        {
                            Ok(err) => err,
                            Err(note) => {
                                let elapsed = now_ms() - t0;
                                slow_previous = elapsed >= attempt_ms as i64 * 3 / 4;
                                guard.settle_billing("channel_fault");
                                guard.active_attempt = None;
                                record_attempt(
                                    &state,
                                    request_id,
                                    ordinal,
                                    channel,
                                    name,
                                    &sent_model,
                                    "channel_fault",
                                    Some(status),
                                    elapsed,
                                    Some(&note),
                                );
                                ordinal += 1;
                                last_fault = Some(format!("{}：{note}", channel.name));
                                // No complete HTTP error is available to pass through. A previous
                                // candidate's error must not mask this transport/protocol failure.
                                last_http = None;
                                if cooldown.failure(&note, pending_retry_ms) {
                                    break;
                                }
                                continue;
                            }
                        };
                        // A precise backend capacity refusal is request-local, not a broken
                        // account/source. Preserve the whole body and walk authorized candidates.
                        if let Some(capacity) = vector_capacity_rejection(req.endpoint, status, &err.body) {
                            let note = format!("{}; request_shape={}", capacity, vector_request_shape(&req.body));
                            guard.usage = serde_json::from_slice::<Value>(&err.body)
                                .map(|value| Usage::read(&value)).unwrap_or_default();
                            guard.settle_billing("context_limit");
                            guard.active_attempt = None;
                            record_attempt(&state, request_id, ordinal, channel, name, &sent_model,
                                "context_limit", Some(status), now_ms()-t0, Some(&note));
                            ordinal += 1;
                            last_fault = Some(note);
                            last_capacity_http = Some(err);
                            // Dropping the ticket releases recovery leases without changing health.
                            continue 'channels;
                        }
                        if matches!(status, 400 | 429) && (pooled || plan.virtual_route.is_some()) {
                            let switched = match advance_explicit_route(
                                &state,
                                &plan,
                                entry_idx,
                                status,
                                req.auth.as_ref(),
                                existing_pin.is_some(),
                                &mut pool_control,
                                &mut active_target,
                            ) {
                                Ok(switched) => switched,
                                Err(error) => {
                                    guard.usage = serde_json::from_slice::<Value>(&err.body)
                                        .map(|value| Usage::read(&value))
                                        .unwrap_or_default();
                                    guard.settle_billing("affinity_changed");
                                    guard.active_attempt = None;
                                    record_attempt(
                                        &state,
                                        request_id,
                                        ordinal,
                                        channel,
                                        name,
                                        &sent_model,
                                        "affinity_changed",
                                        Some(status),
                                        now_ms() - t0,
                                        Some(&error.message),
                                    );
                                    guard.reject(&error);
                                    return Err(error);
                                }
                            };
                            if switched {
                                if status == 429 {
                                    defer_explicit_retry_after(&state, &plan, entry_idx, &err)?;
                                }
                                guard.usage = serde_json::from_slice::<Value>(&err.body)
                                    .map(|value| Usage::read(&value))
                                    .unwrap_or_default();
                                guard.settle_billing("explicit_route_switch");
                                guard.active_attempt = None;
                                record_attempt(&state,request_id,ordinal,channel,name,&sent_model,"explicit_route_switch",Some(status),now_ms()-t0,Some("Explicit request-local key/target switch; no global key suspension"));
                                ordinal += 1;
                                last_fault = Some(format!("HTTP {status}: explicit route switch"));
                                last_http = Some(err);
                                continue 'channels;
                            }
                            if plan.automatic_target_failover() {
                                guard.usage = serde_json::from_slice::<Value>(&err.body)
                                    .map(|value| Usage::read(&value)).unwrap_or_default();
                                guard.settle_billing("explicit_route_stop");
                                guard.active_attempt = None;
                                record_attempt(&state, request_id, ordinal, channel, name, &sent_model,
                                    "client_fault", Some(status), now_ms()-t0,
                                    Some("Target switch disabled or no authorized next target"));
                                guard.finish(&state, Status::ClientError, Some(status), Some(channel.id.0),
                                    started_ms, Some(now_ms()-t0), 0, 0, false,
                                    Some("Target switch disabled or no authorized next target"));
                                return Ok(err.into_response());
                            }
                        }
                        guard.usage = serde_json::from_slice::<Value>(&err.body)
                            .map(|v| Usage::read(&v))
                            .unwrap_or_default();
                        if let Some(fault) = crate::db::faults::classify(status, &err.body) {
                            let reason = fault.label();
                            cooldown.suspend(
                                fault,
                                reason,
                                err.retry_after
                                    .as_ref()
                                    .and_then(|v| v.to_str().ok())
                                    .and_then(crate::upstream::retry_after_ms),
                            )?;
                            guard.settle_billing("account_fault");
                            guard.active_attempt = None;
                            record_attempt(
                                &state,
                                request_id,
                                ordinal,
                                channel,
                                name,
                                &sent_model,
                                "account_fault",
                                Some(status),
                                dur,
                                Some(reason),
                            );
                            ordinal += 1;
                            last_fault = Some(format!("{}：{}", channel.name, reason));
                            last_http = Some(err);
                            continue 'channels;
                        }

                        guard.usage = serde_json::from_slice::<Value>(&err.body)
                            .map(|v| Usage::read(&v))
                            .unwrap_or_default();
                        let after = err
                            .retry_after
                            .as_ref()
                            .and_then(|v| v.to_str().ok())
                            .and_then(crate::upstream::retry_after_ms);
                        guard.settle_billing("channel_fault");
                        guard.active_attempt = None;
                        record_attempt(
                            &state,
                            request_id,
                            ordinal,
                            channel,
                            name,
                            &sent_model,
                            "channel_fault",
                            Some(status),
                            dur,
                            Some(&err.text),
                        );
                        ordinal += 1;
                        last_fault = Some(format!("{}：HTTP {status} {}", channel.name, err.text));
                        pending_retry_ms = pending_retry_ms.max(after.unwrap_or(0) as i64);
                        let cooling = cooldown
                            .failure(&format!("HTTP {status}: {}", err.text), pending_retry_ms);
                        last_http = Some(err);
                        if cooling {
                            break;
                        }
                        if let Some(ms) = after {
                            // Long Retry-After sets the eventual cooldown, not an unbounded request sleep.
                            let ms = ms.min(LONG_RETRY_AFTER_MS);
                            retry_after_override = Some(ms);
                        }
                        continue;
                    }
                    AttemptOutcome::ClientGone => {
                        guard.settle_billing("client_gone");
                        guard.active_attempt = None;
                        record_attempt(
                            &state,
                            request_id,
                            ordinal,
                            channel,
                            name,
                            &sent_model,
                            "client_gone",
                            Some(status),
                            dur,
                            None,
                        );
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
                r.on_channel_exhausted_with_interval(
                    &plan.route,
                    channel.id,
                    &all_ids,
                    now_ms(),
                    cfg.reprobe_interval_ms,
                );
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

    if let Some(error) = last_policy_error {
        guard.reject(&error);
        return Err(error);
    }
    // 一个候选都没敢碰（全在冷却或恢复试用里）：如实回 503 并说清最早什么时候能再试 ——
    // 别拿「所有渠道都失败了」去冤枉渠道（渠道根本没被调过）。普通模型名和 auto 一视同仁。
    if last_channel.0 == 0 {
        let c = state.conn();
        let until = model_routes::list(&c)?
            .iter()
            .flat_map(|r| crate::db::policy::rows(&c, &r.name).unwrap_or_default())
            .map(|r| r.until_ms.max(r.lease_until))
            .filter(|t| *t > now_ms())
            .min();
        drop(c);
        let msg = format!(
            "候选冷却、账号暂停或恢复试用中，请在总览“调用异常”查看{}",
            until
                .map(|t| format!("（最早可重试时间：{t}）"))
                .unwrap_or_default()
        );
        guard.error_code = Some("models_cooling".into());
        guard.finish(
            &state,
            Status::Failed,
            Some(503),
            None,
            started_ms,
            None,
            0,
            0,
            false,
            Some(&msg),
        );
        return Ok(
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "models_cooling", msg).into_response(),
        );
    }
    // If capacity was the reason no candidate could serve the original request,
    // preserve that real backend response rather than manufacturing a gateway 400.
    let last_http = last_capacity_http.or(last_http);
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

/// Narrow backend signatures only: arbitrary validation errors must never be retried.
fn vector_capacity_rejection(endpoint: EndpointKind, status: u16, body: &[u8]) -> Option<Value> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let positive = |digits: &str| -> Option<u64> {
        if digits.is_empty() || !digits.bytes().all(|byte|byte.is_ascii_digit()) {return None;}
        digits.parse::<u64>().ok().filter(|n|*n>0)
    };
    if endpoint == EndpointKind::Rerank && matches!(status,400|413|422) {
        let reason=value["detail"]["reason"].as_str()?;
        let digits=reason.strip_prefix("encoded_query_document_pair_exceeds_")?.strip_suffix("_no_truncation_performed")?;
        let limit=positive(digits)?;
        let reported=value["detail"]["max_encoded_pair_tokens"].as_u64();
        if reported.is_some_and(|n|n<=limit) {return None;}
        return Some(serde_json::json!({"reason":"vector_pair_capacity_exceeded","reported_limit_tokens":limit,"reported_input_tokens":reported}));
    }
    if endpoint == EndpointKind::Embeddings && status==500 {
        let message=value["error"]["message"].as_str()?;
        let tail=message.strip_prefix("input (")?;
        let (input,batch)=tail.split_once(" tokens) is too large to process. increase the physical batch size (current batch size: ")?;
        let input=positive(input)?;let batch=positive(batch.strip_suffix(')')?)?;
        if input<=batch {return None;}
        return Some(serde_json::json!({"reason":"vector_physical_batch_capacity_exceeded","reported_input_tokens":input,"reported_limit_tokens":batch}));
    }
    None
}
#[cfg(test)]
mod vector_capacity_tests {
    use super::*;
    #[test]
    fn precise_capacity_signatures_only_match_correct_vector_kind_and_status() {
        let rerank=serde_json::json!({"detail":{"reason":"encoded_query_document_pair_exceeds_8192_no_truncation_performed","max_encoded_pair_tokens":8193}}).to_string();
        assert!(vector_capacity_rejection(EndpointKind::Rerank,422,rerank.as_bytes()).is_some());
        assert!(vector_capacity_rejection(EndpointKind::Embeddings,422,rerank.as_bytes()).is_none());
        assert!(vector_capacity_rejection(EndpointKind::Chat,422,rerank.as_bytes()).is_none());
        for reason in ["invalid documents", "encoded_query_document_pair_exceeds_8192_no_truncation_performed user text", "encoded_query_document_pair_exceeds_x_no_truncation_performed", "encoded_query_document_pair_exceeds_0_no_truncation_performed"] {
            let raw=serde_json::json!({"detail":{"reason":reason}}).to_string();
            assert!(vector_capacity_rejection(EndpointKind::Rerank,422,raw.as_bytes()).is_none());
        }
        let embedding=serde_json::json!({"error":{"message":"input (8193 tokens) is too large to process. increase the physical batch size (current batch size: 8192)"}}).to_string();
        assert!(vector_capacity_rejection(EndpointKind::Embeddings,500,embedding.as_bytes()).is_some());
        assert!(vector_capacity_rejection(EndpointKind::Rerank,500,embedding.as_bytes()).is_none());
        assert!(vector_capacity_rejection(EndpointKind::Embeddings,400,embedding.as_bytes()).is_none());
        let extra=embedding.replace("8192)","8192) private trailing text");
        assert!(vector_capacity_rejection(EndpointKind::Embeddings,500,extra.as_bytes()).is_none());
    }
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
struct CooldownTicket {state:SharedState,id:i64,name:String,upstream:String,generation:i64,done:bool,account:crate::db::faults::Permit}
impl CooldownTicket {
    fn suspend(&mut self,fault:crate::db::faults::Fault,reason:&str,retry:Option<u64>)->Result<(),ApiError>{
        let c=self.state.conn();
        if !crate::db::faults::current(&c,&self.account)? {return Ok(());}
        crate::db::faults::suspend(&c,&self.account,&self.name,fault,reason,now_ms())?;
        if fault==crate::db::faults::Fault::Quota {if let Some(ms)=retry{crate::db::faults::defer_quota(&c,self.id,now_ms(),ms)?;}}
        crate::db::policy::release(&c,self.id,&self.name,&self.upstream,self.generation)?;
        crate::db::faults::complete(&c,&self.account,false)?;
        self.done=true;Ok(())
    }
    fn failure(&mut self,reason:&str,retry:i64)->bool {
        if self.done {return true;}
        let c=self.state.conn();
        if self.account.recovering(){
            if let Err(e)=crate::db::faults::complete(&c,&self.account,false){tracing::warn!("账号恢复记录失败：{e}");}
            let _=crate::db::policy::release(&c,self.id,&self.name,&self.upstream,self.generation);
            self.done=true;return true;
        }
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
        if let Err(e)=crate::db::faults::complete(&c,&self.account,true){tracing::warn!("账号恢复记录失败：{e}");}
        if let Err(e)=crate::db::policy::complete(&c,self.id,&self.name,&self.upstream,self.generation,true,"",0,now_ms()){tracing::warn!("冷却记录失败：{e}");}
    }
}
impl Drop for CooldownTicket {fn drop(&mut self){if !self.done{let c=self.state.conn();let _=crate::db::policy::release(&c,self.id,&self.name,&self.upstream,self.generation);let _=crate::db::faults::release(&c,&self.account);}}}

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
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage { pub input: Option<i64>, pub output: Option<i64> }
impl Usage {
    pub(crate) fn read(v: &Value) -> Self {
        let v=v.get("response").or_else(||v.get("message")).filter(|v|v.is_object()).unwrap_or(v);
        let get = |names: [&str;2]| names.into_iter().find_map(|n| v.get("usage").and_then(|u|u.get(n)).and_then(Value::as_i64).filter(|n|*n>=0));
        let mut input=get(["prompt_tokens","input_tokens"]);
        for key in ["cache_creation_input_tokens","cache_read_input_tokens"] {
            if let Some(n)=v.get("usage").and_then(|u|u.get(key)).and_then(Value::as_i64).filter(|n|*n>=0) {
                input=input.and_then(|old|old.checked_add(n));
            }
        }
        Self { input, output:get(["completion_tokens","output_tokens"]) }
    }
    fn merge(&mut self, other: Self) {
        if other.input.is_some() {self.input=other.input;}
        if other.output.is_some() {self.output=other.output;}
    }
    fn source(self) -> &'static str { if self.input.is_some() || self.output.is_some() {"upstream"} else {"unreported"} }
}
pub fn usage_from_value(v: &Value) -> (i64,i64) { let u=Usage::read(v); (u.input.unwrap_or(0),u.output.unwrap_or(0)) }

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
async fn read_vector_body(response: reqwest::Response) -> Result<Bytes, String> {
    // Use the same response capacity as a direct model call. Compatibility evidence
    // must never impose an additional body cap on an otherwise valid model result.
    response.bytes().await.map_err(|e| if e.is_timeout() {
        "等待非流式完成超时（读取正文）".into()
    } else { format!("upstream body read failed: {e}") })
}

async fn buffer_passthrough(
    resp: reqwest::Response,
    status: StatusCode,
    unwrap_envelope: bool,
    _check_empty: bool,
    protocols: Option<(crate::protocol::Protocol,crate::protocol::Protocol)>,
    provenance:Option<(&SharedState,&str,&crate::protocol_sessions::Pin)>,
    vector: Option<(&crate::vector_compat::Contract, &Value)>,
) -> (Response, i64, i64, Option<String>, Usage) {
    let ctype = content_type(&resp);
    let bytes = match if vector.is_some() { read_vector_body(resp).await }
        else { resp.bytes().await.map_err(|e| if e.is_timeout() { "等待非流式完成超时（读取正文）".into() } else { format!("upstream body read failed: {e}") }) } {
        Ok(bytes) => bytes,
        Err(note) => return invalid_buffer(note),
    };
    let mut out = bytes.clone();
    let empty: Option<String> = None;
    let usage;
    let (input, output) = match serde_json::from_slice::<Value>(&bytes) {
        Ok(mut v) => {
            if unwrap_envelope && tokengo_shared::unwrap_envelope(&mut v) {
                match serde_json::to_vec(&v) {
                    Ok(b) => out = Bytes::from(b),
                    Err(e) => tracing::warn!("剥壳后重新序列化失败，原样回：{e}"),
                }
            }
            usage = Usage::read(&v);
            let mut validated = v.clone();
            // A configured raw envelope remains byte-for-byte compatible, but its payload must be valid.
            tokengo_shared::unwrap_envelope(&mut validated);
            if let Some((contract, body)) = vector {
                if let Err(note) = crate::vector_compat::validate_output(contract, tokengo_shared::ModelKind::Embeddings, body, &validated) {
                    let mut invalid = invalid_buffer(note); invalid.4 = usage; return invalid;
                }
            }
            let validity=match protocols {Some((source,_))=>crate::protocol::validate_response(source,&validated),None=>crate::upstream::validate_response(&validated,"embeddings")};
            if let Err(note) = validity {
                let mut invalid=invalid_buffer(format!("上游回了 200 但一句话都没说或响应不完整：{note}")); invalid.4=usage; return invalid;
            }
            if let Some((source,client))=protocols {
                if source!=client {
                    match crate::protocol::convert_response(source,client,&validated).and_then(|v|serde_json::to_vec(&v).map_err(|e|e.to_string())) {
                        Ok(b)=>out=Bytes::from(b),
                        Err(note)=>{let mut invalid=invalid_buffer(conversion_fault(note));invalid.4=usage;return invalid;}
                    }
                }
            }
            if let Some((state,owner,pin))=provenance {
                let mut artifacts=crate::protocol_sessions::ArtifactCollector::default();artifacts.value(&validated);
                if artifacts.persist(&state.conn(),owner,pin).is_err(){let mut invalid=invalid_buffer("local_state: opaque artifact ownership could not be persisted".into());invalid.4=usage;return invalid;}
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
        usage,
    )
}

// A valid source may expose semantics the deliberately smaller conversion IR cannot represent.
// Missing fields, malformed JSON, invalid tool snapshots and transport errors remain source faults.
fn conversion_fault(note: String) -> String {
    if note.starts_with("Unsupported ") || note.contains("cannot be converted") || note.contains("cannot be represented") || note.contains("cannot be streamed across protocols") || note.contains("semantics cannot be converted") {
        format!("protocol_conversion: {note}")
    } else { note }
}
fn is_conversion_fault(note:&str)->bool {note.starts_with("protocol_conversion: ")}

fn invalid_buffer(note: String) -> (Response, i64, i64, Option<String>, Usage) {
    (StatusCode::BAD_GATEWAY.into_response(), 0, 0, Some(note), Usage::default())
}

/// Subscription OAuth upstreams require SSE even when the downstream requests JSON.
/// This remains inside the attempt guard: cancellation closes upstream and settles once.
async fn buffer_responses_stream(
    mut resp: reqwest::Response,
    client: crate::protocol::Protocol,
    provenance: Option<(&SharedState,&str,&crate::protocol_sessions::Pin)>,
    remaining: Duration,
    idle: Duration,
    guard: &mut RequestGuard,
) -> (Response, i64, i64, Option<String>, Usage) {
    let mut collector=crate::protocol::collect::ResponsesCollector::default();
    let deadline=tokio::time::Instant::now()+remaining;
    let result=loop {
        let wait=deadline.saturating_duration_since(tokio::time::Instant::now()).min(idle);
        let chunk=match tokio::time::timeout(wait,resp.chunk()).await {
            Ok(Ok(chunk))=>chunk,
            Ok(Err(_))=>break Err("Responses stream body read failed".to_string()),
            Err(_)=>break Err("Responses stream collection deadline exceeded".to_string()),
        };
        let fed=match chunk {Some(chunk)=>collector.feed(&chunk),None=>break collector.finish()};
        guard.usage=Usage {input:collector.observer.input,output:collector.observer.output};
        if let Err(note)=fed {break Err(note);}
        if collector.observer.done {break collector.finish();}
    };
    guard.usage=Usage {input:collector.observer.input,output:collector.observer.output};
    let usage=guard.usage;
    let fail=|note:String| {let mut invalid=invalid_buffer(note);invalid.4=usage;invalid};
    let value=match result {Ok(v)=>v,Err(note)=>return fail(note)};
    let converted=match crate::protocol::convert_oauth_buffered_response(client,&value) {
        Ok(v)=>v,Err(note)=>return fail(conversion_fault(note)),
    };
    let bytes=match serde_json::to_vec(&converted) {Ok(b)=>b,Err(_)=>return fail("Invalid collected Responses JSON".into())};
    if let Some((state,owner,pin))=provenance {
        let mut artifacts=crate::protocol_sessions::ArtifactCollector::default();artifacts.value(&value);
        if artifacts.persist(&state.conn(),owner,pin).is_err() {return fail("local_state: opaque artifact ownership could not be persisted".into());}
    }
    let response=Response::builder().status(StatusCode::OK).header(header::CONTENT_TYPE,"application/json")
        .body(Body::from(bytes)).unwrap_or_else(|_|StatusCode::BAD_GATEWAY.into_response());
    (response,usage.input.unwrap_or(0),usage.output.unwrap_or(0),None,usage)
}

async fn buffer_tool_response(mut resp:reqwest::Response,kind:crate::search_tools::ToolKind)->(Response,Option<i64>,Option<String>) {
    let status=resp.status();let mut bytes=Vec::new();
    loop {match resp.chunk().await {
        Ok(Some(chunk))=>{if bytes.len().saturating_add(chunk.len())>16*1024*1024 {return (StatusCode::BAD_GATEWAY.into_response(),None,Some("Standalone tool body exceeds 16 MiB".into()));}bytes.extend_from_slice(&chunk);},
        Ok(None)=>break,
        Err(_)=>return (StatusCode::BAD_GATEWAY.into_response(),None,Some("Standalone tool response body interrupted".into())),
    }}
    let value:Value=match serde_json::from_slice(&bytes) {Ok(v)=>v,Err(_)=>return (StatusCode::BAD_GATEWAY.into_response(),None,Some("Invalid standalone tool JSON response".into()))};
    match crate::search_tools::response_units(kind,status.as_u16(),&value) {
        Ok(units)=>{let response=Response::builder().status(status).header(header::CONTENT_TYPE,"application/json").body(Body::from(bytes)).unwrap_or_else(|_|StatusCode::BAD_GATEWAY.into_response());(response,Some(units as i64),None)},
        Err(note)=>(StatusCode::BAD_GATEWAY.into_response(),None,Some(note)),
    }
}

/// 重排：整段读下来再回给客户端；上游 `document` 为 null 时按 index 回填请求里的原文。
async fn buffer_rerank_passthrough(
    resp: reqwest::Response,
    status: StatusCode,
    req_body: &Value,
    contract: Option<&crate::vector_compat::Contract>,
) -> (Response, i64, i64, Option<String>, Usage) {
    let ctype = content_type(&resp);
    let bytes = match if contract.is_some() { read_vector_body(resp).await }
        else { resp.bytes().await.map_err(|e| if e.is_timeout() { "等待非流式完成超时（读取正文）".into() } else { format!("upstream body read failed: {e}") }) } {
        Ok(bytes) => bytes,
        Err(note) => return invalid_buffer(note),
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
    let mut usage=Usage::read(&parsed);
    if let Some(contract) = contract {
        if let Err(note) = crate::vector_compat::validate_output(contract, tokengo_shared::ModelKind::Rerank, req_body, &parsed) {
            let mut invalid = invalid_buffer(note); invalid.4 = usage; return invalid;
        }
    }
    if let Err(note) = crate::upstream::validate_response(&parsed, "rerank") {
        let mut invalid=invalid_buffer(note); invalid.4=usage; return invalid;
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
    if let Err(note) = crate::upstream::normalize_rerank_response(&mut parsed) {
        let mut invalid = invalid_buffer(note); invalid.4 = usage; return invalid;
    }
    // Rerank has no generated tokens: an explicit total-only meter is input.
    // This fallback is endpoint-specific; Chat total_tokens includes output too.
    if usage.input.is_none() {
        usage.input = parsed.get("usage").and_then(|u| u.get("total_tokens"))
            .and_then(Value::as_i64).filter(|n| *n >= 0);
    }
    if usage.input.is_some() && usage.output.is_none() { usage.output = Some(0); }
    let out_bytes = serde_json::to_vec(&parsed).unwrap_or_else(|_| bytes.to_vec());
    let (input, output) = (usage.input.unwrap_or(0), usage.output.unwrap_or(0));
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
        usage,
    )
}

/// Bounded precommit preserves source bytes and observes protocol semantics before headers are sent.
struct PreparedStream {
    observer: crate::protocol::sse::Observer,
    transcoder: Option<crate::protocol::sse::Transcoder>,
    prefix: Bytes,
    eof: bool,
    artifacts:crate::protocol_sessions::ArtifactCollector,
}
async fn prepare_stream(resp:&mut reqwest::Response,source:crate::protocol::Protocol,client:crate::protocol::Protocol,
    remaining:Duration,idle:Duration,guard:&mut RequestGuard)->Result<PreparedStream,String> {
    let mut prepared=PreparedStream {observer:crate::protocol::sse::Observer::new(source),
        transcoder:(source!=client).then(||crate::protocol::sse::Transcoder::new(source,client)),prefix:Bytes::new(),eof:false,artifacts:Default::default()};
    let deadline=tokio::time::Instant::now()+remaining;
    let mut prefix=Vec::new();
    loop {
        let wait=deadline.saturating_duration_since(tokio::time::Instant::now()).min(idle);
        let chunk=tokio::time::timeout(wait,resp.chunk()).await
            .map_err(|_|"precommit deadline exceeded before valid output".to_string())?
            .map_err(|_|"precommit upstream body read failed".to_string())?;
        match chunk {
            Some(chunk)=>{
                let observed=prepared.observer.feed(&chunk);
                guard.usage=Usage{input:prepared.observer.input,output:prepared.observer.output};
                let frames=observed.map_err(|note|format!("precommit protocol error: {note}"))?;
                prepared.artifacts.frames(&frames).map_err(|note|format!("precommit artifact error: {note}"))?;
                if let Some(note)=&prepared.observer.failed {return Err(format!("precommit upstream error: {note}"));}
                let out=match prepared.transcoder.as_mut() {Some(t)=>t.feed(&chunk).map_err(|note|{let classified=conversion_fault(note);if is_conversion_fault(&classified){classified}else{format!("precommit conversion error: {classified}")}})?,None=>chunk.to_vec()};
                if prefix.len().saturating_add(out.len())>1<<20 {return Err("precommit prefix exceeds 1 MiB".into());}
                prefix.extend_from_slice(&out);
                if prepared.observer.done {
                    if !prepared.observer.meaningful {return Err("precommit stream completed without output".into());}
                    prepared.eof=true;
                }
                if prepared.observer.meaningful || prepared.observer.done {break;}
            },
            None=>{
                let ended=prepared.observer.finish_frames();
                guard.usage=Usage{input:prepared.observer.input,output:prepared.observer.output};
                let frames=ended.map_err(|note|format!("precommit incomplete stream: {note}"))?;
                prepared.artifacts.frames(&frames).map_err(|note|format!("precommit artifact error: {note}"))?;
                if let Some(t)=prepared.transcoder.as_mut() {prefix.extend_from_slice(&t.finish().map_err(|note|{let classified=conversion_fault(note);if is_conversion_fault(&classified){classified}else{format!("precommit conversion error: {classified}")}})?);}
                prepared.eof=true;
                if prepared.observer.failed.is_some() || !prepared.observer.meaningful {return Err("precommit empty or failed stream".into());}
                break;
            }
        }
    }
    prepared.prefix=Bytes::from(prefix);
    Ok(prepared)
}

#[allow(clippy::too_many_arguments)]
fn stream_passthrough(
    state: SharedState,
    resp: reqwest::Response,
    prepared: PreparedStream,
    provenance: Option<(String, crate::protocol_sessions::Pin)>,
    guard: RequestGuard,
    request_id: i64,
    started_ms: i64,
    channel_id: i64,
    gate: GatewayPermits,
    endpoint: EndpointKind,
    ordinal: u32,
    model_name: String,
    upstream_model: String,
    attempt_started: i64,
    adaptation_note: Option<String>,
    cooldown: CooldownTicket,
    load_ticket: crate::state::ModelLoadTicket,
    idle_timeout: Duration,
) -> Response {
    let status = resp.status();
    let st = StreamState {
        request_id,
        ordinal,
        model_name,
        upstream_model,
        attempt_started,
        adaptation_note,
        cooldown,
        _model_load: load_ticket,
        resp,
        observer: prepared.observer,
        transcoder: prepared.transcoder,
        prefix: Some(prepared.prefix),
        eof: prepared.eof,
        artifacts: prepared.artifacts,
        provenance,
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
        if st.guard.is_none() {
            return None;
        }
        if let Some(prefix) = st.prefix.take().filter(|p| !p.is_empty()) {
            st.first_byte_ms = Some(now_ms() - st.started_ms);
            st.delivered += prefix.len();
            if let Some(g) = st.guard.as_mut() {
                g.streamed = true;
            }
            return Some((Ok::<Bytes, std::io::Error>(prefix), st));
        }
        if st.eof {
            st.finish(None);
            return None;
        }
        match tokio::time::timeout(st.idle_timeout, st.resp.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let observed = st.observer.feed(&chunk);
                if let Some(g) = st.guard.as_mut() {
                    g.usage = Usage {
                        input: st.observer.input,
                        output: st.observer.output,
                    };
                    g.streamed = true;
                }
                let out = observed
                    .and_then(|frames| {
                        st.artifacts.frames(&frames)?;
                        if let Some((owner, pin)) = &st.provenance {
                            st.artifacts
                                .persist(&st.state.conn(), owner, pin)
                                .map_err(|_| {
                                    "local_state: opaque artifact ownership persistence failed"
                                        .to_string()
                                })?;
                        }
                        Ok(())
                    })
                    .and_then(|_| match st.transcoder.as_mut() {
                        Some(t) => t.feed(&chunk).map_err(conversion_fault),
                        None => Ok(chunk.to_vec()),
                    });
                match out {
                    Ok(out) => {
                        st.delivered += out.len();
                        if st.observer.done {
                            st.eof = true;
                        }
                        Some((Ok(Bytes::from(out)), st))
                    }
                    Err(note) => {
                        let out = if st.transcoder.is_none() && st.observer.failed.is_some() {
                            chunk
                        } else {
                            stream_error(st.endpoint.protocol(), &note)
                        };
                        st.finish(Some(&note));
                        Some((Ok(out), st))
                    }
                }
            }
            Ok(Ok(None)) => {
                let ended = st.observer.finish_frames();
                if let Some(g) = st.guard.as_mut() {
                    g.usage = Usage {
                        input: st.observer.input,
                        output: st.observer.output,
                    };
                }
                if let Ok(frames) = ended {
                    if let Err(note) = st.artifacts.frames(&frames).and_then(|_| {
                        if let Some((owner, pin)) = &st.provenance {
                            st.artifacts
                                .persist(&st.state.conn(), owner, pin)
                                .map_err(|_| {
                                    "local_state: opaque artifact ownership persistence failed"
                                        .to_string()
                                })?;
                        }
                        Ok(())
                    }) {
                        st.finish(Some(&note));
                        return Some((Ok(stream_error(st.endpoint.protocol(), &note)), st));
                    }
                }
                let tail = match st.transcoder.as_mut() {
                    Some(t) => t.finish().map_err(conversion_fault),
                    None => Ok(vec![]),
                };
                match tail {
                    Ok(tail) if !tail.is_empty() => {
                        st.eof = true;
                        Some((Ok(Bytes::from(tail)), st))
                    }
                    Ok(_) => {
                        st.finish(None);
                        None
                    }
                    Err(note) => {
                        st.finish(Some(&note));
                        if is_conversion_fault(&note) {
                            Some((Ok(stream_error(st.endpoint.protocol(), &note)), st))
                        } else {
                            Some((Err(std::io::Error::other(note)), st))
                        }
                    }
                }
            }
            Ok(Err(_)) => {
                let note = "上游流式连接异常中断；已输出内容不重试";
                st.finish(Some(note));
                Some((Err(std::io::Error::other(note)), st))
            }
            Err(_) => {
                let note = "上游流式空闲超时；已输出内容不重试";
                st.finish(Some(note));
                Some((
                    Err(std::io::Error::new(std::io::ErrorKind::TimedOut, note)),
                    st,
                ))
            }
        }
    });
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn stream_error(protocol:Option<crate::protocol::Protocol>,note:&str)->Bytes {
    let protocol_fault=is_conversion_fault(note);
    let code=if protocol_fault {"protocol_not_supported"}else{"upstream_error"};
    let message=if protocol_fault {format!("Response conversion is unsupported after output: {note}")}else{format!("Upstream stream failed after output: {note}")};
    let (event,data)=match protocol {
        Some(crate::protocol::Protocol::Messages)=>(Some("error"),serde_json::json!({"type":"error","error":{"type":"api_error","message":message}})),
        Some(crate::protocol::Protocol::Responses)=>(Some("error"),serde_json::json!({"type":"error","code":code,"message":message})),
        _=>(None,serde_json::json!({"error":{"type":"upstream_error","message":message}})),
    };
    Bytes::from(crate::protocol::sse::Frame{event:event.map(str::to_string),data:data.to_string()}.encode())
}

struct StreamState {
    _model_load: crate::state::ModelLoadTicket,
    cooldown: CooldownTicket,
    request_id: i64,
    ordinal: u32,
    model_name: String,
    upstream_model: String,
    attempt_started: i64,
    adaptation_note: Option<String>,
    first_byte_ms: Option<i64>,
    endpoint: EndpointKind,
    resp: reqwest::Response,
    observer: crate::protocol::sse::Observer,
    transcoder: Option<crate::protocol::sse::Transcoder>,
    prefix: Option<Bytes>,
    eof: bool,
    artifacts: crate::protocol_sessions::ArtifactCollector,
    provenance: Option<(String, crate::protocol_sessions::Pin)>,
    guard: Option<RequestGuard>,
    started_ms: i64,
    channel_id: i64,
    state: SharedState,
    delivered: usize,
    idle_timeout: Duration,
    /// 这条渠道的并发名额。**流式期间一直握着**：不然名额一还，
    /// 明明还在往外吐字的那条流就不算数了，上游照样被打爆。
    _gate: GatewayPermits,
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
        let finish_error=if transport_error.is_none(){self.observer.finish().err().map(|e|format!("上游流式缺少结束标记或协议失败：{e}"))}else{None};
        let transport_error=transport_error.or(finish_error.as_deref());
        let (input,output)=(self.observer.input.unwrap_or(0),self.observer.output.unwrap_or(0));
        guard.active_attempt=None;
        guard.usage=Usage{input:self.observer.input,output:self.observer.output};
        let ended=self.observer.done;
        let complete=transport_error.is_none() && ended && self.observer.failed.is_none();
        let empty=complete && !self.observer.meaningful;
        let note = if let Some(error) = transport_error {
            Some(error)
        } else if self.observer.failed.is_some() {
            Some("上游流式返回错误事件，已发出去的内容收不回来")
        } else if !ended {
            Some("上游流式缺少结束标记，回答可能未完成；已输出内容不重试")
        } else if empty {
            Some("上游流里一个字都没说（没 content、没 tool_calls、没推理内容）")
        } else {
            None
        };
        let local_fault=note.is_some_and(|note|note.starts_with("local_state:"));
        let protocol_fault=note.is_some_and(is_conversion_fault);
        if protocol_fault {guard.error_code=Some("protocol_not_supported".into());guard.settle_billing("protocol_fault");}
        if local_fault {guard.error_code=Some("state_unavailable".into());}
        else if !protocol_fault {self.cooldown.finish(complete && !empty,note.unwrap_or(""),0);}
        let args=requests::AttemptArgs{request_id:self.request_id,ordinal:self.ordinal,channel_id:self.channel_id,model_name:self.model_name.clone(),upstream_model:self.upstream_model.clone(),outcome:if local_fault {"local_fault"}else if protocol_fault {"protocol_fault"}else if complete && !empty {"success"}else{"channel_fault"}.into(),http_status:None,duration_ms:now_ms()-self.attempt_started,error:note.map(str::to_string).or_else(||self.adaptation_note.clone()),first_byte_ms:self.first_byte_ms.map(|ms| (ms+self.started_ms-self.attempt_started).max(0))};
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

const MAX_ERROR_BODY: usize = 64 * 1024;

/// Bound error reads before committing a client response; preserve complete errors verbatim.
async fn read_error(
    mut resp: reqwest::Response,
    remaining: Duration,
    idle_timeout: Duration,
) -> Result<UpstreamError, String> {
    if remaining.is_zero() {
        return Err("upstream error body deadline exceeded".into());
    }
    let status = resp.status();
    let ctype = content_type(&resp);
    let retry_after = resp.headers().get(header::RETRY_AFTER).cloned();
    // Keep both limits: chunk activity cannot extend the original attempt or
    // remaining request budget, and a stalled body cannot hold channel capacity.
    let body = tokio::time::timeout(remaining, async {
        let mut body = Vec::new();
        loop {
            let chunk = tokio::time::timeout(idle_timeout, resp.chunk()).await
                .map_err(|_| "upstream error body idle timeout".to_string())?
                .map_err(|_| "upstream error body read failed".to_string())?;
            let Some(chunk) = chunk else { break; };
            if chunk.len() > MAX_ERROR_BODY - body.len() {
                return Err(format!("upstream error body exceeds {MAX_ERROR_BODY} bytes"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Bytes::from(body))
    }).await.map_err(|_| "upstream error body deadline exceeded".to_string())??;
    let text = String::from_utf8_lossy(&body)
        .chars()
        .take(400)
        .collect::<String>();
    Ok(UpstreamError {
        status,
        ctype,
        retry_after,
        body,
        text,
    })
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
    usage: Usage,
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
            self.usage.merge(Usage::read(&v));
            self.input=self.usage.input.unwrap_or(0);
            self.output=self.usage.output.unwrap_or(0);
        }
    }

    #[cfg(test)]
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
    let code = if http == 503 { "models_cooling" }
        else if http == 422 { "vision_not_ready" }
        else if model_routes::is_auto(&req.model) { "auto_no_candidate" }
        else { "model_not_configured" };
    record_routing_refusal_with_code(state, req, started_ms, http, code, msg);
}

/// Compatibility denials preserve the original API error and safe payload dimensions.
/// There was no upstream attempt: never mark a source unhealthy or write an attempt row.
fn record_api_routing_refusal(
    state: &SharedState,
    req: &ForwardRequest,
    started_ms: i64,
    error: &ApiError,
) {
    let message = if matches!(req.endpoint, EndpointKind::Embeddings | EndpointKind::Rerank) {
        format!("{}; request_shape={}", error.message, vector_request_shape(&req.body))
    } else {
        error.message.clone()
    };
    record_routing_refusal_with_code(
        state, req, started_ms, error.status.as_u16(),
        error.code.unwrap_or(error.kind), &message,
    );
}

/// Only numeric request statistics are persisted; never text, parameters, or credentials.
fn vector_request_shape(body: &Value) -> Value {
    let texts: Vec<&str> = if let Some(text) = body.get("input").and_then(Value::as_str) {
        vec![text]
    } else {
        body.get("input").and_then(Value::as_array).into_iter().flatten()
            .filter_map(Value::as_str).collect()
    };
    let documents = body.get("documents").and_then(Value::as_array);
    let document_chars = |v: &Value| v.as_str().or_else(|| v.get("text").and_then(Value::as_str))
        .map_or(0, |text| text.chars().count());
    let query_chars = body.get("query").and_then(Value::as_str).map_or(0, |text| text.chars().count());
    let input_chars: usize = texts.iter().map(|text| text.chars().count()).sum();
    let documents_chars: usize = documents.into_iter().flatten().map(document_chars).sum();
    serde_json::json!({
        "json_bytes": serde_json::to_vec(body).map_or(0, |bytes| bytes.len()),
        "input_count": texts.len(),
        "input_max_chars": texts.iter().map(|text| text.chars().count()).max().unwrap_or(0),
        "input_total_chars": input_chars,
        "query_chars": query_chars,
        "documents_count": documents.map_or(0, Vec::len),
        "document_max_chars": documents.into_iter().flatten().map(document_chars).max().unwrap_or(0),
        "documents_total_chars": documents_chars,
        "total_chars": input_chars + query_chars + documents_chars,
        "top_n": body.get("top_n").and_then(Value::as_u64),
    })
}

fn record_routing_refusal_with_code(
    state: &SharedState,
    req: &ForwardRequest,
    started_ms: i64,
    http: u16,
    code: &str,
    msg: &str,
) {
    let has_image = req.endpoint.has_image(&req.body);
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
    if let Err(e)=guard.identify(req.auth.as_ref()) { tracing::error!("refusal identity write failed: {e:?}"); }
    guard.error_code = Some(code.into());

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
    admission: Option<crate::admission::AdmissionPermit>,
    reservation: Option<i64>,
    billing_nonchat: bool,
    request_billing: bool,
    request_units: Option<i64>,
    usage: Usage,
    active_attempt:Option<(requests::AttemptArgs,i64)>,
    streamed: bool,
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
            admission: None,
            reservation: None,
            billing_nonchat: false,
            request_billing: false,
            request_units: None,
            usage: Usage::default(),
            active_attempt:None,
            streamed: false,
            channel_id: None,
            served_model: None,
            error_code:None,
        }
    }

    fn reject(&mut self,e:&ApiError) {
        self.error_code=Some(e.kind.into());
        let state=self.state.clone();
        self.finish(&state,Status::ClientError,Some(e.status.as_u16()),None,self.started_ms,None,0,0,false,Some(&e.message));
    }
    fn identify(&self, auth: Option<&crate::db::identity::AuthContext>) -> Result<(),ApiError> {
        if let Some(auth) = auth { self.state.conn().execute("UPDATE requests SET user_id=?2,key_id=?3,auth_snapshot=?4 WHERE id=?1",rusqlite::params![self.request_id,auth.user_id,auth.key_id,serde_json::to_string(auth).map_err(|e|ApiError::internal(e.to_string()))?]).map_err(crate::db::DbError::from)?; }
        Ok(())
    }
    fn settle_billing(&mut self, outcome: &str) {
        if let Some(id)=self.reservation.take() {
            let result=if self.request_billing {crate::billing_writer::settle_request(&self.state,id,self.request_units,outcome)}else{crate::billing_writer::settle(&self.state,id,self.usage.input,self.usage.output.or_else(||self.billing_nonchat.then_some(0)),outcome)};
            if let Err(e)=result {
                // The durable hold remains reserved; it cannot be spent twice. Startup reconciles it.
                tracing::error!(reservation_id=id,"Billing settlement pending reconciliation: {e}");
            }
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
        if let Some((mut attempt,started))=self.active_attempt.take() {
            attempt.duration_ms=(now_ms()-started).max(0);attempt.http_status=http_status;
            attempt.outcome=match status {Status::Ok=>"success",Status::Canceled=>"canceled",Status::Failed=>"channel_fault",Status::ClientError=>"client_error"}.into();
            attempt.error=error.map(str::to_string);
            state.enqueue_write(crate::state::WriteJob::Attempt(attempt));
        }
        self.settle_billing(status.as_str());
        self.admission.take();
        self.done = true;
        if matches!(status, Status::ClientError) && self.error_code.is_none() {
            self.error_code=Some(if http_status==Some(422) && error.map(|s|s.contains("图片")).unwrap_or(false) {"vision_not_ready"} else {"request_invalid"}.into());
        }
        // 记账不许把请求带崩：抢不到库锁就丢给后台落账员（2026-10-18 压测踩出来的规矩）
        let args = requests::FinishArgs {
            input_tokens_known: self.usage.input.is_some(),
            output_tokens_known: self.usage.output.is_some(),
            usage_source: self.usage.source().into(),
            request_id: self.request_id,
            status,
            http_status,
            channel_id: channel_id.or(self.channel_id),
            latency_ms: now_ms() - started_ms,
            first_byte_ms,
            input_tokens: self.usage.input.unwrap_or(input_tokens),
            output_tokens: self.usage.output.unwrap_or(output_tokens),
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
        if let Some((mut attempt,started))=self.active_attempt.take() {
            attempt.duration_ms=(now_ms()-started).max(0);
            self.state.enqueue_write(crate::state::WriteJob::Attempt(attempt));
        }
        self.settle_billing("canceled");
        self.admission.take();
        self.done = true;
        // 请求被中断（客户端断开等）也要记上一条 canceled —— 以前这里抢不到库锁就整条丢掉，
        // 面板上那一发就永远停在「进行中」。现在抢不到就丢给后台落账员补写。
        let args = requests::FinishArgs {
            input_tokens_known: self.usage.input.is_some(),
            output_tokens_known: self.usage.output.is_some(),
            usage_source: self.usage.source().into(),
            request_id: self.request_id,
            status: Status::Canceled,
            http_status: None,
            channel_id: self.channel_id,
            latency_ms: now_ms() - self.started_ms,
            first_byte_ms: None,
            input_tokens: self.usage.input.unwrap_or(0),
            output_tokens: self.usage.output.unwrap_or(0),
            streamed: self.streamed,
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
    fn native_output_caps_cannot_bypass_generation_quota() {
        let mut response=serde_json::json!({"model":"m","input":"hello"});
        let (_,output)=billing_bound(&mut response,EndpointKind::Responses,None).unwrap();
        assert_eq!(output,4096);assert_eq!(response["max_output_tokens"],4096);
        assert_eq!(output_bound(&serde_json::json!({"max_output_tokens":37}),EndpointKind::Responses).unwrap(),37);
        assert_eq!(output_bound(&serde_json::json!({"max_tokens":19}),EndpointKind::Messages).unwrap(),19);
        assert!(output_bound(&serde_json::json!({"max_tokens":19}),EndpointKind::Responses).is_err());
        assert!(output_bound(&serde_json::json!({"max_output_tokens":-1}),EndpointKind::Responses).is_err());
        assert!(output_bound(&serde_json::json!({"max_output_tokens":0}),EndpointKind::Responses).is_err());
    }
    #[test]
    fn input_additional_tools_cannot_hide_hosted_tools_or_provider_tool_search() {
        for tool in [serde_json::json!({"type":"web_search"}),serde_json::json!({"type":"namespace","name":"hidden","tools":[{"type":"file_search"}]})] {
            let body=serde_json::json!({"input":[{"type":"additional_tools","role":"developer","tools":[tool]}]});assert!(validate_native_cost_and_state(EndpointKind::Responses,&body).is_err());
        }
        let body=serde_json::json!({"input":[{"type":"additional_tools","role":"developer","tools":[{"type":"function","name":"sum","parameters":{"type":"object"}}]}]});assert!(validate_native_cost_and_state(EndpointKind::Responses,&body).is_ok());
        assert!(validate_native_cost_and_state(EndpointKind::Responses,&serde_json::json!({"input":[{"type":"tool_search_output","tools":[]}]})).is_err());
    }
    #[test]
    fn subscription_context_bounds_do_not_invent_a_token_price() {
        let mut body=serde_json::json!({"model":"m","input":[{"type":"reasoning","encrypted_content":"synthetic-opaque"}],"max_output_tokens":31});
        assert!(billing_bound(&mut body,EndpointKind::Responses,None).is_err());
        assert_eq!(billing_bound_with_context(&mut body,EndpointKind::Responses,None,Some((131072,4096))).unwrap(),(131072,31));
        assert_eq!(body["max_output_tokens"],31);
        assert!(billing_bound_with_context(&mut body,EndpointKind::Responses,None,Some((131072,30))).is_err());
    }
    #[test]
    fn declared_source_context_cannot_be_underreserved_by_smaller_price_cap() {
        let price=crate::db::billing::Price{id:1,channel_id:1,model:"m".into(),input_micro_per_million:1,output_micro_per_million:1,max_input_tokens:512,max_output_tokens:4096};
        let mut body=json!({"model":"m","input":[{"type":"reasoning","encrypted_content":"opaque"}],"max_output_tokens":16});
        assert!(billing_bound_with_context(&mut body,EndpointKind::Responses,Some(&price),Some((4096,4096))).is_err(),"fullsource context4096 must not be replaced by price cap512");
    }
    #[test]
    fn declared_generic_source_context_accounts_for_audio_and_images_without_price() {
        let mut audio=json!({"max_tokens":16,"messages":[{"role":"user","content":[{"type":"input_audio","input_audio":{"data":"YQ==","format":"wav"}}]}]});
        assert_eq!(billing_bound_with_context(&mut audio,EndpointKind::Chat,None,Some((4096,64))).unwrap(),(4096,16));
        let mut image=json!({"max_tokens":16,"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.invalid/image.png"}}]}]});
        assert_eq!(billing_bound_with_context(&mut image,EndpointKind::Chat,None,Some((4096,64))).unwrap(),(4096,16));
        let mut text=json!({"messages":[{"role":"user","content":"hello"}]});
        let (_,out)=billing_bound_with_context(&mut text,EndpointKind::Chat,None,Some((4096,64))).unwrap();
        assert_eq!(out,64);assert_eq!(text["max_tokens"],64);
    }
    #[test]
    fn native_usage_includes_cached_input_and_nested_final_response() {
        assert_eq!(usage_from_value(&serde_json::json!({"usage":{"input_tokens":3,"output_tokens":0,"cache_read_input_tokens":5,"cache_creation_input_tokens":7}})),(15,0));
        assert_eq!(usage_from_value(&serde_json::json!({"response":{"usage":{"input_tokens":8,"output_tokens":4}}})),(8,4));
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
#[tokio::test]
    async fn stream_only_responses_collect_json_without_waiting_for_eof_and_keep_failure_usage() {
        use axum::routing::post;
        let item=serde_json::json!({"type":"message","id":"msg_test","status":"completed","role":"assistant","content":[{"type":"output_text","text":"collected","annotations":[]}]});
        let completed=format!("data: {}\n\ndata: {}\n\n",serde_json::json!({"type":"response.output_item.done","output_index":0,"item":item}),serde_json::json!({"type":"response.completed","response":{"id":"resp_test","object":"response","status":"completed","output":[],"usage":{"input_tokens":7,"output_tokens":3}}}));
        let failed=format!("data: {}\n\n",serde_json::json!({"type":"response.failed","response":{"status":"failed","usage":{"input_tokens":9,"output_tokens":2}}}));
        for (wire,expected_ok,expected_input) in [(completed,true,7),(failed,false,9)] {
            let app=axum::Router::new().route("/responses",post(move || {let wire=wire.clone();async move {
                let stream=futures_util::stream::unfold(Some(wire),|chunk|async move {match chunk {Some(chunk)=>Some((Ok::<_,std::convert::Infallible>(Bytes::from(chunk)),None)),None=>{std::future::pending::<()>().await;None}}});
                ([("content-type","text/event-stream")],Body::from_stream(stream))
            }}));
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
            let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
            let response=reqwest::Client::new().post(format!("http://{address}/responses")).send().await.unwrap();
            let dir=std::env::temp_dir().join(format!("tokengo-oauth-collect-{}",uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let state=crate::AppState::init(crate::Config{db_path:dir.join("state.db"),master_key_path:dir.join("master.key").to_string_lossy().into_owned(),..Default::default()}).unwrap();
            let id=requests::start(&state.conn(),"responses","test",None,false).unwrap();
            let mut guard=RequestGuard::new(state.clone(),id,now_ms());
            let (out,input,output,error,usage)=tokio::time::timeout(Duration::from_secs(1),buffer_responses_stream(response,crate::protocol::Protocol::Chat,None,Duration::from_millis(300),Duration::from_millis(100),&mut guard)).await.unwrap();
            assert_eq!(usage.input,Some(expected_input));assert_eq!(guard.usage.input,Some(expected_input));
            if expected_ok {
                assert!(error.is_none());assert_eq!((input,output),(7,3));
                let bytes=axum::body::to_bytes(out.into_body(),65536).await.unwrap();let value:Value=serde_json::from_slice(&bytes).unwrap();
                assert_eq!(value["choices"][0]["message"]["content"],"collected");assert_eq!(value["usage"]["prompt_tokens"],7);
            } else {assert!(error.is_some());assert_eq!(out.status(),StatusCode::BAD_GATEWAY);assert_eq!(usage.output,Some(2));}
            drop(guard);server.abort();std::fs::remove_dir_all(dir).ok();
        }
    }
