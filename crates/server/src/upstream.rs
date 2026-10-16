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
    fn uuid_is_a_fresh_one_each_time() {
        assert_ne!(new_uuid(), new_uuid());
    }
}
