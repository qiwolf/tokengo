//! 管理面：面板用的接口。只做面板真需要的，不做上游那 24 个管理路由。
//!
//! 鉴权：登录后拿到一个会话 token，之后每个请求带
//! `Authorization: Bearer <token>` 或 Cookie `tokengo_session=<token>`。
//! 除 `ping` / `auth/*` 外，**所有接口都要过会话门**。

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;

use super::{client_ip, unbuilt, ApiError};
use crate::db::{groups, channels, maintenance, requests, routing, sessions, settings, now_ms};
use crate::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        // —— 无鉴权 ——
        .route("/ping", get(ping))
        .route("/auth/status", get(auth_status))
        .route("/auth/setup", post(auth_setup))
        .route("/auth/login", post(auth_login))
        .route("/auth/logout", post(auth_logout))
        // —— 需要登录 ——
        .route("/auth/password", put(auth_change_password))
        .route("/channels", get(channels_list).post(channels_create))
        .route("/channels/{id}", put(channels_update).delete(channels_delete))
        .route("/channels/{id}/probe", post(channels_probe))
        .route("/channels/{id}/models", post(channels_models))
        .route("/channels/{id}/vision-test", post(channels_vision_test))
        .route("/groups", get(groups_list).post(groups_upsert))
        .route("/groups/{name}", delete(groups_delete))
        .route("/groups/{name}/kind", put(groups_set_kind))
        .route("/routing", get(routing_state))
        .route("/routing/config", put(routing_set_config))
        .route("/routing/{group}/reprobe", post(routing_reprobe))
        .route("/stats", get(stats))
        .route("/logs", get(logs))
        .route("/timeline", get(|| async { unbuilt(5, "GET /api/timeline") }))
        .route("/settings", get(settings_get).put(settings_put))
        .route("/settings/generate-gateway-key", post(generate_gateway_key))
        .route("/maintenance", get(maintenance_get).post(maintenance_set))
        .route("/maintenance/prune", post(maintenance_prune))
}

// ───────────────────────── 鉴权 ─────────────────────────

/// 会话门。放在 handler 参数里即生效。
pub struct PanelAuth;

impl FromRequestParts<SharedState> for PanelAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(parts)
            .or_else(|| cookie_token(parts))
            .ok_or_else(|| ApiError::unauthorized("未登录"))?;
        let ok = sessions::check_session(&state.conn(), &token).unwrap_or(false);
        if !ok {
            return Err(ApiError::unauthorized("会话已失效，请重新登录"));
        }
        Ok(PanelAuth)
    }
}

fn bearer_token(parts: &Parts) -> Option<String> {
    let raw = parts.headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let t = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn cookie_token(parts: &Parts) -> Option<String> {
    let raw = parts.headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for kv in raw.split(';') {
        if let Some(v) = kv.trim().strip_prefix("tokengo_session=") {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

// ───────────────────────── 基础 ─────────────────────────

async fn ping() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "brand": tokengo_shared::BRAND,
        "version": tokengo_shared::VERSION,
    }))
}

// ───────────────────────── 登录 ─────────────────────────

async fn auth_status(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let has_password = sessions::has_panel_password(&conn)?;
    let gateway_key_set =
        settings::get(&conn, settings::GATEWAY_KEY_HASH)?.is_some();
    Ok(Json(serde_json::json!({
        // 没设过口令 = 首次使用，面板该弹"设置口令"而不是"登录"
        "needs_setup": !has_password,
        "has_password": has_password,
        "gateway_key_set": gateway_key_set,
        "brand": tokengo_shared::BRAND,
        "version": tokengo_shared::VERSION,
    })))
}

#[derive(Deserialize)]
struct SetupBody {
    password: String,
    /// 首次设置时一并设的对外 key；不传就自动生成一个返回给用户。
    gateway_key: Option<String>,
}

/// 首次设置：只在**还没设过口令**时可用。设完直接算登录。
async fn auth_setup(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<SetupBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    if sessions::has_panel_password(&conn)? {
        return Err(ApiError::bad_request("口令已设置过，请走登录"));
    }
    sessions::set_panel_password(&conn, &body.password)?;

    // 用户偏好：沿用惯用的短口令，不保留自动生成的长随机串。没传才自动生成。
    let (key, generated) = match body.gateway_key.as_deref() {
        Some(k) if !k.trim().is_empty() => (k.trim().to_string(), false),
        _ => (format!("tk-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]), true),
    };
    sessions::set_gateway_key(&conn, &key)?;
    // 换 key 必须把校验快路径忘掉，否则旧 key 还能用最多一个缓存周期
    state.forget_gateway_key();
    sessions::purge_expired(&conn)?;
    let token = sessions::create_session(&conn, client_ip(&headers).as_deref())?;
    settings::set(&conn, "panel_password_set_at_ms", &now_ms().to_string())?;

    tracing::info!("面板已初始化，口令与对外 key 已设置");
    Ok(Json(serde_json::json!({
        "ok": true,
        "token": token,
        // 只在这一次回答里回显明文 key（用户得靠它去配客户端），之后库里只有哈希
        "gateway_key": key,
        "gateway_key_generated": generated,
        "notice": "对外 key 只在这里回显这一次，请现在记下；库里只存哈希，之后拿不回明文。",
    })))
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn auth_login(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<LoginBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    if !sessions::has_panel_password(&conn)? {
        return Err(ApiError::bad_request("还没设置口令，请先初始化"));
    }
    if !sessions::check_panel_password(&conn, &body.password)? {
        tracing::warn!("面板登录失败：口令不对");
        return Err(ApiError::unauthorized("口令不对"));
    }
    sessions::purge_expired(&conn)?;
    let token = sessions::create_session(&conn, client_ip(&headers).as_deref())?;
    Ok(Json(serde_json::json!({ "ok": true, "token": token })))
}

#[derive(Deserialize)]
struct TokenBody {
    token: Option<String>,
}

async fn auth_logout(
    State(state): State<SharedState>,
    Json(body): Json<TokenBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(t) = body.token {
        sessions::revoke_session(&state.conn(), &t)?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ChangePasswordBody {
    old_password: String,
    new_password: String,
}

async fn auth_change_password(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(body): Json<ChangePasswordBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    if !sessions::check_panel_password(&conn, &body.old_password)? {
        return Err(ApiError::unauthorized("原口令不对"));
    }
    sessions::set_panel_password(&conn, &body.new_password)?;
    // 改口令会踢掉所有会话，包括自己，所以这里重新发一个
    let token = sessions::create_session(&conn, None)?;
    Ok(Json(serde_json::json!({ "ok": true, "token": token })))
}

// ───────────────────────── 渠道 ─────────────────────────

fn channel_json(l: &channels::ChannelRow) -> serde_json::Value {
    serde_json::json!({
        "id": l.id.0,
        "name": l.name,
        "base_url": l.base_url,
        "enabled": l.enabled,
        "has_secret": l.has_secret,
        "unwrap_envelope": l.unwrap_envelope,
        "headers": l.headers.iter().map(|(n, v)| serde_json::json!({"name": n, "value": v})).collect::<Vec<_>>(),
        "models": l.models.iter().map(|m| serde_json::json!({
            "model": m.model,
            "upstream": m.upstream,
            "vision": m.vision,
            "drop_fields": m.drop_fields_text(),
        })).collect::<Vec<_>>(),
        // 这条渠道上「能看图」的模型名有几个 —— 渠道列表里直接显示，不用展开也知道。
        "vision_count": l.models.iter().filter(|m| m.vision).count(),
    })
}

async fn channels_list(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let all = channels::list(&conn)?;
    Ok(Json(serde_json::json!({
        "channels": all.iter().map(channel_json).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct HeaderIn {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct ModelIn {
    model: String,
    upstream: Option<String>,
    /// 这个模型名在这条渠道上能不能吃图片输入（面板上的「能看图」勾选）。
    #[serde(default)]
    vision: bool,
    /// 发上游前要丢掉的请求体顶层字段，逗号分隔（如 `reasoning_effort`）。
    #[serde(default)]
    drop_fields: String,
}

#[derive(Deserialize)]
struct ChannelBody {
    name: String,
    base_url: String,
    #[serde(default = "default_true")]
    enabled: bool,
    /// 非流式响应外面套了一层壳时自动剥掉（面板上的「响应解包」勾选）。
    #[serde(default)]
    unwrap_envelope: bool,
    /// 明文密钥。留空表示不改动已存的（新建时留空 = 没密钥）。
    key: Option<String>,
    #[serde(default)]
    headers: Vec<HeaderIn>,
    #[serde(default)]
    models: Vec<ModelIn>,
}

fn default_true() -> bool {
    true
}

impl ChannelBody {
    fn headers(&self) -> Vec<(String, String)> {
        self.headers
            .iter()
            .filter(|h| !h.name.trim().is_empty())
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect()
    }

    fn models(&self) -> Vec<channels::ModelMapping> {
        self.models
            .iter()
            .filter(|m| !m.model.trim().is_empty())
            .map(|m| {
                let mut mapping = channels::ModelMapping::new(
                    m.model.clone(),
                    m.upstream.clone().filter(|u| !u.trim().is_empty()),
                    m.vision,
                );
                mapping.drop_fields =
                    channels::ModelMapping::drop_fields_from_text(&m.drop_fields);
                mapping
            })
            .collect()
    }
}

async fn channels_create(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(body): Json<ChannelBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    // 四条写入（主行 + 密钥 + 请求头 + 模型名）必须一起成或一起败，
    // 否则会留下"名字在、请求头丢了"的半截渠道。
    let id = crate::db::in_tx(&conn, |c| {
        let id = channels::create(
            c,
            &channels::NewChannel {
                name: body.name.clone(),
                base_url: body.base_url.clone(),
                enabled: body.enabled,
                unwrap_envelope: body.unwrap_envelope,
            },
        )?;
        if let Some(k) = body.key.as_deref().filter(|k| !k.trim().is_empty()) {
            channels::set_secret(c, &state.secrets, id, k)?;
        }
        channels::set_headers(c, id, &body.headers())?;
        channels::set_models(c, id, &body.models())?;
        Ok(id)
    })?;
    tracing::info!("新建渠道 {id}：{}", body.name);

    let l = channels::get(&conn, id)?.ok_or_else(|| ApiError::internal("刚建的渠道读不回来"))?;
    Ok(Json(channel_json(&l)))
}

async fn channels_update(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<ChannelBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let id = tokengo_shared::ChannelId(id);
    crate::db::in_tx(&conn, |c| {
        channels::update(
            c,
            id,
            &body.name,
            &body.base_url,
            body.enabled,
            body.unwrap_envelope,
        )?;
        // 密钥：传了才改；传空串 = 明确要清掉
        match body.key.as_deref() {
            Some("") => channels::clear_secret(c, id)?,
            Some(k) if !k.trim().is_empty() => channels::set_secret(c, &state.secrets, id, k)?,
            _ => {}
        }
        channels::set_headers(c, id, &body.headers())?;
        channels::set_models(c, id, &body.models())?;
        Ok(())
    })?;
    tracing::info!("更新渠道 {id}：{}", body.name);

    let l = channels::get(&conn, id)?.ok_or_else(|| ApiError::not_found(format!("渠道 {id} 不存在")))?;
    Ok(Json(channel_json(&l)))
}

async fn channels_delete(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    channels::delete(&conn, tokengo_shared::ChannelId(id))?;
    tracing::info!("删除渠道 #{id}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ProbeQuery {
    /// 探活打哪个端点：chat（默认）或 embeddings。
    kind: Option<String>,
}

/// 一条"能真发请求"的渠道：解密好的密钥 + 自定义头 + 配的模型名。
///
/// 探活和「拉上游模型清单」都要这一套。抽出来是因为这两件事的加载逻辑一模一样，
/// 各写一份迟早会有一边忘了解密密钥（现象是"这条渠道明明有 key，面板却说没有"）。
struct ReadyChannel {
    channel: tokengo_shared::Channel,
    secret: Option<String>,
    has_secret: bool,
    models: Vec<channels::ModelMapping>,
}

fn ready_channel(state: &SharedState, id: tokengo_shared::ChannelId) -> Result<Option<ReadyChannel>, ApiError> {
    let conn = state.conn();
    let Some(row) = channels::get(&conn, id)? else {
        return Ok(None);
    };
    let secret = channels::secret(&conn, &state.secrets, id)?;
    Ok(Some(ReadyChannel {
        channel: tokengo_shared::Channel {
            id: row.id,
            name: row.name.clone(),
            base_url: row.base_url.clone(),
            enabled: row.enabled,
            priority: 0,
            model_alias: None,
            vision: false,
            drop_fields: vec![],
            unwrap_envelope: false,
            headers: row.headers.clone(),
        },
        secret,
        has_secret: row.has_secret,
        models: row.models.clone(),
    }))
}

/// 面板上的「测一下」：**真发一个请求到这条渠道**，回的绿点才是真的。
///
/// 三条纪律：
/// - 探活**不写记账**（它不是用户请求）；
/// - 探活**不动路由状态**（面板上的一次手动检查凭据，不该改变谁在用）；
/// - 模型名是这条渠道自己配的那个，拿不到就明说"探不了"，不猜（拿 `auto` 去探必然 400）。
async fn channels_probe(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(q): Query<ProbeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = tokengo_shared::ChannelId(id);
    let endpoint = match q.kind.as_deref() {
        Some("embeddings") | Some("embedding") | Some("vec") => crate::gateway::EndpointKind::Embeddings,
        _ => crate::gateway::EndpointKind::Chat,
    };

    let Some(ready) = ready_channel(&state, id)? else {
        return Err(ApiError::not_found(format!("渠道 {id} 不存在")));
    };
    let ReadyChannel {
        channel,
        secret,
        has_secret,
        models,
    } = ready;

    let model = models
        .iter()
        .find_map(|m| m.upstream.clone().filter(|u| !u.trim().is_empty()))
        .or_else(|| {
            models
                .iter()
                .map(|m| m.model.clone())
                .find(|n| !n.trim().is_empty())
        });

    let Some(model) = model else {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "channel_id": channel.id.0,
            "endpoint": endpoint.as_str(),
            "note": "这条渠道没配任何模型名，没法探活：面板里先给它填一个「该渠道认的模型名」。拿 auto 去探必然被上游打回 400，绿点就不真了。",
            "has_secret": has_secret,
        })));
    };

    let timeout = {
        let cfg = state.routing().cfg.clone();
        std::time::Duration::from_millis(cfg.attempt_timeout_ms.max(1)).min(std::time::Duration::from_secs(15))
    };
    let t0 = now_ms();
    let r = state
        .upstream
        .probe(
            &channel,
            secret.as_deref(),
            endpoint.path(),
            crate::prober::probe_body(endpoint, &model),
            timeout,
            &crate::upstream::new_uuid(),
            now_ms(),
        )
        .await;
    let latency = now_ms() - t0;

    let (ok, status, note) = match r {
        Ok(code) => {
            let kind = tokengo_shared::routing::classify_status(code);
            let note = match kind {
                tokengo_shared::routing::AttemptOutcome::Success => "通了".to_string(),
                tokengo_shared::routing::AttemptOutcome::ChannelFault => {
                    format!("这条渠道自己的毛病（HTTP {code}）")
                }
                _ => format!("上游不接受这个请求（HTTP {code}），多半是模型名或参数不对"),
            };
            (kind == tokengo_shared::routing::AttemptOutcome::Success, Some(code), note)
        }
        Err(fault) => (false, None, fault.message),
    };

    Ok(Json(serde_json::json!({
        "ok": ok,
        "channel_id": channel.id.0,
        "channel_name": channel.name,
        "endpoint": endpoint.as_str(),
        "model_sent": model,
        "http_status": status,
        "latency_ms": latency,
        "has_secret": has_secret,
        "note": note,
    })))
}

/// 面板上的「拉取上游模型清单」：用这条渠道自己的 URL、key、自定义头真打
/// `GET {base_url}/models`，把上游说它支持的模型名列出来。
///
/// 和探活同纪律：**不写记账、不动路由状态** —— 它是面板上的一次人工查询，
/// 不是用户请求，不该污染统计（否则"请求总数"会莫名其妙涨）。
///
/// 如实报三种失败，绝不给一份编出来的清单：
/// - 连不上/超时 → 说清是哪个 URL；
/// - 上游回了非 2xx → 把状态码报出来（404 很常见：这个上游压根没做 /v1/models）；
/// - 2xx 但格式不像模型清单 → 说"没认出"，并附上正文开头让人自己看。
async fn channels_models(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = tokengo_shared::ChannelId(id);
    let Some(ready) = ready_channel(&state, id)? else {
        return Err(ApiError::not_found(format!("渠道 {id} 不存在")));
    };
    let ReadyChannel { channel, secret, .. } = ready;

    let timeout = {
        let cfg = state.routing().cfg.clone();
        std::time::Duration::from_millis(cfg.attempt_timeout_ms.max(1))
            .min(std::time::Duration::from_secs(15))
    };
    let t0 = now_ms();
    let r = state
        .upstream
        .fetch_models(&channel, secret.as_deref(), timeout, &crate::upstream::new_uuid(), now_ms())
        .await;
    let latency = now_ms() - t0;
    let url = crate::upstream::join_url(&channel.base_url, "models");

    let (ok, status, models, note) = match r {
        Ok((code, body)) if (200..300).contains(&code) => {
            let models = crate::upstream::parse_model_ids(&body);
            if models.is_empty() {
                let note = if crate::upstream::looks_like_a_model_list(&body) {
                    format!(
                        "上游回的是清单形状，但里面一个模型名都没有 —— 它自己没报。正文：{}",
                        snippet(&body)
                    )
                } else {
                    format!(
                        "拉回来了（HTTP {code}），但没认出模型清单 —— 可能这个上游的 /v1/models 回的不是那种格式。正文开头：{}",
                        snippet(&body)
                    )
                };
                (false, Some(code), Vec::new(), note)
            } else {
                // 成功时的说明文字在下面统一拼（要带上真实个数）
                (true, Some(code), models, String::new())
            }
        }
        Ok((code, body)) => (
            false,
            Some(code),
            Vec::new(),
            format!(
                "上游回了 HTTP {code}（{url}）：多半是这个上游不支持 /v1/models。正文开头：{}",
                snippet(&body)
            ),
        ),
        Err(fault) => (false, None, Vec::new(), fault.message),
    };

    let note = if ok {
        format!("拉到 {} 个模型名", models.len())
    } else {
        note
    };

    tracing::info!(
        "拉渠道 {id} 的上游模型清单：{}（{} 个，{latency} ms）",
        if ok { "成功" } else { "失败" },
        models.len()
    );

    Ok(Json(serde_json::json!({
        "ok": ok,
        "channel_id": channel.id.0,
        "channel_name": channel.name,
        "url": url,
        "http_status": status,
        "latency_ms": latency,
        "count": models.len(),
        "models": models,
        "note": note,
    })))
}

/// 出错时给人看的一小段正文（不整段塞回来：上游可能吐几 MB）。
fn snippet(body: &str) -> String {
    crate::upstream::cap_chars(&body.replace('\n', " "), 300)
}

/// 「测视觉」的入参：测这条渠道上的哪个模型名。
///
/// 面板上每一行模型名旁边都有这个按钮，所以要把行里的名字传进来 ——
/// 不能拿"这条渠道随便一个模型名"去测，不然会测出另一个模型的结论。
#[derive(Deserialize)]
struct VisionTestBody {
    /// 对外模型名（通常就是组名，如 `auto`）。
    model: String,
    /// 发给上游的名字。留空 = 原样透传（跟保存的语义一致）。
    #[serde(default)]
    upstream: Option<String>,
    /// 「测完通过就顺手勾上」——默认勾。关掉就只看结果不改配置。
    #[serde(default = "default_true")]
    apply: bool,
}

/// 面板上的「测视觉」：**真发一张图**问这条渠道一个只有看图才能答对的问题。
///
/// 为什么不能靠猜：上游 FreeLLMAPI 有目录表告诉它哪个模型能看图，我们没有
/// （本项目的上游全是用户自定义的），而"模型名里带 vl / vision"这种推断经常错
/// —— deepseek 的 flash 就能读图，反倒有些名字里带 vision 的模型是纯装饰。
/// 所以只有真打一发才算数。
///
/// 三条纪律跟探活/拉清单一致：
/// - **不写记账**（不是用户请求，不能污染统计）；
/// - **不动路由状态**（一次人工检查不该改变谁在用）；
/// - 用**这条渠道自己的** URL / key / 自定义头。
async fn channels_vision_test(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<VisionTestBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = tokengo_shared::ChannelId(id);
    let Some(ready) = ready_channel(&state, id)? else {
        return Err(ApiError::not_found(format!("渠道 {id} 不存在")));
    };
    let ReadyChannel { channel, secret, .. } = ready;

    let model_name = body.model.trim().to_string();
    if model_name.is_empty() {
        return Err(ApiError::bad_request("要测哪个模型名？传 model"));
    }
    // 这一行在库里存不存在：不存在就直接说清楚（免得发自测了却没法勾上）
    let sent = body
        .upstream
        .clone()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| model_name.clone());

    let timeout = {
        let cfg = state.routing().cfg.clone();
        std::time::Duration::from_millis(cfg.attempt_timeout_ms.max(1))
            .min(std::time::Duration::from_secs(30))
    };
    let t0 = now_ms();
    let r = state
        .upstream
        .call_and_read(
            &channel,
            secret.as_deref(),
            crate::gateway::EndpointKind::Chat.path(),
            crate::prober::vision_test_body(&sent),
            timeout,
            &crate::upstream::new_uuid(),
            now_ms(),
        )
        .await;
    let latency = now_ms() - t0;

    let (ok, status, reply, note) = match r {
        Ok((code, text)) => {
            let reply = extract_reply(&text);
            if !(200..300).contains(&code) {
                let note = match code {
                    400 | 404 | 422 => format!(
                        "上游不收这张图（HTTP {code}）：这个模型多半不支持图像输入。它的原话：{}",
                        snippet(&text)
                    ),
                    401 | 403 => format!("凭据不对（HTTP {code}）—— 先点「测一下」看看这条渠道本身通不通"),
                    429 => format!("被上游限流了（HTTP {code}），过一会儿再测"),
                    _ => format!("上游回了 HTTP {code}：{}", snippet(&text)),
                };
                (false, Some(code), reply, note)
            } else if crate::prober::answer_says_red(&reply) {
                (
                    true,
                    Some(code),
                    reply,
                    "真看见了：它认出了左上角是红色（纯文本模型答不出这个）".to_string(),
                )
            } else if reply.trim().is_empty() {
                (
                    false,
                    Some(code),
                    reply,
                    "上游收了图但回了个空回复，看不出它到底看没看见 —— 别勾，先用一张真图在客户端里试一次".to_string(),
                )
            } else {
                let note = format!(
                    "上游收了图，但答的不是红色（它说：{}）—— 多半没真看图，或者这模型认不了这张图。别勾。",
                    snippet(&reply)
                );
                (false, Some(code), reply, note)
            }
        }
        Err(fault) => (false, None, String::new(), fault.message),
    };

    // 测通了、且用户没关掉 apply → 顺手把这行标上（省得再回表单勾一次）
    let applied = if ok && body.apply {
        let conn = state.conn();
        match channels::set_model_vision(&conn, id, &model_name, true) {
            Ok(_) => {
                tracing::info!("渠道 {id} 的「{model_name}」实测能看图，已标上");
                true
            }
            Err(e) => {
                // 这行不在库里（比如用户还没保存）—— 不影响结论，只是没法自动勾
                tracing::warn!("实测通过了但标记没落库：{e}");
                false
            }
        }
    } else {
        false
    };

    Ok(Json(serde_json::json!({
        "ok": ok,
        "channel_id": channel.id.0,
        "channel_name": channel.name,
        "model": model_name,
        "model_sent": sent,
        "http_status": status,
        "latency_ms": latency,
        "reply": crate::upstream::cap_chars(&reply, 300),
        "applied": applied,
        "note": note,
    })))
}

/// 从对话响应里抠出模型回的话（OpenAI 形状；认不出就回空串 —— 不编）。
fn extract_reply(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return String::new();
    };
    let msg = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"));
    if let Some(m) = msg {
        if let Some(s) = m.get("content").and_then(|c| c.as_str()) {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
        // 有的模型把答案放在 reasoning_content 里（拿它当证据总比空着强）
        if let Some(s) = m.get("reasoning_content").and_then(|c| c.as_str()) {
            return s.trim().to_string();
        }
    }
    if let Some(s) = v.get("content").and_then(|c| c.as_str()) {
        return s.trim().to_string();
    }
    String::new()
}

async fn groups_list(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let gs = groups::list(&conn)?;
    Ok(Json(serde_json::json!({
        "groups": gs.iter().map(|g| serde_json::json!({
            "name": g.name,
            "enabled": g.enabled,
            "channels": g.channels.iter().map(|l| l.0).collect::<Vec<_>>(),
            // 这一组现在有没有「能看图」的可用渠道 —— 组层面有个明确说法，
            // 不用让面板自己去把每条渠道的模型名对着组名翻一遍。
            "can_see": groups::group_can_see(&conn, &g.name).unwrap_or(false),
            // 哪种模型（对话/向量/重排）+ 谁定的（还没定 auto manual）——
            // 面板按它分区显示，也让人能一眼看出「这条是实测认出来的还是我自己选的」。
            "kind": g.kind.as_str(),
            "kind_label": g.kind.label(),
            "kind_source": g.kind_source.as_str(),
        })).collect::<Vec<_>>(),
        // 只列真实能用的模型名，给 /v1/models 和面板用同一份判断
        "servable": groups::list_servable(&conn)?
            .into_iter()
            .map(|s| serde_json::json!({
                "name": s.name,
                "vision": s.vision,
                "kind": s.kind.as_str(),
                "kind_label": s.kind.label(),
                "kind_source": s.kind_source.as_str(),
            }))
            .collect::<Vec<_>>(),
    })))
}

/// 给人手动定这组的类型（对话 / 向量 / 重排），或交还给实测（`kind: "auto"`）。
///
/// 手动定过之后**实测不许再覆盖** —— 用户拍过的板不被流量改掉。
async fn groups_set_kind(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(body): Json<GroupKindBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let kind = match body.kind.trim().to_ascii_lowercase().as_str() {
        // 「自动」= 清空，下次真收到请求再学
        "auto" | "" => tokengo_shared::ModelKind::Unknown,
        other => {
            let k = tokengo_shared::ModelKind::parse(other);
            if !k.is_set() {
                return Err(ApiError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "invalid_kind",
                    format!("不认识这个类型：{other}（只能是 auto / chat / embeddings / rerank）"),
                ));
            }
            k
        }
    };
    let conn = state.conn();
    // 先确认这组真在（set_kind 只改一行，不区分"改了几行"和"组不存在"）
    if groups::get(&conn, &name)?.is_none() {
        return Err(ApiError::not_found(format!("模型组 {name} 不存在")));
    }
    groups::set_kind(&conn, &name, kind)?;
    let g = groups::get(&conn, &name)?
        .ok_or_else(|| ApiError::not_found(format!("模型组 {name} 不存在")))?;
    tracing::info!("模型组 {} 的类型被改成 {}", name, g.kind.label());
    Ok(Json(serde_json::json!({
        "ok": true,
        "kind": g.kind.as_str(),
        "kind_label": g.kind.label(),
        "kind_source": g.kind_source.as_str(),
    })))
}

#[derive(Deserialize)]
struct GroupKindBody {
    /// `auto`（交还给实测）/ `chat` / `embeddings` / `rerank`。
    kind: String,
}

#[derive(Deserialize)]
struct GroupBody {
    name: String,
    #[serde(default = "default_true")]
    enabled: bool,
    /// 顺序即优先级，第一个是首选。
    channels: Vec<i64>,
}

async fn groups_upsert(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(body): Json<GroupBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let ids: Vec<tokengo_shared::ChannelId> = body.channels.iter().map(|i| tokengo_shared::ChannelId(*i)).collect();
    groups::upsert(&conn, &body.name, body.enabled, &ids)?;
    tracing::info!("保存模型组 {}：{} 条渠道", body.name, ids.len());
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn groups_delete(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    groups::delete(&state.conn(), &name)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ───────────────────────── 路由状态 ─────────────────────────

async fn routing_state(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let router = state.routing();
    let mut out = Vec::new();
    for g in groups::list(&conn)? {
        let st = router.state(&g.name).cloned().unwrap_or_default();
        let primary = g.channels.first().copied();
        out.push(serde_json::json!({
            "group": g.name,
            "enabled": g.enabled,
            // 首选=列表第一条；在用的可能是它也可能是切过去的某条
            "primary_channel_id": primary.map(|l| l.0),
            "active_channel_id": st.active.map(|l| l.0),
            "switched": st.active.is_some() && st.active != primary,
            "switch_count": st.switch_count,
            "last_switch_at_ms": st.last_switch_at_ms,
            "next_probe_at_ms": st.next_probe_at_ms,
            "now_ms": now_ms(),
            "channels": g.channels.iter().map(|l| l.0).collect::<Vec<_>>(),
        }));
    }
    Ok(Json(serde_json::json!({
        "config": router.cfg,
        "groups": out,
    })))
}

async fn routing_set_config(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(cfg): Json<tokengo_shared::routing::RoutingConfig>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if cfg.retry_times > 20 {
        return Err(ApiError::bad_request("重试次数过大（上限 20）"));
    }
    if cfg.reprobe_interval_ms < 0 {
        return Err(ApiError::bad_request("回切间隔不能是负数"));
    }
    {
        let conn = state.conn();
        routing::save_config(&conn, &cfg)?;
    }
    {
        let mut r = state.routing();
        r.cfg = cfg.clone();
    }
    tracing::info!("路由参数已更新：重试 {} 次、回切间隔 {} ms", cfg.retry_times, cfg.reprobe_interval_ms);
    Ok(Json(serde_json::json!({ "ok": true, "config": cfg })))
}

/// 立刻把某组的回切检查点归零 —— 不想等 10 分钟时用。
async fn routing_reprobe(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(group): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    {
        let conn = state.conn();
        if groups::get(&conn, &group)?.is_none() {
            return Err(ApiError::not_found(format!("模型组 {group} 不存在")));
        }
    }
    {
        let mut r = state.routing();
        if let Some(st) = r.state(&group) {
            let mut st = st.clone();
            st.next_probe_at_ms = Some(now_ms());
            r.restore(&group, st);
        }
    }
    state.persist_state(&group)?;
    Ok(Json(serde_json::json!({ "ok": true, "note": "下次请求会先试首选渠道" })))
}

// ───────────────────────── 记账 ─────────────────────────

#[derive(Deserialize)]
struct StatsQuery {
    /// 看最近多少小时，默认 24。
    hours: Option<i64>,
}

async fn stats(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let hours = q.hours.unwrap_or(24).clamp(1, 24 * 90);
    let conn = state.conn();
    let s = requests::stats_since(&conn, now_ms() - hours * 3600 * 1000)?;

    // 附上渠道名，面板不用再查一次
    let names: std::collections::HashMap<i64, String> = channels::list(&conn)?
        .into_iter()
        .map(|l| (l.id.0, l.name))
        .collect();
    let mut v = serde_json::to_value(&s).map_err(|e| ApiError::internal(e.to_string()))?;
    if let Some(arr) = v.get_mut("by_channel").and_then(|x| x.as_array_mut()) {
        for item in arr.iter_mut() {
            let id = item.get("channel_id").and_then(|x| x.as_i64()).unwrap_or(0);
            let name = names.get(&id).cloned().unwrap_or_else(|| format!("已删除的渠道 #{id}"));
            if let Some(obj) = item.as_object_mut() {
                obj.insert("channel_name".into(), serde_json::json!(name));
                let attempts = obj.get("attempts").and_then(|x| x.as_i64()).unwrap_or(0);
                let succ = obj.get("successes").and_then(|x| x.as_i64()).unwrap_or(0);
                obj.insert(
                    "success_rate".into(),
                    serde_json::json!(if attempts > 0 {
                        (succ as f64 / attempts as f64 * 1000.0).round() / 10.0
                    } else {
                        0.0
                    }),
                );
            }
        }
    }
    Ok(Json(serde_json::json!({
        "window_hours": hours,
        "stats": v,
    })))
}

#[derive(Deserialize)]
struct LogsQuery {
    limit: Option<u32>,
}

async fn logs(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let conn = state.conn();
    let rows = requests::recent(&conn, limit)?;
    Ok(Json(serde_json::json!({ "logs": rows })))
}

// ───────────────────────── 设置 ─────────────────────────

async fn settings_get(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let router = state.routing();
    Ok(Json(serde_json::json!({
        "routing_config": router.cfg,
        "gateway_key_hint": sessions::gateway_key_hint(&conn)?,
        "active_sessions": sessions::active_count(&conn)?,
        "reprobe_interval_minutes": router.cfg.reprobe_interval_ms / 60_000,
    })))
}

#[derive(Deserialize)]
struct SettingsBody {
    /// 换对外 key。传了就立刻生效，客户端要同步改。
    gateway_key: Option<String>,
}

async fn settings_put(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(body): Json<SettingsBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    if let Some(k) = body.gateway_key.as_deref() {
        if k.trim().is_empty() {
            return Err(ApiError::bad_request("对外 key 不能为空"));
        }
        sessions::set_gateway_key(&conn, k)?;
        state.forget_gateway_key();
        tracing::info!("对外 key 已更换");
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn generate_gateway_key(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let key = format!("tk-{}", &uuid::Uuid::new_v4().simple().to_string()[..24]);
    let conn = state.conn();
    sessions::set_gateway_key(&conn, &key)?;
    state.forget_gateway_key();
    tracing::info!("对外 key 已自动生成");
    Ok(Json(serde_json::json!({
        "ok": true,
        "gateway_key": key,
        "notice": "新 key 已生效；正在用旧 key 的客户端会马上 401，请同步更新。",
    })))
}

// ───────────────────────── 账目维护 ─────────────────────────

async fn maintenance_get(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let i = maintenance::info(&conn).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "retention_days": i.retention_days,
        "total_requests": i.total_requests,
        "oldest_ts_ms": i.oldest_ts_ms,
        "db_bytes": i.db_bytes,
        "last_prune_at_ms": i.last_prune_at_ms,
        "last_prune_count": i.last_prune_count,
    })))
}

#[derive(Deserialize)]
struct MaintenanceBody {
    /// 保留多少天。0 = 不清理。
    retention_days: i64,
}

async fn maintenance_set(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(body): Json<MaintenanceBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.retention_days < 0 {
        return Err(ApiError::bad_request("保留天数不能是负数（0 = 不清理）"));
    }
    let conn = state.conn();
    let d = maintenance::set_retention_days(&conn, body.retention_days)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    tracing::info!("账目保留天数改为 {d} 天");
    Ok(Json(serde_json::json!({
        "ok": true,
        "retention_days": d,
        "notice": if d == 0 {
            "已关闭自动清理：旧账不会再被删掉".to_string()
        } else {
            format!("以后只保留最近 {d} 天的请求记录")
        },
    })))
}

/// 立刻清一次（不等一天间隔）。面板上那个「立即清理一次」按钮打这里。
async fn maintenance_prune(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let run = crate::janitor::prune_now(&state)
        .await
        .map_err(|e| ApiError::internal(format!("清理失败：{e}")))?;
    let conn = state.conn();
    let i = maintenance::info(&conn).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "deleted": run.deleted,
        "batches": run.batches,
        "disabled": run.disabled,
        "remaining": i.total_requests,
        "db_bytes": i.db_bytes,
        "notice": if run.disabled {
            "保留天数是 0，清理处于关闭状态 —— 想清的话先把天数改成 30 之类".to_string()
        } else {
            format!("清掉 {} 条过期请求，还剩 {} 条", run.deleted, i.total_requests)
        },
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts_with(pairs: &[(&str, &str)]) -> Parts {
        let mut b = axum::http::Request::builder();
        for (k, v) in pairs {
            b = b.header(*k, *v);
        }
        b.body(()).unwrap().into_parts().0
    }

    #[test]
    fn bearer_and_cookie_tokens_are_both_accepted() {
        let p = parts_with(&[("authorization", "Bearer abc123")]);
        assert_eq!(bearer_token(&p).as_deref(), Some("abc123"));
        let p = parts_with(&[("cookie", "a=1; tokengo_session=xyz; b=2")]);
        assert_eq!(cookie_token(&p).as_deref(), Some("xyz"));
    }

    #[test]
    fn missing_or_empty_tokens_are_none() {
        assert!(bearer_token(&parts_with(&[])).is_none());
        assert!(bearer_token(&parts_with(&[("authorization", "Bearer   ")])).is_none());
        assert!(cookie_token(&parts_with(&[("cookie", "other=1")])).is_none());
    }

    #[test]
    fn client_ip_prefers_forwarded_header() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        assert_eq!(client_ip(&h).as_deref(), Some("1.2.3.4"));

        let mut h = axum::http::HeaderMap::new();
        h.insert("x-real-ip", "10.0.0.5".parse().unwrap());
        assert_eq!(client_ip(&h).as_deref(), Some("10.0.0.5"));

        assert!(client_ip(&axum::http::HeaderMap::new()).is_none());
    }
}
