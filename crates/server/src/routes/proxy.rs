//! Authenticated proxy routes for native Chat, Responses and Messages plus embeddings/reranking.
//!
//! 端点取舍依据（3 天 10 万次请求实测）：
//!
//! ```text
//! POST /v1/chat/completions   13,857 次（13.9%）
//! POST /v1/embeddings         86,146 次（86.1%）
//! POST /v1/rerank             重排（Hindsight 等调用方用）
//! GET  /v1/models             面板与客户端发现用
//! ```
//!
//! 这里只做三件事：**认对外 key**、**拆出模型名与 stream 开关**、**转交给 gateway**。
//! 重试/换渠道/记账/回切都在 `crate::gateway`，别往这个文件里塞业务。
//!
//! 鉴权用对外统一 key（`Authorization: Bearer <key>`）。**没配 key 就一律拒绝**：
//! 少配一次 key 就变成公网裸奔的转发器，这种"默认开放"的坑不能留。

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;

use super::{client_ip, ApiError};
use crate::db::model_routes;
use crate::gateway::{self, EndpointKind, ForwardRequest};
use crate::SharedState;
use tokengo_shared::ModelKind;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/chat/completions", post(chat_completions))
        .route("/responses", post(responses))
        .route("/messages", post(messages))
        .route("/embeddings", post(embeddings))
        .route("/rerank", post(rerank))
        .route("/tools/search", post(search))
        .route("/tools/fetch", post(fetch))
        .route("/models", get(models))
}

// ───────────────────────── 鉴权 ─────────────────────────

/// 对外 key 的门。放在 handler 参数里即生效。
pub struct GatewayKey { pub namespace: String, pub auth: crate::db::identity::AuthContext }

impl FromRequestParts<SharedState> for GatewayKey {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let key = gateway_credential(&parts.headers)?;
        let auth = state.authenticate_context(key.clone()).await;
        match auth {
            Some(auth) => Ok(GatewayKey { namespace: tokengo_shared::short_hash(&key), auth }),
            None => {
                tracing::warn!("Invalid gateway credential from {}", client_ip(&parts.headers).unwrap_or_else(|| "unknown".into()));
                Err(ApiError::unauthorized("Invalid or inactive gateway key"))
            }
        }
    }
}

/// Resolve one local credential. Never fall back from an invalid supplied header
/// to another identity, and never forward either client authentication header.
fn gateway_credential(headers: &HeaderMap) -> Result<String, ApiError> {
    fn single<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ApiError> {
        let mut values = headers.get_all(name).iter();
        let value = values.next();
        if values.next().is_some() { return Err(ApiError::bad_request("Repeated authentication header")); }
        value.map(|value| value.to_str().map_err(|_|ApiError::unauthorized("Invalid authentication header"))).transpose()
    }
    let bearer = single(headers, "authorization")?.map(|raw| {
        raw.strip_prefix("Bearer ").or_else(||raw.strip_prefix("bearer "))
            .map(str::trim).filter(|key| !key.is_empty())
            .ok_or_else(||ApiError::unauthorized("Invalid Authorization header"))
    }).transpose()?;
    let api_key = single(headers, "x-api-key")?.map(|raw| {
        let key = raw.trim();
        if key.is_empty() { Err(ApiError::unauthorized("Invalid x-api-key header")) } else { Ok(key) }
    }).transpose()?;
    match (bearer,api_key) {
        (Some(a),Some(b)) if a != b => Err(ApiError::bad_request("Conflicting authentication headers")),
        (Some(key),_)|(_,Some(key)) => Ok(key.to_string()),
        _ => Err(ApiError::unauthorized("A local gateway key is required")),
    }
}

/// Messages uses the same credential/identity verifier, with its native error envelope.
struct MessagesKey(GatewayKey);
impl FromRequestParts<SharedState> for MessagesKey {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, Response> {
        GatewayKey::from_request_parts(parts,state).await.map(Self).map_err(messages_error)
    }
}
fn messages_error(error: ApiError) -> Response {
    let kind = match error.status.as_u16() {
        400|422 => "invalid_request_error", 401 => "authentication_error", 403 => "permission_error",
        404 => "not_found_error", 413 => "request_too_large", 429 => "rate_limit_error",
        503|529 => "overloaded_error", _ => "api_error",
    };
    (error.status,Json(serde_json::json!({"type":"error","error":{"type":kind,"message":error.message}}))).into_response()
}

async fn forward_native(
    key: GatewayKey, state: SharedState, headers: HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    body: axum::body::Bytes, endpoint: EndpointKind,
) -> Result<Response, ApiError> {
    let (model,value,stream) = parse_body(&body,endpoint)?;
    let explicit_session = explicit_session_id(&headers,&value)?;
    let client_ip = client_ip(&headers).or_else(||peer.map(|p|p.0.0.ip().to_string()));
    gateway::forward(state,ForwardRequest {
        endpoint,model,body:value,stream,client_ip,session_namespace:key.namespace,
        auth:Some(key.auth),explicit_session,
        user_agent:headers.get("user-agent").and_then(|v|v.to_str().ok()).map(str::to_string),
    }).await
}
async fn responses(
    key: GatewayKey, State(state): State<SharedState>, headers: HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    forward_native(key,state,headers,peer,body,EndpointKind::Responses).await
}
async fn forward_tool(key:GatewayKey,state:SharedState,headers:HeaderMap,peer:Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,body:axum::body::Bytes,endpoint:EndpointKind)->Result<Response,ApiError> {
    let mut value:Value=serde_json::from_slice(&body).map_err(|_|ApiError::bad_request("Tool request requires valid JSON"))?;
    let map=value.as_object_mut().ok_or_else(||ApiError::bad_request("Tool request requires a JSON object"))?;
    let default=if endpoint==EndpointKind::Search {"deepseek-search"}else{"deepseek-fetch"};
    let model=match map.get("model") {None=>default.to_string(),Some(Value::String(name)) if !name.trim().is_empty()&&name.len()<=256&&!name.chars().any(char::is_control)=>name.clone(),_=>return Err(ApiError::bad_request("Invalid tool route name"))};
    if model_routes::is_auto(&model) {return Err(ApiError::bad_request("Search and Fetch require an explicit tool route"));}
    crate::search_tools::normalize_request(endpoint.tool_kind().unwrap(),&value).map_err(ApiError::bad_request)?;
    gateway::forward(state,ForwardRequest{endpoint,model,body:value,stream:false,client_ip:client_ip(&headers).or_else(||peer.map(|p|p.0.0.ip().to_string())),session_namespace:key.namespace,auth:Some(key.auth),explicit_session:None,user_agent:headers.get("user-agent").and_then(|v|v.to_str().ok()).map(str::to_string)}).await
}
async fn search(key:GatewayKey,State(state):State<SharedState>,headers:HeaderMap,peer:Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,body:axum::body::Bytes)->Result<Response,ApiError>{forward_tool(key,state,headers,peer,body,EndpointKind::Search).await}
async fn fetch(key:GatewayKey,State(state):State<SharedState>,headers:HeaderMap,peer:Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,body:axum::body::Bytes)->Result<Response,ApiError>{forward_tool(key,state,headers,peer,body,EndpointKind::Fetch).await}
async fn messages(
    MessagesKey(key): MessagesKey, State(state): State<SharedState>, headers: HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    body: Result<axum::body::Bytes,axum::extract::rejection::BytesRejection>,
) -> Response {
    let body = match body {
        Ok(body) => body,
        Err(rejection) => return messages_error(ApiError::new(rejection.status(),"invalid_request_error","Unable to read request body")),
    };
    match forward_native(key,state,headers,peer,body,EndpointKind::Messages).await {
        Ok(response) => response,
        Err(error) => messages_error(error),
    }
}

// ───────────────────────── 端点 ─────────────────────────

async fn chat_completions(
    _key: GatewayKey,
    State(state): State<SharedState>,
    headers: HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let (model, value, stream) = parse_body(&body, EndpointKind::Chat)?;
    let explicit_session = explicit_session_id(&headers, &value)?;
    let ip_raw = client_ip(&headers).or_else(|| peer.map(|p|p.0.0.ip().to_string()));
    let ip = ip_raw.clone().unwrap_or_else(|| "未知来源".into());
    // 进来就记一条 debug：平常(只记 info)看不到，开 15 分钟 debug 就能看到每一发；
    // 失败的会走下面的 warn —— 用户排查时最常问的就是"这一发为什么没成"。
    tracing::debug!("收到对话请求：模型「{model}」流式={stream} 来自 {ip}");
    let out = gateway::forward(
        state,
        ForwardRequest {
            endpoint: EndpointKind::Chat,
            model: model.clone(),
            body: value,
            stream,
            client_ip: ip_raw.clone(),
            session_namespace: _key.namespace,
            auth: Some(_key.auth),
            explicit_session,
            user_agent: headers.get("user-agent").and_then(|v|v.to_str().ok()).map(str::to_string),
        },
    )
    .await;
    if let Err(e) = &out {
        tracing::warn!(
            "对话请求没成：模型「{model}」→ HTTP {} {}（来自 {ip}）：{}",
            e.status.as_u16(),
            e.kind,
            e.message
        );
    }
    out
}

async fn embeddings(
    _key: GatewayKey,
    State(state): State<SharedState>,
    headers: HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let (model, value, stream) = parse_body(&body, EndpointKind::Embeddings)?;
    let explicit_session = explicit_session_id(&headers, &value)?;
    let ip_raw = client_ip(&headers).or_else(|| peer.map(|p|p.0.0.ip().to_string()));
    let ip = ip_raw.clone().unwrap_or_else(|| "未知来源".into());
    // 进来就记一条 debug：平常(只记 info)看不到，开 15 分钟 debug 就能看到每一发；
    // 失败的会走下面的 warn —— 用户排查时最常问的就是"这一发为什么没成"。
    tracing::debug!("收到向量请求：模型「{model}」流式={stream} 来自 {ip}");
    let out = gateway::forward(
        state,
        ForwardRequest {
            endpoint: EndpointKind::Embeddings,
            model: model.clone(),
            body: value,
            stream,
            client_ip: ip_raw.clone(),
            session_namespace: _key.namespace,
            auth: Some(_key.auth),
            explicit_session,
            user_agent: headers.get("user-agent").and_then(|v|v.to_str().ok()).map(str::to_string),
        },
    )
    .await;
    if let Err(e) = &out {
        tracing::warn!(
            "向量请求没成：模型「{model}」→ HTTP {} {}（来自 {ip}）：{}",
            e.status.as_u16(),
            e.kind,
            e.message
        );
    }
    out
}

async fn rerank(
    _key: GatewayKey,
    State(state): State<SharedState>,
    headers: HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let (model, value, stream) = parse_body(&body, EndpointKind::Rerank)?;
    let explicit_session = explicit_session_id(&headers, &value)?;
    let ip_raw = client_ip(&headers).or_else(|| peer.map(|p|p.0.0.ip().to_string()));
    let ip = ip_raw.clone().unwrap_or_else(|| "未知来源".into());
    // 进来就记一条 debug：平常(只记 info)看不到，开 15 分钟 debug 就能看到每一发；
    // 失败的会走下面的 warn —— 用户排查时最常问的就是"这一发为什么没成"。
    tracing::debug!("收到重排请求：模型「{model}」流式={stream} 来自 {ip}");
    let out = gateway::forward(
        state,
        ForwardRequest {
            endpoint: EndpointKind::Rerank,
            model: model.clone(),
            body: value,
            stream,
            client_ip: ip_raw.clone(),
            session_namespace: _key.namespace,
            auth: Some(_key.auth),
            explicit_session,
            user_agent: headers.get("user-agent").and_then(|v|v.to_str().ok()).map(str::to_string),
        },
    )
    .await;
    if let Err(e) = &out {
        tracing::warn!(
            "重排请求没成：模型「{model}」→ HTTP {} {}（来自 {ip}）：{}",
            e.status.as_u16(),
            e.kind,
            e.message
        );
    }
    out
}
/// 模型清单。**只列真的能用的**（有勾过它、而且是启用的渠道）—— 与面板「模型」页、
/// 与实际路由看的是**同一份判断**（`db::model_routes`），不会再出现"清单里有、打了 404"。
///
/// 每条多一个 `vision` 字段：这个模型名后面有没有"能看图"的渠道。客户端（Hermes
/// 之类）可以据此决定要不要把图发过来 —— 带图的请求打到一个没有视觉渠道的模型名
/// 上会被回 422（`code: no_vision_model`）。这是标准 OpenAI 字段之外的增项，
/// 老客户端忽略它就行，行为不变。
///
/// 保留名 `auto`：**只有用户勾选过至少一个模型时才列它**（用户口径：勾选使用的模型
/// 为 0 时，客户端则不显示 auto）。它随请求的端点自己挑（对话 / 嵌入 / 重排），
/// 清单里就给它标**这一刻最能干的那一类**（对话 > 嵌入 > 重排）—— 客户端普遍按类型
/// 过滤清单，标成「没有类型」会被整条滤掉（2026-10-18 实测：某客户端只留 `kind=chat` 的行）。
///
/// 这一条只写标准字段 + `vision` / `kind` 两个增项：客户端各写各的解析，多出来的
/// 自定义字段（早先这里塞过 `auto` / `picks`）会让整条被丢掉。候选顺序改从
/// `GET /api/model-routes` 的 `auto` 对象读，面板「模型」页看的就是它。
async fn models(
    _key: GatewayKey,
    State(state): State<SharedState>,
) -> Result<Json<Value>, ApiError> {
    // OpenAI 这条清单里 created 是「这个模型什么时候有的」；我们现取，客户端普遍只拿它排序。
    let created = crate::db::now_ms() / 1000;
    let (servable, auto_rows) = {
        let conn = state.conn();
        let mut servable = model_routes::servable(&conn)?;
        let mut allowed = Vec::new();
        for model in servable.drain(..) {
            if model_routes::visible(&conn, &model.name)?
                && gateway::permitted_model(&conn, &_key.auth, &model.name)?
            {
                allowed.push(model);
            }
        }
        let servable = allowed;
        let auto_rows = if _key.auth.permits_model(model_routes::AUTO_NAME)
            && model_routes::auto_available(&conn)?
        {
            let now = crate::db::now_ms();
            let chat = model_routes::auto_candidates(&conn, ModelKind::Chat, false, now)?
                .iter()
                .any(|n| {
                    servable
                        .iter()
                        .any(|m| tokengo_shared::same_model_name(n, &m.name))
                });
            let embeddings =
                model_routes::auto_candidates(&conn, ModelKind::Embeddings, false, now)?
                    .iter()
                    .any(|n| {
                        servable
                            .iter()
                            .any(|m| tokengo_shared::same_model_name(n, &m.name))
                    });
            let rerank = model_routes::auto_candidates(&conn, ModelKind::Rerank, false, now)?
                .iter()
                .any(|n| {
                    servable
                        .iter()
                        .any(|m| tokengo_shared::same_model_name(n, &m.name))
                });
            let vision = model_routes::auto_candidates(&conn, ModelKind::Chat, true, now)?
                .iter()
                .any(|n| {
                    servable
                        .iter()
                        .any(|m| tokengo_shared::same_model_name(n, &m.name))
                });
            let kind = if chat {
                ModelKind::Chat
            } else if embeddings {
                ModelKind::Embeddings
            } else if rerank {
                ModelKind::Rerank
            } else {
                ModelKind::Unknown
            };
            let context = crate::capability_detector::conservative_context(
                servable
                    .iter()
                    .filter(|m| {
                        matches!(
                            m.kind,
                            ModelKind::Chat
                                | ModelKind::Unknown
                                | ModelKind::Embeddings
                                | ModelKind::Rerank
                        )
                    })
                    .map(|m| m.context_window),
            );
            if !chat && !embeddings && !rerank {
                Vec::new()
            } else {
                vec![serde_json::json!({
                    "id": model_routes::AUTO_NAME,
                    "object": "model",
                    "created": created,
                    "owned_by": tokengo_shared::BRAND,
                    // 带图的请求打 auto 只有在这时候才走得通
                    "vision": vision,
                    "kind": kind.as_str(),
                    "context_window": context,
                    "context_length": context,
                    "context_source": context.map(|_| {
                        let sources=servable.iter().filter_map(|m|m.context_source.as_deref()).collect::<std::collections::HashSet<_>>();
                        if sources.len()==1 { *sources.iter().next().unwrap() } else { "user_confirmed+upstream" }
                    }),
                })]
            }
        } else {
            Vec::new()
        };
        (servable, auto_rows)
    };
    let mut data: Vec<Value> = auto_rows;
    data.extend(servable.iter().map(|m| {
        serde_json::json!({
            "id": m.name,
            "object": "model",
            "created": created,
            "owned_by": tokengo_shared::BRAND,
            "vision": m.can_see(),
            // 这是向量模型还是对话模型 —— 客户端（Hermes 这类）可以据此挑端点，
            // 不用靠名字猜（现网那批自定义名字一个特征都没有）。
            "kind": m.kind.as_str(),
            "context_window": m.context_window,
            "context_length": m.context_window,
            "context_source": m.context_source,
        })
    }));
    {
        let conn = state.conn();
        let names =
            crate::db::user_virtual_routes::alias_names(&conn, &_key.auth, crate::db::now_ms())
                .map_err(|error| {
                    ApiError::new(axum::http::StatusCode::FORBIDDEN, error.code, error.message)
                })?;
        for name in names {
            let resolved = crate::db::user_virtual_routes::resolve(
                &conn,
                _key.auth.user_id,
                &name,
                crate::db::now_ms(),
            )
            .map_err(|error| {
                ApiError::new(axum::http::StatusCode::FORBIDDEN, error.code, error.message)
            })?;
            let kind = resolved
                .as_ref()
                .and_then(|route| route.targets.first())
                .and_then(|target| {
                    model_routes::routes_for(&conn, &target.logical_model)
                        .ok()
                        .flatten()
                })
                .map(|route| route.kind)
                .unwrap_or(ModelKind::Unknown);
            data.push(serde_json::json!({"id":name,"object":"model","created":created,"owned_by":"user-private","virtual":true,"kind":kind.as_str()}));
        }
    }
    {
        let c = state.conn();
        let names = crate::db::shared_virtual_routes::alias_names(&c, &_key.auth, crate::db::now_ms())
            .map_err(|e| ApiError::new(axum::http::StatusCode::FORBIDDEN, e.code, e.message))?;
        for shared in crate::db::shared_virtual_routes::list(&c)? {
            if names.iter().any(|n| tokengo_shared::same_model_name(n, &shared.config.name)) {
                data.push(serde_json::json!({"id":shared.config.name,"object":"model","created":created,"owned_by":"administrator-shared","virtual":true,"scope":"shared","kind":shared.kind.as_str()}));
            }
        }
    }
    // Preserve the minimal legacy shape when metadata is unknown.
    for model in &mut data {
        if model["context_window"].is_null() {
            if let Some(object) = model.as_object_mut() {
                for key in ["context_window", "context_length", "context_source"] {
                    object.remove(key);
                }
            }
        }
    }
    Ok(Json(serde_json::json!({ "object": "list", "data": data })))
}

/// Resolve client conversation identity without forwarding arbitrary client headers.
/// Validate every supplied ID, including lower-priority fields, instead of silently
/// turning malformed explicit identity into a heuristic content hash.
fn explicit_session_id(headers: &HeaderMap, body: &Value) -> Result<Option<String>, ApiError> {
    fn accept(chosen: &mut Option<String>, id: &str) -> Result<(), ApiError> {
        if id.trim().is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
            return Err(ApiError::bad_request("会话 ID 必须非空、不超过 256 字节且不含控制字符"));
        }
        if chosen.is_none() { *chosen = Some(id.to_owned()); }
        Ok(())
    }
    let mut chosen = None;
    for name in ["x-opencode-session", "x-claude-code-session-id", "session_id", "x-session-id", "x-conversation-id"] {
        for value in headers.get_all(name) {
            let id = value.to_str().map_err(|_| ApiError::bad_request("会话 ID 请求头不是有效文本"))?;
            accept(&mut chosen, id)?;
        }
    }
    for (object, fields) in [
        (Some(body), ["session_id", "conversation_id"]),
        (body.get("metadata"), ["conversation_id", "session_id"]),
    ] {
        for field in fields {
            if let Some(value) = object.and_then(|object| object.get(field)) {
                let id = value.as_str().ok_or_else(|| ApiError::bad_request("会话 ID 必须是字符串"))?;
                accept(&mut chosen, id)?;
            }
        }
    }
    Ok(chosen)
}

/// 请求体：必须是 JSON 对象且带 model。**模型名写错是客户端的错**，回 400/404，
/// 不重试不换渠道（这条规矩见 `docs/03-路由策略.md` 第四节）。
fn parse_body(body: &[u8], endpoint: EndpointKind) -> Result<(String, Value, bool), ApiError> {
    let value: Value = serde_json::from_slice(body).map_err(|e| {
        ApiError::bad_request(format!("{} 的请求体不是合法 JSON：{e}", endpoint.as_str()))
    })?;
    let model = value
        .get("model")
        .and_then(|m| m.as_str())
        .filter(|m| !m.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("请求体里缺 model 字段"))?
        .to_string();
    let native = matches!(endpoint,EndpointKind::Responses|EndpointKind::Messages);
    if native && (model.len()>256 || model.chars().any(char::is_control)) {
        return Err(ApiError::bad_request("Invalid model name"));
    }
    let stream = match value.get("stream") {
        Some(Value::Bool(stream)) => *stream,
        Some(_) if native => return Err(ApiError::bad_request("stream must be a boolean")),
        _ => false,
    };
    match endpoint {
        EndpointKind::Responses => {
            match value.get("input") {
                Some(Value::String(_)|Value::Array(_)) => {},
                None if ["previous_response_id","conversation"].iter().any(|field|value.get(*field).is_some_and(|v|!v.is_null())) => {},
                _ => return Err(ApiError::bad_request("Responses input must be a string or array")),
            }
        }
        EndpointKind::Messages => {
            if !value.get("messages").is_some_and(Value::is_array) {
                return Err(ApiError::bad_request("Messages requires a messages array"));
            }
            if value.get("max_tokens").and_then(Value::as_i64).is_none_or(|tokens|tokens<=0) {
                return Err(ApiError::bad_request("Messages max_tokens must be a positive integer"));
            }
        }
        _ => {},
    }
    Ok((model, value, stream))
}

/// 缺 key 时统一 401，不做别的花样。
pub fn unauthorized() -> Response {
    ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "未授权").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body_must_be_json_with_a_model() {
        assert!(parse_body(b"not json", EndpointKind::Chat).is_err());
        assert!(parse_body(b"{}", EndpointKind::Chat).is_err());
        assert!(parse_body(br#"{"model":""}"#, EndpointKind::Chat).is_err());
        let (m, v, s) = parse_body(
            br#"{"model":"auto","stream":true,"messages":[]}"#,
            EndpointKind::Chat,
        )
        .unwrap();
        assert_eq!(m, "auto");
        assert!(s);
        assert_eq!(v["messages"], json!([]));
    }

    #[test]
    fn stream_defaults_to_false() {
        let (_, _, s) = parse_body(br#"{"model":"auto"}"#, EndpointKind::Chat).unwrap();
        assert!(!s);
    }
}
