use super::{PageHead, policy_editor};
use super::usage_dashboard::UsageDashboard;
use super::usage_period::{UsagePeriod, period_query, default_period_query, applied_period};
use crate::{admin_api as api, Ctx};
use leptos::prelude::*;
use serde_json::{json, Value};
use wasm_bindgen::JsCast;

// Prices belong to a configured local model on one concrete source channel.
#[derive(Debug,Clone,PartialEq)]
struct PriceOption { source_id:i64, model:String, upstream:String, provider:String, channel:String, credential:String, base_url:String }
fn price_options_from_channels(rows:&[Value],credentials:&std::collections::BTreeMap<i64,(bool,String)>)->Vec<PriceOption> {
    let mut unique=std::collections::BTreeMap::new();
    for source in rows {
        let Some(id)=source["id"].as_i64() else {continue;};
        if source["enabled"]!=true || source["provider_enabled"]==false || credentials.get(&id).is_some_and(|(enabled,_)|!*enabled) {continue;}
        for mapping in api::rows(source,"models") {
            if mapping["enabled"]==false || mapping["pricing_enabled"]==false {continue;}
            let raw=mapping["model"].as_str().unwrap_or("");let model=tokengo_shared::normalize_model_name(raw);if model.is_empty(){continue;}
            let option=PriceOption {source_id:id,model:model.clone(),upstream:mapping["upstream"].as_str().filter(|s|!s.trim().is_empty()).unwrap_or(raw).to_string(),provider:source["provider_name"].as_str().unwrap_or("独立渠道").to_string(),channel:source["name"].as_str().unwrap_or("未命名渠道").to_string(),credential:credentials.get(&id).map(|(_,name)|name.clone()).unwrap_or_default(),base_url:source["base_url"].as_str().unwrap_or("").to_string()};
            unique.entry((id,model)).or_insert(option);
        }
    }
    unique.into_values().collect()
}
fn price_sources(ctx:Ctx)->RwSignal<Vec<PriceOption>> {
    let options=RwSignal::new(Vec::new());
    leptos::task::spawn_local(async move {
        let result=async {
            let response=api::request("GET","/api/channels",None).await?;let rows=api::rows(&response,"channels");
            let providers:std::collections::BTreeSet<_>=rows.iter().filter(|c|c["enabled"]==true&&c["provider_enabled"]!=false).filter_map(|c|c["provider_id"].as_i64()).collect();
            let mut credentials=std::collections::BTreeMap::new();
            for provider in providers {
                let response=api::request("GET",&format!("/api/providers/{provider}/credentials"),None).await?;
                for (position,key) in api::rows(&response,"credentials").iter().enumerate() {
                    if let Some(ids)=key["source_ids"].as_array() {for source in ids.iter().filter_map(Value::as_i64) {credentials.insert(source,(key["enabled"]==true,crate::provider_api::credential_label(key,position)));}}
                }
            }
            Ok::<_,crate::api::ApiErr>(price_options_from_channels(&rows,&credentials))
        }.await;
        match result {Ok(v)=>options.set(v),Err(e)=>ctx.note_err(&e)}
    });options
}
fn available_price_options(options:&[PriceOption],objects:&Value)->Vec<PriceOption> {
    let active=api::rows(objects,"channels").into_iter().filter(|c|c["state"]=="active").filter_map(|c|c["id"].as_i64()).collect::<std::collections::BTreeSet<_>>();
    options.iter().filter(|o|active.contains(&o.source_id)).cloned().collect()
}
fn selected_price_pair(channel:&str,model:&str,options:&[PriceOption],objects:&Value)->bool {
    let model=tokengo_shared::normalize_model_name(model);channel.parse::<i64>().ok().is_some_and(|id|available_price_options(options,objects).iter().any(|o|o.source_id==id&&o.model==model))
}
#[component]
fn PriceModelSource(prefix:&'static str,channel:RwSignal<String>,model:RwSignal<String>,options:RwSignal<Vec<PriceOption>>,objects:RwSignal<Value>)->impl IntoView {
    let model_id=format!("{prefix}-model");let channel_id=format!("{prefix}-channel");
    view!{
        <label for=model_id.clone()>"模型"</label><select id=model_id required prop:value=model on:change=move|e|{model.set(event_target_value(&e));channel.set(String::new());}>
            <option value="">"请选择已配置的模型"</option>{move||{let mut names=std::collections::BTreeMap::<String,usize>::new();for option in available_price_options(&options.get(),&objects.get()){*names.entry(option.model).or_default()+=1;}names.into_iter().map(|(name,n)|{let option_value=name.clone();let label=format!("{name}（{n} 个渠道）");view!{<option value=option_value prop:selected=move||model.get()==name>{label}</option>}}).collect_view()}}
        </select>
        <label for=channel_id.clone()>"来源渠道"</label><select id=channel_id required disabled=move||model.get().is_empty() prop:value=channel on:change=move|e|channel.set(event_target_value(&e))><option value="">"请选择该模型的来源渠道"</option>{move||available_price_options(&options.get(),&objects.get()).into_iter().filter(|o|o.model==model.get()).map(|o|{let label=if o.credential.is_empty(){format!("{} · {}",o.provider,o.channel)}else{format!("{} · {} · {}",o.provider,o.channel,o.credential)};let option_id=o.source_id.to_string();view!{<option value=option_id.clone() prop:selected=move||channel.get()==option_id>{label}</option>}}).collect_view()}</select>
        <div class="billing-selected-object" style="grid-column:1 / -1">{move||{
            let current=available_price_options(&options.get(),&objects.get()).into_iter().find(|o|o.source_id.to_string()==channel.get()&&o.model==model.get());
            if let Some(o)=current {view!{<p class="muted small">{format!("渠道：{} · {}",o.provider,o.channel)}</p><p class="muted small">{format!("上游模型：{}",o.upstream)}</p><p class="muted small" style="overflow-wrap:anywhere">{format!("API 地址：{}",o.base_url)}</p>}.into_any()}else{view!{<p class="muted small">"仅显示当前有效渠道中的已启用模型；未找到模型时，请先在供应方与接入中完成模型配置。"</p>}.into_any()}
        }}</div>
    }
}
fn row_currency(row:&Value)->&str {row["currency"].as_str().unwrap_or("USD")}
fn money_for(value:&Value,currency:&str)->String {value.as_i64().map(|n|format!("{} {}",policy_editor::money_input(n),if currency=="CNY"{"元"}else{"USD"})).unwrap_or_else(||"未知".into())}
fn row_money(row:&Value,field:&str)->String {money_for(&row[field],row_currency(row))}
fn unit_label(unit:&str)->&str {match unit {"second"=>"秒","minute"=>"分钟","hour"=>"小时","character"=>"字符","byte"=>"字节","image"=>"张图","video"=>"段视频",_=>"单位"}}
fn duration_unit(unit:&str)->bool {matches!(unit,"second"|"minute"|"hour")}
fn quantity_milli_input(n:i64)->String {let whole=n/1000;let fraction=n%1000;if fraction==0{whole.to_string()}else{format!("{whole}.{fraction:03}").trim_end_matches('0').to_string()}}
fn quantity_milli(s:&str)->Result<i64,String> {let v=policy_editor::optional_limit(s)?.ok_or_else(||"请填写预占上限".to_string())?;if v<=0||v%1000!=0{return Err("预占上限须大于0，最多3位小数。".into());}Ok(v/1000)}

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

fn named(row:&Value, field:&str, fallback:&str)->String {
    row[field].as_str().filter(|s|!s.trim().is_empty()).unwrap_or(fallback).to_string()
}
fn state_label(row:&Value)->String {named(row,"status_label","历史对象")}
fn price_is_current(row:&Value,rows:&[Value])->bool {
    !rows.iter().any(|other|other["channel_id"]==row["channel_id"]&&other["model"]==row["model"]&&row_currency(other)==row_currency(row)&&other["id"].as_i64()>row["id"].as_i64())
}
fn price_source_name(row:&Value,objects:&Value)->String {
    api::rows(objects,"channels").iter().find(|o|o["id"]==row["channel_id"]).map(|o|api::text(o,"name")).unwrap_or_else(||"历史接入（名称未留存）".into())
}
fn service_price(row:&Value)->bool {row["model"].as_str().is_some_and(|name|name.starts_with("passthrough:"))}
fn price_model_name(row:&Value)->String {if service_price(row){"服务调用".into()}else{api::text(row,"model")}}
fn price_effective(row:&Value,rows:&[Value],objects:&Value)->bool {
    row_currency(row)=="CNY"&&price_is_current(row,rows)&&api::rows(objects,"channels").iter().any(|o|o["id"]==row["channel_id"]&&o["state"]=="active")
}
fn ledger_identity(row: &Value) -> (String, String, String) {
    let source=named(row,"channel_name","历史上游接入");
    if row["auth_snapshot"]["kind"] == "admin_diagnostic" {
        let name = row["auth_snapshot"]["probe_name"].as_str().unwrap_or("自定义探针");
        let trigger = if row["auth_snapshot"]["trigger"] == "scheduled" { "定时执行" } else { "手动执行" };
        (source,format!("系统诊断 · {name} · {trigger}"),"不扣用户费用".into())
    } else {
        (source,format!("{} · {}",named(row,"user_name","历史用户"),named(row,"key_name","历史 Key")),row_money(row,"debit_micro"))
    }
}
fn active_object(row: &Value) -> bool {
    row["state"] == "active"
}

#[component]
fn ObjectSelect(
    id: &'static str,
    label: &'static str,
    group: &'static str,
    value: RwSignal<String>,
    objects: RwSignal<Value>,
    active_only: Signal<bool>,
    #[prop(optional)] owner: Option<RwSignal<String>>,
    #[prop(default = "全部")] empty_label: &'static str,
) -> impl IntoView {
    view! {
        <div class="billing-field">
            <label for=id>{label}</label>
            <select id=id prop:value=value on:change=move|e|value.set(event_target_value(&e))>
                <option value="" prop:selected=move||value.get().is_empty()>{empty_label}</option>
                {move|| {
                    let current = active_only.get();
                    api::rows(&objects.get(),group).into_iter()
                        .filter(|o| !current || active_object(o))
                        .filter(|o| owner.is_none_or(|owner| owner.get().is_empty() || api::text(o,"user_id")==owner.get()))
                        .map(|o| {
                            let label=if group=="keys"{
                                format!("{} · {}",api::text(&o,"user_name"),api::text(&o,"name"))
                            }else{api::text(&o,"name")};
                            let label=if active_object(&o){label}else{format!("{label}（{}）",state_label(&o))};
                            let option_id=api::text(&o,"id");
                            view!{<option value=option_id.clone() prop:selected=move||value.get()==option_id>{label}</option>}
                        }).collect_view()
                }}
            </select>
        </div>
    }
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
    view!{<div class="billing-subnav" role="group" aria-label="计费方式">{[("token","按 Token 计费"),("request","按请求计费"),("unit","按时长 / 数量计费")].into_iter().map(|(key,label)|view!{<button type="button" class:active=move||mode.get()==key aria-pressed=move||(mode.get()==key).to_string() on:click=move |_|mode.set(key)>{label}</button>}).collect_view()}</div>{move||match mode.get(){"token"=>view!{<Pricing ctx=ctx objects=objects/>}.into_any(),"request"=>view!{<RequestPricing ctx=ctx objects=objects/>}.into_any(),_=>view!{<ModelPricing ctx=ctx objects=objects kind="unit"/>}.into_any()}}}
}
#[component]
fn Budgets(ctx: Ctx,objects:RwSignal<Value>) -> impl IntoView {
    let scope=RwSignal::new("user".to_string());let id=RwSignal::new(String::new());let limit=RwSignal::new(String::new());let frozen=RwSignal::new(false);
    let currency=RwSignal::new("CNY".to_string());let kind=RwSignal::new("user".to_string());let state=RwSignal::new("active".to_string());
    let rows=RwSignal::new(Vec::<Value>::new());let busy=RwSignal::new(false);let error=RwSignal::new(String::new());let message=RwSignal::new(String::new());let editing=RwSignal::new(false);
    let load=move |_|{busy.set(true);error.set(String::new());message.set(String::new());let q=state.get_untracked();let coin=currency.get_untracked();rows.set(Vec::new());leptos::task::spawn_local(async move{match api::request("GET",&format!("/api/budgets?state={q}&currency={coin}"),None).await{Ok(v)=>{rows.set(api::rows(&v,"budgets"));message.set("预算已刷新".into());},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});};
    Effect::new(move |_|{state.get();currency.get();load(());});
    let save=move|ev:leptos::ev::SubmitEvent|{ev.prevent_default();if busy.get_untracked(){return;}let object=match api::integer(&id.get_untracked(),"预算对象",true){Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        let amount=match policy_editor::optional_limit(&limit.get_untracked()){Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        let target=scope.get_untracked();let group=if target=="user"{"users"}else{"keys"};
        if !api::rows(&objects.get_untracked(),group).iter().any(|o|o["id"]==object&&o["state"]=="active"){error.set("请选择当前有效的用户或 Key。".into());return;}
        busy.set(true);error.set(String::new());message.set(String::new());let hold=frozen.get_untracked();let coin=currency.get_untracked();
        leptos::task::spawn_local(async move{match api::request("PUT",&format!("/api/budgets?scope={target}&scope_id={object}&currency={coin}"),Some(json!({"limit_micro":amount,"frozen":hold}))).await{Ok(_)=>{billing_dialog("budget-dialog",false);load(());message.set("预算已保存，累计使用与在途预占均已保留。".into());},Err(e)=>{ctx.note_err(&e);error.set(e.message);busy.set(false);}}});};
    view!{<section class="card"><header class="card-head"><div><h2>"额度管理"</h2><p class="muted small">"用户额度约束名下所有 Key；单个 Key 可再设置独立额度。"</p></div><button type="button" class="primary" disabled=move||busy.get()||currency.get()!="CNY" on:click=move |_|{editing.set(false);scope.set(kind.get_untracked());id.set(String::new());limit.set(String::new());frozen.set(false);error.set(String::new());billing_dialog("budget-dialog",true);}>"设置额度"</button></header><div class="card-body">
    <div class="billing-toolbar"><div class="billing-field"><label for="budget-currency">"币种"</label><select id="budget-currency" prop:value=currency disabled=move||busy.get() on:change=move|e|currency.set(event_target_value(&e))><option value="CNY">"人民币 CNY"</option><option value="USD">"历史美元 USD"</option></select></div><div class="billing-subnav" role="group" aria-label="预算维度"><button type="button" class:active=move||kind.get()=="user" aria-pressed=move||(kind.get()=="user").to_string() on:click=move |_|kind.set("user".into())>"用户额度"</button><button type="button" class:active=move||kind.get()=="key" aria-pressed=move||(kind.get()=="key").to_string() on:click=move |_|kind.set("key".into())>"Key 额度"</button></div><div class="billing-field"><label for="budget-state">"对象状态"</label><select id="budget-state" prop:value=state disabled=move||busy.get() on:change=move|e|state.set(event_target_value(&e))><option value="active">"当前有效"</option><option value="historical">"历史 / 已停用"</option><option value="all">"全部"</option></select></div><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load(())>"刷新预算"</button></div>
    <p class="muted small">"人民币与历史美元额度分别管理，不换算或混合累计。美元限额只读，可显式调整冻结状态；新额度使用人民币。冻结会阻止后续调用。"</p>
    <div class="table-wrap"><table class="billing-budgets"><thead><tr><th>"对象名称"</th><th>"所属用户"</th><th>"总限额"</th><th>"已使用"</th><th>"在途预占"</th><th>"状态"</th><th>"管理"</th></tr></thead><tbody>{move||rows.get().into_iter().filter(|r|r["scope"]==kind.get()).map(|r|{
        let active=r["object_state"]=="active";let name=named(&r,"object_name","历史对象");let owner=named(&r,"user_name","—");let cap=if r["limit_micro"].is_null(){"无限额".into()}else{row_money(&r,"limit_micro")};let spent=row_money(&r,"spent_micro");let reserved=row_money(&r,"reserved_micro");let status=if r["frozen"]==true{format!("{} · 额度冻结",state_label(&r))}else{state_label(&r)};
        view!{<tr><td data-label="对象名称">{name}</td><td data-label="所属用户">{owner}</td><td data-label="总限额">{cap}</td><td data-label="已使用">{spent}</td><td data-label="在途预占">{reserved}</td><td data-label="状态">{status}</td><td data-label="管理">{if active{view!{<button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{editing.set(true);scope.set(api::text(&r,"scope"));id.set(api::text(&r,"scope_id"));limit.set(r["limit_micro"].as_i64().map(policy_editor::money_input).unwrap_or_default());frozen.set(r["frozen"]==true);error.set(String::new());billing_dialog("budget-dialog",true);}>"管理额度"</button>}.into_any()}else{view!{<span class="muted small">"历史只读"</span>}.into_any()}}</td></tr>}
    }).collect_view()}</tbody></table></div>
    <p role="status" aria-live="polite">{move||if busy.get(){"正在加载预算…".to_string()}else if rows.get().iter().all(|r|r["scope"]!=kind.get()){"当前分类暂无预算，可为有效对象设置额度。".into()}else{message.get()}}</p><p role="alert">{move||error.get()}</p>
    <button type="button" class="ghost" on:click=move |_|ctx.go(crate::Page::Users)>"管理用户与 Key 身份"</button>
    <BillingDialog id="budget-dialog" title="管理额度" busy=busy><form on:submit=save><fieldset disabled=move||busy.get()>
    <div class="billing-field"><label for="budget-scope">"预算对象类型"</label><select id="budget-scope" disabled=move||editing.get() prop:value=scope on:change=move|e|{scope.set(event_target_value(&e));id.set(String::new());}><option value="user">"用户"</option><option value="key">"API Key"</option></select></div>
    {move||if editing.get(){let group=if scope.get()=="user"{"users"}else{"keys"};let label=api::rows(&objects.get(),group).iter().find(|o|api::text(o,"id")==id.get()).map(|o|api::text(o,"name")).unwrap_or_else(||"对象已失效，请关闭并刷新".into());view!{<p class="billing-selected-object">{label}</p>}.into_any()}else if scope.get()=="user"{view!{<ObjectSelect id="budget-object-user" label="用户" group="users" value=id objects=objects active_only=Signal::derive(||true) empty_label="请选择"/>}.into_any()}else{view!{<ObjectSelect id="budget-object-key" label="API Key（所属用户 · 名称）" group="keys" value=id objects=objects active_only=Signal::derive(||true) empty_label="请选择"/>}.into_any()}}
    <div class="billing-field"><label for="budget-limit">{move||if currency.get()=="CNY"{"总限额（人民币元）"}else{"历史总限额（USD，只读）"}}</label><input id="budget-limit" readonly=move||currency.get()!="CNY" type="text" inputmode="decimal" prop:value=limit aria-describedby="budget-help budget-error" placeholder="留空表示无限额" on:input=move|e|limit.set(event_target_value(&e))/><small id="budget-help" class="muted">"总限额包含已使用金额，最多6位小数。"</small></div>
    <button type="button" class="ghost" disabled=move||currency.get()!="CNY" on:click=move |_|limit.set(String::new())>"解除限额"</button>
    <label class="account-check"><input type="checkbox" prop:checked=frozen on:change=move|e|frozen.set(event_target_checked(&e))/><span>"冻结后续调用（取消勾选恢复额度使用）"</span></label>
    <p id="budget-error" role="alert">{move||error.get()}</p><p class="muted small">"保存只调整额度策略，保留已使用金额、在途预占和历史账目。"</p><div class="module-dialog-actions"><button type="button" class="ghost" on:click=move |_|billing_dialog("budget-dialog",false)>"取消"</button><button type="submit" class="primary">"保存额度"</button></div></fieldset></form></BillingDialog>
    </div></section>}
}
#[component]
fn Pricing(ctx:Ctx,objects:RwSignal<Value>)->impl IntoView {view!{<ModelPricing ctx=ctx objects=objects kind="token"/>}}
#[component]
fn RequestPricing(ctx:Ctx,objects:RwSignal<Value>)->impl IntoView {view!{<ModelPricing ctx=ctx objects=objects kind="request"/>}}
#[component]
fn ModelPricing(ctx:Ctx,objects:RwSignal<Value>,kind:&'static str)->impl IntoView {
    let sources=price_sources(ctx);let versions=RwSignal::new("current".to_string());let rows=RwSignal::new(Vec::<Value>::new());
    let busy=RwSignal::new(false);let error=RwSignal::new(String::new());let message=RwSignal::new(String::new());
    let channel=RwSignal::new(String::new());let model=RwSignal::new(String::new());let input=RwSignal::new(String::new());let output=RwSignal::new(String::new());let max_in=RwSignal::new(String::new());let max_out=RwSignal::new(String::new());let unit=RwSignal::new("second".to_string());
    let token=kind=="token";let measured=kind=="unit";
    let (path,array,item,prefix,dialog,title)=match kind {"token"=>("/api/pricing","prices","price","price","price-dialog","Token 价格"),"unit"=>("/api/unit-pricing","unit_prices","unit_price","unit-price","unit-price-dialog","按用量计价"),_=>("/api/request-pricing","request_prices","request_price","request-price","request-price-dialog","按次价格")};
    let load=move |_| {if busy.get_untracked(){return;}busy.set(true);error.set(String::new());leptos::task::spawn_local(async move {match api::request("GET",path,None).await {Ok(v)=>rows.set(api::rows(&v,array)),Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});};
    load(());
    let save=move|ev:leptos::ev::SubmitEvent| {
        ev.prevent_default();if busy.get_untracked(){return;}
        if !selected_price_pair(&channel.get_untracked(),&model.get_untracked(),&sources.get_untracked(),&objects.get_untracked()){error.set("请选择当前有效渠道上已配置的模型；渠道失效后不能新建价格。".into());return;}
        let parsed=(||->Result<Value,String>{
            let source=api::integer(&channel.get_untracked(),"来源渠道",true)?;
            let price=policy_editor::optional_limit(&input.get_untracked())?.ok_or_else(||"请填写人民币价格。".to_string())?;
            let mut body=json!({"channel_id":source,"model":model.get_untracked(),"currency":"CNY"});
            if token {
                body["input_micro_per_million"]=json!(price);
                body["output_micro_per_million"]=json!(policy_editor::optional_limit(&output.get_untracked())?.ok_or_else(||"请填写输出价格。".to_string())?);
                body["max_input_tokens"]=json!(api::integer(&max_in.get_untracked(),"输入 token 预占上限",false)?);
                body["max_output_tokens"]=json!(api::integer(&max_out.get_untracked(),"输出 token 总上限",false)?);
            } else if measured {body["unit"]=json!(unit.get_untracked());body["micro_per_unit"]=json!(price);body["max_quantity_milli"]=json!(quantity_milli(&max_in.get_untracked())?);}
            else {body["micro_per_call"]=json!(price);}
            Ok(body)
        })();
        let body=match parsed {Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        busy.set(true);error.set(String::new());message.set(String::new());
        leptos::task::spawn_local(async move {match api::request("POST",path,Some(body)).await {Ok(v)=>{rows.update(|all|all.insert(0,v[item].clone()));message.set("人民币价格版本已创建，历史账目与原币种保留。".into());billing_dialog(dialog,false);},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});
    };
    view! {
        <section class="card"><header class="card-head"><div><h2>{title}</h2><p class="muted small">"先选模型，再确认来源渠道和上游模型。不同渠道可分别定价。"</p></div><div class="account-actions"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{channel.set(String::new());model.set(String::new());input.set(String::new());output.set(String::new());max_in.set(String::new());max_out.set(String::new());unit.set("second".into());error.set(String::new());message.set(String::new());billing_dialog(dialog,true);}>"配置人民币价格"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load(())>"刷新价格"</button></div></header><div class="card-body">
            <div class="billing-toolbar"><div class="billing-field"><label for=format!("{prefix}-versions")>"价格范围"</label><select id=format!("{prefix}-versions") prop:value=versions on:change=move|e|versions.set(event_target_value(&e))><option value="current">"当前人民币价格"</option><option value="history">"历史版本（含美元）"</option><option value="all">"全部版本"</option></select></div><p class="muted small">"新价格以人民币结算，最多6位小数；历史美元价格只读，不自动换算。0表示明确免费。"</p></div>
            {if token {view!{<p class="muted small">"输入、输出价格单位：元 / 百万 Token。预占上限用于额度预占与安全封顶，不代表上游实测上下文。"</p>}.into_any()}else if measured {view!{<p class="muted small">"ASR可按音频秒、分钟或小时配置价格；TTS需与供应方实际字节或字符口径一致。计量未知会保留未知，不能用请求耗时或token代替。其他单位只定义价格，不表示已启用对应模型接口。"</p>}.into_any()}else{view!{<p class="muted small">"单位：元 / 次。Search、Fetch与服务调用的按次价格独立于token价格。"</p>}.into_any()}}
            <div class="table-wrap"><table class="billing-price-table"><thead><tr><th>"模型 / 来源渠道"</th><th>"价格 / 币种"</th><th>"预占上限"</th><th>"版本状态"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().filter(|r|{let current=price_effective(r,&rows.get(),&objects.get());versions.get()=="all"||(versions.get()=="current"&&current)||(versions.get()=="history"&&!current)}).map(|r|{
                let source_name=price_source_name(&r,&objects.get());let model_label=price_model_name(&r);let current=price_effective(&r,&rows.get(),&objects.get());let legacy=row_currency(&r)!="CNY";
                let status=format!("{} · 版本 {}",if legacy {"历史美元"}else if current {"当前有效"}else{"历史 / 渠道失效"},api::text(&r,"id"));
                let amount=if token {format!("输入 {} / 输出 {} / 百万Token",row_money(&r,"input_micro_per_million"),row_money(&r,"output_micro_per_million"))}else if measured {format!("{} / {}",row_money(&r,"micro_per_unit"),unit_label(r["unit"].as_str().unwrap_or("")))}else{format!("{} / 次",row_money(&r,"micro_per_call"))};
                let cap=if token {format!("输入 {} / 输出 {} tokens",api::text(&r,"max_input_tokens"),api::text(&r,"max_output_tokens"))}else if measured {let n=r["max_quantity_milli"].as_i64().map(quantity_milli_input).unwrap_or_else(||"未知".into());format!("{n} {}",if duration_unit(r["unit"].as_str().unwrap_or("")){"音频秒"}else{unit_label(r["unit"].as_str().unwrap_or(""))})}else{"每次调用".into()};
                view!{<tr><td data-label="模型 / 来源"><strong class="billing-price-model">{model_label}</strong><small class="billing-price-source">{source_name}</small></td><td data-label="价格 / 币种">{amount}</td><td data-label="预占上限">{cap}</td><td data-label="版本状态"><span class="tag billing-price-version">{status}</span></td><td data-label="操作">{if service_price(&r) {view!{<button type="button" class="ghost" on:click=move |_|ctx.go(crate::Page::Services)>"管理服务价格"</button>}.into_any()}else{view!{<button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{
                    channel.set(api::text(&r,"channel_id"));model.set(api::text(&r,"model"));
                    input.set(if legacy {String::new()}else{r[if token {"input_micro_per_million"}else if measured {"micro_per_unit"}else{"micro_per_call"}].as_i64().map(policy_editor::money_input).unwrap_or_default()});
                    output.set(if legacy {String::new()}else{r["output_micro_per_million"].as_i64().map(policy_editor::money_input).unwrap_or_default()});
                    max_in.set(if measured {r["max_quantity_milli"].as_i64().map(quantity_milli_input).unwrap_or_default()}else{r["max_input_tokens"].as_i64().map(|n|n.to_string()).unwrap_or_default()});max_out.set(r["max_output_tokens"].as_i64().map(|n|n.to_string()).unwrap_or_default());unit.set(r["unit"].as_str().unwrap_or("second").to_string());error.set(String::new());message.set(if legacy {"旧美元金额不会复制成人民币，请重新填写人民币价格。"}else{"保存将创建人民币新版本，原版本保持只读。"}.into());billing_dialog(dialog,true);
                }>{if legacy {"为此模型配置人民币价格"}else{"创建新版本"}}</button>}.into_any()}}</td></tr>}
            }).collect_view()}</tbody></table></div>
            <p role="status">{move||if busy.get(){"正在加载价格…"}else if rows.get().iter().all(|r|{let current=price_effective(r,&rows.get(),&objects.get());!(versions.get()=="all"||(versions.get()=="current"&&current)||(versions.get()=="history"&&!current))}){"当前范围暂无价格，可新建人民币价格或切换历史版本。"}else{""}}</p><p role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p>
            <BillingDialog id=dialog title="配置人民币价格（创建新版本）" busy=busy><form on:submit=save><fieldset disabled=move||busy.get()><div class="form-grid">
                <PriceModelSource prefix=prefix channel=channel model=model options=sources objects=objects/>
                {if measured {view!{<label for="unit-price-unit">"计价单位"</label><select id="unit-price-unit" prop:value=unit on:change=move|e|unit.set(event_target_value(&e))><option value="second">"音频秒"</option><option value="minute">"音频分钟"</option><option value="hour">"音频小时"</option><option value="character">"字符"</option><option value="byte">"UTF-8 字节"</option><option value="image">"张图"</option><option value="video">"段视频"</option></select>}.into_any()}else{().into_any()}}
                <label for=format!("{prefix}-in")>{move||if token {"输入价格（元 / 百万 Token）".to_string()}else if measured {format!("价格（元 / {}）",unit_label(&unit.get()))}else{"价格（元 / 次）".into()}}</label><input id=format!("{prefix}-in") required type="text" inputmode="decimal" prop:value=input aria-describedby=format!("{prefix}-error") on:input=move|e|input.set(event_target_value(&e))/>
                {if token {view!{<label for="price-out">"输出价格（元 / 百万 Token）"</label><input id="price-out" required type="text" inputmode="decimal" prop:value=output aria-describedby="price-error" on:input=move|e|output.set(event_target_value(&e))/><label for="price-max-in">"输入 token 预占上限"</label><input id="price-max-in" required inputmode="numeric" prop:value=max_in aria-describedby="price-error" on:input=move|e|max_in.set(event_target_value(&e))/><label for="price-max-out">"输出 token 总上限（含 n）"</label><input id="price-max-out" required inputmode="numeric" prop:value=max_out aria-describedby="price-error" on:input=move|e|max_out.set(event_target_value(&e))/>}.into_any()}else if measured {view!{<label for="unit-price-max">{move||if duration_unit(&unit.get()){"单次预占音频时长上限（秒）".to_string()}else{format!("最大预占数量（{}）",unit_label(&unit.get()))}}</label><input id="unit-price-max" required inputmode="decimal" prop:value=max_in aria-describedby="unit-price-error" on:input=move|e|max_in.set(event_target_value(&e))/><p class="muted small" style="grid-column:1 / -1">"音频上限始终填写秒，可精确到毫秒；请求前按上限预占，完成后按实际计量结算。价格按所选秒、分钟或小时换算。其他数量最多3位小数。"</p><p class="muted small" style="grid-column:1 / -1">"ASR 已支持按音频时长结算；字符、字节、图片和视频单价可预先配置，对应调用接口尚未接入自动结算。"</p>}.into_any()}else{().into_any()}}
            </div><p id=format!("{prefix}-error") role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><div class="module-dialog-actions"><button type="button" class="ghost" on:click=move |_|billing_dialog(dialog,false)>"取消"</button><button type="submit" class="primary">"创建人民币价格版本"</button></div></fieldset></form></BillingDialog>
        </div></section>
    }
}
fn usage_status(row:&Value)->String {
    let outcome=match row["outcome"].as_str().unwrap_or(""){"ok"|"success"=>"成功","error"|"failed"|"failure"|"upstream_error"=>"失败","cancelled"=>"已取消","interrupted"|"body_error"=>"响应中断",""=>"",_=>"已结束"};
    let status=match row["status"].as_str().unwrap_or(""){"reserved"=>"待结算","unpriced"=>"未定价","released"|"not_sent"=>"已释放",_ if !row["settled_ms"].is_null()=>"已结算",_=>"待确认"};
    if outcome.is_empty(){status.into()}else{format!("{outcome} · {status}")}
}
fn current_hold(row:&Value)->String {
    if row["status"]=="reserved"&&row["settled_ms"].is_null(){row_money(row,"reserved_micro")}else{money_for(&json!(0),row_currency(row))}
}
fn usage_basis(row:&Value)->String {match row["usage_certainty"].as_str().unwrap_or(""){"reported"=>"上游报告","measured"=>"已计量","estimated"=>"估算","unpriced"=>"未定价","not_sent"=>"未发送","pending"=>"待确认",_=>"未知"}.into()}
fn updated_time()->String {let d=js_sys::Date::new_0();format!("{:02}:{:02}:{:02}",d.get_hours(),d.get_minutes(),d.get_seconds())}
const DEFAULT_USAGE_STATE: &str = "active";
const DEFAULT_USAGE_LIMIT: &str = "50";

fn default_usage_query() -> String {
    format!("limit={DEFAULT_USAGE_LIMIT}&state={DEFAULT_USAGE_STATE}&currency=CNY{}", default_period_query())
}

fn usage_query_limit(query:&str)->String {
    query.split('&').find_map(|part|part.strip_prefix("limit=")).unwrap_or(DEFAULT_USAGE_LIMIT).to_string()
}
fn usage_query_with_limit(query:&str, limit:u32)->String {
    let filters=query.split('&').filter(|p|!p.starts_with("limit=")&&!p.starts_with("before_id=")&&!p.is_empty()).collect::<Vec<_>>().join("&");
    format!("limit={limit}&{filters}")
}

#[component]
fn Usage(ctx: Ctx,objects:RwSignal<Value>) -> impl IntoView {
    let period=RwSignal::new("today".to_string());let start=RwSignal::new(String::new());let end=RwSignal::new(String::new());let summary=RwSignal::new(Value::Null);let analytics=RwSignal::new(Value::Null);let advanced=RwSignal::new(false);let applied_time=RwSignal::new(String::new());
    let snapshot=RwSignal::new(String::new());let user=RwSignal::new(String::new());let key=RwSignal::new(String::new());let channel=RwSignal::new(String::new());let model=RwSignal::new(String::new());let state=RwSignal::new(DEFAULT_USAGE_STATE.to_string());let currency=RwSignal::new("CNY".to_string());let count=RwSignal::new(DEFAULT_USAGE_LIMIT.to_string());
    let current_only=Signal::derive(move||state.get()==DEFAULT_USAGE_STATE);
    Effect::new(move |_| {
        let selected_user=user.get();let catalog=objects.get();let current=current_only.get();
        let selected_key=key.get_untracked();
        if !selected_key.is_empty() && !api::rows(&catalog,"keys").iter().any(|k|api::text(k,"id")==selected_key && (selected_user.is_empty()||api::text(k,"user_id")==selected_user) && (!current||active_object(k))) {key.set(String::new());}
    });
    let change_state=move|e| {
        let next=event_target_value(&e);
        if next==DEFAULT_USAGE_STATE {
            objects.with_untracked(|catalog| {
                for (group,selection) in [("users",user),("keys",key),("channels",channel)] {
                    let selected=selection.get_untracked();
                    if !selected.is_empty() && !api::rows(catalog,group).iter()
                        .any(|o|active_object(o)&&api::text(o,"id")==selected) {
                        selection.set(String::new());
                    }
                }
            });
        }
        state.set(next);
    };
    let rows=RwSignal::new(Vec::<Value>::new());let cursor=RwSignal::new(None::<i64>);let busy=RwSignal::new(false);let error=RwSignal::new(String::new());let message=RwSignal::new(String::new());let page=RwSignal::new(0usize);let starts=RwSignal::new(vec![None::<i64>]);let active_query=RwSignal::new(default_usage_query());
    let fetch=move|base:String,before:Option<i64>,target:usize|{if busy.get_untracked(){return;}busy.set(true);error.set(String::new());message.set("正在查询…".into());let query=if let Some(id)=before{format!("{base}&before_id={id}")}else{base.clone()};
        leptos::task::spawn_local(async move{match api::request("GET",&format!("/api/usage?{query}"),None).await{Ok(v)=>{let found=api::rows(&v,"entries");let len=found.len();cursor.set(v["next_before_id"].as_i64());rows.set(found);summary.set(v["summary"].clone());analytics.set(v["analytics"].clone());page.set(target);if target==0{starts.set(vec![None]);count.set(usage_query_limit(&base));applied_time.set(applied_period(&base));active_query.set(base);}else{starts.update(|s|{s.truncate(target);s.push(before);});}message.set(format!("查询完成 · 第 {} 页 · {} 条记录 · 更新于 {}",target+1,len,updated_time()));},Err(e)=>{ctx.note_err(&e);count.set(usage_query_limit(&active_query.get_untracked()));error.set(e.message);message.set("查询失败，保留上次结果。".into());}}busy.set(false);});};
    fetch(default_usage_query(),None,0);
    let run_search=move||{if busy.get_untracked(){return;}let n=match api::integer(&count.get_untracked(),"每页条数",true){Ok(v)if v<=200=>v,_=>{error.set("每页条数必须在1至200之间".into());return;}};let mut q=format!("limit={n}&state={}&currency={}",state.get_untracked(),currency.get_untracked());for (value,name) in [(user,"user_id"),(key,"key_id"),(channel,"channel_id")] {if !value.get_untracked().is_empty(){q.push_str(&format!("&{name}={}",api::encode(&value.get_untracked())));}}if !model.get_untracked().trim().is_empty(){q.push_str(&format!("&model={}",api::encode(model.get_untracked().trim())));}match period_query(&period.get_untracked(),&start.get_untracked(),&end.get_untracked()){Ok(time)=>q.push_str(&time),Err(e)=>{error.set(e);return;}}fetch(q,None,0);};
    let search=move|ev:leptos::ev::SubmitEvent|{ev.prevent_default();run_search();};
    view!{<section class="card billing-usage"><header class="card-head"><div><h2>"用量看板"</h2><p class="muted small">"查看区间消耗、用量趋势与调用明细。"</p></div></header><div class="card-body">
    <form class="usage-filter-bar" on:submit=search><fieldset disabled=move||busy.get()><div class="usage-filter-row">
    <UsagePeriod period=period start=start end=end compact=true/>
    <ObjectSelect id="usage-user" label="用户" group="users" value=user objects=objects active_only=current_only/>
    <ObjectSelect id="usage-key" label="API Key" group="keys" value=key objects=objects active_only=current_only owner=user/>
    <div class="billing-field"><label for="usage-model">"模型"</label><input id="usage-model" placeholder="全部模型" prop:value=model on:input=move|e|model.set(event_target_value(&e))/></div>
    <div class="usage-inline-actions"><button type="submit" class="primary">{move||if busy.get(){"查询中…"}else{"查询"}}</button><button type="button" class="ghost" on:click=move |_|{period.set("today".into());start.set(String::new());end.set(String::new());user.set(String::new());key.set(String::new());channel.set(String::new());model.set(String::new());state.set(DEFAULT_USAGE_STATE.into());currency.set("CNY".into());count.set(DEFAULT_USAGE_LIMIT.into());fetch(default_usage_query(),None,0);}>"重置"</button><button type="button" class="ghost" aria-controls="usage-advanced" aria-expanded=move||advanced.get().to_string() on:click=move |_|advanced.update(|open|*open=!*open)>"更多筛选"</button></div>
    </div><div id="usage-advanced" class="usage-advanced" hidden=move||!advanced.get()>
    <div class="billing-field"><label for="usage-currency">"币种"</label><select id="usage-currency" prop:value=currency on:change=move|e|currency.set(event_target_value(&e))><option value="CNY">"人民币 CNY"</option><option value="USD">"历史美元 USD"</option></select></div>
    <ObjectSelect id="usage-channel" label="上游接入" group="channels" value=channel objects=objects active_only=current_only/>
    <div class="billing-field"><label for="usage-state">"对象状态"</label><select id="usage-state" prop:value=state on:change=change_state><option value="active">"当前有效"</option><option value="historical">"历史对象"</option><option value="all">"全部（含历史）"</option></select></div>
    <span class="muted small">"模型精确匹配；停用或删除的用户、Key 和上游可切换历史对象查询。"</span>
    </div></fieldset></form>
    <p id="usage-error" role="alert">{move||error.get()}</p><p class="billing-query-status" role="status" aria-live="polite">{move||message.get()}</p>
    <p class="usage-applied-range muted small">{move||format!("统计区间：{} · 本地时区",applied_time.get())}</p>
    <UsageDashboard summary=summary analytics=analytics/>
    <header class="usage-ledger-heading"><div><h3>"调用明细"</h3><span class="muted small">"所选区间内的逐次账目"</span></div><div class="usage-page-size"><label for="usage-limit">"每页"</label><select id="usage-limit" disabled=move||busy.get() prop:value=count on:change=move |e|{let n=event_target_value(&e).parse::<u32>().unwrap_or(50);fetch(usage_query_with_limit(&active_query.get_untracked(),n),None,0);} ><option value="10">"10 条"</option><option value="50">"50 条"</option><option value="100">"100 条"</option><option value="200">"200 条"</option></select></div></header>
    <div class="table-wrap"><table><thead><tr><th>"用户 / Key"</th><th>"上游接入 / 模型"</th><th>"结果"</th><th>"用量"</th><th>"用户扣款"</th><th>"上游成本"</th><th>"当前占用"</th><th>"详情"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|r|{
        let (source,who,debit)=ledger_identity(&r);let model_name=api::text(&r,"model");let state_text=usage_status(&r);let basis=usage_basis(&r);let units=if r["billing_unit"]=="request"{format!("{} 次",api::billed_requests(&r))}else if r["billing_unit"]=="token"{format!("输入 {} / 输出 {} token",api::text(&r,"input_tokens"),api::text(&r,"output_tokens"))}else{crate::fmt::quantity(r["billing_unit"].as_str().unwrap_or(""),r["quantity_milli"].as_i64())};let cost=row_money(&r,"observed_cost_micro");let reserved=current_hold(&r);let historical=r["record_state"]=="historical";let detail=api::pretty(&r);
        view!{<tr><td data-label="用户 / Key"><strong>{who}</strong>{if historical{Some(view!{<small class="billing-history">"关联历史对象"</small>})}else{None}}</td><td data-label="上游接入 / 模型">{source}<small>{model_name}</small></td><td data-label="结果">{state_text}<small>{basis}</small></td><td data-label="用量">{units}</td><td data-label="用户扣款">{debit}</td><td data-label="上游成本">{cost}</td><td data-label="当前占用">{reserved}</td><td data-label="详情"><button type="button" class="ghost" on:click=move |_|{snapshot.set(detail.clone());billing_dialog("ledger-snapshot-dialog",true);}>"查看详情"</button></td></tr>}
    }).collect_view()}</tbody></table></div>
    {move||if !busy.get()&&rows.get().is_empty(){Some(view!{<p class="muted">"当前筛选没有账目，可调整条件或重置筛选。"</p>})}else{None}}
    <nav class="billing-pagination" aria-label="账本分页"><span>{move||format!("第 {} 页 · 本页 {} 条",page.get()+1,rows.get().len())}</span><div class="billing-pagination-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|fetch(active_query.get_untracked(),None,0)>"最新一页"</button><button type="button" class="ghost" disabled=move||busy.get()||page.get()==0 on:click=move |_|{let target=page.get_untracked()-1;fetch(active_query.get_untracked(),starts.get_untracked()[target],target);}>"上一页"</button><button type="button" class="ghost" disabled=move||busy.get()||cursor.get().is_none()||rows.get().is_empty() on:click=move |_|fetch(active_query.get_untracked(),cursor.get_untracked(),page.get_untracked()+1)>"较早一页"</button></div></nav>
    <p class="muted small">"分页沿用已查询条件；修改筛选后点击“查询”，从最新一页重新开始。"</p>
    <details class="billing-help"><summary>"如何理解账目与费用"</summary><p>"扣款是用户额度的实际扣减；成本为上游报告或估算，不能直接当作供应商对账结果；当前占用仅指尚未结算的额度，结算后显示0；初始预占记录保留在详情中。金额以账目原币种显示；人民币与历史美元分开查询，未知不等于免费。"</p><p>"系统诊断单独标识，不扣用户费用。历史对象包括停用、删除、撤销或到期的用户、Key、上游接入。"</p>{move||api::billing_page_summary(&rows.get()).into_iter().map(|line|view!{<p class="muted small">{line}</p>}).collect_view()}</details>
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
  assert_eq!(current_hold(&settled),"0 USD");assert_eq!(usage_status(&settled),"成功 · 已结算");
  assert_eq!(current_hold(&json!({"status":"reserved","settled_ms":null,"reserved_micro":736})),"0.000736 USD");
 }
 #[test] fn passthrough_prices_use_service_catalog_without_llm_source_options(){
  let price=json!({"channel_id":32,"model":"passthrough:12"});let catalog=json!({"channels":[{"id":32,"name":"GPU03 MINERU","state":"active"}]});
  assert_eq!(price_source_name(&price,&catalog),"GPU03 MINERU");assert!(service_price(&price));assert_eq!(price_model_name(&price),"服务调用");
  assert!(!service_price(&json!({"model":"deepseek-search"})));
 }
 #[test] fn ordinary_usage_keeps_its_real_user(){let (_,who,_)=ledger_identity(&json!({"user_id":7,"key_id":8,"user_name":"Andy","key_name":"Hermes","model":"chat"}));assert_eq!(who,"Andy · Hermes");}
 #[test] fn price_options_are_model_first_deduplicated_and_ignore_disabled_sources(){
  let source=json!({"id":12,"name":"硅基渠道","provider_name":"硅基流动","enabled":true,"provider_enabled":true,"base_url":"https://api.test/v1","models":[{"model":"ASR","upstream":"vendor/asr","enabled":true},{"model":"ASR","upstream":"vendor/asr","enabled":true},{"model":"disabled","enabled":false}]});
  let mut disabled=source.clone();disabled["id"]=json!(13);disabled["enabled"]=json!(false);
  let mut credentials=std::collections::BTreeMap::new();credentials.insert(12,(true,"主Key".into()));
  let options=price_options_from_channels(&[source,disabled],&credentials);assert_eq!(options.len(),1);assert_eq!(options[0].upstream,"vendor/asr");
  let active=json!({"channels":[{"id":12,"state":"active"}]});assert!(selected_price_pair("12","ASR",&options,&active));assert!(!selected_price_pair("3","ASR",&options,&active));assert!(!selected_price_pair("12","other",&options,&active));
  let expired=json!({"channels":[{"id":12,"state":"historical"}]});assert!(available_price_options(&options,&expired).is_empty());
 }
 #[test] fn unit_quantities_preserve_audio_milliseconds_and_reject_too_many_decimals(){assert_eq!(quantity_milli("61.234").unwrap(),61234);assert_eq!(quantity_milli_input(61234),"61.234");assert!(quantity_milli("0").is_err());assert!(quantity_milli("1.2345").is_err());}
 #[test] fn legacy_money_is_never_relabelled_as_cny(){let old=json!({"debit_micro":1230000});assert_eq!(row_money(&old,"debit_micro"),"1.23 USD");let new=json!({"currency":"CNY","debit_micro":1230000});assert_eq!(row_money(&new,"debit_micro"),"1.23 元");}

}

#[cfg(test)]
mod usage_paging_tests {
    use super::*;
    #[test]
    fn changing_page_size_keeps_applied_filters_and_resets_cursor() {
        let old="limit=50&state=historical&user_id=7&key_id=8&model=two%20words&start_at=100&end_at=200&before_id=9";
        let next=usage_query_with_limit(old,100);
        assert_eq!(next,"limit=100&state=historical&user_id=7&key_id=8&model=two%20words&start_at=100&end_at=200");
        assert_eq!(usage_query_limit(&next),"100");
    }
}

#[cfg(test)] mod globally_disabled_price_tests {
 use super::*;
 #[test] fn globally_disabled_models_are_not_new_price_targets(){let rows=vec![json!({"id":1,"enabled":true,"models":[{"model":"retired","enabled":true,"pricing_enabled":false}]})];assert!(price_options_from_channels(&rows,&std::collections::BTreeMap::new()).is_empty());}
}

#[cfg(test)] mod unit_version_group_tests {
 use super::*;
 #[test] fn changing_rate_unit_replaces_the_same_model_price(){let old=json!({"id":1,"channel_id":2,"model":"asr","currency":"CNY","unit":"minute"});let new=json!({"id":2,"channel_id":2,"model":"asr","currency":"CNY","unit":"hour"});let rows=vec![old.clone(),new.clone()];assert!(!price_is_current(&old,&rows));assert!(price_is_current(&new,&rows));}
}
