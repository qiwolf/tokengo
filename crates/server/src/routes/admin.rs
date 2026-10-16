//! 管理面：面板用的接口。只做面板真需要的，不做上游那 24 个管理路由。
//!
//! 上游 FreeLLMAPI 有 24 个管理路由共 192 个 handler；本机面板只需要下面这些。
//! 全部走会话门（面板登录后拿到的会话 cookie），阶段 2 接上中间件。

use axum::routing::{get, post, put};
use axum::Router;

use super::unbuilt;

pub fn router() -> Router {
    Router::new()
        // 无鉴权：面板用来判断服务是否活着
        .route("/ping", get(ping))
        // 面板登录
        .route("/auth/status", get(|| async { unbuilt(2, "GET /api/auth/status") }))
        .route("/auth/login", post(|| async { unbuilt(2, "POST /api/auth/login") }))
        .route("/auth/logout", post(|| async { unbuilt(2, "POST /api/auth/logout") }))
        // 腿的增删改查 —— 用户决议第 4 条：全部自定义添加端点和 key
        .route("/legs", get(|| async { unbuilt(1, "GET /api/legs") }))
        .route("/legs", post(|| async { unbuilt(1, "POST /api/legs") }))
        .route("/legs/{id}", put(|| async { unbuilt(1, "PUT /api/legs/:id") }))
        .route("/legs/{id}", axum::routing::delete(|| async {
            unbuilt(1, "DELETE /api/legs/:id")
        }))
        // 腿连通性自检（面板上一个"测一下"按钮）
        .route("/legs/{id}/probe", post(|| async {
            unbuilt(2, "POST /api/legs/:id/probe")
        }))
        // 模型组与优先级排序
        .route("/groups", get(|| async { unbuilt(1, "GET /api/groups") }))
        .route("/groups", post(|| async { unbuilt(1, "POST /api/groups") }))
        .route("/groups/{name}", put(|| async { unbuilt(1, "PUT /api/groups/:name") }))
        // 路由状态：active / primary / 下次回切时间 / 切换次数（面板要能直接看见）
        .route("/routing", get(|| async { unbuilt(2, "GET /api/routing") }))
        .route("/routing/config", put(|| async { unbuilt(2, "PUT /api/routing/config") }))
        // 立即手动回切一次（不想等 10 分钟时用）
        .route("/routing/{group}/reprobe", post(|| async {
            unbuilt(2, "POST /api/routing/:group/reprobe")
        }))
        // 记账与流水
        .route("/stats", get(|| async { unbuilt(1, "GET /api/stats") }))
        .route("/logs", get(|| async { unbuilt(1, "GET /api/logs") }))
        .route("/timeline", get(|| async { unbuilt(5, "GET /api/timeline") }))
        // 设置
        .route("/settings", get(|| async { unbuilt(1, "GET /api/settings") }))
        .route("/settings", put(|| async { unbuilt(1, "PUT /api/settings") }))
}

async fn ping() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "brand": tokengo_shared::BRAND,
        "version": tokengo_shared::VERSION,
    }))
}
