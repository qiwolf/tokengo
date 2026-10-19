//! tokengo 面板。**用 Rust 写界面（Leptos CSR → WASM），不引 npm 依赖。**
//!
//! 只做本机真正用得上的 5 页，导航在顶栏（照 freellmapi 面板的排法：
//! 顶上一条横菜单、每页一个大标题加一句话、内容就是普通表格）：
//!
//!   1. 总览  四个数 + 最近调用 + 各条渠道的账 + 能被调用的模型名
//!   2. 渠道  增删改查 + 自定义请求头 + 按渠道改模型名 + 「测一下」连通性
//!   3. 模型  一行 = 渠道里勾过的名字（含保留名 auto）+ 现在走哪条、何时回头试首选 + 路由参数
//!   4. 请求  模型、渠道、状态、延迟、token、错误原因
//!   5. 设置  对外 key、面板口令、退出登录
//!
//! 后端始终是唯一数据源：面板**不做本地缓存**，每次进页面重新拉 ——
//! 免得界面上显示的是过期状态（"看着还在用首选、其实早切走了"最坏事）。

pub mod api;
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
    /// 任何接口返回 401 都走这里：清掉本地 token、退回登录页。
    /// **统一处理**才不会出现"某个页面还在傻傻重试"的情况。
    pub fn note_err(&self, e: &api::ApiErr) {
        if e.is_unauthorized() {
            api::clear_token();
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
        self.page.set(p);
        self.toast.set(None);
        // 手机上在菜单里点完就收起菜单
        self.nav_open.set(false);
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
        page,
        toast,
        toast_timer:RwSignal::new(None),
        nav_open,
        theme,
    };

    // 启动：问服务端要初始化状态；顺手用现有 token 试一下，能用就直接进
    wasm_bindgen_futures::spawn_local(async move {
        match api::auth_status().await {
            Ok(st) => {
                if st.needs_setup {
                    phase.set(Phase::Setup);
                    return;
                }
                if api::token().is_none() {
                    phase.set(Phase::Login);
                    return;
                }
                match api::channels().await {
                    Ok(_) => phase.set(Phase::Ready),
                    Err(e) => {
                        if e.is_unauthorized() {
                            api::clear_token();
                            ctx.say("会话过期了，请重新登录");
                        } else {
                            ctx.say(format!("读渠道列表失败：{}", e.message));
                        }
                        phase.set(Phase::Login);
                    }
                }
            }
            Err(e) => {
                phase.set(Phase::Login);
                ctx.say(format!("问不到服务端状态：{}", e.message));
            }
        }
    });

    // URL 的 hash ↔ 当前页：刷新不丢页，浏览器前进后退也能用
    {
        let cb = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
            let p = Page::from_hash();
            if p != page.get_untracked() {
                page.set(p);
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
            Phase::Setup => view! { <SetupScreen ctx=ctx /> }.into_any(),
            Phase::Login => view! { <LoginScreen ctx=ctx /> }.into_any(),
            Phase::Ready => view! { <Shell ctx=ctx /> }.into_any(),
        }}
    }
}

/// 首次初始化：设面板口令，顺便定对外 key。
#[component]
fn SetupScreen(ctx: Ctx) -> impl IntoView {
    let (pw, set_pw) = signal(String::new());
    let (key, set_key) = signal(String::new());
    let (busy, set_busy) = signal(false);
    let (once, set_once) = signal(None::<(String, bool)>);

    let submit = move |_| {
        if busy.get_untracked() {
            return;
        }
        set_busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let k = key.get_untracked();
            let k = if k.trim().is_empty() { None } else { Some(k) };
            match api::setup(&pw.get_untracked(), k.as_deref()).await {
                Ok(r) => {
                    api::set_token(&r.token);
                    set_once.set(Some((r.gateway_key.unwrap_or_default(), r.gateway_key_generated)));
                    set_busy.set(false);
                }
                Err(e) => {
                    ctx.fail(e);
                    set_busy.set(false);
                }
            }
        });
    };

    view! {
        <div class="auth-wrap">
            <div class="auth-card">
                <h1>{tokengo_shared::BRAND}" 初始化"</h1>
                <p class="muted">
                    "第一次用，先定一个面板口令。对外 key 就是客户端填的那个 key ——
                     留空则由这里生成一个。"
                </p>
                {move || match once.get() {
                    Some((k, generated)) => view! {
                        <div class="notice">
                            <b>"对外 key（只显示这一次）"</b>
                            <code class="keybox">{k}</code>
                            <p class="muted">
                                {move || if generated { "这是自动生成的。" } else { "" }}
                                "库里只存哈希，之后拿不回明文，请现在记到客户端配置里。"
                            </p>
                            <button class="primary" on:click=move |_| ctx.phase.set(Phase::Ready)>
                                "记好了，进面板"
                            </button>
                        </div>
                    }.into_any(),
                    None => view! {
                        <div>
                            <label>"面板口令"</label>
                            <input type="password" prop:value=pw
                                on:input=move |ev| set_pw.set(event_target_value(&ev)) />
                            <label>"对外 key（可留空）"</label>
                            <input type="text" prop:value=key placeholder="留空 = 自动生成"
                                on:input=move |ev| set_key.set(event_target_value(&ev)) />
                            <button class="primary" disabled=move || busy.get() on:click=submit>
                                {move || if busy.get() { "正在设置…" } else { "设置并进入" }}
                            </button>
                        </div>
                    }.into_any(),
                }}
            </div>
        </div>
    }
}

#[component]
fn LoginScreen(ctx: Ctx) -> impl IntoView {
    let (pw, set_pw) = signal(String::new());
    let (busy, set_busy) = signal(false);

    // 不带参数的闭包，才能同时给 click 和 keydown 两处用（事件类型不同，
    // 带参数的闭包在两处会被推断成冲突的签名）
    let submit = move || {
        if busy.get_untracked() {
            return;
        }
        set_busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api::login(&pw.get_untracked()).await {
                Ok(r) => {
                    api::set_token(&r.token);
                    set_pw.set(String::new());
                    ctx.toast.set(None);
                    ctx.phase.set(Phase::Ready);
                }
                Err(e) => ctx.say(format!("登录失败：{}", e.message)),
            }
            set_busy.set(false);
        });
    };

    view! {
        <div class="auth-wrap">
            <div class="auth-card">
                <h1>{tokengo_shared::BRAND}" 面板"</h1>
                <label>"口令"</label>
                <input type="password" prop:value=pw
                    on:input=move |ev| set_pw.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            submit();
                        }
                    } />
                <button class="primary" disabled=move || busy.get() on:click=move |_| submit()>
                    {move || if busy.get() { "登录中…" } else { "登录" }}
                </button>
            </div>
        </div>
    }
}

/// 登录后的整体布局：**顶上一条横菜单** + 内容区。
///
/// 为什么不用左侧栏：手机上一竖列塞不下五个够大的按钮、还得做成抽屉；
/// 顶栏横排 + 右上角一个菜单按钮更省地方，桌面端五个两字短名也排得下。
/// 页面本身统一成「大标题 + 一句话 + 一条横线」（见 `pages::PageHead`），
/// 于是到哪一页都知道标题在哪、按钮在哪 —— 这是照 freellmapi 面板的排法抄的。
#[component]
fn Shell(ctx: Ctx) -> impl IntoView {
    let logout = move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            // 撤销会话失败也照样退出本地登录：不能因为网络问题就退不出去
            let _ = api::logout().await;
            api::clear_token();
            ctx.phase.set(Phase::Login);
        });
    };
    let reload = move |_| {
        if let Some(w) = web_sys::window() {
            let _ = w.location().reload();
        }
    };

    view! {
        <div class="shell">
            <header class="topbar">
                <div class="topbar-inner">
                    <button class="brand brand-home" title="返回总览" on:click=move |_| ctx.go(Page::Overview)>
                        "Token-Go"
                        <span class="muted small">"v" {tokengo_shared::VERSION}</span>
                    </button>
                    // 桌面端：五个页面横排（手机上这排会藏起来，走右上角菜单）
                    <nav class="topnav">
                        {Page::ALL
                            .iter()
                            .map(|p| {
                                let p = *p;
                                view! {
                                    <button
                                        class="nav"
                                        class:active=move || ctx.page.get() == p
                                        title=p.blurb()
                                        on:click=move |_| ctx.go(p)
                                    >
                                        {p.title()}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </nav>
                    <span class="spacer"></span>
                    {move || ctx.toast.get().map(|t| view! { <span class="toast">{t}</span> })}
                    <button
                        class="ghost icon-btn"
                        title=move || {
                            if ctx.theme.get().is_light() { "切到夜间" } else { "切到日间" }
                        }
                        on:click=move |_| ctx.toggle_theme()
                    >
                        {move || if ctx.theme.get().is_light() { "☾" } else { "☀" }}
                    </button>
                    <div class="menu-wrap">
                        <button
                            class="ghost icon-btn nav-toggle"
                            title="菜单"
                            on:click=move |_| ctx.nav_open.update(|v| *v = !*v)
                        >
                            "☰"
                        </button>
                    </div>
                </div>
            </header>
            // 菜单的遮罩与弹窗**不能待在顶栏里面**：顶栏带 backdrop-filter（毛玻璃），
            // 它会变成 fixed 后代的包含块 —— 遮罩的 inset:0 于是只铺满 54px 高的顶栏，
            // 点页面别处根本点不到它，菜单就关不掉（2026-10-18 用户实测报的）。
            {move || {
                ctx.nav_open
                    .get()
                    .then(|| {
                        view! {
                            <>
                                <button
                                    class="nav-scrim"
                                    title="收起菜单"
                                    on:click=move |_| ctx.nav_open.set(false)
                                ></button>
                                <div class="menu-pop">
                                    {Page::ALL
                                        .iter()
                                        .map(|p| {
                                            let p = *p;
                                            view! {
                                                <button
                                                    class="menu-item"
                                                    class:active=move || ctx.page.get() == p
                                                    on:click=move |_| ctx.go(p)
                                                >
                                                    {p.title()}
                                                </button>
                                            }
                                        })
                                        .collect_view()}
                                    <div class="menu-sep"></div>
                                    <button class="menu-item" on:click=reload>
                                        "重新载入面板"
                                    </button>
                                    <button class="menu-item danger" on:click=logout>
                                        "退出登录"
                                    </button>
                                </div>
                            </>
                        }
                    })
            }}

            <main class="main">
                <section class="content">
                    {move || match ctx.page.get() {
                        Page::Overview => view! { <pages::overview::Overview ctx=ctx /> }.into_any(),
                        Page::Channels => view! { <pages::channels::Channels ctx=ctx /> }.into_any(),
                        Page::Models => view! { <pages::models::Models ctx=ctx /> }.into_any(),
                        Page::Settings => view! { <pages::settings::Settings ctx=ctx /> }.into_any(),
                    }}
                </section>
            </main>
        </div>
    }
}
