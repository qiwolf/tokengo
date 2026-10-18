//! tokengo 服务端库。二进制入口只有 `main.rs`（起监听），实体都在这里，
//! 方便集成测试直接构建整套 app。

pub mod config;
pub mod crypto;
pub mod db;
pub mod gateway;
pub mod gateway_key;
pub mod janitor;
pub mod logging;
pub mod prober;
pub mod routes;
pub mod state;
pub mod upstream;

pub use config::Config;
pub use state::{AppState, SharedState};

use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

/// 组装完整网关。
///
/// **对外只有这些路径。** 上游 FreeLLMAPI 有 45 个端点（含 Gemini / Ollama /
/// Anthropic / MCP / 媒体 / 搜索 / 审核），本机 3 天实测除对话与向量外全部 0 次调用，
/// 所以都不做（`docs/02-按需功能清单.md`）。
pub fn app(state: SharedState) -> Router {
    Router::new()
        // 探测端点（无鉴权，先于面板静态回退注册）
        .merge(routes::status::router())
        // 对外代理面：对话 / 向量 / 模型清单
        .nest("/v1", routes::proxy::router())
        // 面板管理面
        .nest("/api", routes::admin::router())
        // 面板静态资源 + SPA 回退
        .merge(routes::web::router())
        // 面板资源压缩：wasm 841 KB → gzip 后约 330 KB（首次打开少下 500 KB）。
        // 只压文本类（wasm/js/css/html），图片之类本来就压过的会被自动跳过；
        // 浏览器第二次打开走 immutable 缓存，不会再下一遍。
        //
        // **只用 gzip，不加 brotli**（2026-10-18 实测）：br 只比 gzip 少 21 KB 下载，
        // 但它的压缩器每次要占约 6 MB 内存且不还给系统 —— 连发 5 次面板，
        // 进程常驻从 7.5 MB 涨到 **39.6 MB**（gzip 只到 9.6 MB，原样发到 8.8 MB）。
        // 512 MB 的容器里，30 MB 换 21 KB 不划算。
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
