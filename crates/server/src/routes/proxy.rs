//! 对外代理面：三个 OpenAI 兼容端点。这是全项目最核心的一块。
//!
//! 端点取舍依据（3 天 10 万次请求实测）：
//!   POST /v1/chat/completions   13,857 次（13.9%）
//!   POST /v1/embeddings         86,146 次（86.1%）
//!   GET  /v1/models             面板与客户端发现用
//!
//! 请求处理流程（阶段 2 实装）：
//!   1. 鉴权：统一 key（`Authorization: Bearer …`）
//!   2. 解析 model → 模型组；`auto` 走 auto 组的有序腿列表
//!   3. `Router::attempt_order()` 拿到本次的腿顺序
//!   4. 逐条腿尝试，每条腿最多 `1 + retry_times` 次
//!      - 成功 → `on_success`，回给客户端
//!      - 腿的毛病（网络/5xx/429/401/403）→ 重试，耗尽后 `on_leg_exhausted` 换下一条腿
//!      - 客户端自己的错（400/404/422）→ **原样返回，不重试不换腿**
//!   5. 记账：请求级 + 尝试级

use axum::routing::{get, post};
use axum::Router;

use super::unbuilt;
use crate::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route(
            "/chat/completions",
            post(|| async { unbuilt(2, "POST /v1/chat/completions") }),
        )
        .route("/embeddings", post(|| async { unbuilt(2, "POST /v1/embeddings") }))
        .route("/models", get(|| async { unbuilt(2, "GET /v1/models") }))
}
