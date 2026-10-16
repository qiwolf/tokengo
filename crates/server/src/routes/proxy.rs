//! 对外代理面：三个 OpenAI 兼容端点。这是全项目最核心的一块。
//!
//! 端点取舍依据（3 天 10 万次请求实测）：
//!
//! ```text
//! POST /v1/chat/completions   13,857 次（13.9%）
//! POST /v1/embeddings         86,146 次（86.1%）
//! GET  /v1/models             面板与客户端发现用
//! ```
//!
//! 这里只做三件事：**认对外 key**、**拆出模型名与 stream 开关**、**转交给 gateway**。
//! 重试/换腿/记账/回切都在 `crate::gateway`，别往这个文件里塞业务。
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
use crate::db::groups;
use crate::gateway::{self, EndpointKind, ForwardRequest};
use crate::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/chat/completions", post(chat_completions))
        .route("/embeddings", post(embeddings))
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
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let (model, value, stream) = parse_body(&body, EndpointKind::Chat)?;
    gateway::forward(
        state,
        ForwardRequest {
            endpoint: EndpointKind::Chat,
            model,
            body: value,
            stream,
            client_ip: client_ip(&headers),
        },
    )
    .await
}

async fn embeddings(
    _key: GatewayKey,
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let (model, value, _) = parse_body(&body, EndpointKind::Embeddings)?;
    gateway::forward(
        state,
        ForwardRequest {
            endpoint: EndpointKind::Embeddings,
            model,
            body: value,
            // 向量端点没有流式
            stream: false,
            client_ip: client_ip(&headers),
        },
    )
    .await
}

/// 模型清单。**只列真的能用的**：组是开着的、组里至少有一条可用腿。
/// 面板与客户端看同一份判断（`groups::list_servable`），不会出现"清单里有、打了 404"。
async fn models(
    _key: GatewayKey,
    State(state): State<SharedState>,
) -> Result<Json<Value>, ApiError> {
    let servable = {
        let conn = state.conn();
        groups::list_servable(&conn)?
    };
    Ok(Json(serde_json::json!({
        "object": "list",
        "data": servable.iter().map(|m| serde_json::json!({
            "id": m,
            "object": "model",
            "created": 0,
            "owned_by": tokengo_shared::BRAND,
        })).collect::<Vec<_>>(),
    })))
}

/// 请求体：必须是 JSON 对象且带 model。**模型名写错是客户端的错**，回 400/404，
/// 不重试不换腿（这条规矩见 `docs/03-路由策略.md` 第四节）。
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
