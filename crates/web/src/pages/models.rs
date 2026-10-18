//! 模型：一行 = 渠道里勾过的一个模型名（外加保留名 auto）。
//!
//! **v0.11.0 起没有「模型组」这一层了。** 一个模型名 = 渠道里勾了它的那些渠道：
//! 勾上就出现、取消就消失，名字不用在这里建。先试哪条由**渠道顺序**决定（在「渠道」页调，
//! 全站只有一份），这一页只把它显示出来。这样面板的「模型」页、客户端清单 `/v1/models`
//! 和实际路由读的是同一份事实，不会再出现「挂着渠道却没人勾」的行。
//!
//! 用途（类型）只认两件事：**人在这一页选的**、以及**第一次真被打到时按端点学到的**。
//! 绝不按名字猜（名字里带 embed / vl / tts 都不代表什么）。图像 / Video / 音频 这三种
//! 网关暂时没有对应端点、学不出来，只能手选 —— 手选之后实测也不许覆盖。
//!
//! 保留名 `auto`：客户端打 `auto`（或不写模型名）时，网关在**你勾选过的**模型里
//! 现场挑一个最优的发出去，失败换下一条。它不落库、不占渠道、也**不做兜底** ——
//! 一个勾选过的模型都没有时，客户端清单里不出现 auto。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{AutoRoutes, Channel, NameRoute, RoutingCfg};
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
        title: "待识别",
        kinds: &[""],
        css: "kind-unknown",
    },
];

/// 表格里的一行：一个模型名 + 它现在的全部事实（服务端算好的）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelRow {
    pub name: String,
    /// 用途：`""` / `chat` / `embeddings` / `rerank` / `image` / `video` / `audio`。
    pub kind: String,
    /// 谁定的：`""` / `auto` / `manual`。
    pub kind_source: String,
    /// 勾了它的渠道（**含停用**的）。
    pub declared_by: Vec<i64>,
    /// 现在真能用的那串渠道，按先试顺序（就是请求的尝试顺序）。
    pub channels: Vec<i64>,
    /// 现在有没有渠道能用（`false` = 客户端打这个名字会 404）。
    pub routed: bool,
    /// 有没有启用的渠道勾了它、并标了「能看图」。
    pub can_see: bool,
    /// 现在走的那条。
    pub active: Option<i64>,
    pub switches: i64,
    pub next_probe_at: Option<i64>,
}

impl ModelRow {
    /// 服务端给的一行 → 表格里的一行。**这一层不做判断**：能不能用、有没有得挑
    /// 都是服务端算的，面板照抄，免得两处口径打架。
    pub(crate) fn from_route(r: &NameRoute) -> Self {
        ModelRow {
            name: r.name.clone(),
            kind: r.kind.clone(),
            kind_source: r.kind_source.clone(),
            declared_by: r.declared_by.clone(),
            channels: r.channels.clone(),
            routed: r.routed,
            can_see: r.can_see,
            active: r.active_channel_id,
            switches: r.switch_count,
            next_probe_at: r.next_probe_at_ms,
        }
    }

    /// 首选 = 现在该走的第一条（没有可用渠道就没有首选）。
    pub(crate) fn primary(&self) -> Option<i64> {
        self.channels.first().copied()
    }

    /// 现在走的不是首选（切过去了、还在等回切）。
    pub(crate) fn on_backup(&self) -> bool {
        self.active.is_some() && self.active != self.primary()
    }
}

/// 表格里的一行行。名单与顺序都听服务端的（一行一个勾过的名字）。
pub(crate) fn build_rows(routes: &[NameRoute]) -> Vec<ModelRow> {
    routes.iter().map(ModelRow::from_route).collect()
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

/// 服务端算好的 auto 候选：这一档排第一的（这一刻会挑中的）。挑不出来就 `None`。
pub(crate) fn auto_pick<'a>(auto: &'a AutoRoutes, kind: &str) -> Option<&'a str> {
    if !auto.available {
        return None;
    }
    auto_list(auto, kind).first().map(|s| s.as_str())
}

/// 这一档的候选链（按服务端给的顺序：最近没出过岔、平均更快的排前面）。
pub(crate) fn auto_list<'a>(auto: &'a AutoRoutes, kind: &str) -> &'a [String] {
    match kind {
        "chat" => &auto.chat,
        "embeddings" => &auto.embeddings,
        "rerank" => &auto.rerank,
        _ => &[],
    }
}

#[component]
pub fn Models(ctx: Ctx) -> impl IntoView {
    let routes = RwSignal::new(Vec::<NameRoute>::new());
    let auto = RwSignal::new(AutoRoutes::default());
    let channels = RwSignal::new(Vec::<Channel>::new());
    let now_ms = RwSignal::new(0i64);
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    // 展开看细节的那一行（存模型名）
    let open = RwSignal::new(None::<String>);
    // 只看能看图的
    let only_vision = RwSignal::new(false);
    let search = RwSignal::new(String::new());
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
            match api::model_routes().await {
                Ok(r) => {
                    routes.set(r.models);
                    auto.set(r.auto);
                    now_ms.set(r.now_ms);
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

    // 不想等回切间隔时用：把这个模型名的回切检查点归零。
    let do_reprobe = move |name: String| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::reprobe(&name).await {
                Ok(()) => {
                    ctx.say(format!("{name}：已经让它重新去试首选 —— 成不成看下一次请求"));
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("没成：{}", e.message));
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
                desc="一行 = 渠道里勾过的一个名字。收到请求就按渠道顺序（在「渠道」页用 ↑↓ 调）依次试，第一条不通就换下一条。"
            >
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
            </PageHead>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            // ———————— 保留名 auto ————————
            <section class="card auto-summary">
                <header class="card-head"><h2>"自动选择"</h2><span class="tag">"auto"</span></header>
                <div class="card-body">
                    {move || {
                        let a = auto.get();
                        if !a.available { return view! { <span class="muted small">"暂无候选模型"</span> }.into_any(); }
                        view! {
                            <div class="auto-current">
                                {[("chat", "对话"), ("embeddings", "嵌入"), ("rerank", "重排")].into_iter().filter_map(|(k,label)| {
                                    auto_pick(&a,k).map(|name| view! { <span><small>{label}</small><b>{name.to_string()}</b></span> })
                                }).collect_view()}
                            </div>
                            <details class="fold"><summary>"候选顺序"</summary><div class="panel">
                                {[("chat", "对话"), ("embeddings", "嵌入"), ("rerank", "重排")].into_iter().map(|(k,label)| {
                                    view! { <div class="detail-block"><span class="detail-label">{label}</span><span>{auto_list(&a,k).join(" → ")}</span></div> }
                                }).collect_view()}
                            </div></details>
                        }.into_any()
                    }}
                </div>
            </section>

            <section class="card">
                <header class="card-head">
                    <h2>"模型目录"</h2>
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
                                            {move || tab_counts(&build_rows(&routes.get()))[i]}
                                        </span>
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>

                    <input class="model-search" type="search" placeholder="搜索模型名称" aria-label="搜索模型名称"
                        prop:value=search on:input=move |ev| search.set(event_target_value(&ev)) />
                    <div class="model-list">
                        {move || {
                            let all = build_rows(&routes.get());
                            let cur = tab.get();
                            let mut list = rows_in_tab(&all, &cur);
                            let needle = search.get().to_lowercase();
                            list.retain(|r| r.name.to_lowercase().contains(&needle));
                            if only_vision.get() {
                                list.retain(|r| r.can_see);
                            }
                            if list.is_empty() {
                                let msg = if !needle.is_empty() {
                                    "没有匹配的模型。"
                                } else if all.is_empty() {
                                    "渠道里还没勾过任何模型名 —— 先去「渠道」页勾上要用的名字。"
                                } else if only_vision.get() {
                                    "这一档里没有标了「能看图」的模型名。"
                                } else {
                                    "这一档还没有模型。"
                                };
                                return view! { <div class="model-empty">{msg}</div> }.into_any();
                            }
                            list
                                .into_iter()
                                .map(|r| {
                                    let name = r.name.clone();

                                    let name_open = r.name.clone();
                                    let is_open = open.get().as_deref() == Some(r.name.as_str());


                                    let kind_hint = match r.kind_source.as_str() {
                                        "auto" => "实测认的",
                                        "manual" => "已有人工设置",
                                        "upstream" => "上游声明",
                                        _ => "待识别",
                                    };
                                    let active = r
                                        .active
                                        .map(channel_name)
                                        .unwrap_or_else(|| "—".to_string());
                                    let on_backup = r.on_backup();
                                    let next = r
                                        .next_probe_at
                                        .map(|t| fmt::ago(now_ms.get(), t))
                                        .unwrap_or_else(|| "—".to_string());
                                    let declare = r.declared_by.clone();
                                    let chain = r.channels.clone();
                                    let routed = r.routed;
                                    let vision = r.can_see;
                                    let switches = r.switches;
                                    let conflict = routes.get().iter().find(|x| x.name == r.name).map(|x| x.kind_conflict).unwrap_or(false);
                                    let open_id = r.active;
                                    let css = KIND_TABS[kind_tab(&r.kind)].css;
                                    // 卡片上标用途只用两个字：卡片一行就那么宽，
                                    // 分档条上用「对话模型」那种四字名（那边地方够）。
                                    let kind_tag = match r.kind.as_str() {
                                        "chat" => "对话",
                                        "embeddings" => "嵌入",
                                        "rerank" => "重排",
                                        "image" => "图像",
                                        "video" => "Video",
                                        "audio" => "音频",
                                        _ => "待识别",
                                    };
                                    let name_title = r.name.clone();
                                    view! {
                                        <div class="model-card" class:open=is_open>
                                            <div class="mline">
                                                <b class="mname" title=name_title>{name.clone()}</b>
                                                <span class="spacer"></span>
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
                                            </div>
                                            <div class="model-badges">
                                                {vision
                                                    .then(|| {
                                                        view! {
                                                            <span
                                                                class="tag vision"
                                                                title="有启用的渠道把它标了「能看图」，带图的请求会发给那条渠道"
                                                            >
                                                                "视觉已标记"
                                                            </span>
                                                        }
                                                    })}

                                                <span class=format!("tag {css}")>{kind_tag}</span>
                                                {if routed {
                                                    view! {
                                                        <span
                                                            class="tag ok"
                                                            title="有启用的渠道，不代表调用已经测试通过"
                                                        >
                                                            "已配置"
                                                        </span>
                                                    }
                                                        .into_any()
                                                } else {
                                                    view! {
                                                        <span
                                                            class="tag warn"
                                                            title="勾它的渠道全停用了 —— 客户端打这个名字会拿到 404。去「渠道」页把其中一条启用。"
                                                        >
                                                            "无可用渠道"
                                                        </span>
                                                    }
                                                        .into_any()
                                                }}
                                                {conflict.then(|| view! { <span class="tag warn">"类型有冲突"</span> })}
                                            </div>
                                            <div class="model-channel">{format!("{} · {} 条可用渠道", active, chain.len())}</div>
                                            {is_open
                                                .then(move || {
                                                    let declare_v = declare.clone();
                                                    let chain_v = chain.clone();
                                                    let name_v = r.name.clone();
                                                    view! {
                                                        <div class="model-detail">
                                                            <div class="model-facts">
                                                                <span>{format!("类型来源：{kind_hint}")}</span>
                                                                <span>{format!("切换：{switches} 次")}</span>
                                                                {r.next_probe_at.map(|_| view! { <span>{format!("回试首选：{next}")}</span> })}
                                                                {on_backup.then(|| view! { <span class="tag warn">"当前使用备用渠道"</span> })}
                                                            </div>
                                                            {(!routed)
                                                                .then(|| {
                                                                    view! {
                                                                        <div class="detail-block">
                                                                            <span class="detail-label">
                                                                                "客户端能打这个名字吗"
                                                                            </span>
                                                                            <div>
                                                                                "现在打不了：勾它的渠道都停用了。去「渠道」页把其中一条启用，或者在那条渠道里勾上它。"
                                                                            </div>
                                                                        </div>
                                                                    }
                                                                })}
                                                            <div class="detail-block">
                                                                <span class="detail-label">
                                                                    "渠道顺序（先试 → 后试；在「渠道」页用 ↑↓ 调，全站只有这一份）"
                                                                </span>
                                                                <div class="member-list">
                                                                    {chain_v
                                                                        .iter()
                                                                        .enumerate()
                                                                        .map(|(i, id)| {
                                                                            let id = *id;
                                                                            let is_active = open_id == Some(id);
                                                                            let is_primary = i == 0;
                                                                            let off = !channel_enabled(id);
                                                                            view! {
                                                                                <span class="member">
                                                                                    <span class="num muted">
                                                                                        {format!("{}。", i + 1)}
                                                                                    </span>
                                                                                    {channel_name(id)}
                                                                                    {(is_primary)
                                                                                        .then(|| {
                                                                                            view! {
                                                                                                <span
                                                                                                    class="tag ok"
                                                                                                    title="请求会先试它；它不通才换下一条"
                                                                                                >
                                                                                                    "首选"
                                                                                                </span>
                                                                                            }
                                                                                        })}
                                                                                    {(is_active && !is_primary)
                                                                                        .then(|| {
                                                                                            view! {
                                                                                                <span
                                                                                                    class="tag warn"
                                                                                                    title="现在走的是它（首选不通时切过来的）；到点会回头试首选"
                                                                                                >
                                                                                                    "现在走的"
                                                                                                </span>
                                                                                            }
                                                                                        })}
                                                                                    {off
                                                                                        .then(|| {
                                                                                            view! {
                                                                                                <span
                                                                                                    class="tag warn"
                                                                                                    title="这条渠道停用了 —— 它不会收到这个模型的请求"
                                                                                                >
                                                                                                    "停用"
                                                                                                </span>
                                                                                            }
                                                                                        })}
                                                                                </span>
                                                                            }
                                                                        })
                                                                        .collect_view()}
                                                                </div>
                                                            </div>
                                                            <div class="detail-block">
                                                                <span class="detail-label">
                                                                    "谁勾了它（勾选就是白名单，没勾的渠道不会收到这个名字的请求）"
                                                                </span>
                                                                <div class="member-list">
                                                                    {declare_v
                                                                        .iter()
                                                                        .map(|id| {
                                                                            let id = *id;
                                                                            let in_chain = chain_v.contains(&id);
                                                                            view! {
                                                                                <span class="member">
                                                                                    {channel_name(id)}
                                                                                    {if in_chain {
                                                                                        view! {
                                                                                            <span class="tag ok">"启用"</span>
                                                                                        }
                                                                                            .into_any()
                                                                                    } else {
                                                                                        view! {
                                                                                            <span class="tag warn">"停用"</span>
                                                                                        }
                                                                                            .into_any()
                                                                                    }}
                                                                                </span>
                                                                            }
                                                                        })
                                                                        .collect_view()}
                                                                </div>
                                                            </div>
                                                            <div class="detail-actions">
                                                                {(routed)
                                                                    .then(|| {
                                                                        view! {
                                                                            <button
                                                                                class="ghost"
                                                                                on:click=move |_| do_reprobe(name_v.clone())
                                                                            >
                                                                                "立刻试首选"
                                                                            </button>
                                                                        }
                                                                    })}
                                                            </div>
                                                        </div>
                                                    }
                                                })}
                                        </div>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </div>
                </div>
            </section>

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

    fn route(name: &str, kind: &str, declared: &[i64], chain: &[i64]) -> NameRoute {
        NameRoute {
            name: name.into(),
            kind: kind.into(),
            kind_source: if kind.is_empty() { "".into() } else { "manual".into() },
            declared_by: declared.to_vec(),
            channels: chain.to_vec(),
            routed: !chain.is_empty(),
            ..Default::default()
        }
    }

    /// 一名字一行：名单与顺序都照抄服务端的（面板不再自己算一遍）。
    #[test]
    fn rows_come_from_the_server_as_they_are() {
        let routes = vec![
            route("BAAI/bge-m3", "embeddings", &[1], &[1]),
            route("v4.1flash", "chat", &[1, 2], &[2, 1]),
        ];
        let rows = build_rows(&routes);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["BAAI/bge-m3", "v4.1flash"]);
        let chat = rows.iter().find(|r| r.name == "v4.1flash").unwrap();
        assert_eq!(chat.channels, vec![2, 1], "先试哪条听服务端的");
        assert_eq!(chat.primary(), Some(2), "首选 = 链子里的第一条");
        assert_eq!(chat.declared_by, vec![1, 2], "勾过它但排后面的也算");
    }

    /// 渠道全停用 = 现在打不了（状态列显示「打不了」），不是「没勾过」。
    #[test]
    fn a_name_whose_channels_are_all_off_is_not_routed() {
        let rows = build_rows(&[route("BAAI/bge-m3", "embeddings", &[3], &[])]);
        assert!(!rows[0].routed);
        assert_eq!(rows[0].primary(), None);
        assert_eq!(rows[0].declared_by, vec![3], "但它还是「勾过」的");
    }

    /// 切过去之后「现在走的」不是首选 —— 行上要能标出来。
    #[test]
    fn walking_a_backup_channel_is_visible() {
        let mut r = route("v4.1flash", "chat", &[1, 2], &[1, 2]);
        r.active_channel_id = Some(2);
        r.switch_count = 1;
        let row = build_rows(&[r]).remove(0);
        assert!(row.on_backup());
        assert_eq!(row.switches, 1);

        // 走的就是首选时不标；压根没在用也不标
        let row2 = build_rows(&[route("v4.1flash", "chat", &[1], &[1])]).remove(0);
        assert!(!row2.on_backup(), "还没打过 → active 是空的");
        let mut r3 = route("v4.1flash", "chat", &[1], &[1]);
        r3.active_channel_id = Some(1);
        assert!(!build_rows(&[r3]).remove(0).on_backup(), "走首选不算切过");
    }

    /// 分档：按用途落到六档里，认不出来的是「还没定」，重排跟着嵌入那一档。
    #[test]
    fn kinds_land_in_the_right_tab() {
        assert_eq!(tab_of("rerank"), "embeddings", "重排跟嵌入同一档");
        assert_eq!(tab_of("embeddings"), "embeddings");
        assert_eq!(tab_of("chat"), "chat");
        assert_eq!(tab_of("banana"), "", "不认识的当还没定");
        assert_eq!(tab_of(""), "");
        assert_eq!(kind_tab("image"), 2, "图像是第三档");
        assert_eq!(kind_tab("video"), 3);
        assert_eq!(kind_tab("audio"), 4);
        assert_eq!(kind_tab("banana"), 5, "认不出来的落最后一档（还没定）");
    }

    /// 每一档的数字与行数对得上（分档按钮上的数字就是这么来的）。
    #[test]
    fn tab_counts_match_the_rows() {
        let rows = build_rows(&[
            route("v4.1flash", "chat", &[1], &[1]),
            route("BAAI/bge-reranker-v2-m3", "rerank", &[2], &[2]),
            route("flux-1-schnell", "image", &[1], &[1]),
            route("veo-3", "video", &[1], &[1]),
            route("新名字", "", &[1], &[1]),
        ]);
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

    /// 六档的标题与配色都不重样（重了就看不出区别），顺序固定。
    #[test]
    fn every_tab_has_its_own_title_and_colour() {
        let titles: Vec<&str> = KIND_TABS.iter().map(|t| t.title).collect();
        assert_eq!(
            titles,
            vec!["对话模型", "嵌入模型", "图像", "Video", "音频", "待识别"]
        );
        for i in 0..KIND_TABS.len() {
            for j in (i + 1)..KIND_TABS.len() {
                assert_ne!(KIND_TABS[i].css, KIND_TABS[j].css, "两档配色重了");
                assert_ne!(KIND_TABS[i].key, KIND_TABS[j].key, "两档键重了");
            }
        }
    }

    /// auto：一个模型都没勾选时它不该出现（客户端清单里也没有它）。
    #[test]
    fn auto_picks_nothing_when_it_is_not_available() {
        let a = AutoRoutes {
            available: false,
            chat: vec!["v4.1flash".into()],
            ..Default::default()
        };
        assert_eq!(auto_pick(&a, "chat"), None, "不可用时一个都不挑");
        assert!(!a.available);
    }

    /// auto：挑的是这一档排在第一个的（服务端按实测成绩排好的），失败才往下换。
    #[test]
    fn auto_picks_the_first_candidate_of_that_kind() {
        let a = AutoRoutes {
            available: true,
            chat: vec!["fast".into(), "slow".into()],
            embeddings: vec!["bge-m3".into()],
            rerank: vec!["reranker".into()],
        };
        assert_eq!(auto_pick(&a, "chat"), Some("fast"));
        assert_eq!(auto_pick(&a, "embeddings"), Some("bge-m3"));
        assert_eq!(auto_pick(&a, "rerank"), Some("reranker"));
        assert_eq!(auto_pick(&a, "image"), None, "图像那档永远不参与 auto");
        assert_eq!(auto_pick(&a, "audio"), None, "音频同理");
        assert_eq!(
            auto_list(&a, "chat"),
            &["fast".to_string(), "slow".to_string()],
            "候选链按服务端给的顺序"
        );
    }
}
