use super::{PageHead, policy_editor};
use crate::{admin_api as api, Ctx};
use leptos::prelude::*;
use serde_json::{json, Value};
use wasm_bindgen::JsCast;

// Price attribution uses source IDs, never provider IDs. Keep IDs internal.
fn price_source_label(source: &Value, key: &str) -> String {
    format!("{} · {} · {}",source["provider_name"].as_str().unwrap_or("未分组上游"),key,source["base_url"].as_str().unwrap_or(""))
}
fn price_sources(ctx: Ctx) -> RwSignal<Vec<(i64,String)>> {
    let options=RwSignal::new(Vec::new());
    leptos::task::spawn_local(async move {
        let result=async {
            let channels=api::request("GET","/api/channels",None).await?;
            let mut labels=std::collections::BTreeMap::new();
            let rows=api::rows(&channels,"channels");
            let providers:std::collections::BTreeSet<_>=rows.iter().filter_map(|c|c["provider_id"].as_i64()).collect();
            for id in providers {
                let keys=api::request("GET",&format!("/api/providers/{id}/credentials"),None).await?;
                for (position,key) in api::rows(&keys,"credentials").iter().enumerate() {
                    if let Some(ids)=key["source_ids"].as_array(){for source in ids.iter().filter_map(Value::as_i64){labels.insert(source,crate::provider_api::credential_label(key,position));}}
                }
            }
            Ok::<_,crate::api::ApiErr>(rows.into_iter().filter_map(|c|c["id"].as_i64().map(|id|(id,price_source_label(&c,labels.get(&id).map(String::as_str).unwrap_or("未命名上游 Key"))))).collect())
        }.await;
        match result{Ok(v)=>options.set(v),Err(e)=>ctx.note_err(&e)}
    });
    options
}
#[component]
fn PriceSource(id: &'static str, value: RwSignal<String>, options: RwSignal<Vec<(i64,String)>>) -> impl IntoView {
    view!{<label for=id>"供应方 / 上游 Key / API 地址"</label><select id=id required prop:value=value on:change=move|e|value.set(event_target_value(&e))><option value="">"请选择要定价的上游接入"</option>{move||options.get().into_iter().map(|(id,label)|view!{<option value=id.to_string()>{label}</option>}).collect_view()}</select>}
}
fn selected_price_source(value:&str,options:&[(i64,String)]) -> bool {value.parse::<i64>().ok().is_some_and(|id|options.iter().any(|(source,_)|*source==id))}

// Native dialogs keep editing focused and provide Escape, focus trapping and
// return to the button that opened them without custom keyboard handlers.
fn billing_dialog(id: &str, open: bool) {
    if let Some(dialog) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id)) {
        let method = if open { "showModal" } else { "close" };
        if let Ok(function) = js_sys::Reflect::get(dialog.as_ref(), &method.into()).and_then(|v| v.dyn_into::<js_sys::Function>()) {
            let _ = function.call0(dialog.as_ref());
        }
    }
}

#[component]
fn BillingDialog(id: &'static str, title: &'static str, #[prop(optional)] busy: Option<RwSignal<bool>>, children: Children) -> impl IntoView {
    let title_id = format!("{id}-title");
    view! {<dialog id=id class="module-dialog console-content" aria-labelledby=title_id.clone() on:cancel=move |ev: web_sys::Event|{if busy.is_some_and(|busy|busy.get_untracked()){ev.prevent_default();}}>
        <header class="module-dialog-header"><h2 id=title_id.clone()>{title}</h2><button type="button" class="ghost" disabled=move||busy.is_some_and(|busy|busy.get()) on:click=move |_|{if !busy.is_some_and(|busy|busy.get_untracked()){billing_dialog(id,false);}}>"关闭"</button></header>
        <div class="module-dialog-body">{children()}</div>
    </dialog>}
}

fn money(value: &Value) -> String {
    value.as_i64().map(|n|format!("${}",policy_editor::money_input(n))).unwrap_or_else(||"未知".into())
}
fn named(row:&Value, field:&str, fallback:&str)->String {
    row[field].as_str().filter(|s|!s.trim().is_empty()).unwrap_or(fallback).to_string()
}
fn state_label(row:&Value)->String {named(row,"status_label","历史对象")}
fn price_is_current(row:&Value,rows:&[Value])->bool {
    !rows.iter().any(|other|other["channel_id"]==row["channel_id"]&&other["model"]==row["model"]&&other["id"].as_i64()>row["id"].as_i64())
}
fn price_source_name(row:&Value,objects:&Value)->String {
    api::rows(objects,"channels").iter().find(|o|o["id"]==row["channel_id"]).map(|o|api::text(o,"name")).unwrap_or_else(||"历史接入（名称未留存）".into())
}
fn service_price(row:&Value)->bool {row["model"].as_str().is_some_and(|name|name.starts_with("passthrough:"))}
fn price_model_name(row:&Value)->String {if service_price(row){"服务调用".into()}else{api::text(row,"model")}}
fn price_effective(row:&Value,rows:&[Value],objects:&Value)->bool {
    price_is_current(row,rows)&&api::rows(objects,"channels").iter().any(|o|o["id"]==row["channel_id"]&&o["state"]=="active")
}
fn ledger_identity(row: &Value) -> (String, String, String) {
    let source=named(row,"channel_name","历史上游接入");
    if row["auth_snapshot"]["kind"] == "admin_diagnostic" {
        let name = row["auth_snapshot"]["probe_name"].as_str().unwrap_or("自定义探针");
        let trigger = if row["auth_snapshot"]["trigger"] == "scheduled" { "定时执行" } else { "手动执行" };
        (source,format!("系统诊断 · {name} · {trigger}"),"不扣用户费用".into())
    } else {
        (source,format!("{} · {}",named(row,"user_name","历史用户"),named(row,"key_name","历史 Key")),money(&row["debit_micro"]))
    }
}
#[component]
fn ObjectSelect(id:&'static str, label:&'static str, group:&'static str, value:RwSignal<String>, objects:RwSignal<Value>, #[prop(default=false)] active_only:bool) -> impl IntoView {
    view!{<div class="billing-field"><label for=id>{label}</label><select id=id prop:value=value on:change=move|e|value.set(event_target_value(&e))><option value="">{if active_only{"请选择"}else{"全部"}}</option>{move||api::rows(&objects.get(),group).into_iter().filter(|o|!active_only||o["state"]=="active").map(|o|{let label=if group=="keys"{format!("{} · {}",api::text(&o,"user_name"),api::text(&o,"name"))}else{api::text(&o,"name")};let label=if o["state"]=="active"{label}else{format!("{label}（{}）",state_label(&o))};view!{<option value=api::text(&o,"id")>{label}</option>}}).collect_view()}</select></div>}
}
#[component]
pub fn Billing(ctx: Ctx) -> impl IntoView {
    let section=RwSignal::new("budgets");
    let objects=RwSignal::new(json!({}));let object_error=RwSignal::new(String::new());
    leptos::task::spawn_local(async move{match api::request("GET","/api/billing/objects",None).await{Ok(v)=>objects.set(v),Err(e)=>{ctx.note_err(&e);object_error.set(e.message);}}});
    view!{<div class="page admin-page billing-page"><PageHead title="用量与账务" desc="按管理目标切换：控制额度、维护价格、查询调用费用。"/>
    <nav class="billing-nav" aria-label="账务管理维度">{[("budgets","额度管理","用户与 Key 的使用额度"),("prices","价格配置","模型计费与历史版本"),("usage","用量账本","调用明细与费用追踪")].into_iter().map(|(key,title,desc)|view!{<button type="button" class:active=move||section.get()==key aria-pressed=move||(section.get()==key).to_string() on:click=move |_|section.set(key)><strong>{title}</strong><span>{desc}</span></button>}).collect_view()}</nav>
    <p role="alert">{move||object_error.get()}</p>
    {move||match section.get(){"budgets"=>view!{<Budgets ctx=ctx objects=objects/>}.into_any(),"prices"=>view!{<PriceWorkspace ctx=ctx objects=objects/>}.into_any(),_=>view!{<Usage ctx=ctx objects=objects/>}.into_any()}}
    </div>}
}
#[component]
fn PriceWorkspace(ctx:Ctx,objects:RwSignal<Value>)->impl IntoView {
    let mode=RwSignal::new("token");
    view!{<div class="billing-subnav" role="group" aria-label="计费方式"><button type="button" class:active=move||mode.get()=="token" aria-pressed=move||(mode.get()=="token").to_string() on:click=move |_|mode.set("token")>"按 Token 计费"</button><button type="button" class:active=move||mode.get()=="request" aria-pressed=move||(mode.get()=="request").to_string() on:click=move |_|mode.set("request")>"按请求计费"</button></div>{move||if mode.get()=="token"{view!{<Pricing ctx=ctx objects=objects/>}.into_any()}else{view!{<RequestPricing ctx=ctx objects=objects/>}.into_any()}}}
}
#[component]
fn Budgets(ctx: Ctx,objects:RwSignal<Value>) -> impl IntoView {
    let scope=RwSignal::new("user".to_string());let id=RwSignal::new(String::new());let limit=RwSignal::new(String::new());let frozen=RwSignal::new(false);
    let kind=RwSignal::new("user".to_string());let state=RwSignal::new("active".to_string());
    let rows=RwSignal::new(Vec::<Value>::new());let busy=RwSignal::new(false);let error=RwSignal::new(String::new());let message=RwSignal::new(String::new());let editing=RwSignal::new(false);
    let load=move |_|{busy.set(true);error.set(String::new());message.set(String::new());let q=state.get_untracked();leptos::task::spawn_local(async move{match api::request("GET",&format!("/api/budgets?state={q}"),None).await{Ok(v)=>{rows.set(api::rows(&v,"budgets"));message.set("预算已刷新".into());},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});};
    Effect::new(move |_|{state.get();load(());});
    let save=move|ev:leptos::ev::SubmitEvent|{ev.prevent_default();if busy.get_untracked(){return;}let object=match api::integer(&id.get_untracked(),"预算对象",true){Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        let amount=match policy_editor::optional_limit(&limit.get_untracked()){Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        let target=scope.get_untracked();let group=if target=="user"{"users"}else{"keys"};
        if !api::rows(&objects.get_untracked(),group).iter().any(|o|o["id"]==object&&o["state"]=="active"){error.set("请选择当前有效的用户或 Key。".into());return;}
        busy.set(true);error.set(String::new());message.set(String::new());let hold=frozen.get_untracked();
        leptos::task::spawn_local(async move{match api::request("PUT",&format!("/api/budgets?scope={target}&scope_id={object}"),Some(json!({"limit_micro":amount,"frozen":hold}))).await{Ok(_)=>{billing_dialog("budget-dialog",false);load(());message.set("预算已保存，累计使用与在途预占均已保留。".into());},Err(e)=>{ctx.note_err(&e);error.set(e.message);busy.set(false);}}});};
    view!{<section class="card"><header class="card-head"><div><h2>"额度管理"</h2><p class="muted small">"用户额度约束名下所有 Key；单个 Key 可再设置独立额度。"</p></div><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{editing.set(false);scope.set(kind.get_untracked());id.set(String::new());limit.set(String::new());frozen.set(false);error.set(String::new());billing_dialog("budget-dialog",true);}>"设置额度"</button></header><div class="card-body">
    <div class="billing-toolbar"><div class="billing-subnav" role="group" aria-label="预算维度"><button type="button" class:active=move||kind.get()=="user" aria-pressed=move||(kind.get()=="user").to_string() on:click=move |_|kind.set("user".into())>"用户额度"</button><button type="button" class:active=move||kind.get()=="key" aria-pressed=move||(kind.get()=="key").to_string() on:click=move |_|kind.set("key".into())>"Key 额度"</button></div><div class="billing-field"><label for="budget-state">"对象状态"</label><select id="budget-state" prop:value=state disabled=move||busy.get() on:change=move|e|state.set(event_target_value(&e))><option value="active">"当前有效"</option><option value="historical">"历史 / 已停用"</option><option value="all">"全部"</option></select></div><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load(())>"刷新预算"</button></div>
    <p class="muted small">"金额单位 USD。无限额表示未单独限额，仍受其他预算约束；冻结会阻止后续调用。历史对象仅供核对。"</p>
    <div class="table-wrap"><table class="billing-budgets"><thead><tr><th>"对象名称"</th><th>"所属用户"</th><th>"总限额"</th><th>"已使用"</th><th>"在途预占"</th><th>"状态"</th><th>"管理"</th></tr></thead><tbody>{move||rows.get().into_iter().filter(|r|r["scope"]==kind.get()).map(|r|{
        let active=r["object_state"]=="active";let name=named(&r,"object_name","历史对象");let owner=named(&r,"user_name","—");let cap=if r["limit_micro"].is_null(){"无限额".into()}else{money(&r["limit_micro"])};let spent=money(&r["spent_micro"]);let reserved=money(&r["reserved_micro"]);let status=if r["frozen"]==true{format!("{} · 额度冻结",state_label(&r))}else{state_label(&r)};
        view!{<tr><td data-label="对象名称">{name}</td><td data-label="所属用户">{owner}</td><td data-label="总限额">{cap}</td><td data-label="已使用">{spent}</td><td data-label="在途预占">{reserved}</td><td data-label="状态">{status}</td><td data-label="管理">{if active{view!{<button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{editing.set(true);scope.set(api::text(&r,"scope"));id.set(api::text(&r,"scope_id"));limit.set(r["limit_micro"].as_i64().map(policy_editor::money_input).unwrap_or_default());frozen.set(r["frozen"]==true);error.set(String::new());billing_dialog("budget-dialog",true);}>"管理额度"</button>}.into_any()}else{view!{<span class="muted small">"历史只读"</span>}.into_any()}}</td></tr>}
    }).collect_view()}</tbody></table></div>
    <p role="status" aria-live="polite">{move||if busy.get(){"正在加载预算…".to_string()}else if rows.get().iter().all(|r|r["scope"]!=kind.get()){"当前分类暂无预算，可为有效对象设置额度。".into()}else{message.get()}}</p><p role="alert">{move||error.get()}</p>
    <button type="button" class="ghost" on:click=move |_|ctx.go(crate::Page::Users)>"管理用户与 Key 身份"</button>
    <BillingDialog id="budget-dialog" title="管理额度" busy=busy><form on:submit=save><fieldset disabled=move||busy.get()>
    <div class="billing-field"><label for="budget-scope">"预算对象类型"</label><select id="budget-scope" disabled=move||editing.get() prop:value=scope on:change=move|e|{scope.set(event_target_value(&e));id.set(String::new());}><option value="user">"用户"</option><option value="key">"API Key"</option></select></div>
    {move||if editing.get(){let group=if scope.get()=="user"{"users"}else{"keys"};let label=api::rows(&objects.get(),group).iter().find(|o|api::text(o,"id")==id.get()).map(|o|api::text(o,"name")).unwrap_or_else(||"对象已失效，请关闭并刷新".into());view!{<p class="billing-selected-object">{label}</p>}.into_any()}else if scope.get()=="user"{view!{<ObjectSelect id="budget-object-user" label="用户" group="users" value=id objects=objects active_only=true/>}.into_any()}else{view!{<ObjectSelect id="budget-object-key" label="API Key（所属用户 · 名称）" group="keys" value=id objects=objects active_only=true/>}.into_any()}}
    <div class="billing-field"><label for="budget-limit">"总限额（USD）"</label><input id="budget-limit" type="text" inputmode="decimal" prop:value=limit aria-describedby="budget-help budget-error" placeholder="留空表示无限额" on:input=move|e|limit.set(event_target_value(&e))/><small id="budget-help" class="muted">"总限额包含已使用金额，最多6位小数。"</small></div>
    <button type="button" class="ghost" on:click=move |_|limit.set(String::new())>"解除限额"</button>
    <label class="account-check"><input type="checkbox" prop:checked=frozen on:change=move|e|frozen.set(event_target_checked(&e))/><span>"冻结后续调用（取消勾选恢复额度使用）"</span></label>
    <p id="budget-error" role="alert">{move||error.get()}</p><p class="muted small">"保存只调整额度策略，保留已使用金额、在途预占和历史账目。"</p><div class="module-dialog-actions"><button type="button" class="ghost" on:click=move |_|billing_dialog("budget-dialog",false)>"取消"</button><button type="submit" class="primary">"保存额度"</button></div></fieldset></form></BillingDialog>
    </div></section>}
}
#[component]
fn Pricing(ctx: Ctx,objects:RwSignal<Value>) -> impl IntoView {
    let sources = price_sources(ctx);
    let versions=RwSignal::new("current".to_string());
    let rows = RwSignal::new(Vec::<Value>::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let channel = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let input = RwSignal::new("0".to_string());
    let output = RwSignal::new("0".to_string());
    let max_in = RwSignal::new(String::new());
    let max_out = RwSignal::new(String::new());
    let load = move |_| {
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("GET", "/api/pricing", None).await {
                Ok(v) => rows.set(api::rows(&v, "prices")),
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    load(());
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        if !selected_price_source(&channel.get_untracked(),&sources.get_untracked()) {error.set("请选择有效的上游接入；供应方编号不能用于定价。".into());return;}
        let values = [
            (&channel, "上游接入", true),
            (&input, "输入价格", false),
            (&output, "输出价格", false),
            (&max_in, "输入 token 上限", false),
            (&max_out, "输出 token 上限", false),
        ]
        .into_iter()
        .map(|(s, l, p)| if l=="输入价格"||l=="输出价格" {policy_editor::optional_limit(&s.get_untracked()).and_then(|v|v.ok_or_else(||format!("请填写{l}")))}else{api::integer(&s.get_untracked(), l, p)})
        .collect::<Result<Vec<_>, _>>();
        let n = match values {
            Ok(v) => v,
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let m = model.get_untracked().trim().to_string();
        if m.is_empty() {
            error.set("请填写逻辑模型名".into());
            return;
        }
        let body = json!({"channel_id":n[0],"model":m,"input_micro_per_million":n[1],"output_micro_per_million":n[2],"max_input_tokens":n[3],"max_output_tokens":n[4]});
        busy.set(true);
        error.set(String::new());
        message.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("POST", "/api/pricing", Some(body)).await {
                Ok(v) => {
                    rows.update(|all| all.insert(0, v["price"].clone()));
                    message.set("新价格版本已创建；旧账本保留原价格快照。".into());
                    billing_dialog("price-dialog", false);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<section class="card"><header class="card-head"><h2>"价格版本"</h2><div class="account-actions"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{channel.set(String::new());model.set(String::new());input.set("0".into());output.set("0".into());max_in.set(String::new());max_out.set(String::new());error.set(String::new());message.set(String::new());billing_dialog("price-dialog",true);}>"配置价格"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load(())>"刷新价格"</button></div></header><div class="card-body">
    <div class="billing-toolbar"><div class="billing-field"><label for="token-versions">"价格范围"</label><select id="token-versions" prop:value=versions on:change=move|e|versions.set(event_target_value(&e))><option value="current">"当前生效版本"</option><option value="history">"历史版本"</option><option value="all">"全部版本"</option></select></div><p class="muted small">"配置或调整价格会创建新版本；历史版本只读，保留账目快照。"</p></div><p class="muted small">"价格单位：USD / 百万 Token，最多6位小数。按所选供应方的上游 Key/API 接入 + 本地映射模型名匹配，最新版本生效；缺少价格表示未知成本，不等于免费。0 / 0 价格表示明确零 token 费用。上下限用于请求预占与安全封顶，不代表上游实测上下文。最多展示最近 1000 个版本。"</p>
    <div class="table-wrap"><table class="billing-price-table"><thead><tr><th>"版本 / 上游接入 / 模型"</th><th>"输入 / 输出价格"</th><th>"输入 / 输出上限"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().filter(|r|{let current=price_effective(r,&rows.get(),&objects.get());versions.get()=="all"||(versions.get()=="current"&&current)||(versions.get()=="history"&&!current)}).map(|r|{let source_name=price_source_name(&r,&objects.get());let model_label=price_model_name(&r);let version_label=format!("{} · 版本 {}",if price_effective(&r,&rows.get(),&objects.get()){"当前生效"}else{"历史 / 接入失效"},api::text(&r,"id"));let action_label=if price_effective(&r,&rows.get(),&objects.get()){"调整价格"}else{"沿用此版本"};let price=format!("输入 {} / 输出 {}",money(&r["input_micro_per_million"]),money(&r["output_micro_per_million"]));let caps=format!("{} / {}",api::text(&r,"max_input_tokens"),api::text(&r,"max_output_tokens"));view!{<tr><td data-label="版本 / 来源"><strong class="billing-price-model">{model_label}</strong><small class="billing-price-source">{source_name}</small><span class="tag billing-price-version">{version_label}</span></td><td data-label="价格">{price}</td><td data-label="token 上限">{caps}</td><td data-label="操作"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{channel.set(api::text(&r,"channel_id"));model.set(api::text(&r,"model"));input.set(r["input_micro_per_million"].as_i64().map(policy_editor::money_input).unwrap_or_default());output.set(r["output_micro_per_million"].as_i64().map(policy_editor::money_input).unwrap_or_default());max_in.set(api::text(&r,"max_input_tokens"));max_out.set(api::text(&r,"max_output_tokens"));error.set(String::new());message.set("已填入旧值，保存会创建新版本。".into());billing_dialog("price-dialog",true);}>{action_label}</button></td></tr>}}).collect_view()}</tbody></table></div>
    <p role="status">{move||if busy.get(){"正在请求价格…"}else if !rows.get().iter().any(|r|{let current=price_effective(r,&rows.get(),&objects.get());versions.get()=="all"||(versions.get()=="current"&&current)||(versions.get()=="history"&&!current)}){"当前范围暂无价格，可切换范围或新建价格。"}else{""}}</p>
    <p role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><BillingDialog id="price-dialog" title="配置价格（创建新版本）" busy=busy><form on:submit=save ><fieldset disabled=move||busy.get()><div class="form-grid">
    <PriceSource id="price-channel" value=channel options=sources/>
    <label for="price-model">"逻辑模型名"</label><input id="price-model" required type="text" prop:value=model aria-describedby="price-error" on:input=move|e|model.set(event_target_value(&e))/>
    <label for="price-in">"输入价格（USD / 百万 Token）"</label><input id="price-in" required type="text" inputmode="decimal" prop:value=input aria-describedby="price-error" on:input=move|e|input.set(event_target_value(&e))/>
    <label for="price-out">"输出价格（USD / 百万 Token）"</label><input id="price-out" required type="text" inputmode="decimal" prop:value=output aria-describedby="price-error" on:input=move|e|output.set(event_target_value(&e))/>
    <label for="price-max-in">"输入 token 预占上限"</label><input id="price-max-in" required type="text" inputmode="numeric" prop:value=max_in aria-describedby="price-error" on:input=move|e|max_in.set(event_target_value(&e))/>
    <label for="price-max-out">"输出 token 总上限（含 n）"</label><input id="price-max-out" required type="text" inputmode="numeric" prop:value=max_out aria-describedby="price-error" on:input=move|e|max_out.set(event_target_value(&e))/>
    </div><p id="price-error" role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><div class="module-dialog-actions"><button type="button" class="ghost" on:click=move |_|billing_dialog("price-dialog",false)>"取消"</button><button class="primary" type="submit">"创建价格版本"</button></div></fieldset></form></BillingDialog></div></section>}
}
#[component]
fn RequestPricing(ctx: Ctx,objects:RwSignal<Value>) -> impl IntoView {
    let sources = price_sources(ctx);
    let versions=RwSignal::new("current".to_string());
    let rows = RwSignal::new(Vec::<Value>::new());
    let channel = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let price = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let load = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("GET", "/api/request-pricing", None).await {
                Ok(v) => rows.set(api::rows(&v, "request_prices")),
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    load(());
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        if !selected_price_source(&channel.get_untracked(),&sources.get_untracked()) {error.set("请选择有效的上游接入；供应方编号不能用于定价。".into());return;}
        let values = api::integer(&channel.get_untracked(), "上游接入", true)
            .and_then(|c| policy_editor::optional_limit(&price.get_untracked()).and_then(|v|v.ok_or_else(||"请填写按次价格".into())).map(|p| (c, p)));
        let (c, p) = match values {
            Ok(v) => v,
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let m = model.get_untracked().trim().to_string();
        if m.is_empty() {
            error.set("请填写逻辑模型名".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        message.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request(
                "POST",
                "/api/request-pricing",
                Some(json!({"channel_id":c,"model":m,"micro_per_call":p})),
            )
            .await
            {
                Ok(v) => {
                    rows.update(|all| all.insert(0, v["request_price"].clone()));
                    message.set("新按次价格版本已创建；旧账本保留原快照。".into());
                    billing_dialog("request-price-dialog", false);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<section class="card"><header class="card-head"><h2>"按请求收费价格版本"</h2><div class="account-actions"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{channel.set(String::new());model.set(String::new());price.set(String::new());error.set(String::new());message.set(String::new());billing_dialog("request-price-dialog",true);}>"配置按次价格"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load(())>"刷新按次价格"</button></div></header><div class="card-body">
    <div class="billing-toolbar"><div class="billing-field"><label for="request-versions">"价格范围"</label><select id="request-versions" prop:value=versions on:change=move|e|versions.set(event_target_value(&e))><option value="current">"当前生效版本"</option><option value="history">"历史版本"</option><option value="all">"全部版本"</option></select></div><p class="muted small">"配置或调整价格会创建新版本；历史版本只读，保留账目快照。"</p></div><p class="muted small">"Search / Fetch 按请求计费，单位为 USD / 次，最多6位小数；独立于 token 价格。按所选供应方的上游 Key/API 接入 + 本地映射模型名匹配最新版本，0 表示明确免费。未知用量仍显示未知；预占或估算扣款不表示已确认收费次数。最多展示最近 1000 个版本。"</p>
    <div class="table-wrap"><table class="billing-price-table"><thead><tr><th>"版本 / 上游接入 / 模型"</th><th>"价格 USD / 次"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().filter(|r|{let current=price_effective(r,&rows.get(),&objects.get());versions.get()=="all"||(versions.get()=="current"&&current)||(versions.get()=="history"&&!current)}).map(|r|{
        let source_name=price_source_name(&r,&objects.get());let model_label=price_model_name(&r);let version_label=format!("{} · 版本 {}",if price_effective(&r,&rows.get(),&objects.get()){"当前生效"}else{"历史 / 接入失效"},api::text(&r,"id"));let action_label=if price_effective(&r,&rows.get(),&objects.get()){"调整价格"}else{"沿用此版本"};let amount=money(&r["micro_per_call"]);
        view!{<tr><td data-label="版本 / 来源"><strong class="billing-price-model">{model_label}</strong><small class="billing-price-source">{source_name}</small><span class="tag billing-price-version">{version_label}</span></td><td data-label="USD / 次">{amount}</td><td data-label="操作">{if service_price(&r){view!{<button type="button" class="ghost" on:click=move |_|ctx.go(crate::Page::Services)>"管理服务价格"</button>}.into_any()}else{view!{<button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{channel.set(api::text(&r,"channel_id"));model.set(api::text(&r,"model"));price.set(r["micro_per_call"].as_i64().map(policy_editor::money_input).unwrap_or_default());error.set(String::new());message.set("已填入旧值，保存会创建新版本。".into());billing_dialog("request-price-dialog",true);}>{action_label}</button>}.into_any()}}</td></tr>}
    }).collect_view()}</tbody></table></div>
    <p role="status">{move||if busy.get(){"正在请求按次价格…"}else if !rows.get().iter().any(|r|{let current=price_effective(r,&rows.get(),&objects.get());versions.get()=="all"||(versions.get()=="current"&&current)||(versions.get()=="history"&&!current)}){"当前范围暂无按次价格，可切换范围或新建价格。"}else{""}}</p>
    <p role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><BillingDialog id="request-price-dialog" title="配置按次价格（创建新版本）" busy=busy><form on:submit=save ><fieldset disabled=move||busy.get()><div class="form-grid">
    <PriceSource id="request-price-channel" value=channel options=sources/>
    <label for="request-price-model">"逻辑模型名"</label><input id="request-price-model" required type="text" prop:value=model aria-describedby="request-price-error" on:input=move|e|model.set(event_target_value(&e))/>
    <label for="request-price-micro">"价格（USD / 次）"</label><input id="request-price-micro" required type="text" inputmode="decimal" prop:value=price aria-describedby="request-price-error" on:input=move|e|price.set(event_target_value(&e))/>
    </div><p id="request-price-error" role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><div class="module-dialog-actions"><button type="button" class="ghost" on:click=move |_|billing_dialog("request-price-dialog",false)>"取消"</button><button class="primary" type="submit">"创建按次价格版本"</button></div></fieldset></form></BillingDialog>
    </div></section>}
}
fn usage_status(row:&Value)->String {
    let outcome=match row["outcome"].as_str().unwrap_or(""){"ok"|"success"=>"成功","error"|"failed"|"failure"|"upstream_error"=>"失败","cancelled"=>"已取消","interrupted"|"body_error"=>"响应中断",""=>"",_=>"已结束"};
    let status=match row["status"].as_str().unwrap_or(""){"reserved"=>"待结算","unpriced"=>"未定价","released"|"not_sent"=>"已释放",_ if !row["settled_ms"].is_null()=>"已结算",_=>"待确认"};
    if outcome.is_empty(){status.into()}else{format!("{outcome} · {status}")}
}
fn current_hold(row:&Value)->String {
    if row["status"]=="reserved"&&row["settled_ms"].is_null(){money(&row["reserved_micro"])}else{"$0".into()}
}
fn usage_basis(row:&Value)->String {match row["usage_certainty"].as_str().unwrap_or(""){"reported"=>"上游报告","estimated"=>"估算","unpriced"=>"未定价","not_sent"=>"未发送","pending"=>"待确认",_=>"未知"}.into()}
fn updated_time()->String {let d=js_sys::Date::new_0();format!("{:02}:{:02}:{:02}",d.get_hours(),d.get_minutes(),d.get_seconds())}
#[component]
fn Usage(ctx: Ctx,objects:RwSignal<Value>) -> impl IntoView {
    let snapshot=RwSignal::new(String::new());let user=RwSignal::new(String::new());let key=RwSignal::new(String::new());let channel=RwSignal::new(String::new());let model=RwSignal::new(String::new());let state=RwSignal::new("all".to_string());let count=RwSignal::new("50".to_string());
    let rows=RwSignal::new(Vec::<Value>::new());let cursor=RwSignal::new(None::<i64>);let busy=RwSignal::new(false);let error=RwSignal::new(String::new());let message=RwSignal::new(String::new());let page=RwSignal::new(0usize);let starts=RwSignal::new(vec![None::<i64>]);let active_query=RwSignal::new("limit=50&state=all".to_string());
    let fetch=move|base:String,before:Option<i64>,target:usize|{if busy.get_untracked(){return;}busy.set(true);error.set(String::new());message.set("正在查询…".into());let query=if let Some(id)=before{format!("{base}&before_id={id}")}else{base.clone()};
        leptos::task::spawn_local(async move{match api::request("GET",&format!("/api/usage?{query}"),None).await{Ok(v)=>{let found=api::rows(&v,"entries");let len=found.len();cursor.set(v["next_before_id"].as_i64());rows.set(found);page.set(target);if target==0{starts.set(vec![None]);active_query.set(base);}else{starts.update(|s|{s.truncate(target);s.push(before);});}message.set(format!("查询完成 · 第 {} 页 · {} 条记录 · 更新于 {}",target+1,len,updated_time()));},Err(e)=>{ctx.note_err(&e);error.set(e.message);message.set("查询失败，保留上次结果。".into());}}busy.set(false);});};
    fetch("limit=50&state=all".into(),None,0);
    let search=move|ev:leptos::ev::SubmitEvent|{ev.prevent_default();if busy.get_untracked(){return;}let n=match api::integer(&count.get_untracked(),"每页条数",true){Ok(v)if v<=200=>v,_=>{error.set("每页条数必须在1至200之间".into());return;}};let mut q=format!("limit={n}&state={}",state.get_untracked());for (value,name) in [(user,"user_id"),(key,"key_id"),(channel,"channel_id")] {if !value.get_untracked().is_empty(){q.push_str(&format!("&{name}={}",api::encode(&value.get_untracked())));}}if !model.get_untracked().trim().is_empty(){q.push_str(&format!("&model={}",api::encode(model.get_untracked().trim())));}fetch(q,None,0);};
    view!{<section class="card billing-usage"><header class="card-head"><div><h2>"用量账本"</h2><p class="muted small">"按调用尝试记录费用；重试或回退可产生多笔账目。历史账目完整保留。"</p></div></header><div class="card-body">
    <form class="billing-filter" on:submit=search><fieldset disabled=move||busy.get()><div class="billing-filters">
    <ObjectSelect id="usage-user" label="用户" group="users" value=user objects=objects/>
    <ObjectSelect id="usage-key" label="API Key（所属用户 · 名称）" group="keys" value=key objects=objects/>
    <ObjectSelect id="usage-channel" label="上游接入" group="channels" value=channel objects=objects/>
    <div class="billing-field"><label for="usage-model">"模型（精确匹配）"</label><input id="usage-model" prop:value=model on:input=move|e|model.set(event_target_value(&e))/></div>
    <div class="billing-field"><label for="usage-state">"关联对象状态"</label><select id="usage-state" prop:value=state on:change=move|e|state.set(event_target_value(&e))><option value="all">"全部（含历史）"</option><option value="active">"当前有效"</option><option value="historical">"历史 / 已停用"</option></select></div>
    <div class="billing-field"><label for="usage-limit">"每页条数"</label><select id="usage-limit" prop:value=count on:change=move|e|count.set(event_target_value(&e))><option value="10">"10"</option><option value="50">"50"</option><option value="100">"100"</option><option value="200">"200"</option></select></div>
    </div><div class="billing-filter-actions"><button class="primary" type="submit">{move||if busy.get(){"查询中…"}else{"查询"}}</button><button type="button" class="ghost" on:click=move |_|{user.set(String::new());key.set(String::new());channel.set(String::new());model.set(String::new());state.set("all".into());count.set("50".into());fetch("limit=50&state=all".into(),None,0);}>"重置筛选"</button></div></fieldset></form>
    <p id="usage-error" role="alert">{move||error.get()}</p><p class="billing-query-status" role="status" aria-live="polite">{move||message.get()}</p>
    <details class="billing-help"><summary>"如何理解账目与费用"</summary><p>"扣款是用户额度的实际扣减；成本为上游报告或估算，不能直接当作供应商对账结果；当前占用仅指尚未结算的额度，结算后显示0；初始预占记录保留在详情中。金额单位 USD。未知不等于免费。"</p><p>"系统诊断单独标识，不扣用户费用。历史对象包括停用、删除、撤销或到期的用户、Key、上游接入。"</p>{move||api::billing_page_summary(&rows.get()).into_iter().map(|line|view!{<p class="muted small">{line}</p>}).collect_view()}</details>
    <div class="table-wrap"><table><thead><tr><th>"用户 / Key"</th><th>"上游接入 / 模型"</th><th>"结果"</th><th>"用量"</th><th>"用户扣款"</th><th>"上游成本"</th><th>"当前占用"</th><th>"详情"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|r|{
        let (source,who,debit)=ledger_identity(&r);let model_name=api::text(&r,"model");let state_text=usage_status(&r);let basis=usage_basis(&r);let units=if r["billing_unit"]=="request"{format!("{} 次",api::billed_requests(&r))}else{format!("输入 {} / 输出 {} token",api::text(&r,"input_tokens"),api::text(&r,"output_tokens"))};let cost=money(&r["observed_cost_micro"]);let reserved=current_hold(&r);let historical=r["record_state"]=="historical";let detail=api::pretty(&r);
        view!{<tr><td data-label="用户 / Key"><strong>{who}</strong>{if historical{Some(view!{<small class="billing-history">"关联历史对象"</small>})}else{None}}</td><td data-label="上游接入 / 模型">{source}<small>{model_name}</small></td><td data-label="结果">{state_text}<small>{basis}</small></td><td data-label="用量">{units}</td><td data-label="用户扣款">{debit}</td><td data-label="上游成本">{cost}</td><td data-label="当前占用">{reserved}</td><td data-label="详情"><button type="button" class="ghost" on:click=move |_|{snapshot.set(detail.clone());billing_dialog("ledger-snapshot-dialog",true);}>"查看详情"</button></td></tr>}
    }).collect_view()}</tbody></table></div>
    {move||if !busy.get()&&rows.get().is_empty(){Some(view!{<p class="muted">"当前筛选没有账目，可调整条件或重置筛选。"</p>})}else{None}}
    <nav class="billing-pagination" aria-label="账本分页"><span>{move||format!("第 {} 页 · 本页 {} 条",page.get()+1,rows.get().len())}</span><div class="billing-pagination-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|fetch(active_query.get_untracked(),None,0)>"最新一页"</button><button type="button" class="ghost" disabled=move||busy.get()||page.get()==0 on:click=move |_|{let target=page.get_untracked()-1;fetch(active_query.get_untracked(),starts.get_untracked()[target],target);}>"上一页"</button><button type="button" class="ghost" disabled=move||busy.get()||cursor.get().is_none()||rows.get().is_empty() on:click=move |_|fetch(active_query.get_untracked(),cursor.get_untracked(),page.get_untracked()+1)>"较早一页"</button></div></nav>
    <p class="muted small">"分页沿用已查询条件；修改筛选后点击“查询”，从最新一页重新开始。"</p>
    <BillingDialog id="ledger-snapshot-dialog" title="账目详情"><p class="muted small">"完整原始快照，包括账本编号、请求编号及上游接入编号，用于追溯。"</p><pre class="admin-json">{move||snapshot.get()}</pre></BillingDialog>
    </div></section>}
}

#[cfg(test)] mod price_source_tests {
 use super::*;
 #[test] fn diagnostic_identity_does_not_invent_a_user_or_zero_cost(){
  let row=json!({"id":8,"user_id":0,"key_id":0,"request_id":0,"channel_id":9,"model":"tool", "debit_micro":0,"observed_cost_micro":null,"auth_snapshot":{"kind":"admin_diagnostic","probe_name":"搜索检查","trigger":"scheduled"}});
  let (origin,who,money)=ledger_identity(&row);assert!(who.contains("系统诊断 · 搜索检查 · 定时执行"));assert!(!who.contains("用户 0"));assert!(!origin.contains("请求 0"));assert_eq!(money,"不扣用户费用");assert!(money.contains("不扣用户费用"));
 }
 #[test] fn settled_ledger_never_looks_like_an_outstanding_hold(){
  let settled=json!({"status":"success","outcome":"success","settled_ms":123,"reserved_micro":736,"debit_micro":8});
  assert_eq!(current_hold(&settled),"$0");assert_eq!(usage_status(&settled),"成功 · 已结算");
  assert_eq!(current_hold(&json!({"status":"reserved","settled_ms":null,"reserved_micro":736})),"$0.000736");
 }
 #[test] fn passthrough_prices_use_service_catalog_without_llm_source_options(){
  let price=json!({"channel_id":32,"model":"passthrough:12"});let catalog=json!({"channels":[{"id":32,"name":"GPU03 MINERU","state":"active"}]});
  assert_eq!(price_source_name(&price,&catalog),"GPU03 MINERU");assert!(service_price(&price));assert_eq!(price_model_name(&price),"服务调用");
  assert!(!service_price(&json!({"model":"deepseek-search"})));
 }
 #[test] fn ordinary_usage_keeps_its_real_user(){let (_,who,_)=ledger_identity(&json!({"user_id":7,"key_id":8,"user_name":"Andy","key_name":"Hermes","model":"chat"}));assert_eq!(who,"Andy · Hermes");}
 #[test] fn provider_id_cannot_be_used_as_source_id(){let rows=vec![(12,"供应方 · 主 Key".into())];assert!(!selected_price_source("3",&rows));assert!(selected_price_source("12",&rows));assert!(!selected_price_source("",&rows));}
 #[test] fn source_label_preserves_provider_key_and_endpoint(){assert_eq!(price_source_label(&json!({"provider_name":"本地 GPU","base_url":"http://127.0.0.1:8001/v1"}),"向量 Key"),"本地 GPU · 向量 Key · http://127.0.0.1:8001/v1");}
}
