//! 对应上游挂在 `/v1` 的六个路由器。
//!
//! 上游挂载顺序（`app.ts` 287–313 行，**顺序有语义，不能调**）：
//!   docs → providers → `/v1/t/:token` 限流+url-token → 代理限流
//!   → anthropic → 主代理(proxy) → moderation → responses
//!
//! 说明：
//! - docs / providers 挂在限流之前，所以看文档、轮询状态不消耗调用者的配额；
//! - anthropic 挂在主代理**之前**，因为它要内容协商 `GET /v1/models`
//!   （带 `anthropic-version` 头返回 Anthropic 形状，否则放行给主代理）；
//! - gemini 在 `/v1beta`，单独一个子模块。

use axum::routing::{get, post};
use axum::Router;

use super::unbuilt;

pub fn router() -> Router {
    Router::new()
        // --- docsRouter：无鉴权，不占配额 ---
        .route("/docs", get(|| async { unbuilt(2, "GET /v1/docs") }))
        .route("/openapi.json", get(|| async { unbuilt(2, "GET /v1/openapi.json") }))
        // --- providersRouter：统一 key 在校验器内部判 ---
        .route("/providers", get(|| async { unbuilt(4, "GET /v1/providers") }))
        // --- urlTokenRouter：/v1/t/:token/*，给设不了 header 的客户端 ---
        .route(
            "/t/{token}/chat/completions",
            post(|| async { unbuilt(2, "POST /v1/t/:token/chat/completions") }),
        )
        .route(
            "/t/{token}/embeddings",
            post(|| async { unbuilt(2, "POST /v1/t/:token/embeddings") }),
        )
        .route(
            "/t/{token}/models",
            get(|| async { unbuilt(2, "GET /v1/t/:token/models") }),
        )
        // --- anthropicRouter ---
        .route("/messages", post(|| async { unbuilt(2, "POST /v1/messages") }))
        .route(
            "/messages/count_tokens",
            post(|| async { unbuilt(2, "POST /v1/messages/count_tokens") }),
        )
        // --- proxyRouter（主代理）---
        .route(
            "/chat/completions",
            post(|| async { unbuilt(2, "POST /v1/chat/completions") }),
        )
        .route("/embeddings", post(|| async { unbuilt(2, "POST /v1/embeddings") }))
        .route("/rerank", post(|| async { unbuilt(2, "POST /v1/rerank") }))
        .route("/models", get(|| async { unbuilt(2, "GET /v1/models") }))
        // --- moderationRouter ---
        .route("/moderations", post(|| async { unbuilt(6, "POST /v1/moderations") }))
        // --- responsesRouter ---
        .route("/responses", post(|| async { unbuilt(2, "POST /v1/responses") }))
}
