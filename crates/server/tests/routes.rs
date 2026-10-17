//! 路由层 + 数据层回归测试（真起一套 app，打真 HTTP）。
//!
//! 锁三类**只有跑起来才会暴露**的坑：
//! 1. 路由注册非法 —— axum/matchit 是在 `Router::route()` 里直接 panic 的，
//!    编译期完全看不出来；
//! 2. 不存在的路径必须是 404，不能让客户端把"没实现"误判成"路径写错了"；
//! 3. 本机不做的端点（Gemini / Ollama / MCP / 媒体…）必须**真 404**，
//!    不能悄悄漏进来。
//!
//! 阶段 1 起还多一类：面板登录门与渠道/组的增删改查**真的落库**。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tokengo_server::{app, routes, state::AppState, Config};
use tower::ServiceExt;

struct Fixture {
    dir: std::path::PathBuf,
    state: Arc<AppState>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn fixture() -> Fixture {
    std::env::remove_var(tokengo_server::crypto::MASTER_KEY_ENV);
    let dir = std::env::temp_dir().join(format!("tokengo-route-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = Config {
        listen: "127.0.0.1:0".into(),
        db_path: dir.join("t.db"),
        master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
        web_dir: None,
    };
    Fixture {
        state: AppState::init(cfg).unwrap(),
        dir,
    }
}

fn test_app(f: &Fixture) -> axum::Router {
    app(f.state.clone())
}

async fn send(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

async fn status_of(app: axum::Router, method: &str, uri: &str) -> StatusCode {
    send(app, method, uri, None, None).await.0
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
    let f = fixture();
    assert_eq!(status_of(test_app(&f), "GET", "/livez").await, StatusCode::OK);
    assert_eq!(status_of(test_app(&f), "GET", "/readyz").await, StatusCode::OK);
    assert_eq!(status_of(test_app(&f), "GET", "/api/ping").await, StatusCode::OK);
}

/// readyz 必须真探数据库，不能永远回 ok。这里断开库表让它失败。
#[tokio::test]
async fn readyz_really_checks_the_database() {
    let f = fixture();
    let (s, v) = send(test_app(&f), "GET", "/readyz", None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["checks"]["database"], "ok");
    assert_eq!(v["status"], "ok");

    // 把表拆了：readyz 必须转成 degraded
    f.state
        .conn()
        .execute_batch("DROP TABLE channels;")
        .unwrap();
    let (_, v) = send(test_app(&f), "GET", "/readyz", None, None).await;
    assert_eq!(v["checks"]["database"], "fail");
    assert_eq!(v["status"], "degraded", "库坏了还报 ok 就是假探活");
}

/// 三个对外端点：阶段 2 起是真的了，但**必须带对外 key**；路径本身绝不能是 404。
#[tokio::test]
async fn v1_endpoints_are_registered_and_gated_by_the_key() {
    let f = fixture();
    for (m, uri) in [
        ("POST", "/v1/chat/completions"),
        ("POST", "/v1/embeddings"),
        ("POST", "/v1/rerank"),
        ("GET", "/v1/models"),
    ] {
        let s = status_of(test_app(&f), m, uri).await;
        assert_eq!(
            s,
            StatusCode::UNAUTHORIZED,
            "{m} {uri} 已注册，没带对外 key 时必须是 401（不是 404 也不是 501）"
        );
    }
}

/// 本机不做的端点必须真 404，别让它们悄悄存在。
#[tokio::test]
async fn dropped_upstream_endpoints_are_really_absent() {
    let f = fixture();
    for (m, uri) in [
        ("POST", "/v1/moderations"),
        ("POST", "/v1/search"),
        ("POST", "/v1/responses"),
        ("POST", "/v1/messages"),
        ("POST", "/v1/images/generations"),
        ("POST", "/v1/audio/speech"),
        ("POST", "/v1/decisions"),
        ("POST", "/api/chat"),     // Ollama
        ("GET", "/api/tags"),      // Ollama
        ("GET", "/v1beta/models"), // Gemini
        ("POST", "/mcp"),          // MCP
    ] {
        assert_eq!(
            status_of(test_app(&f), m, uri).await,
            StatusCode::NOT_FOUND,
            "{m} {uri} 不在本机需求里，必须真 404"
        );
    }
}

#[tokio::test]
async fn unknown_path_is_404() {
    let f = fixture();
    assert_eq!(
        status_of(test_app(&f), "GET", "/does-not-exist").await,
        StatusCode::NOT_FOUND
    );
}

/// 面板管理面要有的接口（渠道管理、模型组、路由状态）。
#[tokio::test]
async fn admin_surface_exists() {
    let f = fixture();
    for uri in [
        "/api/channels",
        "/api/groups",
        "/api/routing",
        "/api/stats",
        "/api/logs",
        "/api/settings",
    ] {
        let s = status_of(test_app(&f), "GET", uri).await;
        assert_ne!(s, StatusCode::NOT_FOUND, "GET {uri} 应该存在");
    }
}

// ───────────────── 阶段 1：登录门 ─────────────────

/// 没登录时，管理面必须一律 401 —— 不能把渠道表和密钥配置暴露出去。
#[tokio::test]
async fn admin_surface_is_locked_without_a_session() {
    let f = fixture();
    for uri in [
        "/api/channels",
        "/api/groups",
        "/api/routing",
        "/api/stats",
        "/api/logs",
        "/api/settings",
    ] {
        assert_eq!(
            status_of(test_app(&f), "GET", uri).await,
            StatusCode::UNAUTHORIZED,
            "GET {uri} 未登录时必须 401"
        );
    }
    // 写操作同样
    assert_eq!(
        send(test_app(&f), "DELETE", "/api/channels/1", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_invented_token_is_refused() {
    let f = fixture();
    let s = send(test_app(&f), "GET", "/api/channels", Some("made-up-token"), None)
        .await
        .0;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn first_run_setup_then_use_the_panel() {
    let f = fixture();

    // 1) 还没初始化
    let (s, v) = send(test_app(&f), "GET", "/api/auth/status", None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["needs_setup"], true, "首次该提示去设置口令");

    // 2) 初始化：口令 + 沿用惯用的短 key
    let (s, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["gateway_key"], "test-password");
    assert_eq!(v["gateway_key_generated"], false, "手填的就不该自动生成");
    let token = v["token"].as_str().unwrap().to_string();

    // 3) 带上会话就能进管理面了
    let (s, v) = send(test_app(&f), "GET", "/api/channels", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["channels"].as_array().unwrap().len(), 0);

    // 4) 再初始化一次必须被拒
    let (s, _) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "other" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "已经设过口令就不许再初始化");
}

#[tokio::test]
async fn login_flow_and_wrong_password() {
    let f = fixture();
    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let first_token = v["token"].as_str().unwrap().to_string();

    // 错口令
    let (s, _) = send(
        test_app(&f),
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "password": "nope" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // 对口令
    let (s, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "password": "test-password" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let token = v["token"].as_str().unwrap().to_string();
    assert_ne!(token, first_token);

    // 退出后失效
    let (s, _) = send(
        test_app(&f),
        "POST",
        "/api/auth/logout",
        None,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(test_app(&f), "GET", "/api/channels", Some(&token), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "退出后的 token 不能再用");
}

// ───────────────── 阶段 1：渠道与模型组真的落库 ─────────────────

/// 端到端：建渠道（带自定义请求头和密钥）→ 建模型组 → 查回来。
/// 这条覆盖用户四项决议里的第 2、3 条（自定义请求头、全自定义端点和 key）。
#[tokio::test]
async fn create_a_channel_and_a_group_end_to_end() {
    let f = fixture();
    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let token = v["token"].as_str().unwrap().to_string();

    // 两条渠道：命令式上游那条需要自定义请求头 + 按渠道改模型名
    let (s, sf) = send(
        test_app(&f),
        "POST",
        "/api/channels",
        Some(&token),
        Some(json!({
            "name": "硅基流动",
            "base_url": "https://api.siliconflow.cn/v1",
            "key": "sk-sf-secret",
            "models": [{ "model": "bge-m3", "upstream": null }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sf}");
    assert_eq!(sf["has_secret"], true, "配了 key 就该显示已配");

    let (s, cg) = send(
        test_app(&f),
        "POST",
        "/api/channels",
        Some(&token),
        Some(json!({
            "name": "command-go",
            "base_url": "http://192.0.2.101:3159/v1",
            "key": "test-password",
            "headers": [
                { "name": "x-opencode-client", "value": "cli" },
                { "name": "x-opencode-session", "value": "{{uuid}}" }
            ],
            "models": [{ "model": "auto", "upstream": "deepseek/deepseek-v4.1-flash" }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{cg}");
    let sf_id = sf["id"].as_i64().unwrap();
    let cg_id = cg["id"].as_i64().unwrap();
    assert_eq!(
        cg["headers"].as_array().unwrap().len(),
        2,
        "自定义请求头要存下来"
    );

    // 响应里绝不能出现密钥明文
    let body = sf.to_string() + &cg.to_string();
    assert!(!body.contains("sk-sf-secret"), "接口回显里出现了密钥明文");

    // 建组：顺序即优先级，硅基流动首选
    let (s, _) = send(
        test_app(&f),
        "POST",
        "/api/groups",
        Some(&token),
        Some(json!({ "name": "auto", "channels": [sf_id, cg_id] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 查回来核对
    let (s, v) = send(test_app(&f), "GET", "/api/groups", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    let g = &v["groups"][0];
    assert_eq!(g["name"], "auto");
    assert_eq!(g["channels"], json!([sf_id, cg_id]));
    assert_eq!(
        v["servable"],
        json!([{"name": "auto", "vision": false}]),
        "只列真实能用的模型名，并如实说能不能看图"
    );
    assert_eq!(g["can_see"], json!(false), "这条组里没有标能看图的渠道");

    // 路由状态：首选是第一条，还没切过
    let (s, v) = send(test_app(&f), "GET", "/api/routing", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["groups"][0]["primary_channel_id"], json!(sf_id));
    assert_eq!(v["groups"][0]["switch_count"], 0);

    // 密钥在库里是密文：整个库文件的字节里搜不到明文
    let raw = std::fs::read(&f.state.cfg.db_path).unwrap();
    assert!(
        !raw.windows(12).any(|w| w == b"sk-sf-secret"),
        "库文件里出现了密钥明文"
    );
}

#[tokio::test]
async fn updating_a_channel_replaces_headers_and_can_clear_the_key() {
    let f = fixture();
    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let token = v["token"].as_str().unwrap().to_string();

    let (_, channel) = send(
        test_app(&f),
        "POST",
        "/api/channels",
        Some(&token),
        Some(json!({
            "name": "a", "base_url": "http://x/v1", "key": "sk-1",
            "headers": [{ "name": "H1", "value": "1" }]
        })),
    )
    .await;
    let id = channel["id"].as_i64().unwrap();

    // 不传 key 字段 → 密钥保持
    let (s, v) = send(
        test_app(&f),
        "PUT",
        &format!("/api/channels/{id}"),
        Some(&token),
        Some(json!({
            "name": "a2", "base_url": "http://y/v1", "enabled": false,
            "headers": [{ "name": "H2", "value": "2" }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["name"], "a2");
    assert_eq!(v["enabled"], false);
    assert_eq!(v["has_secret"], true, "没传 key 就不该动已存的密钥");
    assert_eq!(v["headers"].as_array().unwrap().len(), 1);
    assert_eq!(v["headers"][0]["name"], "H2", "请求头是整体替换");

    // 传空串 → 明确清掉密钥
    let (_, v) = send(
        test_app(&f),
        "PUT",
        &format!("/api/channels/{id}"),
        Some(&token),
        Some(json!({
            "name": "a2", "base_url": "http://y/v1", "enabled": true, "key": ""
        })),
    )
    .await;
    assert_eq!(v["has_secret"], false, "传空串要能清掉密钥");
}

/// 删渠道要连带把它从模型组里摘掉，不能留个悬空引用。
#[tokio::test]
async fn deleting_a_channel_removes_it_from_groups() {
    let f = fixture();
    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let token = v["token"].as_str().unwrap().to_string();

    let mut ids = vec![];
    for name in ["a", "b"] {
        let (_, channel) = send(
            test_app(&f),
            "POST",
            "/api/channels",
            Some(&token),
            Some(json!({ "name": name, "base_url": format!("http://{name}/v1") })),
        )
        .await;
        ids.push(channel["id"].as_i64().unwrap());
    }
    send(
        test_app(&f),
        "POST",
        "/api/groups",
        Some(&token),
        Some(json!({ "name": "auto", "channels": ids })),
    )
    .await;

    let (s, _) = send(
        test_app(&f),
        "DELETE",
        &format!("/api/channels/{}", ids[0]),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (_, v) = send(test_app(&f), "GET", "/api/groups", Some(&token), None).await;
    assert_eq!(v["groups"][0]["channels"], json!([ids[1]]));
}

/// 改口令要踢掉所有旧会话。
#[tokio::test]
async fn changing_the_password_invalidates_old_sessions() {
    let f = fixture();
    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let old = v["token"].as_str().unwrap().to_string();

    let (s, v) = send(
        test_app(&f),
        "PUT",
        "/api/auth/password",
        Some(&old),
        Some(json!({ "old_password": "test-password", "new_password": "test-password2" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let new = v["token"].as_str().unwrap().to_string();

    let (s, _) = send(test_app(&f), "GET", "/api/channels", Some(&old), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "旧会话必须失效");
    let (s, _) = send(test_app(&f), "GET", "/api/channels", Some(&new), None).await;
    assert_eq!(s, StatusCode::OK, "新会话要能用");

    // 旧口令不能再用
    let (s, _) = send(
        test_app(&f),
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "password": "test-password" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

/// 路由参数存库、重启还在。
#[tokio::test]
async fn routing_config_is_saved_and_reloaded() {
    let f = fixture();
    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let token = v["token"].as_str().unwrap().to_string();

    let (_, before) = send(test_app(&f), "GET", "/api/settings", Some(&token), None).await;
    assert_eq!(before["routing_config"]["retry_times"], 3);
    assert_eq!(before["reprobe_interval_minutes"], 10, "10 分钟回切");

    let mut cfg = before["routing_config"].clone();
    cfg["retry_times"] = json!(2);
    cfg["reprobe_interval_ms"] = json!(300_000);
    let (s, _) = send(
        test_app(&f),
        "PUT",
        "/api/routing/config",
        Some(&token),
        Some(cfg),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 重启
    let cfg_obj = f.state.cfg.clone();
    let restarted = AppState::init(cfg_obj).unwrap();
    assert_eq!(restarted.routing().cfg.retry_times, 2);
    assert_eq!(restarted.routing().cfg.reprobe_interval_ms, 300_000);
}

#[tokio::test]
async fn absurd_routing_config_is_rejected() {
    let f = fixture();
    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let token = v["token"].as_str().unwrap().to_string();

    let (s, _) = send(
        test_app(&f),
        "PUT",
        "/api/routing/config",
        Some(&token),
        Some(json!({ "retry_times": 999, "attempt_timeout_ms": 30000, "request_budget_ms": 90000, "reprobe_interval_ms": 600000, "probe_enabled": true })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

/// 记账接口能读回刚记的账。
#[tokio::test]
async fn stats_and_logs_read_from_the_database() {
    let f = fixture();
    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let token = v["token"].as_str().unwrap().to_string();

    // 模拟一条完整请求：渠道 1 失败两次，渠道 2 接住
    {
        let conn = f.state.conn();
        let id = tokengo_server::db::requests::start(&conn, "/v1/embeddings", "auto", Some("1.1.1.1"))
            .unwrap();
        tokengo_server::db::requests::add_attempt(&conn, id, 0, 1, "channel_fault", Some(429), 12, None)
            .unwrap();
        tokengo_server::db::requests::add_attempt(&conn, id, 1, 1, "channel_fault", Some(500), 20, None)
            .unwrap();
        tokengo_server::db::requests::add_attempt(&conn, id, 2, 2, "success", Some(200), 44, None)
            .unwrap();
        tokengo_server::db::requests::finish(
            &conn,
            id,
            tokengo_server::db::requests::Status::Ok,
            Some(200),
            Some(2),
            76,
            Some(11),
            7,
            0,
            false,
            None,
        )
        .unwrap();
    }

    let (s, v) = send(test_app(&f), "GET", "/api/stats", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["stats"]["total"], 1);
    assert_eq!(v["stats"]["ok"], 1);
    let by_channel = v["stats"]["by_channel"].as_array().unwrap();
    assert_eq!(by_channel.len(), 2);
    let first = &by_channel[0];
    assert_eq!(first["channel_id"], 1);
    assert_eq!(first["attempts"], 2, "分母是尝试次数，不是成功次数");
    assert_eq!(first["successes"], 0);
    assert_eq!(first["success_rate"], 0.0, "一次都没成的渠道成功率必须是 0");
    // 没有名字的渠道要能兜住，不能报错
    assert!(first["channel_name"].is_string());

    let (s, v) = send(test_app(&f), "GET", "/api/logs?limit=5", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["logs"].as_array().unwrap().len(), 1);
    assert_eq!(v["logs"][0]["latency_ms"], 76);
    assert_eq!(v["logs"][0]["input_tokens"], 7);
}

/// 手动回切：把检查点归零，下次请求先试首选。
#[tokio::test]
async fn manual_reprobe_resets_the_checkpoint() {
    let f = fixture();
    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let token = v["token"].as_str().unwrap().to_string();

    let mut ids = vec![];
    for name in ["a", "b"] {
        let (_, channel) = send(
            test_app(&f),
            "POST",
            "/api/channels",
            Some(&token),
            Some(json!({ "name": name, "base_url": format!("http://{name}/v1") })),
        )
        .await;
        ids.push(channel["id"].as_i64().unwrap());
    }
    send(
        test_app(&f),
        "POST",
        "/api/groups",
        Some(&token),
        Some(json!({ "name": "auto", "channels": ids })),
    )
    .await;

    // 制造一次切换
    {
        let channels = vec![
            tokengo_shared::ChannelId(ids[0]),
            tokengo_shared::ChannelId(ids[1]),
        ];
        let mut r = f.state.routing();
        r.attempt_order("auto", &channels, 1_800_000_000_000);
        r.on_channel_exhausted("auto", channels[0], &channels, 1_800_000_000_000);
    }
    f.state.persist_state("auto").unwrap();
    let (_, v) = send(test_app(&f), "GET", "/api/routing", Some(&token), None).await;
    assert_eq!(v["groups"][0]["switched"], true);
    assert!(v["groups"][0]["next_probe_at_ms"].is_i64());

    // 手动回切
    let (s, _) = send(
        test_app(&f),
        "POST",
        "/api/routing/auto/reprobe",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (_, v) = send(test_app(&f), "GET", "/api/routing", Some(&token), None).await;
    let probe_at = v["groups"][0]["next_probe_at_ms"].as_i64().unwrap();
    let now = v["groups"][0]["now_ms"].as_i64().unwrap();
    assert!(probe_at <= now, "检查点该归零了，实际 {probe_at} vs {now}");

    // 不存在的组要报 404
    let (s, _) = send(
        test_app(&f),
        "POST",
        "/api/routing/nope/reprobe",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// 面板上那个"测一下"按钮（阶段 2 起是真探活了）。
/// 这里只钉"接口在、要登录、渠道不存在要 404"；真探通的用例在 `tests/proxy.rs`。
#[tokio::test]
async fn channel_probe_exists_and_needs_a_session() {
    let f = fixture();
    let (s, _) = send(test_app(&f), "POST", "/api/channels/1/probe", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "探活要登录");

    let (_, v) = send(
        test_app(&f),
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({ "password": "test-password", "gateway_key": "test-password" })),
    )
    .await;
    let token = v["token"].as_str().unwrap().to_string();
    let (s, _) = send(
        test_app(&f),
        "POST",
        "/api/channels/1/probe",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "渠道不存在就 404，不许假绿点");
}
