//! 对应上游 `routes/status.ts`：`GET /livez`、`GET /readyz`。
//! 这两个是编排器探测用，无鉴权，注册在静态回退之前。

use axum::routing::get;
use axum::{Json, Router};

pub fn router() -> Router {
    Router::new()
        .route("/livez", get(livez))
        .route("/readyz", get(readyz))
}

async fn livez() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn readyz() -> Json<serde_json::Value> {
    // 阶段 1 接上数据库与配置校验后，这里要真正探依赖。
    Json(serde_json::json!({ "status": "ok", "checks": {} }))
}
