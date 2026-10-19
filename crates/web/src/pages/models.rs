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
use crate::model::{AutoRoutes, Channel, NameRoute};
use crate::pages::PageHead;
use crate::Ctx;

/// 一档用途：分档按钮上写什么、这一档收哪些类型、配色。
pub(crate) struct KindTab {
    /// 分档键。
    pub key: &'static str,
    pub title: &'static str,
    /// 这一档收的类型字面值（来自 `ModelKind::as_str`）。
    pub kinds: &'static [&'static str],
    #[allow(dead_code)]
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
    #[cfg(test)]
    pub(crate) fn primary(&self) -> Option<i64> {
        self.channels.first().copied()
    }

    /// 现在走的不是首选（切过去了、还在等回切）。
    #[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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

/// 上下文只能展示元数据，不能把一次成功请求当作容量实测。
fn context_badge(route: &NameRoute) -> (String, String) {
    match route.context_window.filter(|n| *n > 0) {
        Some(tokens) => {
            let source = match route.context_source.as_str() { "upstream"=>"上游声明", "user_confirmed"=>"用户确认", "user_confirmed+upstream"=>"用户确认 / 上游声明", _=>"来源未注明" };
            (
                format!("上下文 {} · {source}", fmt::ctx(tokens)),
                format!("{tokens} tokens；{source}，非容量实测，不代表每条渠道都支持相同长度"),
            )
        }
        None => match route.nominal_context_window.filter(|n| *n > 0) {
            Some(tokens) if route.nominal_context_source.as_deref() == Some("models.dev") => (
                format!("上下文 {} · models.dev 标称", fmt::ctx(tokens)),
                format!("{tokens} tokens；models.dev 精确 ID，渠道未验证，不作为对外安全容量。{}", route.context_reason),
            ),
            _ => ("上下文 未知".into(), if route.context_reason.is_empty() {
                "尚无上下文长度元数据，不按模型名猜测".into()
            } else { route.context_reason.clone() }),
        },
    }
}

fn kind_source_label(source: &str) -> &'static str {
    match source {
        "upstream" => "上游声明",
        "auto" => "实测识别",
        "manual" => "人工设置",
        "" => "待识别",
        _ => "来源未注明",
    }
}

#[component]
pub fn Models(ctx:Ctx)->impl IntoView {
    let policy=RwSignal::new(crate::model::AutoPolicy::default());
    let order=RwSignal::new(Vec::<String>::new());
    let ordering=RwSignal::new(false);
    let routes=RwSignal::new(Vec::<NameRoute>::new());
    let auto=RwSignal::new(AutoRoutes::default());
    let chans=RwSignal::new(Vec::<Channel>::new());
    let busy=RwSignal::new(false);let err=RwSignal::new(None::<String>);
    let search=RwSignal::new(String::new());let tab=RwSignal::new("chat".to_string());
    let sort_score=RwSignal::new(true);let open=RwSignal::new(None::<String>);let updated=RwSignal::new(0i64);
    let reload=move || {
        if busy.get_untracked(){return;} busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api::model_routes().await {Ok(r)=>{routes.set(r.models);auto.set(r.auto);updated.set(r.now_ms);err.set(None);},Err(e)=>{ctx.note_err(&e);err.set(Some(e.message));}}
            if let Ok(c)=api::channels().await{chans.set(c);}
            if let Ok(p)=api::auto_policy().await{policy.set(p.policy);order.set(p.order);}
            busy.set(false);
        });
    };
    reload();let timer=crate::every(5000,reload);on_cleanup(move ||crate::stop_timer(timer));
    let save_policy=move |p:crate::model::AutoPolicy|{wasm_bindgen_futures::spawn_local(async move {match api::save_auto_policy(&p).await {Ok(())=>{policy.set(p);reload();},Err(e)=>ctx.say(e.message)}});};
    let move_model=move |name:String,up:bool|{wasm_bindgen_futures::spawn_local(async move {match api::move_model(&name,up).await {Ok(())=>reload(),Err(e)=>ctx.say(e.message)}});};
    let reset=move |name:String,id:Option<i64>|{if !web_sys::window().and_then(|w|w.confirm_with_message("重置冷却并允许立即试用？不会删除历史失败或评分。").ok()).unwrap_or(false){return;}wasm_bindgen_futures::spawn_local(async move {match api::reset_model_cooldown(&name,id).await {Ok(())=>reload(),Err(e)=>ctx.say(e.message)}});};
    let channel_name=move |id:i64|chans.get().iter().find(|c|c.id==id).map(|c|c.name.clone()).unwrap_or_else(||format!("渠道 #{id}"));
    view!{
        <div class="page model-observatory">
            <PageHead title="模型" desc="实时运行表现与自动选择"><button class="ghost" disabled=move ||busy.get() on:click=move |_|reload()>"刷新"</button></PageHead>
            {move ||err.get().map(|e|view!{<div class="banner bad">{e}</div>})}
            <section class="route-spotlight">
                <div class="route-spotlight-head"><h2>"自动选择"</h2><code>"auto"</code></div>
                <div class="policy-controls">{[("sequence","顺序优先"),("balanced","轮流调用"),("fastest","速度优先"),("stable","稳定优先")].into_iter().map(|(key,label)|view!{<button class:active=move ||policy.get().strategy==key on:click=move |_|{let mut p=policy.get_untracked();p.strategy=key.into();save_policy(p);}>{label}</button>}).collect_view()}
                    <label class="pick"><input type="checkbox" prop:checked=move ||policy.get().explore on:change=move |e|{let mut p=policy.get_untracked();p.explore=event_target_checked(&e);save_policy(p);}/>"模型探索 · 5%"</label>
                    <button on:click=move |_|ordering.update(|v|*v=!*v)>{move ||if ordering.get(){"结束排序"}else{"调整模型顺序"}}</button>
                </div>
                {move ||{
                    let a=auto.get();
                    if !a.available{return view!{<p class="muted">"暂无已勾选的可用模型"</p>}.into_any();}
                    view!{<div class="route-selections">{[("chat","对话"),("embeddings","嵌入"),("rerank","重排")].into_iter().filter_map(|(kind,label)|{
                        let names=auto_list(&a,kind);if names.is_empty(){return None;}
                        let first=names[0].clone();let rest=names.join(" → ");
                        Some(view!{<div class="route-selection"><small>{label}</small><b>{first}</b><span class="muted small">{move || match policy.get().strategy.as_str(){"sequence"=>"从第一模型开始，失败才切换","balanced"=>"轮流分配，失败换下一模型","fastest"=>"优先响应更快的模型",_=>"优先近期更稳定的模型"}}</span><details><summary>"候选顺序"</summary><p>{rest}</p></details></div>})
                    }).collect_view()}</div>}.into_any()
                }}
            </section>
            <div class="score-toolbar"><span>"运行评分 · 最近 2 小时"</span><span class="muted small">{move ||format!("更新 {}",fmt::ts_short(updated.get()))}</span></div>
            <nav class="kind-tabs">{KIND_TABS.iter().enumerate().map(|(i,k)|{let key=k.key;view!{<button class=move ||format!("kind-tab{}",if tab.get()==key{" on"}else{""}) on:click=move |_|tab.set(key.into())>{k.title}<span class="tab-n">{move ||tab_counts(&build_rows(&routes.get()))[i]}</span></button>}}).collect_view()}</nav>
            <div class="model-controls"><input type="search" placeholder="搜索模型名称" aria-label="搜索模型名称" prop:value=search on:input=move |e|search.set(event_target_value(&e))/><button class:active=move ||sort_score.get() on:click=move |_|sort_score.set(true)>"按评分"</button><button class:active=move ||!sort_score.get() on:click=move |_|sort_score.set(false)>"按名称"</button></div>
            <div class="score-model-list">{move ||{
                let needle=search.get().to_lowercase();let selected=tab.get();
                let mut list:Vec<_>=routes.get().into_iter().filter(|r|tab_of(&r.kind)==selected && r.name.to_lowercase().contains(&needle)).collect();
                if ordering.get(){let names=order.get();list.sort_by_key(|r|names.iter().position(|n|n==&r.name).unwrap_or(usize::MAX));}else if sort_score.get(){list.sort_by(|a,b|b.runtime.score.unwrap_or(-1.0).total_cmp(&a.runtime.score.unwrap_or(-1.0)).then(a.name.cmp(&b.name)));}else{list.sort_by(|a,b|a.name.cmp(&b.name));}
                if list.is_empty(){return view!{<div class="model-empty">"当前分类暂无匹配模型"</div>}.into_any();}
                list.into_iter().map(|r|{
                    let reorder_name=r.name.clone();let reorder_down=r.name.clone();let reset_name=r.name.clone();
                    let key=r.name.clone();let opened=open.get().as_deref()==Some(&key);let s=r.runtime.clone();
                    let score=s.score.map(|v|format!("{v:.1}")).unwrap_or_else(||"—".into());
                    let confidence=if s.samples==0{"暂无评分"}else if s.samples<5{"样本不足"}else{"运行评分"};
                    let status=if r.routed{"已配置"}else{"无可用渠道"};
                    let source=kind_source_label(&r.kind_source);
                    let (context,context_title)=context_badge(&r);
                    let channels=r.channels.iter().map(|id|channel_name(*id)).collect::<Vec<_>>().join("、");
                    let route_detail=chans.get().into_iter().filter(|c|r.declared_by.contains(&c.id)).map(|c|{
                        let upstream=c.models.iter().find(|m|tokengo_shared::same_model_name(&m.model,&r.name)).and_then(|m|m.upstream.clone()).unwrap_or(r.name.clone());format!("{} → {}",c.name,upstream)
                    }).collect::<Vec<_>>().join("\n");
                    view!{<article class="score-model"><header><div><h2>{r.name.clone()}</h2><div class="model-badges"><span class="tag">{r.kind_label.clone()}</span><span class="tag" title="用途来源；仅说明已识别的用途，不代表图片或音频输入已经实测">{source}</span><span class="tag">{status}</span><span class="tag" title=context_title>{context.clone()}</span>{r.can_see.then(||view!{<span class="tag vision" title="带图路由已启用；可能来自上游声明、人工设置或视觉实测，单凭此标记不能认定已实测">"可接图片"</span>})}{r.kind_conflict.then(||view!{<span class="tag warn">"类型冲突"</span>})}</div></div><div class="score-number"><b>{score}</b><small>{confidence}</small></div></header>
                        <div class="model-telemetry"><span><small>"成功率"</small><b>{if s.samples>0 {fmt::pct(s.successes,s.samples)}else{"—".into()}}</b></span><span><small>"成功平均耗时"</small><b>{s.avg_ms.map(fmt::dur).unwrap_or_else(||"—".into())}</b></span><span><small>"总调用"</small><b>{r.total_calls}</b></span></div>
                        <div class="model-cooling">{r.cooldowns.iter().map(|c|{let id=c.channel_id;let name=r.name.clone();let remaining=(c.until_ms.max(c.lease_until)-updated.get()).max(0);view!{<div><span>{format!("{} · {} · 连续失败 {}",channel_name(id),if remaining>0{format!("冷却剩余 {}",fmt::span(remaining))}else if c.failures>0{"待试用".into()}else{"可用".into()},c.failures)}</span>{(c.failures>0).then(||view!{<button on:click=move |_|reset(name.clone(),Some(id))>"重置冷却"</button>})}</div>}}).collect_view()}</div>
                        {ordering.get().then(||view!{<div class="policy-controls"><button on:click=move |_|move_model(reorder_name.clone(),true)>"上移"</button><button on:click=move |_|move_model(reorder_down.clone(),false)>"下移"</button></div>})}
                        <footer><span>{channels}</span><button class="ghost" aria-expanded=opened on:click=move |_|open.update(|v|*v=if v.as_ref()==Some(&key){None}else{Some(key.clone())})>{if opened{"收起"}else{"详情"}}</button></footer>
                        {opened.then(||view!{<div class="model-detail-grid">
                            <section><h3>"累计调用"</h3><dl><div><dt>"总调用"</dt><dd>{r.total_calls}</dd></div><div><dt>"探索调用"</dt><dd>{r.exploration_count}</dd></div></dl></section>
                            <section><h3>"近2小时表现"</h3><dl><div><dt>"成功 / 失败"</dt><dd>{format!("{} / {}",s.successes,s.faults)}</dd></div><div><dt>"评分尝试数"</dt><dd>{s.samples}</dd></div><div><dt>"平均首字节"</dt><dd>{s.avg_first_byte_ms.map(fmt::dur).unwrap_or_else(||"未记录".into())}</dd></div><div><dt>"最近调用"</dt><dd>{s.last_at.map(fmt::ts).unwrap_or_else(||"无记录".into())}</dd></div></dl></section>
                            <section><h3>"能力信息"</h3><dl><div><dt>"类型来源"</dt><dd>{source}</dd></div><div><dt>"识别状态"</dt><dd>{if r.detection_status.is_empty(){"无待处理检测".into()}else{r.detection_status.clone()}}</dd></div><div><dt>"上下文"</dt><dd>{context}</dd></div></dl></section>
                            <section><h3>"渠道与冷却"</h3><p class="model-mapping">{route_detail}</p>{r.cooldowns.iter().filter(|c|c.failures>0).map(|c|view!{<div class="model-cooldown-detail"><b>{channel_name(c.channel_id)}</b><span>{format!("连续失败 {} · 可重试 {}",c.failures,fmt::ts(c.until_ms))}</span><p>{c.reason.clone()}</p></div>}).collect_view()}<button on:click=move |_|reset(reset_name.clone(),None)>"重置全部冷却"</button></section>
                            {s.last_error.map(|e|view!{<details class="model-last-error"><summary>"最近失败原因"</summary><p>{e}</p></details>})}
                        </div>})}
                    </article>}
                }).collect_view().into_any()
            }}</div>
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

    #[test]
    fn context_badges_keep_unknowns_and_claims_honest() {
        let mut r = route("128k-in-name", "chat", &[1], &[1]);
        assert_eq!(context_badge(&r).0, "上下文 未知");
        for invalid in [0, -1] {
            r.context_window = Some(invalid);
            assert_eq!(context_badge(&r).0, "上下文 未知");
        }
        r.context_window = Some(131072);
        r.context_source = "upstream".into();
        let (label, title) = context_badge(&r);
        assert_eq!(label, "上下文 131K · 上游声明");
        assert!(title.contains("131072 tokens"));
        assert!(title.contains("非容量实测"));
        r.context_window = None;
        r.nominal_context_window = Some(8192);
        r.nominal_context_source = Some("models.dev".into());
        r.context_reason = "no exact ID for custom alias".into();
        let (label, title) = context_badge(&r);
        assert!(label.contains("models.dev 标称"));
        assert!(title.contains("渠道未验证"));
        r.nominal_context_window = None;
        assert!(context_badge(&r).1.contains("no exact ID"));
        r.context_window = Some(131072);
        r.context_source = "future-source".into();
        assert!(context_badge(&r).0.contains("来源未注明"));
    }

    #[test]
    fn declared_and_measured_kinds_have_distinct_labels() {
        assert_eq!(kind_source_label("upstream"), "上游声明");
        assert_eq!(kind_source_label("auto"), "实测识别");
        assert_eq!(kind_source_label("manual"), "人工设置");
        assert_eq!(kind_source_label(""), "待识别");
        assert_eq!(kind_source_label("future-source"), "来源未注明");
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
