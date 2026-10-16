//! 对应上游挂在 `/api` 的 24 个管理路由器（面板用，全部走会话门）。
//!
//! 上游挂载点（`app.ts` 233–276 行），一行一个上游路由文件：
//!   /api             管理限流器（裸挂）
//!   /api/auth        auth.ts            (10)
//!   /api/keys        keys.ts            (16)  含 /api/keys/export（单独限流）
//!   /api/client-profiles  client-profiles.ts (5)
//!   /api/conversations    conversations.ts   (5)
//!   /api/logs        logs.ts            (2)
//!   /api/models      models.ts          (6)
//!   /api/profiles    profiles.ts        (10)
//!   /api/fallback    fallback.ts        (10)
//!   /api/embeddings  embeddings.ts      (5)
//!   /api/media       media.ts           (5)
//!   /api/search      search.ts          (2)
//!   /api/decisions   decisions.ts       (3)   上游 20261016 新增（PR #1537）
//!   /api/analytics   analytics.ts       (11)
//!   /api/health      health.ts          (4)
//!   /api/free-tier   free-tier.ts       (1)
//!   /api/settings    settings.ts        (35)  端点最多的一个
//!   /api/premium     premium.ts         (5)
//!   /api/backups     backups.ts         (8)   导出文件带加密 key，必须会话门
//!   /api/cache       cache.ts           (3)
//!   /api/compression compression.ts     (2)
//!   /api/update      update.ts          (4)
//!   /api/ping        内联，无鉴权
//!
//! 括号内是上游该文件里注册的 handler 数，合计 192。

use axum::routing::get;
use axum::Router;

use super::unbuilt;

pub fn router() -> Router {
    Router::new()
        .route("/ping", get(ping))
        .nest("/auth", stub("auth", 1, &["/status", "/setup", "/login", "/logout"]))
        .nest("/keys", stub("keys", 1, &["/providers", "/", "/:id/cooldowns"]))
        .nest(
            "/client-profiles",
            stub("client-profiles", 2, &["/", "/:id"]),
        )
        .nest("/conversations", stub("conversations", 2, &["/", "/:id"]))
        .nest("/logs", stub("logs", 2, &["/", "/clear"]))
        .nest("/models", stub("models", 3, &["/", "/custom/:id", "/:id"]))
        .nest("/profiles", stub("profiles", 2, &["/", "/active"]))
        .nest(
            "/fallback",
            stub("fallback", 4, &["/routing", "/quota-forecast", "/penalty-inspector"]),
        )
        .nest("/embeddings", stub("embeddings", 2, &["/", "/custom"]))
        .nest("/media", stub("media", 6, &["/", "/usage", "/custom"]))
        .nest("/search", stub("search", 6, &["/engines", "/engines/:id"]))
        .nest("/decisions", stub("decisions", 2, &["/models", "/models/:id", "/try"]))
        .nest(
            "/analytics",
            stub("analytics", 2, &["/summary", "/by-model", "/search"]),
        )
        .nest("/health", stub("health", 1, &["/", "/check-all"]))
        .nest("/free-tier", stub("free-tier", 4, &["/"]))
        .nest("/settings", stub("settings", 2, &["/version", "/update-check"]))
        .nest("/premium", stub("premium", 5, &["/", "/key"]))
        .nest("/backups", stub("backups", 8, &["/tables", "/schedule"]))
        .nest("/cache", stub("cache", 3, &["/stats", "/config"]))
        .nest("/compression", stub("compression", 5, &["/stats", "/preview"]))
        .nest("/update", stub("update", 4, &["/check", "/status", "/release"]))
}

async fn ping() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": tokengo_shared::VERSION,
    }))
}

/// 一个管理子路由的占位：把所有已知路径摆出来，全部回 501。
/// `stage` 是实现它的阶段号，`paths` 取自上游该文件的 handler 路径样本。
fn stub(_name: &'static str, stage: u8, paths: &'static [&'static str]) -> Router {
    let mut r = Router::new();
    for p in paths {
        // 路径里的 `:` 参数在 axum 0.8 写作 `{}`
        let axum_path: String = p
            .split('/')
            .map(|seg| {
                if let Some(rest) = seg.strip_prefix(':') {
                    format!("{{{rest}}}")
                } else {
                    seg.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        r = r.route(
            &axum_path,
            get(move || async move { unbuilt(stage, "管理端点") }).post(move || async move { unbuilt(stage, "管理端点") }),
        );
    }
    r
}
