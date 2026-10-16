//! tokengo 共享层。
//!
//! 与上游 `@freellmapi/shared` 对应：放平台标识、模型标识、请求/响应结构、错误码。
//! 移植原则：上游有的类型这里必须有一份，名字尽量对齐，方便逐字段对拍。

pub mod platform;

pub use platform::{Platform, UnknownPlatform};

/// 本机网关自身的版本，用于 `/api/ping`、`/livez` 与面板页脚。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 上游项目的许可与来源，面板"关于"页要展示。
pub const UPSTREAM: &str = "freellmapi (MIT) — full Rust port";
