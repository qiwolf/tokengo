//! 模型：一个模型名 = 一串按顺序试的渠道。
//!
//! **这一页只列「渠道里勾过的名字」。** v0.9.0 起勾选就是严格白名单：没勾的名字谁也不认，
//! 所以没勾的名字根本不在对外契约里，列它只会让人以为能用。行从渠道的勾选算出来，
//! 顶上按用途分档：对话模型 / 嵌入模型 / 图像 / Video / 音频 / 还没定。
//!
//! 用途（类型）只认两件事：**人在这一页上选的**、以及**第一次真被打到时按端点学到的**。
//! 绝不按名字猜（名字里带 embed / vl / tts 都不代表什么）。图像 / Video / 音频 这三种
//! 网关暂时没有对应端点、学不出来，只能手选 —— 手选之后实测也不许覆盖。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{Channel, Group, NameKind, RoutingCfg, RoutingGroup};
use crate::pages::PageHead;
use crate::Ctx;

/// 一档用途：分档按钮上写什么、这一档收哪些类型、配色。
pub(crate) struct KindTab {
    /// 分档键。
    pub key: &'static str,
    pub title: &'static str,
    /// 这一档收的类型字面值（来自 `ModelKind::as_str`）。
    pub kinds: &'static [&'static str],
    pub css: &'static str,
}

/// 六档，顺序固定（用户口径；参照 freellmapi 把用途当标签用）。
pub(crate) const KIND_TABS: [KindTab; 6] = [
    KindTab {
        key: "chat",
        title: "对话模型",
        kinds: &["chat"],
        css: "kind-chat",
    },
    // 重排归在嵌入这一档：同样是「检索类」用法、同样不是拿来聊天的；行上会标「重排」。
    KindTab {
        key: "embeddings",
        title: "嵌入模型",
        kinds: &["embeddings", "rerank"],
        css: "kind-emb",
    },
    KindTab {
        key: "image",
        title: "图像",
        kinds: &["image"],
        css: "kind-image",
    },
    KindTab {
        key: "video",
        title: "Video",
        kinds: &["video"],
        css: "kind-video",
    },
    KindTab {
        key: "audio",
        title: "音频",
        kinds: &["audio"],
        css: "kind-audio",
    },
    // 还没定：网关只转发对话 / 嵌入两种请求，没被打过的名字学不出来，也不许按名字猜。
    // 真被打过一次就会自动落到前面几档里 —— 所以这一档会自己空掉，不是垃圾桶。
    KindTab {
        key: "",
        title: "还没定",
        kinds: &[""],
        css: "kind-unknown",
    },
];

/// 表格里的一行：一个模型名 + 它现在的全部事实。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelRow {
    pub name: String,
    /// 用途：`""` / `chat` / `embeddings` / `rerank` / `image` / `video` / `audio`。
    pub kind: String,
    /// 谁定的：`""` / `auto` / `manual`。
    pub kind_source: String,
    /// 勾了这个名字的渠道 id（按渠道列表顺序，稳定）。
    pub declare: Vec<i64>,
    /// 有没有**启用的**渠道把它标了「能看图」。
    pub vision: bool,
    /// 有没有同名模型（没有的话客户端打这个名字会 404）。
    pub has_group: bool,
    pub group_enabled: bool,
    /// 同名模型里的渠道顺序（先试 → 后试）。
    pub members: Vec<i64>,
}

/// 这个类型落在第几档（`KIND_TABS` 的下标）。认不出来 = 「还没定」那一档。
pub(crate) fn kind_tab(kind: &str) -> usize {
    KIND_TABS
        .iter()
        .position(|t| t.kinds.contains(&kind))
        .unwrap_or(KIND_TABS.len() - 1)
}

/// 这一档的键。认不出来的一律当「还没定」，不冒充对话模型。
pub(crate) fn tab_of(kind: &str) -> &'static str {
    KIND_TABS[kind_tab(kind)].key
}

/// 表格里的一行行，从「渠道里勾过的名字」算出来。
///
/// 顺序按名字排（忽略大小写），这样刷新前后行的位置不会跳。
/// **归一之后同名只留一行**（`Auto` 与 `auto` 是同一个名字）。
pub(crate) fn build_rows(
    channels: &[Channel],
    groups: &[Group],
    kinds: &[NameKind],
) -> Vec<ModelRow> {
    let mut names: Vec<String> = Vec::new();
    for c in channels {
        for m in &c.models {
            let n = m.model.trim();
            if n.is_empty() {
                continue;
            }
            if !names.iter().any(|x| tokengo_shared::same_model_name(x, n)) {
                names.push(n.to_string());
            }
        }
    }
    names.sort_by_key(|s| s.to_lowercase());

    names
        .into_iter()
        .map(|name| {
            let declare: Vec<i64> = channels
                .iter()
                .filter(|c| {
                    c.models
                        .iter()
                        .any(|m| tokengo_shared::same_model_name(&m.model, &name))
                })
                .map(|c| c.id)
                .collect();
            let vision = channels.iter().filter(|c| c.enabled).any(|c| {
                c.models
                    .iter()
                    .any(|m| tokengo_shared::same_model_name(&m.model, &name) && m.vision)
            });
            let g = groups
                .iter()
                .find(|g| tokengo_shared::same_model_name(&g.name, &name));
            // 用途：名字上那份优先（v0.10.0 起类型挂在名字上），没有就回落同名模型上那份
            // （老库里定过的类型、以及实测学到的都在那儿）。
            let (kind, kind_source) =
                match kinds.iter().find(|k| tokengo_shared::same_model_name(&k.name, &name)) {
                    Some(k) if !k.kind.is_empty() => (k.kind.clone(), k.kind_source.clone()),
                    _ => match g {
                        Some(g) => (g.kind.clone(), g.kind_source.clone()),
                        None => (String::new(), String::new()),
                    },
                };
            ModelRow {
                name,
                kind,
                kind_source,
                declare,
                vision,
                has_group: g.is_some(),
                group_enabled: g.map(|x| x.enabled).unwrap_or(false),
                members: g.map(|x| x.channels.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

/// 这一档里的行（保持 `rows` 的顺序）。
pub(crate) fn rows_in_tab(rows: &[ModelRow], key: &str) -> Vec<ModelRow> {
    rows.iter().filter(|r| tab_of(&r.kind) == key).cloned().collect()
}

/// 每一档有几行（与 [`KIND_TABS`] 一一对应）—— 分档按钮上的数字用它。
pub(crate) fn tab_counts(rows: &[ModelRow]) -> Vec<usize> {
    KIND_TABS
        .iter()
        .map(|t| rows.iter().filter(|r| tab_of(&r.kind) == t.key).count())
        .collect()
}

/// 这一组里**请求不会发给它**的成员：没勾这个组名（含一个都没勾的渠道）。
///
/// 严格白名单（v0.9.0）：一条渠道只认它勾上的那些名字，一个都没勾则谁也不认。
/// 这里把它算出来，行里直接标出来 —— 不然会出现「组里挂着这条渠道，
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
                    !c.models
                        .iter()
                        .any(|m| tokengo_shared::same_model_name(&m.model, &group.name))
                })
                .unwrap_or(false)
        })
        .copied()
        .collect()
}

/// 正在编辑的渠道顺序（草稿状态，保存前只存在浏览器里）。
///
/// 这一页只能改**已有**模型的顺序：模型名从渠道的勾选来，不能凭空捏一个。
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    name: String,
    enabled: bool,
    channels: Vec<i64>,
}

#[component]
pub fn Models(ctx: Ctx) -> impl IntoView {
    let groups = RwSignal::new(Vec::<Group>::new());
    let channels = RwSignal::new(Vec::<Channel>::new());
    let kinds = RwSignal::new(Vec::<NameKind>::new());
    let routes = RwSignal::new(Vec::<RoutingGroup>::new());
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let draft = RwSignal::new(None::<Draft>);
    // 展开看细节的那一行（存模型名）
    let open = RwSignal::new(None::<String>);
    // 只看能看图的
    let only_vision = RwSignal::new(false);
    // 现在看哪一档。默认「对话模型」：绝大多数人在找的就是它
    let tab = RwSignal::new("chat".to_string());

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
            match api::model_kinds().await {
                Ok(v) => kinds.set(v),
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
    let channel_enabled = move |id: i64| {
        channels
            .get()
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.enabled)
            .unwrap_or(false)
    };
    let channel_name_opt = move |id: Option<i64>| match id {
        None => "—".to_string(),
        Some(i) => channel_name(i),
    };

    // 改一个**模型名**的用途：`auto` = 跟着实测走（清空，下次真有请求再学）。
    // **手动定过就不再被实测覆盖** —— 面板上要让人放心按。
    let set_kind = move |name: String, kind: &'static str| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::set_model_kind(&name, kind).await {
                Ok(()) => {
                    ctx.say(if kind == "auto" {
                        "已经交还给实测：下次真有请求进来时按端点重新认一次"
                    } else {
                        "用途已定，之后不会实测再把它改掉"
                    });
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("改用途没成功：{}", e.message));
                }
            }
        });
    };

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
                    ctx.say(format!(
                        "{}没成功：{}",
                        if enabled { "启用" } else { "停用" },
                        e.message
                    ));
                }
            }
        });
    };

    // 「建好它」：这个名字在渠道里勾了、但还没有同名模型 —— 客户端打它现在会 404。
    // 按勾它的那几条渠道建一个（顺序就是渠道列表的顺序）。
    let make_model = move |name: String, members: Vec<i64>| {
        if members.is_empty() {
            ctx.say("这个名字还没有任何渠道勾它 —— 先去「渠道」页勾上");
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match api::save_group(&name, true, &members).await {
                Ok(()) => {
                    ctx.say(format!("{name} 建好了 —— 客户端现在能打这个名字了"));
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("没建成：{}", e.message));
                }
            }
        });
    };

    let open_edit = move |r: ModelRow| {
        draft.set(Some(Draft {
            name: r.name.clone(),
            enabled: r.group_enabled,
            channels: r.members.clone(),
        }));
    };

    let save = move |_| {
        let Some(d) = draft.get_untracked() else {
            return;
        };
        wasm_bindgen_futures::spawn_local(async move {
            match api::save_group(d.name.trim(), d.enabled, &d.channels).await {
                Ok(()) => {
                    draft.set(None);
                    ctx.say("渠道顺序已保存");
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("保存失败：{}", e.message));
                }
            }
        });
    };

    let remove = move |name: String| {
        let ok = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!(
                    "确定删掉模型「{name}」？客户端再用这个名字会拿到 404。\
                     （渠道里的勾选一个都不动，随时能重新建好）"
                ))
                .ok()
            })
            .unwrap_or(false);
        if !ok {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match api::delete_group(&name).await {
                Ok(()) => {
                    ctx.say("模型已删除 —— 客户端再打这个名字会拿到 404");
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
                let j = if up { i.wrapping_sub(1) } else { i + 1 };
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
                desc="只有渠道里勾过的名字会出现在这里，按用途分档。收到请求就按渠道顺序依次试，第一条不通就换下一条。"
            >
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
            </PageHead>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <section class="card">
                <header class="card-head">
                    <h2>"按用途分档"</h2>
                    <span class="spacer"></span>
                    <label class="pick" title="只看有启用的渠道标了「能看图」的模型名">
                        <input
                            type="checkbox"
                            prop:checked=move || only_vision.get()
                            on:change=move |ev| only_vision.set(event_target_checked(&ev))
                        />
                        "只看能看图的"
                    </label>
                </header>
                <div class="card-body">
                    // 分档按钮：数字是这一档现在有几个。空档也留着（让人知道有这么一类）。
                    <div class="kind-tabs">
                        {KIND_TABS
                            .iter()
                            .enumerate()
                            .map(|(i, t)| {
                                let key = t.key;
                                let title = t.title;
                                let css = t.css;
                                view! {
                                    <button
                                        class=move || {
                                            let on = if tab.get() == key { " on" } else { "" };
                                            format!("kind-tab {css}{on}")
                                        }
                                        on:click=move |_| tab.set(key.to_string())
                                    >
                                        <span class="kind-dot"></span>
                                        {title}
                                        <span class="tab-n">
                                            {move || {
                                                tab_counts(&build_rows(&channels.get(), &groups.get(), &kinds.get()))[i]
                                            }}
                                        </span>
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>

                    <div class="table-wrap">
                        <table class="grid">
                            <thead>
                                <tr>
                                    <th>"模型名"</th>
                                    <th>"用途"</th>
                                    <th>"状态"</th>
                                    <th>"勾它的渠道"</th>
                                    <th>"现在走的"</th>
                                    <th class="num">"换过几次"</th>
                                    <th>"下次回头试首选"</th>
                                    <th>"操作"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {move || {
                                    let all = build_rows(&channels.get(), &groups.get(), &kinds.get());
                                    let cur = tab.get();
                                    let mut list = rows_in_tab(&all, &cur);
                                    if only_vision.get() {
                                        list.retain(|r| r.vision);
                                    }
                                    if list.is_empty() {
                                        let msg = if all.is_empty() {
                                            "渠道里还没勾过任何模型名 —— 先去「渠道」页勾上要用的名字。"
                                                .to_string()
                                        } else if only_vision.get() {
                                            "这一档里没有标了「能看图」的模型名。".to_string()
                                        } else {
                                            "这一档还没有模型。".to_string()
                                        };
                                        return view! {
                                            <tr class="empty-row">
                                                <td colspan="8" class="muted empty-cell">{msg}</td>
                                            </tr>
                                        }
                                            .into_any();
                                    }
                                    list
                                        .into_iter()
                                        .map(|r| {
                                            let name = r.name.clone();
                                            let name_kind = r.name.clone();
                                            let name_open = r.name.clone();
                                            let name_del = r.name.clone();
                                            let name_probe = r.name.clone();
                                            let name_make = r.name.clone();
                                            let declare_make = r.declare.clone();
                                            let is_open = open.get().as_deref() == Some(r.name.as_str());
                                            let kind_is_auto = r.kind.is_empty();
                                            let kind = r.kind.clone();
                                            let kind_hint = match r.kind_source.as_str() {
                                                "auto" => "实测认的",
                                                "manual" => "你定的",
                                                _ => "",
                                            };
                                            let route = routes
                                                .get()
                                                .into_iter()
                                                .find(|x| x.group == r.name);
                                            let active = route
                                                .as_ref()
                                                .map(|x| channel_name_opt(x.active_channel_id))
                                                .unwrap_or_else(|| "—".to_string());
                                            let switched =
                                                route.as_ref().map(|x| x.switched).unwrap_or(false);
                                            let count =
                                                route.as_ref().map(|x| x.switch_count).unwrap_or(0);
                                            let next = route
                                                .as_ref()
                                                .and_then(|x| x.next_probe_at_ms.map(|t| fmt::ago(x.now_ms, t)))
                                                .unwrap_or_else(|| "—".to_string());
                                            let declare = r.declare.clone();
                                            let members = r.members.clone();
                                            // 组里哪几条渠道不会收到这个名字的请求（没勾这个名字）。
                                            // 每个成员的小闭包各拿一份克隆（成员就那么几条，克隆便宜，
                                            // 而且 leptos 的视图闭包要求 Send，Rc 过不了）。
                                            let gated = members_not_declaring(
                                                &channels.get_untracked(),
                                                &Group {
                                                    name: r.name.clone(),
                                                    enabled: r.group_enabled,
                                                    channels: members.clone(),
                                                    ..Default::default()
                                                },
                                            );
                                            let has_group = r.has_group;
                                            let enabled = r.group_enabled;
                                            let vision = r.vision;
                                            let rerank = r.kind == "rerank";
                                            let meta = KIND_TABS[kind_tab(&r.kind)].title;
                                            let css = KIND_TABS[kind_tab(&r.kind)].css;
                                            view! {
                                                <>
                                                    <tr>
                                                        <td data-label="模型名">
                                                            <b>{name.clone()}</b>
                                                            {vision
                                                                .then(|| {
                                                                    view! {
                                                                        <span
                                                                            class="tag vision"
                                                                            title="有启用的渠道把它标了「能看图」，带图的请求会发给那条渠道"
                                                                        >
                                                                            "能看图"
                                                                        </span>
                                                                    }
                                                                })}
                                                        </td>
                                                        <td data-label="用途">
                                                            <select
                                                                class="kind-pick"
                                                                title="这个模型是干什么用的。默认「跟着实测走」：第一次真有请求进来时按走的端点认一次；你手动定了之后就不会再被实测改掉。"
                                                                on:change=move |ev| {
                                                                    let v = event_target_value(&ev);
                                                                    let k: &'static str = match v.as_str() {
                                                                        "chat" => "chat",
                                                                        "embeddings" => "embeddings",
                                                                        "rerank" => "rerank",
                                                                        "image" => "image",
                                                                        "video" => "video",
                                                                        "audio" => "audio",
                                                                        _ => "auto",
                                                                    };
                                                                    set_kind(name_kind.clone(), k);
                                                                }
                                                            >
                                                                <option value="auto" selected=kind_is_auto>
                                                                    "跟着实测走"
                                                                </option>
                                                                <option value="chat" selected=kind == "chat">"对话"</option>
                                                                <option value="embeddings" selected=kind == "embeddings">"嵌入"</option>
                                                                <option value="rerank" selected=kind == "rerank">"重排"</option>
                                                                <option value="image" selected=kind == "image">"图像"</option>
                                                                <option value="video" selected=kind == "video">"Video"</option>
                                                                <option value="audio" selected=kind == "audio">"音频"</option>
                                                            </select>
                                                            <span class=format!("tag {css}")>
                                                                {if rerank { "重排" } else { meta }}
                                                            </span>
                                                            <span class="muted small">{kind_hint}</span>
                                                        </td>
                                                        <td data-label="状态">
                                                            {if !has_group {
                                                                view! {
                                                                    <span
                                                                        class="tag warn"
                                                                        title="渠道里勾了这个名字，但还没有同名模型 —— 客户端打它现在会 404。点「详情」里的「建好它」。"
                                                                    >
                                                                        "还打不了"
                                                                    </span>
                                                                }
                                                                    .into_any()
                                                            } else {
                                                                // 这里是**能点的开关**，不是标签：点一下当场启用/停用。
                                                                let (nm, cur2) = (name.clone(), enabled);
                                                                let cls = if enabled { "switch on" } else { "switch" };
                                                                let tip = if enabled {
                                                                    "点一下停用：客户端再打这个名字会拿到 404"
                                                                } else {
                                                                    "点一下启用：客户端就能打这个名字了"
                                                                };
                                                                let toggle = toggle_group.clone();
                                                                view! {
                                                                    <button
                                                                        class=cls
                                                                        title=tip
                                                                        on:click=move |_| toggle(nm.clone(), !cur2)
                                                                    >
                                                                        <span class="knob"></span>
                                                                        <span class="switch-text">
                                                                            {if cur2 { "启用" } else { "停用" }}
                                                                        </span>
                                                                    </button>
                                                                }
                                                                    .into_any()
                                                            }}
                                                        </td>
                                                        <td data-label="勾它的渠道">
                                                            {format!("{} 条", declare.len())}
                                                        </td>
                                                        <td data-label="现在走的">
                                                            {active}
                                                            {switched
                                                                .then(|| view! { <span class="tag warn">"没走首选"</span> })}
                                                        </td>
                                                        <td class="num" data-label="换过几次">{count}</td>
                                                        <td data-label="下次回头试首选">{next}</td>
                                                        <td class="actions">
                                                            <button
                                                                class="ghost"
                                                                on:click=move |_| {
                                                                    let n = name_open.clone();
                                                                    open
                                                                        .update(|o| {
                                                                            if o.as_deref() == Some(n.as_str()) {
                                                                                *o = None;
                                                                            } else {
                                                                                *o = Some(n.clone());
                                                                            }
                                                                        });
                                                                }
                                                            >
                                                                {if is_open { "收起" } else { "详情" }}
                                                            </button>
                                                            {has_group
                                                                .then(|| {
                                                                    let nm = name_del.clone();
                                                                    view! {
                                                                        <button
                                                                            class="ghost danger"
                                                                            on:click=move |_| remove(nm.clone())
                                                                        >
                                                                            "删除"
                                                                        </button>
                                                                    }
                                                                })}
                                                        </td>
                                                    </tr>
                                                    {is_open
                                                        .then(move || {
                                                            let members_v = members.clone();
                                                            let gated_v = gated.clone();
                                                            let name_v = r.name.clone();
                                                            let declare_v = declare.clone();
                                                            view! {
                                                                <tr class="detail-row">
                                                                    <td colspan="8">
                                                                        <div class="model-detail">
                                                                            <div class="detail-block">
                                                                                <span class="detail-label">"勾它的渠道"</span>
                                                                                <div class="member-list">
                                                                                    {declare_v
                                                                                        .iter()
                                                                                        .map(|id| {
                                                                                            let id = *id;
                                                                                            let on = channel_enabled(id);
                                                                                            view! {
                                                                                                <span class="member">
                                                                                                    {channel_name(id)}
                                                                                                    <span class=if on { "tag ok" } else { "tag warn" }>
                                                                                                        {if on { "启用" } else { "停用" }}
                                                                                                    </span>
                                                                                                </span>
                                                                                            }
                                                                                        })
                                                                                        .collect_view()}
                                                                                </div>
                                                                            </div>
                                                                            {(!has_group)
                                                                                .then(|| {
                                                                                    view! {
                                                                                        <div class="detail-block">
                                                                                            <span class="detail-label">"客户端能打这个名字吗"</span>
                                                                                            <div>
                                                                                                "还不行：渠道里勾了它，但还没有同名模型，客户端打这个名字会拿到 404。"
                                                                                                <button
                                                                                                    class="ghost"
                                                                                                    on:click=move |_| make_model(name_make.clone(), declare_make.clone())
                                                                                                >
                                                                                                    "建好它"
                                                                                                </button>
                                                                                            </div>
                                                                                        </div>
                                                                                    }
                                                                                })}
                                                                            {(has_group
                                                                                && !members_v.is_empty())
                                                                                .then(|| {
                                                                                    view! {
                                                                                        <div class="detail-block">
                                                                                            <span class="detail-label">"渠道顺序（先试 → 后试）"</span>
                                                                                            <div class="member-list">
                                                                                                {members_v
                                                                                                    .iter()
                                                                                                    .map(|id| {
                                                                                                        let id = *id;
                                                                                                        let is_gated = gated_v.contains(&id);
                                                                                                        let gated_inner = gated_v.clone();
                                                                                                        let name_inner = name_v.clone();
                                                                                                        view! {
                                                                                                            <span class="member">
                                                                                                                {channel_name(id)}
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
                                                                                                                    let g_name = name_inner.clone();
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
                                                                                                                                    .any(|m| tokengo_shared::same_model_name(&m.model, &name))
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
                                                                                        </div>
                                                                                    }
                                                                                })}
                                                                            {has_group
                                                                                .then(|| {
                                                                                    view! {
                                                                                        <div class="detail-actions">
                                                                                            <button
                                                                                                class="ghost"
                                                                                                on:click=move |_| {
                                                                                                    let want = name_v.clone();
                                                                                                    let rows = build_rows(
                                                                                                        &channels.get_untracked(),
                                                                                                        &groups.get_untracked(),
                                                                                                        &kinds.get_untracked(),
                                                                                                    );
                                                                                                    if let Some(r) = rows.into_iter().find(|x| x.name == want) {
                                                                                                        open_edit(r);
                                                                                                    }
                                                                                                }
                                                                                            >
                                                                                                "调整顺序"
                                                                                            </button>
                                                                                            <button
                                                                                                class="ghost"
                                                                                                on:click=move |_| do_reprobe(name_probe.clone())
                                                                                            >
                                                                                                "立刻试首选"
                                                                                            </button>
                                                                                        </div>
                                                                                    }
                                                                                })}
                                                                        </div>
                                                                    </td>
                                                                </tr>
                                                            }
                                                        })}
                                                </>
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
                None => ().into_any(),
                Some(d) => {
                    let d2 = d.clone();
                    let off = d.channels.iter().filter(|id| !channel_enabled(**id)).count();
                    view! {
                        <div class="panel card">
                            <h3>{format!("「{}」的渠道顺序（先试 → 后试）", d.name)}</h3>
                            <table class="grid tight">
                                <tbody>
                                    {move || {
                                        let Some(d) = draft.get() else { return ().into_any() };
                                        if d.channels.is_empty() {
                                            return view! {
                                                <tr>
                                                    <td class="muted">"还没选渠道 —— 下面挑一条加进来"</td>
                                                </tr>
                                            }
                                                .into_any();
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
                                                            <button
                                                                class="ghost"
                                                                disabled=i == 0
                                                                on:click=move |_| move_channel(i, true)
                                                            >
                                                                "↑"
                                                            </button>
                                                            <button
                                                                class="ghost"
                                                                disabled=last
                                                                on:click=move |_| move_channel(i, false)
                                                            >
                                                                "↓"
                                                            </button>
                                                            <button
                                                                class="ghost danger"
                                                                on:click=move |_| {
                                                                    draft
                                                                        .update(|d| {
                                                                            if let Some(d) = d {
                                                                                d.channels.remove(i);
                                                                            }
                                                                        });
                                                                }
                                                            >
                                                                "移除"
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

                            <h3>"加一条渠道"</h3>
                            {move || {
                                let in_draft: Vec<i64> = draft.get().map(|d| d.channels).unwrap_or_default();
                                let avail: Vec<Channel> = channels
                                    .get()
                                    .into_iter()
                                    .filter(|l| !in_draft.contains(&l.id))
                                    .collect();
                                if avail.is_empty() {
                                    return view! {
                                        <p class="muted small">"能加的渠道都已经在这个模型里了。"</p>
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
                                    {if off > 0 {
                                        format!("（其中 {off} 条停用）")
                                    } else {
                                        String::new()
                                    }}
                                </span>
                            </div>
                        </div>
                    }
                        .into_any()
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
                    <div class="form-grid">
                        <label>"重试次数（0–20）"</label>
                        <div>
                            <input
                                type="number"
                                min="0"
                                max="20"
                                prop:value=f_retry
                                on:input=move |ev| f_retry.set(event_target_value(&ev).parse().unwrap_or(0))
                            />
                            <div class="muted small">
                                "首次失败后再试几次：填 3 = 单条渠道最多试 4 次，全败才换渠道。"
                            </div>
                        </div>

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
                            <div class="muted small">
                                "超了这个预算就不再重试，直接把错误回给客户端。"
                            </div>
                        </div>

                        <label>"回切间隔（分钟）"</label>
                        <div>
                            <input
                                type="number"
                                min="0"
                                prop:value=f_reprobe_min
                                on:input=move |ev| f_reprobe_min.set(event_target_value(&ev).parse().unwrap_or(10))
                            />
                            <div class="muted small">
                                "切换之后隔这么久回头试首选，成了就切回去。"
                            </div>
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

    fn chan_vision(id: i64, model: &str) -> Channel {
        let mut c = chan(id, true, &[model]);
        c.models[0].vision = true;
        c
    }

    fn group_with(name: &str, members: &[i64]) -> Group {
        Group {
            name: name.into(),
            enabled: true,
            channels: members.to_vec(),
            ..Default::default()
        }
    }

    fn name_kind(name: &str, kind: &str) -> NameKind {
        NameKind {
            name: name.into(),
            kind: kind.into(),
            kind_source: "manual".into(),
            ..Default::default()
        }
    }

    /// 这一页只列渠道里勾过的名字 —— 没勾的模型不许混进来（用户口径）。
    #[test]
    fn only_names_declared_in_channels_get_a_row() {
        let chans = vec![chan(1, true, &["auto"]), chan(2, true, &["BAAI/bge-m3"])];
        let groups = vec![
            group_with("auto", &[1]),
            group_with("BAAI/bge-m3", &[2]),
            group_with("没人勾的模型", &[1]),
        ];
        let rows = build_rows(&chans, &groups, &[]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["auto", "BAAI/bge-m3"],
            "顺序按名字排，且只有勾过的名字"
        );
    }

    /// 归一之后同名只留一行，并且把勾它的渠道都算上。
    #[test]
    fn the_same_name_in_two_channels_is_one_row() {
        let chans = vec![
            chan(1, true, &["Auto"]),
            chan(2, true, &["auto"]),
            chan(3, true, &["别的名字"]),
        ];
        let groups = vec![group_with("auto", &[1])];
        let rows = build_rows(&chans, &groups, &[]);
        assert_eq!(rows.len(), 2, "Auto 与 auto 是同一个名字");
        let row = rows.iter().find(|r| r.name == "Auto").unwrap();
        assert_eq!(row.declare, vec![1, 2], "两条渠道都勾了它");
    }

    /// 用途：名字上那份优先；名字上没有就回落同名模型上那份（老库里的类型照样看得见）。
    #[test]
    fn the_kind_comes_from_the_name_then_falls_back_to_the_model() {
        let chans = vec![chan(1, true, &["auto"]), chan(2, true, &["v4.1flash"])];
        let mut g = group_with("auto", &[1]);
        g.kind = "embeddings".into();
        g.kind_source = "auto".into();
        let groups = vec![g, group_with("v4.1flash", &[2])];

        let rows = build_rows(&chans, &groups, &[name_kind("auto", "image")]);
        let auto = rows.iter().find(|r| r.name == "auto").unwrap();
        assert_eq!(auto.kind, "image", "名字上定过的用途优先");
        assert_eq!(auto.kind_source, "manual");

        let v = rows.iter().find(|r| r.name == "v4.1flash").unwrap();
        assert_eq!(v.kind, "", "两边都没有 = 还没定，不许猜一个出来");
        assert_eq!(v.kind_source, "");
    }

    /// 分档：重排归「嵌入」那一档，认不出来的归「还没定」。
    #[test]
    fn tabs_put_rerank_with_embeddings_and_junk_with_undecided() {
        assert_eq!(tab_of("rerank"), "embeddings");
        assert_eq!(tab_of("embeddings"), "embeddings");
        assert_eq!(tab_of("chat"), "chat");
        assert_eq!(tab_of("banana"), "");
        assert_eq!(tab_of(""), "");
        assert_eq!(kind_tab("image"), 2, "图像是第三档");
        assert_eq!(kind_tab("video"), 3);
        assert_eq!(kind_tab("audio"), 4);
        assert_eq!(kind_tab("banana"), 5, "认不出来的落最后一档（还没定）");
    }

    /// 每一档的数字与行数对得上（分档按钮上的数字就是这么来的）。
    #[test]
    fn tab_counts_match_the_rows() {
        let chans = vec![
            chan(1, true, &["auto", "v4.1flash", "flux-1-schnell", "veo-3"]),
            chan(2, true, &["BAAI/bge-reranker-v2-m3"]),
        ];
        let groups = vec![
            group_with("auto", &[1]),
            group_with("v4.1flash", &[1]),
            group_with("flux-1-schnell", &[1]),
            group_with("veo-3", &[1]),
            {
                // 重排：用途在模型上定过（名字上那份还没有），照样归「嵌入」那一档
                let mut g = group_with("BAAI/bge-reranker-v2-m3", &[2]);
                g.kind = "rerank".into();
                g.kind_source = "manual".into();
                g
            },
        ];
        let kinds = vec![
            name_kind("auto", "chat"),
            name_kind("flux-1-schnell", "image"),
            name_kind("veo-3", "video"),
        ];
        let rows = build_rows(&chans, &groups, &kinds);
        let counts = tab_counts(&rows);
        assert_eq!(
            counts,
            vec![1, 1, 1, 1, 0, 1],
            "对话/嵌入(含重排)/图像/Video/音频/还没定"
        );
        assert_eq!(rows_in_tab(&rows, "embeddings").len(), 1);
        assert_eq!(
            rows_in_tab(&rows, "embeddings")[0].name,
            "BAAI/bge-reranker-v2-m3"
        );
        assert_eq!(rows_in_tab(&rows, "audio").len(), 0, "音频这档现在是空的");
    }

    /// 「能看图」只认**启用的**渠道标过的那个名字。
    #[test]
    fn vision_needs_an_enabled_channel() {
        let chans = vec![chan_vision(1, "auto"), {
            let mut c = chan_vision(2, "别的名字");
            c.enabled = false;
            c
        }];
        let groups = vec![group_with("auto", &[1])];
        let rows = build_rows(&chans, &groups, &[]);
        assert!(rows.iter().find(|r| r.name == "auto").unwrap().vision);

        let mut off = chan_vision(3, "glm-4v");
        off.enabled = false;
        let rows = build_rows(&[off], &[], &[]);
        assert!(!rows[0].vision, "停用的渠道标了也不算");
    }

    /// 「还没建好」：渠道里勾了但还没有同名模型的，行上要能看出来（客户端打会 404）。
    #[test]
    fn a_declared_name_without_a_model_is_flagged() {
        let chans = vec![chan(1, true, &["auto", "新名字"])];
        let groups = vec![group_with("auto", &[1])];
        let rows = build_rows(&chans, &groups, &[]);
        assert!(rows.iter().find(|r| r.name == "auto").unwrap().has_group);
        let new = rows.iter().find(|r| r.name == "新名字").unwrap();
        assert!(!new.has_group, "没有同名模型要说出来，不能默认能打");
        assert_eq!(new.declare, vec![1], "「建好它」要按勾它的渠道建");
    }

    /// 严格白名单：没勾这个名字的成员都会被跳过 —— 一个都没勾的也一样。
    #[test]
    fn members_without_this_name_are_flagged_as_skipped() {
        let chans = vec![
            chan(1, true, &[]),          // 什么都没勾：谁也不认
            chan(2, true, &["别的名字"]), // 勾了别的：跳过
            chan(3, true, &["auto"]),    // 勾了这个：照旧服务
        ];
        let g = group_with("auto", &[1, 2, 3]);
        assert_eq!(members_not_declaring(&chans, &g), vec![1, 2]);
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

    /// 六档的标题与配色都不重样（重了就看不出区别），顺序固定。
    #[test]
    fn every_tab_has_its_own_title_and_colour() {
        let titles: Vec<&str> = KIND_TABS.iter().map(|t| t.title).collect();
        assert_eq!(
            titles,
            vec!["对话模型", "嵌入模型", "图像", "Video", "音频", "还没定"]
        );
        for i in 0..KIND_TABS.len() {
            for j in (i + 1)..KIND_TABS.len() {
                assert_ne!(KIND_TABS[i].css, KIND_TABS[j].css, "两档配色重了");
                assert_ne!(KIND_TABS[i].key, KIND_TABS[j].key, "两档键重了");
            }
        }
    }
}
