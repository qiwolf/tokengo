//! tokengo 服务端库。二进制入口只有 `main.rs`（起监听），实体都在这里，
//! 方便集成测试直接构建整套 app。

pub mod config;
pub mod routes;

pub use config::Config;

use axum::Router;
use tower_http::trace::TraceLayer;

/// 组装完整网关。
///
/// **对外只有这些路径。** 上游 FreeLLMAPI 有 45 个端点（含 Gemini / Ollama /
/// Anthropic / MCP / 媒体 / 搜索 / 审核），本机 3 天实测除对话与向量外全部 0 次调用，
/// 所以都不做（`docs/02-按需功能清单.md`）。
pub fn app(cfg: &Config) -> Router {
    let _ = cfg;
    Router::new()
        // 探测端点（无鉴权，先于面板静态回退注册）
        .merge(routes::status::router())
        // 对外代理面：对话 / 向量 / 模型清单
        .nest("/v1", routes::proxy::router())
        // 面板管理面
        .nest("/api", routes::admin::router())
        // 面板静态资源 + SPA 回退（阶段 7）
        .merge(routes::web::router())
        .layer(TraceLayer::new_for_http())
}
