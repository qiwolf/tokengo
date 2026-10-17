//! 总览：一段时间的账 + 最近调用 + 各条渠道的分布 + 能被调用的模型名。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{LogRow, Servable, StatsResp, status_label};
use crate::pages::PageHead;
use crate::Ctx;

#[component]
pub fn Overview(ctx: Ctx) -> impl IntoView {
    let (hours, set_hours) = signal(24i64);
    let stats = RwSignal::new(StatsResp::default());
    let servable = RwSignal::new(Vec::<Servable>::new());
    let recent = RwSignal::new(Vec::<LogRow>::new());
    let channels = RwSignal::new(Vec::<crate::model::Channel>::new());
    let sessions = RwSignal::new(0i64);
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let reload = move || {
        busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let h = hours.get_untracked();
            match api::stats(h).await {
                Ok(s) => {
                    stats.set(s);
                    err.set(None);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(Some(e.message));
                }
            }
            match api::groups().await {
                Ok(g) => servable.set(g.servable),
                Err(e) => ctx.note_err(&e),
            }
            match api::logs(12).await {
                Ok(v) => recent.set(v),
                Err(e) => ctx.note_err(&e),
            }
            match api::channels().await {
                Ok(v) => channels.set(v),
                Err(e) => ctx.note_err(&e),
            }
            match api::settings().await {
                Ok(s) => sessions.set(s.active_sessions),
                Err(e) => ctx.note_err(&e),
            }
            busy.set(false);
        });
    };
    reload();

    let pick = move |h: i64| {
        set_hours.set(h);
        reload();
    };

    let channel_name = move |id: Option<i64>| match id {
        None => "—".to_string(),
        Some(i) => channels
            .get()
            .iter()
            .find(|l| l.id == i)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("已删 #{i}")),
    };

    view! {
        <div class="page">
            <PageHead
                title="总览"
                desc="最近这段时间的请求、各条渠道的表现，以及客户端能打的模型名。"
            >
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
            </PageHead>

            <div class="toolbar">
                <span class="muted small">"时间窗口"</span>
                {[1i64, 24, 72, 168]
                    .into_iter()
                    .map(|h| {
                        let label = match h {
                            1 => "1 小时".to_string(),
                            24 => "24 小时".to_string(),
                            72 => "3 天".to_string(),
                            _ => "7 天".to_string(),
                        };
                        view! {
                            <button
                                class="chip"
                                class:active=move || hours.get() == h
                                on:click=move |_| pick(h)
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
                <span class="spacer"></span>
                <span class="muted small">"在线会话 " {move || sessions.get()}</span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            // 四个数：并排的数字条（不是卡片，靠竖线分隔）
            <div class="stats">
                <div class="stat">
                    <div class="stat-title">"请求数"</div>
                    <div class="stat-value">{move || fmt::num(stats.get().stats.total)}</div>
                    <div class="stat-sub">{move || format!("窗口 {} 小时", stats.get().window_hours)}</div>
                </div>
                <div class="stat">
                    <div class="stat-title">"成功率"</div>
                    <div class="stat-value">
                        {move || {
                            let s = stats.get().stats;
                            fmt::pct(s.ok, s.total)
                        }}
                    </div>
                    <div class="stat-sub">{move || format!("成功 {}", fmt::num(stats.get().stats.ok))}</div>
                </div>
                <div class="stat">
                    <div class="stat-title">"失败的请求"</div>
                    <div class="stat-value">{move || fmt::num(stats.get().stats.failed)}</div>
                    <div class="stat-sub">
                        {move || {
                            let s = stats.get().stats;
                            format!("取消 {} · 客户端错 {}", fmt::num(s.canceled), fmt::num(s.client_errors))
                        }}
                    </div>
                </div>
                <div class="stat">
                    <div class="stat-title">"Token"</div>
                    <div class="stat-value">
                        {move || {
                            let s = stats.get().stats;
                            fmt::num(s.input_tokens + s.output_tokens)
                        }}
                    </div>
                    <div class="stat-sub">
                        {move || {
                            let s = stats.get().stats;
                            format!("入 {} · 出 {}", fmt::num(s.input_tokens), fmt::num(s.output_tokens))
                        }}
                    </div>
                </div>
            </div>

            <h2>"最近调用"</h2>
            <p class="muted small">"最新进来的请求 —— 哪个模型、走了哪条渠道、成没成、花了多久。"</p>
            <div class="table-wrap">
                <table class="grid">
                    <thead>
                        <tr>
                            <th>"时间"</th>
                            <th>"模型"</th>
                            <th>"走的渠道"</th>
                            <th>"结果"</th>
                            <th class="num">"耗时"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let v = recent.get();
                            if v.is_empty() {
                                return view! {
                                    <tr class="empty-row">
                                        <td colspan="5" class="muted empty-cell">
                                            "还没有请求进来。客户端打 /v1/chat/completions 或 /v1/embeddings 就会出现在这里。"
                                        </td>
                                    </tr>
                                }
                                .into_any();
                            }
                            v.into_iter()
                                .map(|r| {
                                    let (label, class) = status_label(&r.status);
                                    let channel = channel_name(r.channel_id);
                                    view! {
                                        <tr>
                                            <td class="nowrap small">{fmt::ts_short(r.ts_ms)}</td>
                                            <td><code class="small">{r.model.clone()}</code></td>
                                            <td>{channel}</td>
                                            <td><span class=format!("tag {class}")>{label}</span></td>
                                            <td class="num">{fmt::dur(r.latency_ms)}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </tbody>
                </table>
            </div>

            <h2>"各条渠道的账"</h2>
            <p class="muted small">
                "分母是「被叫了几次」，不是「成功了几次」—— 判断一条渠道值不值得留就看这个。"
            </p>
            <div class="table-wrap">
                <table class="grid">
                    <thead>
                        <tr>
                            <th>"渠道"</th>
                            <th class="num">"被叫"</th>
                            <th class="num">"成功"</th>
                            <th class="num">"成功率"</th>
                            <th class="num">"渠道故障"</th>
                            <th class="num">"平均耗时"</th>
                            <th class="num">"Token"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let rows = stats.get().stats.by_channel;
                            if rows.is_empty() {
                                return view! {
                                    <tr class="empty-row">
                                        <td colspan="7" class="muted empty-cell">
                                            "这个窗口里还没有账（没流量）"
                                        </td>
                                    </tr>
                                }
                                .into_any();
                            }
                            rows.into_iter()
                                .map(|l| {
                                    let name = l.channel_name.clone().unwrap_or_else(|| format!("#{}", l.channel_id));
                                    view! {
                                        <tr>
                                            <td>{name}</td>
                                            <td class="num">{fmt::num(l.attempts)}</td>
                                            <td class="num">{fmt::num(l.successes)}</td>
                                            <td class="num">
                                                {l.success_rate.map(|r| format!("{r:.1}%")).unwrap_or_else(|| "—".into())}
                                            </td>
                                            <td class="num">
                                                {if l.channel_faults > 0 {
                                                    view! { <span class="bad-text">{fmt::num(l.channel_faults)}</span> }.into_any()
                                                } else {
                                                    view! { <span class="muted">"0"</span> }.into_any()
                                                }}
                                            </td>
                                            <td class="num">{fmt::dur(l.avg_ms)}</td>
                                            <td class="num">{fmt::num(l.input_tokens + l.output_tokens)}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </tbody>
                </table>
            </div>

            <h2>"能被调用的模型名"</h2>
            <p class="muted small">
                "客户端填这些名字才打得通（和 "
                <code>"/v1/models"</code>
                " 用的是同一份判断：组是开着的，且组里至少有一条可用渠道）。"
                "标了"
                <span class="tag vision">"视觉"</span>
                "的名字后面有能看图的渠道 —— 带图请求要打这类名字，"
                "打到没标的会回 422（说清楚，不悄悄丢图）。"
            </p>
            <div class="chips">
                {move || {
                    let v = servable.get();
                    if v.is_empty() {
                        return view! { <span class="muted">"一个都没有 —— 先去「模型」建一个"</span> }.into_any();
                    }
                    v.into_iter()
                        .map(|m| {
                            view! {
                                <code
                                    class="chip code"
                                    title=if m.vision {
                                        "收得下图片输入"
                                    } else {
                                        "纯文本：带图的请求会被回 422"
                                    }
                                >
                                    {m.name}
                                    {m.vision.then(|| view! { <span class="tag vision">"视觉"</span> })}
                                </code>
                            }
                        })
                        .collect_view()
                        .into_any()
                }}
            </div>
        </div>
    }
}

/// 流水页也用得到的「状态」小标签在这里定义一次。
#[component]
pub fn StatusTag(status: String) -> impl IntoView {
    let (label, class) = status_label(&status);
    view! { <span class=format!("tag {class}")>{label}</span> }
}
