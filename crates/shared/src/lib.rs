//! tokengo 共享层：渠道/模型组的数据结构 + 路由状态机。
//!
//! 本项目**不是** FreeLLMAPI 的全量移植，而是按本机实际用量做的自用网关
//! （见 `docs/02-按需功能清单.md`）。所以这里没有 72 个平台枚举，也没有
//! 37 个 provider 适配器 —— 现网 9 条渠道全部是自定义端点，一个内置适配器都没用到。

pub mod channel;
pub mod routing;

pub use channel::{
    chunk_has_payload, empty_chat_reply, render_header_value, request_has_image,
    session_id_from_body, short_hash, strip_fields, unwrap_envelope, Channel, ChannelId,
    KindSource, ModelGroup, ModelKind,
};
pub use routing::{classify_status, AttemptOutcome, GroupState, Router, RoutingConfig};

/// 本机网关版本，用于 `/api/ping`、`/livez`、面板页脚。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 面板与服务端共用的品牌名。
pub const BRAND: &str = "tokengo";

/// 项目来源说明。上游 FreeLLMAPI 的功能取舍与差异见 `docs/02-按需功能清单.md`。
pub const ORIGIN: &str = "自用网关，参考 freellmapi (MIT) 的端点形状";
