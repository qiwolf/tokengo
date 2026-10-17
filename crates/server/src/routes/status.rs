//! 对应上游 `routes/status.ts`：`GET /livez`、`GET /readyz`。
//! 这两个是编排器探测用，无鉴权，注册在静态回退之前。

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/livez", get(livez))
        .route("/readyz", get(readyz))
}

async fn livez() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// 真探依赖：能不能拿到数据库锁、库头读不读得出来。
/// 假探活（永远返回 ok）在容器编排下等于没有探活。
async fn readyz(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let db_ok = {
        let conn = state.conn();
        conn.query_row("SELECT COUNT(*) FROM channels", [], |r| r.get::<_, i64>(0))
            .is_ok()
    };
    let routing_ok = state.routing().all_states().len();

    let all_ok = db_ok;
    Json(serde_json::json!({
        "status": if all_ok { "ok" } else { "degraded" },
        "checks": {
            "database": if db_ok { "ok" } else { "fail" },
            "routing_groups_tracked": routing_ok,
            "panel_password_set": db_ok && crate::db::sessions::has_panel_password(&state.conn()).unwrap_or(false),
            "gateway_key_set": crate::db::settings::get(&state.conn(), crate::db::settings::GATEWAY_KEY_HASH).ok().flatten().is_some(),
        }
    }))
}
