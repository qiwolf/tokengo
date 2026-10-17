//! 阶段 2 的真机回归：**假上游真起 HTTP 服务**，网关真发请求出去。
//!
//! 这里不用 mock 框架、不 mock HTTP 客户端 —— 起一个真的 axum 假上游监听 127.0.0.1:0，
//! 让 tokengo 走真的 TCP 去打它。理由：上一轮踩过的坑（连接错端口、超时不生效、
//! 流式被缓冲、请求头没带出去）全都是**只有真发请求才暴露**的。
//!
//! 覆盖 `docs/03-路由策略.md` 第八节的全部验收点：
//!   1/2/3 首选渠道真坏（指向没人监听的端口）→ 4 次尝试 → 切到第二优先 → 客户端拿到成功
//!   4     到点后自动回切首选（后台探活）
//!   5     反证：连发错模型名（400）不许换渠道
//!   6     反证：客户端断开不计渠道的账
//!
//! 另外还钉住几条纪律：对外 key 必须校验、`/v1/models` 只列真能用的、
//! 自定义请求头 + 按渠道改名确实发出去了、流式不会被缓存。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::post as route_post;
use axum::Router;
use serde_json::{json, Value};
use tokengo_server::{app, state::AppState, Config};
use tokengo_shared::ChannelId;
use tower::ServiceExt;

// ───────────────────────── 假上游 ─────────────────────────

#[derive(Clone, Debug)]
enum Reply {
    /// 正常的 OpenAI 形状响应（带 usage）
    Ok,
    /// 指定状态码 + 正文
    Status(u16, &'static str),
    /// SSE 流（带一段中文与 usage）
    Sse,
    /// 先睡一会儿再回 200 —— 用来测"客户端中途断开"
    Slow(u64),
    /// 假装"真看见了那张图"：回一个"红"字（「测视觉」判对错用的就是这个词）
    SawRed,
    /// 请求里 `max_tokens` 小于 N 就只说半句（`finish_reason=length`），给够了才答"红"。
    /// 用来演「额度给少了导致假红」和「多给额度就答对」。
    TruncatesBelow(u32),
    /// 非流式响应外面套一层壳（cline.bot 的真形状）：`{"data": {…}, "success": true}`。
    /// 「响应解包」勾上才该剥掉。
    Enveloped,
    /// 200 但一句话都没说（没 content、没 tool_calls）—— 现网 4 天 9 次那种。
    Empty,
    /// 429 + `Retry-After`（秒）。上游自己说「过多久再来」。
    RateLimited(u64),
}

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    model: String,
    auth: Option<String>,
    custom: Vec<(String, String)>,
    /// 这次请求的正文里有没有图片内容块 —— 「带图只走能看图的渠道」全靠它证明。
    has_image: bool,
    /// 发过来的完整请求体 —— 「哪些字段被丢掉了」只能由上游侧回看证明，
    /// 看网关自己的日志不算。
    body: Value,
}

struct MockInner {
    replies: Mutex<VecDeque<Reply>>,
    default: Reply,
    seen: Mutex<Vec<Seen>>,
}

#[derive(Clone)]
struct Mock {
    inner: Arc<MockInner>,
    base: String,
}

impl Mock {
    fn seen(&self) -> Vec<Seen> {
        self.inner.seen.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.inner.seen.lock().unwrap().len()
    }
}

async fn start_mock(default: Reply) -> Mock {
    let inner = Arc::new(MockInner {
        replies: Mutex::new(VecDeque::new()),
        default,
        seen: Mutex::new(Vec::new()),
    });
    let app = Router::new()
        .route("/v1/chat/completions", route_post(mock_handler))
        .route("/v1/embeddings", route_post(mock_handler))
        .route("/v1/models", axum::routing::get(mock_models))
        .with_state(inner.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Mock {
        inner,
        base: format!("http://127.0.0.1:{port}/v1"),
    }
}

async fn mock_handler(
    State(inner): State<Arc<MockInner>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("");
    record(&inner, &headers, &uri, model, body_has_image(&v), &v);

    let reply = next_reply(&inner);

    match reply {
        Reply::Ok => axum::Json(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "model": "mock",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "你好"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18}
        }))
        .into_response(),
        Reply::SawRed => axum::Json(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "model": "mock",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "红"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 1, "total_tokens": 13}
        }))
        .into_response(),
        // 演「额度给少了」：请求里 max_tokens 小于 N 就只说半句（finish_reason=length），
        // 给够了才答「红」。**别把额度压到 8** 的回归就靠它 —— 8 那版在这儿必然被截断。
        Reply::TruncatesBelow(n) => {
            let given = v.get("max_tokens").and_then(|m| m.as_u64()).unwrap_or(0);
            if given < n as u64 {
                axum::Json(json!({
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "model": "mock",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "这张图的左上角那一"},
                                 "finish_reason": "length"}],
                    "usage": {"prompt_tokens": 12, "completion_tokens": given, "total_tokens": 12 + given}
                }))
                .into_response()
            } else {
                axum::Json(json!({
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "model": "mock",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "红"}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 1, "total_tokens": 13}
                }))
                .into_response()
            }
        }
        Reply::Enveloped => axum::Json(json!({
            "data": {
                "id": "chatcmpl-wrapped",
                "object": "chat.completion",
                "model": "mock",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "壳里的你好"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 31, "completion_tokens": 4, "total_tokens": 35}
            },
            "success": true
        }))
        .into_response(),
        Reply::Status(code, body) => (
            StatusCode::from_u16(code).unwrap(),
            body.to_string(),
        )
            .into_response(),
        Reply::Empty => axum::Json(json!({
            "id": "chatcmpl-empty",
            "object": "chat.completion",
            "model": "mock",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": ""}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 0}
        }))
        .into_response(),
        Reply::RateLimited(secs) => Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("retry-after", secs.to_string())
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"error":{"message":"Go usage limit exceeded"}}"#,
            ))
            .unwrap(),
        Reply::Slow(ms) => {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            axum::Json(json!({"choices": [], "usage": {}})).into_response()
        }
        Reply::Sse => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from(
                "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"content\":\"世界\"}}]}\n\n\
                 data: {\"usage\":{\"prompt_tokens\":21,\"completion_tokens\":13}}\n\n\
                 data: [DONE]\n\n",
            ))
            .unwrap(),
    }
}

/// 假上游的 `/v1/models`：默认回一份 OpenAI 形状的清单；
/// 队列里排了 `Status(…)` 就先按那个回（用来演"上游压根没做这个端点"）。
async fn mock_models(State(inner): State<Arc<MockInner>>, headers: HeaderMap, uri: Uri) -> Response {
    record(&inner, &headers, &uri, "", false, &Value::Null);
    match next_reply(&inner) {
        Reply::Status(code, body) => {
            (StatusCode::from_u16(code).unwrap(), body.to_string()).into_response()
        }
        _ => axum::Json(json!({
            "object": "list",
            "data": [
                {"id": "mock-chat-a", "object": "model"},
                {"id": "mock-chat-a", "object": "model"},
                {"id": "mock-embed-b", "object": "model"}
            ]
        }))
        .into_response(),
    }
}

/// 每次请求都记一条：路径、模型名、Authorization、自定义头、有没有带图。
/// GET 没有正文，模型名就记空的。
fn record(
    inner: &Arc<MockInner>,
    headers: &HeaderMap,
    uri: &Uri,
    model: &str,
    has_image: bool,
    body: &Value,
) {
    let custom = headers
        .iter()
        .filter(|(k, _)| k.as_str().starts_with("x-"))
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect::<Vec<_>>();
    inner.seen.lock().unwrap().push(Seen {
        path: uri.path().to_string(),
        model: model.to_string(),
        has_image,
        body: body.clone(),
        auth: headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .map(str::to_string),
        custom,
    });
}

fn next_reply(inner: &Arc<MockInner>) -> Reply {
    inner
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| inner.default.clone())
}

// ───────────────────────── 网关侧夹具 ─────────────────────────

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
    let dir = std::env::temp_dir().join(format!("tokengo-proxy-{}", uuid::Uuid::new_v4()));
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

const KEY: &str = "test-password";

impl Fixture {
    /// 配好对外 key（客户端拿它连网关）。
    fn with_key(&self) {
        let conn = self.state.conn();
        tokengo_server::db::sessions::set_gateway_key(&conn, KEY).unwrap();
    }

    fn add_channel(&self, name: &str, base_url: &str, spec: ChannelSpec) -> ChannelId {
        let conn = self.state.conn();
        let id = tokengo_server::db::channels::create(
            &conn,
            &tokengo_server::db::channels::NewChannel {
                name: name.into(),
                base_url: base_url.into(),
                enabled: true,
                unwrap_envelope: spec.unwrap_envelope,
            },
        )
        .unwrap();
        if let Some(s) = spec.secret {
            tokengo_server::db::channels::set_secret(&conn, &self.state.secrets, id, &s).unwrap();
        }
        tokengo_server::db::channels::set_headers(&conn, id, &spec.headers).unwrap();
        let models: Vec<_> = spec
            .models
            .iter()
            .map(|(m, u)| {
                let mut mapping = tokengo_server::db::channels::ModelMapping::new(
                    m.clone(),
                    u.clone(),
                    spec.vision.iter().any(|v| v == m),
                );
                mapping.drop_fields = spec
                    .drop_fields
                    .iter()
                    .find(|(name, _)| name == m)
                    .map(|(_, fields)| fields.clone())
                    .unwrap_or_default();
                mapping
            })
            .collect();
        tokengo_server::db::channels::set_models(&conn, id, &models).unwrap();
        id
    }

    fn set_group(&self, name: &str, channels: &[ChannelId]) {
        tokengo_server::db::groups::upsert(&self.state.conn(), name, true, channels).unwrap();
    }

    fn fix_channel(&self, id: ChannelId, base_url: &str) {
        tokengo_server::db::channels::update(&self.state.conn(), id, "修好的渠道", base_url, true, false)
            .unwrap();
    }

    fn routing(&self, group: &str) -> tokengo_shared::routing::GroupState {
        self.state
            .routing()
            .state(group)
            .cloned()
            .unwrap_or_default()
    }

    fn request_rows(&self) -> Vec<(String, Option<i64>, i64, i64, i64, Option<String>)> {
        let conn = self.state.conn();
        let mut stmt = conn
            .prepare("SELECT status, channel_id, input_tokens, output_tokens, streamed, error FROM requests ORDER BY id")
            .unwrap();
        let v = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        v
    }

    fn attempts(&self) -> Vec<(i64, String, Option<i64>)> {
        let conn = self.state.conn();
        let mut stmt = conn
            .prepare("SELECT channel_id, outcome, http_status FROM attempts ORDER BY request_id, ordinal")
            .unwrap();
        let v = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        v
    }
}

#[derive(Default, Clone)]
struct ChannelSpec {
    secret: Option<String>,
    headers: Vec<(String, String)>,
    models: Vec<(String, Option<String>)>,
    /// 这些模型名在这条渠道上标「能看图」（写模型名，就是 models 里的第一个字段）。
    vision: Vec<String>,
    /// 这条渠道上某个模型名要丢掉的请求体顶层字段：`(模型名, [字段名, …])`。
    drop_fields: Vec<(String, Vec<String>)>,
    /// 这条渠道勾了「响应解包」。
    unwrap_envelope: bool,
}

impl ChannelSpec {
    /// 标一个模型名「能看图」。链式用：`ChannelSpec { ..Default::default() }.with_vision("auto")`
    fn with_vision(mut self, model: &str) -> Self {
        self.vision.push(model.into());
        self
    }

    /// 给某个模型名配「要丢的字段」。链式用：`.with_drop_fields("beam-free", &["reasoning_effort"])`
    fn with_drop_fields(mut self, model: &str, fields: &[&str]) -> Self {
        self.drop_fields
            .push((model.into(), fields.iter().map(|f| (*f).to_string()).collect()));
        self
    }
}

async fn post(
    f: &Fixture,
    uri: &str,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Bytes, Option<String>) {
    let mut b = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(k) = key {
        b = b.header("authorization", format!("Bearer {k}"));
    }
    let req = b.body(axum::body::Body::from(body.to_string())).unwrap();
    let resp = app(f.state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, bytes, ctype)
}

async fn get(f: &Fixture, uri: &str, key: Option<&str>) -> (StatusCode, Value) {
    let mut b = axum::http::Request::builder().method("GET").uri(uri);
    if let Some(k) = key {
        b = b.header("authorization", format!("Bearer {k}"));
    }
    let resp = app(f.state.clone())
        .oneshot(b.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

const CHAT: &str = "/v1/chat/completions";

// ───────────────────────── 验收点 5：对外 key ─────────────────────────

#[tokio::test]
async fn v1_needs_the_gateway_key() {
    let f = fixture();
    let m = start_mock(Reply::Ok).await;
    f.with_key();
    let channel = f.add_channel("mock", &m.base, ChannelSpec::default());
    f.set_group("auto", &[channel]);

    // 没带 key
    let (s, _, _) = post(&f, CHAT, None, json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "没 key 不许转发");

    // key 不对
    let (s, _, _) = post(&f, CHAT, Some("wrong"), json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // 带上就通
    let (s, _, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(m.count(), 1, "只应该真发出去一次");
}

/// 反证：一个 key 都没配的时候必须一律拒绝，绝不能"没配就放行"。
#[tokio::test]
async fn without_any_configured_key_everything_is_refused() {
    let f = fixture();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel("mock", &m.base, ChannelSpec::default());
    f.set_group("auto", &[channel]);

    let (s, _, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(m.count(), 0, "没配 key 就一个字节都不该发出去");
}

// ───────────────────────── 正常转发 + 记账 ─────────────────────────

#[tokio::test]
async fn a_plain_chat_request_is_forwarded_and_accounted() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel("mock", &m.base, ChannelSpec { secret: Some("sk-mock".into()), ..Default::default() });
    f.set_group("auto", &[channel]);

    let (s, body, ctype) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "你好"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(ctype.unwrap().contains("json"));
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["choices"][0]["message"]["content"], "你好");

    // 上游确实收到了：路径对、正文里的 model 是 auto、带上了这条渠道的 key
    let seen = m.seen();
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(seen[0].model, "auto");
    assert_eq!(seen[0].auth.as_deref(), Some("Bearer sk-mock"));

    // 记账：一条请求 + 一条尝试，token 记上了
    let rows = f.request_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "ok");
    assert_eq!(rows[0].1, Some(channel.0));
    assert_eq!((rows[0].2, rows[0].3), (11, 7), "token 账");
    assert_eq!(f.attempts(), vec![(channel.0, "success".to_string(), Some(200))]);
}

/// 按渠道改名 + 自定义请求头（含逐请求模板）真的发出去了。
#[tokio::test]
async fn per_channel_model_names_and_custom_headers_reach_the_upstream() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel(
        "command-go",
        &m.base,
        ChannelSpec {
            secret: Some("sk-cg".into()),
            headers: vec![
                ("x-opencode-client".into(), "cli".into()),
                ("x-opencode-session".into(), "{{uuid}}".into()),
            ],
            models: vec![("auto".into(), Some("deepseek/deepseek-v4.1-flash".into()))],
            vision: vec![],
            ..Default::default()
        },
    );
    f.set_group("auto", &[channel]);

    for _ in 0..2 {
        let (s, _, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
        assert_eq!(s, StatusCode::OK);
    }

    let seen = m.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].model, "deepseek/deepseek-v4.1-flash",
        "这条渠道认的上游模型名要替换上去"
    );
    let sess = |i: usize| {
        seen[i]
            .custom
            .iter()
            .find(|(k, _)| k == "x-opencode-session")
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_ne!(sess(0), sess(1), "{{uuid}} 必须逐请求换新的");
    assert!(seen[0]
        .custom
        .iter()
        .any(|(k, v)| k == "x-opencode-client" && v == "cli"));
}

// ───────────────────────── 验收点 1/2/3：真故障切换 ─────────────────────────

#[tokio::test]
async fn a_dead_primary_is_retried_then_the_next_channel_takes_over() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let dead = f.add_channel("坏渠道", "http://127.0.0.1:59999/v1", ChannelSpec::default());
    let good = f.add_channel("好渠道", &m.base, ChannelSpec::default());
    f.set_group("auto", &[dead, good]);

    let (s, body, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::OK, "客户端必须拿到成功响应");
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["choices"][0]["message"]["content"], "你好");

    // 尝试级：坏渠道 4 次全败（1 + 3 次重试），好渠道 1 次成功
    let att = f.attempts();
    assert_eq!(att.len(), 5, "应该是 4 次重试 + 1 次成功：{att:?}");
    assert!(att[..4].iter().all(|(l, o, _)| *l == dead.0 && o == "channel_fault"));
    assert_eq!(att[4], (good.0, "success".to_string(), Some(200)));
    assert_eq!(m.count(), 1);

    // 路由状态：切到第二条渠道、检查点 +10 分钟、切换次数 1
    let st = f.routing("auto");
    assert_eq!(st.active, Some(good), "当前渠道应该是第二条");
    assert_eq!(st.switch_count, 1);
    let now = tokengo_server::db::now_ms();
    let probe = st.next_probe_at_ms.expect("要记下回切检查点");
    let delta = probe - now;
    assert!(
        (9 * 60 * 1000..=10 * 60 * 1000 + 5000).contains(&delta),
        "检查点该在 10 分钟后，实际差 {delta} ms"
    );

    // 第二个请求应该直接用第二条渠道，不再去戳坏渠道
    let (s, _, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(m.count(), 2);
    assert_eq!(f.attempts().len(), 6, "第二个请求只该有 1 次尝试");
}

/// 所有渠道都坏 → 客户端拿到明确的 502，账上记 failed。
#[tokio::test]
async fn everything_down_returns_a_clear_502() {
    let f = fixture();
    f.with_key();
    let a = f.add_channel("坏渠道A", "http://127.0.0.1:59998/v1", ChannelSpec::default());
    let b = f.add_channel("坏渠道B", "http://127.0.0.1:59997/v1", ChannelSpec::default());
    f.set_group("auto", &[a, b]);

    let (s, body, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    let v: Value = serde_json::from_slice(&body).unwrap();
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("所有渠道都失败了"), "错误要说清楚：{msg}");

    assert_eq!(f.attempts().len(), 8, "两条渠道各 4 次尝试");
    let rows = f.request_rows();
    assert_eq!(rows[0].0, "failed");
    assert!(rows[0].5.as_deref().unwrap().contains("坏渠道B"));
}

/// 上游真回了 500/429（不是网络错）也一样算渠道的毛病，重试 + 切换。
#[tokio::test]
async fn upstream_5xx_is_a_channel_fault_and_triggers_a_switch() {
    let f = fixture();
    f.with_key();
    let bad = start_mock(Reply::Status(500, "upstream exploded")).await;
    let good = start_mock(Reply::Ok).await;
    let a = f.add_channel("会500的渠道", &bad.base, ChannelSpec::default());
    let b = f.add_channel("好渠道", &good.base, ChannelSpec::default());
    f.set_group("auto", &[a, b]);

    let (s, _, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(bad.count(), 4, "5xx 要重试满 4 次");
    assert_eq!(good.count(), 1);
    assert_eq!(f.routing("auto").active, Some(b));
}

// ───────────────────────── 验收点 5：反证 ─────────────────────────

/// 反证（关键）：客户端自己写错（400）**不许**重试、不许换渠道。
/// 上一轮就是因为把 4xx 计进冷却，客户端连发几次错模型名就把唯一一条向量渠道打死了。
#[tokio::test]
async fn client_errors_are_returned_as_is_and_never_switch_channels() {
    let f = fixture();
    f.with_key();
    let bad = start_mock(Reply::Status(400, r#"{"error":{"message":"model not found"}}"#)).await;
    let good = start_mock(Reply::Ok).await;
    let a = f.add_channel("首选", &bad.base, ChannelSpec::default());
    let b = f.add_channel("备选", &good.base, ChannelSpec::default());
    f.set_group("auto", &[a, b]);

    for _ in 0..6 {
        let (s, body, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "上游的 400 要原样传给客户端");
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert!(v["error"]["message"].as_str().unwrap().contains("model not found"));
    }

    assert_eq!(bad.count(), 6, "每次只该打一次（不重试）");
    assert_eq!(good.count(), 0, "备选渠道一次都不该被叫");
    assert_eq!(f.attempts().len(), 6, "六次请求 = 六次尝试，没有重试");
    assert!(
        f.attempts().iter().all(|(_, o, _)| o == "client_fault"),
        "要记成 client_fault，不能算渠道的毛病"
    );

    let st = f.routing("auto");
    assert_eq!(st.active, Some(a), "渠道必须原地不动");
    assert_eq!(st.switch_count, 0);
    assert_eq!(st.next_probe_at_ms, None);

    // 账上也不该算成渠道的锅
    let rows = f.request_rows();
    assert!(rows.iter().all(|r| r.0 == "client_error"), "{rows:?}");
}

/// 模型名压根没配 → 404，一次上游请求都不发。
#[tokio::test]
async fn an_unknown_model_is_404_without_touching_any_channel() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel("mock", &m.base, ChannelSpec::default());
    f.set_group("auto", &[channel]);

    let (s, body, _) = post(&f, CHAT, Some(KEY), json!({"model": "no-such", "messages": []})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let v: Value = serde_json::from_slice(&body).unwrap();
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("没有配置"));
    // **别在报错里点名某个具体模型名**（老文案写着「或者打 auto」）：线上不一定有那个名字，
    // 用户照着打反而又撞一个 404。
    assert!(!msg.contains("auto"), "报错不该点名某个模型名：{msg}");
    assert_eq!(m.count(), 0);
    assert!(f.request_rows().is_empty(), "没试过就不该建记账行");
}

/// 组里全是停用的渠道 = 对外不该暴露，打了要 404 而不是 500。
#[tokio::test]
async fn a_group_with_only_disabled_channels_is_404() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel("停用的渠道", &m.base, ChannelSpec::default());
    f.set_group("auto", &[channel]);
    tokengo_server::db::channels::update(&f.state.conn(), channel, "停用的渠道", &m.base, false, false).unwrap();

    let (s, _, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(m.count(), 0, "停用的渠道一个字节都不许发出去");
}

/// 请求体不是 JSON / 缺 model：这是客户端自己的错，400，且不碰渠道。
#[tokio::test]
async fn a_malformed_body_is_a_400() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel("mock", &m.base, ChannelSpec::default());
    f.set_group("auto", &[channel]);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri(CHAT)
        .header("authorization", format!("Bearer {KEY}"))
        .body(axum::body::Body::from("这不是 json"))
        .unwrap();
    let resp = app(f.state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let (s, _, _) = post(&f, CHAT, Some(KEY), json!({"messages": []})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "缺 model 也是 400");
    assert_eq!(m.count(), 0);
}

// ───────────────────────── 流式 ─────────────────────────

#[tokio::test]
async fn streaming_is_passed_through_and_usage_is_scraped() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    m.inner.replies.lock().unwrap().push_back(Reply::Sse);
    let channel = f.add_channel("mock", &m.base, ChannelSpec::default());
    f.set_group("auto", &[channel]);

    let (s, body, ctype) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "stream": true, "messages": []}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        ctype.unwrap().contains("text/event-stream"),
        "流式的 content-type 要传下去"
    );
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("你好"), "内容要真转发：{text}");
    assert!(text.contains("[DONE]"));

    let rows = f.request_rows();
    assert_eq!(rows[0].0, "ok");
    assert_eq!(rows[0].4, 1, "要记成流式");
    assert_eq!((rows[0].2, rows[0].3), (21, 13), "SSE 里的 usage 要抠出来记账");
}

// ───────────────────────── 验收点 6：客户端断开 ─────────────────────────

/// 反证：客户端中途断开，**不算渠道的账**，请求记成 canceled。
#[tokio::test]
async fn a_client_that_hangs_up_is_recorded_as_canceled() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    m.inner.replies.lock().unwrap().push_back(Reply::Slow(1500));
    let channel = f.add_channel("慢渠道", &m.base, ChannelSpec::default());
    f.set_group("auto", &[channel]);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri(CHAT)
        .header("authorization", format!("Bearer {KEY}"))
        .body(axum::body::Body::from(
            json!({"model": "auto", "messages": []}).to_string(),
        ))
        .unwrap();
    let state = f.state.clone();
    let handle = tokio::spawn(async move {
        let _ = app(state).oneshot(req).await;
    });

    // 等它真打到上游了再"拔线"
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while m.count() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(m.count(), 1, "上游应该已经收到了请求");
    handle.abort();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let rows = f.request_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].0, "canceled",
        "客户端断开要记 canceled（默认的 failed 会冤枉这条渠道）：{rows:?}"
    );
    assert!(
        rows[0].5.as_deref().unwrap().contains("客户端断开"),
        "要说清是客户端断了：{:?}",
        rows[0].5
    );
}

// ───────────────────────── 验收点 4：后台探活回切 ─────────────────────────

#[tokio::test]
async fn the_prober_switches_back_once_the_primary_recovers() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let first = f.add_channel(
        "首选",
        "http://127.0.0.1:59996/v1",
        ChannelSpec {
            models: vec![("auto".into(), Some("real-model".into()))],
            ..Default::default()
        },
    );
    let second = f.add_channel("备选", &m.base, ChannelSpec::default());
    f.set_group("auto", &[first, second]);

    // 先让首选真坏一次，制造一次切换
    let (s, _, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(f.routing("auto").active, Some(second));

    // 首选修好了 + 把检查点提前到点
    f.fix_channel(first, &m.base);
    {
        let now = tokengo_server::db::now_ms();
        let mut st = f.routing("auto");
        st.next_probe_at_ms = Some(now - 1);
        f.state.routing().restore("auto", st);
    }

    let out = tokengo_server::prober::tick(&f.state, tokengo_server::db::now_ms()).await;
    assert_eq!(out.len(), 1, "到点的组应该被探：{out:?}");
    assert_eq!(
        out[0].outcome,
        tokengo_server::prober::ProbeOutcome::SwitchedBack
    );

    let st = f.routing("auto");
    assert_eq!(st.active, Some(first), "首选活了就该切回来");
    assert_eq!(st.next_probe_at_ms, None, "回切后清掉检查点");
    assert_eq!(st.switch_count, 2, "切走一次、切回来一次");
    assert_eq!(f.request_rows().len(), 1, "探活不许写记账");
}

/// 反证：探活本身被上游判成 400（模型名不对）时**什么都不许动** ——
/// 不能凭一个 400 就把账号切成坏的，也不能把检查点推走。
#[tokio::test]
async fn an_inconclusive_probe_changes_nothing() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Status(400, "bad model name")).await;
    let first = f.add_channel(
        "首选",
        &m.base,
        ChannelSpec {
            models: vec![("auto".into(), Some("real-model".into()))],
            ..Default::default()
        },
    );
    let second = f.add_channel("备选", "http://127.0.0.1:59995/v1", ChannelSpec::default());
    f.set_group("auto", &[first, second]);

    // 手工把状态摆成"切到了备选、检查点到点"
    let now = tokengo_server::db::now_ms();
    {
        let ids = [first, second];
        let mut r = f.state.routing();
        r.attempt_order("auto", &ids, now);
        r.on_channel_exhausted("auto", first, &ids, now);
    }
    let before = f.routing("auto");
    {
        let mut st = before.clone();
        st.next_probe_at_ms = Some(now - 1);
        f.state.routing().restore("auto", st);
    }

    let out = tokengo_server::prober::tick(&f.state, now).await;
    assert_eq!(out.len(), 1);
    assert!(
        matches!(
            out[0].outcome,
            tokengo_server::prober::ProbeOutcome::Inconclusive(_)
        ),
        "400 只能得出「没结论」：{out:?}"
    );
    let after = f.routing("auto");
    assert_eq!(after.active, Some(second), "当前渠道不许变");
    assert_eq!(after.switch_count, before.switch_count, "切换次数不许变");
}

/// 探活真失败（连不上）→ 检查点往后推一个间隔，状态机不会变成"每次请求都戳坏渠道"。
#[tokio::test]
async fn a_failing_probe_pushes_the_checkpoint_forward() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let first = f.add_channel(
        "首选",
        "http://127.0.0.1:59994/v1",
        ChannelSpec {
            models: vec![("auto".into(), Some("real-model".into()))],
            ..Default::default()
        },
    );
    let second = f.add_channel("备选", &m.base, ChannelSpec::default());
    f.set_group("auto", &[first, second]);

    let now = tokengo_server::db::now_ms();
    {
        let ids = [first, second];
        let mut r = f.state.routing();
        r.attempt_order("auto", &ids, now);
        r.on_channel_exhausted("auto", first, &ids, now);
    }
    {
        let mut st = f.routing("auto");
        st.next_probe_at_ms = Some(now - 1);
        f.state.routing().restore("auto", st);
    }

    let out = tokengo_server::prober::tick(&f.state, now).await;
    assert_eq!(out[0].outcome, tokengo_server::prober::ProbeOutcome::StillDown);

    let st = f.routing("auto");
    assert_eq!(st.active, Some(second));
    let probe = st.next_probe_at_ms.unwrap();
    assert!(probe > now + 9 * 60 * 1000, "检查点要往后推，实际 {probe}");
}

/// 探活不许拿虚拟组名 `auto` 去发（那一定 400）：没配模型名就跳过并说明原因。
#[tokio::test]
async fn probing_skips_a_channel_without_any_model_name() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel("没配模型名的渠道", &m.base, ChannelSpec::default());
    f.set_group("auto", &[channel]);
    {
        let mut r = f.state.routing();
        r.restore(
            "auto",
            tokengo_shared::routing::GroupState {
                active: Some(channel),
                next_probe_at_ms: Some(tokengo_server::db::now_ms() - 1),
                switch_count: 0,
                last_switch_at_ms: None,
            },
        );
    }

    let out = tokengo_server::prober::tick(&f.state, tokengo_server::db::now_ms()).await;
    assert!(
        matches!(
            out[0].outcome,
            tokengo_server::prober::ProbeOutcome::Skipped(_)
        ),
        "{out:?}"
    );
    assert_eq!(m.count(), 0, "拿 auto 去探是白探，不如不探");
}

// ───────────────────────── /v1/models ─────────────────────────

#[tokio::test]
async fn models_lists_only_what_really_works() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let a = f.add_channel("a", &m.base, ChannelSpec::default());
    let b = f.add_channel("b", "http://127.0.0.1:59993/v1", ChannelSpec::default());
    f.set_group("auto", &[a]);
    f.set_group("bge-m3", &[a, b]);
    f.set_group("坏组", &[b]);
    tokengo_server::db::channels::update(&f.state.conn(), b, "b", "http://127.0.0.1:59993/v1", false, false)
        .unwrap();

    let (s, v) = get(&f, "/v1/models", Some(KEY)).await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<String> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["auto".to_string(), "bge-m3".to_string()]);

    // 没 key 不给看
    let (s, _) = get(&f, "/v1/models", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

// ───────────────────────── 向量端点 ─────────────────────────

#[tokio::test]
async fn embeddings_go_to_their_own_group() {
    let f = fixture();
    f.with_key();
    let chat_mock = start_mock(Reply::Ok).await;
    let vec_mock = start_mock(Reply::Ok).await;
    let chat_channel = f.add_channel("对话渠道", &chat_mock.base, ChannelSpec::default());
    let vec_channel = f.add_channel("向量渠道", &vec_mock.base, ChannelSpec::default());
    f.set_group("auto", &[chat_channel]);
    f.set_group("bge-m3", &[vec_channel]);

    let (s, _, _) = post(&f, "/v1/embeddings", Some(KEY), json!({"model": "bge-m3", "input": "你好"})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(vec_mock.count(), 1);
    assert_eq!(chat_mock.count(), 0, "向量请求不许跑到对话渠道上");
    let seen = vec_mock.seen();
    assert_eq!(seen[0].path, "/v1/embeddings");
}

/// 记账里"没试过就不建行"这条纪律：一次上游都没打，不该有 requests 行。
#[tokio::test]
async fn failed_before_any_attempt_leaves_no_rows() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel("停用的渠道", &m.base, ChannelSpec::default());
    f.set_group("auto", &[channel]);
    tokengo_server::db::channels::update(&f.state.conn(), channel, "停用的渠道", &m.base, false, false).unwrap();

    let _ = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": []})).await;
    assert!(f.request_rows().is_empty());
    assert!(f.attempts().is_empty());
}

/// 面板「测一下」：真发请求，通与不通都要有真凭据。
#[tokio::test]
async fn the_panel_probe_really_calls_the_channel() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel(
        "mock",
        &m.base,
        ChannelSpec {
            secret: Some("sk-mock".into()),
            models: vec![("auto".into(), Some("real-model".into()))],
            ..Default::default()
        },
    );
    let conn = f.state.conn();
    let token = tokengo_server::db::sessions::create_session(&conn, None).unwrap();
    drop(conn);

    let (s, v) = get_via_post(&f, &format!("/api/channels/{}/probe", channel.0), &token).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["model_sent"], "real-model");
    assert_eq!(m.count(), 1, "绿点必须是真的探过");
    assert!(f.request_rows().is_empty(), "面板探活不许写记账");

    // 连不上的渠道回 ok=false + 原因，不许假装通
    let dead = f.add_channel(
        "死渠道",
        "http://127.0.0.1:59992/v1",
        ChannelSpec {
            models: vec![("auto".into(), Some("m".into()))],
            ..Default::default()
        },
    );
    let (s, v) = get_via_post(&f, &format!("/api/channels/{}/probe", dead.0), &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], false);
    assert!(v["note"].as_str().unwrap().contains("连不上"), "{v}");
}

async fn get_via_post(f: &Fixture, uri: &str, session: &str) -> (StatusCode, Value) {
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {session}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app(f.state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// 面板会话 + JSON 正文的 POST（管理面写操作用）。
async fn post_with_token(f: &Fixture, uri: &str, session: &str, body: Value) -> (StatusCode, Value) {
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {session}"))
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = app(f.state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// 面板接口的 PUT 调用（改模型组类型用）。
async fn put_with_token(f: &Fixture, uri: &str, session: &str, body: Value) -> (StatusCode, Value) {
    let req = axum::http::Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {session}"))
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = app(f.state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// 面板会话的 GET（管理面读操作用）。
async fn get_with_token(f: &Fixture, uri: &str, session: &str) -> (StatusCode, Value) {
    let req = axum::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {session}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app(f.state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

// ───────────────────────── 拉上游模型清单 ─────────────────────────

/// 面板「拉取上游模型清单」：真打这条渠道的 `GET /v1/models`，用它自己的 key 和自定义头。
#[tokio::test]
async fn the_panel_can_pull_the_upstream_model_list() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel(
        "mock",
        &m.base,
        ChannelSpec {
            secret: Some("sk-mock".into()),
            headers: vec![("x-opencode-client".into(), "cli".into())],
            ..Default::default()
        },
    );
    let conn = f.state.conn();
    let token = tokengo_server::db::sessions::create_session(&conn, None).unwrap();
    drop(conn);

    let (s, v) = get_via_post(&f, &format!("/api/channels/{}/models", channel.0), &token).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    // 重复的名字只留一个，顺序按上游给的
    assert_eq!(v["models"], json!(["mock-chat-a", "mock-embed-b"]), "{v}");
    assert_eq!(v["count"], 2);
    assert!(v["url"].as_str().unwrap().ends_with("/v1/models"), "{v}");

    // 真的发出去了，而且带的是**这条渠道自己的** key 和自定义头
    let seen = m.seen();
    assert_eq!(seen.len(), 1, "清单必须真去上游拉，不能是本地编的");
    assert_eq!(seen[0].path, "/v1/models");
    assert_eq!(seen[0].auth.as_deref(), Some("Bearer sk-mock"));
    assert_eq!(seen[0].custom, vec![("x-opencode-client".to_string(), "cli".to_string())]);

    // 和探活一个纪律：这不是用户请求，不许写记账
    assert!(f.request_rows().is_empty(), "拉清单不许写记账");
    assert!(f.attempts().is_empty(), "拉清单不许写尝试");
}

#[tokio::test]
async fn pulling_the_model_list_reports_a_real_failure() {
    let f = fixture();
    f.with_key();
    let conn = f.state.conn();
    let token = tokengo_server::db::sessions::create_session(&conn, None).unwrap();
    drop(conn);

    // 连不上：说清是哪个 URL，不许给一份编出来的清单
    let dead = f.add_channel("死渠道", "http://127.0.0.1:59993/v1", ChannelSpec::default());
    let (s, v) = get_via_post(&f, &format!("/api/channels/{}/models", dead.0), &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["models"], json!([]));
    assert!(v["note"].as_str().unwrap().contains("连不上"), "{v}");
    assert!(v["note"].as_str().unwrap().contains("59993"), "{v}");

    // 上游没做这个端点（404）：把状态码报出来，并附上正文开头
    let m = start_mock(Reply::Ok).await;
    m.inner.replies.lock().unwrap().push_back(Reply::Status(404, "404 page not found"));
    let channel = f.add_channel("没有清单的渠道", &m.base, ChannelSpec::default());
    let (s, v) = get_via_post(&f, &format!("/api/channels/{}/models", channel.0), &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["http_status"], 404);
    assert!(v["note"].as_str().unwrap().contains("不支持"), "{v}");

    // 200 但回的不是模型清单（常见：回的是一段 HTML 或错误 JSON）→ 说"没认出"，不编
    m.inner
        .replies
        .lock()
        .unwrap()
        .push_back(Reply::Status(200, r#"{"detail":"Not Found"}"#));
    let (s, v) = get_via_post(&f, &format!("/api/channels/{}/models", channel.0), &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["http_status"], 200);
    assert!(v["note"].as_str().unwrap().contains("没认出"), "{v}");
    assert!(v["note"].as_str().unwrap().contains("Not Found"), "{v}");

    // 200 且是清单形状但空的 → 那是"上游自己没报模型"，别说成"没认出"
    m.inner
        .replies
        .lock()
        .unwrap()
        .push_back(Reply::Status(200, r#"{"object":"list","data":[]}"#));
    let (s, v) = get_via_post(&f, &format!("/api/channels/{}/models", channel.0), &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["count"], 0);
    assert!(v["note"].as_str().unwrap().contains("一个模型名都没有"), "{v}");
    assert!(
        !v["note"].as_str().unwrap().contains("没认出"),
        "空清单和格式没认出是两回事：{v}"
    );

    assert!(f.request_rows().is_empty());
}

/// 渠道不存在时是 404，不能悄悄去打个空 URL。
#[tokio::test]
async fn pulling_models_for_an_unknown_channel_is_404() {
    let f = fixture();
    f.with_key();
    let conn = f.state.conn();
    let token = tokengo_server::db::sessions::create_session(&conn, None).unwrap();
    drop(conn);

    let (s, _) = get_via_post(&f, "/api/channels/9999/models", &token).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // 没有面板会话同样不行：管理面全部要登录
    let (s, _) = get_via_post(&f, "/api/channels/9999/models", "not-a-session").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

// ───────────────────── 视觉能力标记（阶段 4.1） ─────────────────────

/// 假上游自己判断"这次请求带没带图"。**故意不复用网关的实现**（`request_has_image`）：
/// 两边共用一份判断的话，一起错也测不出来。
fn body_has_image(v: &Value) -> bool {
    v.get("messages")
        .and_then(|m| m.as_array())
        .map(|ms| {
            ms.iter().any(|m| {
                m.get("content")
                    .and_then(|c| c.as_array())
                    .map(|ps| {
                        ps.iter()
                            .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("image_url"))
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// 一条带图的对话请求（形状就是客户端真会发的那个）。
fn image_chat_body(model: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": "这张图左上角那格是什么颜色？"},
            {"type": "image_url", "image_url": {
                "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAIklEQVR42mP4z8CAFTH8JxGNaqBIAy6Z//8ZsKJRDTTRAACmXD7QsJ/a+gAAAABJRU5ErkJggg=="
            }}
        ]}]
    })
}

/// 核心规矩：**带图的请求只走标了「能看图」的渠道**。
///
/// 两条渠道都配了同一个模型名，只有第二条标了能看图：
/// 图必须落到第二条上，而且**图得真发出去**（不是被网关剥掉再转发）。
#[tokio::test]
async fn an_image_request_only_goes_to_a_channel_marked_as_seeing() {
    let f = fixture();
    f.with_key();
    let text_only = start_mock(Reply::Ok).await;
    let can_see = start_mock(Reply::Ok).await;

    let a = f.add_channel(
        "纯文本上游",
        &text_only.base,
        ChannelSpec {
            secret: Some("sk-a".into()),
            models: vec![("auto".into(), Some("m".into()))],
            vision: vec![],
            ..Default::default()
        },
    );
    let b = f.add_channel(
        "能看图的上游",
        &can_see.base,
        ChannelSpec {
            secret: Some("sk-b".into()),
            models: vec![("auto".into(), Some("m".into()))],
            vision: vec![],
            ..Default::default()
        }
        .with_vision("auto"),
    );
    f.set_group("auto", &[a, b]);

    let (s, body, _) = post(&f, CHAT, Some(KEY), image_chat_body("auto")).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let seen = can_see.seen();
    assert_eq!(seen.len(), 1, "图应该落到标了能看图的那条渠道上：{seen:?}");
    assert_eq!(seen[0].model, "m", "发出去的是这条渠道认的模型名");
    assert!(seen[0].has_image, "图必须真发到上游，不能被网关吞掉");
    assert_eq!(
        text_only.count(),
        0,
        "没标能看图的渠道一次都不该被碰（否则上游会回一个含糊的 400）"
    );

    // 记账：成功一次，渠道是第二条
    let rows = f.request_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "ok");
    assert_eq!(rows[0].1, Some(b.0), "账上记的必须是真干活的渠道");
}

/// 不带图的普通请求**不受视觉标记影响**：还是按优先级走第一条。
#[tokio::test]
async fn a_text_request_ignores_the_vision_marker() {
    let f = fixture();
    f.with_key();
    let text_only = start_mock(Reply::Ok).await;
    let can_see = start_mock(Reply::Ok).await;

    let a = f.add_channel(
        "纯文本上游",
        &text_only.base,
        ChannelSpec {
            secret: Some("sk-a".into()),
            models: vec![("auto".into(), Some("m".into()))],
            vision: vec![],
            ..Default::default()
        },
    );
    let b = f.add_channel(
        "能看图的上游",
        &can_see.base,
        ChannelSpec {
            secret: Some("sk-b".into()),
            models: vec![("auto".into(), Some("m".into()))],
            vision: vec![],
            ..Default::default()
        }
        .with_vision("auto"),
    );
    f.set_group("auto", &[a, b]);

    let (s, _, _) = post(&f, CHAT, Some(KEY), json!({"model": "auto", "messages": [{"role":"user","content":"你好"}]})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(text_only.count(), 1, "普通请求还是走首选渠道");
    assert_eq!(can_see.count(), 0);
}

/// 带图请求**不许把整组的「当前渠道」改掉**。
///
/// 这是一个真踩过的坑：一开始把"筛过能看图的那几条渠道"当成整组的候选名单喂给
/// 路由状态机，结果一条带图请求命中那条能看图的渠道之后，状态机就把**整组**的当前渠道
/// 改成了它 —— 后面十分钟的纯文字请求全被一起改道。带图请求只该在能看图的小圈子里
/// 挑渠道，挑完之后纯文字请求还是走原来的首选。
#[tokio::test]
async fn an_image_request_does_not_reroute_the_plain_traffic() {
    let f = fixture();
    f.with_key();
    let text_only = start_mock(Reply::Ok).await;
    let can_see = start_mock(Reply::SawRed).await;

    let a = f.add_channel(
        "纯文本上游（首选）",
        &text_only.base,
        ChannelSpec {
            secret: Some("sk-a".into()),
            models: vec![("auto".into(), Some("m".into()))],
            vision: vec![],
            ..Default::default()
        },
    );
    let b = f.add_channel(
        "能看图的上游（备选）",
        &can_see.base,
        ChannelSpec {
            secret: Some("sk-b".into()),
            models: vec![("auto".into(), Some("m".into()))],
            vision: vec![],
            ..Default::default()
        }
        .with_vision("auto"),
    );
    f.set_group("auto", &[a, b]);

    let plain = json!({"model": "auto", "messages": [{"role": "user", "content": "你好"}]});
    let (s, _, _) = post(&f, CHAT, Some(KEY), plain.clone()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((text_only.count(), can_see.count()), (1, 0), "纯文字：走首选");

    let (s, body, _) = post(&f, CHAT, Some(KEY), image_chat_body("auto")).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        (text_only.count(), can_see.count()),
        (1, 1),
        "带图：只有能看图的那条渠道该收到图"
    );

    // ← 关键的一半：带图请求发完之后，纯文字请求的走向必须没变
    let (s, _, _) = post(&f, CHAT, Some(KEY), plain).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (text_only.count(), can_see.count()),
        (2, 1),
        "带图请求把整组的当前渠道改掉了：纯文字请求被带偏到能看图的那条渠道上"
    );

    // 顺带看住记账（第 1 行纯文字、第 2 行带图、第 3 行纯文字）
    let rows = f.request_rows();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[1].1, Some(b.0), "带图请求该记在能看图的那条渠道上");
    assert_eq!(rows[2].1, Some(a.0), "带图之后那发纯文字还是记在首选渠道上");
}

/// 一条能看图的渠道都没有时：**当场 422 说清楚**，不发上游、不把图悄悄丢掉。
#[tokio::test]
async fn an_image_request_without_a_seeing_channel_is_refused_up_front() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let channel = f.add_channel(
        "纯文本上游",
        &m.base,
        ChannelSpec {
            secret: Some("sk-a".into()),
            models: vec![("auto".into(), Some("m".into()))],
            vision: vec![],
            ..Default::default()
        },
    );
    f.set_group("auto", &[channel]);

    let (s, body, _) = post(&f, CHAT, Some(KEY), image_chat_body("auto")).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{}", String::from_utf8_lossy(&body));
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "no_vision_model", "{v}");
    assert_eq!(v["error"]["type"], "invalid_request_error", "{v}");
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("能看图"), "要说清怎么解决：{msg}");
    assert!(msg.contains("测视觉"), "要指出去哪儿实测：{msg}");

    assert_eq!(m.count(), 0, "一条渠道都不该被碰");
    assert!(f.attempts().is_empty(), "没发过上游就没有尝试记录");

    // 记账要看得见这次被拦下的请求，且算客户端错（不是渠道的毛病）
    let rows = f.request_rows();
    assert_eq!(rows.len(), 1, "被拦下的请求也要落一条账：{rows:?}");
    assert_eq!(rows[0].0, "client_error");
    assert!(rows[0].5.as_deref().unwrap_or("").contains("能看图"));
}

/// `/v1/models` 要如实说每个模型名能不能吃图 —— 客户端据此决定发不发图。
#[tokio::test]
async fn the_model_list_says_which_names_can_see_images() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let text = f.add_channel(
        "纯文本",
        &m.base,
        ChannelSpec {
            secret: Some("sk-a".into()),
            models: vec![("auto".into(), Some("m".into())), ("BAAI/bge-m3".into(), None)],
            vision: vec![],
            ..Default::default()
        },
    );
    let see = f.add_channel(
        "能看图",
        &m.base,
        ChannelSpec {
            secret: Some("sk-b".into()),
            models: vec![("auto".into(), Some("m".into()))],
            vision: vec![],
            ..Default::default()
        }
        .with_vision("auto"),
    );
    f.set_group("auto", &[text, see]);
    f.set_group("BAAI/bge-m3", &[text]);

    let (s, v) = get(&f, "/v1/models", Some(KEY)).await;
    assert_eq!(s, StatusCode::OK);
    let data = v["data"].as_array().unwrap();
    let auto = data.iter().find(|x| x["id"] == "auto").unwrap();
    let vecm = data.iter().find(|x| x["id"] == "BAAI/bge-m3").unwrap();
    assert_eq!(auto["vision"], true, "{v}");
    assert_eq!(vecm["vision"], false, "{v}");
}

/// 面板要能标「能看图」，也要能读回来（列表 + 每条渠道的个数）。
#[tokio::test]
async fn the_panel_can_mark_a_model_as_seeing_and_reads_it_back() {
    let f = fixture();
    f.with_key();
    let conn = f.state.conn();
    let token = tokengo_server::db::sessions::create_session(&conn, None).unwrap();
    drop(conn);

    let (s, v) = post_with_token(
        &f,
        "/api/channels",
        &token,
        json!({
            "name": "能看图的渠道",
            "base_url": "http://127.0.0.1:1/v1",
            "enabled": true,
            "key": "sk-x",
            "headers": [],
            "models": [
                {"model": "auto", "upstream": "glm-4v-flash", "vision": true},
                {"model": "BAAI/bge-m3", "upstream": null, "vision": false}
            ]
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["vision_count"], 1, "这条渠道上有一个能看图的模型名：{v}");
    let auto = v["models"].as_array().unwrap().iter().find(|m| m["model"] == "auto").unwrap();
    assert_eq!(auto["vision"], true, "{v}");
    let vecm = v["models"].as_array().unwrap().iter().find(|m| m["model"] == "BAAI/bge-m3").unwrap();
    assert_eq!(vecm["vision"], false, "{v}");

    // 重新读一遍（不是拿着刚写进去的回显糊弄）
    let (s, v) = get_with_token(&f, "/api/channels", &token).await;
    assert_eq!(s, StatusCode::OK);
    let channel = v["channels"].as_array().unwrap().iter().find(|l| l["id"] == 1).unwrap();
    assert_eq!(channel["vision_count"], 1, "{v}");

    // 模型组列表要带 can_see：这条渠道在这一组里是能看图的
    f.set_group("auto", &[ChannelId(1)]);
    let (s, v) = get_with_token(&f, "/api/groups", &token).await;
    assert_eq!(s, StatusCode::OK);
    let g = v["groups"].as_array().unwrap().iter().find(|g| g["name"] == "auto").unwrap();
    assert_eq!(g["can_see"], true, "{v}");
}

/// 「测视觉」：真发一张带图的请求，看上游答的是什么。
///
/// - 答"红" → 通了，而且**顺手把这行标上**（面板点一下就该有结果，不该再让人回去勾一遍）；
/// - 上游回 400（不收图）→ 明说不支持；
/// - 两种情况都**不写记账、不动路由状态**（跟探活、拉清单同纪律）。
#[tokio::test]
async fn vision_test_says_what_the_model_actually_answered() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::SawRed).await;
    let channel = f.add_channel(
        "某条渠道",
        &m.base,
        ChannelSpec {
            secret: Some("sk-a".into()),
            models: vec![("auto".into(), Some("m".into()))],
            vision: vec![],
            ..Default::default()
        },
    );
    f.set_group("auto", &[channel]);
    let token = {
        let conn = f.state.conn();
        tokengo_server::db::sessions::create_session(&conn, None).unwrap()
    };

    let (s, v) = post_with_token(
        &f,
        &format!("/api/channels/{}/vision-test", channel.0),
        &token,
        json!({"model": "auto", "upstream": "m"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["reply"], "红", "{v}");
    assert_eq!(v["applied"], true, "测通了要顺手标上：{v}");
    assert!(v["note"].as_str().unwrap().contains("真看见了"), "{v}");

    // 上游收到的必须是真带图、且用这条渠道自己的 key
    let seen = m.seen();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].has_image, "「测视觉」要真发图");
    assert_eq!(seen[0].auth.as_deref(), Some("Bearer sk-a"));

    // 标上了，而且读得回来
    let (_, v) = get_with_token(&f, "/api/channels", &token).await;
    let l = v["channels"].as_array().unwrap().iter().find(|l| l["id"] == channel.0).unwrap();
    assert_eq!(l["vision_count"], 1, "{v}");

    // 一次人工测试不许污染统计、不许动路由状态
    assert!(f.request_rows().is_empty(), "「测视觉」不该写记账");
    assert!(f.attempts().is_empty());
    assert_eq!(f.routing("auto").switch_count, 0);

    // 反证：上游不收图（400）→ 不标绿，也不给一句含糊的"通了"
    m.inner
        .replies
        .lock()
        .unwrap()
        .push_back(Reply::Status(400, r#"{"error":{"message":"This model does not support image inputs"}}"#));
    let (s, v) = post_with_token(
        &f,
        &format!("/api/channels/{}/vision-test", channel.0),
        &token,
        json!({"model": "auto", "upstream": "m"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["http_status"], 400, "{v}");
    assert!(v["note"].as_str().unwrap().contains("不支持图像输入"), "{v}");
    assert!(f.request_rows().is_empty());

    // 渠道不存在是 404，没有面板会话是 401
    let (s, _) = post_with_token(&f, "/api/channels/9999/vision-test", &token, json!({"model": "auto"})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = post_with_token(&f, &format!("/api/channels/{}/vision-test", channel.0), "bad", json!({"model": "auto"})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

// ───────── 渠道级请求/响应适配：把两条 python 中转（3157 / 3158）的活接过来 ─────────

/// 「要丢的字段」：**只在声明了它的那条渠道的那个模型名上**丢，别的渠道一个字节都不许动。
///
/// 依据是现网的账：opencode 的 `beam-free` 不认 `reasoning_effort`（Hermes 默认发
/// high），4 天里 28 次请求就这么整条失败 —— 现在只能靠外挂 python 中转替它剥。
#[tokio::test]
async fn an_unwanted_field_is_dropped_only_where_the_channel_declares_it() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;

    // 甲渠道：给 beam-free 这个模型名声明「要丢 reasoning_effort」
    let a = f.add_channel(
        "声明了要丢字段的渠道",
        &m.base,
        ChannelSpec {
            models: vec![("beam-free".into(), None)],
            ..Default::default()
        }
        .with_drop_fields("beam-free", &["reasoning_effort"]),
    );
    f.set_group("beam-free", &[a]);
    // 乙渠道：同一个上游，一个字都没声明
    let b = f.add_channel(
        "没声明任何字段的渠道",
        &m.base,
        ChannelSpec {
            models: vec![("deepseek-v4.1-flash".into(), None)],
            ..Default::default()
        },
    );
    f.set_group("deepseek-v4.1-flash", &[b]);

    let body = json!({
        "model": "beam-free",
        "reasoning_effort": "high",
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "你好"}]
    });
    let (s, _, _) = post(&f, CHAT, Some(KEY), body.clone()).await;
    assert_eq!(s, StatusCode::OK);
    let sent = m.seen().last().cloned().unwrap();
    assert!(
        sent.body.get("reasoning_effort").is_none(),
        "声明了要丢的字段必须真丢掉：{:?}",
        sent.body
    );
    assert_eq!(sent.body["max_tokens"], 64, "别的字段一个都不许动：{:?}", sent.body);
    assert_eq!(sent.model, "beam-free");

    // 反证：没声明的渠道照原样发（这是「按渠道 + 模型名」的能力，不是全局开关）
    let mut other = body.clone();
    other["model"] = json!("deepseek-v4.1-flash");
    let (s, _, _) = post(&f, CHAT, Some(KEY), other).await;
    assert_eq!(s, StatusCode::OK);
    let sent = m.seen().last().cloned().unwrap();
    assert_eq!(
        sent.body["reasoning_effort"], "high",
        "没声明的渠道不许替用户删字段：{:?}",
        sent.body
    );
}

/// `{{session_id}}`：同一段对话的每次请求带同一个值，换一段对话必须换
/// —— 上游靠它认「这还是同一段对话」（命中提示缓存），现网那条 python 中转就是干这个的。
#[tokio::test]
async fn the_session_header_is_stable_within_one_conversation_and_new_for_another() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let a = f.add_channel(
        "要会话头的渠道",
        &m.base,
        ChannelSpec {
            models: vec![("auto".into(), None)],
            headers: vec![
                ("x-opencode-session".into(), "{{session_id}}".into()),
                ("x-opencode-request".into(), "{{uuid}}".into()),
            ],
            ..Default::default()
        },
    );
    f.set_group("auto", &[a]);

    let turn1 = json!({"model": "auto", "messages": [{"role": "user", "content": "帮我看下这段代码"}]});
    let (s, _, _) = post(&f, CHAT, Some(KEY), turn1).await;
    assert_eq!(s, StatusCode::OK);
    // 同一段对话的下一轮：第一条 user 消息没变，后面接了更多轮
    let (s, _, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [
            {"role": "user", "content": "帮我看下这段代码"},
            {"role": "assistant", "content": "好的"},
            {"role": "user", "content": "再改一下"}
        ]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // 换一段对话
    let (s, _, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "换个问题"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let seen = m.seen();
    assert_eq!(seen.len(), 3);
    let pick = |i: usize, name: &str| {
        seen[i]
            .custom
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    assert_eq!(pick(0, "x-opencode-session"), pick(1, "x-opencode-session"), "同段对话必须同一个会话 ID");
    assert_ne!(pick(0, "x-opencode-session"), pick(2, "x-opencode-session"), "换一段对话要换 ID");
    assert_eq!(pick(0, "x-opencode-session").len(), 32);
    assert_ne!(
        pick(0, "x-opencode-request"),
        pick(1, "x-opencode-request"),
        "逐请求的那个头还是要每次都不一样"
    );
}

/// 「响应解包」：勾了的渠道剥掉外壳（cline.bot 形状），没勾的一律原样透传 —— 网关不替人猜。
#[tokio::test]
async fn the_envelope_is_unwrapped_only_when_the_channel_says_so() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Enveloped).await;
    let a = f.add_channel(
        "套壳上游（勾了解包）",
        &m.base,
        ChannelSpec {
            models: vec![("auto".into(), None)],
            unwrap_envelope: true,
            ..Default::default()
        },
    );
    f.set_group("auto", &[a]);
    let b = f.add_channel(
        "同一个套壳上游（没勾解包）",
        &m.base,
        ChannelSpec {
            models: vec![("raw".into(), None)],
            ..Default::default()
        },
    );
    f.set_group("raw", &[b]);

    // 勾了的：客户端拿到标准形状，token 账也照样记
    let (s, body, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "你好"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["choices"][0]["message"]["content"], "壳里的你好", "勾了解包就得剥掉外层：{v}");
    assert!(v.get("data").is_none(), "外壳不许还在：{v}");
    assert_eq!(v["success"], true, "外壳上的 success 要留在里层：{v}");
    let rows = f.request_rows();
    assert_eq!(rows[0].2, 31, "剥壳之后照样要记上 token 账：{rows:?}");
    assert_eq!(rows[0].3, 4, "{rows:?}");

    // 反证：没勾的照原样透传
    let (s, body, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "raw", "messages": [{"role": "user", "content": "你好"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert!(v.get("choices").is_none(), "没勾解包的渠道不许动响应：{v}");
    assert_eq!(v["data"]["choices"][0]["message"]["content"], "壳里的你好", "{v}");
}

// ───────── 模型按类型分开（对话 / 向量 / 重排）+ 顺手补的两条可靠性 ─────────

/// 类型是**实测学来的**：真有向量请求进来一次，这组的类型就记成向量，
/// 面板与 `/v1/models` 都读得到。人手动定过之后，实测不许再改。
#[tokio::test]
async fn the_model_kind_is_learned_from_real_traffic_and_then_frozen_by_a_human_choice() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let a = f.add_channel(
        "载荷渠道",
        &m.base,
        ChannelSpec {
            models: vec![("BAAI/bge-m3".into(), None), ("auto".into(), None)],
            ..Default::default()
        },
    );
    f.set_group("BAAI/bge-m3", &[a]);
    f.set_group("auto", &[a]);
    let token = {
        let conn = f.state.conn();
        tokengo_server::db::sessions::create_session(&conn, None).unwrap()
    };

    // 一开始：两组都「还没定」，谁也不许按名字猜
    let (_, v) = get_with_token(&f, "/api/groups", &token).await;
    for name in ["BAAI/bge-m3", "auto"] {
        let g = v["groups"].as_array().unwrap().iter().find(|g| g["name"] == name).unwrap();
        assert_eq!(g["kind"], "", "{name} 不该凭空有个类型：{g}");
        assert_eq!(g["kind_source"], "");
    }

    // 真打一发向量请求 → 只有那一组被记成向量
    let (s, _, _) = post(
        &f,
        "/v1/embeddings",
        Some(KEY),
        json!({"model": "BAAI/bge-m3", "input": ["爱吃鱼"]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = get_with_token(&f, "/api/groups", &token).await;
    let vec_g = v["groups"].as_array().unwrap().iter().find(|g| g["name"] == "BAAI/bge-m3").unwrap();
    let chat_g = v["groups"].as_array().unwrap().iter().find(|g| g["name"] == "auto").unwrap();
    assert_eq!(vec_g["kind"], "embeddings", "实测验到的类型要落库：{vec_g}");
    assert_eq!(vec_g["kind_source"], "auto");
    assert_eq!(vec_g["kind_label"], "向量");
    assert_eq!(chat_g["kind"], "", "没被碰过的组不许跟着变：{chat_g}");

    // /v1/models 也要如实说类型（客户端据此挑端点）
    let (_, v) = get(&f, "/v1/models", Some(KEY)).await;
    let m_ = v["data"].as_array().unwrap().iter().find(|x| x["id"] == "BAAI/bge-m3").unwrap();
    assert_eq!(m_["kind"], "embeddings", "{v}");
    assert_eq!(m_["vision"], false, "向量模型不看图：{m_}");

    // 人手动改成「对话」→ 来源变 manual，再来向量流量也不许把它改回去
    let (s, v) = put_with_token(
        &f,
        "/api/groups/BAAI%2Fbge-m3/kind",
        &token,
        json!({"kind": "chat"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["kind"], "chat");
    assert_eq!(v["kind_source"], "manual");
    let (s, _, _) = post(
        &f,
        "/v1/embeddings",
        Some(KEY),
        json!({"model": "BAAI/bge-m3", "input": ["再来一发"]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = get_with_token(&f, "/api/groups", &token).await;
    let g = v["groups"].as_array().unwrap().iter().find(|g| g["name"] == "BAAI/bge-m3").unwrap();
    assert_eq!(g["kind"], "chat", "手动定过的类型不许被实测改掉：{g}");
    assert_eq!(g["kind_source"], "manual");

    // 交还给实测 → 清空
    let (s, v) = put_with_token(&f, "/api/groups/auto/kind", &token, json!({"kind": "auto"})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["kind"], "");

    // 乱写的类型要 400，不许悄悄当成交给实测
    let (s, _) = put_with_token(&f, "/api/groups/auto/kind", &token, json!({"kind": "随便"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // 不存在的组是 404（不是 200 假成功）
    let (s, _) = put_with_token(&f, "/api/groups/没有这个组/kind", &token, json!({"kind": "chat"})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// 空回复（200 但一句话没说）当**渠道的毛病**处理：立刻重试，不再把空泡泡丢给客户端。
#[tokio::test]
async fn an_empty_reply_is_retried_instead_of_being_passed_through() {
    let f = fixture();
    f.with_key();
    // 第一发回空，第二发正常 → 客户端拿到的是正常回复
    // （假上游的队列「先排的先回、排空了才用默认值」：默认给正常的，把空的那发排在最前）
    let m = start_mock(Reply::Ok).await;
    let a = f.add_channel("偶尔发呆的渠道", &m.base, ChannelSpec::default());
    f.set_group("auto", &[a]);
    m.inner.replies.lock().unwrap().push_back(Reply::Empty);
    let (s, body, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "在吗"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "空回复之后重试成功，客户端不该看到错误");
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["choices"][0]["message"]["content"], "你好", "拿到的必须是那发正常的：{v}");

    let att = f.attempts();
    assert_eq!(att.len(), 2, "第一次空回复、第二次正常：{att:?}");
    assert_eq!(att[0].1, "empty_reply", "空回复那次要记成 empty_reply：{att:?}");
    assert_eq!(att[1].1, "success");

    // 反证：一直空 → 4 次尝试都记 empty_reply，请求记失败并**说清原因**
    let f2 = fixture();
    f2.with_key();
    let m2 = start_mock(Reply::Empty).await;
    let b = f2.add_channel("一直发呆的渠道", &m2.base, ChannelSpec::default());
    f2.set_group("auto", &[b]);
    let (s, body, _) = post(
        &f2,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "在吗"}]}),
    )
    .await;
    assert_ne!(s, StatusCode::OK, "整条链路都只回空，绝不能当成功");
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("一句话都没说"),
        "要说清上游一个字都没说，不能给个含糊的 502：{text}"
    );
    let att = f2.attempts();
    assert_eq!(att.len(), 4, "按重试策略走满 4 次：{att:?}");
    assert!(att.iter().all(|a| a.1 == "empty_reply"), "{att:?}");
    let rows = f2.request_rows();
    assert_eq!(rows[0].0, "failed", "{rows:?}");
    assert!(
        rows[0].5.as_deref().unwrap_or("").contains("一句话都没说"),
        "流水里要写清原因：{rows:?}"
    );
}

/// 上游在 `Retry-After` 里说要等很久 → **别在这条渠道上耗**，剩下的重试直接放弃、换下一条。
#[tokio::test]
async fn a_long_retry_after_skips_the_remaining_attempts_on_that_channel() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::RateLimited(300)).await;
    let a = f.add_channel("配额没了的渠道", &m.base, ChannelSpec::default());
    let good = start_mock(Reply::Ok).await;
    let b = f.add_channel("好好的渠道", &good.base, ChannelSpec::default());
    f.set_group("auto", &[a, b]);

    let (s, body, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "在吗"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "第一条要等 300 秒 → 直接换下一条，客户端照样拿到回复");
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["choices"][0]["message"]["content"], "你好");
    let att = f.attempts();
    assert_eq!(att.len(), 2, "第一条只准试一次就放弃：{att:?}");
    assert_eq!(att[0].0, a.0);
    assert_eq!(att[0].2, Some(429));
    assert_eq!(att[1].0, b.0);
    assert_eq!(m.count(), 1, "要求等 300 秒的渠道，一次都不许多试: {}", m.count());

    // 反证：上游只要求等 0 秒 → 不算「很久」，照旧重试满 4 次
    let f2 = fixture();
    f2.with_key();
    let m2 = start_mock(Reply::RateLimited(0)).await;
    let c = f2.add_channel("很快就好的渠道", &m2.base, ChannelSpec::default());
    f2.set_group("auto", &[c]);
    let (s, _, _) = post(
        &f2,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "在吗"}]}),
    )
    .await;
    assert_ne!(s, StatusCode::OK);
    assert_eq!(f2.attempts().len(), 4, "等 0 秒的 429 该照旧重试满：{:?}", f2.attempts());
}

// ───────── 面板上那个「一键启用/停用」开关 ─────────

/// 点一下开关就能停用一组：客户端立刻打不通（404），`/v1/models` 里也没了；
/// 再点一下又能用。**只改这一列** —— 渠道顺序与类型都不许被顺手改掉。
#[tokio::test]
async fn the_panel_switch_disables_and_enables_a_group_in_one_call() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let a = f.add_channel(
        "开关渠道",
        &m.base,
        ChannelSpec { models: vec![("auto".into(), None)], ..Default::default() },
    );
    f.set_group("auto", &[a]);
    // 顺手手动定个类型：开关不该把这列改掉
    let token = {
        let conn = f.state.conn();
        tokengo_server::db::sessions::create_session(&conn, None).unwrap()
    };
    let (s, _) = put_with_token(&f, "/api/groups/auto/kind", &token, json!({"kind": "chat"})).await;
    assert_eq!(s, StatusCode::OK);

    // 启用着：能打，清单里有
    let (s, _, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "在吗"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = get(&f, "/v1/models", Some(KEY)).await;
    assert!(v["data"].as_array().unwrap().iter().any(|m| m["id"] == "auto"), "{v}");

    // 点开关 → 停用
    let before = m.count();
    let (s, v) = put_with_token(&f, "/api/groups/auto/enabled", &token, json!({"enabled": false})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["enabled"], false);

    // 停用后：客户端拿到 404（不是 500，也不是假装成功），清单里也没了
    let (s, body, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "在吗"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "停用的名字要像不存在一样：{body:?}");
    let (_, v) = get(&f, "/v1/models", Some(KEY)).await;
    assert!(
        !v["data"].as_array().unwrap().iter().any(|m| m["id"] == "auto"),
        "停用的组不该出现在客户端清单里：{v}"
    );

    // 停用期间上游一次都没被打（不是「还发请求只是不回话」）
    assert_eq!(m.count(), before, "停用后不许再往这家上游发请求");

    // 再点一下 → 启用，立刻恢复
    let (s, v) = put_with_token(&f, "/api/groups/auto/enabled", &token, json!({"enabled": true})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, _, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "回来了吗"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "启用后要立刻能打");

    // 只动了 enabled 这一列：类型还是刚才手动定的
    let (_, v) = get_with_token(&f, "/api/groups", &token).await;
    let g = v["groups"].as_array().unwrap().iter().find(|g| g["name"] == "auto").unwrap();
    assert_eq!(g["enabled"], true);
    assert_eq!(g["kind"], "chat", "开关不许把类型改掉：{g}");
    assert_eq!(g["kind_source"], "manual");

    // 不存在的组 → 404（不许假装成功）
    let (s, _) = put_with_token(&f, "/api/groups/没有这个组/enabled", &token, json!({"enabled": false})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// 渠道卡片上那个开关：停用一条渠道之后，流量只走还启用的那条；
/// 两条都停 → 客户端拿不到 200（而不是悄悄放行）。
#[tokio::test]
async fn the_panel_switch_disables_a_channel_and_traffic_moves_off_it() {
    let f = fixture();
    f.with_key();
    let first = start_mock(Reply::Ok).await;
    let second = start_mock(Reply::Ok).await;
    let a = f.add_channel(
        "一号",
        &first.base,
        ChannelSpec { models: vec![("auto".into(), None)], ..Default::default() },
    );
    let b = f.add_channel(
        "二号",
        &second.base,
        ChannelSpec { models: vec![("auto".into(), None)], ..Default::default() },
    );
    f.set_group("auto", &[a, b]);
    let token = {
        let conn = f.state.conn();
        tokengo_server::db::sessions::create_session(&conn, None).unwrap()
    };

    // 停用一号 → 只剩二号，账上看得见
    let (s, v) = put_with_token(
        &f,
        &format!("/api/channels/{}/enabled", a.0),
        &token,
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["enabled"], false);
    let (_, v) = get_with_token(&f, "/api/channels", &token).await;
    let c = |id: i64| {
        v["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(c(a.0)["enabled"], false);
    assert_eq!(c(b.0)["enabled"], true);
    assert_eq!(c(a.0)["base_url"], first.base, "开关不许把别的字段改掉");

    let (s, body, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "走哪条"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body:?}");
    assert_eq!(first.count(), 0, "停用的渠道不该再被叫");
    assert_eq!(second.count(), 1, "活的渠道该顶上");

    // 两条都停 → 不许假装成功
    let (s, _) = put_with_token(
        &f,
        &format!("/api/channels/{}/enabled", b.0),
        &token,
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = put_with_token(
        &f,
        &format!("/api/channels/{}/enabled", b.0),
        &token,
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, body, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "都没了"}]}),
    )
    .await;
    assert_ne!(s, StatusCode::OK, "两条渠道都停用，不许回 200：{body:?}");
    assert_eq!(first.count() + second.count(), 1, "都停用后上游一次都不该再被打");

    // 不存在的渠道 → 404
    let (s, _) = put_with_token(&f, "/api/channels/9999/enabled", &token, json!({"enabled": false})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// 「测视觉」的额度不能压到 8：模型先来半句铺垫就把额度用光，颜色词还没出口，
/// 明明能看图的模型被误判成「看不见」—— 用户报的就是这个假红。
///
/// 这一版：第一发给 64 token，够答；真的被 `finish_reason=length` 截断时，
/// 再给 256 重问一次，并且如实把「重问过」写进回答里。
#[tokio::test]
async fn vision_test_gives_enough_room_and_retries_when_truncated() {
    let f = fixture();
    f.with_key();
    // 额度 < 32 才截断：给 64 就一次答对（回归「别压到 8」）
    let roomy = start_mock(Reply::TruncatesBelow(32)).await;
    let a = f.add_channel(
        "够额度的渠道",
        &roomy.base,
        ChannelSpec {
            secret: Some("sk-a".into()),
            models: vec![("auto".into(), Some("m".into()))],
            ..Default::default()
        },
    );
    let token = {
        let conn = f.state.conn();
        tokengo_server::db::sessions::create_session(&conn, None).unwrap()
    };
    let (s, v) = post_with_token(
        &f,
        &format!("/api/channels/{}/vision-test", a.0),
        &token,
        json!({"model": "auto", "upstream": "m"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true, "给了 64 就该答对：{v}");
    assert_eq!(v["max_tokens"], 64, "{v}");
    assert_eq!(v["retried_with_more_tokens"], false, "一次就成，不必重问：{v}");
    assert_eq!(roomy.count(), 1, "只该打一发");

    // 额度 < 200 才截断：第一发 64 会被截断，重问 256 才答对
    let tight = start_mock(Reply::TruncatesBelow(200)).await;
    let b = f.add_channel(
        "要更多额度的渠道",
        &tight.base,
        ChannelSpec {
            secret: Some("sk-b".into()),
            models: vec![("auto".into(), Some("m".into()))],
            ..Default::default()
        },
    );
    let (s, v) = post_with_token(
        &f,
        &format!("/api/channels/{}/vision-test", b.0),
        &token,
        json!({"model": "auto", "upstream": "m"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true, "截断之后多给额度答对了，要算它能看图：{v}");
    assert_eq!(v["retried_with_more_tokens"], true, "{v}");
    assert_eq!(v["max_tokens"], 256, "重问要给更多：{v}");
    assert_eq!(v["reply"], "红", "{v}");
    assert!(v["note"].as_str().unwrap().contains("真看见了"), "{v}");
    assert_eq!(tight.count(), 2, "截断那一发 + 重问那一发 = 两发");
    // 两发都要真带图
    let seen = tight.seen();
    assert!(seen.iter().all(|x| x.has_image), "重问也要带图");
}

/// 一直截断（额度怎么给都不够）：不许判成「能看图」，而且要说清是**被截断**，
/// 别让用户以为模型看不见。
#[tokio::test]
async fn a_reply_that_stays_truncated_is_not_a_yes_and_says_so() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::TruncatesBelow(99_999)).await;
    let ch = f.add_channel(
        "永远截断的渠道",
        &m.base,
        ChannelSpec {
            secret: Some("sk-a".into()),
            models: vec![("auto".into(), Some("m".into()))],
            ..Default::default()
        },
    );
    let token = {
        let conn = f.state.conn();
        tokengo_server::db::sessions::create_session(&conn, None).unwrap()
    };
    let (s, v) = post_with_token(
        &f,
        &format!("/api/channels/{}/vision-test", ch.0),
        &token,
        json!({"model": "auto", "upstream": "m"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], false, "没答出颜色就不许判成能看图：{v}");
    assert_eq!(v["applied"], false, "更不许把标记落库：{v}");
    let note = v["note"].as_str().unwrap();
    assert!(note.contains("截断"), "要明说是被截断：{note}");
    assert!(note.contains("256"), "要把给了多少额度说出来：{note}");
    assert_eq!(m.count(), 2, "截断 + 重问 = 两发，不多打");
    // 没标上
    let (_, v) = get_with_token(&f, "/api/channels", &token).await;
    let l = v["channels"].as_array().unwrap().iter().find(|l| l["id"] == ch.0).unwrap();
    assert_eq!(l["vision_count"], 0, "{v}");
}

/// 组名里带斜杠（`BAAI/bge-m3` 这种）时，面板必须把它转义了再进 URL ——
/// 不转义就是用户报的「删除模型 → 删除失败 HTTP404」。
///
/// 这条钉两件事：① 服务端认 `%2F` 这种写法（转义之后能匹配上路由）；
/// ② 不转义的写法确实是 404（所以面板那边必须转义，不能手拼）。
#[tokio::test]
async fn a_group_name_with_a_slash_can_be_deleted_when_it_is_escaped() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let a = f.add_channel(
        "渠道",
        &m.base,
        ChannelSpec {
            models: vec![("BAAI/bge-m3".into(), None)],
            ..Default::default()
        },
    );
    f.set_group("BAAI/bge-m3", &[a]);
    let token = {
        let conn = f.state.conn();
        tokengo_server::db::sessions::create_session(&conn, None).unwrap()
    };

    // 不转义：路由匹配不上（这就是那个 404 的来源）
    let req = axum::http::Request::builder()
        .method("DELETE")
        .uri("/api/groups/BAAI/bge-m3")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app(f.state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不转义就该匹配不上");

    // 转义之后：删得掉，而且真从库里没了
    let req = axum::http::Request::builder()
        .method("DELETE")
        .uri("/api/groups/BAAI%2Fbge-m3")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app(f.state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "转义之后要删得掉");
    let (_, v) = get_with_token(&f, "/api/groups", &token).await;
    assert!(
        !v["groups"].as_array().unwrap().iter().any(|g| g["name"] == "BAAI/bge-m3"),
        "删完就不该还在：{v}"
    );

    // 同理，回切检查点那一条也得转义（面板上「立刻试首选」用的就是它）
    f.set_group("BAAI/bge-m3", &[a]);
    let (s, _) = post_with_token(&f, "/api/routing/BAAI%2Fbge-m3/reprobe", &token, json!({})).await;
    assert_eq!(s, StatusCode::OK, "转义的组名也要能回切");
}

/// 删一个已经不在的东西：要老实回 404（以前渠道那条漏了检查，回的是 500「内部错误」；
/// 组那条更糟 —— 假装成功回 200）。面板上「删除失败」的文案就靠这个状态码分辨。
#[tokio::test]
async fn deleting_something_that_is_not_there_says_404_not_500() {
    let f = fixture();
    f.with_key();
    let token = {
        let conn = f.state.conn();
        tokengo_server::db::sessions::create_session(&conn, None).unwrap()
    };

    // 渠道：建了再删（200），删第二下要 404
    let m = start_mock(Reply::Ok).await;
    let ch = f.add_channel("删两次的渠道", &m.base, ChannelSpec::default());
    let req = |uri: &str| {
        axum::http::Request::builder()
            .method("DELETE")
            .uri(uri.to_string())
            .header("authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let resp = app(f.state.clone())
        .oneshot(req(&format!("/api/channels/{}", ch.0)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app(f.state.clone())
        .oneshot(req(&format!("/api/channels/{}", ch.0)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "删第二下要 404，不许 500");

    // 组：建了再删（200），删第二下要 404（不许假装成功）
    f.set_group("auto", &[]);
    let resp = app(f.state.clone()).oneshot(req("/api/groups/auto")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app(f.state.clone()).oneshot(req("/api/groups/auto")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "删第二下要 404");
}

/// 「这条请求到底带没带图」要记进账：用户报「模型识别不了图片」时，先看这一列就能分清
/// 是**客户端根本没把图发上来**，还是网关/上游看不见。
///
/// 三种情形都钉住：纯文字 = 不带图；带图且有人能看 = 带图 + 成功；
/// 带图但一条能看图的渠道都没有 = 带图 + 422（图确实到了网关，只是没人能接）。
#[tokio::test]
async fn the_request_log_says_whether_an_image_came_in() {
    let f = fixture();
    f.with_key();
    let m = start_mock(Reply::Ok).await;
    let plain = f.add_channel(
        "看不见的渠道",
        &m.base,
        ChannelSpec {
            models: vec![("auto".into(), None)],
            vision: vec![],
            ..Default::default()
        },
    );
    f.set_group("auto", &[plain]);
    let token = {
        let conn = f.state.conn();
        tokengo_server::db::sessions::create_session(&conn, None).unwrap()
    };
    let img = json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}});

    // ① 纯文字
    let (s, _, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": "在吗"}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // ② 带图，但这条渠道没标能看图 → 422，账上仍要记「带图」
    let (s, _, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": [
            {"type": "text", "text": "这是什么"},
            img.clone(),
        ]}]}),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "没人能看图要给 422");

    // ③ 把渠道标成能看图 → 同样带图的一发应当成功
    {
        let conn = f.state.conn();
        tokengo_server::db::channels::set_model_vision(&conn, plain, "auto", true).unwrap();
    }
    let (s, _, _) = post(
        &f,
        CHAT,
        Some(KEY),
        json!({"model": "auto", "messages": [{"role": "user", "content": [
            {"type": "text", "text": "这是什么"},
            img.clone(),
        ]}]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "标了能看图就该走起来");

    // 账上读回来：三条请求的 has_image 依次是 否 / 是 / 是
    let (s, v) = get_with_token(&f, "/api/logs?limit=10", &token).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let logs = v["logs"].as_array().unwrap();
    let got: Vec<(String, bool)> = logs
        .iter()
        .rev()
        .map(|r| (r["status"].as_str().unwrap().to_string(), r["has_image"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("ok".to_string(), false),
            ("client_error".to_string(), true),
            ("ok".to_string(), true),
        ],
        "账上要能分清哪条带了图：{v}"
    );
}
