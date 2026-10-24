//! 按服务端事实汇总对外逻辑模型，默认只显示公开、启用且可路由的模型。
//! 停用与兼容隐藏项可显式筛选恢复；服务商来源与底层连接分开。
//! 推理档位是原生参数，不按名称后缀合并或推断；运行评分与策略放高级区域。

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api;
use crate::fmt;
use crate::model::{AutoRoutes, Channel, NameRoute};
use crate::pages::PageHead;
use crate::Ctx;

pub(crate) fn model_visible_in(r:&NameRoute,filter:&str)->bool {match filter {"all"=>true,"disabled"=>r.visible&&(!r.enabled||!r.routed),"hidden"=>!r.visible,"internal"=>r.internal_only,_=>r.visible&&r.enabled&&r.routed}}
pub(crate) fn source_labels(r:&NameRoute)->Vec<String>{
    let mut names=r.sources.iter().map(|s|s.provider_name.as_deref().filter(|n|!n.trim().is_empty()).unwrap_or("未分组").to_string()).collect::<Vec<_>>();
    if names.is_empty(){names.push("未分组".into());}names.sort();names.dedup();names
}
pub(crate) fn purpose_label(r:&NameRoute)->&str {if r.catalog_purpose_label.trim().is_empty(){&r.kind_label}else{&r.catalog_purpose_label}}
pub(crate) fn purpose_kind(r:&NameRoute)->&str {if r.catalog_purpose.is_empty(){&r.kind}else{&r.catalog_purpose}}
pub(crate) fn model_title(r:&NameRoute)->&str {if r.display_name.trim().is_empty(){&r.name}else{&r.display_name}}
pub(crate) fn model_matches(r:&NameRoute,provider:&str,search:&str)->bool {
    let labels=source_labels(r);let needle=search.trim().to_lowercase();
    (provider.is_empty()||labels.iter().any(|p|p==provider)) && (r.name.to_lowercase().contains(&needle)||r.display_name.to_lowercase().contains(&needle)||labels.iter().any(|p|p.to_lowercase().contains(&needle)))
}
pub(crate) fn provider_options(routes:&[NameRoute])->Vec<String>{let mut labels=routes.iter().flat_map(source_labels).collect::<Vec<_>>();labels.sort();labels.dedup();labels}


/// Native modal dialogs keep expanded statistics out of the card grid and
/// provide browser-managed focus, Escape dismissal and focus restoration.
fn show_model_details(id: &str, ctx: Ctx) {
    let opened = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|dialog| {
            js_sys::Reflect::get(dialog.as_ref(), &"showModal".into())
                .ok()?
                .dyn_into::<js_sys::Function>()
                .ok()?
                .call0(dialog.as_ref())
                .ok()
        });
    if opened.is_none() {
        ctx.say("无法打开模型详情，请刷新页面或更新浏览器。");
    }
}

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

/// 七档，顺序固定（用户口径；参照 freellmapi 把用途当标签用）。
pub(crate) const KIND_TABS: [KindTab; 7] = [
    KindTab {
        key: "chat",
        title: "对话模型",
        kinds: &["chat"],
        css: "kind-chat",
    },
    // 重排归在嵌入这一档：同样是「检索类」用法、同样不是拿来聊天的；行上会标「重排」。
    KindTab {
        key: "embeddings",
        title: "向量与重排",
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
        title: "视频模型",
        kinds: &["video"],
        css: "kind-video",
    },
    KindTab {
        key: "audio",
        title: "音频",
        kinds: &["audio"],
        css: "kind-audio",
    },
    KindTab {
        key: "tools",
        title: "工具服务",
        kinds: &["tools"],
        css: "kind-tools",
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

// Compact mobile category glyphs; full names remain on the button's accessible label.
fn model_kind_icon(index: usize) -> impl IntoView {
    let path = match index {
        0 => "M5 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-6 4V6a2 2 0 0 1 2-2Z",
        1 => "M4 6h5v5H4zM15 13h5v5h-5zM9 8h8v5M6 11v5h9",
        2 => "M3 4h18v16H3zM3 16l6-6 5 5 3-3 4 4M16 8h.01",
        3 => "M3 5h12v14H3zM15 10l6-4v12l-6-4",
        4 => "M4 10v4M8 6v12M12 3v18M16 6v12M20 10v4",
        5 => "M14 4a6 6 0 0 0-7 7l-4 4a3 3 0 0 0 4 4l4-4a6 6 0 0 0 9-7l-4 4-4-4 4-4Z",
        _ => "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20ZM9 8a3 3 0 0 1 6 0c0 2-3 2-3 5M12 17h.01",
    };
    view! { <svg class="model-kind-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d=path /></svg> }
}

/// 表格里的一行：一个模型名 + 它现在的全部事实（服务端算好的）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelRow {
    pub catalog_purpose:String,
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
            catalog_purpose:r.catalog_purpose.clone(),
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
    rows.iter().filter(|r| tab_of(if r.catalog_purpose.is_empty(){&r.kind}else{&r.catalog_purpose}) == key).cloned().collect()
}

/// 每一档有几行（与 [`KIND_TABS`] 一一对应）—— 分档按钮上的数字用它。
pub(crate) fn tab_counts(rows: &[ModelRow]) -> Vec<usize> {
    KIND_TABS
        .iter()
        .map(|t| rows.iter().filter(|r| tab_of(if r.catalog_purpose.is_empty(){&r.kind}else{&r.catalog_purpose}) == t.key).count())
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

// Polling must not replace the card DOM while its editor/detail dialog is open.
// Check again after the request to cover opening a dialog during an in-flight poll.
fn model_details_open() -> bool {
    web_sys::window().and_then(|w|w.document()).and_then(|d|d.query_selector(".compact-model-dialog[open]").ok().flatten()).is_some()
}

/// Independent access control: compatibility hiding does not revoke direct-call rights.
#[component]
fn ModelAccess(ctx:Ctx,name:String,internal_only:bool)->impl IntoView {
    let internal=RwSignal::new(internal_only);
    let busy=RwSignal::new(false);
    let error=RwSignal::new(String::new());
    let notice=RwSignal::new(String::new());
    let save=move |_| {
        if busy.get_untracked(){return;}
        let next=!internal.get_untracked();let name=name.clone();
        busy.set(true);error.set(String::new());notice.set(String::new());
        leptos::task::spawn_local(async move {
            let path=format!("/api/model-access?model={}",js_sys::encode_uri_component(&name));
            match api::call("PUT",&path,Some(serde_json::json!({"internal_only":next}))).await {
                Ok(_)=>{internal.set(next);notice.set(if next{"已设为仅内部调用。用户无法直接调用此名称，请通过已授权共享模型访问。"}else{"已恢复普通模型访问，仍受用户和 Key 的授权限制。"}.into());},
                Err(e)=>{ctx.note_err(&e);error.set(e.message);}
            }
            busy.set(false);
        });
    };
    view!{<section><h3>"访问权限"</h3><p class="muted small">"开启后，用户不能直接调用或通过私有策略使用此模型；管理员共享模型仍可将它作为内部目标。兼容隐藏仅控制模型发现，两项独立。"</p><button type="button" class="ghost" role="switch" aria-checked=move||internal.get().to_string() disabled=move||busy.get() on:click=save>{move||if busy.get(){"正在保存访问权限…"}else if internal.get(){"仅内部调用：已开启"}else{"仅内部调用：已关闭"}}</button><p role="alert">{move||error.get()}</p><p role="status">{move||notice.get()}</p></section>}
}

#[component]
fn ModelPresentation(ctx:Ctx,name:String,display_name:String,dialog_id:String,on_saved:Callback<()>)->impl IntoView {
    let value=RwSignal::new(display_name);let busy=RwSignal::new(false);let error=RwSignal::new(String::new());
    let input_id=format!("model-display-{}",js_sys::encode_uri_component(&name));
    let save=move |ev:leptos::ev::SubmitEvent| {
        ev.prevent_default();if busy.get_untracked(){return;}busy.set(true);error.set(String::new());
        let name=name.clone();let dialog_id=dialog_id.clone();let display_name=value.get_untracked().trim().to_string();
        leptos::task::spawn_local(async move {
            let path=format!("/api/model-presentation?model={}",js_sys::encode_uri_component(&name));
            match api::call("PUT",&path,Some(serde_json::json!({"display_name":display_name}))).await {
                Ok(_)=>{
                    if let Some(d)=web_sys::window().and_then(|w|w.document()).and_then(|d|d.get_element_by_id(&dialog_id)){if let Ok(f)=js_sys::Reflect::get(d.as_ref(),&"close".into()).and_then(|v|v.dyn_into::<js_sys::Function>()){let _=f.call0(d.as_ref());}}
                    ctx.say("展示名称已保存，API 调用名和授权保持不变");on_saved.run(());
                },Err(e)=>{ctx.note_err(&e);error.set(e.message);}
            }busy.set(false);
        });
    };
    view!{<section><h3>"展示名称"</h3><form on:submit=save><div class="account-field"><label for=input_id.clone()>"目录显示名称"</label><input id=input_id maxlength="128" prop:value=value on:input=move|e|value.set(event_target_value(&e))/></div><p class="muted small">"仅用于目录展示；API 调用名、回退顺序和授权不变。留空恢复原名称。"</p><button type="submit" class="ghost" disabled=move||busy.get()>{move||if busy.get(){"正在保存…"}else{"保存展示名称"}}</button><p role="alert">{move||error.get()}</p></form></section>}
}

#[component]
pub fn Models(ctx:Ctx)->impl IntoView {
    let auto_policy_open=RwSignal::new(false);
    let route_policy_open=RwSignal::new(false);
    let route_policy_busy=RwSignal::new(false);
    let policy=RwSignal::new(crate::model::AutoPolicy::default());
    let policy_busy=RwSignal::new(false);
    let policy_message=RwSignal::new(String::new());
    let order=RwSignal::new(Vec::<String>::new());
    let ordering=RwSignal::new(false);
    let routes=RwSignal::new(Vec::<NameRoute>::new());
    let auto=RwSignal::new(AutoRoutes::default());
    let chans=RwSignal::new(Vec::<Channel>::new());
    let busy=RwSignal::new(false);let err=RwSignal::new(None::<String>);
    let provider_filter=RwSignal::new(String::new());
    let visibility=RwSignal::new("enabled".to_string());let search=RwSignal::new(String::new());let tab=RwSignal::new("chat".to_string());
    let sort_score=RwSignal::new(false);let updated=RwSignal::new(0i64);
    let reload=move || {
        if busy.get_untracked() || model_details_open(){return;} busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let routing_result=api::model_routes().await;
            let channels_result=api::channels().await;
            let policy_result=api::auto_policy().await;
            if model_details_open(){busy.set(false);return;}
            match routing_result {Ok(r)=>{routes.set(r.models);auto.set(r.auto);updated.set(r.now_ms);err.set(None);},Err(e)=>{ctx.note_err(&e);err.set(Some(e.message));}}
            if let Ok(c)=channels_result{chans.set(c);}
            if let Ok(p)=policy_result{policy.set(p.policy);order.set(p.order);}
            busy.set(false);
        });
    };
    reload();let timer=crate::every(5000,reload);on_cleanup(move ||crate::stop_timer(timer));
    let save_policy=move |p:crate::model::AutoPolicy|{
        if policy_busy.get_untracked(){return;} policy_busy.set(true);policy_message.set(String::new());
        wasm_bindgen_futures::spawn_local(async move {
            match api::save_auto_policy(&p).await {
                Ok(())=>{match api::auto_policy().await {Ok(saved)=>{policy.set(saved.policy);order.set(saved.order);policy_message.set("自动选择策略已保存，并重新读取确认。".into());},Err(e)=>{ctx.note_err(&e);policy_message.set(format!("已提交，重新读取确认失败：{}",e.message));}}
                    if let Ok(r)=api::model_routes().await{routes.set(r.models);auto.set(r.auto);updated.set(r.now_ms);}},
                Err(e)=>{ctx.note_err(&e);policy_message.set(e.message);}
            }
            policy_busy.set(false);
        });
    };
    let move_model=move |name:String,up:bool|{wasm_bindgen_futures::spawn_local(async move {match api::move_model(&name,up).await {Ok(())=>reload(),Err(e)=>ctx.say(e.message)}});};
    let toggle_model=move |name:String,enabled:bool|{wasm_bindgen_futures::spawn_local(async move {match api::set_model_enabled(&name,enabled).await{Ok(())=>{ctx.say(if enabled{"模型已启用"}else{"模型已停用"});reload();},Err(e)=>ctx.say(e.message)}});};
    let channel_name=move |id:i64|chans.get().iter().find(|c|c.id==id).map(|c|c.name.clone()).unwrap_or_else(||format!("渠道 #{id}"));
    view!{
        <div class="page model-observatory">
            <PageHead title="模型与路由" desc="汇总服务商中已启用且可路由的对外模型；推理档位作为参数，不另建模型名。"><button class="ghost" disabled=move ||busy.get() on:click=move |_|reload()>"刷新"</button></PageHead>
            {move ||err.get().map(|e|view!{<div class="banner bad">{e}</div>})}
            <div class="toolbar"><button type="button" class="ghost" aria-haspopup="dialog" on:click=move |_|auto_policy_open.set(true)>"自动选择与路由策略"</button><button type="button" class="ghost" aria-haspopup="dialog" on:click=move |_|route_policy_open.set(true)>"模型路由与回退约束"</button></div><super::ModuleDialog open=auto_policy_open busy=policy_busy title="自动选择与路由策略".to_string()><section class="route-spotlight">
                <div class="auto-header"><div class="route-spotlight-head"><h2>"自动选择模型"</h2><code>"auto"</code></div><div class="auto-header-actions">                    <label class="pick"><input type="checkbox" disabled=move||policy_busy.get() prop:checked=move ||policy.get().explore on:change=move |e|{let mut p=policy.get_untracked();p.explore=event_target_checked(&e);save_policy(p);}/>"允许 5% 请求探索其他候选"</label>
                    <button on:click=move |_|{ordering.update(|v|*v=!*v);auto_policy_open.set(false);}>{move ||if ordering.get(){"结束排序"}else{"调整模型顺序"}}</button></div></div>
                <p class="muted small">"客户端将模型名填写为 auto 时，网关会在下面的已接入模型中按所选策略选择，并在失败时尝试其他候选。这里配置的是跨模型自动选择；单个模型的上游 Key 顺序请使用“模型路由与回退约束”。显示的候选来自服务端实时状态，会随配置与可用性变化。"</p><div class="policy-controls">{[("sequence","顺序优先"),("balanced","轮流调用"),("fastest","速度优先"),("stable","稳定优先"),("idle","空闲优先")].into_iter().map(|(key,label)|view!{<button type="button" disabled=move||policy_busy.get() aria-pressed=move||policy.get().strategy==key class:active=move ||policy.get().strategy==key on:click=move |_|{let mut p=policy.get_untracked();p.strategy=key.into();save_policy(p);}>{label}</button>}).collect_view()}

                </div>
                {move ||{
                    let a=auto.get();
                    if !a.available{return view!{<p class="muted">"暂无可参与 auto 的模型。请先接入并启用模型，再在该模型的路由约束中允许参与自动选择。"</p>}.into_any();}
                    view!{<div class="route-selections">{[("chat","对话"),("embeddings","嵌入"),("rerank","重排")].into_iter().filter_map(|(kind,label)|{
                        let names=auto_list(&a,kind);if names.is_empty(){return None;}
                        let candidates=names.to_vec();let count=candidates.len();
                        Some(view!{<div class="route-selection"><h3>{format!("{label} · {count} 个候选模型")}</h3><span class="muted small">{move || match policy.get().strategy.as_str(){"sequence"=>"从第一模型开始，失败才切换","balanced"=>"轮流分配，失败换下一模型","fastest"=>"优先响应更快的模型","idle"=>"优先当前在途请求更少的模型",_=>"优先近期更稳定的模型"}}</span><ol>{candidates.into_iter().map(|name|view!{<li><code>{name}</code></li>}).collect_view()}</ol></div>})
                    }).collect_view()}</div>}.into_any()
                }}
                <p role="status">{move||if policy_busy.get(){"正在保存自动选择策略…".into()}else{policy_message.get()}}</p>
            </section>
            </super::ModuleDialog><Show when=move ||ordering.get()><div class="toolbar"><p>"使用列表中的上移、下移调整顺序。"</p><button type="button" class="primary" on:click=move |_|ordering.set(false)>"完成排序"</button></div></Show><div class="score-toolbar"><span>"服务商已配置模型"</span><span class="muted small">{move ||format!("更新 {}",fmt::ts_short(updated.get()))}</span></div>
            <nav class="kind-tabs" aria-label="模型用途分类">{KIND_TABS.iter().enumerate().map(|(i,k)|{let key=k.key;let title=k.title;view!{<button class=move ||format!("kind-tab{}",if tab.get()==key{" on"}else{""}) title=title aria-label=move ||format!("{}，{}个模型",title,tab_counts(&build_rows(&routes.get().into_iter().filter(|r|model_visible_in(r,&visibility.get())&&model_matches(r,&provider_filter.get(),&search.get())).collect::<Vec<_>>()))[i]) aria-pressed=move ||(tab.get()==key).to_string() on:click=move |_|tab.set(key.into())>{model_kind_icon(i)}<span class="model-kind-title">{k.title}</span><span class="tab-n">{move ||tab_counts(&build_rows(&routes.get().into_iter().filter(|r|model_visible_in(r,&visibility.get())&&model_matches(r,&provider_filter.get(),&search.get())).collect::<Vec<_>>()))[i]}</span></button>}}).collect_view()}</nav>
            <div class="model-controls"><div class="model-filter-fields"><div class="model-filter-field"><label for="model-visibility">"显示"</label><select id="model-visibility" prop:value=visibility on:change=move|e|visibility.set(event_target_value(&e))><option value="enabled">"已启用"</option><option value="disabled">"已停用 / 无可用连接"</option><option value="hidden">"兼容隐藏"</option><option value="internal">"仅内部调用"</option><option value="all">"全部"</option></select></div><div class="model-filter-field"><label for="model-provider">"供应商"</label><select id="model-provider" prop:value=provider_filter on:change=move|e|provider_filter.set(event_target_value(&e))><option value="">"全部供应商"</option>{move||provider_options(&routes.get().into_iter().filter(|r|model_visible_in(r,&visibility.get())).collect::<Vec<_>>()).into_iter().map(|p|view!{<option value=p.clone()>{p.clone()}</option>}).collect_view()}</select></div></div><input type="search" placeholder="搜索展示名称、调用名或供应商" aria-label="搜索模型名称或供应商" prop:value=search on:input=move |e|search.set(event_target_value(&e))/><div class="model-sort-controls" role="group" aria-label="模型排序"><button type="button" title="按评分排序" aria-label="按评分" aria-pressed=move ||sort_score.get().to_string() class:active=move ||sort_score.get() on:click=move |_|sort_score.set(true)><svg class="model-sort-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"><path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2-5.6-3-5.6 3 1.1-6.2L3 9.6l6.2-.9Z" /></svg><span class="model-sort-label">"按评分"</span></button><button type="button" title="按名称排序" aria-label="按名称" aria-pressed=move ||(!sort_score.get()).to_string() class:active=move ||!sort_score.get() on:click=move |_|sort_score.set(false)><svg class="model-sort-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="m3 17 5-12 5 12M5 13h6M19 5v14m-3-3 3 3 3-3" /></svg><span class="model-sort-label">"按名称"</span></button></div></div>
            <div class="model-record-head" aria-hidden="true"><span>"模型与调用名"</span><span>"供应商 / 可用连接"</span><span>"全局状态"</span><span>"操作"</span></div><div class="score-model-list">{move ||{
                let needle=search.get().to_lowercase();let selected=tab.get();
                let mut list:Vec<_>=routes.get().into_iter().filter(|r|model_visible_in(r,&visibility.get())&&tab_of(purpose_kind(r))==selected && model_matches(r,&provider_filter.get(),&needle)).collect();
                if ordering.get(){let names=order.get();list.sort_by_key(|r|names.iter().position(|n|n==&r.name).unwrap_or(usize::MAX));}else if sort_score.get(){list.sort_by(|a,b|b.runtime.score.unwrap_or(-1.0).total_cmp(&a.runtime.score.unwrap_or(-1.0)).then(a.name.cmp(&b.name)));}else{list.sort_by(|a,b|a.name.cmp(&b.name));}
                if list.is_empty(){return view!{<div class="model-empty">"当前分类暂无匹配模型"</div>}.into_any();}
                list.into_iter().map(|r|{
                    let reorder_name=r.name.clone();let reorder_down=r.name.clone();
                    let toggle_name=r.name.clone();let channel_available=!r.channels.is_empty();let model_enabled=r.enabled;
                    let visible_name=r.name.clone();let s=r.runtime.clone();
                    let dialog_id=format!("model-details-{}",js_sys::encode_uri_component(&r.name));
                    let trigger_dialog=dialog_id.clone();let visibility_dialog=dialog_id.clone();
                    let title=model_title(&r).to_string();let detail_title=format!("{} 的详情与统计",title);
                    let labels=source_labels(&r);
                    let supplier_summary=if labels.is_empty(){"未分组".to_string()}else{labels.join("、")};
                    let score=s.score.map(|v|format!("{v:.1}")).unwrap_or_else(||"—".into());
                    let confidence=if s.samples==0{"暂无评分"}else if s.samples<5{"样本不足"}else{"运行评分"};
                    let status=if !channel_available{"已停用（无可用渠道）"}else if !r.enabled{"已停用"}else{"已配置"};
                    let source=if r.catalog_purpose.is_empty(){kind_source_label(&r.kind_source)}else{"目录分类"};
                    let catalog_label=purpose_label(&r).to_string();
                    let (context,context_title)=context_badge(&r);

                    view!{<article class=if ordering.get(){"score-model score-model-ordering"}else{"score-model"}><header><div class="model-record-identity"><h2 title=r.name.clone()>{title.clone()}</h2><p class="muted small">"API 调用名："<code>{r.name.clone()}</code></p><div class="model-badges" title="完整能力信息可在详情中查看"><span class="tag">{catalog_label}</span><span class="tag" title="用途来源；仅说明已识别的用途，不代表图片或音频输入已经实测">{source}</span><span class="tag">{status}</span>{r.internal_only.then(||view!{<span class="tag">"仅内部调用"</span>})}<span class="tag" title=context_title>{context.clone()}</span>{r.can_see.then(||view!{<span class="tag vision" title="带图路由已启用；可能来自上游声明、人工设置或视觉实测，单凭此标记不能认定已实测">"可接图片"</span>})}{r.kind_conflict.then(||view!{<span class="tag warn">"类型冲突"</span>})}</div></div><div class="model-card-actions"><button class="model-toggle" disabled=r.declared_by.is_empty() role="switch" aria-checked=if model_enabled{"true"}else{"false"} on:click=move |_|toggle_model(toggle_name.clone(),!model_enabled)>{if model_enabled{"全局已启用"}else{"全局已停用"}}</button></div></header>
                        <div class="model-status-row"><div class="model-cooling"><div class="model-supplier-summary" title=supplier_summary.clone()>{supplier_summary.clone()}</div><span class="muted small">{format!("{} 个可用连接",r.channels.len())}</span></div><button type="button" class="ghost" aria-haspopup="dialog" aria-controls=dialog_id.clone() on:click=move |_|show_model_details(&trigger_dialog,ctx)>"详情与统计"</button></div>
                        {ordering.get().then(||view!{<div class="policy-controls"><button on:click=move |_|move_model(reorder_name.clone(),true)>"上移"</button><button on:click=move |_|move_model(reorder_down.clone(),false)>"下移"</button></div>})}
                        <dialog class="compact-model-dialog model-detail-drawer" id=dialog_id.clone() aria-label=detail_title><div class="compact-model-dialog-head"><h2>{title}</h2><form method="dialog"><button type="submit" class="ghost" autofocus>"关闭"</button></form></div><div class="model-detail-grid">
                            <section class="compact-model-statistics"><h3>"运行统计"</h3><div class="model-telemetry"><span><small>{confidence}</small><b>{score}</b></span><span><small>"成功率"</small><b>{if s.samples>0 {fmt::pct(s.successes,s.samples)}else{"—".into()}}</b></span><span><small>"成功平均耗时"</small><b>{s.avg_ms.map(fmt::dur).unwrap_or_else(||"—".into())}</b></span><span><small>"总调用"</small><b>{r.total_calls}</b></span></div></section>
                            <ModelPresentation ctx=ctx name=r.name.clone() display_name=r.display_name.clone() dialog_id=dialog_id.clone() on_saved=Callback::new(move|_:()|reload())/><ModelAccess ctx=ctx name=r.name.clone() internal_only=r.internal_only/><section><h3>"展示与连接"</h3><p class="muted small">"兼容隐藏仅从模型清单和 auto 排除，有权用户仍可显式调用；仅内部调用会拒绝用户直调，全局停用则拒绝该逻辑名。"</p><button class="ghost" on:click=move|_|{let n=visible_name.clone();let close_id=visibility_dialog.clone();leptos::task::spawn_local(async move{let path=format!("/api/model-visibility?model={}",js_sys::encode_uri_component(&n));match crate::api::call("PUT",&path,Some(serde_json::json!({"visible":!r.visible}))).await{Ok(_)=>{if let Some(d)=web_sys::window().and_then(|w|w.document()).and_then(|d|d.get_element_by_id(&close_id)){if let Ok(f)=js_sys::Reflect::get(d.as_ref(),&"close".into()).and_then(|v|v.dyn_into::<js_sys::Function>()){let _=f.call0(d.as_ref());}}ctx.say("模型展示设置已保存");reload();},Err(e)=>ctx.say(e.message)}});}>{if r.visible{"设为兼容隐藏"}else{"恢复公开展示"}}</button>{r.declared_by.iter().map(|id|view!{<p>{channel_name(*id)}</p>}).collect_view()}</section><section><h3>"原生推理档位"</h3><p class="muted small">"以每条连接真实声明为准；客户端显式参数优先。"</p>{r.sources.iter().map(|s|{let cap=s.reasoning.clone().unwrap_or_default();view!{<p>{format!("{}：{}",s.provider_name.clone().unwrap_or_else(||"未分组".into()),super::model_reasoning::capability_label(&cap))}</p>}}).collect_view()}</section><section><h3>"累计调用"</h3><dl><div><dt>"总调用"</dt><dd>{r.total_calls}</dd></div><div><dt>"探索调用"</dt><dd>{r.exploration_count}</dd></div></dl></section>
                            <section><h3>"近2小时表现"</h3><dl><div><dt>"成功 / 失败"</dt><dd>{format!("{} / {}",s.successes,s.faults)}</dd></div><div><dt>"评分尝试数"</dt><dd>{s.samples}</dd></div><div><dt>"平均首字节"</dt><dd>{s.avg_first_byte_ms.map(fmt::dur).unwrap_or_else(||"未记录".into())}</dd></div><div><dt>"最近调用"</dt><dd>{s.last_at.map(fmt::ts).unwrap_or_else(||"无记录".into())}</dd></div></dl></section>
                            <section><h3>"能力信息"</h3><dl><div><dt>"实际路由类型"</dt><dd>{r.kind_label.clone()}</dd></div><div><dt>"类型来源"</dt><dd>{source}</dd></div><div><dt>"识别状态"</dt><dd>{if r.detection_status.is_empty(){"无待处理检测".into()}else{r.detection_status.clone()}}</dd></div><div><dt>"上下文"</dt><dd>{context}</dd></div></dl></section>
                            {s.last_error.map(|e|view!{<details class="model-last-error"><summary>"最近失败原因"</summary><p>{e}</p></details>})}
                        </div></dialog>
                    </article>}
                }).collect_view().into_any()
            }}</div>
            <super::ModuleDialog open=route_policy_open busy=route_policy_busy title="模型路由与回退约束".to_string()><super::route_policy::RoutePolicy ctx=ctx open=route_policy_open saving=route_policy_busy/></super::ModuleDialog>
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
    fn enabled_summary_excludes_hidden_and_unroutable_but_filters_restore_them(){let mut r=route("ordinary-name","chat",&[1],&[1]);r.enabled=true;r.visible=true;assert!(model_visible_in(&r,"enabled"));r.visible=false;assert!(!model_visible_in(&r,"enabled"));assert!(model_visible_in(&r,"hidden"));assert!(model_visible_in(&r,"all"));r.visible=true;r.routed=false;assert!(model_visible_in(&r,"disabled"));assert!(!model_visible_in(&r,"enabled"));}
    #[test]
    fn internal_access_filter_is_independent_from_compatibility_hiding() {
        let mut r=route("internal-target","embeddings",&[1],&[1]);r.internal_only=true;r.visible=true;
        assert!(model_visible_in(&r,"internal"));assert!(!model_visible_in(&r,"hidden"));
        r.visible=false;assert!(model_visible_in(&r,"internal"));assert!(model_visible_in(&r,"hidden"));
        r.internal_only=false;assert!(!model_visible_in(&r,"internal"));assert!(model_visible_in(&r,"hidden"));
    }
    #[test]
    fn provider_sources_are_explicit_and_deduplicated(){let mut r=route("model","chat",&[1,2],&[1,2]);r.sources=vec![crate::model::ModelSource{provider_name:Some("官方".into()),..Default::default()},crate::model::ModelSource{provider_name:Some("官方".into()),..Default::default()},crate::model::ModelSource::default()];assert_eq!(source_labels(&r),vec!["官方","未分组"]);}
    #[test]
    fn presentation_filter_preserves_identity_and_matches_any_explicit_supplier() {
        let mut r=route("api-stable-id","embeddings",&[1,2],&[2,1]);r.display_name="BGE 向量入口".into();
        r.sources=vec![crate::model::ModelSource{provider_name:Some("Cloud".into()),..Default::default()},crate::model::ModelSource{provider_name:Some("GPU".into()),..Default::default()}];
        assert_eq!(model_title(&r),"BGE 向量入口");assert!(model_matches(&r,"Cloud","bge"));assert!(model_matches(&r,"GPU","API-STABLE"));assert!(model_matches(&r,"","cloud"));assert!(!model_matches(&r,"Other",""));
        assert_eq!(r.name,"api-stable-id");assert_eq!(r.channels,vec![2,1]);assert_eq!(provider_options(&[r.clone(),r.clone()]),vec!["Cloud","GPU"]);
        r.display_name="  ".into();assert_eq!(model_title(&r),"api-stable-id");r.sources.clear();assert!(model_matches(&r,"未分组",""));
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

    /// 分档：按用途落到七档里，认不出来的是「还没定」，重排跟着嵌入那一档。
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
        assert_eq!(kind_tab("tools"), 5);
        assert_eq!(kind_tab("banana"), 6, "认不出来的落最后一档（还没定）");
    }

    #[test]
    fn catalog_purpose_changes_directory_without_changing_route_type_or_chain() {
        let mut asr=route("asr-api","chat",&[1,2],&[2,1]);asr.catalog_purpose="audio".into();asr.catalog_purpose_label="语音识别".into();asr.kind_label="对话".into();
        let mut tools=route("search-api","chat",&[3],&[3]);tools.catalog_purpose="tools".into();tools.catalog_purpose_label="搜索工具".into();
        let rows=build_rows(&[asr.clone(),tools]);
        assert_eq!(rows_in_tab(&rows,"audio").len(),1);assert_eq!(rows_in_tab(&rows,"tools").len(),1);assert!(rows_in_tab(&rows,"chat").is_empty());
        assert_eq!(purpose_label(&asr),"语音识别");assert_eq!(purpose_kind(&asr),"audio");assert_eq!(rows[0].kind,"chat");assert_eq!(asr.kind,"chat");assert_eq!(asr.channels,vec![2,1]);assert_eq!(asr.name,"asr-api");
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
            vec![1, 1, 1, 1, 0, 0, 1],
            "对话/向量与重排/图像/视频/音频/工具/还没定"
        );
        assert_eq!(rows_in_tab(&rows, "embeddings").len(), 1);
        assert_eq!(
            rows_in_tab(&rows, "embeddings")[0].name,
            "BAAI/bge-reranker-v2-m3"
        );
        assert_eq!(rows_in_tab(&rows, "audio").len(), 0, "音频这档现在是空的");
    }

    /// 七档的标题与配色都不重样（重了就看不出区别），顺序固定。
    #[test]
    fn every_tab_has_its_own_title_and_colour() {
        let titles: Vec<&str> = KIND_TABS.iter().map(|t| t.title).collect();
        assert_eq!(
            titles,
            vec!["对话模型", "向量与重排", "图像", "视频模型", "音频", "工具服务", "待识别"]
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
