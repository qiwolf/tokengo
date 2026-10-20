//! Provider-facing management with explicit, persistent grouping.
use crate::{Ctx, api, model::Channel, provider_api as p};
use leptos::prelude::*;
use std::collections::BTreeSet;


pub(crate) fn model_connections(channels: &[Channel], provider: i64, model: &str) -> Vec<Channel> {
    channels
        .iter()
        .filter(|c| c.provider_id == Some(provider) && c.models.iter().any(|m| m.model == model))
        .cloned()
        .collect()
}
/// Main counts match the default public model table; compatibility aliases are separate.
pub(crate) fn model_counts(models: &[p::ProviderModel]) -> (usize, usize, usize) {
    let public = models.iter().filter(|m| m.visible).count();
    let enabled = models
        .iter()
        .filter(|m| m.visible && m.global_enabled && !m.routable_connection_ids.is_empty())
        .count();
    (enabled, public, models.len() - public)
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
    let base_url = RwSignal::new(String::new());
    let endpoint_mode = RwSignal::new("single".to_string());
    let ungrouped_open = RwSignal::new(false);
    let creating = RwSignal::new(false);
    let show_disabled = RwSignal::new(false);
    let model_panels = RwSignal::new(BTreeSet::<i64>::new());
    let key_panels = RwSignal::new(BTreeSet::<i64>::new());
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
            match p::save_config(None, serde_json::json!({"name":n,"enabled":true,"base_url":if endpoint_mode.get_untracked()=="local"{None}else{Some(base_url.get_untracked().trim().to_string())},"endpoint_mode":endpoint_mode.get_untracked(),"key_strategy":"ordered","switch_on_429":true,"switch_on_400":false})).await {
                Ok(()) => {
                    creating.set(false);
                    name.set(String::new());
                    base_url.set(String::new());
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
    view! {<div class="page provider-page"><super::PageHead title="渠道" desc="按供应方管理服务地址、上游 Key 和可用模型。本地供应方可包含多个服务地址。"><button type="button" class="primary" on:click=move |_|creating.set(true)>"新建渠道"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move|_|reload.run(())>"刷新"</button></super::PageHead>
    <super::ModuleDialog open=creating busy=busy title="新建渠道".to_string()><form class="provider-create" on:submit=create><div class="account-field"><label for="provider-name">"服务商名称"</label><input id="provider-name" required=true maxlength="128" placeholder="如：OpenAI、DeepSeek、本地推理服务" prop:value=name on:input=move|e|name.set(event_target_value(&e))/></div><label class="account-field"><span>"服务类型"</span><select prop:value=endpoint_mode on:change=move|e|endpoint_mode.set(event_target_value(&e))><option value="single">"在线服务：统一 API 地址"</option><option value="local">"本地服务：多个 API 地址"</option></select></label><Show when=move||endpoint_mode.get()=="single"><div class="account-field"><label for="provider-url">"API URL"</label><input id="provider-url" type="url" required placeholder="https://api.example.com/v1" prop:value=base_url on:input=move|e|base_url.set(event_target_value(&e))/></div></Show><button type="submit" class="primary" disabled=move||busy.get()>"创建渠道"</button></form><Show when=move||!err.get().is_empty()><p class="banner bad" role="alert">{move||err.get()}</p></Show></super::ModuleDialog>
    <Show when=move||!err.get().is_empty()><p class="banner bad" role="alert">{move||err.get()}</p></Show>
    <Show when=move||busy.get()&&providers.get().is_empty()><p role="status">"正在读取服务商…"</p></Show>
    <label class="account-check provider-filter"><input type="checkbox" prop:checked=show_disabled on:change=move|e|show_disabled.set(event_target_checked(&e))/>"显示已停用渠道"</label><div class="provider-list">{move||providers.get().into_iter().filter(|p|p.enabled||show_disabled.get()).map(|provider|view!{<ProviderCard ctx=ctx provider=provider channels=channels.get() all_providers=providers.get() reload=reload model_panels=model_panels key_panels=key_panels/>}).collect_view()}</div>
    <Show when=move||!busy.get()&&providers.get().is_empty()><p class="account-empty">"尚未设置渠道。填写供应方名称与API URL，再添加上游Key和模型。"</p></Show>
    <section class="card provider-ungrouped"><header class="card-head"><h2>"未分组与旧连接"</h2><span>{move||format!("{} 条",channels.get().iter().filter(|c|c.provider_id.is_none()).count())}</span><button type="button" class="ghost" aria-expanded=move||ungrouped_open.get().to_string() on:click=move |_|ungrouped_open.update(|v|*v=!*v)>{move||if ungrouped_open.get(){"收起"}else{"查看与整理"}}</button></header><super::ModuleDialog open=ungrouped_open title="未分组与旧连接".to_string()><Show when=move||ungrouped_open.get()><div class="card-body"><p class="muted small">"此处保留旧配置，停用项不会作为服务商模型铺开。可在各连接详情中选择服务商；ID、凭据和历史保持独立。"</p><super::channels::Connections ctx=ctx on_changed=reload providers=providers.get()/></div></Show></super::ModuleDialog></section>
    </div>}
}
#[component]
fn ProviderCard(
    ctx: Ctx,
    provider: p::Provider,
    channels: Vec<Channel>,
    all_providers: Vec<p::Provider>,
    reload: Callback<()>,
    model_panels: RwSignal<BTreeSet<i64>>,
    key_panels: RwSignal<BTreeSet<i64>>,
) -> impl IntoView {
    let id = provider.id;
    let open = RwSignal::new(false);
    let editing = RwSignal::new(false);
    let connections_open = RwSignal::new(false);
    let all_providers = StoredValue::new(all_providers);
    let show_compat = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let edited = RwSignal::new(provider.name.clone());
    let enabled = RwSignal::new(provider.enabled);
    let edited_url = RwSignal::new(provider.base_url.clone().unwrap_or_default());
    let edited_mode = RwSignal::new(provider.endpoint_mode.clone());
    let strategy = RwSignal::new(if provider.key_strategy=="concurrent"{"concurrent".to_string()}else{"ordered".into()});
    let switch_429 = RwSignal::new(provider.switch_on_429);
    let switch_400 = RwSignal::new(provider.switch_on_400);
    let provider = StoredValue::new(provider);
    let channels = StoredValue::new(channels);
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let captured_name = edited.get_untracked().trim().to_string();
        let captured_enabled = enabled.get_untracked();
        if captured_name.is_empty() {
            error.set("请输入服务商名称。".into());
            return;
        }
        busy.set(true);
        leptos::task::spawn_local(async move {
            match p::save_config(Some(id),serde_json::json!({"name":captured_name,"enabled":captured_enabled,"base_url":if edited_mode.get_untracked()=="local"{None}else{Some(edited_url.get_untracked().trim().to_string())},"endpoint_mode":edited_mode.get_untracked(),"key_strategy":strategy.get_untracked(),"switch_on_429":switch_429.get_untracked(),"switch_on_400":switch_400.get_untracked()})).await {
                Ok(()) => {editing.set(false);reload.run(());},
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    let remove = move |_| {
        if busy.get_untracked() {
            return;
        }
        let p = provider.get_value();
        let message = if p.connection_ids.is_empty() {
            format!(
                "确定删除空服务商「{}」？仅删除此服务商分组，不删除任何历史记录。",
                p.name
            )
        } else {
            format!(
                "服务商「{}」仍有 {} 个连接，系统会阻止删除。请先在连接设置中移动或删除无依赖连接；有关联历史的连接请停用保留。继续检查删除条件？",
                p.name,
                p.connection_ids.len()
            )
        };
        if !web_sys::window()
            .and_then(|w| w.confirm_with_message(&message).ok())
            .unwrap_or(false)
        {
            return;
        }
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match p::remove(id).await {
                Ok(()) => reload.run(()),
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                    open.set(true);
                    connections_open.set(true);
                }
            }
            busy.set(false);
        });
    };
    view! {<section class="card provider-card"><header class="provider-heading"><div><h2>{provider.get_value().name}</h2><p class="muted small provider-url">{{let p=provider.get_value();if p.endpoint_mode=="local"{format!("本地服务 · {} 个 API 地址",p.base_urls.len())}else{p.base_url.unwrap_or_else(||"请设置 API URL".into())}}}</p><p class="muted small">{{let p=provider.get_value();let (enabled,public,_)=model_counts(&p.models);format!("启用 {enabled} / 已配置 {public} 个模型")}}</p>{let (_,_,hidden)=model_counts(&provider.get_value().models);(hidden>0).then(||view!{<p class="muted small">{format!("另有 {hidden} 个兼容别名")}</p>})}</div><span class=if provider.get_value().enabled{"tag ok"}else{"tag warn"}>{if provider.get_value().enabled{"渠道启用"}else{"渠道停用"}}</span><button type="button" class="ghost" disabled=move||busy.get() aria-expanded=move||open.get().to_string() on:click=move|_|{open.set(true);model_panels.update(|ids|{if open.get_untracked(){ids.insert(id);}else{ids.remove(&id);}});}>"管理模型"</button><button type="button" class="ghost" disabled=move||busy.get() aria-expanded=move||editing.get().to_string() on:click=move |_|{let p=provider.get_value();edited.set(p.name);enabled.set(p.enabled);edited_url.set(p.base_url.unwrap_or_default());edited_mode.set(p.endpoint_mode);strategy.set(p.key_strategy);switch_429.set(p.switch_on_429);switch_400.set(p.switch_on_400);error.set(String::new());editing.set(true);}>"编辑渠道"</button><button type="button" class="ghost danger" disabled=move||busy.get() on:click=remove>"删除渠道"</button></header>
    <super::ModuleDialog open=editing busy=busy title="编辑渠道".to_string()><div class="card-body"><form class="provider-edit" on:submit=save><div class="account-field"><label for=format!("provider-edit-{id}")>"服务商名称"</label><input id=format!("provider-edit-{id}") required=true maxlength="128" prop:value=edited on:input=move|e|edited.set(event_target_value(&e))/></div><label class="account-field"><span>"服务类型"</span><select prop:value=edited_mode on:change=move|e|edited_mode.set(event_target_value(&e))><option value="single">"在线服务：统一 API 地址"</option><option value="local">"本地服务：多个 API 地址"</option></select></label><Show when=move||edited_mode.get()=="single"><div class="account-field"><label for=format!("provider-url-{id}")>"API URL"</label><input id=format!("provider-url-{id}") type="url" required prop:value=edited_url on:input=move|e|edited_url.set(event_target_value(&e))/></div></Show><label class="account-field"><span>"上游Key使用策略"</span><select prop:value=strategy on:change=move|e|strategy.set(event_target_value(&e))><option value="ordered">"逐个切换（按优先顺序）"</option><option value="concurrent">"并发分配（用户固定 Key）"</option></select></label><div class="account-actions provider-switch-rules"><label class="account-check"><input type="checkbox" prop:checked=switch_429 on:change=move|e|switch_429.set(event_target_checked(&e))/>"429时切换Key"</label><label class="account-check"><input type="checkbox" prop:checked=switch_400 on:change=move|e|switch_400.set(event_target_checked(&e))/>"400时切换Key（按需开启）"</label></div><div class="account-actions provider-edit-actions"><label class="pick"><input type="checkbox" prop:checked=enabled on:change=move|e|enabled.set(event_target_checked(&e))/>"启用此服务商"</label><button type="submit" class="primary" disabled=move||busy.get()>"保存渠道"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|editing.set(false)>"取消"</button></div></form><p class="muted small">"停用整个服务商会停止其全部模型的路由，保留连接、密钥、授权和历史。"</p><Show when=move||!error.get().is_empty()><p class="banner bad" role="alert">{move||error.get()}</p></Show></div></super::ModuleDialog>
    <Show when=move||!error.get().is_empty()><div class="card-body"><p class="banner bad" role="alert">{move||error.get()}</p></div></Show>
    <super::provider_keys::ProviderKeys ctx=ctx provider_id=id on_changed=reload local={provider.get_value().endpoint_mode=="local"} initially_open=key_panels.get_untracked().contains(&id) on_open=Callback::new(move |opened|key_panels.update(|ids|{if opened{ids.insert(id);}else{ids.remove(&id);}}))/>
    <super::ModuleDialog open=open title=format!("{} · 模型管理",provider.get_value().name)><Show when=move||open.get()><div class="card-body"><Show when=move||!error.get().is_empty()><p class="banner bad" role="alert">{move||error.get()}</p></Show><p class="muted small">"在这里选择此供应方提供哪些模型。停用后暂停通过此供应方调用该模型，配置和历史保留；模型页可设置全局启停。"</p>
    <label class="pick"><input type="checkbox" prop:checked=show_compat on:change=move|e|show_compat.set(event_target_checked(&e))/>{format!("显示兼容隐藏模型（{}）",provider.get_value().models.iter().filter(|m|!m.visible).count())}</label><Show when=move||!show_compat.get()&&!provider.get_value().models.is_empty()&&provider.get_value().models.iter().all(|m|!m.visible)><p class="muted small">"没有公开展示的模型。可以显示兼容隐藏项，或在上游 Key 的模型选择中添加模型。"</p></Show>
    {if provider.get_value().models.is_empty(){view!{<p class="account-empty">"尚未配置模型，请添加上游Key并配置可用模型。"</p>}.into_any()}else{view!{<div class="table-wrap"><table class="provider-model-table"><thead><tr><th>"上游模型名"</th><th>"本地映射名"</th><th>"原生协议"</th><th>"接入配置"</th><th>"此渠道内模型"</th><th>"详情"</th></tr></thead><tbody>{move||provider.get_value().models.into_iter().filter(|m|m.visible||show_compat.get()).map(|m|{let connections=model_connections(&channels.get_value(),id,&m.name);let upstream:Vec<_>=connections.iter().flat_map(|c|c.models.iter().filter(|p|p.model==m.name).map(|p|p.upstream.clone().unwrap_or_else(||p.model.clone()))).collect::<BTreeSet<_>>().into_iter().collect();let protocols=protocol_names(&connections);let on=all_enabled(&m);let mixed=!m.enabled_connection_ids.is_empty()&&!on;let target=m.name.clone();let target_for_details=m.name.clone();let detail_open=RwSignal::new(false);let scope=m.declared_connection_ids.clone();view!{<tr><td data-label="上游模型名">{upstream.join(" / ")}</td><td data-label="本地映射名"><code>{m.name}</code>{(!m.global_enabled).then(||view!{<span class="tag warn">"全局已停用"</span>})}</td><td data-label="原生协议">{protocols}</td><td data-label="接入配置">{format!("可用 {} / 已配置 {}",m.routable_connection_ids.len(),scope.len())}</td><td data-label="此渠道内模型"><div class="model-state-actions"><span class=if on{"tag ok"}else{"tag warn"}>{if on{"已启用"}else if mixed{"部分启用"}else{"已停用"}}</span><button type="button" class="ghost" disabled=move||busy.get() aria-label=format!("{}此渠道内模型 {} 的全部 {} 个连接",if on{"停用"}else{"启用"},target,scope.len()) on:click=move|_|{if busy.get_untracked(){return;}busy.set(true);let n=target.clone();let ids=scope.clone();leptos::task::spawn_local(async move{match p::toggle(id,&n,!on,&ids).await{Ok(())=>reload.run(()),Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});}>{if on{"停用"}else if mixed{"全部启用"}else{"启用"}}</button></div></td><td data-label="详情"><button type="button" class="ghost" on:click=move |_|detail_open.set(true)>"接入与推理设置"</button><super::ModuleDialog open=detail_open title=format!("{} · 接入与推理设置",target_for_details)><Show when=move||detail_open.get()><ModelAccessDetails ctx=ctx provider_id=id connections=connections.clone() model=target_for_details.clone()/></Show></super::ModuleDialog></td></tr>}}).collect_view()}</tbody></table></div>}.into_any()}}
    <button type="button" class="ghost" on:click=move |_|connections_open.set(true)>"高级诊断与旧配置"</button>
    <super::ModuleDialog open=connections_open title="高级诊断与旧配置".to_string()><p class="muted small">"仅用于兼容旧协议与能力配置。上游 Key 和地址请在渠道中管理；旧来源标识保留用于授权和账单追溯。"</p><Show when=move||connections_open.get()><super::channels::Connections ctx=ctx provider_id=id on_changed=reload providers=all_providers.get_value()/></Show></super::ModuleDialog>
    </div></Show></super::ModuleDialog></section>}
}
/// The main UI identifies model access by the operator's credential label, never storage IDs.
#[component]
fn ModelAccessDetails(ctx: Ctx, provider_id: i64, connections: Vec<Channel>, model: String) -> impl IntoView {
    let credentials = RwSignal::new(Vec::<serde_json::Value>::new());
    let error = RwSignal::new(String::new());
    leptos::task::spawn_local(async move {
        match crate::admin_api::request("GET", &format!("/api/providers/{provider_id}/credentials"), None).await {
            Ok(v) => credentials.set(crate::admin_api::rows(&v, "credentials")),
            Err(e) => {ctx.note_err(&e);error.set(e.message);}
        }
    });
    let connections=StoredValue::new(connections);
    let model=StoredValue::new(model);
    view! {<div class="model-access-list"><Show when=move||!error.get().is_empty()><p class="account-error" role="alert">{move||error.get()}</p></Show>{move||connections.get_value().into_iter().map(|c|{
        let credential=credentials.get().into_iter().enumerate().find(|(_,v)|v["source_ids"].as_array().is_some_and(|a|a.iter().any(|v|v.as_i64()==Some(c.id))));
        let title=credential.as_ref().map(|(position,row)|p::credential_label(row,*position)).unwrap_or_else(||"模型接入配置".into());
        let credential_enabled=credential.as_ref().map(|(_,v)|v["enabled"]==true);
        let enabled=c.enabled&&credential_enabled==Some(true)&&c.models.iter().any(|m|m.model==model.get_value()&&m.enabled);
        view! {<section class="model-access-item"><div class="model-state-actions"><strong>{title.clone()}</strong><span class=if enabled{"tag ok"}else{"tag warn"}>{if credential_enabled.is_none(){"凭据状态待确认"}else if enabled{"已启用"}else{"已停用"}}</span></div><p class="provider-url muted small">{c.base_url}</p><super::model_reasoning::ReasoningConfig ctx=ctx channel=Some(c.id) model=model.get_value() label=format!("{title} · 推理设置")/></section>}
    }).collect_view()}</div>}
}
#[component]
pub(crate) fn Assignment(
    ctx: Ctx,
    connection: Channel,
    providers: Vec<p::Provider>,
    reload: Callback<()>,
) -> impl IntoView {
    let selected = RwSignal::new(
        connection
            .provider_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
    );
    let busy = RwSignal::new(false);
    let err = RwSignal::new(String::new());
    let id = connection.id;
    let assigned = connection.provider_id.is_some();
    let fixed_assignment = assigned && providers.is_empty();
    let assignment_label = connection
        .provider_name
        .clone()
        .unwrap_or_else(|| format!("已归属服务商 #{}", connection.provider_id.unwrap_or(0)));
    let submit = move |_| {
        busy.set(true);
        let target = if fixed_assignment {
            None
        } else {
            selected.get_untracked().parse::<i64>().ok()
        };
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
    view! {<div class="provider-assignment"><strong>{format!("连接 #{} · {}",id,connection.name)}</strong>{if fixed_assignment{view!{<span class="muted small">{format!("当前归属：{assignment_label}")}</span>}.into_any()}else{view!{<label class="sr-only" for=format!("assign-{id}")>"服务商归属"</label><select id=format!("assign-{id}") prop:value=selected on:change=move|e|selected.set(event_target_value(&e))><option value="">"未分组"</option>{providers.into_iter().map(|p|view!{<option value=p.id.to_string()>{p.name}</option>}).collect_view()}</select>}.into_any()}}<button type="button" class="ghost" disabled=move||busy.get() on:click=submit>{if fixed_assignment{"移至未分组"}else{"保存归属"}}</button><Show when=move||!err.get().is_empty()><span class="bad-text" role="alert">{move||err.get()}</span></Show></div>}
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
    fn main_counts_match_visible_table_and_separate_compatibility_aliases() {
        let model = |visible, global_enabled, routable| p::ProviderModel {
            name: "no-suffix-inference".into(),
            visible,
            global_enabled,
            declared_connection_ids: vec![1],
            enabled_connection_ids: vec![1],
            routable_connection_ids: if routable { vec![1] } else { vec![] },
        };
        let rows = vec![
            model(true, true, true),
            model(true, false, true),
            model(true, true, false),
            model(false, true, true),
            model(false, true, true),
            model(false, true, true),
        ];
        assert_eq!(model_counts(&rows), (1, 3, 3));
        assert_eq!(model_counts(&[]), (0, 0, 0));
    }
    #[test]
    fn partial_enable_is_not_all_enabled() {
        let mut m = p::ProviderModel {
            name: "m".into(),
            global_enabled: true,
            visible: true,
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
