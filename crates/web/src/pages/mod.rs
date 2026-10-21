//! 页面清单与页面之间的公共约定。
//!
//! 面板：总览 · 渠道 · 模型 · 用户 · 账务 · 设置。
//!
//! 「请求」那一页 2026-10-18 撤掉了：它列的东西「总览」里都有（用户口径：内容重复）。
//!
//! 「日志」是给排障用的（用户 2026-10-18 要求）：出问题时不用 SSH，面板里就能看现场。
//! 原先「模型组」和「路由状态」是分开的两页、还各有一套数据；v0.11.0 起没有「模型组」
//! 这一层了（一个模型名 = 渠道里勾了它的那些渠道），面板只剩「模型」一页看它。

pub mod channels;
pub mod custom_probes;
pub mod provider_channels;
pub mod provider_keys;
pub mod model_reasoning;
pub mod accounts;
pub mod policy_editor;
pub mod key_manager;
pub mod virtual_models;
pub mod portal;
pub mod users;
pub mod billing;
pub mod models;
pub mod route_policy;
pub mod logs;
pub mod overview;
pub mod settings;
pub mod operations_tokens;

use leptos::prelude::*;

/// 面板的页面。**顺序就是顶栏顺序。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Channels,
    Models,
    Probes,
    Users,
    Billing,
    Settings,
    Accounts,
    Portal,
    Strategies,
    Account,
}

impl Page {
    pub const ALL: &'static [Page] = &[
        Page::Overview,
        Page::Channels,
        Page::Models,
        Page::Probes,
        Page::Users,
        Page::Billing,
        Page::Settings,
    ];

    /// UI mounting boundary mirrors the server's distinct account roles.
    pub fn for_account(self, root:bool)->Page {
        if root {if self==Page::Portal {Page::Overview}else{self}}
        else if matches!(self,Page::Account|Page::Portal|Page::Strategies){self}else{Page::Portal}
    }

    /// URL hash 里用的名字。
    pub fn slug(self) -> &'static str {
        match self {
            Page::Overview => "overview",
            Page::Channels => "channels",
            Page::Models => "models",
            Page::Probes => "probes",
            Page::Users => "users",
            Page::Billing => "billing",
            Page::Settings => "settings",
            Page::Strategies=>"strategies",Page::Accounts=>"accounts",Page::Portal=>"portal",Page::Account=>"account",
        }
    }

    /// 顶栏与菜单上显示的名字。**两个字的短名，手机上排得下。**
    pub fn title(self) -> &'static str {
        match self {
            Page::Overview => "总览",
            Page::Channels => "渠道",
            Page::Models => "模型",
            Page::Probes => "探针",
            Page::Users => "用户",
            Page::Billing => "账务",
            Page::Settings => "设置",
            Page::Strategies=>"调用策略",Page::Accounts=>"用户",Page::Portal=>"我的空间",Page::Account=>"账号安全",
        }
    }

    /// 每个页面顶上那句话说明 —— 「这一页是干什么的」。
    pub fn blurb(self) -> &'static str {
        match self {
            Page::Overview => "最近这段时间的请求、各条渠道的表现，以及客户端能打的模型名。",
            Page::Channels => "各个上游的地址和密钥：加一条、改一条、测通不通。",
            Page::Models => "你客户端里填的模型名，按什么顺序走哪条渠道。",
            Page::Probes => "配置渠道与模型的定时检查，查看结果和响应速度。",
            Page::Users => "管理用户资料、模型授权、限额与 API Key。",
            Page::Billing => "预算、价格版本和持久化的逐次尝试账本。",
            Page::Settings => "网关密钥、运行参数与维护。",
            Page::Strategies=>"私有虚拟模型与顺序、错误码和时段调用策略。",Page::Accounts=>"登录控制台的独立账号和启用状态。",Page::Portal=>"本人密钥元数据与用量。",Page::Account=>"当前登录账号与密码。",
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
            "strategies"=>Page::Strategies,"portal"=>Page::Portal,"account"=>Page::Account,"accounts"=>Page::Users,
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

/// Focused editor/detail surface. The browser handles modal focus containment and
/// restores the launching control; Escape and the close button update the same state.
#[component]
pub fn ModuleDialog(open: RwSignal<bool>, title: String, #[prop(optional)] busy: Option<RwSignal<bool>>, children: Children) -> impl IntoView {
    use wasm_bindgen::JsCast;
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let id = format!("module-dialog-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let title_id = format!("{id}-title");
    let effect_id = id.clone();
    Effect::new(move |_| {
        let wanted = open.get();
        if let Some(dialog) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&effect_id)) {
            if wanted != dialog.has_attribute("open") {
                let method = if wanted { "showModal" } else { "close" };
                if let Ok(function) = js_sys::Reflect::get(dialog.as_ref(), &method.into()).and_then(|v| v.dyn_into::<js_sys::Function>()) {
                    let _ = function.call0(dialog.as_ref());
                }
            }
        }
    });
    view! {
        <dialog id=id class="module-dialog console-content" aria-labelledby=title_id.clone()
            on:cancel=move |ev: web_sys::Event| { ev.prevent_default(); if !busy.map(|s|s.get_untracked()).unwrap_or(false) { open.set(false); } }
            on:close=move |_| { if open.get_untracked() { open.set(false); } }>
            <header class="module-dialog-header"><h2 id=title_id.clone()>{title}</h2><button type="button" class="ghost" autofocus disabled=move ||busy.map(|s|s.get()).unwrap_or(false) on:click=move |_|open.set(false)>"关闭"</button></header>
            <div class="module-dialog-body">{children()}</div>
        </dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_accounts_cannot_mount_administrator_pages() {
        for page in Page::ALL {assert_eq!(page.for_account(false),Page::Portal);}
        assert_eq!(Page::Account.for_account(false),Page::Account);
        assert_eq!(Page::Portal.for_account(true),Page::Overview);
        assert_eq!(Page::Accounts.for_account(true),Page::Accounts);
    }

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
        // 总览 · 渠道 · 模型 · 用户 · 账务 · 设置
        assert_eq!(Page::ALL.len(), 7);
        assert!(!Page::ALL.contains(&Page::Accounts));
        assert_eq!(Page::Users.title(), "用户");
    }
}
