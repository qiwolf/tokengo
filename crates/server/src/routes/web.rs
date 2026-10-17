//! 面板静态资源 + SPA 深链回退。对应上游 `app.ts` 336 行往后那段（从 `CLIENT_DIST`
//! 读产物、未构建时回退）。
//!
//! 产物是 `crates/web` 用 trunk 编出来的（Rust → WASM），**从磁盘目录服务**，
//! 不编进二进制：面板改动只 rebuild 前端，不动服务端；服务端也能在没有前端产物时
//! 正常编译与测试（否则 `cargo test` 会要求先装 trunk）。
//!
//! 目录查找顺序见 `crate::config::Config::web_dir`。找不到就**如实说没构建**，
//! 并把该跑的命令打出来 —— 不留一个空白页让人以为是坏了。

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;

use crate::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        // 根路径直接进面板；没构建就回一页说清楚怎么构建
        .route("/", get(root))
        .route("/ui", get(index))
        .route("/ui/", get(index))
        // 深链（/ui/legs 这类）与静态资源（/ui/pkg/xxx.wasm）都走这里
        .route("/ui/{*path}", get(asset))
}

async fn root(State(state): State<SharedState>) -> Response {
    if state.cfg.web_dir().is_some() {
        // 308 永久跳转：面板的固定地址是 /ui/
        Redirect::permanent("/ui/").into_response()
    } else {
        not_built()
    }
}

async fn index(State(state): State<SharedState>) -> Response {
    match state.cfg.web_dir() {
        Some(dir) => serve(dir, "index.html").await,
        None => not_built(),
    }
}

async fn asset(State(state): State<SharedState>, axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let Some(dir) = state.cfg.web_dir() else {
        return not_built();
    };
    let Some(rel) = safe_relative(&path) else {
        // `..`、绝对路径、Windows 分隔符一律挡掉：静态目录是公开面。
        return not_found("路径不合法");
    };

    let file = dir.join(&rel);
    if file.is_file() {
        return serve(dir, rel.to_string_lossy().as_ref()).await;
    }
    // 带扩展名却没这个文件 = 产物不全或链接写错，**必须回 404**：
    // 回 index.html 会让浏览器拿到一坨 HTML 去当 wasm 解析，报错完全看不懂。
    if rel.extension().is_some() {
        return not_found("静态资源不存在");
    }
    // 没有扩展名 = 前端路由的深链，回 index.html 交给前端自己找页
    serve(dir, "index.html").await
}

/// 去掉危险成分，只留目录内的相对路径。
fn safe_relative(path: &str) -> Option<PathBuf> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() || trimmed.contains('\\') || trimmed.contains('\0') {
        return None;
    }
    let rel = PathBuf::from(trimmed);
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(rel)
}

async fn serve(dir: &Path, rel: &str) -> Response {
    let file = dir.join(rel);
    match tokio::fs::read(&file).await {
        Ok(bytes) => {
            let ct = content_type(&file);
            let mut r = Response::new(axum::body::Body::from(bytes));
            r.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
            // trunk 的产物文件名带内容哈希，可以长缓存；index.html 必须每次回源，
            // 否则面板更新后浏览器一直拿旧的。
            let cache = if rel == "index.html" {
                "no-cache"
            } else {
                "public, max-age=31536000, immutable"
            };
            r.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            r
        }
        Err(e) => {
            tracing::warn!("面板文件 {} 读不出来：{e}", file.display());
            not_found("静态资源读不出来")
        }
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") | Some("map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn not_found(why: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": { "message": why, "type": "not_found" }
        })),
    )
        .into_response()
}

/// 面板产物没找到 —— 说清楚"没构建"，并把命令打出来。
fn not_built() -> Response {
    let (brand, version) = (tokengo_shared::BRAND, tokengo_shared::VERSION);
    let body = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>{brand} 面板未构建</title>
<style>body{{font-family:system-ui,sans-serif;max-width:44rem;margin:4rem auto;padding:0 1.5rem;line-height:1.7;color:#111}}
code{{background:#f3f4f6;padding:.15rem .4rem;border-radius:.25rem}}
h1{{margin:0 0 .5rem}} .muted{{color:#6b7280}}</style></head>
<body>
<h1>{brand} 面板没找到</h1>
<p class="muted">服务端 v{version} 在跑，但面板（Rust/WASM）的构建产物不在。</p>
<p>构建：<br><code>cd crates/web &amp;&amp; trunk build --release</code></p>
<p>或指定产物目录：<code>TOKENGO_WEB_DIR=/路径/to/dist</code></p>
<p class="muted">查找顺序：<code>TOKENGO_WEB_DIR</code> → 二进制旁的 <code>web/</code>
→ <code>crates/web/dist</code> → <code>dist</code>（目录里必须有 <code>index.html</code>）。</p>
<p class="muted">后端接口不受影响：<code>/livez</code>、<code>/readyz</code>、<code>/v1/*</code>、<code>/api/*</code> 照常。</p>
</body></html>"#
    );
    (
        StatusCode::NOT_IMPLEMENTED,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_paths_are_rejected() {
        for bad in ["../etc/passwd", "a/../../b", "/../x", "a\\b", "", "/"] {
            assert!(safe_relative(bad).is_none(), "{bad:?} 不该通过");
        }
        assert_eq!(
            safe_relative("/pkg/tokengo-web.js"),
            Some(PathBuf::from("pkg/tokengo-web.js"))
        );
    }

    #[test]
    fn content_types_cover_what_trunk_emits() {
        assert!(content_type(Path::new("a/index.html")).starts_with("text/html"));
        assert_eq!(content_type(Path::new("a/x.wasm")), "application/wasm");
        assert!(content_type(Path::new("a/x.js")).starts_with("text/javascript"));
        assert_eq!(content_type(Path::new("a/x.css")).starts_with("text/css"), true);
        // 认不出来的按二进制发，别瞎猜成 HTML 让浏览器当页面解析
        assert_eq!(content_type(Path::new("a/x.bin")), "application/octet-stream");
    }
}
