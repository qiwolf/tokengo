//! 路由层回归测试。
//!
//! 锁三类**只有跑起来才会暴露**的坑：
//! 1. 路由注册非法 —— axum/matchit 是在 `Router::route()` 里直接 panic 的，
//!    编译期完全看不出来；
//! 2. 不存在的路径必须是 404，不能让客户端把"没实现"误判成"路径写错了"；
//! 3. 本机不做的端点（Gemini / Ollama / MCP / 媒体…）必须**真 404**，
//!    不能悄悄漏进来。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokengo_server::{app, routes, Config};
use tower::ServiceExt;

fn test_app() -> axum::Router {
    app(&Config {
        listen: "127.0.0.1:0".into(),
    })
}

async fn status_of(app: axum::Router, method: &str, uri: &str) -> StatusCode {
    app.oneshot(
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
    .status()
}

/// 构建整套路由表。路由串写错会在这里 panic —— 这就是复现用例。
#[test]
fn router_builds() {
    let _ = routes::status::router();
    let _ = routes::proxy::router();
    let _ = routes::admin::router();
    let _ = routes::web::router();
}

#[tokio::test]
async fn probes_are_alive() {
    assert_eq!(status_of(test_app(), "GET", "/livez").await, StatusCode::OK);
    assert_eq!(status_of(test_app(), "GET", "/readyz").await, StatusCode::OK);
    assert_eq!(status_of(test_app(), "GET", "/api/ping").await, StatusCode::OK);
}

/// 要做的三个端点：现在是 501（还没实现），但**不能是 404**。
#[tokio::test]
async fn planned_endpoints_are_present_but_unbuilt() {
    for (m, uri) in [
        ("POST", "/v1/chat/completions"),
        ("POST", "/v1/embeddings"),
        ("GET", "/v1/models"),
    ] {
        assert_eq!(
            status_of(test_app(), m, uri).await,
            StatusCode::NOT_IMPLEMENTED,
            "{m} {uri} 应该已注册（回 501）"
        );
    }
}

/// 本机不做的端点必须真 404，别让它们悄悄存在。
#[tokio::test]
async fn dropped_upstream_endpoints_are_really_absent() {
    for (m, uri) in [
        ("POST", "/v1/rerank"),
        ("POST", "/v1/moderations"),
        ("POST", "/v1/search"),
        ("POST", "/v1/responses"),
        ("POST", "/v1/messages"),
        ("POST", "/v1/images/generations"),
        ("POST", "/v1/audio/speech"),
        ("POST", "/v1/decisions"),
        ("POST", "/api/chat"),          // Ollama
        ("GET", "/api/tags"),           // Ollama
        ("GET", "/v1beta/models"),      // Gemini
        ("POST", "/mcp"),               // MCP
    ] {
        assert_eq!(
            status_of(test_app(), m, uri).await,
            StatusCode::NOT_FOUND,
            "{m} {uri} 不在本机需求里，必须真 404"
        );
    }
}

#[tokio::test]
async fn unknown_path_is_404() {
    assert_eq!(
        status_of(test_app(), "GET", "/does-not-exist").await,
        StatusCode::NOT_FOUND
    );
}

/// 面板管理面要有的接口（腿管理、模型组、路由状态）。
#[tokio::test]
async fn admin_surface_exists() {
    for uri in [
        "/api/legs",
        "/api/groups",
        "/api/routing",
        "/api/stats",
        "/api/logs",
        "/api/settings",
    ] {
        let s = status_of(test_app(), "GET", uri).await;
        assert_ne!(s, StatusCode::NOT_FOUND, "GET {uri} 应该存在");
    }
}
