//! 对应上游 `app.ts` 336 行往后的静态服务：面板产物 + SPA 回退。
//!
//! 上游从 `CLIENT_DIST` 读构建产物，未构建时回退。我们的面板是 Rust/WASM
//! （阶段 7），最终同样编进二进制或从磁盘目录服务——阶段 7 再定，先占位。

use axum::routing::get;
use axum::Router;

pub fn router() -> Router {
    Router::new().route("/", get(root))
}

async fn root() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::NOT_IMPLEMENTED,
        axum::Json(serde_json::json!({
            "error": {
                "message": "面板尚未移植（计划在阶段 7，Rust/WASM）",
                "type": "not_implemented",
                "stage": 7,
            }
        })),
    )
        .into_response()
}
