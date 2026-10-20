//! 打上游的 HTTP 客户端。全项目只有这一处往外发请求。
//!
//! 三件事必须在这里做对，否则后面全是玄学：
//!
//! 1. **URL 拼接**：面板里填的 base_url 结尾有没有斜杠、有没有多写一段路径，
//!    都得好使（`https://x/v1` + `chat/completions`，以及 `https://x/v1/`）。
//! 2. **请求头**：Authorization 用这条渠道自己的 key；自定义请求头里的
//!    `{{uuid}}` / `{{timestamp_ms}}` 逐请求求值（上游 FreeLLMAPI 没有这个能力，
//!    本机要它就是因为 OpenCode 那条路要求每次带新的 session id）。
//! 3. **错误分类**：网络错/超时算渠道的毛病；4xx（客户端的错）不算 —— 分类在
//!    `tokengo_shared::routing::classify_status`，这里只负责把两种情形区分开。

use std::time::Duration;

use bytes::Bytes;
use tokengo_shared::channel::{render_header_value, Channel};

/// 一次尝试直接失败（网络层），没有拿到任何 HTTP 响应。算渠道的毛病。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelFault {
    pub message: String,
}

impl std::fmt::Display for ChannelFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// 打上游的客户端。内部连接池复用，进程里只建一个。
#[derive(Clone)]
pub struct Upstream {
    client: reqwest::Client,
    subscription_client:reqwest::Client,
    models_client: reqwest::Client,
}

impl Upstream {
    pub fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("tokengo/", env!("CARGO_PKG_VERSION")))
            .tcp_nodelay(true)
            .connect_timeout(Duration::from_secs(10))
            // 不设全局超时：每次尝试的超时由面板参数决定，逐请求传入
            .pool_idle_timeout(Duration::from_secs(90))
            .build()?;
        let subscription_client=crate::openai_transport::configure(reqwest::Client::builder()
            .user_agent(concat!("tokengo/",env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none()).tcp_nodelay(true)
            .connect_timeout(Duration::from_secs(10)).pool_idle_timeout(Duration::from_secs(90)))
            .map_err(anyhow::Error::msg)?.build().map_err(|_|anyhow::anyhow!("OpenAI inference client configuration failed"))?;
        let models_client = reqwest::Client::builder()
            .user_agent(concat!("tokengo/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .tcp_nodelay(true).connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90)).build()?;
        Ok(Self { client,subscription_client,models_client })
    }

    /// 往一条渠道上发一次请求。
    ///
    /// 返回 Ok 只表示"上游回了 HTTP 响应"，**不代表成功** —— 状态码是 200 还是 429
    /// 由调用方按 `classify_status` 判。返回 Err 表示网络层就没通（连不上/超时/TLS），
    /// 这属于渠道的毛病。
    ///
    /// `session` 是同一段对话稳定不变的会话 ID（自定义头 `{{session_id}}` 用）；
    /// 探活/拉清单这类没有请求体的场合传 `None` —— 那时这个头不会发出去。
    #[allow(clippy::too_many_arguments)]
    pub async fn call(
        &self,
        channel: &Channel,
        secret: Option<&str>,
        path: &str,
        body: Bytes,
        stream: bool,
        timeout: Duration,
        uuid: &str,
        now_ms: i64,
        session: Option<&str>,
    ) -> Result<reqwest::Response, ChannelFault> {
        self.call_via(channel,secret,path,body,stream,timeout,uuid,now_ms,session,false).await
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call_via(&self,channel:&Channel,secret:Option<&str>,path:&str,body:Bytes,stream:bool,timeout:Duration,uuid:&str,now_ms:i64,session:Option<&str>,subscription:bool)->Result<reqwest::Response,ChannelFault> {
        if subscription {
            let allowed=channel.base_url.trim_end_matches('/')==crate::oauth::API_BASE&&path=="responses";
            #[cfg(test)]let allowed=allowed||reqwest::Url::parse(&channel.base_url).is_ok_and(|url|url.host_str()==Some("127.0.0.1"));
            if !allowed {return Err(ChannelFault{message:"Invalid OpenAI subscription destination".into()});}
        }
        let url = join_url(&channel.base_url, path);
        let client=if subscription {&self.subscription_client}else{&self.client};
        let mut rb = client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body);
        if stream {
            rb = rb.header(reqwest::header::ACCEPT, "text/event-stream");
        }
        let mut headers=build_headers(channel,secret,uuid,now_ms,session);
        if path.trim_start_matches('/')=="messages" {
            let explicit_auth=channel.headers.iter().any(|(n,_)|n.eq_ignore_ascii_case("authorization"));
            if !explicit_auth {
                headers.retain(|(n,_)|!n.eq_ignore_ascii_case("authorization"));
                if !headers.iter().any(|(n,_)|n.eq_ignore_ascii_case("x-api-key")) {
                    if let Some(secret)=secret.filter(|s|!s.trim().is_empty()){headers.push(("x-api-key".into(),secret.trim().into()));}
                }
            }
            if !headers.iter().any(|(n,_)|n.eq_ignore_ascii_case("anthropic-version")){headers.push(("anthropic-version".into(),"2023-06-01".into()));}
        }
        for (name, value) in headers {
            let mut header = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| ChannelFault { message: "invalid configured header".into() })?;
            header.set_sensitive(true);
            rb = rb.header(name, header);
        }
        if !stream {
            return rb.timeout(timeout).send().await.map_err(|e| ChannelFault {
                message: if e.is_timeout() && !e.is_connect() { format!("等待非流式完成超时：{url}") } else { fault_message(&url, &e) },
            });
        }
        // A reqwest request timeout also covers the body. Streaming needs a
        // header deadline here and a fresh idle deadline for each body read.
        tokio::time::timeout(timeout, rb.send()).await
            .map_err(|_| ChannelFault { message: format!("等待上游响应超时：{url}") })?
            .map_err(|e| ChannelFault { message: fault_message(&url, &e) })
    }

    /// Custom diagnostics never follow redirects, expose raw errors, or buffer an unbounded body.
    #[allow(clippy::too_many_arguments)]
    pub async fn custom_probe_request(&self,channel:&Channel,secret:Option<&str>,path:&str,body:Bytes,timeout:Duration,limit:usize)->Result<(u16,Bytes),ChannelFault> {
        let uuid=new_uuid();let now=crate::db::now_ms();
        let mut headers=build_headers(channel,secret,&uuid,now,None);
        if path.trim_start_matches('/')=="messages" {
            if !channel.headers.iter().any(|(name,_)|name.eq_ignore_ascii_case("authorization")) {
                headers.retain(|(name,_)|!name.eq_ignore_ascii_case("authorization"));
                if !headers.iter().any(|(name,_)|name.eq_ignore_ascii_case("x-api-key")) {if let Some(secret)=secret.filter(|s|!s.trim().is_empty()){headers.push(("x-api-key".into(),secret.trim().into()));}}
            }
            if !headers.iter().any(|(name,_)|name.eq_ignore_ascii_case("anthropic-version")){headers.push(("anthropic-version".into(),"2023-06-01".into()));}
        }
        let fault=||ChannelFault{message:"custom probe transport failed".into()};
        let mut request=self.models_client.post(join_url(&channel.base_url,path)).header(reqwest::header::CONTENT_TYPE,"application/json").header(reqwest::header::ACCEPT,"application/json").body(body).timeout(timeout);
        for(name,value)in headers {let mut value=reqwest::header::HeaderValue::from_str(&value).map_err(|_|fault())?;value.set_sensitive(true);request=request.header(name,value);}
        let mut response=request.send().await.map_err(|_|fault())?;let status=response.status().as_u16();let mut bytes=Vec::new();
        while let Some(chunk)=response.chunk().await.map_err(|_|fault())? {if chunk.len()>limit.saturating_sub(bytes.len()){return Err(fault());}bytes.extend_from_slice(&chunk);}
        Ok((status,Bytes::from(bytes)))
    }

    /// OAuth diagnostics use the same subscription transport and native SSE collector as routing.
    pub async fn custom_probe_oauth_request(&self,source:&Channel,credential:&crate::oauth::OAuthCredential,body:Bytes,timeout:Duration,limit:usize)->Result<(u16,Bytes),ChannelFault> {
        let mut channel=source.clone();channel.base_url=credential.api_base.clone();channel.headers=crate::oauth::inference_headers(credential);
        let fault=||ChannelFault{message:"OAuth diagnostic response unavailable".into()};
        let mut response=self.call_via(&channel,Some(&credential.access_token),"responses",body,true,timeout,&new_uuid(),crate::db::now_ms(),None,true).await.map_err(|_|fault())?;
        let code=response.status().as_u16();
        let mime=response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v|v.to_str().ok()).map(str::to_ascii_lowercase);
        let mime_bucket=match mime.as_deref(){None=>"missing",Some(s)if s.contains("text/event-stream")=>"sse",Some(s)if s.contains("json")=>"json",Some(s)if s.contains("html")=>"html",_=>"other"};
        let encoding=response.headers().get(reqwest::header::CONTENT_ENCODING).and_then(|v|v.to_str().ok()).map(str::to_ascii_lowercase);
        let encoding_bucket=match encoding.as_deref(){None|Some("identity")=>"identity",Some("gzip")=>"gzip",Some("br")=>"br",Some("zstd")=>"zstd",_=>"other"};
        let mut count=0usize;let mut bytes=Vec::new();let mut collector=crate::protocol::collect::ResponsesCollector::default();
        // stream=true is authoritative. MIME is only a hint: official/proxied SSE
        // may omit or mislabel it. A JSON success body remains compatible.
        let mut sse=None;let mut prefix_format="empty";
        let response_fault=|stage:&str,format:&str|ChannelFault{message:format!("oauth_response_{stage}; http={code}; mime={mime_bucket}; encoding={encoding_bucket}; format={format}")};
        while let Some(chunk)=response.chunk().await.map_err(|_|response_fault("read",prefix_format))? {
            if chunk.len()>limit.saturating_sub(count){return Err(response_fault("too_large",prefix_format));}count+=chunk.len();
            if sse.is_none(){
                bytes.extend_from_slice(&chunk);
                if bytes.len()<3&&[0xef,0xbb,0xbf].starts_with(&bytes){continue;}
                if bytes.starts_with(&[0xef,0xbb,0xbf]){bytes.drain(..3);}
                let Some(first)=bytes.iter().copied().find(|b|!b.is_ascii_whitespace())else{continue};
                let is_sse=(200..300).contains(&code)&&!matches!(first,b'{'|b'[');
                prefix_format=if matches!(first,b'{'|b'['){"json"}else if first==0x1f||first>=0x80{"binary"}else{"sse_or_text"};
                sse=Some(is_sse);
                if is_sse{collector.feed(&bytes).map_err(|_|response_fault("events",prefix_format))?;bytes.clear();}
            }else if sse==Some(true){collector.feed(&chunk).map_err(|_|response_fault("events",prefix_format))?;}else{bytes.extend_from_slice(&chunk);}
            if sse==Some(true)&&collector.observer.done{break;}
        }
        if sse==Some(true){bytes=serde_json::to_vec(&collector.finish().map_err(|_|response_fault("incomplete_events",prefix_format))?).map_err(|_|response_fault("json",prefix_format))?;}
        else if serde_json::from_slice::<serde_json::Value>(&bytes).is_err(){return Err(response_fault("invalid_json",prefix_format));}
        if bytes.len()>limit{return Err(response_fault("too_large",prefix_format));}Ok((code,Bytes::from(bytes)))
    }

    /// Successful probes consume a bounded body and validate the endpoint structure.
    #[allow(clippy::too_many_arguments)]
    pub async fn probe(
        &self,
        channel: &Channel,
        secret: Option<&str>,
        path: &str,
        body: Bytes,
        timeout: Duration,
        uuid: &str,
        now_ms: i64,
    ) -> Result<u16, ChannelFault> {
        self.probe_report(channel, secret, path, body, timeout, uuid, now_ms).await.map(|v|v.0)
    }

    /// Diagnostic variant: bounded, credential-redacted upstream error message.
    #[allow(clippy::too_many_arguments)]
    pub async fn probe_report(
        &self, channel:&Channel, secret:Option<&str>, path:&str, body:Bytes,
        timeout:Duration, uuid:&str, now_ms:i64,
    ) -> Result<(u16,Option<String>),ChannelFault> {
        let mut r = self
            .call(channel, secret, path, body, false, timeout, uuid, now_ms, None)
            .await?;
        let status = r.status().as_u16();
        if !(200..300).contains(&status) {
            let mut bytes=Vec::new();
            while let Some(chunk)=r.chunk().await.map_err(|_|ChannelFault{message:"读取诊断错误响应失败".into()})? {
                let take=chunk.len().min(4096usize.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&chunk[..take]);
                if bytes.len()==4096 {break;}
            }
            let mut redactions=channel.headers.iter().map(|(_,v)|v.as_str()).collect::<Vec<_>>();
            if let Some(secret)=secret {redactions.push(secret);}
            return Ok((status,probe_error_note(&bytes,&redactions)));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = r.chunk().await.map_err(|e| ChannelFault { message: format!("probe body read failed: {e}") })? {
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(ChannelFault { message: "probe response exceeds 2 MiB".into() });
            }
            bytes.extend_from_slice(&chunk);
        }
        let mut v: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| ChannelFault { message: format!("invalid probe JSON: {e}") })?;
        if channel.unwrap_envelope { tokengo_shared::unwrap_envelope(&mut v); }
        validate_response(&v, path).map_err(|message| ChannelFault { message })?;
        Ok((status,None))
    }

    /// 「测视觉」用：要状态码 **和** 正文开头 —— 判断"它到底看没看见图"得看模型
    /// 回的话（回了"红"才是真看见了；回 400 说明它压根不收图片）。
    ///
    /// 正文最多留 4 KB：上游可能吐一整个错误页，这里只要够看懂就行。
    #[allow(clippy::too_many_arguments)]
    pub async fn call_and_read(
        &self,
        channel: &Channel,
        secret: Option<&str>,
        path: &str,
        body: Bytes,
        timeout: Duration,
        uuid: &str,
        now_ms: i64,
    ) -> Result<(u16, String), ChannelFault> {
        let r = self
            .call(channel, secret, path, body, false, timeout, uuid, now_ms, None)
            .await?;
        let code = r.status().as_u16();
        let text = r.text().await.unwrap_or_default();
        Ok((code, cap_chars(&text, 4000)))
    }

    /// 拉这条渠道支持的模型清单：`GET {base_url}/models`，用这条渠道自己的
    /// URL、key 和自定义请求头（头里的模板照旧逐请求求值）。
    ///
    /// **面板专用的只读动作**，和探活一个纪律：不写记账、不动路由状态。
    /// 返回 `(状态码, 正文)` —— 正文交给 `parse_model_ids` 挑名字，
    /// 状态码交给调用方判断"上游是不是根本不支持这个端点"。
    pub async fn fetch_models(
        &self,
        channel: &Channel,
        secret: Option<&str>,
        timeout: Duration,
        uuid: &str,
        now_ms: i64,
    ) -> Result<(u16, String), ChannelFault> {
        let url = join_url(&channel.base_url, "models");
        let mut rb = self
            .models_client
            .get(&url)
            .timeout(timeout)
            .header(reqwest::header::ACCEPT, "application/json");
        for (name, value) in build_headers(channel, secret, uuid, now_ms, None) {
            let mut header = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| ChannelFault { message: "invalid configured header".into() })?;
            header.set_sensitive(true);
            rb = rb.header(name, header);
        }
        let mut r = rb.send().await.map_err(|e| ChannelFault {
            message: fault_message(&url, &e),
        })?;
        let status = r.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = r.chunk().await.map_err(|e| ChannelFault {
            message: fault_message(&url, &e),
        })? {
            if bytes.len() + chunk.len() > MAX_MODELS_BODY {
                return Err(ChannelFault { message: "Upstream model catalog exceeds 2 MiB".into() });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
    }
}

/// 上游模型清单的正文上限。真有上游吐上千个模型，超过就当拉不到 ——
/// 面板本来也塞不下那个量级，硬留着只会把网关内存顶起来。
const MAX_MODELS_BODY: usize = 2 * 1024 * 1024;


/// 上游 `/v1/models` 里的一条：模型名 + **上游自己报**的说明。
///
/// 每个说明字段只在**上游真给了**的时候才有值；上游没给就是 `None`，
/// 界面上什么都不显示。**不许拿名字猜** —— "名字里带 vl 就能看图"这条
/// 已经在视觉能力上栽过一次（deepseek 的 flash 能读图，有的 vision 名字反倒不能）。
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct UpstreamModel {
    pub reasoning: crate::reasoning_capabilities::ReasoningCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 上游认的名字（客户端要发的就是它）。
    pub id: String,
    /// 归属方（`owned_by` / `organization` / `publisher` …）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
    /// 上游自报的上下文长度（tokens）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    /// 价格提示：上游写 `free` 就是 `free`，写数字就换算成"每百万 tokens 多少美元"。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_note: Option<String>,
    /// 上游报的每一档价格都是 0（或者它直接写了 free）。**只按上游的话判**，
    /// 绝不拿提示文字去猜（不然"$10/M in"会被涂成绿色）。
    #[serde(skip_serializing_if = "is_false")]
    pub is_free: bool,
    /// 上游**自称**能不能收图。这不是我们的"能看图"标记 ——
    /// 那个只认面板上实测（真发一张图问它左上角什么颜色）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// 模型清单的信封字段：`data`（OpenAI 与多数中转）、`models`（Ollama）、
/// `result`/`results`/`items`（别家网关）。
const LIST_KEYS: [&str; 5] = ["data", "models", "result", "results", "items"];
/// 名字字段的各种拼法。
const ID_KEYS: [&str; 6] = ["id", "name", "model", "model_id", "modelId", "slug"];
/// 归属方字段的各种拼法。
const OWNER_KEYS: [&str; 6] = [
    "owned_by",
    "ownedBy",
    "organization",
    "owner",
    "provider",
    "publisher",
];
/// 上下文长度字段的各种拼法（OpenRouter / Ollama / vLLM 各写各的）。
const CONTEXT_KEYS: [&str; 7] = [
    "context_length",
    "context_window",
    "max_model_len",
    "max_context_length",
    "max_context_tokens",
    "ctx_len",
    "contextWindow",
];
/// 价格字段（OpenRouter 是 `pricing:{prompt,completion}`，也有的直接写字符串）。
const PRICE_KEYS: [&str; 2] = ["price", "pricing"];
/// "能不能收图"的布尔字段。
const VISION_KEYS: [&str; 5] = [
    "vision",
    "supports_vision",
    "supportsVision",
    "image_input",
    "multimodal",
];
/// 模态字段（OpenRouter 把真信号放在 `architecture` 里）。
const MODALITY_KEYS: [&str; 4] = ["modalities", "input_modalities", "inputModalities", "modality"];
/// 模态里哪几个值代表能收图。
const VISION_MODALITIES: [&str; 3] = ["image", "vision", "image-input"];
/// 低于这个数只可能是"每 token"报价（OpenRouter 写 `0.00000125`），
/// 大于等于它的已经是"每百万"了。
const PER_TOKEN_CEILING: f64 = 0.01;
/// 名字长到这个程度肯定不是模型名（别把一段正文塞进列表）。
const MAX_MODEL_ID_LEN: usize = 256;

/// 模型数组在哪个字段里 —— 信封长得五花八门，最多再往下一层找
/// （`{"data":{"models":[…]}}` 这种包两层的也见过）。
fn find_model_array(v: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    fn dive(v: &serde_json::Value, depth: u8) -> Option<&Vec<serde_json::Value>> {
        if let Some(arr) = v.as_array() {
            return Some(arr);
        }
        let o = v.as_object()?;
        for key in LIST_KEYS {
            let Some(value) = o.get(key) else { continue };
            if let Some(arr) = value.as_array() {
                return Some(arr);
            }
            if depth < 1 {
                if let Some(found) = dive(value, depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }
    dive(v, 0)
}

fn first_string(o: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        o.get(*k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}

/// 上下文长度：数字或者"数字字符串"都认（有的上游把数字写成字符串），
/// 不是正数就当没给。
fn first_positive_int(
    o: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<i64> {
    keys.iter().find_map(|k| {
        let v = o.get(*k)?;
        let n = match v {
            serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
            _ => None,
        }?;
        (n > 0).then_some(n)
    })
}

/// 一份价格换算成"每百万 tokens 多少美元"。上游写的是每 token 就按量级换算。
fn per_million(raw: &serde_json::Value) -> Option<f64> {
    let value = match raw {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some(if value > 0.0 && value < PER_TOKEN_CEILING {
        value * 1_000_000.0
    } else {
        value
    })
}

/// 好看的美元数字：最多四位小数，不留浮点尾巴。
fn usd(value: f64) -> String {
    let rounded = (value * 10_000.0).round() / 10_000.0;
    format!("${rounded}")
}

/// 价格提示 + "是不是免费"。认不出来就返回 None（界面上一个标签都不显示）。
fn price_hint(o: &serde_json::Map<String, serde_json::Value>) -> Option<(String, bool)> {
    for key in PRICE_KEYS {
        let Some(value) = o.get(key) else { continue };
        if let Some(text) = value.as_str() {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            // 只有"就是 free 这个词"才算免费 —— 绝不拿提示文字去猜
            if text.eq_ignore_ascii_case("free") {
                return Some(("free".to_string(), true));
            }
            return Some((cap_chars(text, 39), false));
        }
        let Some(pricing) = value.as_object() else {
            continue;
        };
        let prompt = pricing.get("prompt").and_then(per_million);
        let completion = pricing.get("completion").and_then(per_million);
        if prompt.is_none() && completion.is_none() {
            continue;
        }
        // OpenRouter 的 `:free` 名字报的是平的 {"prompt":"0","completion":"0"}
        if prompt.unwrap_or(0.0) == 0.0 && completion.unwrap_or(0.0) == 0.0 {
            return Some(("free".to_string(), true));
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(p) = prompt.filter(|p| *p > 0.0) {
            parts.push(format!("{}/M in", usd(p)));
        }
        if let Some(c) = completion.filter(|c| *c > 0.0) {
            parts.push(format!("{}/M out", usd(c)));
        }
        return Some((cap_chars(&parts.join(" "), 39), false));
    }
    None
}

/// 从模态字段判能不能收图：数组（`["text","image"]`）或者字符串
/// （`"text+image->text"` —— 只看箭头左边的输入侧）。
fn modality_vision(o: &serde_json::Map<String, serde_json::Value>) -> Option<bool> {
    for key in MODALITY_KEYS {
        let Some(value) = o.get(key) else { continue };
        if let Some(arr) = value.as_array() {
            return Some(arr.iter().any(|m| {
                m.as_str()
                    .map(|s| {
                        let s = s.to_lowercase();
                        VISION_MODALITIES.iter().any(|v| s == *v)
                    })
                    .unwrap_or(false)
            }));
        }
        if let Some(text) = value.as_str() {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let input = text.to_lowercase();
            let input = input.split("->").next().unwrap_or("").to_string();
            return Some(VISION_MODALITIES.iter().any(|v| input.contains(v)));
        }
    }
    None
}

/// 上游**自称**能不能收图：布尔字段、顶层模态数组，或者 OpenRouter 放在
/// `architecture` 里的那份。都没有就是"没说"（None），界面上不标。
fn vision_of(o: &serde_json::Map<String, serde_json::Value>) -> Option<bool> {
    for key in VISION_KEYS {
        if let Some(b) = o.get(key).and_then(|v| v.as_bool()) {
            return Some(b);
        }
    }
    if let Some(v) = modality_vision(o) {
        return Some(v);
    }
    o.get("architecture")
        .and_then(|a| a.as_object())
        .and_then(modality_vision)
}

/// Only explicit type/endpoint declarations count; model names and input modalities do not.
fn declared_kind(o: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let mut found = Vec::new();
    for key in ["kind", "type", "model_type", "task", "supported_endpoints", "endpoints"] {
        if let Some(v) = o.get(key) {
            let values: Vec<&str> = if let Some(s) = v.as_str() { vec![s] }
                else { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default() };
            for raw in values {
                let k = match raw.trim().trim_start_matches('/').trim_start_matches("v1/") {
                    "chat" | "chat/completions" | "text-generation" => "chat",
                    "embedding" | "embeddings" | "feature-extraction" => "embeddings",
                    "rerank" | "reranking" | "text-ranking" => "rerank",
                    "image" | "images/generations" | "text-to-image" => "image",
                    "video" | "text-to-video" => "video",
                    "audio" | "audio/speech" | "audio/transcriptions" => "audio",
                    _ => continue,
                };
                if !found.contains(&k) { found.push(k); }
            }
        }
    }
    if found.len() == 1 { Some(found[0].to_string()) } else { None }
}

/// 一条上游条目 → 一条说明。认不出名字就整条丢掉（不编）。
fn to_upstream_model(entry: &serde_json::Value) -> Option<UpstreamModel> {
    if let Some(s) = entry.as_str() {
        let id = s.trim();
        return (!id.is_empty())
            .then(|| UpstreamModel { id: id.to_string(), ..Default::default() });
    }
    let o = entry.as_object()?;
    let id = first_string(o, &ID_KEYS)?;
    if id.chars().count() > MAX_MODEL_ID_LEN {
        return None;
    }
    let mut m = UpstreamModel {
        id,
        owned_by: first_string(o, &OWNER_KEYS),
        ..Default::default()
    };
    m.context_window = first_positive_int(o, &CONTEXT_KEYS);
    if let Some((note, free)) = price_hint(o) {
        m.price_note = Some(note);
        m.is_free = free;
    }
    m.kind = declared_kind(o);
    m.vision = vision_of(o);
    m.reasoning = crate::reasoning_capabilities::from_metadata(entry);
    Some(m)
}

/// 解析上游 `/v1/models`：模型名 + 上游自己报的说明，顺序按上游给的
/// （不排序），同名的只留第一条，最多 [`MAX_MODEL_IDS`] 条。
///
/// 同时回一个"上游一共报了多少条"（截断之前），面板要如实说
/// "报了 N 个，这里只列了前 …" 而不是假装就这么多。
pub fn parse_model_catalog_with_total(body: &str) -> (Vec<UpstreamModel>, usize) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return (Vec::new(), 0);
    };
    let Some(entries) = find_model_array(&v) else {
        return (Vec::new(), 0);
    };
    let mut out: Vec<UpstreamModel> = Vec::with_capacity(entries.len().min(MAX_MODEL_IDS));
    for entry in entries {
        let Some(m) = to_upstream_model(entry) else {
            continue;
        };
        if out.iter().any(|x| x.id == m.id) {
            continue;
        }
        out.push(m);
    }
    let seen = out.len();
    out.truncate(MAX_MODEL_IDS);
    (out, seen)
}

/// 只要清单本身（不要"一共报了多少"那一位）。
pub fn parse_model_catalog(body: &str) -> Vec<UpstreamModel> {
    parse_model_catalog_with_total(body).0
}

/// 只挑模型名（老口径，面板之外的地方还在用）。
///
/// **认不出来就回空列表** —— 面板会说"拉回来了但没认出清单"，绝不编造名字。
pub fn parse_model_ids(body: &str) -> Vec<String> {
    parse_model_catalog(body).into_iter().map(|m| m.id).collect()
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
    find_model_array(&v).is_some()
}

/// 网络错分类成人能看懂的话。**不要把 key 打进日志。**
fn fault_message(url: &str, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        if e.is_connect() { format!("连接超时：{url}") } else { format!("超时：{url}") }
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

/// 解析 `Retry-After` 头 —— **只认「秒数」这一种写法**（HTTP-date 那种一律当没给，
/// 宁可不猜）。返回毫秒。
///
/// 为什么要它：上游回 429 时它自己会说「等多久再来」。现网实测里 429 有 13 次，
/// 其中「Go usage limit exceeded」和「周额度用完」这两种等 1 秒重试是毫无意义的
/// —— 额度的恢复时间以小时计。照着它说的等，或者干脆别在这条渠道上耗，更省时间。
pub fn retry_after_ms(value: &str) -> Option<u64> {
    let v = value.trim();
    if v.is_empty() || !v.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    v.parse::<u64>().ok().map(|s| s.saturating_mul(1000))
}

/// 自定义 Authorization 优先；未配置时才用渠道 key 生成 Bearer 头。
///
/// 模板逐请求求值：`{{uuid}}` 每次都是新的，就是这个让 OpenCode 那条路能通；
/// `{{session_id}}` 是同一段对话稳定不变的那个（`session` 为 None 时 —— 探活、
/// 拉清单 —— 渲染成空，**整条头不发**：宁可这个头不发出去，也不要发一个空值去上游碰运气）。
///
/// 名字为空、值渲染后为空的头一律丢掉。
pub fn build_headers(
    channel: &Channel,
    secret: Option<&str>,
    uuid: &str,
    now_ms: i64,
    session: Option<&str>,
) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(channel.headers.len() + 1);
    for (name, value) in &channel.headers {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let value = render_header_value(value, uuid, now_ms, session);
        if value.trim().is_empty() {
            continue;
        }
        out.push((name.to_string(), value));
    }
    if !out.iter().any(|(n, _)| n.eq_ignore_ascii_case("authorization") || n.eq_ignore_ascii_case("x-api-key")) {
        if let Some(s) = secret.filter(|s| !s.trim().is_empty()) {
            out.push(("authorization".to_string(), format!("Bearer {}", s.trim())));
        }
    }
    out
}

/// 新的一次尝试用的 uuid（模板求值用）。
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Add LiteLLM/Jina response conveniences without discarding raw provider usage.
/// Missing/null documents are filled separately by the gateway's existing contract.
pub(crate) fn normalize_rerank_response(value: &mut serde_json::Value) -> Result<(), String> {
    use serde_json::{Map, Value};
    let usage = value.get("usage").filter(|v| !v.is_null()).cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let usage = usage.as_object().ok_or("invalid upstream rerank usage object")?;
    // A malformed meter must never become a reported zero-token charge.
    for field in ["input_tokens", "output_tokens", "prompt_tokens", "completion_tokens", "total_tokens", "search_units"] {
        if usage.get(field).is_some_and(|v| !v.is_null() && v.as_i64().is_none_or(|n| n < 0)) {
            return Err(format!("invalid upstream rerank usage.{field}"));
        }
    }
    let object = value.as_object_mut().ok_or("invalid upstream rerank response object")?;
    if object.get("id").is_none_or(|v| v.is_null() || v.as_str().is_some_and(str::is_empty)) {
        object.insert("id".into(), Value::String(uuid::Uuid::new_v4().to_string()));
    }
    if let Some(results) = object.get_mut("results").and_then(Value::as_array_mut) {
        for result in results {
            if let Some(text) = result.get("document").and_then(Value::as_str).map(str::to_owned) {
                result["document"] = serde_json::json!({"text": text});
            }
        }
    }
    let meta = object.entry("meta").or_insert_with(|| Value::Object(Map::new()));
    if meta.is_null() { *meta = Value::Object(Map::new()); }
    let meta = meta.as_object_mut().ok_or("invalid upstream rerank meta object")?;
    // These are the installed Jina adapter's TypedDict fields. Preserve existing
    // provider meta extensions, and keep the original usage object untouched.
    for (group, fields) in [
        ("billed_units", ["search_units", "total_tokens"]),
        ("tokens", ["input_tokens", "output_tokens"]),
    ] {
        let target = meta.entry(group).or_insert_with(|| Value::Object(Map::new()));
        if target.is_null() { *target = Value::Object(Map::new()); }
        let target = target.as_object_mut().ok_or("invalid upstream rerank meta group")?;
        for field in fields {
            if let Some(v) = usage.get(field) { target.insert(field.into(), v.clone()); }
        }
    }
    Ok(())
}

/// Validate the endpoint structure without rewriting provider-specific fields.
pub(crate) fn validate_response(v: &serde_json::Value, path: &str) -> Result<(), String> {
    if tokengo_shared::empty_chat_reply(v) {
        return Err("上游回了 200 但一句话都没说（空回复）".into());
    }
    let valid = match path.trim_start_matches('/') {
        "responses"=>return crate::protocol::validate_response(crate::protocol::Protocol::Responses,v),
        "messages"=>return crate::protocol::validate_response(crate::protocol::Protocol::Messages,v),
        "chat/completions" => v.get("choices").and_then(|x| x.as_array())
            .is_some_and(|c| !c.is_empty() && !tokengo_shared::empty_chat_reply(v)),
        "embeddings" => v.get("data").and_then(|x| x.as_array())
            .is_some_and(|items| !items.is_empty() && items.iter().all(|x|
                x.get("embedding").is_some_and(|e| e.is_array() || e.is_string()))),
        "rerank" => v.get("results").and_then(|x| x.as_array())
            .is_some_and(|items| items.iter().all(|x| x.get("index").and_then(|i| i.as_u64()).is_some()
                && x.get("relevance_score").and_then(|i| i.as_f64()).is_some())),
        _ => false,
    };
    if valid { Ok(()) } else { Err(format!("invalid or empty upstream {path} response")) }
}

/// Only expose structured error fields, never arbitrary HTML or a full response body.
fn probe_error_note(bytes:&[u8],redactions:&[&str])->Option<String> {
    let value:serde_json::Value=serde_json::from_slice(bytes).ok()?;
    let message=value.pointer("/error/message").or_else(||value.get("message")).or_else(||value.get("error")).and_then(|v|v.as_str())?;
    let mut safe=message.to_string();
    for raw in redactions.iter().filter(|s|!s.is_empty()) {safe=safe.replace(raw,"[已隐藏凭据]");}
    Some(safe.chars().filter(|c|!c.is_control()||*c==' ').take(512).collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn probe_errors_are_structured_bounded_and_redacted() {
        let v=super::probe_error_note(br#"{"error":{"message":"quota exhausted secret-key custom-header"}}"#,&["secret-key","custom-header"]).unwrap();
        assert!(!v.contains("secret-key"));assert!(!v.contains("custom-header"));assert!(v.contains("quota exhausted"));
        assert!(super::probe_error_note(b"<html>secret</html>",&[]).is_none());
        let bytes=serde_json::to_vec(&serde_json::json!({"message":"x".repeat(2000)})).unwrap();
        assert_eq!(super::probe_error_note(&bytes,&[]).unwrap().chars().count(),512);
    }

    use super::*;
    use tokengo_shared::ChannelId;

    fn channel(base_url: &str, headers: Vec<(&str, &str)>) -> Channel {
        Channel {
            id: ChannelId(1),
            name: "opencode-go".into(),
            base_url: base_url.into(),
            enabled: true,
            priority: 0,
            model_alias: Some("deepseek/deepseek-v4.1-flash".into()),
            vision: false,
            drop_fields: vec![],
            unwrap_envelope: false,
            headers: headers
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
                max_concurrent: 0,
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
    fn custom_authorization_works_without_key_and_overrides_bearer() {
        let c = channel("https://example.invalid/v1", vec![("Authorization", "Basic private-{{uuid}}")]);
        for key in [None, Some("fallback-key")] {
            assert_eq!(build_headers(&c,key,"fresh",42,None),vec![("Authorization".into(),"Basic private-fresh".into())]);
        }
    }

    #[test]
    fn the_channel_key_becomes_a_bearer_header() {
        let h = build_headers(
            &channel("https://x/v1", vec![]),
            Some("sk-abc"),
            "u",
            1,
            None,
        );
        assert_eq!(h, vec![("authorization".to_string(), "Bearer sk-abc".to_string())]);
    }

    #[test]
    fn a_channel_without_a_key_sends_no_authorization_at_all() {
        // OpenCode Zen 这类上游不需要 key，塞个空的 Bearer 反而被打回
        let h = build_headers(&channel("https://x/v1", vec![]), None, "u", 1, None);
        assert!(h.is_empty(), "没有 key 就不该带头: {h:?}");
        let h = build_headers(&channel("https://x/v1", vec![]), Some("   "), "u", 1, None);
        assert!(h.is_empty());
    }

    /// 关键增项：自定义请求头里的模板**逐请求**求值。
    #[test]
    fn custom_headers_are_rendered_per_request() {
        let l = channel(
            "https://x/v1",
            vec![
                ("x-opencode-client", "cli"),
                ("x-opencode-session", "{{uuid}}"),
                ("x-ts", "{{timestamp_ms}}"),
            ],
        );
        let a = build_headers(&l, None, "AAA", 1000, None);
        let b = build_headers(&l, None, "BBB", 1000, None);
        assert_eq!(a[0], ("x-opencode-client".to_string(), "cli".to_string()));
        assert_eq!(a[1], ("x-opencode-session".to_string(), "AAA".to_string()));
        assert_eq!(b[1], ("x-opencode-session".to_string(), "BBB".to_string()));
        assert_eq!(a[2].1, "1000");
    }

    /// 会话 ID 走的是**另一个**模板：同一段对话两次请求必须带同一个值。
    #[test]
    fn the_session_header_carries_the_stable_id_and_is_dropped_when_there_is_none() {
        let l = channel(
            "https://x/v1",
            vec![
                ("x-opencode-session", "{{session_id}}"),
                ("x-opencode-request", "{{uuid}}"),
            ],
        );
        let a = build_headers(&l, None, "AAA", 1000, Some("sess-1"));
        let b = build_headers(&l, None, "BBB", 1000, Some("sess-1"));
        assert_eq!(a[0].1, "sess-1");
        assert_eq!(b[0].1, "sess-1", "会话头不许跟着 uuid 一起变");
        assert_eq!(a[1].1, "AAA");
        assert_eq!(b[1].1, "BBB");
        // 探活/拉清单：没有会话 → 这个头整条不发（不是发一个空值）
        let c = build_headers(&l, None, "AAA", 1000, None);
        assert_eq!(c.len(), 1, "空值的头必须被丢掉: {c:?}");
        assert_eq!(c[0].0, "x-opencode-request");
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
    fn retry_after_only_trusts_plain_seconds() {
        assert_eq!(retry_after_ms("0"), Some(0));
        assert_eq!(retry_after_ms(" 3 "), Some(3000));
        assert_eq!(retry_after_ms("120"), Some(120_000));
        // 反证：HTTP-date 与垃圾值一律当没给（不猜）
        assert_eq!(retry_after_ms("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(retry_after_ms("-1"), None);
        assert_eq!(retry_after_ms("3.5"), None);
        assert_eq!(retry_after_ms(""), None);
    }

    #[test]
    fn uuid_is_a_fresh_one_each_time() {
        assert_ne!(new_uuid(), new_uuid());
    }

    #[test]
    fn explicit_catalog_types_are_parsed_without_guessing() {
        let cat = parse_model_catalog(r#"{"data":[
            {"id":"unknown","kind":"embeddings"},
            {"id":"another","supported_endpoints":["/v1/rerank"]},
            {"id":"image-vision-embedding"},
            {"id":"ambiguous","kind":"chat","type":"image"},
            {"id":"multi","endpoints":["chat/completions","embeddings"]}
        ]}"#);
        assert_eq!(cat[0].kind.as_deref(), Some("embeddings"));
        assert_eq!(cat[1].kind.as_deref(), Some("rerank"));
        assert!(cat[2..].iter().all(|m| m.kind.is_none()));
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

    // ---------- 拉清单时把上游说的"信息"也带回来 ----------

    /// OpenAI 形状：名字 + 归属方。上游没给的字段（上下文/价格/能不能收图）
    /// **一个都不许凭空出现** —— 界面据此决定显示哪些小标签。
    #[test]
    fn the_catalog_carries_only_what_the_upstream_actually_said() {
        let cat = parse_model_catalog(
            r#"{"object":"list","data":[
                {"id":"deepseek-v4.1-flash","object":"model","created":1750000000,"owned_by":"deepseek"},
                {"id":"bge-m3","object":"model","owned_by":"BAAI"}
            ]}"#,
        );
        assert_eq!(cat.len(), 2);
        assert_eq!(cat[0].id, "deepseek-v4.1-flash");
        assert_eq!(cat[0].owned_by.as_deref(), Some("deepseek"));
        assert_eq!(cat[0].context_window, None, "上游没说上下文，就不许自己填");
        assert_eq!(cat[0].price_note, None, "上游没说价钱，就不许自己填");
        assert_eq!(cat[0].vision, None, "上游没说能不能收图，就不许猜");
        assert!(!cat[0].is_free);
        assert_eq!(cat[1].owned_by.as_deref(), Some("BAAI"));
    }

    /// OpenRouter 那种：上下文长度 + `pricing:{prompt,completion}`（写的是每 token
    /// 的价，要按量级换算成每百万）+ `architecture` 里的 `input_modalities`。
    #[test]
    fn openrouter_style_details_are_read() {
        let cat = parse_model_catalog(
            r#"{"data":[{
                "id":"deepseek/deepseek-v4.1-flash",
                "context_length":163840,
                "pricing":{"prompt":"0.00000125","completion":"0.0000025"},
                "architecture":{"input_modalities":["text","image"],"modality":"text+image->text"}
            }]}"#,
        );
        assert_eq!(cat[0].context_window, Some(163_840));
        assert_eq!(cat[0].price_note.as_deref(), Some("$1.25/M in $2.5/M out"));
        assert!(!cat[0].is_free, "有价钱就不是免费");
        assert_eq!(cat[0].vision, Some(true));
    }

    /// 免费只有两种写法算数：就是 `free` 这个词，或者每一档价都是 0。
    /// **绝不拿提示文字去猜** —— 不然 "$10/M in" 会被涂成绿色。
    #[test]
    fn free_is_only_the_word_free_or_all_zero_prices() {
        let a = parse_model_catalog(r#"{"data":[{"id":"x","price":"free"}]}"#);
        assert_eq!(a[0].price_note.as_deref(), Some("free"));
        assert!(a[0].is_free);

        let b = parse_model_catalog(
            r#"{"data":[{"id":"y","pricing":{"prompt":"0","completion":"0"}}]}"#,
        );
        assert!(b[0].is_free, "OpenRouter 的 :free 报的是平的 0");

        let c = parse_model_catalog(r#"{"data":[{"id":"z","price":"$10/M in $20/M out"}]}"#);
        assert_eq!(c[0].price_note.as_deref(), Some("$10/M in $20/M out"));
        assert!(!c[0].is_free, "写了价钱就不是免费");

        // 反证：别家写的 free 字样混在句子里，不算
        let d = parse_model_catalog(r#"{"data":[{"id":"w","price":"freemium $1/M"}]}"#);
        assert!(!d[0].is_free);
    }

    /// 别的信封与拼法：Ollama 的 `models`+`name`+`ctx_len`、包两层的 `result`、
    /// 纯字符串数组、数字写成字符串。
    #[test]
    fn other_envelopes_and_spellings_are_recognised() {
        let a = parse_model_catalog(r#"{"models":[{"name":"llama3.2","ctx_len":"8192"}]}"#);
        assert_eq!(a[0].id, "llama3.2");
        assert_eq!(a[0].context_window, Some(8192), "数字写成字符串也认");

        let b = parse_model_catalog(r#"{"result":{"data":[{"model_id":"m1","publisher":"acme"}]}}"#);
        assert_eq!(b[0].id, "m1");
        assert_eq!(b[0].owned_by.as_deref(), Some("acme"));

        let c = parse_model_catalog(r#"["a","b"]"#);
        assert_eq!(c.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);

        // 布尔字段与顶层模态数组两种说法
        let d = parse_model_catalog(r#"{"data":[{"id":"p","supports_vision":true},{"id":"q","modalities":["text"]}]}"#);
        assert_eq!(d[0].vision, Some(true));
        assert_eq!(d[1].vision, Some(false), "只列了文本 = 上游明说不能收图");
    }

    /// 同名的只留第一条（第一个说法的信息为准），顺序按上游给的。
    #[test]
    fn duplicates_keep_the_first_entry_and_upstream_order() {
        let cat = parse_model_catalog(
            r#"{"data":[{"id":"b","owned_by":"first"},{"id":"a"},{"id":"b","owned_by":"second"}]}"#,
        );
        assert_eq!(cat.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
        assert_eq!(cat[0].owned_by.as_deref(), Some("first"));
    }

    /// 上限之外的条目要能被数出来（面板要如实说"报了 N 个、这里列了前 M 个"）。
    #[test]
    fn the_total_count_survives_the_cap() {
        let items: Vec<String> = (0..(MAX_MODEL_IDS + 5))
            .map(|i| format!(r#"{{"id":"m{i}"}}"#))
            .collect();
        let body = format!(r#"{{"data":[{}]}}"#, items.join(","));
        let (cat, seen) = parse_model_catalog_with_total(&body);
        assert_eq!(cat.len(), MAX_MODEL_IDS);
        assert_eq!(seen, MAX_MODEL_IDS + 5, "截断前一共多少条要数得出来");
    }
}
