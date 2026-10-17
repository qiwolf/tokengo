//! 模型组：一个对外可见的模型名 = 一张**有序**的腿列表（顺序即优先级，第一个是首选）。
//!
//! 界面刻意把"顺序"做成能上下挪的列表：路由策略是"按优先级试"，顺序错了就等于
//! 策略错了，这里必须一眼看得见、一眼改得动。

use leptos::prelude::*;

use crate::api;
use crate::model::{Group, Leg};
use crate::Ctx;

/// 正在编辑的组（草稿状态，保存前只存在浏览器里）。
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    /// `None` = 新建
    original: Option<String>,
    name: String,
    enabled: bool,
    legs: Vec<i64>,
}

#[component]
pub fn Groups(ctx: Ctx) -> impl IntoView {
    let groups = RwSignal::new(Vec::<Group>::new());
    let legs = RwSignal::new(Vec::<Leg>::new());
    let servable = RwSignal::new(Vec::<String>::new());
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let draft = RwSignal::new(None::<Draft>);

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
            match api::legs().await {
                Ok(v) => legs.set(v),
                Err(e) => ctx.note_err(&e),
            }
            busy.set(false);
        });
    };
    reload();

    let leg_name = move |id: i64| {
        legs.get()
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("已删除的腿 #{id}"))
    };

    let open_new = move |_| {
        draft.set(Some(Draft {
            original: None,
            name: String::new(),
            enabled: true,
            legs: Vec::new(),
        }));
    };

    let open_edit = move |g: Group| {
        draft.set(Some(Draft {
            original: Some(g.name.clone()),
            name: g.name.clone(),
            enabled: g.enabled,
            legs: g.legs.clone(),
        }));
    };

    let save = move |_| {
        let Some(d) = draft.get_untracked() else { return };
        if d.name.trim().is_empty() {
            ctx.say("组名不能空（客户端就是拿这个名字打过来的）");
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match api::save_group(d.name.trim(), d.enabled, &d.legs).await {
                Ok(()) => {
                    draft.set(None);
                    ctx.say("模型组已保存");
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
                    "确定删掉模型组「{}」？客户端再用这个名字会拿到 404。",
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
                    ctx.say("模型组已删除");
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("删除失败：{}", e.message));
                }
            }
        });
    };

    // 草稿里挪动/增删腿：都只改浏览器里的草稿，点保存才落库
    let move_leg = move |i: usize, up: bool| {
        draft.update(|d| {
            if let Some(d) = d {
                let j = if up { i.wrapping_sub(1) } else { i + 1 };
                if i < d.legs.len() && j < d.legs.len() {
                    d.legs.swap(i, j);
                }
            }
        });
    };

    view! {
        <div class="page">
            <div class="toolbar">
                <button class="primary" on:click=open_new>"新建模型组"</button>
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
                <span class="spacer"></span>
                <span class="muted small">"客户端能打的模型名："</span>
                {move || {
                    let v = servable.get();
                    if v.is_empty() {
                        return view! { <span class="muted small">"（一个都没有）"</span> }.into_any();
                    }
                    v.into_iter()
                        .map(|m| view! { <code class="chip code">{m}</code> })
                        .collect_view()
                        .into_any()
                }}
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <div class="split">
                <div>
                    <h2>"现有模型组"</h2>
                    <table class="grid">
                        <thead>
                            <tr>
                                <th>"名字"</th>
                                <th>"状态"</th>
                                <th class="num">"腿数"</th>
                                <th>"操作"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let v = groups.get();
                                if v.is_empty() {
                                    return view! {
                                        <tr>
                                            <td colspan="4" class="muted">
                                                "还没有模型组 —— 客户端现在打任何模型名都会 404。"
                                            </td>
                                        </tr>
                                    }.into_any();
                                }
                                v.into_iter()
                                    .map(|g| {
                                        let g2 = g.clone();
                                        let g3 = g.clone();
                                        let name = g.name.clone();
                                        let legs_txt = g
                                            .legs
                                            .iter()
                                            .map(|id| leg_name(*id))
                                            .collect::<Vec<_>>()
                                            .join(" → ");
                                        view! {
                                            <tr>
                                                <td>
                                                    <b>{name.clone()}</b>
                                                    <div class="muted small">{legs_txt}</div>
                                                </td>
                                                <td>
                                                    {if g.enabled {
                                                        view! { <span class="tag ok">"启用"</span> }.into_any()
                                                    } else {
                                                        view! { <span class="tag muted">"停用"</span> }.into_any()
                                                    }}
                                                </td>
                                                <td class="num">{g.legs.len()}</td>
                                                <td class="actions">
                                                    <button class="ghost" on:click=move |_| open_edit(g2.clone())>
                                                        "编排"
                                                    </button>
                                                    <button class="ghost danger" on:click=move |_| remove(g3.clone())>
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

                <div>
                    <h2>{move || {
                        match draft.get() {
                            Some(d) if d.original.is_none() => "新建模型组",
                            Some(_) => "编排优先级",
                            None => "编排优先级",
                        }
                    }}</h2>
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
                                                placeholder="如 auto（客户端打这个名字）"
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
                                                    "组名是客户端打的模型名，改名等于换一个组："
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
                                                if d.legs.is_empty() {
                                                    return view! {
                                                        <tr>
                                                            <td class="muted">"还没选腿 —— 下面挑一条加进来"</td>
                                                        </tr>
                                                    }.into_any();
                                                }
                                                d.legs
                                                    .iter()
                                                    .enumerate()
                                                    .map(|(i, id)| {
                                                        let id = *id;
                                                        let n = leg_name(id);
                                                        let last = i + 1 == d.legs.len();
                                                        view! {
                                                            <tr>
                                                                <td class="num muted">{i + 1}</td>
                                                                <td>
                                                                    {n.clone()}
                                                                    <span class="muted small">{format!(" #{id}")}</span>
                                                                </td>
                                                                <td class="actions">
                                                                    <button class="ghost" disabled=i == 0
                                                                        on:click=move |_| move_leg(i, true)>"↑"</button>
                                                                    <button class="ghost" disabled=last
                                                                        on:click=move |_| move_leg(i, false)>"↓"</button>
                                                                    <button class="ghost danger" on:click=move |_| {
                                                                        draft.update(|d| {
                                                                            if let Some(d) = d {
                                                                                d.legs.remove(i);
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

                                    <h3>"加一条腿"</h3>
                                    <select on:change=move |ev| {
                                        if let Ok(id) = event_target_value(&ev).parse::<i64>() {
                                            draft.update(|d| {
                                                if let Some(d) = d {
                                                    if !d.legs.contains(&id) {
                                                        d.legs.push(id);
                                                    }
                                                }
                                            });
                                        }
                                    }>
                                        <option value="">"挑一条腿…"</option>
                                        {move || legs.get().into_iter().map(|l| {
                                            view! {
                                                <option value=l.id.to_string()>
                                                    {format!("{} (#{}, {})", l.name, l.id, if l.enabled { "启用" } else { "停用" })}
                                                </option>
                                            }
                                        }).collect_view()}
                                    </select>

                                    <div class="modal-foot">
                                        <button class="primary" on:click=save>"保存"</button>
                                        <button class="ghost" on:click=move |_| draft.set(None)>"取消"</button>
                                        <span class="spacer"></span>
                                        <span class="muted small">
                                            {format!("{} 条腿", d2.legs.len())}
                                            {d3.original.map(|o| format!("（原组名 {o}）")).unwrap_or_default()}
                                        </span>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}
