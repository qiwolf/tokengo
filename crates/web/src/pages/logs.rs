//! 日志：**出问题时不用 SSH，在面板里就能看现场**。
//!
//! 用户 2026-10-18 要求：「增加日志功能，方便使用过程中出现 bug 可以排查问题」。
//!
//! 这一页给三样东西：
//!   1. 最近的运行日志（内存里那一份，重启清零；文件那份在磁盘上带轮转、有上限）
//!   2. 能按级别和关键词筛 —— 关键词可以直接填请求号，一个请求从进到出错整条线都在
//!   3. 「临时开 debug」：平常只记 info，出怪事时开 15 分钟，到点自动收回
//!      （不收的话日志会一直很啰嗦，还会把磁盘刷满）

use leptos::prelude::*;

use crate::api;
use crate::pages::PageHead;
use crate::Ctx;

#[component]
pub fn Logs(ctx: Ctx) -> impl IntoView {
    let lines = RwSignal::new(Vec::<String>::new());
    let kept = RwSignal::new(0usize);
    let level = RwSignal::new(String::new());
    let revert_at = RwSignal::new(0i64);
    let file = RwSignal::new(String::new());
    let pick_level = RwSignal::new("all".to_string());
    let keyword = RwSignal::new(String::new());
    let limit = RwSignal::new(200u32);
    let auto = RwSignal::new(true); // 日志页开着就该跟着动
    let timer = RwSignal::new(0i32);
    let busy = RwSignal::new(false);
    let err = RwSignal::new(None::<String>);
    let now = RwSignal::new(crate::now_ms());
    let opened = RwSignal::new(false); // 「临时开 debug」的二次确认

    let level_word = move || level_label(&level.get());

    let reload = move || {
        busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let lv = pick_level.get_untracked();
            let lv = if lv == "all" { None } else { Some(lv) };
            let kw = keyword.get_untracked();
            let kw = if kw.trim().is_empty() { None } else { Some(kw) };
            match api::logs_live(lv, kw, limit.get_untracked()).await {
                Ok(v) => {
                    lines.set(v.lines);
                    kept.set(v.kept);
                    level.set(v.level);
                    revert_at.set(v.revert_at_ms.unwrap_or(0));
                    file.set(v.file.unwrap_or_default());
                    err.set(None);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(Some(e.message));
                }
            }
            now.set(crate::now_ms());
            busy.set(false);
        });
    };
    reload();

    // 自动刷新：日志是"正在发生"的东西，默认开着；关掉页面就停
    let toggle_auto = move |on: bool| {
        auto.set(on);
        let id = timer.get_untracked();
        if on {
            let handle = crate::every(3000, move || reload());
            timer.set(handle);
        } else if id != 0 {
            crate::stop_timer(id);
            timer.set(0);
        }
    };
    toggle_auto(true);

    // 每 15 秒走一次"当前时间"，让限时倒计时会动
    let _tick = crate::every(15000, move || now.set(crate::now_ms()));

    let remaining = move || {
        let at = revert_at.get();
        if at <= 0 {
            return String::new();
        }
        let left = (at - now.get()).max(0) / 1000;
        format!("{} 分 {} 秒后自动收回", left / 60, left % 60)
    };

    let set_level_now = move |lv: &'static str, minutes: Option<u32>| {
        busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api::set_log_level(lv, minutes).await {
                Ok(v) => {
                    level.set(v.level);
                    revert_at.set(v.revert_at_ms.unwrap_or(0));
                    ctx.say(match minutes {
                        Some(m) if m > 0 => format!("日志级别已开到 {lv}，{m} 分钟后自动收回"),
                        _ => format!("日志级别已设成 {lv}"),
                    });
                    // 立刻拉一次：能马上看到 debug 行冒出来，才叫"好用"
                    let lv = pick_level.get_untracked();
                    let lv = if lv == "all" { None } else { Some(lv) };
                    if let Ok(v) = api::logs_live(lv, None, limit.get_untracked()).await {
                        lines.set(v.lines);
                        kept.set(v.kept);
                    }
                }
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(Some(e.message));
                }
            }
            busy.set(false);
        });
    };

    view! {
        <div class="page">
            <PageHead
                title="日志"
                desc="网关自己记的日志：出问题时在这里找现场，按级别和关键词筛，关键词可以直接填请求号。"
            >
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
                <button
                    class="chip"
                    class:active=move || auto.get()
                    on:click=move |_| toggle_auto(!auto.get_untracked())
                >
                    {move || if auto.get() { "自动刷新：开（3 秒）" } else { "自动刷新：关" }}
                </button>
            </PageHead>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            // 当前级别 + 限时开关
            <section class="card">
                <div class="card-body toolbar">
                    <div>
                        <div class="muted small">"当前日志级别"</div>
                        <div>
                            <strong>{move || level_word()}</strong>
                            <span class="muted small" style="margin-left:8px">
                                {move || remaining()}
                            </span>
                        </div>
                        <div class="muted small" style="margin-top:4px">
                            {move || if file.get().is_empty() {
                                "（这台机器上没写日志文件，只有内存里这一份）".to_string()
                            } else {
                                format!("日志文件：{}（单文件 4 MB 自动轮转，最多留 3 个）", file.get())
                            }}
                        </div>
                    </div>
                    <span class="spacer"></span>
                    <div class="actions">
                        <button
                            class="ghost"
                            disabled=move || busy.get()
                            on:click=move |_| set_level_now("debug", Some(15))
                        >
                            "临时开 debug（15 分钟）"
                        </button>
                        <button
                            class="ghost"
                            disabled=move || busy.get()
                            on:click=move |_| {
                                if opened.get_untracked() {
                                    opened.set(false);
                                    set_level_now("info", None);
                                } else {
                                    opened.set(true);
                                }
                            }
                        >
                            {move || if opened.get() { "再点一次：长期开 info" } else { "改回 info" }}
                        </button>
                    </div>
                </div>
            </section>

            // 筛选条
            <div class="toolbar">
                <span class="seg">
                    <span class="seg-label">"级别"</span>
                    {[
                        ("all", "全部"),
                        ("info", "信息以上"),
                        ("warn", "警告以上"),
                        ("error", "只看错误"),
                    ]
                        .into_iter()
                        .map(|(v, label)| view! {
                            <button
                                class="chip"
                                class:active=move || pick_level.get() == v
                                on:click=move |_| {
                                    pick_level.set(v.to_string());
                                    reload();
                                }
                            >
                                {label}
                            </button>
                        })
                        .collect_view()}
                </span>
                <span class="seg">
                    <span class="seg-label">"行数"</span>
                    {[100u32, 200, 500, 1000]
                        .into_iter()
                        .map(|n| view! {
                            <button
                                class="chip"
                                class:active=move || limit.get() == n
                                on:click=move |_| {
                                    limit.set(n);
                                    reload();
                                }
                            >
                                {n}
                            </button>
                        })
                        .collect_view()}
                </span>
                <span class="spacer"></span>
                <input
                    class="input"
                    style="min-width:14rem"
                    placeholder="关键词 / 请求号 / 渠道名"
                    prop:value=move || keyword.get()
                    on:input=move |ev| keyword.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            reload();
                        }
                    }
                />
                <button class="primary" on:click=move |_| reload()>"筛"</button>
            </div>

            <section class="card">
                <div class="card-body">
                    <div class="muted small" style="margin-bottom:6px">
                        {move || format!("内存里留着 {} 行，下面是筛出来的 {}", kept.get(), lines.get().len())}
                    </div>
                    <div class="logview">
                        {move || {
                            let ls = lines.get();
                            if ls.is_empty() {
                                view! { <div class="muted">"没有符合条件的日志 —— 也许这段时间本来就没出过事。"</div> }
                                    .into_any()
                            } else {
                                ls.into_iter()
                                    .map(|l| {
                                        let cls = if l.contains(" ERROR") {
                                            "logline error"
                                        } else if l.contains(" WARN") {
                                            "logline warn"
                                        } else {
                                            "logline"
                                        };
                                        view! { <div class=cls>{l}</div> }
                                    })
                                    .collect_view()
                                    .into_any()
                            }
                        }}
                    </div>
                </div>
            </section>
        </div>
    }
}

/// 过滤串 → 人话（顺带告诉用户"这个档记什么"）。抽出来是为了能测：
/// 面板上这几句话是用户判断"现在到底记了多少"的唯一依据，写错会误导排障。
///
/// **只看我自己那一段**（`tokengo_server=…`）：默认档就带着 `tower_http=warn`
/// （那是 HTTP 层的噪声开关），拿整串去认会把「平常档」误报成「warn 档」——
/// 第一版就是这么写错的，测试当场逮到。
pub fn level_label(filter: &str) -> String {
    let own = filter
        .split(',')
        .find(|p| p.trim_start().starts_with("tokengo_server="))
        .unwrap_or(filter);
    let word = own
        .rsplit('=')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let out = match word.as_str() {
        "debug" | "trace" => "debug（很啰嗦，排查完记得收回）",
        "warn" => "warn（警告与错误）",
        "error" => "error（只记错误）",
        _ => "info（平常档）",
    };
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_filter_gets_an_honest_label() {
        assert!(level_label("tokengo_server=debug,tower_http=info").contains("debug"));
        assert!(level_label("tokengo_server=warn,tower_http=warn").contains("警告"));
        assert!(level_label("tokengo_server=error,tower_http=error").contains("只记错误"));
        // 默认档（部署时就是这一串）必须落到 info，别误报成 warn/error
        assert!(level_label("tokengo_server=info,tower_http=warn").starts_with("info"));
        assert!(level_label("").starts_with("info"), "认不出就当平常档");
    }
}
