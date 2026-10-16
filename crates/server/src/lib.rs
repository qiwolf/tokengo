//! tokengo 服务端库。
//!
//! 二进制入口只有 `main.rs`（起监听），实体都在这里，方便集成测试直接构建整套 app。

pub mod config;
pub mod routes;

pub use config::Config;

use axum::Router;
use tower_http::trace::TraceLayer;

/// 组装完整网关。**中间件顺序严格照上游 `app.ts` 排**，
/// 顺序语义见 `docs/01-上游清点.md` 第八节。
pub fn app(cfg: &Config) -> Router {
    let _ = cfg;
    Router::new()
        // --- 探测端点（无鉴权，先于静态回退注册，否则根路径会落到面板 index）---
        .merge(routes::status::router())
        // --- 公开 API ---
        .nest("/v1", routes::v1::router())
        .nest("/v1beta", routes::v1beta::router())
        .nest("/mcp", routes::mcp::router())
        // --- 管理 API（面板用，会话门；阶段 2 接上中间件）---
        .nest("/api", routes::admin::router())
        // --- Ollama 兼容层裸挂在根 ---
        .merge(routes::ollama::router())
        // --- 面板静态资源 + SPA 回退（阶段 7）---
        .merge(routes::web::router())
        .layer(TraceLayer::new_for_http())
}
