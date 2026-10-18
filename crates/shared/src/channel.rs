//! 渠道（上游通道）与模型组。
//!
//! **本项目不做免费模型目录，也不做 provider 适配器。** 上游全部由用户自定义添加：
//! 一条渠道 = 名称 + base_url + key + 自定义请求头 + 该渠道认的上游模型名。
//!
//! 上游 FreeLLMAPI 有 37 个内置 provider 适配器和 72 个平台枚举，但本机现网 9 条渠道
//! **全部**是 `platform=custom`，一个内置适配器都没用到 —— 所以那些代码一行都不要。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一个模型名（模型组）是哪种模型。
///
/// 为什么要它有：同一个网关后面挂着向量模型（bge-m3，占现网 86% 的量）、对话模型和重排
/// 模型，它们的**用法完全不同** —— 向量模型不看图、不发 SSE、不吃 `tools`；重排要的是
/// `query`+`documents`。面板上不分类型就得靠人记名字；探活也必须知道该打哪个端点
/// （拿对话请求去探向量组必然 400）。所以类型是**模型组自己的属性**，写进库。
///
/// **只认实测，不许按名字猜**：名字里带 `embed` 不代表它就是向量端点（现网那批自定义
/// 模型名一个特征都没有）。所以默认是「还没定」，第一次真收到请求时按走的端点自动记上；
/// 记错了人在面板上改一次，之后就不再用实测覆盖（见 [`ModelKind::is_set`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    /// 还没定：还没人来打过，或者配置里选了「跟着实测走」。
    #[default]
    Unknown,
    Chat,
    Embeddings,
    Rerank,
    /// 生成图片的模型。网关暂时不转发这类端点，所以只能人在面板上标 —— 学不出来。
    Image,
    /// 生成视频的模型（同上）。
    Video,
    /// 语音合成 / 转写的模型（同上）。
    Audio,
}

impl ModelKind {
    /// 库里/接口里用的字面值（`""` / `chat` / `embeddings` / `rerank`）。
    pub fn as_str(self) -> &'static str {
        match self {
            ModelKind::Unknown => "",
            ModelKind::Chat => "chat",
            ModelKind::Embeddings => "embeddings",
            ModelKind::Rerank => "rerank",
            ModelKind::Image => "image",
            ModelKind::Video => "video",
            ModelKind::Audio => "audio",
        }
    }

    /// 认字面值。**认不出来一律当「还没定」**，绝不猜成对话（猜错会让向量组收不到探活）。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "chat" | "completion" | "completions" => ModelKind::Chat,
            "embedding" | "embeddings" | "vector" => ModelKind::Embeddings,
            "rerank" | "reranker" => ModelKind::Rerank,
            "image" | "images" | "image_generation" => ModelKind::Image,
            "video" | "videos" | "video_generation" => ModelKind::Video,
            "audio" | "speech" | "tts" | "transcription" | "audio_speech" => ModelKind::Audio,
            _ => ModelKind::Unknown,
        }
    }

    /// 已经定下来了（不是「还没定」）。
    pub fn is_set(self) -> bool {
        self != ModelKind::Unknown
    }

    /// 面板上给人看的中文名。
    pub fn label(self) -> &'static str {
        match self {
            ModelKind::Unknown => "还没定",
            ModelKind::Chat => "对话",
            ModelKind::Embeddings => "向量",
            ModelKind::Rerank => "重排",
            ModelKind::Image => "图像",
            ModelKind::Video => "视频",
            ModelKind::Audio => "音频",
        }
    }
}

/// 类型是谁定的：实测学到的还是人手动选的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KindSource {
    /// 还没定（多半是因为还没有流量）。
    #[default]
    Unknown,
    /// 第一次真收到请求时按端点自动记的。
    Auto,
    /// 人在面板上选的 —— 从此**不再被实测覆盖**。
    Manual,
}

impl KindSource {
    pub fn as_str(self) -> &'static str {
        match self {
            KindSource::Unknown => "",
            KindSource::Auto => "auto",
            KindSource::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => KindSource::Auto,
            "manual" => KindSource::Manual,
            _ => KindSource::Unknown,
        }
    }
}

/// 渠道的数据库主键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChannelId(pub i64);

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// 一条上游通道。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    /// 面板显示名，如「硅基流动」「opencode-go」。
    pub name: String,
    /// 上游 base_url，如 `https://api.siliconflow.cn/v1`。
    pub base_url: String,
    pub enabled: bool,
    /// 同一模型组内的排序，小的先用。
    pub priority: i32,
    /// 该渠道认的上游模型名。为空表示把请求里的模型名原样透传。
    /// 例：客户端打 `deepseek-v4.1-flash`，command-go 这条渠道要的是
    /// `deepseek/deepseek-v4.1-flash` —— 只能按渠道改名，全局别名表做不到。
    #[serde(default)]
    pub model_alias: Option<String>,
    /// **这个模型名在这条渠道上能不能吃图片输入**（面板上勾「能看图」）。
    ///
    /// 能力是「上游模型 + 这条渠道」的属性，不是全局属性：同一个 `auto` 在 A 渠道上
    /// 落到能看图的模型、在 B 渠道上落到纯文本模型，所以标记挂在按渠道模型名那一行上。
    /// 没标 = 不敢保证，带图的请求不会发给它（宁可 422 说清楚，也不把图悄悄丢掉
    /// 或让上游回一个含糊的 400）。
    #[serde(default)]
    pub vision: bool,
    /// 这个模型名发过去之前要**丢掉哪些请求体顶层字段**（面板上填「要丢的字段」）。
    ///
    /// 有的上游收下参数却直接回 400：opencode 的 `beam-free` 不认 `reasoning_effort`
    /// （Hermes 默认发 high），实测 4 天里 28 次请求就这么整条失败 —— 现在只能靠外挂
    /// python 中转替它剥掉。**挂在「按渠道模型名」这一行上**，因为同一个上游对不同
    /// 模型认的参数不一样，挂渠道上会把别的模型一起改坏。
    #[serde(default)]
    pub drop_fields: Vec<String>,
    /// 这条渠道「同时最多几发」（面板上填「同时最多几发」）。**0 = 不限**。
    ///
    /// 上游免费额度多数是按凭据限并发的：同一把 key 上同时压几十发，上游直接回
    /// 429/503（现网 4 天 503「bridge busy」151 次、502「上游不可用」168 次就是这类）。
    /// 由 `db::groups::resolve_channels` 从渠道行带上来，网关按它在门口排队/换渠道。
    #[serde(default)]
    pub max_concurrent: u32,
    /// **非流式响应外面套了一层壳时自动剥掉**（面板上勾「响应解包」）。
    ///
    /// 有的上游不直接回 OpenAI 形状，而是包一层：`{"data": {<标准响应>}, "success": true}`
    /// （cline.bot 的 ClinePass 就是这样）。不剥的话客户端读顶层的 `choices` 只能读到空。
    /// 只在「`data` 是对象、且它里面有 `choices`」时才剥，别的一律原样透传 —— 不猜。
    #[serde(default)]
    pub unwrap_envelope: bool,
    /// 自定义请求头。上游 FreeLLMAPI 没有这个能力，本项目主动加上。
    /// 值支持模板：`{{uuid}}`、`{{timestamp_ms}}`，逐请求求值。
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

impl Channel {
    /// 这条渠道实际该发给上游的模型名。
    pub fn upstream_model<'a>(&'a self, requested: &'a str) -> &'a str {
        self.model_alias.as_deref().unwrap_or(requested)
    }

    /// 该渠道是否可用参与路由。
    pub fn routable(&self) -> bool {
        self.enabled && !self.base_url.trim().is_empty()
    }
}

/// 一个对外可见的模型名，对应一张**有序**的渠道列表（顺序即优先级）。
///
/// `auto` 是虚拟模型名：客户端打 `auto`，网关按这个列表的顺序挑渠道。
/// 具体模型名（如 `deepseek-v4.1-flash`）走它自己的列表，不会掉到 `auto` 去。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelGroup {
    pub name: String,
    /// 已按优先级排好序的渠道。
    pub channels: Vec<ChannelId>,
    pub enabled: bool,
}

impl ModelGroup {
    /// 首选渠道 = 优先级最小的那条。
    pub fn primary(&self) -> Option<ChannelId> {
        self.channels.first().copied()
    }
}

/// 自定义请求头里的模板替换。
///
/// 有的上游要求逐请求变化的头（OpenCode Zen 的 `x-opencode-session` 要新 UUID），
/// 写死的头做不到 —— 上游 FreeLLMAPI 当年就是因为这个（把 extraHeaders 从
/// 「可传函数」退回「只能传死值」）彻底放弃了 OpenCode 那条路。
///
/// 现在支持三个模板：
///
/// - `{{uuid}}`：每次尝试一个新 UUID（要「逐请求都不一样」的头用它）；
/// - `{{timestamp_ms}}`：当前毫秒时间戳；
/// - `{{session_id}}`：**同一段对话稳定不变**（要「同一段对话保持一致」的头用它，
///   见 [`session_id_from_body`]；探活/拉清单这类没有请求体的场合会渲染成空，
///   空的头不发出去）。
pub fn render_header_value(tpl: &str, uuid: &str, now_ms: i64, session: Option<&str>) -> String {
    tpl.replace("{{uuid}}", uuid)
        .replace("{{timestamp_ms}}", &now_ms.to_string())
        // 没有会话的场合（探活、拉模型清单）替换成空串，由 build_headers 把「渲染后为空」
        // 的头整条丢掉：宁可这个头不发出去，也不要发一个空的 x-opencode-session 去上游碰运气。
        .replace("{{session_id}}", session.unwrap_or(""))
}

/// 从请求体里算一个**同一段对话稳定不变**的会话 ID（32 位十六进制）。
///
/// 为什么要它：OpenCode Zen 这类上游靠 `x-opencode-session` 认「这还是同一段对话」——
/// 同一个 ID 的后续请求能命中上游的提示缓存；用 `{{uuid}}` 每轮都换新 ID，等于每轮都
/// 让上游从头算一遍，又慢又贵。现网那个 python 中转（`ocgo-relay`）就是干这个的，
/// 这里是它的等价物，好让那条中转可以退役。
///
/// 取法：**首条 user 消息的正文** → 哈希取前 32 位。同段对话的第一条 user 消息不变，
/// 所以 ID 稳定；换一段对话必然不同。三种形状都认：
///
/// - `content` 是字符串（最常见）；
/// - `content` 是数组（多模态）→ 只取其中的文字段，图片那一段不参与（base64 又长又变）；
/// - 一条 user 消息都没有（工具调用接力之类）→ 退化成**整个请求体的哈希**，
///   仍然稳定、仍然不随机。
///
/// 只有连退化都做不到（空请求体）才返回 `None`，交给调用方决定怎么兜。
///
/// 输入一律先截断再哈希：上万个 token 的大 prompt 不该为了算个 ID 被完整哈希一遍。
pub fn session_id_from_body(body: &Value) -> Option<String> {
    /// 只取头一段，够区分对话就行；也挡住超大 prompt / base64 图的哈希开销。
    const MAX_SEED: usize = 4096;

    let seed: Option<String> = body
        .get("messages")
        .and_then(|m| m.as_array())
        .and_then(|msgs| {
            msgs.iter()
                .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                .map(|m| m.get("content"))
        })
        .and_then(|content| match content {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(Value::Array(parts)) => {
                let text: String = parts
                    .iter()
                    .filter_map(|p| {
                        matches!(
                            p.get("type").and_then(|t| t.as_str()),
                            Some("text") | Some("input_text") | None
                        )
                        .then(|| p.get("text").and_then(|t| t.as_str()))
                        .flatten()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (!text.is_empty()).then_some(text)
            }
            _ => None,
        });

    let seed = match seed {
        Some(s) => s,
        // 退化：整个请求体的稳定序列化（serde_json 的键序由数据决定，同一个 Value 每次一样）
        None => serde_json::to_string(body).ok().filter(|s| s != "null" && s != "{}")?,
    };
    let seed: String = seed.chars().take(MAX_SEED).collect();
    Some(short_hash(&seed))
}

/// 32 位十六进制的短哈希（sha256 前 16 字节）。算会话 ID 与请求指纹都用它。
pub fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let d = h.finalize();
    d[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// 这个请求体里有没有图片（客户端是不是在让模型看图）。
///
/// 上游 FreeLLMAPI 用的是同一套判据：`messages[].content` 是数组时，看有没有
/// `type == "image_url"` 的元素（见其 `lib/inbound-chat.ts` 的 `hasImages`）。
/// 这里把新老形状都认下来：
///
/// - `{"type":"image_url","image_url":{"url":"data:image/png;base64,…"}}`（OpenAI 老形状）
/// - `{"type":"image"}`（部分上游/客户端的写法）
/// - `{"type":"input_image"}`（OpenAI Responses 形状，客户端有时直接复用）
///
/// **只认数组形状**：`content` 是字符串就是纯文本，不算带图。宁可漏判成"没图"
/// （最坏情况是上游回 400，用户看得见），也不要误判成"有图"把普通请求拦下来。
pub fn request_has_image(body: &serde_json::Value) -> bool {
    let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) else {
        return false;
    };
    msgs.iter().any(|m| {
        m.get("content")
            .and_then(|c| c.as_array())
            .map(|parts| {
                parts.iter().any(|p| {
                    matches!(
                        p.get("type").and_then(|t| t.as_str()),
                        Some("image_url") | Some("image") | Some("input_image")
                    )
                })
            })
            .unwrap_or(false)
    })
}

/// 发上游之前把这条渠道/这个模型不认的顶层字段丢掉。返回新体（**不改原体**）。
///
/// 为什么要有它：有的上游是「收下参数再回 400」—— opencode 的 `beam-free` 不认
/// `reasoning_effort`，Hermes 默认发 high，于是整条请求必然失败（现网实测 4 天 28 次）。
/// 上游 FreeLLMAPI 没这个能力，只能在外挂 python 中转里写死 `if 模型 == beam-free
/// 就删掉这个字段`；这里改成按「渠道 + 模型名」配一份清单，代码不认模型名。
///
/// 只在顶层删（`messages` 里的字段不碰）；字段名逐字符精确匹配，大小写敏感 ——
/// 宁可少删（上游报错看得见）也不要凭大小写猜着删（静默改坏请求体最难查）。
pub fn strip_fields(body: &Value, fields: &[String]) -> Value {
    let mut v = body.clone();
    if fields.is_empty() {
        return v;
    }
    if let Some(obj) = v.as_object_mut() {
        for name in fields {
            let name = name.trim();
            if !name.is_empty() {
                obj.remove(name);
            }
        }
    }
    v
}

/// 非流式响应外面套了一层壳时，把它剥掉。剥到了返回 true。
///
/// 认得的形状只有一种（cline.bot 的 ClinePass）：
///
/// ```json
/// {"data": {"choices": [...], "usage": {...}}, "success": true}
/// ```
///
/// **只在 `data` 是对象且里面有 `choices` 时才剥**，别的一律原样透传 —— 这跟面板上
/// 「响应解包」那个勾选是配套的：用户明确知道这条上游是这种形状才勾，网关不猜。
/// 剥的时候把外壳的 `success`（如果有）留在里层，客户端原来能读到的字段不会丢。
pub fn unwrap_envelope(body: &mut Value) -> bool {
    let Some(obj) = body.as_object_mut() else {
        return false;
    };
    let Some(inner) = obj.get("data").and_then(|d| d.as_object()) else {
        return false;
    };
    if !inner.contains_key("choices") {
        return false;
    }
    let mut inner = inner.clone();
    if let Some(success) = obj.get("success").filter(|s| s.is_boolean()) {
        inner.entry("success".to_string()).or_insert(success.clone());
    }
    *body = Value::Object(inner);
    true
}

/// 上游回了 200，但**一句话都没说**：既没有正文，也没有工具调用，也没有推理内容。
///
/// 为什么要认出它：现网 4 天里有 9 次这种回复（cline 那条渠道自己都说约 3% 的上游 500
/// 是「empty response content」），客户端拿到的是一个空泡泡 —— 用户以为模型发呆，
/// 其实换一条渠道重发就好了。判据对齐现网老网关的说法（`stream produced no content and
/// no tool calls`），另加 `reasoning_content`：有些推理模型把答案全放在推理字段里、
/// content 是空的，那**不是**空回复（判错了会把好好的回答重发一遍）。
///
/// **只认「有 message/delta 但里头是空的」**：choices 缺失、choices 是空的、
/// 连 message 都没有，都算「形状不认识」→ 返回 false，不猜（宁可漏判也不误判）。
pub fn empty_chat_reply(v: &Value) -> bool {
    let Some(choices) = v.get("choices").and_then(|c| c.as_array()) else {
        return false;
    };
    if choices.is_empty() {
        return false;
    }
    let mut saw_a_message = false;
    for c in choices {
        let Some(m) = c.get("message").or_else(|| c.get("delta")) else {
            continue;
        };
        saw_a_message = true;
        let has_text = |k: &str| {
            m.get(k)
                .and_then(|x| x.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
        };
        let has_tools = m
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
            || m.get("function_call").map(|f| !f.is_null()).unwrap_or(false);
        if has_text("content") || has_text("reasoning_content") || has_tools {
            return false;
        }
    }
    saw_a_message
}

/// 这一块流式数据里有没有**真内容**（正文 / 工具调用 / 推理内容）。
///
/// 流式不能像非流式那样"读完整段再判断空不空"（第一个字节早就发给客户端了），
/// 所以边转发边记一笔：整条流从头到尾一块真内容都没有 → 记成失败并说明原因，
/// 用户至少能在流水里看见「上游一个字都没说」，而不是对着空泡泡猜。
pub fn chunk_has_payload(v: &Value) -> bool {
    let Some(choices) = v.get("choices").and_then(|c| c.as_array()) else {
        return false;
    };
    choices.iter().any(|c| {
        let m = c.get("delta").or_else(|| c.get("message"));
        let Some(m) = m else { return false };
        let text = |k: &str| {
            m.get(k)
                .and_then(|x| x.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
        };
        text("content")
            || text("reasoning_content")
            || m.get("tool_calls")
                .and_then(|t| t.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: i64, alias: Option<&str>) -> Channel {
        Channel {
            id: ChannelId(id),
 max_concurrent: 0,
            name: format!("channel{id}"),
            base_url: "http://127.0.0.1:1/v1".into(),
            enabled: true,
            priority: id as i32,
            model_alias: alias.map(str::to_owned),
            vision: false,
            drop_fields: vec![],
            unwrap_envelope: false,
            headers: vec![],
        }
    }

    #[test]
    fn per_channel_model_alias_wins_over_requested_name() {
        let l = channel(1, Some("deepseek/deepseek-v4.1-flash"));
        assert_eq!(l.upstream_model("auto"), "deepseek/deepseek-v4.1-flash");
    }

    #[test]
    fn channel_without_alias_passes_the_name_through() {
        let l = channel(1, None);
        assert_eq!(l.upstream_model("bge-m3"), "bge-m3");
    }

    #[test]
    fn disabled_or_urlless_channel_is_not_routable() {
        let mut l = channel(1, None);
        assert!(l.routable());
        l.enabled = false;
        assert!(!l.routable());
        l.enabled = true;
        l.base_url = "   ".into();
        assert!(!l.routable());
    }

    #[test]
    fn primary_is_the_first_by_priority() {
        let g = ModelGroup {
            name: "auto".into(),
            channels: vec![ChannelId(44), ChannelId(47), ChannelId(51)],
            enabled: true,
        };
        assert_eq!(g.primary(), Some(ChannelId(44)));
    }

    #[test]
    fn header_templates_are_rendered_per_request() {
        let a = render_header_value("sess-{{uuid}}", "AAA", 1, None);
        let b = render_header_value("sess-{{uuid}}", "BBB", 1, None);
        assert_ne!(a, b, "逐请求必须生成不同的值");
        assert_eq!(render_header_value("t={{timestamp_ms}}", "x", 1234, None), "t=1234");
    }

    #[test]
    fn the_session_template_uses_the_stable_id() {
        assert_eq!(
            render_header_value("{{session_id}}", "u", 1, Some("deadbeef")),
            "deadbeef"
        );
        assert_eq!(
            render_header_value("s-{{session_id}}", "u", 1, None),
            "s-",
            "没有会话的场合（探活/拉清单）渲染成空，由 build_headers 整条丢掉"
        );
    }

    /// 会话 ID 的全部意义就在这两条：同段对话稳定、换段对话必须变。
    #[test]
    fn the_session_id_is_stable_for_one_conversation_and_changes_for_another() {
        let body = |text: &str| {
            serde_json::json!({
                "model": "auto",
                "messages": [
                    {"role": "system", "content": "你是助手"},
                    {"role": "user", "content": text},
                    {"role": "assistant", "content": "好的"},
                ]
            })
        };
        let a = session_id_from_body(&body("帮我看下这段代码")).unwrap();
        // 同一段对话的下一轮：第一条 user 消息没变，后面接了更多轮
        let mut later = body("帮我看下这段代码");
        later["messages"].as_array_mut().unwrap().extend([
            serde_json::json!({"role": "user", "content": "再改一下"}),
        ]);
        assert_eq!(a, session_id_from_body(&later).unwrap(), "同段对话必须同一个 ID");
        assert_eq!(a.len(), 32);
        // 换一段对话必须换 ID（不然两段对话会共用上游的提示缓存）
        assert_ne!(a, session_id_from_body(&body("帮我看下这段日志")).unwrap());
    }

    /// 多模态：只认文字段（图片 base64 又长又变，参与哈希会让 ID 每次都变）。
    #[test]
    fn a_multimodal_first_message_hashes_only_its_text_parts() {
        let body = |url: &str| {
            serde_json::json!({
                "messages": [{"role": "user", "content": [
                    {"type": "text", "text": "这张图里是什么"},
                    {"type": "image_url", "image_url": {"url": url}}
                ]}]
            })
        };
        let a = session_id_from_body(&body("data:image/png;base64,AAAA")).unwrap();
        let b = session_id_from_body(&body("data:image/png;base64,BBBB")).unwrap();
        assert_eq!(a, b, "换一张图不该换会话 ID（图内容不进哈希）");
        // 文字变了就一定要变
        let mut other = body("data:image/png;base64,AAAA");
        other["messages"][0]["content"][0]["text"] = serde_json::json!("换个问题");
        assert_ne!(a, session_id_from_body(&other).unwrap());
    }

    /// 没有 user 消息（工具调用接力）→ 退化成整包哈希，**不随机**。
    #[test]
    fn a_body_without_a_user_message_falls_back_to_the_whole_body_and_stays_stable() {
        let body = serde_json::json!({
            "messages": [{"role": "assistant", "tool_calls": [{"id": "1"}]}],
            "tools": [{"type": "function"}]
        });
        let a = session_id_from_body(&body).unwrap();
        assert_eq!(a, session_id_from_body(&body).unwrap(), "退化路径也必须是稳定的");
        assert_ne!(
            a,
            session_id_from_body(&serde_json::json!({
                "messages": [{"role": "assistant", "tool_calls": [{"id": "2"}]}]
            }))
            .unwrap()
        );
        // 连退化都做不到才返回 None（调用方自己兜一个随机的）
        assert!(session_id_from_body(&serde_json::json!({})).is_none());
        assert!(session_id_from_body(&serde_json::Value::Null).is_none());
    }

    /// 大 prompt 不该为了算 ID 被整段哈希（现网那条 python 中转就是在这上面吃满内存的）。
    #[test]
    fn a_huge_prompt_is_truncated_before_hashing() {
        let long = "字".repeat(200_000);
        let mut body = serde_json::json!({
            "messages": [{"role": "user", "content": long.clone()}]
        });
        let a = session_id_from_body(&body).unwrap();
        // 只改第 100 万个字符之后的内容：ID 不变（说明哈希只看了开头那段）
        body["messages"][0]["content"] = serde_json::json!(format!("{long}{}", "尾".repeat(50_000)));
        assert_eq!(a, session_id_from_body(&body).unwrap());
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn stream_chunks_are_judged_for_real_payload() {
        assert!(chunk_has_payload(&serde_json::json!({
            "choices": [{"delta": {"content": "你"}}]
        })));
        assert!(chunk_has_payload(&serde_json::json!({
            "choices": [{"delta": {"tool_calls": [{"index": 0}]}}]
        })));
        assert!(chunk_has_payload(&serde_json::json!({
            "choices": [{"delta": {"reasoning_content": "想…"}}]
        })));
        // 只有角色、只有 usage、空 choices —— 都不算真内容
        assert!(!chunk_has_payload(&serde_json::json!({"choices": [{"delta": {"role": "assistant"}}]})));
        assert!(!chunk_has_payload(&serde_json::json!({"usage": {"prompt_tokens": 1}})));
        assert!(!chunk_has_payload(&serde_json::json!({"choices": [{"delta": {"content": "  "}}]})));
        assert!(!chunk_has_payload(&serde_json::json!({})));
    }

    #[test]
    fn an_empty_reply_is_recognised_without_misfiring_on_real_answers() {
        // 真·空回复：message 在、里头什么都没有
        assert!(empty_chat_reply(&serde_json::json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": ""}, "finish_reason": "stop"}]
        })));
        assert!(empty_chat_reply(&serde_json::json!({
            "choices": [{"delta": {"role": "assistant", "content": null}}]
        })));
        assert!(empty_chat_reply(&serde_json::json!({
            "choices": [{"message": {"role": "assistant"}}]
        })));

        // 反证：有内容 / 有工具调用 / 有推理内容 / 只放了图 —— 都不算空
        assert!(!empty_chat_reply(&serde_json::json!({
            "choices": [{"message": {"content": "你好"}}]
        })));
        assert!(!empty_chat_reply(&serde_json::json!({
            "choices": [{"message": {"content": "", "tool_calls": [{"id": "1"}]}}]
        })));
        assert!(!empty_chat_reply(&serde_json::json!({
            "choices": [{"message": {"content": "", "reasoning_content": "想一下…"}}]
        })), "推理内容也是话，不许当空回复重发");
        assert!(!empty_chat_reply(&serde_json::json!({
            "choices": [{"message": {"content": "", "function_call": {"name": "f"}}}]
        })));
        // 反证：形状不认识的一律不当空回复（宁可漏判）
        assert!(!empty_chat_reply(&serde_json::json!({})));
        assert!(!empty_chat_reply(&serde_json::json!({"choices": []})));
        assert!(!empty_chat_reply(&serde_json::json!({"choices": [{"index": 0}]})));
        assert!(!empty_chat_reply(&serde_json::json!({"data": {"embedding": [0.1]}})));
    }

    #[test]
    fn model_kind_round_trips_and_never_guesses() {
        for k in [
            ModelKind::Unknown,
            ModelKind::Chat,
            ModelKind::Embeddings,
            ModelKind::Rerank,
        ] {
            assert_eq!(ModelKind::parse(k.as_str()), k, "{k:?} 存下去读不回来");
        }
        // 认字面值要宽容：大小写、几个常见别名
        assert_eq!(ModelKind::parse("Embeddings"), ModelKind::Embeddings);
        assert_eq!(ModelKind::parse(" vector "), ModelKind::Embeddings);
        assert_eq!(ModelKind::parse("reranker"), ModelKind::Rerank);
        // **反证：认不出来一律「还没定」，绝不默认成对话**（猜错 = 向量组收不到探活）
        assert_eq!(ModelKind::parse(""), ModelKind::Unknown);
        assert_eq!(ModelKind::parse("bge-m3"), ModelKind::Unknown);
        assert_eq!(ModelKind::parse("随便什么"), ModelKind::Unknown);
        assert!(!ModelKind::Unknown.is_set());
        assert!(ModelKind::Embeddings.is_set());
        assert_eq!(ModelKind::Rerank.label(), "重排");
    }

    #[test]
    fn kind_source_says_who_decided() {
        assert_eq!(KindSource::parse(KindSource::Manual.as_str()), KindSource::Manual);
        assert_eq!(KindSource::parse("AUTO"), KindSource::Auto);
        assert_eq!(KindSource::parse("没这个值"), KindSource::Unknown);
        assert_eq!(KindSource::Unknown.as_str(), "");
    }

    #[test]
    fn drop_fields_removes_only_the_listed_top_level_fields() {
        let body = serde_json::json!({
            "model": "beam-free",
            "reasoning_effort": "high",
            "messages": [{"role": "user", "content": "hi", "extra": 1}],
            "stream": false
        });
        let out = strip_fields(&body, &["reasoning_effort".to_string()]);
        assert!(out.get("reasoning_effort").is_none());
        // 别的字段一个都不能动（尤其 messages 里面同名的字段）
        assert_eq!(out["model"], "beam-free");
        assert_eq!(out["stream"], false);
        assert_eq!(out["messages"][0]["extra"], 1);
        // 原体不被改（调用方还要拿它记别的账）
        assert_eq!(body["reasoning_effort"], "high");
        // 空白项、项外的字段名都不许误伤
        let out = strip_fields(&body, &["  ".to_string(), "Reasoning_Effort".to_string()]);
        assert_eq!(out["reasoning_effort"], "high", "大小写不匹配就不许删");
        // 空清单 = 原样
        assert_eq!(strip_fields(&body, &[]), body);
    }

    #[test]
    fn an_enveloped_response_is_unwrapped_only_when_it_really_is_one() {
        // cline.bot 的真形状
        let mut v = serde_json::json!({
            "data": {"choices": [{"message": {"content": "你好"}}], "usage": {"prompt_tokens": 3}},
            "success": true
        });
        assert!(unwrap_envelope(&mut v));
        assert_eq!(v["choices"][0]["message"]["content"], "你好");
        assert_eq!(v["usage"]["prompt_tokens"], 3);
        assert_eq!(v["success"], true, "外壳上的 success 要留在里层");

        // 反证：普通 OpenAI 响应（顶层就有 choices）一个字节都不该动
        let mut v = serde_json::json!({"choices": [{"message": {"content": "x"}}], "usage": {}});
        let before = v.clone();
        assert!(!unwrap_envelope(&mut v));
        assert_eq!(v, before);

        // 反证：data 是数组 / data 里没有 choices / 不是对象 —— 都不猜
        for mut v in [
            serde_json::json!({"data": [1, 2, 3], "success": true}),
            serde_json::json!({"data": {"result": "ok"}, "success": true}),
            serde_json::json!({"data": "字符串", "success": true}),
            serde_json::json!({"success": true}),
        ] {
            let before = v.clone();
            assert!(!unwrap_envelope(&mut v), "不该剥的形状被剥了：{before}");
            assert_eq!(v, before);
        }
    }

    #[test]
    fn an_image_part_marks_the_request_as_vision() {
        let body = serde_json::json!({
            "model": "auto",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "这是什么"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}}
            ]}]
        });
        assert!(request_has_image(&body), "带图请求必须被认出来");
    }

    #[test]
    fn plain_string_content_is_not_vision() {
        let body = serde_json::json!({
            "model": "auto",
            "messages": [{"role": "user", "content": "就是一句话"}]
        });
        assert!(!request_has_image(&body));
    }

    #[test]
    fn newer_image_part_names_count_too() {
        for t in ["image", "input_image"] {
            let body = serde_json::json!({
                "messages": [{"role": "user", "content": [{"type": t}]}]
            });
            assert!(request_has_image(&body), "{t} 也要认");
        }
    }

    /// 反证：没有 messages、messages 是别的形状、文字+工具调用混排，都不算带图
    /// —— 误判成"有图"会把普通请求拦下来（比漏判更糟）。
    #[test]
    fn non_vision_shapes_are_never_mistaken_for_images() {
        assert!(!request_has_image(&serde_json::json!({})));
        assert!(!request_has_image(&serde_json::json!({"messages": "nope"})));
        assert!(!request_has_image(&serde_json::json!({"messages": []})));
        assert!(!request_has_image(&serde_json::json!({
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "看图这件事我不会"},
                {"type": "tool_result", "content": "{}"}
            ]}]
        })));
    }
}
