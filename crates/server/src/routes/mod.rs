//! 路由层。按本机实际用量取舍（见 `docs/02-按需功能清单.md`），只留真在跑的端点。
//!
//! **对外只做四个端点。** 3 天 10 万次请求里：向量 86%、对话 14%，
//! 其余 41 个端点一次都没被调用过，一律不做。

pub mod operations;
pub mod operations_tokens;
pub mod providers;
pub mod accounts;
pub mod managed_users;
pub mod oauth;
pub mod identity;
pub mod billing;
pub mod admin;
pub mod model_routing;
pub mod proxy;
pub mod status;
pub mod web;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// 接口错误。响应体形状沿用现网 FreeLLMAPI 的 `{"error":{"message":…}}`，
/// 客户端那边不用改解析。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    /// 机器可读的类型标识，面板按它分支。
    pub kind: &'static str,
}

impl ApiError {
    pub fn new(status: StatusCode, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request_error", message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": { "message": self.message, "type": self.kind }
            })),
        )
            .into_response()
    }
}

impl From<crate::db::DbError> for ApiError {
    fn from(e: crate::db::DbError) -> Self {
        Self::internal(e.to_string())
    }
}

impl From<crate::crypto::CryptoError> for ApiError {
    fn from(e: crate::crypto::CryptoError) -> Self {
        Self::internal(e.to_string())
    }
}

/// 客户端 IP。优先取反代填的转发头（对外入口是雷池 → Cloudflare，直连 IP 没意义）。
pub fn client_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    for h in ["x-forwarded-for", "x-real-ip"] {
        if let Some(v) = headers.get(h).and_then(|v| v.to_str().ok()) {
            if let Some(first) = v.split(',').next() {
                if !first.trim().is_empty() {
                    return Some(first.trim().to_string());
                }
            }
        }
    }
    None
}

/// 暂未实现端点的统一响应：明确说"还没做"，并标出属于哪个阶段。
/// 阶段推进时逐个替换掉，最终应该只剩 0 个调用点。
pub fn unbuilt(stage: u8, route: &str) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": {
                "message": format!("{route} 尚未实现（计划在阶段 {stage}）"),
                "type": "not_implemented",
                "stage": stage,
            }
        })),
    )
        .into_response()
}

pub mod virtual_models;
