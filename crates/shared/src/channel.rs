//! 渠道（上游通道）与模型组。
//!
//! **本项目不做免费模型目录，也不做 provider 适配器。** 上游全部由用户自定义添加：
//! 一条渠道 = 名称 + base_url + key + 自定义请求头 + 该渠道认的上游模型名。
//!
//! 上游 FreeLLMAPI 有 37 个内置 provider 适配器和 72 个平台枚举，但本机现网 9 条渠道
//! **全部**是 `platform=custom`，一个内置适配器都没用到 —— 所以那些代码一行都不要。

use serde::{Deserialize, Serialize};

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
pub fn render_header_value(tpl: &str, uuid: &str, now_ms: i64) -> String {
    tpl.replace("{{uuid}}", uuid)
        .replace("{{timestamp_ms}}", &now_ms.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: i64, alias: Option<&str>) -> Channel {
        Channel {
            id: ChannelId(id),
            name: format!("channel{id}"),
            base_url: "http://127.0.0.1:1/v1".into(),
            enabled: true,
            priority: id as i32,
            model_alias: alias.map(str::to_owned),
            vision: false,
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
        let a = render_header_value("sess-{{uuid}}", "AAA", 1);
        let b = render_header_value("sess-{{uuid}}", "BBB", 1);
        assert_ne!(a, b, "逐请求必须生成不同的值");
        assert_eq!(render_header_value("t={{timestamp_ms}}", "x", 1234), "t=1234");
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
