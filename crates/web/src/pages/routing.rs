//! 路由状态：现在到底在走哪条渠道、什么时候会回头试首选、切了几次，以及路由参数。
//!
//! 这一页回答的是"为什么现在走的是这条渠道" —— 出问题时第一个要看的页。
//! 「手动回切」把检查点归零，不用干等 10 分钟。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{Channel, RoutingCfg, RoutingGroup};
use crate::Ctx;

#[component]
pub fn Routing(ctx: Ctx) -> impl IntoView {
    let rows = RwSignal::new(Vec::<RoutingGroup>::new());
    let channels = RwSignal::new(Vec::<Channel>::new());
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    // 参数表单
    let f_retry = RwSignal::new(3i64);
    let f_timeout = RwSignal::new(30_000i64);
    let f_budget = RwSignal::new(90_000i64);
    let f_reprobe_min = RwSignal::new(10i64);
    let f_probe = RwSignal::new(true);
    let cfg_loaded = RwSignal::new(false);

    let reload = move || {
        busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api::routing().await {
                Ok(r) => {
                    // 第一次拉到才填表单：否则用户正在改的数字会被刷新覆盖掉
                    if !cfg_loaded.get_untracked() {
                        f_retry.set(r.config.retry_times as i64);
                        f_timeout.set(r.config.attempt_timeout_ms as i64);
                        f_budget.set(r.config.request_budget_ms as i64);
                        f_reprobe_min.set(r.config.reprobe_minutes());
                        f_probe.set(r.config.probe_enabled);
                        cfg_loaded.set(true);
                    }
                    rows.set(r.groups);
                    err.set(None);
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
            .map(|l| format!("{} (#{i})", l.name))
            .unwrap_or_else(|| format!("已删除的渠道 #{i}")),
    };

    let do_reprobe = move |g: String| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::reprobe(&g).await {
                Ok(()) => {
                    ctx.say(format!("{g}：下次请求会先试首选渠道"));
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("回切失败：{}", e.message));
                }
            }
        });
    };

    let save_cfg = move |_| {
        let cfg = RoutingCfg {
            retry_times: f_retry.get_untracked().clamp(0, 20) as u32,
            attempt_timeout_ms: f_timeout.get_untracked().clamp(1, 600_000) as u64,
            request_budget_ms: f_budget.get_untracked().clamp(1, 3_600_000) as u64,
            reprobe_interval_ms: f_reprobe_min.get_untracked().clamp(0, 24 * 60) * 60_000,
            probe_enabled: f_probe.get_untracked(),
        };
        wasm_bindgen_futures::spawn_local(async move {
            match api::save_routing_config(&cfg).await {
                Ok(()) => {
                    ctx.say("路由参数已保存，立即生效");
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("保存失败：{}", e.message));
                }
            }
        });
    };

    view! {
        <div class="page">
            <div class="toolbar">
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
                <span class="muted small">
                    "切换后隔多久回头试首选，由下面「回切间隔」决定；不想等就点「现在试回切」。"
                </span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <table class="grid">
                <thead>
                    <tr>
                        <th>"模型组"</th>
                        <th>"首选渠道"</th>
                        <th>"当前在用"</th>
                        <th>"状态"</th>
                        <th class="num">"切换次数"</th>
                        <th>"上次切换"</th>
                        <th>"下次回切"</th>
                        <th>"操作"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        let v = rows.get();
                        if v.is_empty() {
                            return view! {
                                <tr class="empty-row">
                                    <td colspan="8" class="muted empty-cell">"还没有模型组"</td>
                                </tr>
                            }.into_any();
                        }
                        v.into_iter()
                            .map(|g| {
                                let name = g.group.clone();
                                let g2 = g.clone();
                                let active = channel_name(g.active_channel_id);
                                let primary = channel_name(g.primary_channel_id);
                                let switched = g.switched;
                                let next = g.next_probe_at_ms.map(|t| fmt::ago(g.now_ms, t));
                                let last = g.last_switch_at_ms.map(|t| fmt::ago(g.now_ms, t));
                                view! {
                                    <tr>
                                        <td data-label="模型组">
                                            <b>{name.clone()}</b>
                                            {(!g.enabled).then(|| view! { <span class="tag muted">"停用"</span> })}
                                        </td>
                                        <td data-label="首选渠道">{primary}</td>
                                        <td data-label="当前在用">
                                            {active}
                                            {switched.then(|| view! { <span class="tag warn">"已切换"</span> })}
                                        </td>
                                        <td data-label="状态">
                                            {if switched {
                                                view! { <span class="tag warn">"没走首选"</span> }.into_any()
                                            } else {
                                                view! { <span class="tag ok">"走首选"</span> }.into_any()
                                            }}
                                        </td>
                                        <td class="num" data-label="切换次数">{g.switch_count}</td>
                                        <td data-label="上次切换">{last.unwrap_or_else(|| "—".into())}</td>
                                        <td data-label="下次回切">{next.unwrap_or_else(|| "—".into())}</td>
                                        <td class="actions">
                                            <button class="ghost" on:click=move |_| do_reprobe(g2.group.clone())>
                                                "现在试回切"
                                            </button>
                                        </td>
                                    </tr>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </tbody>
            </table>

            <h2>"路由参数"</h2>
            <p class="muted small">
                "「重试次数」是首次失败后再试几次：填 3 = 单条渠道最多试 4 次，全败才换渠道。"
            </p>
            <div class="panel">
                <div class="form-grid">
                    <label>"重试次数（0–20）"</label>
                    <input type="number" min="0" max="20" prop:value=f_retry
                        on:input=move |ev| f_retry.set(event_target_value(&ev).parse().unwrap_or(0)) />

                    <label>"单次尝试超时（毫秒）"</label>
                    <input type="number" min="1" prop:value=f_timeout
                        on:input=move |ev| f_timeout.set(event_target_value(&ev).parse().unwrap_or(30_000)) />

                    <label>"整个请求的重试预算（毫秒）"</label>
                    <div>
                        <input type="number" min="1" prop:value=f_budget
                            on:input=move |ev| f_budget.set(event_target_value(&ev).parse().unwrap_or(90_000)) />
                        <div class="muted small">"超了这个预算就不再重试，直接把错误回给客户端。"</div>
                    </div>

                    <label>"回切间隔（分钟）"</label>
                    <div>
                        <input type="number" min="0" prop:value=f_reprobe_min
                            on:input=move |ev| f_reprobe_min.set(event_target_value(&ev).parse().unwrap_or(10)) />
                        <div class="muted small">"切换之后隔这么久回头试首选，成了就切回去。"</div>
                    </div>

                    <label>"没流量时后台探活"</label>
                    <div>
                        <input type="checkbox" prop:checked=f_probe
                            on:change=move |ev| f_probe.set(event_target_checked(&ev)) />
                        <span class="muted small">
                            "开着才不会「夜里没请求就永远不回切」。探活不写记账、不动路由状态。"
                        </span>
                    </div>
                </div>
                <div class="modal-foot">
                    <button class="primary" on:click=save_cfg>"保存参数"</button>
                </div>
            </div>
        </div>
    }
}
