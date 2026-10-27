//! tokengo 服务端库。二进制入口只有 `main.rs`（起监听），实体都在这里，
//! 方便集成测试直接构建整套 app。

pub mod oauth;
pub mod oauth_external;
pub mod egress;
pub mod passthrough;
pub mod reasoning_capabilities;
pub mod search_tools;
pub mod source_rates;
pub mod openai_transport;
pub mod config;
pub mod capability_detector;
pub mod model_registry;
pub mod crypto;
pub mod db;
pub mod billing_writer;
pub mod admission;
pub mod gateway;
pub mod protocol;
pub mod protocol_config;
pub mod runtime_identity;
pub mod provider_diagnostics;
pub mod parameter_contracts;
pub mod contract_evidence;
pub mod request_plan;
pub mod message_roles;
pub mod parameter_contracts_china;
pub mod parameter_budgets;
pub mod protocol_sessions;
pub mod litellm_import;
pub mod gateway_key;
pub mod janitor;
pub mod logging;
pub mod prober;
pub mod custom_probes;
pub mod vector_compat;
pub mod vector_renewal;
pub mod type_detector;
pub mod vision_detector;
pub mod routes;
mod operations_guard;
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
        .merge(routes::providers::router())
        .merge(routes::passthrough_admin::router())
        .merge(routes::passthrough_probe::router())
        .merge(custom_probes::router())
        .merge(routes::accounts::router())
        .merge(routes::operations_tokens::router())
        .merge(routes::operations::router())
        .merge(routes::managed_users::router())
        .merge(routes::virtual_models::router())
        .merge(routes::shared_virtual_models::router())
        .merge(routes::model_routing::router())
        .merge(routes::identity::router())
        .merge(routes::oauth::router())
        .merge(routes::network::router())
        .merge(search_tools::router())
        .merge(routes::billing::router())
        .merge(litellm_import::router())
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
        .layer(axum::middleware::from_fn_with_state(state.clone(),routes::operations::middleware))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(routes::operations::transport_gate))
        .layer(axum::middleware::from_fn_with_state(state.clone(),passthrough::dispatch))
        .with_state(state)
}
