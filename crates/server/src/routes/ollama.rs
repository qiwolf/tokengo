//! 对应上游 `routes/ollama.ts`：Ollama 兼容层，**裸挂在根**（不带 `/v1` 前缀），
//! 因为 Ollama 客户端打的是 `/api/tags`、`/api/version`、`/api/show`。
//!
//! 注意：上游这个文件里有 7 个 handler，但挂载是在 `app.use(ollamaRouter)`，
//! 不经过会话门——鉴权在处理器内部用统一 key 判。

use axum::routing::get;
use axum::Router;

use super::unbuilt;

pub fn router() -> Router {
    Router::new()
        .route("/api/tags", get(|| async { unbuilt(2, "GET /api/tags") }))
        .route("/api/version", get(|| async { unbuilt(2, "GET /api/version") }))
        .route("/api/show", get(|| async { unbuilt(2, "GET /api/show") }))
}
