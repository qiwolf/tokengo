//! 对应上游 `routes/gemini.ts`：原生 Gemini 线协议，供 Gemini CLI 与 Gemini 系 agent 使用。
//! 上游挂在 `/v1beta`，与 `/v1` 各自一个限流桶。
//!
//! **坑（阶段 0 实测）**：Gemini 把动作写在路径段的尾巴上——
//! `/v1beta/models/gemini-2.5-flash:generateContent`。axum 底层的 matchit
//! 一个路径段只允许一个参数，`{model}:generateContent` 会被判成两个参数，
//! 而且是在**注册路由时直接 panic**（`Invalid route ... Only one parameter is
//! allowed per path segment`），不是请求时 404——进程根本起不来，很难注意。
//!
//! 所以这里收整段 `{model_action}`，在处理器里按最后一个 `:` 拆开。
//! 后面碰到别的"动作写进路径段"的协议同样处理。

use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;

use super::unbuilt;

pub fn router() -> Router {
    Router::new()
        .route("/models/{model_action}", post(dispatch))
        .route("/models", get(|| async { unbuilt(2, "GET /v1beta/models") }))
}

/// 把 `gemini-2.5-flash:generateContent` 拆成（模型名，动作）。
/// 没有 `:` 时视为 `GET /v1beta/models/{model}`（取单个模型详情）。
///
/// 抽成纯函数是为了能被测试直接钉住——路由段的拆分规则很容易写错，
/// 而写错了只有真发请求才看得出来。
pub fn split_gemini_action(s: &str) -> Option<(&str, &str)> {
    s.rsplit_once(':')
}

async fn dispatch(Path(model_action): Path<String>) -> Response {
    match split_gemini_action(&model_action) {
        Some((model, action)) => {
            let route = format!("POST /v1beta/models/{model}:{action}");
            match action {
                // 动作非法时回 400 而不是 501：免得客户端把"动作不支持"
                // 误当成"网关还没实现这个功能"。
                "generateContent" | "streamGenerateContent" | "countTokens" => unbuilt(2, &route),
                other => (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({
                        "error": {
                            "code": 400,
                            "message": format!("不支持的 Gemini 动作: {other}"),
                            "status": "INVALID_ARGUMENT",
                        }
                    })),
                )
                    .into_response(),
            }
        }
        None => unbuilt(2, "GET /v1beta/models/{model}"),
    }
}
