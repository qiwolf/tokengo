//! 路由层回归测试。
//!
//! 这里主要锁两类**只有跑起来才会暴露**的坑：
//! 1. 路由注册本身非法——axum/matchit 是在 `Router::route()` 里直接 panic 的，
//!    编译期完全看不出来（见 `v1beta.rs` 里 Gemini 那段注释）；
//! 2. 路径段里带动作后缀的协议（`{model}:action`）必须能正确拆分。
//!
//! 每次加新协议（anthropic / ollama / gemini / responses / MCP）都要补一条。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use tokengo_server::routes;

/// 构建整套路由表。路由串写错会在这里 panic —— 这就是复现用例。
#[test]
fn router_builds() {
    let _ = routes::status::router();
    let _ = routes::v1::router();
    let _ = routes::v1beta::router();
    let _ = routes::mcp::router();
    let _ = routes::admin::router();
    let _ = routes::ollama::router();
    let _ = routes::web::router();
}

#[tokio::test]
async fn gemini_action_splits_on_last_colon() {
    use routes::v1beta::split_gemini_action;
    assert_eq!(
        split_gemini_action("gemini-2.5-flash:generateContent"),
        Some(("gemini-2.5-flash", "generateContent"))
    );
    // 模型名里本身带冒号时，只按**最后**一个冒号拆
    assert_eq!(
        split_gemini_action("a:b:countTokens"),
        Some(("a:b", "countTokens"))
    );
    // 没有冒号 = 取模型详情
    assert_eq!(split_gemini_action("gemini-2.5-flash"), None);
}

#[tokio::test]
async fn bogus_gemini_action_is_400_not_501() {
    let app = routes::v1beta::router();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/models/x:bogusAction")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn known_gemini_action_is_501_until_stage_2() {
    let app = routes::v1beta::router();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/models/gemini-2.5-flash:generateContent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
}

/// 未移植 + 不存在的路径必须是 404，不能让客户端把"没实现"误判成"路径写错了"。
#[tokio::test]
async fn unknown_path_is_404() {
    let app = routes::v1::router();
    let res = app
        .oneshot(
            Request::builder()
                .uri("/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
