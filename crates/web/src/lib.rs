//! tokengo 面板。**用 Rust 写界面（Leptos CSR → WASM），不引 npm 依赖。**
//!
//! 管理员控制台与普通账号门户分离；浏览器仅使用同源 HttpOnly 会话。
//! 后端是账号、网关状态与账本的唯一数据源，每次进入页面重新读取。

pub mod api;
pub mod account_api;
pub mod provider_api;
pub mod account_ui;
pub mod admin_api;
pub mod fmt;
pub mod model;
pub mod pages;

pub use pages::Page;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// 面板启动入口（`main.rs` 调）。
pub fn start() {
    console_error_panic_hook::set_once();
    // 主题先在 WASM 之前定好（index.html 里那段内联脚本也做同一件事，负责「不闪」），
    // 这里再把同一个结果读进 Rust 侧的状态。
    apply_theme(init_theme());
    mount_to_body(App);
}

/// 面板主题。深色是默认，浅色挂在 `html[data-theme="light"]` 上（style.css 里两套变量）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn attr(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    pub fn is_light(self) -> bool {
        matches!(self, Theme::Light)
    }
}

/// localStorage 里存主题的键。
/// **`index.html` 里那段内联脚本读的是同一个键** —— 改这里就得两边一起改。
pub const THEME_KEY: &str = "tokengo.theme";

/// 启动时定主题：浏览器里记着就用记着的，没记过就跟随系统设置。
fn init_theme() -> Theme {
    let saved = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(THEME_KEY).ok())
        .flatten();
    match saved.as_deref() {
        Some("light") => Theme::Light,
        Some("dark") => Theme::Dark,
        // 跟随系统：读一次浏览器的颜色偏好，不做实时监听（用户手动切过就以手动为准）
        _ => {
            let light = web_sys::window()
                .and_then(|w| w.match_media("(prefers-color-scheme: light)").ok())
                .flatten()
                .map(|m| m.matches())
                .unwrap_or(false);
            if light {
                Theme::Light
            } else {
                Theme::Dark
            }
        }
    }
}

fn apply_theme(t: Theme) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = el.set_attribute("data-theme", t.attr());
    }
}

fn remember_theme(t: Theme) {
    if let Some(s) = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
    {
        let _ = s.set_item(THEME_KEY, t.attr());
    }
}

/// 当前毫秒时间。面板只拿它算"几分钟前"，不做业务判断（业务判断全在服务端）。
pub fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

/// 每 `ms` 毫秒跑一次 `f`，返回句柄（用 `stop_timer` 停掉）。
///
/// 用浏览器的定时器而不是起个循环：页面一关定时器就没了，不会在后台空转。
pub fn every(ms: i32, f: impl Fn() + 'static) -> i32 {
    let cb = wasm_bindgen::closure::Closure::wrap(Box::new(f) as Box<dyn FnMut()>);
    // into_js_value：把闭包交给 JS 堆管，定时器自己持有引用，不用在 Rust 侧存着
    let func: js_sys::Function = cb.into_js_value().unchecked_into();
    web_sys::window()
        .and_then(|w| {
            w.set_interval_with_callback_and_timeout_and_arguments_0(&func, ms)
                .ok()
        })
        .unwrap_or(0)
}

pub fn stop_timer(handle: i32) {
    if let Some(w) = web_sys::window() {
        w.clear_interval_with_handle(handle);
    }
}

/// 启动阶段：先问服务端"要不要初始化"，再决定给用户看哪个界面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 还在问服务端
    Loading,
    /// 从没设过口令 → 首次初始化
    Setup,
    /// 要登录
    Login,
    /// 已登录，进面板
    Ready,
}

/// 全局面板上下文。`Copy` 是为了能塞进各个页面的 props。
#[derive(Clone, Copy)]
pub struct Ctx {
    pub phase: RwSignal<Phase>,
    pub account: RwSignal<Option<account_api::Account>>,
    /// 当前页（同时写进 URL 的 hash，刷新与前进后退都还在这一页）
    pub page: RwSignal<Page>,
    /// 一句话提示，显示在顶栏
    pub toast: RwSignal<Option<String>>,
    toast_timer: RwSignal<Option<i32>>,
    /// 顶栏右上角那个菜单开没开（手机上主要靠它跳页）
    pub nav_open: RwSignal<bool>,
    /// 当前主题（日间 / 夜间）
    pub theme: RwSignal<Theme>,
}

impl Ctx {
    pub fn is_root(&self)->bool {self.account.get().is_some_and(|a|a.is_root())}
    pub fn accept_account(&self,a:account_api::Account) {
        let root=a.is_root();self.account.set(Some(a));
        let current=self.page.get_untracked();
        let target=current.for_account(root);
        self.go(target);self.phase.set(Phase::Ready);
    }
    /// 任何接口返回 401 都走这里：清掉本地 token、退回登录页。
    /// **统一处理**才不会出现"某个页面还在傻傻重试"的情况。
    pub fn note_err(&self, e: &api::ApiErr) {
        if e.is_unauthorized() {
            api::clear_token();
            self.account.set(None);
            self.phase.set(Phase::Login);
        }
    }

    pub fn say(&self, msg: impl Into<String>) {
        if let Some(id)=self.toast_timer.get_untracked(){if let Some(w)=web_sys::window(){w.clear_timeout_with_handle(id);}}
        self.toast.set(Some(msg.into()));
        let toast=self.toast;let timer=self.toast_timer;
        let cb=wasm_bindgen::closure::Closure::once_into_js(move ||{toast.set(None);timer.set(None);});
        let id=web_sys::window().and_then(|w|w.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(),4000).ok());
        self.toast_timer.set(id);
    }

    /// 切页并把当前页写进 URL 的 hash —— 刷新、前进、后退都还在这一页。
    pub fn go(&self, p: Page) {
        let p=p.for_account(self.is_root());
        let menu_was_open=self.nav_open.get_untracked();
        self.page.set(p);
        self.toast.set(None);
        // 手机上在菜单里点完就收起菜单
        self.nav_open.set(false);
        if menu_was_open {if let Some(el)=web_sys::window().and_then(|w|w.document()).and_then(|d|d.get_element_by_id("console-main")).and_then(|e|e.dyn_into::<web_sys::HtmlElement>().ok()){let _=el.focus();}}
        if let Some(w) = web_sys::window() {
            let _ = w.location().set_hash(p.slug());
        }
    }

    /// 日间 / 夜间切换：改 `html[data-theme]`，并记住这次选择。
    pub fn toggle_theme(&self) {
        let next = if self.theme.get_untracked().is_light() {
            Theme::Dark
        } else {
            Theme::Light
        };
        self.theme.set(next);
        apply_theme(next);
        remember_theme(next);
    }

    /// 把错误显示出来（401 顺便退回登录）。
    pub fn fail(&self, e: api::ApiErr) {
        self.note_err(&e);
        self.say(e.message);
    }
}

#[component]
pub fn App() -> impl IntoView {
    let phase = RwSignal::new(Phase::Loading);
    let page = RwSignal::new(Page::from_hash());
    let toast = RwSignal::new(None::<String>);
    let nav_open = RwSignal::new(false);
    // init_theme() 在 start() 里已经算过一次并落到 DOM 上了，这里只是把同一份状态拿进信号
    let theme = RwSignal::new(init_theme());
    let ctx = Ctx {
        phase,
        account:RwSignal::new(None),
        page,
        toast,
        toast_timer:RwSignal::new(None),
        nav_open,
        theme,
    };

    // Remove the historical bearer before any browser request. Cookie identity is authoritative.
    api::clear_token();
    wasm_bindgen_futures::spawn_local(async move {
        match api::auth_status().await {
            Ok(st) if st.needs_setup=>phase.set(Phase::Setup),
            Ok(_)=>match account_api::me().await {Ok(a)=>ctx.accept_account(a),Err(e)=>{phase.set(Phase::Login);if !e.is_unauthorized(){ctx.say(format!("无法恢复登录：{}",e.message));}}},
            Err(e)=>{phase.set(Phase::Login);ctx.say(format!("无法连接网关：{}",e.message));}
        }
    });
    Effect::new(move |_| {let title=match phase.get(){Phase::Setup=>"首次设置".to_string(),Phase::Login=>"登录".into(),Phase::Loading=>"正在连接".into(),Phase::Ready=>page.get().title().into()};if let Some(d)=web_sys::window().and_then(|w|w.document()){d.set_title(&format!("{title} · Token-Go 控制台"));}});

    // URL 的 hash ↔ 当前页：刷新不丢页，浏览器前进后退也能用
    {
        let cb = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
            let p = Page::from_hash();
            if p != page.get_untracked() {
                if ctx.account.get_untracked().is_some(){ctx.go(p);}else{page.set(p);}
            }
        }) as Box<dyn FnMut()>);
        if let Some(w) = web_sys::window() {
            let _ = w.add_event_listener_with_callback("hashchange", cb.as_ref().unchecked_ref());
        }
        // 监听器要活到页面结束 —— 换成 forget 而不是让它出作用域被回收
        cb.forget();
    }

    view! {
        {move || match phase.get() {
            Phase::Loading => view! { <div class="centered muted">"正在连网关…"</div> }.into_any(),
            Phase::Setup => view! { <account_ui::AuthScreen ctx=ctx setup=true /> }.into_any(),
            Phase::Login => view! { <account_ui::AuthScreen ctx=ctx /> }.into_any(),
            Phase::Ready => view! { <Shell ctx=ctx /> }.into_any(),
        }}
    }
}

/// Root and user share the visual shell; user routes never mount administrator pages.
#[component]
fn Shell(ctx: Ctx) -> impl IntoView {
    let busy=RwSignal::new(false);let logout_error=RwSignal::new(String::new());
    let logout=move |_| {if busy.get_untracked(){return;}busy.set(true);logout_error.set(String::new());leptos::task::spawn_local(async move{match account_api::logout().await {Ok(())=>{ctx.account.set(None);api::clear_token();ctx.nav_open.set(false);ctx.phase.set(Phase::Login);},Err(e)=>{ctx.note_err(&e);logout_error.set(format!("退出未完成，请重试：{}",e.message));}}busy.set(false);});};
    view! {<div class="console-layout" on:keydown=move|ev|{if ev.key()=="Escape"{ctx.nav_open.set(false);if let Some(el)=web_sys::window().and_then(|w|w.document()).and_then(|d|d.get_element_by_id("console-menu")).and_then(|e|e.dyn_into::<web_sys::HtmlElement>().ok()){let _=el.focus();}}}>
        <a class="skip-link" href="#console-main" on:click=move|ev|{ev.prevent_default();if let Some(el)=web_sys::window().and_then(|w|w.document()).and_then(|d|d.get_element_by_id("console-main")).and_then(|e|e.dyn_into::<web_sys::HtmlElement>().ok()){let _=el.focus();}}>"跳到主内容"</a>
        <header class="console-header"><button type="button" class="console-brand" on:click=move|_|ctx.go(if ctx.is_root(){Page::Overview}else{Page::Portal})><strong>"Token-Go"</strong><span>"私有网关"</span></button><span class="console-header-page">{move||ctx.page.get().title()}</span><span class="spacer"></span><button type="button" class="ghost console-theme" on:click=move|_|ctx.toggle_theme()>{move||if ctx.theme.get().is_light(){"切换深色"}else{"切换浅色"}}</button><button id="console-menu" type="button" class="ghost console-menu-toggle" aria-controls="console-nav" aria-expanded=move||ctx.nav_open.get().to_string() on:click=move|_|ctx.nav_open.update(|v|*v=!*v)>"菜单"</button></header>
        <aside id="console-nav" class="console-sidebar" class:is-open=move||ctx.nav_open.get()><div class="console-nav-caption">{move||if ctx.is_root(){"网关管理"}else{"个人空间"}}</div><nav aria-label="主导航">{move||{let pages=if ctx.is_root(){Page::ALL.to_vec()}else{vec![Page::Portal]};pages.into_iter().map(|p|view!{<button type="button" class="console-nav-item" class:active=move||ctx.page.get()==p aria-current=move||if ctx.page.get()==p{Some("page")}else{None} on:click=move|_|ctx.go(p)>{p.title()}</button>}).collect_view()}}</nav>
        <div class="console-account"><div class="console-account-label"><strong>{move||ctx.account.get().map(|a|a.label()).unwrap_or_default()}</strong><span>{move||if ctx.is_root(){"管理员"}else{"普通账号"}}</span></div><p class="muted small">{move||ctx.account.get().map(|a|a.username).unwrap_or_default()}</p><button type="button" class="console-nav-item" class:active=move||ctx.page.get()==Page::Account on:click=move|_|ctx.go(Page::Account)>"账号安全"</button><button type="button" class="console-nav-item" disabled=move||busy.get() on:click=logout>{move||if busy.get(){"正在退出…"}else{"退出登录"}}</button><span class="console-version">"v"{tokengo_shared::VERSION}</span></div></aside>
        <main id="console-main" class="console-main" tabindex="-1"><div class="console-content"><Show when=move||!logout_error.get().is_empty()><div class="banner bad" role="alert">{move||logout_error.get()}</div></Show>
        {move||if ctx.page.get()==Page::Account {view!{<account_ui::Profile ctx=ctx/>}.into_any()}else if !ctx.is_root(){view!{<pages::portal::Portal ctx=ctx/>}.into_any()}else{match ctx.page.get(){
            Page::Overview=>view!{<pages::overview::Overview ctx=ctx/>}.into_any(),Page::Channels=>view!{<pages::channels::Channels ctx=ctx/>}.into_any(),Page::Models=>view!{<pages::models::Models ctx=ctx/>}.into_any(),Page::Users=>view!{<pages::users::Users ctx=ctx/>}.into_any(),Page::Accounts=>view!{<pages::users::Users ctx=ctx/>}.into_any(),Page::Billing=>view!{<pages::billing::Billing ctx=ctx/>}.into_any(),Page::Settings=>view!{<pages::settings::Settings ctx=ctx/>}.into_any(),_=>view!{<pages::overview::Overview ctx=ctx/>}.into_any(),
        }}}
        </div></main>{move||ctx.toast.get().map(|t|view!{<div class="console-toast" role="status">{t}</div>})}
    </div>}
}
