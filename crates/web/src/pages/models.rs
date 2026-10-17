//! 模型：客户端打过来的一个模型名 = 一串按顺序试的渠道。
//!
//! 原先把「模型组」和「路由状态」拆成两页，看的人得来回翻才知道
//! 「这个模型名现在到底在走哪条渠道」—— 现在并成一页：
//! 上面一张表看每个模型名**现在**怎么样（在走哪条、换过几次、什么时候回头试首选），
//! 下面是「调整顺序」和「路由参数」。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{Channel, Group, RoutingCfg, RoutingGroup, Servable};
use crate::pages::PageHead;
use crate::Ctx;

/// 顶部那一排「客户端能打的模型名」要显示什么：勾了「只看含视觉的」就只留能收图的。
/// 抽成纯函数是为了能测 —— 面板的筛选逻辑一旦写错，人看到的是「明明标了视觉却不显示」。
pub(crate) fn filter_servable(all: &[Servable], only_vision: bool) -> Vec<Servable> {
    all.iter()
        .filter(|m| !only_vision || m.vision)
        .cloned()
        .collect()
}

/// 正在编辑的组（草稿状态，保存前只存在浏览器里）。
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    /// `None` = 新建
    original: Option<String>,
    name: String,
    enabled: bool,
    channels: Vec<i64>,
}

#[component]
pub fn Models(ctx: Ctx) -> impl IntoView {
    let groups = RwSignal::new(Vec::<Group>::new());
    let channels = RwSignal::new(Vec::<Channel>::new());
    let routes = RwSignal::new(Vec::<RoutingGroup>::new());
    let servable = RwSignal::new(Vec::<Servable>::new());
    // 「只看含视觉的」：模型多了以后，一眼找出哪个模型名能收图
    let only_vision = RwSignal::new(false);
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let draft = RwSignal::new(None::<Draft>);

    // 路由参数表单
    let f_retry = RwSignal::new(3i64);
    let f_timeout = RwSignal::new(30_000i64);
    let f_budget = RwSignal::new(90_000i64);
    let f_reprobe_min = RwSignal::new(10i64);
    let f_probe = RwSignal::new(true);
    let cfg_loaded = RwSignal::new(false);

    let reload = move || {
        busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api::groups().await {
                Ok(g) => {
                    groups.set(g.groups);
                    servable.set(g.servable);
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
                    routes.set(r.groups);
                }
                Err(e) => {
                    ctx.note_err(&e);
                }
            }
            busy.set(false);
        });
    };
    reload();

    let channel_name = move |id: i64| {
        channels
            .get()
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("已删除的渠道 #{id}"))
    };
    let channel_name_opt = move |id: Option<i64>| match id {
        None => "—".to_string(),
        Some(i) => channels
            .get()
            .iter()
            .find(|l| l.id == i)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("已删除的渠道 #{i}")),
    };

    let open_new = move |_| {
        draft.set(Some(Draft {
            original: None,
            name: String::new(),
            enabled: true,
            channels: Vec::new(),
        }));
    };

    let open_edit = move |g: Group| {
        draft.set(Some(Draft {
            original: Some(g.name.clone()),
            name: g.name.clone(),
            enabled: g.enabled,
            channels: g.channels.clone(),
        }));
    };

    let save = move |_| {
        let Some(d) = draft.get_untracked() else {
            return;
        };
        if d.name.trim().is_empty() {
            ctx.say("模型名不能空（客户端就是拿这个名字打过来的）");
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match api::save_group(d.name.trim(), d.enabled, &d.channels).await {
                Ok(()) => {
                    draft.set(None);
                    ctx.say("模型已保存");
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("保存失败：{}", e.message));
                }
            }
        });
    };

    let remove = move |g: Group| {
        let ok = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!(
                    "确定删掉模型「{}」？客户端再用这个名字会拿到 404。",
                    g.name
                ))
                .ok()
            })
            .unwrap_or(false);
        if !ok {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match api::delete_group(&g.name).await {
                Ok(()) => {
                    ctx.say("模型已删除");
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("删除失败：{}", e.message));
                }
            }
        });
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

    let move_channel = move |i: usize, up: bool| {
        draft.update(|d| {
            if let Some(d) = d {
                let j = if up {
                    i.wrapping_sub(1)
                } else {
                    i + 1
                };
                if i < d.channels.len() && j < d.channels.len() {
                    d.channels.swap(i, j);
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
            <PageHead
                title="模型"
                desc="你客户端里填的模型名。收到请求就按下面的顺序依次试渠道，第一条不通就换下一条。"
            >
                <>
                    <button class="primary" on:click=open_new>"新建模型"</button>
                    <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                        {move || if busy.get() { "刷新中…" } else { "刷新" }}
                    </button>
                </>
            </PageHead>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <div class="toolbar">
                <span class="muted small">"客户端能打的模型名："</span>
                {move || {
                    let v = filter_servable(&servable.get(), only_vision.get());
                    if v.is_empty() {
                        return view! { <span class="muted small">"（一个都没有）"</span> }.into_any();
                    }
                    v.into_iter()
                        .map(|m| {
                            view! {
                                <code
                                    class="chip code"
                                    title=if m.vision {
                                        "这个模型名收得下图片输入"
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
                <span class="spacer"></span>
                <label class="pick" title="只看后面有「能看图」渠道的模型名">
                    <input
                        type="checkbox"
                        prop:checked=move || only_vision.get()
                        on:change=move |ev| only_vision.set(event_target_checked(&ev))
                    />
                    "只看含视觉的"
                </label>
            </div>

            <div class="table-wrap">
                <table class="grid">
                    <thead>
                        <tr>
                            <th>"模型名"</th>
                            <th>"状态"</th>
                            <th>"渠道顺序（先试 → 后试）"</th>
                            <th>"现在走的"</th>
                            <th class="num">"换过几次"</th>
                            <th>"下次回头试首选"</th>
                            <th>"操作"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let gs = groups.get();
                            if gs.is_empty() {
                                return view! {
                                    <tr class="empty-row">
                                        <td colspan="7" class="muted empty-cell">
                                            "还没有模型 —— 点右上角「新建模型」加一个，客户端才能打模型名。"
                                        </td>
                                    </tr>
                                }
                                .into_any();
                            }
                            gs.into_iter()
                                .map(|g| {
                                    let name = g.name.clone();
                                    let g2 = g.clone();
                                    let g3 = g.clone();
                                    let g4 = g.clone();
                                    let r = routes.get().into_iter().find(|x| x.group == g.name);
                                    let active = r.as_ref().map(|x| channel_name_opt(x.active_channel_id)).unwrap_or_else(|| "—".to_string());
                                    let switched = r.as_ref().map(|x| x.switched).unwrap_or(false);
                                    let count = r.as_ref().map(|x| x.switch_count).unwrap_or(0);
                                    let next = r.as_ref().and_then(|x| x.next_probe_at_ms.map(|t| fmt::ago(x.now_ms, t))).unwrap_or_else(|| "—".to_string());
                                    let channels_txt = g
                                        .channels
                                        .iter()
                                        .map(|id| channel_name(*id))
                                        .collect::<Vec<_>>()
                                        .join(" → ");
                                    let channels_txt = if channels_txt.is_empty() {
                                        "还没配渠道".to_string()
                                    } else {
                                        channels_txt
                                    };
                                    view! {
                                        <tr>
                                            <td data-label="模型名">
                                                <b>{name.clone()}</b>
                                                {g.can_see.then(|| view! {
                                                    <span class="tag vision" title="这一组里有「能看图」的渠道，带图的请求走得通">
                                                        "视觉"
                                                    </span>
                                                })}
                                                {(!g.can_see).then(|| view! {
                                                    <span class="tag muted" title="这一组里没有能看图的渠道：带图的请求会被回 422（说清楚，不悄悄丢图）">
                                                        "不收图"
                                                    </span>
                                                })}
                                            </td>
                                            <td data-label="状态">
                                                {if g.enabled {
                                                    view! { <span class="tag ok">"启用"</span> }.into_any()
                                                } else {
                                                    view! { <span class="tag muted">"停用"</span> }.into_any()
                                                }}
                                            </td>
                                            <td data-label="渠道顺序（先试 → 后试）">{channels_txt}</td>
                                            <td data-label="现在走的">
                                                {active}
                                                {switched.then(|| view! { <span class="tag warn">"没走首选"</span> })}
                                            </td>
                                            <td class="num" data-label="换过几次">{count}</td>
                                            <td data-label="下次回头试首选">{next}</td>
                                            <td class="actions">
                                                <button class="ghost" on:click=move |_| open_edit(g2.clone())>
                                                    "调整顺序"
                                                </button>
                                                <button
                                                    class="ghost"
                                                    on:click=move |_| do_reprobe(g4.name.clone())
                                                >
                                                    "立刻试首选"
                                                </button>
                                                <button
                                                    class="ghost danger"
                                                    on:click=move |_| remove(g3.clone())
                                                >
                                                    "删除"
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
            </div>

            // 编排面板（草稿状态，保存前只存在浏览器里）
                    {move || match draft.get() {
                        None => view! {
                            <p class="muted">"左边点「编排」改一个组的优先级，或点「新建模型组」。"</p>
                        }.into_any(),
                        Some(d) => {
                            let is_new = d.original.is_none();
                            let d2 = d.clone();
                            let d3 = d.clone();
                            view! {
                                <div class="panel">
                                    <div class="form-grid">
                                        <label>"组名"</label>
                                        <div>
                                            <input type="text" prop:value=d.name.clone()
                                                disabled=!is_new
                                                placeholder="如 auto（你客户端里填的就是这个名字）"
                                                on:input=move |ev| {
                                                    let s = event_target_value(&ev);
                                                    draft.update(|d| {
                                                        if let Some(d) = d {
                                                            d.name = s;
                                                        }
                                                    });
                                                } />
                                            {(!is_new).then(|| view! {
                                                <div class="muted small">
                                                    "这个名字就是你客户端里填的模型名，改名等于换一个组："
                                                    "要改名请新建一个，再把旧的删掉。"
                                                </div>
                                            })}
                                        </div>
                                        <label>"启用"</label>
                                        <div>
                                            <input type="checkbox" prop:checked=d.enabled
                                                on:change=move |ev| {
                                                    let b = event_target_checked(&ev);
                                                    draft.update(|d| {
                                                        if let Some(d) = d {
                                                            d.enabled = b;
                                                        }
                                                    });
                                                } />
                                            <span class="muted small">"停用后这个名字不再对外提供服务"</span>
                                        </div>
                                    </div>

                                    <h3>"顺序即优先级（上面的先用）"</h3>
                                    <table class="grid tight">
                                        <tbody>
                                            {move || {
                                                let Some(d) = draft.get() else {
                                                    return ().into_any();
                                                };
                                                if d.channels.is_empty() {
                                                    return view! {
                                                        <tr>
                                                            <td class="muted">"还没选渠道 —— 下面挑一条加进来"</td>
                                                        </tr>
                                                    }.into_any();
                                                }
                                                d.channels
                                                    .iter()
                                                    .enumerate()
                                                    .map(|(i, id)| {
                                                        let id = *id;
                                                        let n = channel_name(id);
                                                        let last = i + 1 == d.channels.len();
                                                        view! {
                                                            <tr>
                                                                <td class="num muted">{i + 1}</td>
                                                                <td>
                                                                    {n.clone()}
                                                                    <span class="muted small">{format!(" #{id}")}</span>
                                                                </td>
                                                                <td class="actions">
                                                                    <button class="ghost" disabled=i == 0
                                                                        on:click=move |_| move_channel(i, true)>"↑"</button>
                                                                    <button class="ghost" disabled=last
                                                                        on:click=move |_| move_channel(i, false)>"↓"</button>
                                                                    <button class="ghost danger" on:click=move |_| {
                                                                        draft.update(|d| {
                                                                            if let Some(d) = d {
                                                                                d.channels.remove(i);
                                                                            }
                                                                        });
                                                                    }>"移除"</button>
                                                                </td>
                                                            </tr>
                                                        }
                                                    })
                                                    .collect_view()
                                                    .into_any()
                                            }}
                                        </tbody>
                                    </table>

                                    <h3>"加一条渠道"</h3>
                                    <p class="muted small">
                                        "点一下加进来。手机上不再用下拉框 —— 那种一次只看得到一条，挑起来太慢。"
                                    </p>
                                    {move || {
                                        let in_draft: Vec<i64> = draft
                                            .get()
                                            .map(|d| d.channels)
                                            .unwrap_or_default();
                                        let avail: Vec<Channel> = channels
                                            .get()
                                            .into_iter()
                                            .filter(|l| !in_draft.contains(&l.id))
                                            .collect();
                                        if avail.is_empty() {
                                            return view! {
                                                <p class="muted small">"能加的渠道都已经在这个组里了。"</p>
                                            }
                                                .into_any();
                                        }
                                        view! {
                                            <div class="pick-grid">
                                                {avail
                                                    .into_iter()
                                                    .map(|l| {
                                                        let id = l.id;
                                                        view! {
                                                            <button
                                                                class="ghost"
                                                                on:click=move |_| {
                                                                    draft
                                                                        .update(|d| {
                                                                            if let Some(d) = d {
                                                                                if !d.channels.contains(&id) {
                                                                                    d.channels.push(id);
                                                                                }
                                                                            }
                                                                        });
                                                                }
                                                            >
                                                                <span>{l.name.clone()}</span>
                                                                <span class="muted small">
                                                                    {format!(" #{id} · {}", if l.enabled { "启用" } else { "停用" })}
                                                                </span>
                                                            </button>
                                                        }
                                                    })
                                                    .collect_view()}
                                            </div>
                                        }
                                            .into_any()
                                    }}

                                    <div class="modal-foot">
                                        <button class="primary" on:click=save>"保存"</button>
                                        <button class="ghost" on:click=move |_| draft.set(None)>"取消"</button>
                                        <span class="spacer"></span>
                                        <span class="muted small">
                                            {format!("{} 条渠道", d2.channels.len())}
                                            {d3.original.map(|o| format!("（原组名 {o}）")).unwrap_or_default()}
                                        </span>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}

            // 路由参数平时收着，点开才看到细节
            <details class="fold">
                <summary>
                    {move || format!(
                        "路由参数（重试 {} 次 · 单次超时 {} 毫秒 · {} 分钟回头试首选）",
                        f_retry.get(),
                        f_timeout.get(),
                        f_reprobe_min.get()
                    )}
                </summary>
                <div class="panel">
                    <p class="muted small">
                        "「重试次数」是首次失败后再试几次：填 3 = 单条渠道最多试 4 次，全败才换渠道。"
                    </p>
                    <div class="form-grid">
                        <label>"重试次数（0–20）"</label>
                        <input
                            type="number"
                            min="0"
                            max="20"
                            prop:value=f_retry
                            on:input=move |ev| f_retry.set(event_target_value(&ev).parse().unwrap_or(0))
                        />

                        <label>"单次尝试超时（毫秒）"</label>
                        <input
                            type="number"
                            min="1"
                            prop:value=f_timeout
                            on:input=move |ev| f_timeout.set(event_target_value(&ev).parse().unwrap_or(30_000))
                        />

                        <label>"整个请求的重试预算（毫秒）"</label>
                        <div>
                            <input
                                type="number"
                                min="1"
                                prop:value=f_budget
                                on:input=move |ev| f_budget.set(event_target_value(&ev).parse().unwrap_or(90_000))
                            />
                            <div class="muted small">"超了这个预算就不再重试，直接把错误回给客户端。"</div>
                        </div>

                        <label>"回切间隔（分钟）"</label>
                        <div>
                            <input
                                type="number"
                                min="0"
                                prop:value=f_reprobe_min
                                on:input=move |ev| f_reprobe_min.set(event_target_value(&ev).parse().unwrap_or(10))
                            />
                            <div class="muted small">"切换之后隔这么久回头试首选，成了就切回去。"</div>
                        </div>

                        <label>"没人用时也自动去试"</label>
                        <div>
                            <input
                                type="checkbox"
                                prop:checked=f_probe
                                on:change=move |ev| f_probe.set(event_target_checked(&ev))
                            />
                            <span class="muted small">
                                "开着才不会「夜里没请求就永远不回切」。这种试探不算进请求记录，也不改当前走哪条。"
                            </span>
                        </div>
                    </div>
                    <div class="modal-foot">
                        <button class="primary" on:click=save_cfg>"保存参数"</button>
                    </div>
                </div>
            </details>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, vision: bool) -> Servable {
        Servable {
            name: name.into(),
            vision,
        }
    }

    #[test]
    fn vision_filter_keeps_everything_when_off() {
        let all = vec![m("auto", true), m("BAAI/bge-m3", false)];
        assert_eq!(filter_servable(&all, false).len(), 2);
    }

    #[test]
    fn vision_filter_keeps_only_the_ones_that_can_see() {
        let all = vec![m("auto", true), m("BAAI/bge-m3", false), m("glm-4v", true)];
        let got = filter_servable(&all, true);
        let names: Vec<&str> = got.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["auto", "glm-4v"], "顺序要保持原样");
    }
}
