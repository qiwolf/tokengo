//! 腿（上游通道）与模型组。
//!
//! **本项目不做免费模型目录，也不做 provider 适配器。** 上游全部由用户自定义添加：
//! 一条腿 = 名称 + base_url + key + 自定义请求头 + 该腿认的上游模型名。
//!
//! 上游 FreeLLMAPI 有 37 个内置 provider 适配器和 72 个平台枚举，但本机现网 9 条腿
//! **全部**是 `platform=custom`，一个内置适配器都没用到 —— 所以那些代码一行都不要。

use serde::{Deserialize, Serialize};

/// 腿的数据库主键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LegId(pub i64);

impl std::fmt::Display for LegId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// 一条上游通道。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Leg {
    pub id: LegId,
    /// 面板显示名，如「硅基流动」「opencode-go」。
    pub name: String,
    /// 上游 base_url，如 `https://api.siliconflow.cn/v1`。
    pub base_url: String,
    pub enabled: bool,
    /// 同一模型组内的排序，小的先用。
    pub priority: i32,
    /// 该腿认的上游模型名。为空表示把请求里的模型名原样透传。
    /// 例：客户端打 `deepseek-v4.1-flash`，command-go 这条腿要的是
    /// `deepseek/deepseek-v4.1-flash` —— 只能按腿改名，全局别名表做不到。
    #[serde(default)]
    pub model_alias: Option<String>,
    /// 自定义请求头。上游 FreeLLMAPI 没有这个能力，本项目主动加上。
    /// 值支持模板：`{{uuid}}`、`{{timestamp_ms}}`，逐请求求值。
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

impl Leg {
    /// 这条腿实际该发给上游的模型名。
    pub fn upstream_model<'a>(&'a self, requested: &'a str) -> &'a str {
        self.model_alias.as_deref().unwrap_or(requested)
    }

    /// 该腿是否可用参与路由。
    pub fn routable(&self) -> bool {
        self.enabled && !self.base_url.trim().is_empty()
    }
}

/// 一个对外可见的模型名，对应一张**有序**的腿列表（顺序即优先级）。
///
/// `auto` 是虚拟模型名：客户端打 `auto`，网关按这个列表的顺序挑腿。
/// 具体模型名（如 `deepseek-v4.1-flash`）走它自己的列表，不会掉到 `auto` 去。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelGroup {
    pub name: String,
    /// 已按优先级排好序的腿。
    pub legs: Vec<LegId>,
    pub enabled: bool,
}

impl ModelGroup {
    /// 首选腿 = 优先级最小的那条。
    pub fn primary(&self) -> Option<LegId> {
        self.legs.first().copied()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(id: i64, alias: Option<&str>) -> Leg {
        Leg {
            id: LegId(id),
            name: format!("leg{id}"),
            base_url: "http://127.0.0.1:1/v1".into(),
            enabled: true,
            priority: id as i32,
            model_alias: alias.map(str::to_owned),
            headers: vec![],
        }
    }

    #[test]
    fn per_leg_model_alias_wins_over_requested_name() {
        let l = leg(1, Some("deepseek/deepseek-v4.1-flash"));
        assert_eq!(l.upstream_model("auto"), "deepseek/deepseek-v4.1-flash");
    }

    #[test]
    fn leg_without_alias_passes_the_name_through() {
        let l = leg(1, None);
        assert_eq!(l.upstream_model("bge-m3"), "bge-m3");
    }

    #[test]
    fn disabled_or_urlless_leg_is_not_routable() {
        let mut l = leg(1, None);
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
            legs: vec![LegId(44), LegId(47), LegId(51)],
            enabled: true,
        };
        assert_eq!(g.primary(), Some(LegId(44)));
    }

    #[test]
    fn header_templates_are_rendered_per_request() {
        let a = render_header_value("sess-{{uuid}}", "AAA", 1);
        let b = render_header_value("sess-{{uuid}}", "BBB", 1);
        assert_ne!(a, b, "逐请求必须生成不同的值");
        assert_eq!(render_header_value("t={{timestamp_ms}}", "x", 1234), "t=1234");
    }
}
