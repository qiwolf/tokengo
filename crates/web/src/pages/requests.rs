//! 请求流水：最近这些请求走了哪条渠道、成没成、多久、错在哪。
//!
//! 「自动刷新」用的是浏览器定时器（不是轮询服务端），关掉页面就停，不会在后台空转。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{LogRow, Channel};
use crate::pages::PageHead;
use crate::Ctx;

#[component]
pub fn Requests(ctx: Ctx) -> impl IntoView {
    let rows = RwSignal::new(Vec::<LogRow>::new());
    let channels = RwSignal::new(Vec::<Channel>::new());
    let limit = RwSignal::new(100u32);
    let auto = RwSignal::new(false);
    let timer = RwSignal::new(0i32);
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let last_at = RwSignal::new(0i64);

    let reload = move || {
        busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api::logs(limit.get_untracked()).await {
                Ok(v) => {
                    rows.set(v);
                    err.set(None);
                    // 拿最新一条请求的时间当"数据截止"，比显示"刚刷新"诚实
                    last_at.set(crate::now_ms());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(Some(e.message));
                }
            }
            match api::channels().await {
                Ok(v) => channels.set(v),
                Err(e) => ctx.note_err(&e),
            }
            busy.set(false);
        });
    };
    reload();

    let channel_name = move |id: Option<i64>| match id {
        None => "—".to_string(),
        Some(i) => channels
            .get()
            .iter()
            .find(|l| l.id == i)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("已删 #{i}")),
    };

    // 自动刷新开关：开就起一个 5 秒定时器，关就停掉
    let toggle_auto = move |on: bool| {
        auto.set(on);
        let id = timer.get_untracked();
        if on {
            let handle = crate::every(5000, move || reload());
            timer.set(handle);
        } else if id != 0 {
            crate::stop_timer(id);
            timer.set(0);
        }
    };

    view! {
        <div class="page">
            <PageHead
                title="请求"
                desc="每个请求走了哪条渠道、成没成、花了多久、错在哪。"
            />
            <div class="toolbar">
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
                <span class="spacer"></span>
                <span class="muted small">"显示最近"</span>
                {[50u32, 100, 500]
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
                            {format!("{n} 条")}
                        </button>
                    })
                    .collect_view()}
                <button
                    class="chip"
                    class:active=move || auto.get()
                    on:click=move |_| toggle_auto(!auto.get_untracked())
                >
                    {move || if auto.get() { "自动刷新：开（5 秒）" } else { "自动刷新：关" }}
                </button>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <p class="muted small">
                {move || if last_at.get() > 0 {
                    format!("数据截止：{}", fmt::ts(last_at.get()))
                } else {
                    "还没有数据".to_string()
                }}
            </p>

            <div class="table-wrap">
            <table class="grid">
                <thead>
                    <tr>
                        <th>"时间"</th>
                        <th>"端点"</th>
                        <th>"模型"</th>
                        <th>"走的渠道"</th>
                        <th>"结果"</th>
                        <th class="num">"HTTP"</th>
                        <th class="num">"耗时"</th>
                        <th class="num">"入/出 token"</th>
                        <th>"错误"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        let v = rows.get();
                        if v.is_empty() {
                            return view! {
                                <tr class="empty-row">
                                    <td colspan="9" class="muted empty-cell">
                                        "还没有请求进来。客户端打 /v1/chat/completions 或 /v1/embeddings 就会出现在这里。"
                                    </td>
                                </tr>
                            }.into_any();
                        }
                        v.into_iter()
                            .map(|r| {
                                let (label, class) = crate::model::status_label(&r.status);
                                let channel = channel_name(r.channel_id);
                                let endpoint = crate::model::endpoint_label(&r.endpoint).to_string();
                                view! {
                                    // data-label 是手机上用的：那一档表格变成卡片，标签从这里取
                                    <tr>
                                        <td class="nowrap small" data-label="时间">{fmt::ts_short(r.ts_ms)}</td>
                                        <td data-label="端点">{endpoint}</td>
                                        <td data-label="模型">
                                            <code class="small">{r.model.clone()}</code>
                                        </td>
                                        <td data-label="走的渠道">{channel}</td>
                                        <td data-label="结果">
                                            <span class=format!("tag {class}")>{label}</span>
                                        </td>
                                        <td class="num" data-label="HTTP">
                                            {r.http_status.map(|c| c.to_string()).unwrap_or_else(|| "—".into())}
                                        </td>
                                        <td class="num" data-label="耗时">{fmt::dur(r.latency_ms)}</td>
                                        <td class="num" data-label="入/出 token">
                                            {format!("{} / {}", fmt::num(r.input_tokens), fmt::num(r.output_tokens))}
                                        </td>
                                        <td class="err-cell small" data-label="错误">
                                            {r.error.clone().unwrap_or_default()}
                                        </td>
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
    }
}
