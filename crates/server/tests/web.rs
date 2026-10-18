//! 面板静态服务的回归测试。**面板产物不在仓库里**（得先 `trunk build`），
//! 所以这里造一个假的产物目录，只钉服务端那半边：
//!
//! 1. 深链要能回 index.html（否则刷新页面就 404）；
//! 2. 带扩展名却不存在的资源必须**真 404**，不能回 HTML —— 浏览器会把 HTML
//!    当 wasm/JS 解析，报错完全看不懂；
//! 3. `..` 之类的路径不许越出产物目录；
//! 4. 产物没构建时**如实说没构建**并给出构建命令，而不是回一个空白页。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokengo_server::{app, state::AppState, Config};
use tower::ServiceExt;

struct Fixture {
    dir: std::path::PathBuf,
    state: Arc<AppState>,
}

impl Fixture {
    /// `with_panel = true` 时造一个假的产物目录（index.html + pkg/app.js + pkg/app.wasm）。
    fn new(with_panel: bool) -> Self {
        std::env::remove_var(tokengo_server::crypto::MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-web-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let web_dir = if with_panel {
            let d = dir.join("dist");
            std::fs::create_dir_all(d.join("pkg")).unwrap();
            std::fs::write(d.join("index.html"), "<html><body>面板占位</body></html>").unwrap();
            std::fs::write(d.join("pkg/app.js"), "console.log('hi')").unwrap();
            std::fs::write(d.join("pkg/app.wasm"), b"\0asm\x01\0\0\0").unwrap();
            Some(d)
        } else {
            None
        };

        let cfg = Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("t.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            web_dir,
           log_dir: None,
 };
        Self {
            state: AppState::init(cfg).unwrap(),
            dir,
        }
    }

    fn router(&self) -> axum::Router {
        app(self.state.clone())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// 发一个请求，返回 (状态码, 响应头, 正文文本)。
async fn get(app: axum::Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let mut body = resp.into_body().into_data_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = futures_util::StreamExt::next(&mut body).await {
        buf.extend_from_slice(&chunk.unwrap());
    }
    (status, headers, String::from_utf8_lossy(&buf).into_owned())
}

#[tokio::test]
async fn root_sends_the_browser_to_the_panel() {
    let f = Fixture::new(true);
    let (status, headers, _) = get(f.router(), "/").await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT, "根路径该跳 /ui/");
    assert_eq!(headers.get("location").unwrap(), "/ui/");
}

#[tokio::test]
async fn panel_index_and_assets_are_served_with_right_types() {
    let f = Fixture::new(true);

    let (status, headers, body) = get(f.router(), "/ui/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/html"));
    assert!(body.contains("面板占位"));
    // index.html 必须每次回源，否则面板更新后浏览器一直拿旧的
    assert_eq!(headers["cache-control"], "no-cache");

    let (status, headers, body) = get(f.router(), "/ui/pkg/app.js").await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/javascript"));
    assert!(body.contains("console.log"));
    // 带内容哈希的产物可以长缓存
    assert!(headers["cache-control"].to_str().unwrap().contains("immutable"));

    let (status, headers, _) = get(f.router(), "/ui/pkg/app.wasm").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/wasm");
}

#[tokio::test]
async fn deep_links_fall_back_to_index_but_missing_assets_do_not() {
    let f = Fixture::new(true);

    // /ui/channels 是前端路由，服务端回 index.html 交给前端自己找页
    let (status, headers, body) = get(f.router(), "/ui/channels").await;
    assert_eq!(status, StatusCode::OK, "深链刷新不能 404");
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/html"));
    assert!(body.contains("面板占位"));

    // 带扩展名却没有 = 产物不全，必须真 404（回 HTML 会让浏览器拿它当代码解析）
    for uri in ["/ui/pkg/missing.js", "/ui/pkg/missing.wasm"] {
        let (status, headers, body) = get(f.router(), uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} 该是 404");
        assert!(
            headers["content-type"].to_str().unwrap().contains("json"),
            "404 也不能回 HTML"
        );
        assert!(!body.contains("面板占位"));
    }
}

#[tokio::test]
async fn traversal_out_of_the_panel_dir_is_refused() {
    let f = Fixture::new(true);
    let secrets = f.dir.join("dist").join("passwd");
    std::fs::write(&secrets, "不该被读到").unwrap();
    std::fs::write(f.dir.join("outside.txt"), "目录外的东西").unwrap();

    for uri in [
        "/ui/../outside.txt",
        "/ui/..%2Foutside.txt",
        "/ui/pkg/../../outside.txt",
    ] {
        let (status, _, body) = get(f.router(), uri).await;
        assert_ne!(status, StatusCode::OK, "{uri} 不该被服务出去");
        assert!(!body.contains("目录外的东西"), "{uri} 泄漏了目录外文件");
    }
}

#[tokio::test]
async fn without_a_build_the_panel_says_it_is_not_built() {
    let f = Fixture::new(false);

    let (status, headers, body) = get(f.router(), "/").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "该如实说没构建");
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/html"));
    assert!(body.contains("trunk build"), "得把构建命令打出来：{body}");
    assert!(body.contains("TOKENGO_WEB_DIR"), "得说清目录怎么指定");

    let (status, _, body) = get(f.router(), "/ui/").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(body.contains("trunk build"));

    // 面板没构建不影响后端：探测端点与管理面照常
    let (status, _, _) = get(f.router(), "/livez").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = get(f.router(), "/api/channels").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "无会话该 401，不是 404");
}

#[tokio::test]
async fn unreadable_index_html_is_reported_not_blank() {
    let f = Fixture::new(true);
    // 把 index.html 换成一个目录：读它会失败。现象是"面板打不开"，但必须留痕、
    // 不能回一个 200 空页面让人以为面板坏了。
    let idx = f.dir.join("dist").join("index.html");
    std::fs::remove_file(&idx).unwrap();
    std::fs::create_dir_all(&idx).unwrap();

    let (status, _, body) = get(f.router(), "/ui/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("读不出来") || body.contains("不存在"));
}
