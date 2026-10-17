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
use crate::db::{groups, legs, requests, routing, sessions, settings, now_ms};
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
        .route("/legs", get(legs_list).post(legs_create))
        .route("/legs/{id}", put(legs_update).delete(legs_delete))
        .route("/legs/{id}/probe", post(legs_probe))
        .route("/legs/{id}/models", post(legs_models))
        .route("/groups", get(groups_list).post(groups_upsert))
        .route("/groups/{name}", delete(groups_delete))
        .route("/routing", get(routing_state))
        .route("/routing/config", put(routing_set_config))
        .route("/routing/{group}/reprobe", post(routing_reprobe))
        .route("/stats", get(stats))
        .route("/logs", get(logs))
        .route("/timeline", get(|| async { unbuilt(5, "GET /api/timeline") }))
        .route("/settings", get(settings_get).put(settings_put))
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

// ───────────────────────── 腿 ─────────────────────────

fn leg_json(l: &legs::LegRow) -> serde_json::Value {
    serde_json::json!({
        "id": l.id.0,
        "name": l.name,
        "base_url": l.base_url,
        "enabled": l.enabled,
        "has_secret": l.has_secret,
        "headers": l.headers.iter().map(|(n, v)| serde_json::json!({"name": n, "value": v})).collect::<Vec<_>>(),
        "models": l.models.iter().map(|(m, u)| serde_json::json!({"model": m, "upstream": u})).collect::<Vec<_>>(),
    })
}

async fn legs_list(
    _auth: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let all = legs::list(&conn)?;
    Ok(Json(serde_json::json!({
        "legs": all.iter().map(leg_json).collect::<Vec<_>>()
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
}

#[derive(Deserialize)]
struct LegBody {
    name: String,
    base_url: String,
    #[serde(default = "default_true")]
    enabled: bool,
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

impl LegBody {
    fn headers(&self) -> Vec<(String, String)> {
        self.headers
            .iter()
            .filter(|h| !h.name.trim().is_empty())
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect()
    }

    fn models(&self) -> Vec<(String, Option<String>)> {
        self.models
            .iter()
            .filter(|m| !m.model.trim().is_empty())
            .map(|m| (m.model.clone(), m.upstream.clone()))
            .collect()
    }
}

async fn legs_create(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(body): Json<LegBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    // 四条写入（主行 + 密钥 + 请求头 + 模型名）必须一起成或一起败，
    // 否则会留下"名字在、请求头丢了"的半截腿。
    let id = crate::db::in_tx(&conn, |c| {
        let id = legs::create(
            c,
            &legs::NewLeg {
                name: body.name.clone(),
                base_url: body.base_url.clone(),
                enabled: body.enabled,
            },
        )?;
        if let Some(k) = body.key.as_deref().filter(|k| !k.trim().is_empty()) {
            legs::set_secret(c, &state.secrets, id, k)?;
        }
        legs::set_headers(c, id, &body.headers())?;
        legs::set_models(c, id, &body.models())?;
        Ok(id)
    })?;
    tracing::info!("新建腿 {id}：{}", body.name);

    let l = legs::get(&conn, id)?.ok_or_else(|| ApiError::internal("刚建的腿读不回来"))?;
    Ok(Json(leg_json(&l)))
}

async fn legs_update(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<LegBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let id = tokengo_shared::LegId(id);
    crate::db::in_tx(&conn, |c| {
        legs::update(c, id, &body.name, &body.base_url, body.enabled)?;
        // 密钥：传了才改；传空串 = 明确要清掉
        match body.key.as_deref() {
            Some("") => legs::clear_secret(c, id)?,
            Some(k) if !k.trim().is_empty() => legs::set_secret(c, &state.secrets, id, k)?,
            _ => {}
        }
        legs::set_headers(c, id, &body.headers())?;
        legs::set_models(c, id, &body.models())?;
        Ok(())
    })?;
    tracing::info!("更新腿 {id}：{}", body.name);

    let l = legs::get(&conn, id)?.ok_or_else(|| ApiError::not_found(format!("腿 {id} 不存在")))?;
    Ok(Json(leg_json(&l)))
}

async fn legs_delete(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    legs::delete(&conn, tokengo_shared::LegId(id))?;
    tracing::info!("删除腿 #{id}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ProbeQuery {
    /// 探活打哪个端点：chat（默认）或 embeddings。
    kind: Option<String>,
}

/// 一条"能真发请求"的腿：解密好的密钥 + 自定义头 + 配的模型名。
///
/// 探活和「拉上游模型清单」都要这一套。抽出来是因为这两件事的加载逻辑一模一样，
/// 各写一份迟早会有一边忘了解密密钥（现象是"这条腿明明有 key，面板却说没有"）。
struct ReadyLeg {
    leg: tokengo_shared::Leg,
    secret: Option<String>,
    has_secret: bool,
    models: Vec<(String, Option<String>)>,
}

fn ready_leg(state: &SharedState, id: tokengo_shared::LegId) -> Result<Option<ReadyLeg>, ApiError> {
    let conn = state.conn();
    let Some(row) = legs::get(&conn, id)? else {
        return Ok(None);
    };
    let secret = legs::secret(&conn, &state.secrets, id)?;
    Ok(Some(ReadyLeg {
        leg: tokengo_shared::Leg {
            id: row.id,
            name: row.name.clone(),
            base_url: row.base_url.clone(),
            enabled: row.enabled,
            priority: 0,
            model_alias: None,
            headers: row.headers.clone(),
        },
        secret,
        has_secret: row.has_secret,
        models: row.models.clone(),
    }))
}

/// 面板上的「测一下」：**真发一个请求到这条腿**，回的绿点才是真的。
///
/// 三条纪律：
/// - 探活**不写记账**（它不是用户请求）；
/// - 探活**不动路由状态**（面板上的一次手动检查凭据，不该改变谁在用）；
/// - 模型名是这条腿自己配的那个，拿不到就明说"探不了"，不猜（拿 `auto` 去探必然 400）。
async fn legs_probe(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(q): Query<ProbeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = tokengo_shared::LegId(id);
    let endpoint = match q.kind.as_deref() {
        Some("embeddings") | Some("embedding") | Some("vec") => crate::gateway::EndpointKind::Embeddings,
        _ => crate::gateway::EndpointKind::Chat,
    };

    let Some(ready) = ready_leg(&state, id)? else {
        return Err(ApiError::not_found(format!("腿 {id} 不存在")));
    };
    let ReadyLeg {
        leg,
        secret,
        has_secret,
        models,
    } = ready;

    let model = models
        .iter()
        .find_map(|(_, upstream)| upstream.clone().filter(|u| !u.trim().is_empty()))
        .or_else(|| {
            models
                .iter()
                .map(|(name, _)| name.clone())
                .find(|n| !n.trim().is_empty())
        });

    let Some(model) = model else {
        return Ok(Json(serde_json::json!({
            "ok": false,
            "leg_id": leg.id.0,
            "endpoint": endpoint.as_str(),
            "note": "这条腿没配任何模型名，没法探活：面板里先给它填一个「该腿认的模型名」。拿 auto 去探必然被上游打回 400，绿点就不真了。",
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
            &leg,
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
                tokengo_shared::routing::AttemptOutcome::LegFault => {
                    format!("这条腿自己的毛病（HTTP {code}）")
                }
                _ => format!("上游不接受这个请求（HTTP {code}），多半是模型名或参数不对"),
            };
            (kind == tokengo_shared::routing::AttemptOutcome::Success, Some(code), note)
        }
        Err(fault) => (false, None, fault.message),
    };

    Ok(Json(serde_json::json!({
        "ok": ok,
        "leg_id": leg.id.0,
        "leg_name": leg.name,
        "endpoint": endpoint.as_str(),
        "model_sent": model,
        "http_status": status,
        "latency_ms": latency,
        "has_secret": has_secret,
        "note": note,
    })))
}

/// 面板上的「拉取上游模型清单」：用这条腿自己的 URL、key、自定义头真打
/// `GET {base_url}/models`，把上游说它支持的模型名列出来。
///
/// 和探活同纪律：**不写记账、不动路由状态** —— 它是面板上的一次人工查询，
/// 不是用户请求，不该污染统计（否则"请求总数"会莫名其妙涨）。
///
/// 如实报三种失败，绝不给一份编出来的清单：
/// - 连不上/超时 → 说清是哪个 URL；
/// - 上游回了非 2xx → 把状态码报出来（404 很常见：这个上游压根没做 /v1/models）；
/// - 2xx 但格式不像模型清单 → 说"没认出"，并附上正文开头让人自己看。
async fn legs_models(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = tokengo_shared::LegId(id);
    let Some(ready) = ready_leg(&state, id)? else {
        return Err(ApiError::not_found(format!("腿 {id} 不存在")));
    };
    let ReadyLeg { leg, secret, .. } = ready;

    let timeout = {
        let cfg = state.routing().cfg.clone();
        std::time::Duration::from_millis(cfg.attempt_timeout_ms.max(1))
            .min(std::time::Duration::from_secs(15))
    };
    let t0 = now_ms();
    let r = state
        .upstream
        .fetch_models(&leg, secret.as_deref(), timeout, &crate::upstream::new_uuid(), now_ms())
        .await;
    let latency = now_ms() - t0;
    let url = crate::upstream::join_url(&leg.base_url, "models");

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
        "拉腿 {id} 的上游模型清单：{}（{} 个，{latency} ms）",
        if ok { "成功" } else { "失败" },
        models.len()
    );

    Ok(Json(serde_json::json!({
        "ok": ok,
        "leg_id": leg.id.0,
        "leg_name": leg.name,
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
    let t = body.trim();
    let cut = t.char_indices().nth(300).map(|(i, _)| i).unwrap_or(t.len());
    let head = t[..cut].replace('\n', " ");
    if cut < t.len() {
        format!("{head}…")
    } else {
        head
    }
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
            "legs": g.legs.iter().map(|l| l.0).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        // 只列真实能用的模型名，给 /v1/models 和面板用同一份判断
        "servable": groups::list_servable(&conn)?,
    })))
}

#[derive(Deserialize)]
struct GroupBody {
    name: String,
    #[serde(default = "default_true")]
    enabled: bool,
    /// 顺序即优先级，第一个是首选。
    legs: Vec<i64>,
}

async fn groups_upsert(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(body): Json<GroupBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.conn();
    let ids: Vec<tokengo_shared::LegId> = body.legs.iter().map(|i| tokengo_shared::LegId(*i)).collect();
    groups::upsert(&conn, &body.name, body.enabled, &ids)?;
    tracing::info!("保存模型组 {}：{} 条腿", body.name, ids.len());
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
        let primary = g.legs.first().copied();
        out.push(serde_json::json!({
            "group": g.name,
            "enabled": g.enabled,
            // 首选=列表第一条；在用的可能是它也可能是切过去的某条
            "primary_leg_id": primary.map(|l| l.0),
            "active_leg_id": st.active.map(|l| l.0),
            "switched": st.active.is_some() && st.active != primary,
            "switch_count": st.switch_count,
            "last_switch_at_ms": st.last_switch_at_ms,
            "next_probe_at_ms": st.next_probe_at_ms,
            "now_ms": now_ms(),
            "legs": g.legs.iter().map(|l| l.0).collect::<Vec<_>>(),
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
    Ok(Json(serde_json::json!({ "ok": true, "note": "下次请求会先试首选腿" })))
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

    // 附上腿名，面板不用再查一次
    let names: std::collections::HashMap<i64, String> = legs::list(&conn)?
        .into_iter()
        .map(|l| (l.id.0, l.name))
        .collect();
    let mut v = serde_json::to_value(&s).map_err(|e| ApiError::internal(e.to_string()))?;
    if let Some(arr) = v.get_mut("by_leg").and_then(|x| x.as_array_mut()) {
        for item in arr.iter_mut() {
            let id = item.get("leg_id").and_then(|x| x.as_i64()).unwrap_or(0);
            let name = names.get(&id).cloned().unwrap_or_else(|| format!("已删除的腿 #{id}"));
            if let Some(obj) = item.as_object_mut() {
                obj.insert("leg_name".into(), serde_json::json!(name));
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
