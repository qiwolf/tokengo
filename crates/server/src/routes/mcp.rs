//! 对应上游 `routes/mcp.ts`：MCP over 无状态 Streamable HTTP。
//! 给 MCP 系 agent 用的网关内省工具。统一 key 鉴权，与 `/v1` 同一个 key，
//! 但用**自己独立的限流桶**——两个面都守同一个 key，未鉴权的爆破不能在这里
//! 拿到免限流的探测口。

use axum::routing::post;
use axum::Router;

use super::unbuilt;

pub fn router() -> Router {
    Router::new().route("/", post(|| async { unbuilt(2, "POST /mcp") }))
}
