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
pub mod passthrough;
pub mod custom_probes;
pub mod provider_channels;
pub mod provider_keys;
pub mod family_picker;
pub mod oauth_enrollment;
pub mod connection_parameters;
pub mod proxy_settings;
pub mod model_reasoning;
pub mod provider_compatibility;
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
    SharedModels,
    Services,
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
        Page::SharedModels,
        Page::Services,
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
            Page::SharedModels => "shared-models",
            Page::Services => "services",
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
            Page::Overview => "运行总览",
            Page::Channels => "供应商与接入",
            Page::Models => "模型与路由",
            Page::SharedModels => "共享虚拟模型",
            Page::Services => "透传服务",
            Page::Probes => "探针",
            Page::Users => "用户与 Key",
            Page::Billing => "用量与账务",
            Page::Settings => "系统设置",
            Page::Strategies=>"调用策略",Page::Accounts=>"用户",Page::Portal=>"我的空间",Page::Account=>"账号安全",
        }
    }

    /// 每个页面顶上那句话说明 —— 「这一页是干什么的」。
    pub fn blurb(self) -> &'static str {
        match self {
            Page::Overview => "最近这段时间的请求、各条渠道的表现，以及客户端能打的模型名。",
            Page::Channels => "各个上游的地址和密钥：加一条、改一条、测通不通。",
            Page::Models => "你客户端里填的模型名，按什么顺序走哪条渠道。",
            Page::SharedModels => "管理员维护共享调用入口，再向普通用户分配访问权限。",
            Page::Services => "管理按路径访问的透传服务，并在用户或 API Key 中分配权限。",
            Page::Probes => "配置渠道与模型的定时检查，查看结果和响应速度。",
            Page::Users => "管理用户资料、模型与透传服务授权、限额及 API Key。",
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
            <div class="page-head-text">
                <h1>{title}</h1>
                <p class="muted small">{desc}</p>
            </div>
            {children.map(|c| view! { <div class="page-head-actions">{c()}</div> })}
        </div>
    }
}

/// Explicitly cycle a native modal's own focusable controls; no positive tab order.
fn modal_tab_target(count:usize,current:Option<usize>,backward:bool)->Option<usize> {
    if count==0 {return None}
    Some(match (current,backward) {(Some(index),true)=>(index+count-1)%count,(Some(index),false)=>(index+1)%count,(None,true)=>count-1,(None,false)=>0})
}
fn modal_is_innermost(dialog:&web_sys::Element)->bool {
    dialog.is_connected()&&dialog.matches(":modal").unwrap_or(false)&&dialog.query_selector("dialog:modal").ok().flatten().is_none()
}
fn modal_control_is_rendered(node:&web_sys::HtmlElement)->bool {
    if node.offset_width()==0&&node.offset_height()==0{return false}
    // Closed details may retain layout metrics for descendants that cannot focus.
    let mut ancestor=node.parent_element();
    while let Some(element)=ancestor {
        if element.matches("details:not([open])").unwrap_or(false) && !element.query_selector(":scope > summary").ok().flatten().is_some_and(|summary|summary.contains(Some(node))) {return false}
        ancestor=element.parent_element();
    }
    true
}
fn modal_controls(dialog:&web_sys::Element)->Vec<web_sys::HtmlElement> {
    use wasm_bindgen::JsCast;
    let Ok(nodes)=dialog.query_selector_all("button, a[href], input:not([type=hidden]), select, textarea, summary, [tabindex], [contenteditable=true]")else{return vec![]};
    (0..nodes.length()).filter_map(|index|nodes.item(index)?.dyn_into::<web_sys::HtmlElement>().ok()).filter(|node| {
        node.tab_index()>=0 && modal_control_is_rendered(node)
            && !node.matches(":disabled").unwrap_or(true)
            && node.closest("[hidden], [inert]").ok().flatten().is_none()
            && node.closest("dialog:modal").ok().flatten().is_some_and(|owner|owner.is_same_node(Some(dialog)))
    }).collect()
}
fn modal_recover_focus(id:&str) {
    use wasm_bindgen::JsCast;
    let Some(document)=web_sys::window().and_then(|w|w.document())else{return};
    let Some(dialog)=document.get_element_by_id(id)else{return};
    if !modal_is_innermost(&dialog){return}
    if let Some(active)=document.active_element() {
        if dialog.contains(Some(&active)){return}
        // Opening another native modal transfers ownership to that modal.
        if active.closest("dialog:modal").ok().flatten().is_some(){return}
    }
    if let Some(first)=modal_controls(&dialog).first(){let _=first.focus();}
    else if let Some(dialog)=dialog.dyn_ref::<web_sys::HtmlElement>(){let _=dialog.focus();}
}
/// Native dialogs retain Escape and launching-control restoration. Explicit Tab
/// containment also covers nested dialogs and the browser focus-to-BODY boundary.
#[component]
pub fn ModuleDialog(open: RwSignal<bool>, title: String, #[prop(optional)] busy: Option<RwSignal<bool>>, children: Children) -> impl IntoView {
    use wasm_bindgen::JsCast;
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let id = format!("module-dialog-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let title_id = format!("{id}-title");
    let effect_id = id.clone();
    let keyboard_id=id.clone();let focus_id=id.clone();
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
            on:keydown=move |ev:web_sys::KeyboardEvent| {
                if ev.key()!="Tab"||ev.alt_key()||ev.ctrl_key()||ev.meta_key(){return}
                let Some(document)=web_sys::window().and_then(|w|w.document())else{return};
                let Some(dialog)=document.get_element_by_id(&keyboard_id)else{return};
                if !modal_is_innermost(&dialog){return}
                let controls=modal_controls(&dialog);
                let active=document.active_element();
                let current=controls.iter().position(|control|active.as_ref().is_some_and(|active|control.is_same_node(Some(active))));
                ev.prevent_default();ev.stop_propagation();
                if let Some(next)=modal_tab_target(controls.len(),current,ev.shift_key()){let _=controls[next].focus();}
                else if let Some(dialog)=dialog.dyn_ref::<web_sys::HtmlElement>(){let _=dialog.focus();}
            }
            on:focusout=move |ev:web_sys::FocusEvent| {
                // focusout fires while activeElement is temporarily BODY, before
                // focusin finishes. Never interrupt a valid native transition.
                if let Some(related)=ev.related_target().and_then(|target|target.dyn_into::<web_sys::Element>().ok()) {
                    let current=web_sys::window().and_then(|w|w.document()).and_then(|document|document.get_element_by_id(&focus_id));
                    if current.is_some_and(|dialog|dialog.contains(Some(&related)))||related.closest("dialog:modal").ok().flatten().is_some(){return}
                }
                let id=focus_id.clone();
                leptos::task::spawn_local(async move {
                    // Check after the browser has completed the native focus move.
                    let _=wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&wasm_bindgen::JsValue::UNDEFINED)).await;
                    modal_recover_focus(&id);
                });
            }
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
    fn modal_tab_ring_wraps_both_directions_and_recovers_outside_focus() {
        assert_eq!(modal_tab_target(3,Some(2),false),Some(0));
        assert_eq!(modal_tab_target(3,Some(0),true),Some(2));
        assert_eq!(modal_tab_target(3,None,false),Some(0));
        assert_eq!(modal_tab_target(3,None,true),Some(2));
        assert_eq!(modal_tab_target(1,Some(0),false),Some(0));
        assert_eq!(modal_tab_target(0,None,false),None);
    }
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
        assert_eq!(Page::ALL.len(), 9);
        assert!(!Page::ALL.contains(&Page::Accounts));
        assert_eq!(Page::Users.title(), "用户与 Key");
    }
}
