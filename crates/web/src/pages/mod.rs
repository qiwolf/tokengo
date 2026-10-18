//! 页面清单与页面之间的公共约定。
//!
//! 面板只做本机真正用得上的 5 页：总览 · 渠道 · 模型 · 日志 · 设置。
//!
//! 「请求」那一页 2026-10-18 撤掉了：它列的东西「总览」里都有（用户口径：内容重复）。
//!
//! 「日志」是给排障用的（用户 2026-10-18 要求）：出问题时不用 SSH，面板里就能看现场。
//! 原先「模型组」和「路由状态」是分开的两页、还各有一套数据；v0.11.0 起没有「模型组」
//! 这一层了（一个模型名 = 渠道里勾了它的那些渠道），面板只剩「模型」一页看它。

pub mod channels;
pub mod models;
pub mod logs;
pub mod overview;
pub mod settings;

use leptos::prelude::*;

/// 面板的页面。**顺序就是顶栏顺序。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Channels,
    Models,
    Settings,
}

impl Page {
    pub const ALL: &'static [Page] = &[
        Page::Overview,
        Page::Channels,
        Page::Models,
        Page::Settings,
    ];

    /// URL hash 里用的名字。
    pub fn slug(self) -> &'static str {
        match self {
            Page::Overview => "overview",
            Page::Channels => "channels",
            Page::Models => "models",
            Page::Settings => "settings",
        }
    }

    /// 顶栏与菜单上显示的名字。**两个字的短名，手机上排得下。**
    pub fn title(self) -> &'static str {
        match self {
            Page::Overview => "总览",
            Page::Channels => "渠道",
            Page::Models => "模型",
            Page::Settings => "设置",
        }
    }

    /// 每个页面顶上那句话说明 —— 「这一页是干什么的」。
    pub fn blurb(self) -> &'static str {
        match self {
            Page::Overview => "最近这段时间的请求、各条渠道的表现，以及客户端能打的模型名。",
            Page::Channels => "各个上游的地址和密钥：加一条、改一条、测通不通。",
            Page::Models => "你客户端里填的模型名，按什么顺序走哪条渠道。",
            Page::Settings => "对外 key、面板口令、登录着的设备。",
        }
    }

    /// 从 URL hash 猜当前页（`#channels` → 渠道）。认不出就回总览。
    ///
    /// 旧的 `#groups` / `#routing` 也认 —— 那两个页面早就并进了「模型」，
    /// 老书签不该变成「打开就跳回总览」。
    pub fn from_hash() -> Page {
        let raw = web_sys::window()
            .and_then(|w| w.location().hash().ok())
            .unwrap_or_default();
        let name = raw.trim_start_matches('#').trim_start_matches('/');
        let name = name.split('/').next().unwrap_or("");
        match name {
            "groups" | "routing" => Page::Models,
            "logs" => Page::Settings,
            _ => Self::ALL
                .iter()
                .copied()
                .find(|p| p.slug() == name)
                .unwrap_or(Page::Overview),
        }
    }
}

/// 每个页面顶上的那一块：大标题 + 一句话说明 + 右侧操作按钮，底下一条横线。
///
/// 抽出来是为了**五个页面长得一样** —— 到哪一页都知道「标题在哪、按钮在哪」。
#[component]
pub fn PageHead(
    /// 大标题（跟顶栏那个词一致，别换叫法）
    title: &'static str,
    /// 一句话说明这一页是干什么的
    desc: &'static str,
    /// 右侧操作区（按钮）。不传就是空的。
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="page-head">
            <div class="page-title-row">
                <h1>{title}</h1>
                {children.map(|c| view! { <div class="page-head-actions">{c()}</div> })}
            </div>
            <p class="muted small">{desc}</p>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_a_unique_slug_and_a_title() {
        // 顶栏靠 slug 跳页，重复的 slug 会让某一页永远打不开。
        let mut slugs: Vec<&str> = Page::ALL.iter().map(|p| p.slug()).collect();
        slugs.sort_unstable();
        let before = slugs.len();
        slugs.dedup();
        assert_eq!(slugs.len(), before, "slug 必须唯一");
        assert!(Page::ALL.iter().all(|p| !p.title().is_empty()));
        assert!(Page::ALL.iter().all(|p| !p.blurb().is_empty()));
        // 五页：总览 · 渠道 · 模型 · 日志 · 设置（「请求」撤了）
        assert_eq!(Page::ALL.len(), 4);
    }
}
