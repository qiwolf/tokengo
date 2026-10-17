//! 页面清单与页面之间的公共约定。

pub mod groups;
pub mod legs;
pub mod overview;
pub mod requests;
pub mod routing;
pub mod settings;

/// 面板的页面。**顺序就是侧栏顺序。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Legs,
    Groups,
    Routing,
    Requests,
    Settings,
}

impl Page {
    pub const ALL: &'static [Page] = &[
        Page::Overview,
        Page::Legs,
        Page::Groups,
        Page::Routing,
        Page::Requests,
        Page::Settings,
    ];

    /// URL hash 里用的名字。
    pub fn slug(self) -> &'static str {
        match self {
            Page::Overview => "overview",
            Page::Legs => "legs",
            Page::Groups => "groups",
            Page::Routing => "routing",
            Page::Requests => "requests",
            Page::Settings => "settings",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Overview => "总览",
            Page::Legs => "腿管理",
            Page::Groups => "模型组",
            Page::Routing => "路由状态",
            Page::Requests => "请求流水",
            Page::Settings => "设置",
        }
    }

    /// 侧栏图标。用字符，不引图标库（少一个依赖，也少一堆字体文件）。
    pub fn icon(self) -> &'static str {
        match self {
            Page::Overview => "▤",
            Page::Legs => "⇄",
            Page::Groups => "≡",
            Page::Routing => "◈",
            Page::Requests => "☰",
            Page::Settings => "⚙",
        }
    }

    /// 从 URL hash 猜当前页（`#legs` → 腿管理）。认不出就回总览。
    pub fn from_hash() -> Page {
        let raw = web_sys::window()
            .and_then(|w| w.location().hash().ok())
            .unwrap_or_default();
        let name = raw.trim_start_matches('#').trim_start_matches('/');
        let name = name.split('/').next().unwrap_or("");
        Self::ALL
            .iter()
            .copied()
            .find(|p| p.slug() == name)
            .unwrap_or(Page::Overview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_a_unique_slug_and_a_title() {
        // 侧栏靠 slug 跳页，重复的 slug 会让某一页永远打不开。
        let mut slugs: Vec<&str> = Page::ALL.iter().map(|p| p.slug()).collect();
        slugs.sort_unstable();
        let before = slugs.len();
        slugs.dedup();
        assert_eq!(slugs.len(), before, "slug 必须唯一");
        assert!(Page::ALL.iter().all(|p| !p.title().is_empty()));
        // 计划的 5 页 + 设置页
        assert_eq!(Page::ALL.len(), 6);
    }
}
