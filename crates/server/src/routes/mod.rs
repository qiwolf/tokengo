//! 路由层。按本机实际用量取舍（见 `docs/02-按需功能清单.md`），只留真在跑的端点。
//!
//! **对外只做四个端点。** 3 天 10 万次请求里：向量 86%、对话 14%，
//! 其余 41 个端点一次都没被调用过，一律不做。

pub mod admin;
pub mod proxy;
pub mod status;
pub mod web;

/// 暂未实现端点的统一响应：明确说"还没做"，并标出属于哪个阶段。
/// 阶段推进时逐个替换掉，最终应该只剩 0 个调用点。
pub fn unbuilt(stage: u8, route: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::NOT_IMPLEMENTED,
        axum::Json(serde_json::json!({
            "error": {
                "message": format!("{route} 尚未实现（计划在阶段 {stage}）"),
                "type": "not_implemented",
                "stage": stage,
            }
        })),
    )
        .into_response()
}
