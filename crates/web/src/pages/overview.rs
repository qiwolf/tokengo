//! 总览：一段时间的账 + 最近调用 + 各条渠道的分布 + 能被调用的模型名。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{LogRow, NameRoute, StatsResp, status_label};
use crate::pages::PageHead;
use crate::Ctx;

#[component]
pub fn Overview(ctx: Ctx) -> impl IntoView {
    let (hours, set_hours) = signal(24i64);
    let stats = RwSignal::new(StatsResp::default());
    let callable = RwSignal::new(Vec::<NameRoute>::new());
    // 客户端清单里有没有 auto（一个模型都没勾选时它不出现）
    let auto_ready = RwSignal::new(false);
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
            match api::model_routes().await {
                Ok(r) => {
                    callable.set(r.models);
                    auto_ready.set(r.auto.available);
                }
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
                <span class="seg">
                    <span class="seg-label">"时间窗口"</span>
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
                </span>
                <span class="spacer"></span>
                <span class="muted small">"在线会话 " <b>{move || sessions.get()}</b></span>
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

            <section class="card">
                <header class="card-head">
            <h2>"最近调用"</h2>
            <p class="muted small">"最新进来的请求 —— 哪个模型、走了哪条渠道、成没成、花了多久。"</p>
                </header>
                <div class="card-body flush">
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
                                            "还没有请求进来。有程序往 /v1/chat/completions 或 /v1/embeddings 发请求，就会出现在这里。"
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
                                            <td data-label="时间" class="nowrap small">{fmt::ts_short(r.ts_ms)}</td>
                                            <td data-label="模型"><code class="small">{r.model.clone()}</code></td>
                                            <td data-label="走的渠道">{channel}</td>
                                            <td data-label="结果"><span class=format!("tag {class}")>{label}</span></td>
                                            <td data-label="耗时" class="num">{fmt::dur(r.latency_ms)}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </tbody>
                </table>
            </div>
                </div>
            </section>

            <section class="card">
                <header class="card-head">
            <h2>"按类型"</h2>
            <p class="muted small">
                "对话 / 向量 / 重排分开看 —— 三条路各有各的用法，混在一起看不出哪条不稳。"
                "归类看的是这条请求当时实际打的端点，不是这个名字现在的用途（用途随时能改，"
                "改了不该让历史账跟着变）。耗时只算成功的那些。"
            </p>
                </header>
                <div class="card-body flush">
            <div class="table-wrap">
                <table class="grid">
                    <thead>
                        <tr>
                            <th>"类型"</th>
                            <th class="num">"请求数"</th>
                            <th class="num">"成功率"</th>
                            <th class="num">"成功"</th>
                            <th class="num">"失败"</th>
                            <th class="num">"客户端错"</th>
                            <th class="num">"平均耗时"</th>
                            <th class="num">"Token"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let rows = stats.get().stats.by_kind;
                            if rows.is_empty() {
                                return view! {
                                    <tr class="empty-row">
                                        <td colspan="8" class="muted empty-cell">
                                            "这个窗口里还没有账（没流量）"
                                        </td>
                                    </tr>
                                }
                                .into_any();
                            }
                            rows.into_iter()
                                .map(|k| {
                                    let rate = k.success_rate();
                                    view! {
                                        <tr>
                                            <td data-label="类型">{k.label.clone()}</td>
                                            <td data-label="请求数" class="num">{fmt::num(k.requests)}</td>
                                            <td data-label="成功率" class="num">
                                                {if k.requests > 0 {
                                                    format!("{rate:.1}%")
                                                } else {
                                                    "—".to_string()
                                                }}
                                            </td>
                                            <td data-label="成功" class="num">{fmt::num(k.ok)}</td>
                                            <td data-label="失败" class="num">
                                                {if k.failed > 0 {
                                                    view! { <span class="bad-text">{fmt::num(k.failed)}</span> }.into_any()
                                                } else {
                                                    view! { <span class="muted">"0"</span> }.into_any()
                                                }}
                                            </td>
                                            <td data-label="客户端错" class="num">{fmt::num(k.client_errors)}</td>
                                            <td data-label="平均耗时" class="num">
                                                {if k.ok > 0 { fmt::dur(k.avg_ms) } else { "—".to_string() }}
                                            </td>
                                            <td data-label="Token" class="num">{fmt::num(k.input_tokens + k.output_tokens)}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </tbody>
                </table>
            </div>
                </div>
            </section>

            <section class="card">
                <header class="card-head">
            <h2>"各条渠道的账"</h2>
            <p class="muted small">
                "分母是「被叫了几次」，不是「成功了几次」—— 判断一条渠道值不值得留就看这个。"
            </p>
                </header>
                <div class="card-body flush">
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
                                            <td data-label="渠道">{name}</td>
                                            <td data-label="被叫" class="num">{fmt::num(l.attempts)}</td>
                                            <td data-label="成功" class="num">{fmt::num(l.successes)}</td>
                                            <td data-label="成功率" class="num">
                                                {l.success_rate.map(|r| format!("{r:.1}%")).unwrap_or_else(|| "—".into())}
                                            </td>
                                            <td data-label="渠道故障" class="num">
                                                {if l.channel_faults > 0 {
                                                    view! { <span class="bad-text">{fmt::num(l.channel_faults)}</span> }.into_any()
                                                } else {
                                                    view! { <span class="muted">"0"</span> }.into_any()
                                                }}
                                            </td>
                                            <td data-label="平均耗时" class="num">{fmt::dur(l.avg_ms)}</td>
                                            <td data-label="Token" class="num">{fmt::num(l.input_tokens + l.output_tokens)}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </tbody>
                </table>
            </div>
                </div>
            </section>

            <section class="card">
                <header class="card-head">
            <h2>"按模型名"</h2>
            <p class="muted small">
                "看的是别人请求了哪个名字 —— 哪个名字最常被用、哪个在报错，一眼能看出来。"
            </p>
                </header>
                <div class="card-body flush">
            <div class="table-wrap">
                <table class="grid">
                    <thead>
                        <tr>
                            <th>"模型名"</th>
                            <th class="num">"请求数"</th>
                            <th class="num">"成功"</th>
                            <th class="num">"失败"</th>
                            <th class="num">"客户端错"</th>
                            <th class="num">"Token"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let rows = stats.get().stats.by_model;
                            if rows.is_empty() {
                                return view! {
                                    <tr class="empty-row">
                                        <td colspan="6" class="muted empty-cell">"这个窗口里还没有账"</td>
                                    </tr>
                                }
                                .into_any();
                            }
                            rows.into_iter()
                                .map(|m| {
                                    view! {
                                        <tr>
                                            <td data-label="模型名"><code>{m.model}</code></td>
                                            <td data-label="请求" class="num">{fmt::num(m.requests)}</td>
                                            <td data-label="成功" class="num">{fmt::num(m.ok)}</td>
                                            <td data-label="失败" class="num">
                                                {if m.failed > 0 {
                                                    view! { <span class="bad-text">{fmt::num(m.failed)}</span> }.into_any()
                                                } else {
                                                    view! { <span class="muted">"0"</span> }.into_any()
                                                }}
                                            </td>
                                            <td data-label="客户端错" class="num">{fmt::num(m.client_errors)}</td>
                                            <td data-label="Token" class="num">{fmt::num(m.input_tokens + m.output_tokens)}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </tbody>
                </table>
            </div>
                </div>
            </section>

            <section class="card">
                <header class="card-head">
            <h2>"按客户端"</h2>
            <p class="muted small">
                "谁在调这个网关、各自多少请求。没取到来源地址的单独占一行（显示「未知」），不藏起来。"
            </p>
                </header>
                <div class="card-body flush">
            <div class="table-wrap">
                <table class="grid">
                    <thead>
                        <tr>
                            <th>"来源"</th>
                            <th class="num">"请求数"</th>
                            <th class="num">"成功"</th>
                            <th class="num">"失败"</th>
                            <th class="num">"客户端错"</th>
                            <th class="num">"Token"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let rows = stats.get().stats.by_client;
                            if rows.is_empty() {
                                return view! {
                                    <tr class="empty-row">
                                        <td colspan="6" class="muted empty-cell">"这个窗口里还没有账"</td>
                                    </tr>
                                }
                                .into_any();
                            }
                            rows.into_iter()
                                .map(|c| {
                                    view! {
                                        <tr>
                                            <td data-label="来源">
                                                <code>{c.client_ip.unwrap_or_else(|| "未知".into())}</code>
                                            </td>
                                            <td data-label="请求" class="num">{fmt::num(c.requests)}</td>
                                            <td data-label="成功" class="num">{fmt::num(c.ok)}</td>
                                            <td data-label="失败" class="num">
                                                {if c.failed > 0 {
                                                    view! { <span class="bad-text">{fmt::num(c.failed)}</span> }.into_any()
                                                } else {
                                                    view! { <span class="muted">"0"</span> }.into_any()
                                                }}
                                            </td>
                                            <td data-label="客户端错" class="num">{fmt::num(c.client_errors)}</td>
                                            <td data-label="Token" class="num">{fmt::num(c.input_tokens + c.output_tokens)}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </tbody>
                </table>
            </div>
                </div>
            </section>

            <section class="card">
                <header class="card-head">
            <h2>"错误分布"</h2>
            <p class="muted small">
                "失败和客户端错各自是什么原因、各多少次。没记原因的也占一行，免得「一堆失败却看不到原因」。"
            </p>
                </header>
                <div class="card-body flush">
            <div class="table-wrap">
                <table class="grid">
                    <thead>
                        <tr>
                            <th>"原因"</th>
                            <th class="num">"次数"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let rows = stats.get().stats.errors;
                            if rows.is_empty() {
                                return view! {
                                    <tr class="empty-row">
                                        <td colspan="2" class="muted empty-cell">"这个窗口里没有失败"</td>
                                    </tr>
                                }
                                .into_any();
                            }
                            rows.into_iter()
                                .map(|e| {
                                    view! {
                                        <tr>
                                            <td data-label="原因" class="small">{e.error}</td>
                                            <td data-label="次数" class="num">{fmt::num(e.count)}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </tbody>
                </table>
            </div>
                </div>
            </section>

            <section class="card">
                <header class="card-head">
            <h2>"能被调用的模型名"</h2>
            <p class="muted small">
                "客户端填这些名字才打得通（和 "
                <code>"/v1/models"</code>
                " 用的是同一份判断：渠道里勾过它、并且至少有一条启用的渠道在服务它）。"
                "标了"
                <span class="tag vision">"视觉"</span>
                "的名字后面有能看图的渠道 —— 带图请求要打这类名字，"
                "打到没标的会回 422（说清楚，不悄悄丢图）。"
                "名字不用在这儿建：在「渠道」页勾上就出现。"
            </p>
                </header>
                <div class="card-body">
            <div class="chips">
                {move || {
                    let v: Vec<NameRoute> = callable
                        .get()
                        .into_iter()
                        .filter(|m| m.routed)
                        .collect();
                    if v.is_empty() {
                        return view! {
                            <span class="muted">
                                "一个都没有 —— 先去「渠道」页勾上要用的模型名"
                            </span>
                        }
                            .into_any();
                    }
                    // auto 是一个保留名：勾选过的模型一个都没有时它不出现，这里也不列。
                    let auto_chip = if auto_ready.get() {
                        view! {
                            <code
                                class="chip code"
                                title="保留名：客户端打它就在你勾选过的模型里挑一个最好的"
                            >
                                "auto"
                                <span class="tag">"自动挑"</span>
                            </code>
                        }
                            .into_any()
                    } else {
                        ().into_any()
                    };
                    view! {
                        <>
                            {auto_chip}
                            {v
                                .into_iter()
                                .map(|m| {
                                    view! {
                                        <code
                                            class="chip code"
                                            title=if m.can_see {
                                                "收得下图片输入"
                                            } else {
                                                "纯文本：带图的请求会被回 422"
                                            }
                                        >
                                            {m.name}
                                            {m
                                                .can_see
                                                .then(|| view! { <span class="tag vision">"视觉"</span> })}
                                        </code>
                                    }
                                        .into_any()
                                })
                                .collect_view()}
                        </>
                    }
                        .into_any()
                }}
            </div>
                </div>
            </section>
        </div>
    }
}

/// 流水页也用得到的「状态」小标签在这里定义一次。
#[component]
pub fn StatusTag(status: String) -> impl IntoView {
    let (label, class) = status_label(&status);
    view! { <span class=format!("tag {class}")>{label}</span> }
}

#[cfg(test)]
mod tests {
    use crate::model::KindStat;

    /// 成功率的分子分母要跟总览四张卡一个口径：分母是「打进来几次」，不是「试了几次」。
    #[test]
    fn kind_success_rate_uses_requests_as_the_denominator() {
        let mut k = KindStat {
            kind: "embeddings".into(),
            label: "向量".into(),
            requests: 8,
            ok: 7,
            ..Default::default()
        };
        assert_eq!(k.success_rate(), 87.5);
        k.requests = 0;
        assert_eq!(k.success_rate(), 0.0, "没请求时面板显示「—」，不该算出个百分比");
        // 全成功 / 全失败两个边界
        k.requests = 3;
        k.ok = 3;
        assert_eq!(k.success_rate(), 100.0);
        k.ok = 0;
        assert_eq!(k.success_rate(), 0.0);
    }
}
