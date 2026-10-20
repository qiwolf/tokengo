//! Provider-facing management with explicit, persistent grouping.
use crate::{Ctx, api, model::Channel, provider_api as p};
use leptos::prelude::*;
use std::collections::BTreeSet;
use wasm_bindgen::JsCast;

pub(crate) fn model_connections(channels: &[Channel], provider: i64, model: &str) -> Vec<Channel> {
    channels
        .iter()
        .filter(|c| c.provider_id == Some(provider) && c.models.iter().any(|m| m.model == model))
        .cloned()
        .collect()
}
/// Main counts match the default public model table; compatibility aliases are separate.
pub(crate) fn model_counts(models:&[p::ProviderModel])->(usize,usize,usize) {
    let public=models.iter().filter(|m|m.visible).count();
    let enabled=models.iter().filter(|m|m.visible&&m.global_enabled&&!m.routable_connection_ids.is_empty()).count();
    (enabled,public,models.len()-public)
}
pub(crate) fn all_enabled(model: &p::ProviderModel) -> bool {
    !model.declared_connection_ids.is_empty()
        && model
            .declared_connection_ids
            .iter()
            .all(|id| model.enabled_connection_ids.contains(id))
}
pub(crate) fn protocol_names(channels: &[Channel]) -> String {
    let mut names = BTreeSet::new();
    for c in channels {
        if let Some(a) = c
            .protocols
            .as_ref()
            .and_then(|p| p.get("native"))
            .and_then(|v| v.as_array())
        {
            for v in a {
                if let Some(s) = v.as_str() {
                    names.insert(s.to_string());
                }
            }
        }
    }
    if names.is_empty() {
        "未声明".into()
    } else {
        names.into_iter().collect::<Vec<_>>().join(" / ")
    }
}
#[component]
pub fn Providers(ctx: Ctx) -> impl IntoView {
    let providers = RwSignal::new(Vec::<p::Provider>::new());
    let channels = RwSignal::new(Vec::<Channel>::new());
    let err = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let name = RwSignal::new(String::new());
    let reload = Callback::new(move |()| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            match p::list().await {
                Ok(v) => {
                    providers.set(v.providers);
                    err.set(String::new());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(e.message);
                }
            }
            match api::channels().await {
                Ok(v) => channels.set(v),
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(e.message);
                }
            }
            busy.set(false);
        });
    });
    reload.run(());
    let create = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let n = name.get_untracked().trim().to_string();
        if n.is_empty() {
            err.set("请输入服务商名称。".into());
            return;
        }
        busy.set(true);
        leptos::task::spawn_local(async move {
            match p::save(None, &n, true).await {
                Ok(()) => {
                    name.set(String::new());
                    err.set(String::new());
                    reload.run(());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(e.message);
                    busy.set(false);
                }
            }
        });
    };
    view! {<div class="page provider-page"><super::PageHead title="渠道" desc="按服务商管理已配置模型；连接的地址、凭据与协议在高级设置中独立保留。"><button class="ghost" disabled=move||busy.get() on:click=move|_|reload.run(())>"刷新"</button></super::PageHead>
    <form class="provider-create" on:submit=create><div class="account-field"><label for="provider-name">"服务商名称"</label><input id="provider-name" required=true maxlength="128" placeholder="如：OpenAI、DeepSeek、本地推理服务" prop:value=name on:input=move|e|name.set(event_target_value(&e))/></div><button type="submit" class="primary" disabled=move||busy.get()>"新建服务商"</button></form>
    <Show when=move||!err.get().is_empty()><p class="banner bad" role="alert">{move||err.get()}</p></Show>
    <Show when=move||busy.get()&&providers.get().is_empty()><p role="status">"正在读取服务商…"</p></Show>
    <div class="provider-list">{move||providers.get().into_iter().map(|provider|view!{<ProviderCard ctx=ctx provider=provider channels=channels.get() reload=reload/>}).collect_view()}</div>
    <Show when=move||!busy.get()&&providers.get().is_empty()><p class="account-empty">"尚未设置服务商。请新建名称，再将已有连接明确归属。系统不会按网址或模型名猜测。"</p></Show>
    <section class="card provider-ungrouped"><header class="card-head"><h2>"未分组连接"</h2><span>{move||format!("{} 条",channels.get().iter().filter(|c|c.provider_id.is_none()).count())}</span></header><div class="card-body"><p class="muted small">"旧连接保持原 ID 与配置。选择服务商仅修改归属，不合并凭据、预算、并发或路由顺序。"</p>{move||channels.get().into_iter().filter(|c|c.provider_id.is_none()).map(|c|view!{<Assignment ctx=ctx connection=c providers=providers.get() reload=reload/>}).collect_view()}<details class="provider-advanced"><summary>"高级：未分组连接设置 / 新建连接"</summary><super::channels::Connections ctx=ctx on_changed=reload/></details></div></section>
    </div>}
}
#[component]
fn ProviderCard(
    ctx: Ctx,
    provider: p::Provider,
    channels: Vec<Channel>,
    reload: Callback<()>,
) -> impl IntoView {
    let id = provider.id;
    let open = RwSignal::new(false);
    let show_compat=RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let edited = RwSignal::new(provider.name.clone());
    let enabled = RwSignal::new(provider.enabled);
    let provider = StoredValue::new(provider);
    let channels = StoredValue::new(channels);
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        busy.set(true);
        leptos::task::spawn_local(async move {
            match p::save(
                Some(id),
                edited.get_untracked().trim(),
                enabled.get_untracked(),
            )
            .await
            {
                Ok(()) => reload.run(()),
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<section class="card provider-card"><header class="provider-heading"><div><h2>{provider.get_value().name}</h2><p class="muted small">{{let p=provider.get_value();let (enabled,public,_)=model_counts(&p.models);format!("启用 {enabled} / 已配置 {public} 个模型 · {} 个连接",p.connection_ids.len())}}</p>{let (_,_,hidden)=model_counts(&provider.get_value().models);(hidden>0).then(||view!{<p class="muted small">{format!("另有 {hidden} 个兼容别名")}</p>})}</div><span class="tag">{if provider.get_value().enabled{"渠道启用"}else{"渠道停用"}}</span><button class="ghost" aria-expanded=move||open.get().to_string() on:click=move|_|open.update(|v|*v=!*v)>{move||if open.get(){"收起"}else{"管理模型"}}</button></header>
    <Show when=move||open.get()><div class="card-body"><p class="muted small">"这里的开关只控制此服务商内该模型的已列连接。停用保留映射与能力配置；全局模型开关在模型页。"</p>
    <label class="pick"><input type="checkbox" prop:checked=show_compat on:change=move|e|show_compat.set(event_target_checked(&e))/>{format!("显示兼容隐藏模型（{}）",provider.get_value().models.iter().filter(|m|!m.visible).count())}</label><Show when=move||!show_compat.get()&&provider.get_value().models.iter().all(|m|!m.visible)><p class="muted small">"没有公开展示的模型。可以显示兼容隐藏项，或在连接设置中添加标准模型名。"</p></Show><Show when=move||!error.get().is_empty()><p class="banner bad" role="alert">{move||error.get()}</p></Show>
    {if provider.get_value().models.is_empty(){view!{<p class="account-empty">"尚未配置模型，请在下方连接设置中配置映射。"</p>}.into_any()}else{view!{<div class="table-wrap"><table class="provider-model-table"><thead><tr><th>"对外模型名"</th><th>"上游模型"</th><th>"原生协议"</th><th>"连接"</th><th>"此渠道内模型"</th><th>"详情"</th></tr></thead><tbody>{move||provider.get_value().models.into_iter().filter(|m|m.visible||show_compat.get()).map(|m|{let connections=model_connections(&channels.get_value(),id,&m.name);let upstream:Vec<_>=connections.iter().flat_map(|c|c.models.iter().filter(|p|p.model==m.name).map(|p|p.upstream.clone().unwrap_or_else(||p.model.clone()))).collect::<BTreeSet<_>>().into_iter().collect();let protocols=protocol_names(&connections);let on=all_enabled(&m);let mixed=!m.enabled_connection_ids.is_empty()&&!on;let target=m.name.clone();let target_for_details=m.name.clone();let detail_open=RwSignal::new(false);let scope=m.declared_connection_ids.clone();let detail=connections.iter().map(|c|format!("#{} {}：{}",c.id,c.name,if c.models.iter().any(|p|p.model==m.name&&p.enabled){"模型启用"}else{"模型停用"})).collect::<Vec<_>>().join("；");view!{<tr><td data-label="对外模型名"><code>{m.name}</code>{(!m.global_enabled).then(||view!{<span class="tag warn">"全局已停用"</span>})}</td><td data-label="上游模型">{upstream.join(" / ")}</td><td data-label="原生协议">{protocols}</td><td data-label="连接">{format!("可路由 {} / 已配置 {}",m.routable_connection_ids.len(),scope.len())}</td><td data-label="此渠道内模型"><button class="ghost" disabled=move||busy.get() aria-label=format!("{}此渠道内模型 {} 的全部 {} 个连接",if on{"停用"}else{"启用"},target,scope.len()) on:click=move|_|{if busy.get_untracked(){return;}busy.set(true);let n=target.clone();let ids=scope.clone();leptos::task::spawn_local(async move{match p::toggle(id,&n,!on,&ids).await{Ok(())=>reload.run(()),Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});}>{if on{"已启用 · 停用"}else if mixed{"部分启用 · 全部启用"}else{"已停用 · 启用"}}</button></td><td data-label="详情"><details on:toggle=move|ev|{detail_open.set(ev.target().and_then(|t|t.dyn_into::<web_sys::Element>().ok()).is_some_and(|e|e.has_attribute("open")));}><summary>"连接与推理参数"</summary><p>{detail}</p><Show when=move||detail_open.get()>{connections.clone().into_iter().map(|c|view!{<super::model_reasoning::ReasoningConfig ctx=ctx channel=Some(c.id) model=target_for_details.clone()/>}).collect_view()}</Show></details></td></tr>}}).collect_view()}</tbody></table></div>}.into_any()}}
    <details class="provider-advanced"><summary>"高级：服务商与连接设置"</summary><form class="provider-edit" on:submit=save><div class="account-field"><label for=format!("provider-edit-{id}")>"服务商名称"</label><input id=format!("provider-edit-{id}") required=true maxlength="128" prop:value=edited on:input=move|e|edited.set(event_target_value(&e))/></div><label class="pick"><input type="checkbox" prop:checked=enabled on:change=move|e|enabled.set(event_target_checked(&e))/>{"启用整个服务商渠道"}</label><p class="muted small">"停用后其全部连接不参与路由，单个模型配置保留。"</p><button class="primary" disabled=move||busy.get()>"保存服务商"</button></form>{channels.get_value().into_iter().filter(|c|c.provider_id==Some(id)).map(|c|view!{<Assignment ctx=ctx connection=c providers=vec![] reload=reload/>}).collect_view()}<super::channels::Connections ctx=ctx provider_id=id on_changed=reload/></details>
    </div></Show></section>}
}
#[component]
fn Assignment(
    ctx: Ctx,
    connection: Channel,
    providers: Vec<p::Provider>,
    reload: Callback<()>,
) -> impl IntoView {
    let selected = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let err = RwSignal::new(String::new());
    let id = connection.id;
    let assigned = connection.provider_id.is_some();
    let fixed_assignment=assigned&&providers.is_empty();
    let assignment_label=connection.provider_name.clone().unwrap_or_else(||format!("已归属服务商 #{}",connection.provider_id.unwrap_or(0)));
    let submit = move |_| {
        busy.set(true);
        let target = selected.get_untracked().parse::<i64>().ok();
        leptos::task::spawn_local(async move {
            match p::assign(id, target).await {
                Ok(()) => reload.run(()),
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<div class="provider-assignment"><strong>{format!("连接 #{} · {}",id,connection.name)}</strong>{if fixed_assignment{view!{<span class="muted small">{format!("当前归属：{assignment_label}")}</span>}.into_any()}else{view!{<label class="sr-only" for=format!("assign-{id}")>"服务商归属"</label><select id=format!("assign-{id}") prop:value=selected on:change=move|e|selected.set(event_target_value(&e))><option value="">"未分组"</option>{providers.into_iter().map(|p|view!{<option value=p.id.to_string()>{p.name}</option>}).collect_view()}</select>}.into_any()}}<button class="ghost" disabled=move||busy.get() on:click=submit>{if assigned{"移至未分组"}else{"保存归属"}}</button><Show when=move||!err.get().is_empty()><span class="bad-text" role="alert">{move||err.get()}</span></Show></div>}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouping_is_explicit_not_inferred_from_host_or_model() {
        let a = Channel {
            id: 1,
            provider_id: Some(7),
            base_url: "https://same.test".into(),
            models: vec![crate::model::ModelPair {
                model: "same".into(),
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut b = a.clone();
        b.id = 2;
        b.provider_id = None;
        assert_eq!(model_connections(&[a, b], 7, "same").len(), 1);
    }
    #[test]
    fn main_counts_match_visible_table_and_separate_compatibility_aliases(){
        let model=|visible,global_enabled,routable|p::ProviderModel{name:"no-suffix-inference".into(),visible,global_enabled,declared_connection_ids:vec![1],enabled_connection_ids:vec![1],routable_connection_ids:if routable{vec![1]}else{vec![]}};
        let rows=vec![model(true,true,true),model(true,false,true),model(true,true,false),model(false,true,true),model(false,true,true),model(false,true,true)];
        assert_eq!(model_counts(&rows),(1,3,3));
        assert_eq!(model_counts(&[]),(0,0,0));
    }
    #[test]
    fn partial_enable_is_not_all_enabled() {
        let mut m = p::ProviderModel {
            name: "m".into(),
            global_enabled: true,
            visible:true,
            declared_connection_ids: vec![1, 2],
            enabled_connection_ids: vec![1],
            routable_connection_ids: vec![1],
        };
        assert!(!all_enabled(&m));
        m.enabled_connection_ids.push(2);
        assert!(all_enabled(&m));
        m.declared_connection_ids.clear();
        assert!(!all_enabled(&m));
    }
}
