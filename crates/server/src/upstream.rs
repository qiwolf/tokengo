//! 打上游的 HTTP 客户端。全项目只有这一处往外发请求。
//!
//! 三件事必须在这里做对，否则后面全是玄学：
//!
//! 1. **URL 拼接**：面板里填的 base_url 结尾有没有斜杠、有没有多写一段路径，
//!    都得好使（`https://x/v1` + `chat/completions`，以及 `https://x/v1/`）。
//! 2. **请求头**：Authorization 用这条腿自己的 key；自定义请求头里的
//!    `{{uuid}}` / `{{timestamp_ms}}` 逐请求求值（上游 FreeLLMAPI 没有这个能力，
//!    本机要它就是因为 OpenCode 那条路要求每次带新的 session id）。
//! 3. **错误分类**：网络错/超时算腿的毛病；4xx（客户端的错）不算 —— 分类在
//!    `tokengo_shared::routing::classify_status`，这里只负责把两种情形区分开。

use std::time::Duration;

use bytes::Bytes;
use tokengo_shared::leg::{render_header_value, Leg};

/// 一次尝试直接失败（网络层），没有拿到任何 HTTP 响应。算腿的毛病。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegFault {
    pub message: String,
}

impl std::fmt::Display for LegFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// 打上游的客户端。内部连接池复用，进程里只建一个。
#[derive(Clone)]
pub struct Upstream {
    client: reqwest::Client,
}

impl Upstream {
    pub fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("tokengo/", env!("CARGO_PKG_VERSION")))
            .tcp_nodelay(true)
            // 不设全局超时：每次尝试的超时由面板参数决定，逐请求传入
            .pool_idle_timeout(Duration::from_secs(90))
            .build()?;
        Ok(Self { client })
    }

    /// 往一条腿上发一次请求。
    ///
    /// 返回 Ok 只表示"上游回了 HTTP 响应"，**不代表成功** —— 状态码是 200 还是 429
    /// 由调用方按 `classify_status` 判。返回 Err 表示网络层就没通（连不上/超时/TLS），
    /// 这属于腿的毛病。
    pub async fn call(
        &self,
        leg: &Leg,
        secret: Option<&str>,
        path: &str,
        body: Bytes,
        stream: bool,
        timeout: Duration,
        uuid: &str,
        now_ms: i64,
    ) -> Result<reqwest::Response, LegFault> {
        let url = join_url(&leg.base_url, path);
        let mut rb = self
            .client
            .post(&url)
            .timeout(timeout)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body);
        if stream {
            rb = rb.header(reqwest::header::ACCEPT, "text/event-stream");
        }
        for (name, value) in build_headers(leg, secret, uuid, now_ms) {
            rb = rb.header(name, value);
        }
        rb.send().await.map_err(|e| LegFault {
            message: fault_message(&url, &e),
        })
    }

    /// 探活用：只取状态码，不关心正文（正文可能很大）。
    #[allow(clippy::too_many_arguments)]
    pub async fn probe(
        &self,
        leg: &Leg,
        secret: Option<&str>,
        path: &str,
        body: Bytes,
        timeout: Duration,
        uuid: &str,
        now_ms: i64,
    ) -> Result<u16, LegFault> {
        let r = self
            .call(leg, secret, path, body, false, timeout, uuid, now_ms)
            .await?;
        Ok(r.status().as_u16())
    }

    /// 「测视觉」用：要状态码 **和** 正文开头 —— 判断"它到底看没看见图"得看模型
    /// 回的话（回了"红"才是真看见了；回 400 说明它压根不收图片）。
    ///
    /// 正文最多留 4 KB：上游可能吐一整个错误页，这里只要够看懂就行。
    #[allow(clippy::too_many_arguments)]
    pub async fn call_and_read(
        &self,
        leg: &Leg,
        secret: Option<&str>,
        path: &str,
        body: Bytes,
        timeout: Duration,
        uuid: &str,
        now_ms: i64,
    ) -> Result<(u16, String), LegFault> {
        let r = self
            .call(leg, secret, path, body, false, timeout, uuid, now_ms)
            .await?;
        let code = r.status().as_u16();
        let text = r.text().await.unwrap_or_default();
        Ok((code, cap_chars(&text, 4000)))
    }

    /// 拉这条腿支持的模型清单：`GET {base_url}/models`，用这条腿自己的
    /// URL、key 和自定义请求头（头里的模板照旧逐请求求值）。
    ///
    /// **面板专用的只读动作**，和探活一个纪律：不写记账、不动路由状态。
    /// 返回 `(状态码, 正文)` —— 正文交给 `parse_model_ids` 挑名字，
    /// 状态码交给调用方判断"上游是不是根本不支持这个端点"。
    pub async fn fetch_models(
        &self,
        leg: &Leg,
        secret: Option<&str>,
        timeout: Duration,
        uuid: &str,
        now_ms: i64,
    ) -> Result<(u16, String), LegFault> {
        let url = join_url(&leg.base_url, "models");
        let mut rb = self
            .client
            .get(&url)
            .timeout(timeout)
            .header(reqwest::header::ACCEPT, "application/json");
        for (name, value) in build_headers(leg, secret, uuid, now_ms) {
            rb = rb.header(name, value);
        }
        let r = rb.send().await.map_err(|e| LegFault {
            message: fault_message(&url, &e),
        })?;
        let status = r.status().as_u16();
        let text = r.text().await.unwrap_or_default();
        Ok((status, clamp_body(text)))
    }
}

/// 上游模型清单的正文上限。真有上游吐上千个模型，超过就当拉不到 ——
/// 面板本来也塞不下那个量级，硬留着只会把网关内存顶起来。
const MAX_MODELS_BODY: usize = 2 * 1024 * 1024;

fn clamp_body(text: String) -> String {
    if text.len() <= MAX_MODELS_BODY {
        return text;
    }
    let mut cut = MAX_MODELS_BODY;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_string()
}

/// 从上游 `/v1/models` 的响应里挑出模型名。
///
/// 三种**真实见过**的形状都认：
///
/// - OpenAI 标准：`{"data":[{"id":"gpt-x","object":"model"}, …]}`
/// - `{"models":[{"name":"llama3"}, …]}`（Ollama 风格）
/// - 直接一个字符串数组：`["a","b"]`
///
/// **认不出来就回空列表** —— 面板会说"拉回来了但没认出清单"，绝不编造名字。
/// 顺序按上游给的来（不排序），只去重、去空白、限量。
pub fn parse_model_ids(body: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };

    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let s = s.trim();
        if !s.is_empty() && !out.iter().any(|x| x == s) {
            out.push(s.to_string());
        }
    };

    let from_array = |arr: &Vec<serde_json::Value>| {
        let mut ids: Vec<String> = Vec::new();
        for item in arr {
            let name = match item {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(o) => ["id", "name", "model"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(|x| x.as_str()).map(str::to_owned)),
                _ => None,
            };
            if let Some(n) = name {
                let n = n.trim().to_string();
                if !n.is_empty() && !ids.contains(&n) {
                    ids.push(n);
                }
            }
        }
        ids
    };

    match &v {
        serde_json::Value::Array(arr) => {
            for id in from_array(arr) {
                push(&id);
            }
        }
        serde_json::Value::Object(o) => {
            for key in ["data", "models"] {
                if let Some(arr) = o.get(key).and_then(|x| x.as_array()) {
                    for id in from_array(arr) {
                        push(&id);
                    }
                }
            }
        }
        _ => {}
    }

    out.truncate(MAX_MODEL_IDS);
    out
}

/// 面板列表里最多显示这么多模型名。
const MAX_MODEL_IDS: usize = 2000;

/// 这份正文是不是"模型清单那种形状"（哪怕里面一个名字都没有）。
///
/// 用来把两种 200 分开：**上游有清单但空**（它就是不报模型）和
/// **上游回的压根不是清单**（一段 HTML、一个错误 JSON）。前者说"它没报任何模型"，
/// 后者说"没认出"，别混成一句话。
pub fn looks_like_a_model_list(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    match &v {
        serde_json::Value::Array(_) => true,
        serde_json::Value::Object(o) => ["data", "models"]
            .iter()
            .any(|k| o.get(*k).map(|x| x.is_array()).unwrap_or(false)),
        _ => false,
    }
}

/// 网络错分类成人能看懂的话。**不要把 key 打进日志。**
fn fault_message(url: &str, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        format!("超时：{url}")
    } else if e.is_connect() {
        format!("连不上：{url}")
    } else {
        format!("网络错误：{url}（{e}）")
    }
}

/// 拼上游 URL。base_url 结尾的斜杠、多段路径都得能对上。
pub fn join_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

/// 只留正文开头这么多字符（按字符数不算字节数，中文不会被切半个字）。
pub fn cap_chars(s: &str, max: usize) -> String {
    let t = s.trim();
    match t.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &t[..i]),
        None => t.to_string(),
    }
}

/// 这条腿本次请求要带的头（自定义头在前，Authorization 最后 ——
/// 免得用户自定义头里写了个 Authorization 又被覆盖，反过来更好查）。
///
/// 模板逐请求求值：`{{uuid}}` 每次都是新的，就是这个让 OpenCode 那条路能通。
pub fn build_headers(
    leg: &Leg,
    secret: Option<&str>,
    uuid: &str,
    now_ms: i64,
) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(leg.headers.len() + 1);
    for (name, value) in &leg.headers {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case("authorization") && secret.is_none() {
            continue;
        }
        out.push((name.to_string(), render_header_value(value, uuid, now_ms)));
    }
    if let Some(s) = secret.filter(|s| !s.trim().is_empty()) {
        out.push(("authorization".to_string(), format!("Bearer {}", s.trim())));
    }
    out
}

/// 新的一次尝试用的 uuid（模板求值用）。
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokengo_shared::LegId;

    fn leg(base_url: &str, headers: Vec<(&str, &str)>) -> Leg {
        Leg {
            id: LegId(1),
            name: "opencode-go".into(),
            base_url: base_url.into(),
            enabled: true,
            priority: 0,
            model_alias: Some("deepseek/deepseek-v4.1-flash".into()),
            vision: false,
            headers: headers
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }

    #[test]
    fn url_join_handles_trailing_and_leading_slashes() {
        assert_eq!(
            join_url("https://api.siliconflow.cn/v1", "chat/completions"),
            "https://api.siliconflow.cn/v1/chat/completions"
        );
        assert_eq!(
            join_url("https://api.siliconflow.cn/v1/", "/chat/completions"),
            "https://api.siliconflow.cn/v1/chat/completions"
        );
        // 面板里多填了一整段路径也要能对上
        assert_eq!(
            join_url("https://x.example.com/openai/v1", "embeddings"),
            "https://x.example.com/openai/v1/embeddings"
        );
    }

    #[test]
    fn the_leg_key_becomes_a_bearer_header() {
        let h = build_headers(&leg("https://x/v1", vec![]), Some("sk-abc"), "u", 1);
        assert_eq!(h, vec![("authorization".to_string(), "Bearer sk-abc".to_string())]);
    }

    #[test]
    fn a_leg_without_a_key_sends_no_authorization_at_all() {
        // OpenCode Zen 这类上游不需要 key，塞个空的 Bearer 反而被打回
        let h = build_headers(&leg("https://x/v1", vec![]), None, "u", 1);
        assert!(h.is_empty(), "没有 key 就不该带头: {h:?}");
        let h = build_headers(&leg("https://x/v1", vec![]), Some("   "), "u", 1);
        assert!(h.is_empty());
    }

    /// 关键增项：自定义请求头里的模板**逐请求**求值。
    #[test]
    fn custom_headers_are_rendered_per_request() {
        let l = leg(
            "https://x/v1",
            vec![
                ("x-opencode-client", "cli"),
                ("x-opencode-session", "{{uuid}}"),
                ("x-ts", "{{timestamp_ms}}"),
            ],
        );
        let a = build_headers(&l, None, "AAA", 1000);
        let b = build_headers(&l, None, "BBB", 1000);
        assert_eq!(a[0], ("x-opencode-client".to_string(), "cli".to_string()));
        assert_eq!(a[1], ("x-opencode-session".to_string(), "AAA".to_string()));
        assert_eq!(b[1], ("x-opencode-session".to_string(), "BBB".to_string()));
        assert_eq!(a[2].1, "1000");
    }

    #[test]
    fn an_empty_but_well_formed_list_is_not_confused_with_junk() {
        // 上游"有清单但空"和"回的压根不是清单"要能分开说
        assert!(looks_like_a_model_list(r#"{"object":"list","data":[]}"#));
        assert!(looks_like_a_model_list(r#"{"models":[]}"#));
        assert!(looks_like_a_model_list(r#"[]"#));
        assert!(!looks_like_a_model_list(r#"{"detail":"Not Found"}"#));
        assert!(!looks_like_a_model_list("<html>502</html>"));
        assert!(!looks_like_a_model_list(""));
    }

    #[test]
    fn uuid_is_a_fresh_one_each_time() {
        assert_ne!(new_uuid(), new_uuid());
    }

    #[test]
    fn openai_style_model_list_is_parsed() {
        let body = r#"{"object":"list","data":[
            {"id":"deepseek-v4.1-flash","object":"model"},
            {"id":"bge-m3","object":"model"},
            {"id":"deepseek-v4.1-flash","object":"model"}
        ]}"#;
        // 顺序按上游给的，重复的只留一个
        assert_eq!(parse_model_ids(body), vec!["deepseek-v4.1-flash", "bge-m3"]);
    }

    #[test]
    fn other_real_shapes_are_recognised_too() {
        // Ollama 风格用 name
        assert_eq!(
            parse_model_ids(r#"{"models":[{"name":"llama3.2"},{"name":"qwen3"}]}"#),
            vec!["llama3.2", "qwen3"]
        );
        // 有的上游直接回字符串数组
        assert_eq!(parse_model_ids(r#"["a","b"]"#), vec!["a", "b"]);
        // 只有 model 字段也认
        assert_eq!(parse_model_ids(r#"{"data":[{"model":"m1"}]}"#), vec!["m1"]);
    }

    #[test]
    fn junk_never_becomes_a_model_name() {
        // 认不出来就回空，**绝不编造名字** —— 面板会如实说"没认出清单"
        assert!(parse_model_ids("").is_empty());
        assert!(parse_model_ids("<html>502 Bad Gateway</html>").is_empty());
        assert!(parse_model_ids(r#"{"error":{"message":"unauthorized"}}"#).is_empty());
        assert!(parse_model_ids(r#"{"data":[{"nope":1},{"id":"  "},3]}"#).is_empty());
        assert!(parse_model_ids(r#"{"data":null}"#).is_empty());
    }

    #[test]
    fn whitespace_is_trimmed_and_blanks_dropped() {
        assert_eq!(
            parse_model_ids(r#"{"data":[{"id":"  spaced  "},{"id":""},{"id":"ok"}]}"#),
            vec!["spaced", "ok"]
        );
    }
}
