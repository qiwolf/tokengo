//! 路由层。目录结构对齐上游 `server/src/routes/`，一个上游文件对应一个子模块。
//!
//! 阶段 0 所有端点都是 501 占位：先把**路由树**摆完整，保证上游打得到的路径这里都
//! 有响应（哪怕是 501），不会出现 404 假象；再逐阶段把实现填进去。
//!
//! 对应关系见 `docs/01-上游清点.md` 第三节（31 个路由文件 / 192 个 handler）
//! 与第八节（对外暴露面）。

pub mod admin;
pub mod mcp;
pub mod ollama;
pub mod status;
pub mod v1;
pub mod v1beta;
pub mod web;

use axum::response::{IntoResponse, Response};
use axum::Json;

/// 未移植端点的统一响应：明确说"还没做"，并标出属于哪个阶段。
/// 阶段 2 起这个函数会被逐个替换掉，最终应该只剩 0 个调用点。
pub fn unbuilt(stage: u8, route: &str) -> Response {
    let body = serde_json::json!({
        "error": {
            "message": format!("{route} 尚未移植（计划在阶段 {stage}）"),
            "type": "not_implemented",
            "stage": stage,
        }
    });
    (axum::http::StatusCode::NOT_IMPLEMENTED, Json(body)).into_response()
}
