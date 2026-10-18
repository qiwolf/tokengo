//! 对外代理面：四个 OpenAI 兼容端点。这是全项目最核心的一块。
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
        .route("/embeddings", post(embeddings))
        .route("/rerank", post(rerank))
        .route("/models", get(models))
}

// ───────────────────────── 鉴权 ─────────────────────────

/// 对外 key 的门。放在 handler 参数里即生效。
pub struct GatewayKey;

impl FromRequestParts<SharedState> for GatewayKey {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let key = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|raw| {
                raw.strip_prefix("Bearer ")
                    .or_else(|| raw.strip_prefix("bearer "))
                    .map(str::trim)
            })
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                ApiError::unauthorized("缺少 Authorization: Bearer <对外 key>（面板「设置」里配）")
            })?;
        if !state.check_gateway_key(key) {
            // 记一条：客户端连不上时最常就是这个原因（key 复制少了、还是 FreeLLMAPI 那把）。
            // 只记来源与掩码，**不记 key 本身** —— 日志会被下载/贴给人看。
            let hint: String = {
                let k: String = key.chars().take(4).collect();
                format!("{k}…（{} 位）", key.chars().count())
            };
            tracing::warn!(
                "有请求拿着不对的对外 key 打进来了：{}，来自 {}",
                hint,
                client_ip(&parts.headers).unwrap_or_else(|| "未知来源".into())
            );
            return Err(ApiError::unauthorized("对外 key 不对"));
        }
        Ok(GatewayKey)
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
        let servable = model_routes::servable(&conn)?;
        let auto_rows = if model_routes::auto_available(&conn)? {
            let now = crate::db::now_ms();
            let chat = !model_routes::auto_candidates(&conn, ModelKind::Chat, false, now)?.is_empty();
            let embeddings =
                !model_routes::auto_candidates(&conn, ModelKind::Embeddings, false, now)?.is_empty();
            let rerank = !model_routes::auto_candidates(&conn, ModelKind::Rerank, false, now)?.is_empty();
            let vision = !model_routes::auto_candidates(&conn, ModelKind::Chat, true, now)?.is_empty();
            let kind = if chat {
                ModelKind::Chat
            } else if embeddings {
                ModelKind::Embeddings
            } else if rerank {
                ModelKind::Rerank
            } else {
                ModelKind::Unknown
            };
            vec![serde_json::json!({
                "id": model_routes::AUTO_NAME,
                "object": "model",
                "created": created,
                "owned_by": tokengo_shared::BRAND,
                // 带图的请求打 auto 只有在这时候才走得通
                "vision": vision,
                "kind": kind.as_str(),
            })]
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
        })
    }));
    Ok(Json(serde_json::json!({ "object": "list", "data": data })))
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
    let stream = value
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
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
