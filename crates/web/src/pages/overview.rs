//! 总览：一段时间的账 + 各条腿的分布 + 能被调用的模型名。
//!
//! 这里的数字**全部来自请求记账表**，面板自己不算 —— 面板算出来的数和
//! `sqlite3` 里查到的不一样时，谁都不信了。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{StatsResp, status_label};
use crate::Ctx;

#[component]
pub fn Overview(ctx: Ctx) -> impl IntoView {
    let (hours, set_hours) = signal(24i64);
    let stats = RwSignal::new(StatsResp::default());
    let servable = RwSignal::new(Vec::<crate::model::Servable>::new());
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

    view! {
        <div class="page">
            <div class="toolbar">
                <span class="muted">"时间窗口"</span>
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
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
                <span class="spacer"></span>
                <span class="muted small">"在线会话 " {move || sessions.get()}</span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <div class="cards">
                <Card
                    title="请求数"
                    value=Signal::derive(move || fmt::num(stats.get().stats.total))
                    sub=Signal::derive(move || {
                        format!("窗口 {} 小时", stats.get().window_hours)
                    })
                />
                <Card
                    title="成功率"
                    value=Signal::derive(move || {
                        let s = stats.get().stats;
                        fmt::pct(s.ok, s.total)
                    })
                    sub=Signal::derive(move || {
                        let s = stats.get().stats;
                        format!("成功 {}", fmt::num(s.ok))
                    })
                />
                <Card
                    title="失败的请求"
                    value=Signal::derive(move || fmt::num(stats.get().stats.failed))
                    sub=Signal::derive(move || {
                        let s = stats.get().stats;
                        format!("取消 {} · 客户端错 {}", fmt::num(s.canceled), fmt::num(s.client_errors))
                    })
                />
                <Card
                    title="Token"
                    value=Signal::derive(move || {
                        let s = stats.get().stats;
                        fmt::num(s.input_tokens + s.output_tokens)
                    })
                    sub=Signal::derive(move || {
                        let s = stats.get().stats;
                        format!("入 {} · 出 {}", fmt::num(s.input_tokens), fmt::num(s.output_tokens))
                    })
                />
            </div>

            <h2>"各条腿的账"</h2>
            <p class="muted small">
                "分母是「被叫了几次」，不是「成功了几次」—— 判断一条腿值不值得留就看这个。"
            </p>
            <table class="grid">
                <thead>
                    <tr>
                        <th>"腿"</th>
                        <th class="num">"被叫"</th>
                        <th class="num">"成功"</th>
                        <th class="num">"成功率"</th>
                        <th class="num">"腿故障"</th>
                        <th class="num">"平均耗时"</th>
                        <th class="num">"Token"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        let rows = stats.get().stats.by_leg;
                        if rows.is_empty() {
                            return view! {
                                <tr>
                                    <td colspan="7" class="muted">"这个窗口里还没有账（没流量）"</td>
                                </tr>
                            }.into_any();
                        }
                        rows.into_iter()
                            .map(|l| {
                                let name = l
                                    .leg_name
                                    .clone()
                                    .unwrap_or_else(|| format!("#{}", l.leg_id));
                                view! {
                                    <tr>
                                        <td>{name}</td>
                                        <td class="num">{fmt::num(l.attempts)}</td>
                                        <td class="num">{fmt::num(l.successes)}</td>
                                        <td class="num">
                                            {l
                                                .success_rate
                                                .map(|r| format!("{r:.1}%"))
                                                .unwrap_or_else(|| "—".into())}
                                        </td>
                                        <td class="num">
                                            {if l.leg_faults > 0 {
                                                view! { <span class="bad-text">{fmt::num(l.leg_faults)}</span> }.into_any()
                                            } else {
                                                view! { <span class="muted">"0"</span> }.into_any()
                                            }}
                                        </td>
                                        <td class="num">{fmt::dur(l.avg_ms)}</td>
                                        <td class="num">
                                            {fmt::num(l.input_tokens + l.output_tokens)}
                                        </td>
                                    </tr>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </tbody>
            </table>

            <h2>"能被调用的模型名"</h2>
            <p class="muted small">
                "客户端填这些名字才打得通（和 " <code>"/v1/models"</code>
                " 用的是同一份判断：组是开着的，且组里至少有一条可用腿）。"
                "标了" <span class="tag vision">"视觉"</span>
                "的名字后面有能看图的腿 —— 带图请求要打这类名字，"
                "打到没标的会回 422（说清楚，不悄悄丢图）。"
            </p>
            <div class="chips">
                {move || {
                    let v = servable.get();
                    if v.is_empty() {
                        return view! { <span class="muted">"一个都没有 —— 先去「模型组」建一个"</span> }
                            .into_any();
                    }
                    v.into_iter()
                        .map(|m| {
                            view! {
                                <code class="chip code" title=if m.vision { "收得下图片输入" } else { "纯文本：带图的请求会被回 422" }>
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

#[component]
fn Card(
    title: &'static str,
    value: Signal<String>,
    sub: Signal<String>,
) -> impl IntoView {
    view! {
        <div class="card">
            <div class="card-title">{title}</div>
            <div class="card-value">{move || value.get()}</div>
            <div class="card-sub muted small">{move || sub.get()}</div>
        </div>
    }
}

/// 流水页也用得到的「状态」小标签在这里定义一次。
#[component]
pub fn StatusTag(status: String) -> impl IntoView {
    let (label, class) = status_label(&status);
    view! { <span class=format!("tag {class}")>{label}</span> }
}
