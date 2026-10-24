//! 管理面：面板用的接口。只做面板真需要的，不做上游那 24 个管理路由。
//!
//! 鉴权：登录后拿到一个会话 token，之后每个请求带
//! 兼容运维 `Authorization: Bearer <token>`；新浏览器使用独立HttpOnly账号Cookie。
//! Cookie写操作必须通过统一CSRF门，普通面板账号不能进入管理面。
//! 除 `ping` / `auth/*` 外，**所有接口都要过会话门**。

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;

use super::{client_ip, unbuilt, ApiError};
use crate::db::{
    catalog, channels, gateway_keys, kinds, maintenance, model_routes, now_ms, requests, routing,
    sessions, settings,
};
use crate::SharedState;

pub fn router() -> Router<SharedState> {
    // 请求体格式不对时，axum 默认回的是**英文散文**
    // （`Failed to deserialize the JSON body into the target type: …`）、状态码 422、
    // 内容类型 text/plain —— 与全站 `{"error":{…}}` 信封格格不入，面板与体检脚本都按信封解析，
    // 外部调用方也一头雾水。这里统一改写成 400 + 信封（并带上原话，方便对字段）。
    // 只改写「反序列化失败」这一类 422；领域里的 422（如带图却没有视觉渠道）原样放过。
    router_inner().layer(axum::middleware::map_response(rewrite_deserialize_error))
}

async fn rewrite_deserialize_error(resp: Response) -> Response {
    if resp.status() != StatusCode::UNPROCESSABLE_ENTITY {
        return resp;
    }
    let plain = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("text/plain"))
        .unwrap_or(false);
    if !plain {
        return resp;
    }
    let (parts, body) = resp.into_parts();
    let bytes = match axum::body::to_bytes(body, 64 * 1024).await {
        Ok(b) => b,
        Err(_) => return ApiError::bad_request("请求体格式不对").into_response(),
    };
    let text = String::from_utf8_lossy(&bytes).to_string();
    if !text.contains("Failed to deserialize") {
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    }
    if super::operations::actor_id().is_some() {
        return ApiError::bad_request("请求字段或格式无效").into_response();
    }
    tracing::debug!("面板接口收到格式不对的请求体：{text}");
    ApiError::bad_request(format!(
        "请求体格式不对：{}。这一般是面板与后端版本不一致（刷新页面）或字段名写错了。",
        text.trim()
    ))
    .into_response()
}

fn router_inner() -> Router<SharedState> {
    Router::new()
        // —— 无鉴权 ——
        .route("/ping", get(ping))
        .route("/auth/status", get(auth_status))
        .route("/auth/setup", post(auth_setup).layer(axum::middleware::map_response(super::accounts::json_errors)))
        .route("/auth/login", post(auth_login).layer(axum::middleware::map_response(super::accounts::json_errors)))
        .route("/auth/logout", post(auth_logout).layer(axum::middleware::map_response(super::accounts::json_errors)))
        // —— 需要登录 ——
        .route("/auth/password", put(auth_change_password).layer(axum::middleware::map_response(super::accounts::json_errors)))
        .route("/channels", get(channels_list).post(channels_create))
        .route("/channels/{id}", put(channels_update).delete(channels_delete))
        .route("/channels/{id}/enabled", put(channels_set_enabled))
        .route("/channels/{id}/probe", post(channels_probe))
        .route("/channels/{id}/models", post(channels_models))
        .route("/channels/{id}/catalog", get(channel_catalog))
        .route("/channels/{id}/registry", get(registry_get).put(registry_set))
        .route("/channels/{id}/context", get(context_override_get).put(context_override_set))
        .route("/channels/{id}/vision-test", post(channels_vision_test))
        // 「模型名 → 用途」：面板「模型」页以渠道里勾过的名字为行，用途读这张表
        .route("/channels/{id}/move", post(channels_move))
        .route("/model-kinds", get(model_kinds_list))
        .route("/auto-policy", get(auto_policy_get).put(auto_policy_save))
        .route("/model-order/move", post(model_order_move))
        .route("/model-enabled", put(model_enabled_save))
        .route("/channel-faults/retry", post(channel_fault_retry))
        .route("/model-cooldowns", get(model_cooldowns))
        .route("/model-cooldown/reset", post(model_cooldown_reset))
        .route("/models/{name}/kind", put(models_set_kind))
        // 面板「模型」页要的全部事实（一名字一行 + 它后面那串渠道 + 路由状态 + auto 的候选）。
        .route("/model-routes", get(model_routes_list))
        .route("/routing", get(routing_state))
        .route("/routing/config", put(routing_set_config))
        .route("/routing/{model}/reprobe", post(routing_reprobe))
        .route("/stats", get(stats))
        .route("/logs", get(logs))
        .route("/requests/{id}", get(request_detail))
        // 面板「日志」页：看运行日志（内存里最近那些）+ 临时开 debug
        .route("/logs/live", get(logs_live))
        .route("/logs/level", get(logs_level_get).put(logs_level_set))
        .route("/timeline", get(|| async { unbuilt(5, "GET /api/timeline") }))
        .route("/gateway-keys", get(gateway_keys_list).post(gateway_keys_create))
        .route("/gateway-keys/{id}/reveal", get(gateway_keys_reveal).layer(axum::middleware::map_response(super::identity::reveal_no_store)))
        .route("/gateway-keys/{id}", axum::routing::delete(gateway_keys_delete))
        .route("/settings", get(settings_get).put(settings_put))
        .route("/settings/generate-gateway-key", post(generate_gateway_key))
        // 面板「复制」按钮走这里：拿到明文（要面板会话）。
        // 为什么单独一个接口：明文不进列表响应，减少它出现在日志/缓存里的机会。
        .route("/settings/gateway-key", get(reveal_gateway_key))
        .route("/maintenance", get(maintenance_get).post(maintenance_set))
        .route("/maintenance/prune", post(maintenance_prune))
}

// ───────────────────────── 鉴权 ─────────────────────────

/// 会话门。放在 handler 参数里即生效。
use super::operations::ManagementAuth;
pub struct PanelAuth;

impl FromRequestParts<SharedState> for PanelAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        super::accounts::root_authorize(parts, state).await?;
        Ok(PanelAuth)
    }
}

#[cfg(test)]
fn bearer_token(parts: &Parts) -> Option<String> {
    let raw = parts.headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let t = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
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
        gateway_keys::any(&conn)?;
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
    super::accounts::legacy_request_headers(&headers)?;
    crate::db::accounts::password(&body.password).map_err(|e|ApiError::bad_request(e.to_string()))?;
    let conn = state.conn();
    if sessions::has_panel_password(&conn)? {
        return Err(ApiError::bad_request("口令已设置过，请走登录"));
    }
    sessions::set_panel_password(&conn, &body.password)?;

    // 用户偏好：沿用惯用的短口令，不保留自动生成的长随机串。没传才自动生成。
    let (key, generated) = match body.gateway_key.as_deref() {
        Some(k) if !k.trim().is_empty() => (k.trim().to_string(), false),
        _ => (gateway_keys::generate(), true),
    };
    sessions::set_gateway_key(&conn, &state.secrets, &key)?;
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
        "notice": "Key is encrypted at rest and can be copied again from settings.",
    })))
}

#[derive(Deserialize)]
struct LoginBody {
    #[serde(default)]
    username: Option<String>,
    password: String,
}

async fn auth_login(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<LoginBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    super::accounts::legacy_request_headers(&headers)?;
    let username = body
        .username
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("登录需要用户名和密码"))?;
    if body.password.len() > 256 {
        return Err(ApiError::unauthorized("账号或密码错误"));
    }
    state
        .panel_login_gate
        .check(&username.to_ascii_lowercase())?;
    let root = crate::db::accounts::root(&state.conn())?
        .ok_or_else(|| ApiError::unauthorized("账号或密码错误"))?;
    let username_ok = root.account.username.eq_ignore_ascii_case(username);
    let hash = settings::get(&state.conn(), settings::PANEL_PASSWORD_HASH)?
        .ok_or_else(|| ApiError::bad_request("还没设置口令，请先初始化"))?;
    let slot = state.auth_slots.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "auth_busy",
            "Authentication busy; try again later",
        )
    })?;
    let verify_hash = hash.clone();
    let ok = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        sessions::verify_password(&body.password, &verify_hash)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let conn = state.conn();
    if !ok
        || !username_ok
        || !crate::db::accounts::legacy_root_valid(&conn)?
        || crate::db::accounts::root(&conn)?.is_none_or(|now| {
            now.version != root.version || now.account.username != root.account.username
        })
        || settings::get(&conn, settings::PANEL_PASSWORD_HASH)?.as_deref() != Some(hash.as_str())
    {
        return Err(ApiError::unauthorized("口令不对"));
    }
    state
        .panel_login_gate
        .success(&username.to_ascii_lowercase());
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
    headers: axum::http::HeaderMap,
    Json(body): Json<TokenBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    super::accounts::legacy_request_headers(&headers)?;
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
    auth: super::accounts::RootGrant,
    State(state): State<SharedState>,
    Json(body): Json<ChangePasswordBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::db::accounts::password(&body.new_password).map_err(|e|ApiError::bad_request(e.to_string()))?;
    let root = crate::db::accounts::root(&state.conn())?.ok_or_else(|| ApiError::unauthorized("未登录"))?;
    state.panel_login_gate.check(&root.account.username)?;
    if !super::accounts::verify(&state, body.old_password, root.hash.clone()).await? {
        return Err(ApiError::unauthorized("原口令不对"));
    }
    let hash = super::accounts::hash(&state, body.new_password).await?;
    let conn = state.conn();
    if !auth.valid(&conn)? || crate::db::accounts::root(&conn)?.is_none_or(|a|a.version!=root.version || a.hash!=root.hash) {
        return Err(ApiError::unauthorized("会话已失效"));
    }
    let token = crate::db::accounts::atomic(&conn, || {
        sessions::set_panel_hash(&conn, &hash)?;
        crate::db::accounts::audit(&conn,root.account.id,"panel.password",root.account.id)?;
        sessions::create_session(&conn, None)
    })?;
    state.panel_login_gate.success(&root.account.username);
    Ok(Json(serde_json::json!({ "ok": true, "token": token })))
}

// ───────────────────────── 渠道 ─────────────────────────

fn channel_json(c:&rusqlite::Connection,l: &channels::ChannelRow) -> serde_json::Value {
    serde_json::json!({
        "id": l.id.0,
        "provider_id":l.provider_id,
        "provider_name":l.provider_name,
        "provider_enabled":l.provider_enabled,
        "name": l.name,
        "base_url": l.base_url,
        "enabled": l.enabled,
        "has_secret": l.has_secret,
        "account_status": crate::db::faults::rows(c).unwrap_or_default().into_iter().find(|r|r.channel_id==l.id.0 && r.model.is_empty()).map(|r|if r.ready {"等待恢复试用".to_string()}else if r.lease_until>now_ms(){"恢复试用中".into()}else{r.reason}).unwrap_or_default(),
        "unwrap_envelope": l.unwrap_envelope,
        "max_concurrent": l.max_concurrent,
        "protocols": crate::protocol_config::load(c,l.id.0).ok(),
        // 全局顺序：面板「渠道」页从上到下就是「先试哪条」（小的先试）
        "priority": l.priority,
        "headers": l.headers.iter().map(|(n, _)| serde_json::json!({"name": n, "original_name": n, "value": "********", "action": "keep"})).collect::<Vec<_>>(),
        "models": l.models.iter().map(|m| serde_json::json!({
            "model": m.model,
            "enabled":m.enabled,
            "reasoning":crate::reasoning_capabilities::for_mapping(c,l.id.0,&m.model).ok(),
            "upstream": m.upstream,
            "vision": m.vision,
            "drop_fields": m.drop_fields_text(),
        })).collect::<Vec<_>>(),
        // 这条渠道上「能看图」的模型名有几个 —— 渠道列表里直接显示，不用展开也知道。
        "vision_count": l.models.iter().filter(|m| m.vision).count(),
    })
}

async fn channels_list(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let all = channels::list(&conn)?;
    Ok(Json(serde_json::json!({
        "channels": all.iter().map(|l|channel_json(&conn,l)).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct HeaderIn {
    name: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    original_name: Option<String>,
}

#[derive(Deserialize)]
struct ModelIn {
    #[serde(default)]
    enabled: Option<bool>,
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
    #[serde(default)]
    protocols: Option<crate::protocol_config::Capabilities>,
    name: String,
    base_url: String,
    #[serde(default = "default_true")]
    enabled: bool,
    /// 非流式响应外面套了一层壳时自动剥掉（面板上的「响应解包」勾选）。
    #[serde(default)]
    unwrap_envelope: bool,
    /// 这条渠道「同时最多几发」（面板上的「同时最多几发」）。0 = 不限。
    #[serde(default)]
    max_concurrent: u32,
    /// 明文密钥。留空表示不改动已存的（新建时留空 = 没密钥）。
    key: Option<String>,
    #[serde(default)]
    headers: Option<Vec<HeaderIn>>,
    #[serde(default)]
    models: Vec<ModelIn>,
}

fn default_true() -> bool {
    true
}

impl ChannelBody {
    fn headers(
        &self,
        conn: &rusqlite::Connection,
        state: &SharedState,
        id: Option<tokengo_shared::ChannelId>,
    ) -> Result<Option<Vec<(String, String)>>, ApiError> {
        let Some(input) = &self.headers else {
            return Ok(None);
        };
        let old = match id {
            Some(id) => channels::headers(conn, &state.secrets, id)?,
            None => vec![],
        };
        let mut output = Vec::new();
        let mut names = std::collections::HashSet::new();
        for h in input {
            let name = h.name.trim();
            if name.is_empty() && h.value.is_empty() && h.action.is_none() {
                continue;
            }
            if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err()
                || !names.insert(name.to_ascii_lowercase())
            {
                return Err(ApiError::bad_request("Invalid or duplicate header name"));
            }
            let action = h.action.as_deref().unwrap_or("replace");
            let value = match action {
                "keep" => old
                    .iter()
                    .find(|(n, _)| {
                        n.eq_ignore_ascii_case(h.original_name.as_deref().unwrap_or(name))
                    })
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| ApiError::bad_request("Header to keep does not exist"))?,
                "replace" => {
                    if h.value == "********" || h.value.starts_with("sealed-header-v1:") {
                        return Err(ApiError::bad_request(
                            "Enter a replacement value, not a mask",
                        ));
                    }
                    h.value.clone()
                }
                "clear" => String::new(),
                "delete" => continue,
                _ => return Err(ApiError::bad_request("Unknown header action")),
            };
            if reqwest::header::HeaderValue::from_str(&value).is_err() {
                return Err(ApiError::bad_request("Invalid header value"));
            }
            output.push((name.to_string(), value));
        }
        Ok(Some(output))
    }

    fn models(&self) -> Result<Vec<channels::ModelMapping>, ApiError> {
        let mut names = std::collections::HashSet::new();
        let mut mappings = Vec::with_capacity(self.models.len());
        for input in &self.models {
            let model = input.model.trim();
            let normalized = tokengo_shared::normalize_model_name(model);
            if normalized.is_empty() || input.model.chars().any(char::is_control) {
                return Err(ApiError::bad_request("模型名不能为空或包含控制字符"));
            }
            if normalized == "auto" {
                return Err(ApiError::bad_request("auto 是自动路由入口，不能保存为渠道模型"));
            }
            if !names.insert(normalized) {
                return Err(ApiError::bad_request("同一渠道不能重复声明归一后相同的模型名"));
            }
            if input.upstream.as_deref().is_some_and(|name| name.chars().any(char::is_control)) {
                return Err(ApiError::bad_request("上游模型名不能包含控制字符"));
            }
            // Only logical names use our identity normalization. Upstream names are
            // provider-owned identifiers: preserve their case and meaningful symbols.
            let upstream = input.upstream.as_deref().map(str::trim)
                .filter(|name| !name.is_empty()).map(str::to_string);
            let mut mapping = channels::ModelMapping::new(model, upstream, input.vision);
            mapping.enabled = input.enabled.unwrap_or(true);
            mapping.drop_fields = channels::ModelMapping::drop_fields_from_text(&input.drop_fields);
            mappings.push(mapping);
        }
        Ok(mappings)
    }
}

async fn channels_create(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Json(body): Json<ChannelBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(p) = &body.protocols {
        p.validate().map_err(ApiError::bad_request)?;
    }
    let models = body.models()?;
    let conn = state.conn();
    let headers = body.headers(&conn, &state, None)?;
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
                max_concurrent: body.max_concurrent,
            },
        )?;
        if let Some(k) = body.key.as_deref().filter(|k| !k.trim().is_empty()) {
            channels::set_secret(c, &state.secrets, id, k)?;
        }
        if let Some(headers) = &headers {
            if channels::headers(c, &state.secrets, id)? != *headers {
                channels::set_headers(c, &state.secrets, id, headers)?;
            }
        }
        channels::set_models(c, id, &models)?;
        if let Some(p) = &body.protocols {
            crate::protocol_config::save(c, id.0, p)?;
        }
        Ok(id)
    })?;
    tracing::info!("新建渠道 {id}：{}", body.name);

    let l = channels::get(&conn, id)?.ok_or_else(|| ApiError::internal("刚建的渠道读不回来"))?;
    Ok(Json(channel_json(&conn, &l)))
}

async fn channels_update(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<ChannelBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(p) = &body.protocols {
        p.validate().map_err(ApiError::bad_request)?;
    }
    let mut models = body.models()?;
    let conn = state.conn();
    let id = tokengo_shared::ChannelId(id);
    if let Some(old) = channels::get(&conn, id)? {
        for (m, input) in models.iter_mut().zip(&body.models) {
            if input.enabled.is_none() {
                if let Some(was) = old
                    .models
                    .iter()
                    .find(|v| tokengo_shared::same_model_name(&v.model, &m.model))
                {
                    m.enabled = was.enabled;
                }
            }
        }
    }
    let headers = body.headers(&conn, &state, Some(id))?;
    let managed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM source_credentials WHERE source_id=?1)",[id.0],|r|r.get(0)).map_err(crate::db::DbError::from)?;
    if managed {
        let old = channels::get(&conn, id)?.ok_or_else(|| ApiError::not_found("连接不存在"))?;
        let url_changed = crate::db::provider_pool::canonical_url(&body.base_url).ok()
            != crate::db::provider_pool::canonical_url(&old.base_url).ok();
        let secret_changed = body.key.as_deref().is_some_and(|k| {
            channels::secret(&conn, &state.secrets, id)
                .ok()
                .flatten()
                .as_deref()
                != Some(k)
        });
        if url_changed || secret_changed {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "provider_pool_managed",
                "此来源已由渠道 Key 池管理，请在渠道入口或 Key 管理中修改地址、密钥和认证头",
            ));
        }
    }
    crate::db::in_tx(&conn, |c| {
        channels::update(
            c,
            id,
            &body.name,
            &body.base_url,
            body.enabled,
            body.unwrap_envelope,
            body.max_concurrent,
        )?;
        // 密钥：传了才改；传空串 = 明确要清掉
        match body.key.as_deref() {
            Some("") => channels::clear_secret(c, id)?,
            Some(k) if !k.trim().is_empty() => channels::set_secret(c, &state.secrets, id, k)?,
            _ => {}
        }
        if let Some(headers) = &headers {
            if managed {crate::db::provider_pool::update_non_auth_headers(c,&state.secrets,id.0,headers)?;}
            else {channels::set_headers(c, &state.secrets, id, headers)?;}
        }
        channels::set_models(c, id, &models)?;
        if let Some(p) = &body.protocols {
            crate::protocol_config::save(c, id.0, p)?;
        }
        Ok(())
    }).map_err(|e|if managed {ApiError::new(StatusCode::CONFLICT,"provider_pool_managed",e.to_string())}else{ApiError::from(e)})?;
    tracing::info!("更新渠道 {id}：{}", body.name);

    let l = channels::get(&conn, id)?
        .ok_or_else(|| ApiError::not_found(format!("渠道 {id} 不存在")))?;
    Ok(Json(channel_json(&conn, &l)))
}

async fn channels_delete(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let cid = tokengo_shared::ChannelId(id);
    // 不在了就老老实实说 404（以前漏了这一步，删两下第二下回的是 500「内部错误」）
    if channels::get(&conn, cid)?.is_none() {
        return Err(ApiError::not_found(format!("渠道 {cid} 不存在")));
    }
    let tx = conn
        .unchecked_transaction()
        .map_err(crate::db::DbError::from)?;
    let names = crate::db::lifecycle::delete_source(&tx,id).map_err(|e|ApiError::new(StatusCode::CONFLICT,"source_delete_blocked",e.to_string()))?;
    tx.commit().map_err(crate::db::DbError::from)?;
    drop(conn);
    let mut routing = state.routing();
    for name in names { routing.restore(&name,Default::default()); }
    Ok(Json(serde_json::json!({ "ok": true,"history_preserved":true })))
}

#[derive(Deserialize)]
struct ProbeQuery {
    /// 探活打哪个端点：chat（默认）或 embeddings。
    kind: Option<String>,
    model: Option<String>,
}

/// 一条"能真发请求"的渠道：解密好的密钥 + 自定义头 + 配的模型名。
///
/// 探活和「拉上游模型清单」都要这一套。抽出来是因为这两件事的加载逻辑一模一样，
/// 各写一份迟早会有一边忘了解密密钥（现象是"这条渠道明明有 key，面板却说没有"）。
struct ReadyChannel {
    source_identity: String,
    channel: tokengo_shared::Channel,
    secret: Option<String>,
    has_secret: bool,
    models: Vec<channels::ModelMapping>,
}

fn ready_channel(state: &SharedState, id: tokengo_shared::ChannelId) -> Result<Option<ReadyChannel>, ApiError> {
    let conn = state.conn();
    if !crate::protocol_config::background_probe_allowed(&conn,id.0)? {
        return Err(ApiError::bad_request("OAuth and standalone tools require authenticated, metered verification; use the OAuth account catalog or the normal VKey endpoint"));
    }
    let Some(row) = channels::get(&conn, id)? else {
        return Ok(None);
    };
    let secret = channels::secret(&conn, &state.secrets, id)?;
    Ok(Some(ReadyChannel {
        source_identity:crate::reasoning_capabilities::fingerprint(&conn,id.0)?,
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
            headers: channels::headers(&conn, &state.secrets, id)?,
            max_concurrent: 0,
        },
        secret,
        has_secret: row.has_secret,
        models: row.models.clone(),
    }))
}

fn diagnostic_model(models:&[channels::ModelMapping],requested:Option<&str>)->Result<Option<String>,ApiError> {
    let mapping=if let Some(name)=requested {
        if name.trim().is_empty(){return Err(ApiError::bad_request("请选择要诊断的模型"));}
        Some(models.iter().find(|m|m.model==name&&m.enabled).ok_or_else(||ApiError::bad_request("所选模型未在此 Key 启用；请重新选择"))?)
    } else {models.iter().find(|m|m.enabled&&!m.model.trim().is_empty())};
    Ok(mapping.map(|m|m.upstream.as_deref().filter(|u|!u.trim().is_empty()).unwrap_or(&m.model).to_string()))
}

/// 面板上的「测一下」：**真发一个请求到这条渠道**，回的绿点才是真的。
///
/// 三条纪律：
/// - 探活**不写记账**（它不是用户请求）；
/// - 探活**不动路由状态**（面板上的一次手动检查凭据，不该改变谁在用）；
/// - 模型名是这条渠道自己配的那个，拿不到就明说"探不了"，不猜（拿 `auto` 去探必然 400）。
async fn channels_probe(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(q): Query<ProbeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = tokengo_shared::ChannelId(id);
    let endpoint = match q.kind.as_deref() {
        Some("embeddings") | Some("embedding") | Some("vec") => {
            crate::gateway::EndpointKind::Embeddings
        }
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
        ..
    } = ready;

    let model = diagnostic_model(&models,q.model.as_deref())?;

    let Some(model) = model else {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "channel_id": channel.id.0,
            "endpoint": endpoint.as_str(),
            "note": "这条渠道没配任何模型名，没法探活：面板里先给它填一个「该渠道认的模型名」。拿 auto 去探必然被上游打回 400，绿点就不真了。",
            "has_secret": has_secret,
        })));
    };

    let endpoint = crate::protocol_config::probe_endpoint(&state.conn(), channel.id.0, endpoint)?;

    let timeout = {
        let cfg = state.routing().cfg.clone();
        std::time::Duration::from_millis(cfg.attempt_timeout_ms.max(1))
            .min(std::time::Duration::from_secs(15))
    };
    let t0 = now_ms();
    let r = state
        .upstream
        .probe_report(
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
        Ok((code, upstream_note)) => {
            let kind = tokengo_shared::routing::classify_status(code);
            let mut note = match code {
                200..=299 => "诊断请求成功；仅代表本次所选模型调用成功".to_string(),
                400 => "上游拒绝本次请求；请核对所选模型、映射及参数，不能据此判断整个 Key 停服".to_string(),
                401|403 => "上游鉴权或权限检查失败；请核对 Key 和所选模型的访问权限".to_string(),
                429 => "上游返回限流或额度不足；需结合上游说明判断，不能仅凭 429 认定 Key 已停服".to_string(),
                _ => format!("本次诊断收到上游 HTTP {code}；不代表该 Key 的全部模型均不可用"),
            };
            if let Some(detail)=upstream_note.filter(|s|!s.is_empty()){note.push_str("；上游说明：");note.push_str(&detail);}
            (
                kind == tokengo_shared::routing::AttemptOutcome::Success,
                Some(code),
                note,
            )
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
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = tokengo_shared::ChannelId(id);
    let Some(ready) = ready_channel(&state, id)? else {
        return Err(ApiError::not_found(format!("渠道 {id} 不存在")));
    };
    let ReadyChannel { channel, secret, source_identity, .. } = ready;

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

    // 拉回来的不只是名字：上游自己报的归属方/上下文/价钱/能不能收图也一并带回去
    // （`UpstreamModel`）。上游没说的字段就是空，界面据此决定显示哪些小标签。
    let (ok, status, catalog, seen, note) = match r {
        Ok((code, body)) if (200..300).contains(&code) => {
            let (catalog, seen) = crate::upstream::parse_model_catalog_with_total(&body);
            if catalog.is_empty() {
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
                (false, Some(code), Vec::new(), 0usize, note)
            } else {
                // 成功时的说明文字在下面统一拼（要带上真实个数）
                (true, Some(code), catalog, seen, String::new())
            }
        }
        Ok((code, body)) => (
            false,
            Some(code),
            Vec::new(),
            0usize,
            format!(
                "上游回了 HTTP {code}（{url}）：多半是这个上游不支持 /v1/models。正文开头：{}",
                snippet(&body)
            ),
        ),
        Err(fault) => (false, None, Vec::new(), 0usize, fault.message),
    };

    // Reject reflected credentials before persisting catalog IDs or metadata. This also
    // protects future reads of a catalog fetched through the root compatibility path.
    let reflected = serde_json::to_value(&catalog).map_err(|_| ApiError::internal("Cannot validate model catalog"))?;
    let forbidden = secret.iter().cloned().chain(channel.headers.iter().map(|(_,v)|v.clone())).collect::<Vec<_>>();
    super::operations::reject_credential_echo(&reflected, &forbidden)?;
    let note = if ok {
        if seen > catalog.len() {
            // 如实说：上游报的比这里列出来的多（列表有上限）
            format!(
                "拉到 {} 个模型名（上游一共报了 {seen} 个，列表只放得下前 {} 个）",
                catalog.len(),
                catalog.len()
            )
        } else {
            format!("拉到 {} 个模型名", catalog.len())
        }
    } else {
        note
    };

    // 拉成功就**记住**：下次打开弹窗不用再问一遍（有的上游要花钱、有限流），
    // 也才有「上游这次比上次多了/少了哪些名字」这种漂移事实。
    // 拉失败一律不写：一次连不上不能把上次真拿到手的清单删掉。
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut fetched_at = 0i64;
    if ok {
        let conn = state.conn();
        if channels::get(&conn,channel.id)?.is_none() || crate::reasoning_capabilities::fingerprint(&conn,channel.id.0)?!=source_identity {
            return Err(ApiError::bad_request("Source configuration changed; refresh metadata again"));
        }
        let before = catalog::list(&conn, channel.id)?;
        let (a, r) = catalog::diff(&before, &catalog);
        added = a;
        removed = r;
        let now = now_ms();
        crate::db::in_tx(&conn, |c| catalog::replace(c, channel.id, &catalog, now))?;
        fetched_at = now;
        tracing::info!(
            "渠道 {id} 的上游模型清单已记住：{} 个（比上次多 {}、少 {}）",
            catalog.len(),
            added.len(),
            removed.len()
        );
    }

    tracing::info!(
        "拉渠道 {id} 的上游模型清单：{}（{} 个，{latency} ms）",
        if ok { "成功" } else { "失败" },
        catalog.len()
    );

    Ok(Json(serde_json::json!({
        "ok": ok,
        "channel_id": channel.id.0,
        "channel_name": channel.name,
        "url": url,
        "http_status": status,
        "latency_ms": latency,
        "count": catalog.len(),
        // 名字数组（老口径，别动：旧面板只认它）
        "models": catalog.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        // 名字 + 上游自己报的说明
        "info": catalog,
        "note": note,
        // 这次拉到的是不是已经落库了（拉失败 = false），以及落库时间
        "saved": ok,
        "fetched_at": fetched_at,
        // 跟上次记住的比：多了哪些、少了哪些（漂移看得见）
        "added": added,
        "removed": removed,
    })))
}

#[derive(Deserialize)]
struct RegistryQuery {
    model: String,
    upstream: String,
    #[serde(default)]
    q: String,
}
#[derive(Deserialize)]
struct RegistryWrite {
    model: String,
    upstream: String,
    identity: Option<String>,
}

fn registry_mapping(conn: &rusqlite::Connection, id: i64, model: &str, upstream: &str) -> Result<channels::ModelMapping, ApiError> {
    let channel = channels::get(conn, tokengo_shared::ChannelId(id))?
        .ok_or_else(|| ApiError::not_found("渠道不存在"))?;
    let mapping = channel.models.into_iter().find(|m| m.model == model)
        .ok_or_else(|| ApiError::not_found("请先保存渠道中的模型映射"))?;
    if crate::model_registry::upstream(&mapping) != upstream {
        return Err(ApiError::bad_request("上游映射已改变，请先保存并重新打开详情"));
    }
    Ok(mapping)
}

#[derive(Deserialize)]
struct ContextOverrideWrite {
    model: String,
    upstream: String,
    tokens: Option<i64>,
}
async fn context_override_get(_auth:ManagementAuth,State(state):State<SharedState>,Path(id):Path<i64>,Query(query):Query<RegistryQuery>)->Result<Json<serde_json::Value>,ApiError>{
 let c=state.conn();let mapping=registry_mapping(&c,id,&query.model,&query.upstream)?;
 Ok(Json(serde_json::json!({"override_tokens":crate::model_registry::context_override(&c,tokengo_shared::ChannelId(id),&mapping)?})))
}
async fn context_override_set(_auth: ManagementAuth, State(state): State<SharedState>, Path(id): Path<i64>, Json(body): Json<ContextOverrideWrite>) -> Result<Json<serde_json::Value>, ApiError> {
    let conn=state.conn();
    let mapping=registry_mapping(&conn,id,&body.model,&body.upstream)?;
    if let Some(tokens)=body.tokens {
        if tokens<=0 { return Err(ApiError::bad_request("用户确认上下文必须为正整数 tokens；清除请传 null")); }
        conn.execute("INSERT INTO channel_context_overrides(channel_id,upstream_model,context_window) VALUES(?1,?2,?3) ON CONFLICT(channel_id,upstream_model) DO UPDATE SET context_window=excluded.context_window",rusqlite::params![id,crate::model_registry::upstream(&mapping),tokens]).map_err(crate::db::DbError::from)?;
    } else {
        conn.execute("DELETE FROM channel_context_overrides WHERE channel_id=?1 AND upstream_model=?2",rusqlite::params![id,crate::model_registry::upstream(&mapping)]).map_err(crate::db::DbError::from)?;
    }
    let tokens=crate::model_registry::context_override(&conn,tokengo_shared::ChannelId(id),&mapping)?;
    Ok(Json(serde_json::json!({"tokens":tokens,"source":tokens.map(|_|"user_confirmed")})))
}

async fn registry_get(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(query): Query<RegistryQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let mapping = registry_mapping(&conn, id, &query.model, &query.upstream)?;
    let channel = tokengo_shared::ChannelId(id);
    let context = crate::model_registry::mapped(&conn, channel, &mapping)?;
    let identity = crate::model_registry::associated(&conn, channel, &mapping)?;
    let verified =
        crate::capability_detector::mapped_context(&catalog::list(&conn, channel)?, &mapping);
    let mut stmt = conn.prepare("SELECT model_id,context_window FROM registry_context WHERE instr(lower(model_id),lower(?1))>0 AND context_window>0 ORDER BY model_id LIMIT 100").map_err(crate::db::DbError::from)?;
    let entries = stmt
        .query_map([&query.q], |r| {
            Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"tokens":r.get::<_,i64>(1)?}))
        })
        .map_err(crate::db::DbError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::db::DbError::from)?;
    Ok(Json(
        serde_json::json!({"identity":identity,"tokens":context.tokens,"reason":context.reason,"upstream_context":verified,"override_tokens":crate::model_registry::context_override(&conn,channel,&mapping)?,"entries":entries}),
    ))
}

async fn registry_set(_auth: ManagementAuth, State(state): State<SharedState>, Path(id): Path<i64>, Json(body): Json<RegistryWrite>) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let mapping = registry_mapping(&conn, id, &body.model, &body.upstream)?;
    if let Some(identity) = &body.identity {
        let candidate = channels::ModelMapping::new("", Some(identity.clone()), false);
        let context = crate::model_registry::mapped(&conn, tokengo_shared::ChannelId(-1), &candidate)?;
        if context.tokens.is_none() {
            return Err(ApiError::bad_request(format!("不可关联该精确 ID：{}", context.reason)));
        }
        conn.execute("INSERT INTO registry_associations(channel_id,model_name,upstream_model,model_id) VALUES(?1,?2,?3,?4) ON CONFLICT(channel_id,model_name,upstream_model) DO UPDATE SET model_id=excluded.model_id",
            rusqlite::params![id,mapping.model,body.upstream,identity]).map_err(crate::db::DbError::from)?;
    } else {
        conn.execute("DELETE FROM registry_associations WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3", rusqlite::params![id,mapping.model,body.upstream]).map_err(crate::db::DbError::from)?;
    }
    Ok(Json(serde_json::json!({"ok":true})))
}

/// 这条渠道**上次**问到的上游模型清单（不联网，读库）。
///
/// 面板打开编辑弹窗先读它，省一次上游请求；要不要刷新由用户按「问一次上游」决定。
/// `ok` = 库里到底有没有东西（没有就是 `ok: false` + `fetched_at: 0`，界面据此说
/// 「还没问过上游」而不是假装有一份空清单）。
async fn channel_catalog(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = tokengo_shared::ChannelId(id);
    let (entries, fetched_at, name) = {
        let conn = state.conn();
        let Some(l) = channels::get(&conn, id)? else {
            return Err(ApiError::not_found(format!("渠道 {id} 不存在")));
        };
        (
            catalog::list(&conn, id)?,
            catalog::fetched_at(&conn, id)?,
            l.name,
        )
    };
    Ok(Json(serde_json::json!({
        "ok": !entries.is_empty(),
        "channel_id": id.0,
        "channel_name": name,
        "fetched_at": fetched_at,
        "count": entries.len(),
        "models": entries.iter().map(|e| e.model.clone()).collect::<Vec<_>>(),
        "info": entries.iter().map(|e| serde_json::json!({
            "id": e.model,
            "reasoning":e.reasoning,
            "kind": e.kind,
            "owned_by": e.owned_by,
            "context_window": e.context_window,
            "price_note": e.price_note,
            "is_free": e.is_free.unwrap_or(false),
            "vision": e.vision,
        })).collect::<Vec<_>>(),
        "note": if entries.is_empty() {
            "还没问过这条上游有哪些模型（或者上次没问到）".to_string()
        } else {
            format!("这是上次问到的 {} 个模型名（点「问一次上游」可以重新问一遍）", entries.len())
        },
        "saved": true,
        "added": Vec::<String>::new(),
        "removed": Vec::<String>::new(),
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
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<VisionTestBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = tokengo_shared::ChannelId(id);
    let Some(ready) = ready_channel(&state, id)? else {
        return Err(ApiError::not_found(format!("渠道 {id} 不存在")));
    };
    let ReadyChannel {
        channel, secret, ..
    } = ready;

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
    // 第一发给 64 token。**被截断就再给 256 重问一次** ——
    // 「话没说完」是我们额度给少了，不能算模型看不见（8 token 那版就是这么冤枉模型的）。
    let mut budget = crate::prober::VISION_TEST_TOKENS;
    let mut r = state
        .upstream
        .call_and_read(
            &channel,
            secret.as_deref(),
            crate::gateway::EndpointKind::Chat.path(),
            crate::prober::vision_test_body(&sent, budget),
            timeout,
            &crate::upstream::new_uuid(),
            now_ms(),
        )
        .await;
    let mut latency = now_ms() - t0;
    let mut retried_with_more_tokens = false;
    if let Ok((code, text)) = &r {
        let said_red = crate::prober::answer_says_red(&extract_reply(text));
        if (200..300).contains(code) && !said_red && crate::prober::reply_was_truncated(text) {
            budget = crate::prober::VISION_TEST_TOKENS_RETRY;
            retried_with_more_tokens = true;
            let t1 = now_ms();
            r = state
                .upstream
                .call_and_read(
                    &channel,
                    secret.as_deref(),
                    crate::gateway::EndpointKind::Chat.path(),
                    crate::prober::vision_test_body(&sent, budget),
                    timeout,
                    &crate::upstream::new_uuid(),
                    now_ms(),
                )
                .await;
            latency = now_ms() - t1;
        }
    }

    let (ok, status, reply, note) = match r {
        Ok((code, text)) => {
            let reply = extract_reply(&text);
            if !(200..300).contains(&code) {
                let note = match code {
                    400 | 404 | 422 => format!(
                        "上游不收这张图（HTTP {code}）：这个模型多半不支持图像输入。它的原话：{}",
                        snippet(&text)
                    ),
                    401 | 403 => {
                        format!("凭据不对（HTTP {code}）—— 先点「测一下」看看这条渠道本身通不通")
                    }
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
                let why = if crate::prober::reply_was_truncated(&text) {
                    format!("（给了 {budget} token 还是被截断，一句话都没说完）")
                } else {
                    String::new()
                };
                (
                    false,
                    Some(code),
                    reply,
                    format!(
                        "上游收了图但回了个空回复{why}，看不出它到底看没看见 —— 别勾，先用一张真图在客户端里试一次"
                    ),
                )
            } else {
                let why = if crate::prober::reply_was_truncated(&text) {
                    format!("而且回复被 {budget} token 截断了（话没说完，不一定是它看不见）")
                } else {
                    String::new()
                };
                let note = format!(
                    "上游收了图，但答的不是红色（它说：{}）{why} —— 多半没真看图，或者这模型认不了这张图。别勾。",
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
        // 用了多少额度、有没有因为截断重问一次 —— 面板把这句写出来，省得人以为一次就灵
        "max_tokens": budget,
        "retried_with_more_tokens": retried_with_more_tokens,
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

/// 面板「模型」页要的全部事实：一名字一行 + 它后面那串渠道（按先试顺序）+ 路由状态，
/// 外加 `auto` 的候选顺序。**面板不再自己算行** —— 以前面板按「渠道里勾过的名字」算、
/// 对外清单按模型组算，两处各算各的就会漂移（v0.11.0 起只有这一份）。
///
/// `auto` 三类的候选各给一份：面板上点开 auto 就能看见「这一刻会挑中谁」。
#[derive(Deserialize)]
struct ModelEnabledBody {
    model: String,
    enabled: bool,
}
async fn model_enabled_save(_auth:ManagementAuth,State(state):State<SharedState>,Json(body):Json<ModelEnabledBody>)->Result<Json<serde_json::Value>,ApiError>{
 let c=state.conn();if model_routes::routes_for(&c,&body.model)?.is_none(){return Err(ApiError::not_found("模型未勾选"));}
 model_routes::set_enabled(&c,&body.model,body.enabled)?;
 Ok(Json(serde_json::json!({"ok":true,"enabled":body.enabled})))
}

async fn auto_policy_get(_auth:ManagementAuth,State(state):State<SharedState>)->Result<Json<serde_json::Value>,ApiError>{let c=state.conn();Ok(Json(serde_json::json!({"policy":crate::db::policy::get(&c)?,"order":crate::db::policy::ordered(&c)?})))}
async fn auto_policy_save(_auth:ManagementAuth,State(state):State<SharedState>,Json(body):Json<crate::db::policy::Policy>)->Result<Json<serde_json::Value>,ApiError>{let c=state.conn();crate::db::policy::save(&c,&body).map_err(|e|ApiError::bad_request(e.to_string()))?;Ok(Json(serde_json::json!({"ok":true})))}
#[derive(Deserialize)]
struct OrderMove {
    model: String,
    up: bool,
}
async fn model_order_move(_auth:ManagementAuth,State(state):State<SharedState>,Json(body):Json<OrderMove>)->Result<Json<serde_json::Value>,ApiError>{let c=state.conn();crate::db::policy::move_model(&c,&body.model,body.up)?;Ok(Json(serde_json::json!({"ok":true})))}
#[derive(Deserialize)]
struct CoolReset {
    model: Option<String>,
    channel_id: Option<i64>,
}
async fn model_cooldowns(_auth:ManagementAuth,State(state):State<SharedState>)->Result<Json<serde_json::Value>,ApiError>{
 let c=state.conn();
 // 表里存的是归一化后的名字（连字符/下划线都成了空格），面板上要看的是**真名**：
 // 拿勾选过的模型名对一遍，对得上就换成真名（对不上就原样给，不能凭空造名字）。
 let real:Vec<String>=crate::db::model_routes::list(&c)?.into_iter().map(|r|r.name).collect();
 let mut rows=crate::db::policy::active(&c)?;
 for r in rows.iter_mut(){if let Some(n)=real.iter().find(|x|tokengo_shared::same_model_name(x,&r.model)){r.model=n.clone();}}
 Ok(Json(serde_json::json!({"cooldowns":rows,"faults":crate::db::faults::rows(&c)?,"now_ms":now_ms()})))
}
#[derive(Deserialize)]
struct FaultRetry {
    channel_id: i64,
    #[serde(default)]
    model: String,
}
async fn channel_fault_retry(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Json(body): Json<FaultRetry>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let c = state.conn();
    crate::db::in_tx(&c, |c| {
        crate::db::faults::retry(c, body.channel_id, &body.model)
    })?;
    Ok(Json(serde_json::json!({"ok":true})))
}
async fn model_cooldown_reset(_auth:ManagementAuth,State(state):State<SharedState>,Json(body):Json<CoolReset>)->Result<Json<serde_json::Value>,ApiError>{
 let c=state.conn();if let Some(name)=body.model {crate::db::policy::reset(&c,&name,body.channel_id).map_err(crate::db::DbError::from)?;} else {
 if body.channel_id.is_some(){return Err(ApiError::bad_request("全部解除不能指定渠道"));}
 c.execute("UPDATE model_cooldowns SET failures=0,until_ms=0,lease_until=0,reason='',generation=generation+1",[]).map_err(crate::db::DbError::from)?;
 }Ok(Json(serde_json::json!({"ok":true})))
}

async fn model_routes_list(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let routes = {
        let conn = state.conn();
        model_routes::list(&conn)?
    };
    let auto = {
        let conn = state.conn();
        let cand = |k: tokengo_shared::ModelKind| {
            let mut names =
                model_routes::auto_candidates(&conn, k, false, now_ms()).unwrap_or_default();
            names.retain(|n| {
                model_routes::channels_for(&conn, n)
                    .unwrap_or_default()
                    .iter()
                    .any(|c| {
                        crate::db::policy::peek(&conn, c.id.0, n, c.upstream_model(n), now_ms())
                            .ok()
                            .flatten()
                            .is_none()
                    })
            });
            let mut p = crate::db::policy::get(&conn).unwrap_or_default();
            p.explore = false;
            let _ = crate::db::policy::rank(
                &conn,
                &mut names,
                &p,
                state
                    .auto_sequence
                    .load(std::sync::atomic::Ordering::Relaxed),
                99,
            );
            if p.strategy == "idle" {
                state.model_loads.preview(&mut names);
            }
            names
        };
        serde_json::json!({
            // 一个勾选过的模型都没有 → 对外不显示 auto（用户口径）
            "available": model_routes::auto_available(&conn)?,
            "chat": cand(tokengo_shared::ModelKind::Chat),
            "embeddings": cand(tokengo_shared::ModelKind::Embeddings),
            "rerank": cand(tokengo_shared::ModelKind::Rerank),
        })
    };
    let scores = {
        let conn = state.conn();
        crate::db::scores::all(&conn, now_ms())?
    };
    let totals = {
        let conn = state.conn();
        crate::db::scores::total_calls(&conn)?
    };
    // Snapshot route state before subsequent per-model DB queries.
    let route_states = state.routing().all_states().clone();
    let rows = routes
        .iter()
        .map(|r| {
            let st = route_states.get(&r.name).cloned().unwrap_or_default();
            let detection_status = {
                let conn=state.conn();
                conn.query_row("SELECT status FROM model_type_checks WHERE model_name=?1 ORDER BY next_at DESC LIMIT 1", [&r.name], |x| x.get::<_,String>(0)).unwrap_or_else(|_| if r.kind.is_set() { String::new() } else { "等待自动识别".into() })
            };
            serde_json::json!({
                "name": r.name,
                "display_name": ({let c=state.conn();crate::db::model_access::display_name(&c,&r.name).unwrap_or_default()}),
                "catalog_purpose": ({let c=state.conn();crate::db::model_access::catalog_purpose(&c,&r.name).unwrap_or_default()}),
                "catalog_purpose_label": ({let c=state.conn();crate::db::model_access::catalog_purpose_label(&crate::db::model_access::catalog_purpose(&c,&r.name).unwrap_or_default())}),
                "visible": ({let c=state.conn();model_routes::visible(&c,&r.name).unwrap_or(false)}),
                "internal_only": ({let c=state.conn();crate::db::model_access::internal_only(&c,&r.name).unwrap_or(true)}),
                "sources": ({let c=state.conn();crate::db::provider_channels::source_json(&c,&r.name).unwrap_or_default()}),
                "enabled": r.enabled,
                "total_calls": totals.get(&tokengo_shared::normalize_model_name(&r.name)).copied().unwrap_or(0),
                "runtime": scores.get(&tokengo_shared::normalize_model_name(&r.name)).cloned().unwrap_or_default(),
                "score_window_ms": crate::db::scores::WINDOW_MS,
                "exploration_count": ({let c=state.conn();c.query_row("SELECT COUNT(*) FROM requests WHERE explored=1 AND served_model=?1",[&r.name],|r|r.get::<_,i64>(0)).unwrap_or(0)}),
                "cooldowns": ({let c=state.conn();crate::db::policy::rows(&c,&r.name).unwrap_or_default()}),
                "kind": r.kind.as_str(),
                "kind_label": r.kind.label(),
                "kind_source": r.kind_origin,
                "kind_conflict": r.kind_conflict,
                "detection_status": detection_status,
                // 有启用的渠道勾了它并标了能看图（只对对话类有意义）
                "can_see": r.can_see(),
                "context_window": r.context_window,
                "nominal_context_window": r.nominal_context_window,
                "nominal_context_source": r.nominal_context_source,
                "context_reason": r.context_reason,
                "context_source": r.context_source.as_deref().unwrap_or_default(),
                // 现在有没有渠道能用（没有 = 客户端打这个名字会 404）
                "routed": r.routed(),
                // 谁勾了它（含停用的渠道）；以及现在真能用的那串，按先试顺序
                "declared_by": r.declared_by.iter().map(|l| l.0).collect::<Vec<_>>(),
                "channels": r.channels.iter().map(|l| l.0).collect::<Vec<_>>(),
                "active_channel_id": st.active.map(|l| l.0),
                "switch_count": st.switch_count,
                "next_probe_at_ms": st.next_probe_at_ms,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({
        "models": rows,
        "shared_virtual_models": crate::db::shared_virtual_routes::list(&state.conn())?,
        "auto": auto,
        "now_ms": now_ms(),
    })))
}

/// 全部「名字 → 用途」。面板一次拿走，省得一个名字一个请求。
async fn model_kinds_list(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let ks = kinds::all(&conn)?;
    Ok(Json(serde_json::json!({
        "kinds": ks.iter().map(|k| serde_json::json!({
            "name": k.name,
            "kind": k.kind.as_str(),
            "kind_label": k.kind.label(),
            "kind_source": k.source.as_str(),
        })).collect::<Vec<_>>(),
    })))
}

/// 给人手动定一个**模型名**的用途（对话 / 嵌入 / 重排 / 图像 / 视频 / 音频），
/// 或交还给实测（`kind: "auto"`）。名字没建模型组也能定 —— 这正是它单独立表的原因。
async fn models_set_kind(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(body): Json<ModelKindBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let kind = parse_kind(&body.kind)?;
    let conn = state.conn();
    // 用途挂在**名字**上（v0.10.0 起），模型组那一层已经没有了（v0.11.0）——
    // 路由、对外清单、面板读的都是这一份。
    kinds::set(&conn, &name, kind)?;
    tracing::info!("模型名 {} 的类型被改成 {}", name.trim(), kind.label());
    Ok(Json(serde_json::json!({
        "ok": true,
        "kind": kind.as_str(),
        "kind_label": kind.label(),
        "kind_source": if kind.is_set() { "manual" } else { "" },
    })))
}

/// 界面上的类型字面值 → `ModelKind`。`auto` / 空 = 交还给实测（清空）。
fn parse_kind(raw: &str) -> Result<tokengo_shared::ModelKind, ApiError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" | "" => Ok(tokengo_shared::ModelKind::Unknown),
        other => {
            let k = tokengo_shared::ModelKind::parse(other);
            if !k.is_set() {
                return Err(ApiError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "invalid_kind",
                    format!(
                        "不认识这个类型：{other}（只能是 auto / chat / embeddings / rerank / image / video / audio）"
                    ),
                ));
            }
            Ok(k)
        }
    }
}

/// 把一条渠道在全局顺序里上移/下移一位（面板「渠道」页的 ▲▼）。
///
/// 顺序全站只有一份（`channels.priority`）：模型组拿掉之后，「这个模型名下先试哪条」
/// 也由它决定 —— 用户只需要在一个地方维护优先级，不用为每个模型名各排一遍。
async fn channels_move(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<MoveBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let cid = tokengo_shared::ChannelId(id);
    if channels::get(&conn, cid)?.is_none() {
        return Err(ApiError::not_found(format!("渠道 {cid} 不存在")));
    }
    channels::reorder(&conn, cid, body.up)?;
    let order: Vec<i64> = channels::list(&conn)?.iter().map(|l| l.id.0).collect();
    Ok(Json(serde_json::json!({ "ok": true, "order": order })))
}

#[derive(Deserialize)]
struct MoveBody {
    /// true = 往上挪一位（更要紧），false = 往下挪一位。
    up: bool,
}

/// 一键启用/停用一条渠道（面板状态开关）。**只改这一列**，密钥/请求头/模型名一个都不碰。
async fn channels_set_enabled(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<EnabledBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let cid = tokengo_shared::ChannelId(id);
    if channels::get(&conn, cid)?.is_none() {
        return Err(ApiError::not_found(format!("渠道 {cid} 不存在")));
    }
    channels::set_enabled(&conn, cid, body.enabled)?;
    let l = channels::get(&conn, cid)?
        .ok_or_else(|| ApiError::internal("刚改完的渠道读不回来"))?;
    tracing::info!("渠道 {cid} 被{}", if body.enabled { "启用" } else { "停用" });
    Ok(Json(serde_json::json!({
        "ok": true,
        "id": l.id.0,
        "enabled": l.enabled,
    })))
}

#[derive(Deserialize)]
struct ModelKindBody {
    /// `auto`（交还给实测）/ `chat` / `embeddings` / `rerank` / `image` / `video` / `audio`。
    kind: String,
}

/// 「一键启用/停用」的请求体（渠道与模型组共用）。
#[derive(Deserialize)]
struct EnabledBody {
    enabled: bool,
}

// ───────────────────────── 路由状态 ─────────────────────────

async fn routing_state(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let routes = {
        let conn = state.conn();
        model_routes::list(&conn)?
    };
    let router = state.routing();
    let mut out = Vec::new();
    for r in routes.iter().filter(|r| r.routed()) {
        let st = router.state(&r.name).cloned().unwrap_or_default();
        // 首选 = 这个名字下第一条可用渠道（渠道顺序 = 先试顺序）
        let primary = r.channels.first().copied();
        out.push(serde_json::json!({
            "model": r.name,
            // 在用的可能是首选，也可能是切过去的某一条
            "primary_channel_id": primary.map(|l| l.0),
            "active_channel_id": st.active.map(|l| l.0),
            "switched": st.active.is_some() && st.active != primary,
            "switch_count": st.switch_count,
            "last_switch_at_ms": st.last_switch_at_ms,
            "next_probe_at_ms": st.next_probe_at_ms,
            "now_ms": now_ms(),
            "channels": r.channels.iter().map(|l| l.0).collect::<Vec<_>>(),
        }));
    }
    Ok(Json(serde_json::json!({
        "config": router.cfg,
        "models": out,
    })))
}

async fn routing_set_config(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Json(cfg): Json<tokengo_shared::routing::RoutingConfig>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if cfg.retry_times > 20 {
        return Err(ApiError::bad_request("重试次数过大（上限 20）"));
    }
    if [cfg.attempt_timeout_ms, cfg.nonstream_timeout_ms, cfg.request_budget_ms].iter().any(|v| *v == 0 || *v > 3_600_000) {
        return Err(ApiError::bad_request("超时与总预算必须在 1～3600000 毫秒之间"));
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

/// 立刻把某个模型名的回切检查点归零 —— 不想等 10 分钟时用（面板上的「立刻试首选」）。
async fn routing_reprobe(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(model): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let next = state.routing().state(&model).cloned().map(|mut st| {st.next_probe_at_ms=Some(now_ms());st});
    {
        let conn = state.conn();
        if model_routes::routes_for(&conn, &model)?.is_none() {
            return Err(ApiError::not_found(format!("模型名 {model} 这里没有")));
        }
        if let Some(next) = &next {
            // Commit under the revocation/audit gate, then release DB before taking routing.
            crate::db::routing::save_state(&conn, &model, next)?;
        }
    }
    if let Some(next) = next {state.routing().restore(&model, next);}
    Ok(Json(serde_json::json!({ "ok": true, "note": "下次请求会先试首选渠道" })))
}

// ───────────────────────── 记账 ─────────────────────────

#[derive(Deserialize)]
struct StatsQuery {
    /// 看最近多少小时，默认 24。
    hours: Option<i64>,
}

async fn stats(
    _auth: ManagementAuth,
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
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let conn = state.conn();
    let rows = requests::recent(&conn, limit)?;
    Ok(Json(serde_json::json!({ "logs": rows })))
}

async fn request_detail(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<requests::RequestDetail>, ApiError> {
    if id <= 0 { return Err(ApiError::bad_request("请求号必须是正整数")); }
    let conn = state.conn();
    let detail = requests::detail(&conn, id)?
        .ok_or_else(|| ApiError::not_found("请求不存在或已按保留期限清理"))?;
    Ok(Json(detail))
}

/// 面板「日志」页：最近的运行日志。`level`=error/warn/info 是门槛，`q` 是关键词（也当请求号用）。
async fn logs_live(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Query(q): Query<LogsLiveQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let lines = state.logs().query(q.level.as_deref(), q.q.as_deref(), limit);
    let ctl = state.log_control();
    Ok(Json(serde_json::json!({
        "lines": lines,
        "kept": state.logs().len(),
        "limit": limit,
        "level": ctl.level(),
        "revert_at_ms": ctl.revert_at_ms(),
        "file": state.log_file().map(|p| p.display().to_string()),
    })))
}

/// 当前日志级别（面板上显示 + 限时 debug 的倒计时）。
async fn logs_level_get(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ctl = state.log_control();
    Ok(Json(serde_json::json!({
        "level": ctl.level(),
        "revert_at_ms": ctl.revert_at_ms(),
    })))
}

/// 改日志级别。`minutes > 0` = 只开这么久，到点自动收回（防着开完 debug 忘了关、
/// 日志把磁盘刷满 —— 这台机器上吃过不设上限的亏）。
async fn logs_level_set(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Json(body): Json<LogLevelBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Serialize revocation with the actual in-memory mutation, not just request admission.
    let _mutation_guard = state.conn();
    if let Some(id) = super::operations::actor_id() {
        if !crate::db::operations_tokens::active(&_mutation_guard, id)? {
            return Err(ApiError::new(axum::http::StatusCode::UNAUTHORIZED, "invalid_operations_token", "Operations credential is inactive"));
        }
        crate::db::operations_tokens::audit(&_mutation_guard, "operations_token", id, "operations.logs.level", "runtime", None, None, true)?;
    }
    let ctl = state.log_control();
    let revert = body.minutes.filter(|m| *m > 0).map(|m| {
        (m, now_ms() + i64::from(m) * 60_000)
    });
    let generation = ctl.configure(&body.level, revert.map(|(_, at)| at))
        .map_err(ApiError::bad_request)?;
    tracing::info!("日志级别改成「{}」（{}）", body.level,
        match revert { Some((m, _)) => format!("{m} 分钟后自动收回"), None => "长期生效".into() });
    if let Some((m, _)) = revert {
        // 到点收回：改成 info（默认档）
        let ctl2 = ctl.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(u64::from(m) * 60)).await;
            match ctl2.expire(generation) {
                Ok(true) => tracing::info!("限时到期，日志级别已自动收回 info"),
                Ok(false) => {},
                Err(e) => tracing::warn!("自动收回日志级别失败：{e}"),
            }
        });
    }
    Ok(Json(serde_json::json!({
        "level": ctl.level(),
        "revert_at_ms": ctl.revert_at_ms(),
    })))
}

#[derive(serde::Deserialize)]
struct LogsLiveQuery {
    level: Option<String>,
    q: Option<String>,
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
struct LogLevelBody {
    level: String,
    /// 多少分钟后自动收回（不填或 0 = 长期生效）。
    minutes: Option<u32>,
}

// ───────────────────────── 设置 ─────────────────────────

async fn settings_get(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let router = state.routing();
    Ok(Json(serde_json::json!({
        "routing_config": router.cfg,
        "gateway_key_hint": sessions::gateway_key_hint(&conn, &state.secrets)?,
        // 能不能一键复制：老库只有哈希时为 false（面板会提示「生成新 key」）。
        "gateway_key_copyable": sessions::gateway_key_plain(&conn, &state.secrets)?.is_some(),
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
        sessions::set_gateway_key(&conn, &state.secrets, k)?;
        state.forget_gateway_key();
        tracing::info!("对外 key 已更换");
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 面板「复制对外 key」：返回明文。必须带面板会话（PanelAuth）。
///
/// 用户要求：「生成一次之后就一直可以用，真实 key 不显示，点一下复制就行」。
/// 所以明文不渲染进页面，只在点复制时取一次。
async fn reveal_gateway_key(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    match sessions::gateway_key_plain(&conn, &state.secrets)? {
        Some(k) => Ok(Json(serde_json::json!({ "gateway_key": k }))),
        None => Err(ApiError::not_found(
            "这把 key 是旧版生成的，库里只有哈希、取不回明文：点「生成新 key」换一把，之后就随时能复制",
        )),
    }
}

async fn generate_gateway_key(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let key = gateway_keys::generate();
    let conn = state.conn();
    sessions::set_gateway_key(&conn, &state.secrets, &key)?;
    state.forget_gateway_key();
    tracing::info!("对外 key 已自动生成");
    Ok(Json(serde_json::json!({
        "ok": true,
        "gateway_key": key,
        "notice": "Default key replaced; other named keys are unchanged.",
    })))
}

// ───────────────────────── 账目维护 ─────────────────────────

async fn maintenance_get(
    _auth: ManagementAuth,
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
    _auth: ManagementAuth,
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
    #[test]
    fn diagnostic_selection_uses_requested_mapping_and_rejects_unavailable_models() {
        use crate::db::channels::ModelMapping;
        let mut off=ModelMapping::new("disabled",Some("disabled-upstream".into()),false);off.enabled=false;
        let rows=vec![off,ModelMapping::new("first",Some("up-first".into()),false),ModelMapping::new("second",Some("up-second".into()),false)];
        assert_eq!(super::diagnostic_model(&rows,Some("second")).unwrap(),Some("up-second".into()));
        assert_eq!(super::diagnostic_model(&rows,None).unwrap(),Some("up-first".into()));
        for name in ["disabled","unknown",""] {assert!(super::diagnostic_model(&rows,Some(name)).is_err());}
    }

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayKeyBody {
    name: String,
}

async fn gateway_keys_list(_auth: ManagementAuth, State(state): State<SharedState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({"keys": gateway_keys::list(&state.conn())?})))
}

async fn gateway_keys_create(
    _auth: ManagementAuth,
    State(state): State<SharedState>,
    Json(body): Json<GatewayKeyBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    gateway_keys::validate_name(&body.name).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let conn = state.conn();
    let key = gateway_keys::generate();
    let row = gateway_keys::create(&conn, &state.secrets, &body.name, &key, false)?;
    let mut value = serde_json::to_value(row).map_err(|_| ApiError::internal("Cannot serialize gateway key"))?;
    if super::operations::actor_id().is_some() { value["secret"] = serde_json::json!(key); }
    Ok(Json(value))
}

async fn gateway_keys_reveal(
    root: super::accounts::RootGrant,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn=state.conn();
    if !root.valid(&conn)? { return Err(ApiError::unauthorized("管理员登录已失效")); }
    let key = gateway_keys::reveal(&conn, &state.secrets, id)?.ok_or_else(|| {
        ApiError::not_found("Key not found or hash-only legacy key cannot be revealed")
    })?;
    crate::db::in_tx(&conn,|c|crate::db::identity::audit(c,None,"key.reveal","key",id,serde_json::json!({"actor_account_id":root.actor_account_id(),"source":"legacy_gateway_key_reveal"})))?;
    Ok(Json(serde_json::json!({"gateway_key":key})))
}

async fn gateway_keys_delete(_auth: ManagementAuth, State(state): State<SharedState>, Path(id): Path<i64>) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let row=crate::db::identity::key(&conn,id)?.ok_or_else(||ApiError::not_found("Key not found"))?;
    if row.user_id!=1 && row.enabled && row.revoked_at.is_none() {
        return Err(ApiError::new(StatusCode::CONFLICT,"key_not_revoked","请先停用或撤销密钥，再从列表删除"));
    }
    crate::db::in_tx(&conn,|c| {
        // Legacy Settings credentials retain one-step deletion, but public deletion
        // must preserve credential ancestry and financial/audit history too.
        if row.user_id==1 && !crate::db::identity::key_deleted(c,id)? { crate::db::identity::revoke(c,id)?; }
        if crate::db::identity::delete_key(c,id)? {
            crate::db::identity::audit(c,None,"key.delete","key",id,serde_json::json!({"user_id":row.user_id,"source":"legacy_gateway_key_delete","operations_token_id":super::operations::actor_id(),"physical_record_retained":true}))?;
        }
        Ok(())
    })?;
    state.forget_gateway_key();
    Ok(Json(serde_json::json!({"deleted":true})))
}
