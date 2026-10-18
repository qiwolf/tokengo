//! 模型：客户端打过来的一个模型名 = 一串按顺序试的渠道。
//!
//! 原先把「模型组」和「路由状态」拆成两页，看的人得来回翻才知道
//! 「这个模型名现在到底在走哪条渠道」—— 现在并成一页：
//! 上面一张表看每个模型名**现在**怎么样（在走哪条、换过几次、什么时候回头试首选），
//! 下面是「调整顺序」和「路由参数」。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{Channel, Group, RoutingCfg, RoutingGroup, Servable, ModelPair};
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

/// 一种用途的模型该怎么说给用户听。
///
/// 用户的原话：「怎么区分重排模型，嵌入模型，对话模型，视觉模型，不好区分，你们排版好」。
/// 所以每一类都要说清三件事：**叫什么**、**客户端打哪个端点**、**拿它干什么**，
/// 再给一个固定的配色（颜色是给人做「一眼归类」用的，不是装饰）。
pub(crate) struct KindMeta {
    pub kind: &'static str,
    /// 界面上那句中文名（带用途，不用行话）。
    pub title: &'static str,
    /// 客户端该打哪个端点。
    pub endpoint: &'static str,
    /// 一句话：拿它干什么 / 什么情况下用。
    pub blurb: &'static str,
    /// 配色用的类名。
    pub css: &'static str,
}

/// 四类模型的固定说法与顺序：对话 → 向量 → 重排 → 还没定。
pub(crate) const KIND_META: [KindMeta; 4] = [
    KindMeta {
        kind: "chat",
        title: "对话模型",
        endpoint: "POST /v1/chat/completions",
        blurb: "问答、写作、写代码都走它。名字旁边标了「视觉」的还能读图；没标的收到图片请求会回 422。",
        css: "kind-chat",
    },
    KindMeta {
        kind: "embeddings",
        title: "向量模型（嵌入）",
        endpoint: "POST /v1/embeddings",
        blurb: "把一段文字变成一串数字，给检索/记忆库算相似度用。它不会「回话」，别拿去聊天。",
        css: "kind-emb",
    },
    KindMeta {
        kind: "rerank",
        title: "重排模型",
        endpoint: "POST /v1/rerank",
        blurb: "给「一个问题 + 一堆候选」打分排序，挑出最相关的几条。搜索/记忆召回的第二道筛子。",
        css: "kind-rerank",
    },
    KindMeta {
        kind: "",
        title: "还没定用途",
        endpoint: "打一次就自动认出来",
        blurb: "第一次被真实请求打到时，网关按端点自动认类型；也可以在这里手动定死。",
        css: "kind-unknown",
    },
];

/// 这一类的说法。**认不出来的类型落到「还没定」那一档**，不能冒充对话模型
/// （那会让界面把重排模型说成对话模型，比不显示更糟）。
pub(crate) fn kind_meta(kind: &str) -> &'static KindMeta {
    KIND_META
        .iter()
        .find(|m| m.kind == kind)
        .unwrap_or(&KIND_META[KIND_META.len() - 1])
}

/// 把能打的模型名按类型分堆，顺序固定：对话 → 向量 → 重排 → 还没定。
///
/// 为什么要分：同一个网关后面挂着三种用途完全不同的模型（现网 86% 的量是向量），
/// 混在一起列出来，人得靠名字去猜哪个是哪个 —— 而现网那批名字（`v4.1flash`、
/// `cline-pass/deepseek-v4.1-flash`）一个特征都没有。
///
/// 空的那几堆不返回（不给界面上留几个空标题）。纯函数，好测。
pub(crate) fn group_servable_by_kind(all: &[Servable]) -> Vec<(&'static KindMeta, Vec<Servable>)> {
    KIND_META
        .iter()
        .filter_map(|m| {
            let v: Vec<Servable> = all.iter().filter(|x| x.kind == m.kind).cloned().collect();
            (!v.is_empty()).then_some((m, v))
        })
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

/// 渠道里声明了、但**还没有同名模型组**的名字 —— 客户端打这些名字会 404
/// （对外的名字是「模型组」，渠道上那一行只是说「这条渠道认这个名字」）。
///
/// 只认**启用着的**渠道；渠道按 id 从小到大排（id 顺序就是优先级顺序），名字按字典序，
/// 这样一键补建出来的组顺序是稳的，不是随机的。
pub(crate) fn groups_missing_for_channel_models(
    channels: &[Channel],
    groups: &[Group],
) -> Vec<(String, Vec<i64>)> {
    use std::collections::BTreeMap;
    let has_group = |name: &str| {
        groups
            .iter()
            .any(|g| tokengo_shared::same_model_name(&g.name, name))
    };
    let mut sorted: Vec<&Channel> = channels.iter().filter(|c| c.enabled).collect();
    sorted.sort_by_key(|c| c.id);
    let mut want: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for c in sorted {
        for m in &c.models {
            let name = m.model.trim();
            // 比对走归一：`DeepSeek-V4.1-Flash` 与 `deepseek-v4.1-flash` 是一个名字，
            // 不然会误报「还没有同名模型组」，还多建一个重复的组。
            if name.is_empty() || has_group(name) {
                continue;
            }
            want.entry(name.to_string()).or_default().push(c.id);
        }
    }
    want.into_iter().collect()
}

/// 提示条上「删掉这些名字」要动哪些渠道的哪些行：`渠道 id → 要删的名字`。
///
/// 为什么要这个动作（2026-10-18 踩到的循环）：保存渠道时会把勾选的上游名并进模型表
/// （用户要的便利），于是**上游的内部名字**也变成了「渠道声明的客户端名字」；
/// 提示条又如实喊「没有同名模型组，客户端打它会 404」。这时候有两条正当出路：
/// 建同名模型组（用户真想让客户端打这个名字），或者把这些多余的声明删掉（本来就是上游名）。
/// 这个按钮就是第二条路，省得一条一条去渠道里删。
///
/// 只认「启用中的渠道」；同一个名字可能在多条渠道上声明过，全都要删。
pub(crate) fn rows_to_drop_for_missing_names(
    channels: &[Channel],
    groups: &[Group],
) -> Vec<(i64, Vec<String>)> {
    let miss = groups_missing_for_channel_models(channels, groups);
    if miss.is_empty() {
        return Vec::new();
    }
    let mut wanted: std::collections::BTreeMap<String, ()> = std::collections::BTreeMap::new();
    for (name, _) in &miss {
        wanted.insert(name.clone(), ());
    }
    let mut out: Vec<(i64, Vec<String>)> = Vec::new();
    let mut sorted: Vec<&Channel> = channels.iter().filter(|c| c.enabled).collect();
    sorted.sort_by_key(|c| c.id);
    for c in sorted {
        let mut drop: Vec<String> = c
            .models
            .iter()
            .map(|m| m.model.trim().to_string())
            .filter(|n| !n.is_empty() && wanted.contains_key(n))
            .collect();
        drop.sort();
        drop.dedup();
        if !drop.is_empty() {
            out.push((c.id, drop));
        }
    }
    out
}

/// 这一组里**请求不会发给它**的成员：勾过名字、却没勾这个组名。
///
/// v0.7.0 起「勾选就是白名单」：一条渠道只要勾了名字，就只认勾上的那些。
/// 这里把它算出来，模型页上直接标出来 —— 不然会出现「组里挂着这条渠道，
/// 但它一直没被用过」这种看名字看不出来的事。
pub(crate) fn members_not_declaring(channels: &[Channel], group: &Group) -> Vec<i64> {
    group
        .channels
        .iter()
        .filter(|id| {
            channels
                .iter()
                .find(|c| c.id == **id)
                .map(|c| {
                    !c.models.is_empty()
                        && !c
                            .models
                            .iter()
                            .any(|m| tokengo_shared::same_model_name(&m.model, &group.name))
                })
                .unwrap_or(false)
        })
        .copied()
        .collect()
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

    // 改一组是哪种模型：`auto` = 跟着实测走（清空，下次真有请求再学）。
    //
    // **手动定过就不再被实测覆盖** —— 面板上要让人放心按。
    let set_kind = move |name: String, kind: &'static str| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::set_group_kind(&name, kind).await {
                Ok(()) => {
                    ctx.say(if kind == "auto" {
                        "已经交还给实测：下次真有请求进来时按端点重新认一次"
                    } else {
                        "类型已定，之后不会实测再把它改掉"
                    });
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("改类型没成功：{}", e.message));
                }
            }
        });
    };

    // 渠道里声明了、却还没有同名模型组的名字：客户端打这些名字会 404。
    // 一键按名字补建模型组，把「声明了这个名字的渠道」按 id 顺序挂进去。
    let fill_groups = move |_| {
        let miss = groups_missing_for_channel_models(&channels.get_untracked(), &groups.get_untracked());
        if miss.is_empty() {
            ctx.say("没有要补的：渠道里声明的名字都已经有模型组了");
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            let mut done: Vec<String> = Vec::new();
            let mut failed = 0;
            for (name, ids) in miss {
                match api::save_group(&name, true, &ids).await {
                    Ok(()) => done.push(name),
                    Err(e) => {
                        failed += 1;
                        ctx.note_err(&e);
                    }
                }
            }
            ctx.say(if failed == 0 {
                format!("补建了 {} 个模型组：{}", done.len(), done.join("、"))
            } else {
                format!("补建了 {} 个，另有 {} 个没成（看红字）", done.len(), failed)
            });
            reload();
        });
    };

    // 提示条的第二个出路：把这些「声明了却没有同名组」的名字从渠道上删掉。
    // 走既有的「改渠道」接口（整份模型表提交），名称/地址/密钥/请求头/上限都原样带回，
    // 只少掉那几个多余的名字。
    let drop_missing = move |_: leptos::ev::MouseEvent| {
        let plan = rows_to_drop_for_missing_names(&channels.get_untracked(), &groups.get_untracked());
        if plan.is_empty() {
            ctx.say("没有要删的：渠道里声明的名字都已经有模型组了");
            return;
        }
        let snapshot = channels.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            let mut removed = 0usize;
            let mut failed = 0usize;
            for (id, names) in plan {
                let Some(c) = snapshot.iter().find(|c| c.id == id) else { continue };
                let kept: Vec<ModelPair> = c
                    .models
                    .iter()
                    .filter(|m| !names.contains(&m.model.trim().to_string()))
                    .cloned()
                    .collect();
                removed += names.len();
                match api::save_channel(
                    Some(id),
                    &c.name,
                    &c.base_url,
                    c.enabled,
                    c.unwrap_envelope,
                    c.max_concurrent,
                    None, // 密钥不动
                    &c.headers,
                    &kept,
                )
                .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        failed += 1;
                        ctx.note_err(&e);
                    }
                }
            }
            ctx.say(if failed == 0 {
                format!("删掉了 {removed} 个多余的模型名声明（客户端能打的名字没变）")
            } else {
                format!("删了 {removed} 个，有 {failed} 条渠道没改成（看红字）")
            });
            reload();
        });
    };

    // 一键启用/停用：状态列那个开关点的就是它。启停只改这一列，不动顺序与类型。
    let toggle_group = move |name: String, enabled: bool| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::set_group_enabled(&name, enabled).await {
                Ok(()) => {
                    ctx.say(if enabled {
                        format!("{name} 已启用 —— 客户端现在能打这个名字了")
                    } else {
                        format!("{name} 已停用 —— 客户端再打这个名字会拿到 404")
                    });
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("{}没成功：{}", if enabled { "启用" } else { "停用" }, e.message));
                }
            }
        });
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

            <section class="card">
                <header class="card-head">
                    <h2>"客户端能打的模型（按用途分）"</h2>
                    <p class="muted small">
                        "四种用途各占一块，颜色固定：蓝=对话、绿=向量、橙=重排、灰=还没定。"
                        "同一块里的名字可以随便挑，但**别拿错端点** —— 拿对话的写法去调向量模型，上游只会回 400。"
                    </p>
                    <span class="spacer"></span>
                    <label class="pick" title="只看后面有「能看图」渠道的模型名">
                        <input
                            type="checkbox"
                            prop:checked=move || only_vision.get()
                            on:change=move |ev| only_vision.set(event_target_checked(&ev))
                        />
                        "只看能看图的"
                    </label>
                </header>
                <div class="card-body">
                    <div class="kind-cards">
                        {move || {
                            let v = filter_servable(&servable.get(), only_vision.get());
                            if v.is_empty() {
                                return view! {
                                    <div class="muted small">"（一个都没有：先去「渠道」页声明模型名）"</div>
                                }
                                    .into_any();
                            }
                            group_servable_by_kind(&v)
                                .into_iter()
                                .map(|(meta, list)| {
                                    let n = list.len();
                                    view! {
                                        <div class=format!("kind-card {}", meta.css)>
                                            <div class="kind-card-head">
                                                <span class="kind-dot"></span>
                                                <strong>{meta.title}</strong>
                                                <span class="tag count">{format!("{n} 个")}</span>
                                                <span class="spacer"></span>
                                                <code class="small">{meta.endpoint}</code>
                                            </div>
                                            <div class="muted small">{meta.blurb}</div>
                                            <div class="kind-names">
                                                {list
                                                    .into_iter()
                                                    .map(|m| {
                                                        view! {
                                                            <code
                                                                class="chip code"
                                                                title=if m.kind == "embeddings" {
                                                                    "向量模型：客户端用 /v1/embeddings 调它"
                                                                } else if m.kind == "rerank" {
                                                                    "重排模型：客户端用 /v1/rerank 调它"
                                                                } else if m.vision {
                                                                    "对话模型，收得下图片输入"
                                                                } else {
                                                                    "对话模型；带图的请求会被回 422"
                                                                }
                                                            >
                                                                {m.name}
                                                                {m.vision.then(|| view! { <span class="tag vision">"能看图"</span> })}
                                                            </code>
                                                        }
                                                    })
                                                    .collect_view()}
                                            </div>
                                        </div>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </div>
                </div>
            </section>

            // 渠道里声明了、却还没有同名模型组：客户端打这些名字会 404。给个一键补建。
            {move || {
                let miss = groups_missing_for_channel_models(&channels.get(), &groups.get());
                if miss.is_empty() {
                    return ().into_any();
                }
                let names = miss.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>().join("、");
                let fill = fill_groups.clone();
                let drop_names = drop_missing.clone();
                view! {
                    <div class="banner warn">
                        <span>
                            "渠道里声明了这些名字，但还没有同名模型组："
                            <code>{names}</code>
                            " —— 客户端现在打它们会 404。" "想让它能打就建组；这些只是上游的内部名字，就用右边「删掉这些名字」。"
                        </span>
                        <span class="spacer"></span>
                        <button class="ghost" on:click=drop_names.clone()>"删掉这些名字"</button>
                        <button class="primary" on:click=fill>"一键补建模型组"</button>
                    </div>
                }
                .into_any()
            }}

            <section class="card">
                <header class="card-head">
                    <h2>"模型名 → 渠道顺序"</h2>
                    <p class="muted small">
                        "客户端填的名字都列在这里。收到请求就按「渠道顺序」从第一条往下试，第一条不通换下一条；"
                        "全都不通就等「下次回头试首选」到点再试一次首选。"
                    </p>
                </header>
                <div class="card-body flush">
            <div class="table-wrap">
                <table class="grid">
                    <thead>
                        <tr>
                            <th>"模型名"</th>
                            <th>"类型"</th>
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
                                        <td colspan="8" class="muted empty-cell">
                                            "还没有模型 —— 点右上角「新建模型」加一个，客户端才能打模型名。"
                                        </td>
                                    </tr>
                                }
                                .into_any();
                            }
                            gs.into_iter()
                                .map(|g| {
                                    let name = g.name.clone();
                                    // 类型那一列要用的几份拷贝（每份都被自己的闭包吃掉）
                                    let name_kind = g.name.clone();
                                    let kind_is_auto = g.kind.is_empty();
                                    let kind_is_chat = g.kind == "chat";
                                    let kind_is_emb = g.kind == "embeddings";
                                    let kind_is_rerank = g.kind == "rerank";
                                    let kind_hint = match g.kind_source.as_str() {
                                        "auto" => "实测认的",
                                        "manual" => "你定的",
                                        _ => "还没人来打过",
                                    };
                                    let g2 = g.clone();
                                    let g3 = g.clone();
                                    let g4 = g.clone();
                                    let r = routes.get().into_iter().find(|x| x.group == g.name);
                                    let active = r.as_ref().map(|x| channel_name_opt(x.active_channel_id)).unwrap_or_else(|| "—".to_string());
                                    let switched = r.as_ref().map(|x| x.switched).unwrap_or(false);
                                    let count = r.as_ref().map(|x| x.switch_count).unwrap_or(0);
                                    let next = r.as_ref().and_then(|x| x.next_probe_at_ms.map(|t| fmt::ago(x.now_ms, t))).unwrap_or_else(|| "—".to_string());
                                    // 组里哪几条渠道不会收到这个名字的请求（勾过别的名字）。
                                    // 每个成员的小闭包各拿一份克隆（成员就那么几条，克隆便宜，
                                    // 而且 leptos 的视图闭包要求 Send，Rc 过不了）。
                                    let gated = members_not_declaring(&channels.get_untracked(), &g);
                                    view! {
                                        <tr>
                                            <td data-label="模型名">
                                                <b>{name.clone()}</b>
                                                {g.can_see.then(|| view! {
                                                    <span class="tag vision" title="这一组里有「能看图」的渠道，带图的请求走得通">
                                                        "视觉"
                                                    </span>
                                                })}
                                                {(!g.can_see
                                                    && g.kind != "embeddings"
                                                    && g.kind != "rerank")
                                                    .then(|| view! {
                                                        <span class="tag muted" title="这一组里没有能看图的渠道：带图的请求会被回 422（说清楚，不悄悄丢图）">
                                                            "不收图"
                                                        </span>
                                                    })}
                                            </td>
                                            <td data-label="类型">
                                                <select
                                                    class="kind-pick"
                                                    title="这组是哪种模型。默认「跟着实测走」：第一次真有请求进来时按它走的端点认一次；你手动定了之后就不会再被实测改掉。"
                                                    on:change=move |ev| {
                                                        let v = event_target_value(&ev);
                                                        let k: &'static str = match v.as_str() {
                                                            "chat" => "chat",
                                                            "embeddings" => "embeddings",
                                                            "rerank" => "rerank",
                                                            _ => "auto",
                                                        };
                                                        set_kind(name_kind.clone(), k);
                                                    }
                                                >
                                                    <option value="auto" selected=kind_is_auto>
                                                        "跟着实测走"
                                                    </option>
                                                    <option value="chat" selected=kind_is_chat>"对话"</option>
                                                    <option value="embeddings" selected=kind_is_emb>"向量"</option>
                                                    <option value="rerank" selected=kind_is_rerank>"重排"</option>
                                                </select>
                                                <span class="muted small">{kind_hint}</span>
                                                // 同一套说法与配色，和上面「按用途分」那四块对得上：
                                                // 用户要的是「一眼看出这组是哪种」，光一个下拉框看不出来。
                                                {
                                                    let meta = kind_meta(if g.kind.is_empty() { "" } else { g.kind.as_str() });
                                                    view! {
                                                        <div>
                                                            <span class=format!("tag {}", meta.css)>{meta.title}</span>
                                                            <code class="small">{meta.endpoint}</code>
                                                        </div>
                                                    }
                                                }
                                            </td>
                                            <td data-label="状态">
                                                // 这里是**能点的开关**，不是标签：点一下当场启用/停用，
                                                // 只改这一列（顺序与类型都不动）。
                                                {
                                                    let (nm, cur) = (g.name.clone(), g.enabled);
                                                    let cls = if g.enabled { "switch on" } else { "switch" };
                                                    let tip = if g.enabled {
                                                        "点一下停用：客户端再打这个名字会拿到 404"
                                                    } else {
                                                        "点一下启用：客户端就能打这个名字了"
                                                    };
                                                    let toggle = toggle_group.clone();
                                                    view! {
                                                        <button
                                                            class=cls
                                                            title=tip
                                                            on:click=move |_| toggle(nm.clone(), !cur)
                                                        >
                                                            <span class="knob"></span>
                                                            <span class="switch-text">
                                                                {if cur { "启用" } else { "停用" }}
                                                            </span>
                                                        </button>
                                                    }
                                                }
                                            </td>
                                            <td data-label="渠道顺序（先试 → 后试）">
                                                {if g.channels.is_empty() {
                                                    view! { <span class="muted">"还没配渠道"</span> }.into_any()
                                                } else {
                                                    let g_name = g.name.clone();
                                                    view! {
                                                        <div class="member-list">
                                                            {g
                                                                .channels
                                                                .iter()
                                                                .map(|id| {
                                                                    let id = *id;
                                                                    let nm = channel_name(id);
                                                                    let is_gated = gated.contains(&id);
                                                                    let gated_inner = gated.clone();
                                                                    let g_name = g_name.clone();
                                                                    view! {
                                                                        <span class="member">
                                                                            {nm}
                                                                            {is_gated
                                                                                .then(|| {
                                                                                    view! {
                                                                                        <span
                                                                                            class="tag warn"
                                                                                            title="这条渠道勾过名字（勾选就是白名单），但没勾这个名字 —— 打这个名字的请求不会发给它。点右边的按钮替它勾上。"
                                                                                        >
                                                                                            "没勾这个名字，不会被用到"
                                                                                        </span>
                                                                                    }
                                                                                })}
                                                                            {move || {
                                                                                if !gated_inner.contains(&id) {
                                                                                    return ().into_any();
                                                                                }
                                                                                let g_name = g_name.clone();
                                                                                view! {
                                                                                    <button
                                                                                        class="ghost"
                                                                                        on:click=move |_| {
                                                                                            let name = g_name.clone();
                                                                                            let Some(c) = channels
                                                                                                .get_untracked()
                                                                                                .into_iter()
                                                                                                .find(|c| c.id == id) else {
                                                                                                ctx.say("这条渠道已经不在了");
                                                                                                return;
                                                                                            };
                                                                                            let mut models = c.models.clone();
                                                                                            if !models
                                                                                                .iter()
                                                                                                .any(|m| {
                                                                                                    tokengo_shared::same_model_name(&m.model, &name)
                                                                                                })
                                                                                            {
                                                                                                models
                                                                                                    .push(crate::model::ModelPair {
                                                                                                        model: name.clone(),
                                                                                                        upstream: None,
                                                                                                        vision: false,
                                                                                                        drop_fields: String::new(),
                                                                                                    });
                                                                                            }
                                                                                            wasm_bindgen_futures::spawn_local(async move {
                                                                                                match api::save_channel(
                                                                                                        Some(c.id),
                                                                                                        &c.name,
                                                                                                        &c.base_url,
                                                                                                        c.enabled,
                                                                                                        c.unwrap_envelope,
                                                                                                        c.max_concurrent,
                                                                                                        None,
                                                                                                        &c.headers,
                                                                                                        &models,
                                                                                                    )
                                                                                                    .await
                                                                                                {
                                                                                                    Ok(_) => {
                                                                                                        ctx
                                                                                                            .say(
                                                                                                                format!(
                                                                                                                    "已经把「{name}」加到这条渠道上 —— 现在请求会发给它了",
                                                                                                                ),
                                                                                                            );
                                                                                                        reload();
                                                                                                    }
                                                                                                    Err(e) => {
                                                                                                        ctx.note_err(&e);
                                                                                                        ctx.say(format!("没成：{}", e.message));
                                                                                                    }
                                                                                                }
                                                                                            });
                                                                                        }
                                                                                    >
                                                                                        "替它勾上"
                                                                                    </button>
                                                                                }
                                                                                    .into_any()
                                                                            }}
                                                                        </span>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </div>
                                                    }
                                                        .into_any()
                                                }}
                                            </td>
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
                </div>
            </section>

            // 编排面板（草稿状态，保存前只存在浏览器里）
                    {move || match draft.get() {
                        None => view! {
                            <p class="muted small">
                                "点表格里的「编排」改一个模型名的渠道顺序，或点右上角「新建模型」加一个 —— "
                                "客户端打的名字必须先在这里有，否则回 404。"
                            </p>
                        }.into_any(),
                        Some(d) => {
                            let is_new = d.original.is_none();
                            let d2 = d.clone();
                            let d3 = d.clone();
                            view! {
                                // .card 是为了给它一张卡的外壳（20px 圆角 + 发丝边）
                                <div class="panel card">
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
    use crate::model::ModelPair;

    fn m(name: &str, vision: bool) -> Servable {
        Servable {
            name: name.into(),
            vision,
            kind: String::new(),
            kind_label: String::new(),
            kind_source: String::new(),
        }
    }

    #[test]
    fn vision_filter_keeps_everything_when_off() {
        let all = vec![m("auto", true), m("BAAI/bge-m3", false)];
        assert_eq!(filter_servable(&all, false).len(), 2);
    }

    #[test]
    fn servable_splits_into_kind_buckets_in_a_fixed_order() {
        let s = |name: &str, kind: &str| Servable {
            name: name.into(),
            vision: false,
            kind: kind.into(),
            kind_label: String::new(),
            kind_source: String::new(),
        };
        let all = vec![
            s("v4.1flash", ""),
            s("BAAI/bge-m3", "embeddings"),
            s("auto", "chat"),
            s("rerank-x", "rerank"),
            s("deepseek-v4.1-flash", "chat"),
        ];
        let got = group_servable_by_kind(&all);
        let labels: Vec<&str> = got.iter().map(|(m, _)| m.title).collect();
        assert_eq!(
            labels,
            vec!["对话模型", "向量模型（嵌入）", "重排模型", "还没定用途"],
            "顺序固定，空的堆不出现（标题换成带用途的说法，v0.7.2）"
        );
        assert_eq!(got[0].1.len(), 2, "两个对话模型在同一堆里");
        assert_eq!(got[1].1[0].name, "BAAI/bge-m3");
        assert_eq!(got[3].1[0].name, "v4.1flash");

        // 反证：只有一个类型时，别的堆不许留空标题
        let only_vec = group_servable_by_kind(&[s("m", "embeddings")]);
        assert_eq!(only_vec.len(), 1);
        assert_eq!(only_vec[0].0.title, "向量模型（嵌入）");
        assert!(group_servable_by_kind(&[]).is_empty());
    }

    #[test]
    fn vision_filter_keeps_only_the_ones_that_can_see() {
        let all = vec![m("auto", true), m("BAAI/bge-m3", false), m("glm-4v", true)];
        let got = filter_servable(&all, true);
        let names: Vec<&str> = got.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["auto", "glm-4v"], "顺序要保持原样");
    }

    /// 回归：渠道里声明了 `deepseek-flash`、却没有同名模型组时要报出来等着补建 ——
    /// 这正是「新增渠道 + 选模型，保存完客户端还是打不通」的那个坑。
    #[test]
    fn a_channel_model_without_a_same_named_group_is_reported() {
        let chans = vec![chan(7, true, &["deepseek-flash"])];
        assert_eq!(
            groups_missing_for_channel_models(&chans, &[]),
            vec![("deepseek-flash".to_string(), vec![7])],
            "没有同名组就要报出来"
        );
        // 已经有同名组：不再报（别重复建）
        let g = Group {
            name: "deepseek-flash".into(),
            enabled: true,
            channels: vec![7],
            ..Default::default()
        };
        assert!(groups_missing_for_channel_models(&chans, &[g]).is_empty());
    }

    /// 停用的渠道不算数：给它建组也打不通，只会多一个空组。
    #[test]
    fn disabled_channels_do_not_ask_for_groups() {
        let chans = vec![chan(1, false, &["deepseek-flash"])];
        assert!(groups_missing_for_channel_models(&chans, &[]).is_empty());
    }

    /// 两条渠道声明同一个名字 → 挂两条，按 id 升序（id 顺序 = 先试哪条）。
    #[test]
    fn the_same_name_from_two_channels_carries_both_ids_in_priority_order() {
        let chans = vec![chan(9, true, &["auto2"]), chan(3, true, &["auto2"]), chan(5, true, &[" "])];
        assert_eq!(
            groups_missing_for_channel_models(&chans, &[]),
            vec![("auto2".to_string(), vec![3, 9])],
            "空名字要忽略，同名要合并且按 id 升序"
        );
    }

    fn group_with(name: &str, members: &[i64]) -> Group {
        Group {
            name: name.into(),
            enabled: true,
            channels: members.to_vec(),
            can_see: false,
            kind: String::new(),
            kind_label: String::new(),
            kind_source: String::new(),
            ..Default::default()
        }
    }

    /// 「这条渠道会被跳过吗」：一条都没勾 = 什么都认（不跳）；勾了别的名字 = 跳。
    #[test]
    fn only_channels_that_declared_other_names_are_flagged_as_skipped() {
        let chans = vec![
            chan(1, true, &[]),          // 什么都没勾：什么都认
            chan(2, true, &["别的名字"]), // 勾了别的：会被跳过
            chan(3, true, &["auto"]),    // 勾了这个：照旧服务
        ];
        let g = group_with("auto", &[1, 2, 3]);
        assert_eq!(members_not_declaring(&chans, &g), vec![2]);
    }

    /// 名字比对走归一：`Auto` / `auto` 是一个名字，不该被误报成「没勾」。
    #[test]
    fn the_skip_check_ignores_case_and_separators() {
        let chans = vec![chan(7, true, &["DeepSeek-V4.1-Flash"])];
        let g = group_with("deepseek_v4.1_flash", &[7]);
        assert!(members_not_declaring(&chans, &g).is_empty());
    }

    /// 不在组里的渠道不算数（只报这一组的成员）。
    #[test]
    fn only_group_members_are_considered() {
        let chans = vec![chan(1, true, &["别的名字"]), chan(2, true, &["别的名字"])];
        let g = group_with("auto", &[2]);
        assert_eq!(members_not_declaring(&chans, &g), vec![2]);
    }

    fn chan(id: i64, enabled: bool, models: &[&str]) -> Channel {
        Channel {
            id,
            name: format!("渠道{id}"),
            base_url: "http://x/v1".into(),
            enabled,
            has_secret: true,
            unwrap_envelope: false,
            max_concurrent: 0,
            headers: Vec::new(),
            models: models
                .iter()
                .map(|m| ModelPair {
                    model: m.to_string(),
                    upstream: None,
                    vision: false,
                    drop_fields: String::new(),
                })
                .collect(),
            vision_count: 0,
        }
    }
}


// ─────────── 提示条第二个出路：把多余的声明删掉（2026-10-18） ───────────

#[cfg(test)]
mod drop_tests {
    use super::*;
    use crate::model::{Group, ModelPair};

    fn ch(id: i64, enabled: bool, models: &[&str]) -> Channel {
        Channel {
            id,
            name: format!("渠道{id}"),
            base_url: format!("http://127.0.0.1/{id}/v1"),
            enabled,
            has_secret: true,
            unwrap_envelope: false,
            max_concurrent: 0,
            headers: Vec::new(),
            models: models
                .iter()
                .map(|m| ModelPair {
                    model: (*m).to_string(),
                    upstream: Some((*m).to_string()),
                    vision: false,
                    drop_fields: String::new(),
                })
                .collect(),
            vision_count: 0,
        }
    }

    fn group(name: &str, channels: &[i64]) -> Group {
        Group {
            name: name.into(),
            enabled: true,
            kind: "chat".into(),
            kind_source: "auto".into(),
            channels: channels.to_vec(),
            can_see: false,
            kind_label: "对话".into(),
        }
    }

    /// 提示条说「这些名字没有同名模型组」时，「删掉这些名字」要动的正是**声明了它们的那些启用渠道**，
    /// 一个不多一个不少，且绝不碰已经能用（有同名组）的名字。
    ///
    /// 反证：把 `rows_to_drop_for_missing_names` 里的 `wanted.contains_key(n)` 去掉
    /// （变成「删掉渠道上所有名字」）→ 这条用例立刻变红（auto 也会被删掉）。
    #[test]
    fn the_drop_list_names_exactly_the_channels_that_declared_the_missing_names() {
        let channels = vec![
            ch(1, true, &["auto", "deepseek-v4.1-flash"]),
            ch(2, true, &["auto", "cline-pass/deepseek-v4.1-flash"]),
            // 停用的渠道不算数：它就算声明了也不该被这个按钮动
            ch(3, false, &["v4.1flash"]),
        ];
        let groups = vec![group("auto", &[1, 2])];

        let got = rows_to_drop_for_missing_names(&channels, &groups);
        assert_eq!(
            got,
            vec![
                (1, vec!["deepseek-v4.1-flash".to_string()]),
                (2, vec!["cline-pass/deepseek-v4.1-flash".to_string()]),
            ],
            "只删声明了这些名字的启用渠道，且不碰 auto"
        );
    }

    /// 名字都有同名模型组时，这个按钮什么都不该删。
    #[test]
    fn nothing_is_dropped_when_every_declared_name_has_a_group() {
        let channels = vec![ch(1, true, &["auto"])];
        assert!(
            rows_to_drop_for_missing_names(&channels, &[group("auto", &[1])]).is_empty(),
            "什么都对的时候按钮必须是空操作"
        );
    }
}

// ─────────── 四种用途的说法与配色（v0.7.2，用户要求「一眼分得清」） ───────────

#[cfg(test)]
mod kind_meta_tests {
    use super::*;

    /// 四类各有一套互不相同的说法、端点与配色；端点必须点对（对话别写成向量）。
    ///
    /// 反证：把 `kind_meta` 的兜底从「还没定」改回 `KIND_META[0]`（对话）→
    /// `an_unknown_kind_falls_back_to_undecided` 立刻变红。
    #[test]
    fn each_kind_has_its_own_words_endpoint_and_colour() {
        let chat = kind_meta("chat");
        let emb = kind_meta("embeddings");
        let rerank = kind_meta("rerank");
        let unknown = kind_meta("");

        assert_eq!(chat.endpoint, "POST /v1/chat/completions");
        assert_eq!(emb.endpoint, "POST /v1/embeddings");
        assert_eq!(rerank.endpoint, "POST /v1/rerank");
        assert!(
            unknown.endpoint.contains("自动认"),
            "还没定的那一档要说清是「打一次自动认」"
        );

        let titles = [chat.title, emb.title, rerank.title, unknown.title];
        let css = [chat.css, emb.css, rerank.css, unknown.css];
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert_ne!(titles[i], titles[j], "四个标题不能重样");
                assert_ne!(css[i], css[j], "四套配色不能重样（重了就看不出区别）");
            }
        }
        assert!(emb.title.contains("向量"), "向量模型要说清是向量");
        assert!(rerank.title.contains("重排"), "重排模型要说清是重排");
    }

    /// 认不出来的类型落「还没定」，不能冒充对话模型。
    #[test]
    fn an_unknown_kind_falls_back_to_undecided() {
        assert_eq!(kind_meta("banana").kind, "");
        assert!(!kind_meta("banana").title.contains("对话"));
    }

    /// 分堆只用已知的四类，顺序固定；空的不出现。
    #[test]
    fn buckets_follow_the_fixed_order_and_skip_empty_ones() {
        let s = |name: &str, kind: &str| Servable {
            name: name.into(),
            kind: kind.into(),
            vision: false,
            kind_label: String::new(),
            kind_source: String::new(),
        };
        let got = group_servable_by_kind(&[s("r", "rerank"), s("c", "chat"), s("e", "embeddings")]);
        let titles: Vec<&str> = got.iter().map(|(m, _)| m.title).collect::<Vec<&str>>();
        assert_eq!(
            titles,
            vec!["对话模型", "向量模型（嵌入）", "重排模型"],
            "顺序固定、空的（还没定）不出现"
        );
    }
}
