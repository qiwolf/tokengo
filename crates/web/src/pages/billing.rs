use super::{PageHead, model_specifications::quantity_milli_input, policy_editor};
use super::usage_dashboard::UsageDashboard;
use super::usage_period::{UsagePeriod, period_query, default_period_query, applied_period};
use crate::{admin_api as api, Ctx};
use leptos::prelude::*;
use serde_json::{json, Value};
use wasm_bindgen::JsCast;

// Prices belong to a configured local model on one concrete source channel.
#[derive(Debug,Clone,PartialEq)]
struct PriceOption { source_id:i64, model:String, upstream:String, provider:String, channel:String, credential:String, base_url:String, metadata:Value }
fn price_options_from_channels(rows:&[Value],credentials:&std::collections::BTreeMap<i64,(bool,String)>)->Vec<PriceOption> {
    let mut unique=std::collections::BTreeMap::new();
    for source in rows {
        let Some(id)=source["id"].as_i64() else {continue;};
        if source["enabled"]!=true || source["provider_enabled"]==false || credentials.get(&id).is_some_and(|(enabled,_)|!*enabled) {continue;}
        for mapping in api::rows(source,"models") {
            if mapping["enabled"]==false || mapping["pricing_enabled"]==false {continue;}
            let raw=mapping["model"].as_str().unwrap_or("");let model=tokengo_shared::normalize_model_name(raw);if model.is_empty(){continue;}
            let option=PriceOption {source_id:id,model:model.clone(),upstream:mapping["upstream"].as_str().filter(|s|!s.trim().is_empty()).unwrap_or(raw).to_string(),provider:source["provider_name"].as_str().unwrap_or("独立渠道").to_string(),channel:source["name"].as_str().unwrap_or("未命名渠道").to_string(),credential:credentials.get(&id).map(|(_,name)|name.clone()).unwrap_or_default(),base_url:source["base_url"].as_str().unwrap_or("").to_string(),metadata:Value::Null};
            unique.entry((id,model)).or_insert(option);
        }
    }
    unique.into_values().collect()
}
fn apply_price_metadata(options: &mut [PriceOption], response: &Value) {
    let metadata = api::rows(response, "options");
    for option in options {
        option.metadata = metadata.iter().find(|row| {
            row["channel_id"].as_i64() == Some(option.source_id)
                && row["model"].as_str().is_some_and(|name| tokengo_shared::same_model_name(name, &option.model))
        }).cloned().unwrap_or(Value::Null);
    }
}

async fn refresh_price_metadata(options: RwSignal<Vec<PriceOption>>) -> Result<(), crate::api::ApiErr> {
    if options.try_get_untracked().is_none_or(|values| values.is_empty()) {
        return Ok(());
    }
    let result = api::request("GET", "/api/pricing/options", None).await;
    options.try_update(|values| apply_price_metadata(values, result.as_ref().unwrap_or(&Value::Null)));
    result.map(|_| ())
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
            let metadata=api::request("GET","/api/pricing/options",None).await?;
            let mut options=price_options_from_channels(&rows,&credentials);
            apply_price_metadata(&mut options, &metadata);
            Ok::<_,crate::api::ApiErr>(options)
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
fn PriceModelSource(prefix:&'static str,channel:RwSignal<String>,model:RwSignal<String>,options:RwSignal<Vec<PriceOption>>,objects:RwSignal<Value>,on_selection:Callback<()>)->impl IntoView {
    let model_id=format!("{prefix}-model");let channel_id=format!("{prefix}-channel");
    view!{
        <label for=model_id.clone()>"模型"</label><select id=model_id required prop:value=model on:change=move|e|{model.set(event_target_value(&e));let matches=available_price_options(&options.get_untracked(),&objects.get_untracked()).into_iter().filter(|o|o.model==model.get_untracked()).collect::<Vec<_>>();channel.set(if matches.len()==1{matches[0].source_id.to_string()}else{String::new()});on_selection.run(());}>
            <option value="">"请选择已配置的模型"</option>{move||{let mut names=std::collections::BTreeMap::<String,usize>::new();for option in available_price_options(&options.get(),&objects.get()){*names.entry(option.model).or_default()+=1;}names.into_iter().map(|(name,n)|{let option_value=name.clone();let label=format!("{name}（{n} 个渠道）");view!{<option value=option_value prop:selected=move||model.get()==name>{label}</option>}}).collect_view()}}
        </select>
        <label for=channel_id.clone()>"来源渠道"</label><select id=channel_id required disabled=move||model.get().is_empty() prop:value=channel on:change=move|e|{channel.set(event_target_value(&e));on_selection.run(());} ><option value="">"请选择该模型的来源渠道"</option>{move||available_price_options(&options.get(),&objects.get()).into_iter().filter(|o|o.model==model.get()).map(|o|{let label=if o.credential.is_empty(){format!("{} · {}",o.provider,o.channel)}else{format!("{} · {} · {}",o.provider,o.channel,o.credential)};let option_id=o.source_id.to_string();view!{<option value=option_id.clone() prop:selected=move||channel.get()==option_id>{label}</option>}}).collect_view()}</select>
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
fn duration_subject(task:&str)->&'static str {match task{"video"=>"视频","asr"|"audio"=>"音频",_=>"媒体"}}

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
    if let Some(latest)=row["is_latest"].as_bool(){return latest;}
    !rows.iter().any(|other|other["channel_id"]==row["channel_id"]&&other["model"]==row["model"]&&price_row_kind(other)==price_row_kind(row)&&row_currency(other)==row_currency(row)&&other["id"].as_i64()>row["id"].as_i64())
}
fn price_source_name(row:&Value,objects:&Value)->String {
    api::rows(objects,"channels").iter().find(|o|o["id"]==row["channel_id"]).map(|o|api::text(o,"name")).unwrap_or_else(||"历史接入（名称未留存）".into())
}
fn service_price(row:&Value)->bool {row["model"].as_str().is_some_and(|name|name.starts_with("passthrough:"))}
fn price_model_name(row:&Value)->String {if service_price(row){"服务调用".into()}else{api::text(row,"model")}}
fn price_effective(row:&Value,rows:&[Value],objects:&Value)->bool {
    row_currency(row)=="CNY"&&row["enabled"]!=false&&row["deleted"]!=true&&price_is_current(row,rows)&&api::rows(objects,"channels").iter().any(|o|o["id"]==row["channel_id"]&&o["state"]=="active")
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
    view!{<div class="page admin-page billing-page"><PageHead title="用量与账务" desc="查看消费记录、维护价格、查询调用费用。"/>
    <nav class="billing-nav" aria-label="账务管理维度">{[("budgets","消费汇总","用户与 Key 的账目记录"),("prices","价格配置","模型计费与历史版本"),("usage","用量账本","调用明细与费用追踪")].into_iter().map(|(key,title,desc)|view!{<button type="button" class:active=move||section.get()==key aria-pressed=move||(section.get()==key).to_string() on:click=move |_|section.set(key)><strong>{title}</strong><span>{desc}</span></button>}).collect_view()}</nav>
    <p role="alert">{move||object_error.get()}</p>
    {move||match section.get(){"budgets"=>view!{<Budgets ctx=ctx/>}.into_any(),"prices"=>view!{<PriceWorkspace ctx=ctx objects=objects/>}.into_any(),_=>view!{<Usage ctx=ctx objects=objects/>}.into_any()}}
    </div>}
}
#[component]
fn PriceWorkspace(ctx:Ctx,objects:RwSignal<Value>)->impl IntoView {
    view!{<ModelPricing ctx=ctx objects=objects/>}
}
#[component]
fn Budgets(ctx: Ctx) -> impl IntoView {
    let rows=RwSignal::new(Vec::<Value>::new());let busy=RwSignal::new(false);let error=RwSignal::new(String::new());
    let kind=RwSignal::new("user".to_string());let state=RwSignal::new("active".to_string());let currency=RwSignal::new("CNY".to_string());
    let load=Callback::new(move |()|{busy.set(true);error.set(String::new());let q=state.get_untracked();let coin=currency.get_untracked();rows.set(Vec::new());leptos::task::spawn_local(async move{match api::request("GET",&format!("/api/budgets?state={q}&currency={coin}"),None).await{Ok(v)=>rows.set(api::rows(&v,"budgets")),Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});});
    Effect::new(move |_|{state.get();currency.get();load.run(());});
    view!{<section class="card"><header class="card-head"><div><h2>"消费汇总"</h2><p class="muted small">"查看用户与 Key 的消费记录，不设置本地消费额度。"</p></div></header><div class="card-body">
    <div class="billing-toolbar"><div class="billing-field"><label for="budget-currency">"币种"</label><select id="budget-currency" prop:value=currency disabled=move||busy.get() on:change=move|e|currency.set(event_target_value(&e))><option value="CNY">"人民币 CNY"</option><option value="USD">"历史美元 USD"</option></select></div><div class="billing-subnav" role="group" aria-label="消费记录维度"><button type="button" class:active=move||kind.get()=="user" aria-pressed=move||(kind.get()=="user").to_string() on:click=move |_|kind.set("user".into())>"用户记录"</button><button type="button" class:active=move||kind.get()=="key" aria-pressed=move||(kind.get()=="key").to_string() on:click=move |_|kind.set("key".into())>"Key 记录"</button></div><div class="billing-field"><label for="budget-state">"对象状态"</label><select id="budget-state" prop:value=state disabled=move||busy.get() on:change=move|e|state.set(event_target_value(&e))><option value="active">"当前有效"</option><option value="historical">"历史 / 已停用"</option><option value="all">"全部"</option></select></div><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load.run(())>"刷新记录"</button></div>
    <p class="muted small" role="status">"本地预算门限已取消；历史限额和冻结标记不再限制调用。费用与用量记录保留，历史预留金额不代表当前调用限制。"</p>
    <div class="table-wrap"><table class="billing-budgets"><thead><tr><th>"对象名称"</th><th>"所属用户"</th><th>"累计费用"</th><th>"历史预留金额"</th><th>"对象状态"</th></tr></thead><tbody>{move||rows.get().into_iter().filter(|r|r["scope"]==kind.get()).map(|r|{
        let name=named(&r,"object_name","历史对象");let owner=named(&r,"user_name","—");let spent=row_money(&r,"spent_micro");let reserved=row_money(&r,"reserved_micro");let status=state_label(&r);
        view!{<tr><td data-label="对象名称">{name}</td><td data-label="所属用户">{owner}</td><td data-label="累计费用">{spent}</td><td data-label="历史预留金额">{reserved}</td><td data-label="对象状态">{status}</td></tr>}
    }).collect_view()}</tbody></table></div>
    <p role="status" aria-live="polite">{move||if busy.get(){"正在加载消费记录…"}else if rows.get().iter().all(|r|r["scope"]!=kind.get()){"当前分类暂无消费记录。"}else{"消费记录已读取。"}}</p><p role="alert">{move||error.get()}</p>
    <button type="button" class="ghost" on:click=move |_|ctx.go(crate::Page::Users)>"管理用户与 Key 身份"</button>
    </div></section>}
}
fn selected_price_option(channel:&str,model:&str,options:&[PriceOption],objects:&Value)->Option<PriceOption> {
    available_price_options(options,objects).into_iter().find(|o|o.source_id.to_string()==channel&&o.model==tokengo_shared::normalize_model_name(model))
}
fn price_row_kind(row:&Value)->&'static str {
    if row.get("input_micro_per_million").is_some(){"token"}else if row.get("micro_per_unit").is_some(){"unit"}else{"request"}
}
fn price_scheme(option:&PriceOption,manual:&str)->String {
    if matches!(manual,"token"|"unit"|"request")&&api::rows(&option.metadata,"allowed_pricing_kinds").iter().any(|k|k.as_str()==Some(manual)){return manual.into();}
    match option.metadata["pricing_kind"].as_str(){Some("token")=>"token".into(),Some("unit")=>"unit".into(),Some("request")=>"request".into(),Some("unknown") if matches!(manual,"token"|"unit"|"request")=>manual.into(),_=>String::new()}
}
fn pricing_scheme_label(kind:&str)->&str {match kind{"token"=>"按 Token 计价","unit"=>"按时长或数量计价","request"=>"按次计价",_=>"请选择计价方式"}}
fn task_label(task:&str)->&str {match task{"chat"=>"对话 / 推理","embeddings"=>"向量","rerank"=>"重排","asr"=>"语音识别（ASR）","tts"=>"语音合成（TTS）","audio"=>"音频","image"=>"图像","video"=>"视频","tools"=>"工具",_=>"类型未配置"}}
fn usage_reference(option: &PriceOption, field: &str) -> Option<i64> {
    option.metadata[field].as_i64()
}

fn model_price_payload(
    option: &PriceOption,
    manual_kind: &str,
    input: &str,
    output: &str,
    unit: &str,
) -> Result<(&'static str, &'static str, Value), String> {
    if option.metadata.is_null() {
        return Err("未能读取该模型的计价配置，请刷新后重试。".into());
    }
    let kind = price_scheme(option, manual_kind);
    let amount = policy_editor::optional_limit(input)?
        .ok_or_else(|| "请填写人民币价格，免费请填 0。".to_string())?;
    let mut body = json!({"channel_id": option.source_id, "model": option.model, "currency": "CNY"});
    match kind.as_str() {
        "token" => {
            let output = if usage_reference(option, "output_usage_reference") == Some(0) {
                0
            } else {
                policy_editor::optional_limit(output)?
                    .ok_or_else(|| "请填写输出价格，免费请填 0。".to_string())?
            };
            body["input_micro_per_million"] = json!(amount);
            body["output_micro_per_million"] = json!(output);
            Ok(("/api/pricing", "price", body))
        }
        "unit" => {
            if !api::rows(&option.metadata, "allowed_units").iter().any(|value| value.as_str() == Some(unit)) {
                return Err("请选择该模型支持的计价单位。".into());
            }
            body["unit"] = json!(unit);
            body["micro_per_unit"] = json!(amount);
            Ok(("/api/unit-pricing", "unit_price", body))
        }
        "request" => {
            body["micro_per_call"] = json!(amount);
            Ok(("/api/request-pricing", "request_price", body))
        }
        _ => Err("该模型尚未配置类型，请先选择计价方式。".into()),
    }
}

fn reference_origin(value: &Value) -> &'static str {
    match value.as_str() {
        Some("channel_model") => "渠道模型",
        Some("existing_price") => "沿用已有价格配置",
        Some("model_specification") => "模型规格",
        Some("non_generation_model") => "模型类型",
        Some("source_model_discovery" | "upstream_catalog") => "上游目录",
        Some("oauth_binding" | "source_protocols" | "model_context_override") => "来源配置",
        _ => "",
    }
}

#[component]
fn PriceUsageReferences(ctx: Ctx, rows: Vec<(String, Option<String>, &'static str)>) -> impl IntoView {
    view! {
        <div id="price-model-bounds" class="billing-selected-object" style="grid-column:1 / -1">
            <dl class="billing-price-references">{rows.into_iter().map(|(label, value, source)| view! {
                <dt>{label}</dt><dd>
                    {value.unwrap_or_else(|| "未配置".into())}
                    {(!source.is_empty()).then(|| view! { <span class="muted small">" · "{source}</span> })}
                </dd>
            }).collect_view()}</dl>
            <p class="muted small">"用量参考值用于费用估算，可在渠道模型中修改。"</p>
            <button type="button" class="ghost" on:click=move |_| {
                billing_dialog("price-dialog", false);
                ctx.go(crate::Page::Channels);
            }>"前往渠道模型配置"</button>
        </div>
    }
}

fn compatible_price_unit(row:&Value,option:&PriceOption)->bool {
    price_row_kind(row)!="unit"||api::rows(&option.metadata,"allowed_units").iter().any(|unit|unit==&row["unit"])
}
fn compatible_price_kind(row:&Value,options:&[PriceOption])->bool {
    if service_price(row){return true;}
    options.iter().find(|o|row["channel_id"].as_i64()==Some(o.source_id)&&row["model"].as_str()==Some(o.model.as_str())).is_none_or(|o|{let configured=o.metadata["pricing_kind"].as_str().unwrap_or("unknown");compatible_price_unit(row,o)&&(configured=="unknown"||price_row_kind(row)==configured||api::rows(&o.metadata,"allowed_pricing_kinds").iter().any(|k|k.as_str()==Some(price_row_kind(row))))})
}
fn model_price_endpoint(kind:&str)->&'static str {match kind{"unit"=>"/api/unit-pricing","request"=>"/api/request-pricing",_=>"/api/pricing"}}
async fn load_model_prices()->Result<Vec<Value>,crate::api::ApiErr> {
    let mut all=Vec::new();
    for (path,array) in [("/api/pricing?include_deleted=true","prices"),("/api/unit-pricing?include_deleted=true","unit_prices"),("/api/request-pricing?include_deleted=true","request_prices")] {
        let response=api::request("GET",path,None).await?;all.extend(api::rows(&response,array));
    }
    Ok(all)
}
fn price_visible(row:&Value,scope:&str,rows:&[Value])->bool {
    match scope {"all"=>true,"deleted"=>row["deleted"]==true,"history"=>row["deleted"]!=true&&(!price_is_current(row,rows)||row_currency(row)!="CNY"),_=>row["deleted"]!=true&&price_is_current(row,rows)&&row_currency(row)=="CNY"}
}
#[component]
fn ModelPricing(ctx:Ctx,objects:RwSignal<Value>)->impl IntoView {
    let sources=price_sources(ctx);let versions=RwSignal::new("current".to_string());let rows=RwSignal::new(Vec::<Value>::new());
    let busy=RwSignal::new(false);let error=RwSignal::new(String::new());let message=RwSignal::new(String::new());
    let edit_target=RwSignal::new(None::<(&'static str,i64)>);let pending_delete=RwSignal::new(None::<Value>);
    let channel = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let input = RwSignal::new(String::new());
    let output = RwSignal::new(String::new());
    let unit = RwSignal::new(String::new());
    let manual_kind = RwSignal::new(String::new());
    let selected = Memo::new(move |_| selected_price_option(&channel.get(), &model.get(), &sources.get(), &objects.get()));
    let scheme = Memo::new(move |_| selected.get().map(|option| price_scheme(&option, &manual_kind.get())).unwrap_or_default());
    let on_selection = Callback::new(move |()| {
        input.set(String::new());
        output.set(String::new());
        manual_kind.set(String::new());
        error.set(String::new());
        message.set(String::new());
        unit.set(selected.get_untracked().and_then(|option| {
            option.metadata["recommended_unit"].as_str().map(str::to_owned)
        }).unwrap_or_default());
    });
    let load=move |_| {if busy.get_untracked(){return;}busy.set(true);error.set(String::new());leptos::task::spawn_local(async move {
        let (result, metadata) = futures_util::join!(load_model_prices(), refresh_price_metadata(sources));
        match result {Ok(v)=>rows.set(v),Err(e)=>{ctx.note_err(&e);error.set(e.message);}}
        if let Err(e) = metadata {ctx.note_err(&e);error.set(format!("刷新用量参考值失败，{}", e.message));}
        busy.set(false);
    });};load(());
    let save=move|ev:leptos::ev::SubmitEvent| {
        ev.prevent_default();if busy.get_untracked(){return;}
        let Some(option)=selected_price_option(&channel.get_untracked(),&model.get_untracked(),&sources.get_untracked(),&objects.get_untracked())else{error.set("请选择当前有效渠道上已配置的模型。".into());return;};
        let (path,item,body)=match model_price_payload(&option,&manual_kind.get_untracked(),&input.get_untracked(),&output.get_untracked(),&unit.get_untracked()){Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        busy.set(true);error.set(String::new());message.set(String::new());
        let editing=edit_target.get_untracked();let save_path=editing.map(|(path,id)|format!("{path}/{id}")).unwrap_or_else(||path.to_owned());
        leptos::task::spawn_local(async move {match api::request(if editing.is_some(){"PUT"}else{"POST"},&save_path,Some(body)).await {
            Ok(v)=>{match load_model_prices().await {Ok(all)=>rows.set(all),Err(e)=>{rows.update(|all|all.insert(0,v[item].clone()));error.set(format!("价格已保存，刷新列表失败：{}",e.message));}}message.set(if editing.is_some(){"价格已修改，新版本用于后续调用；历史账目保留。"}else{"人民币价格版本已创建。历史账目保留。"}.into());edit_target.set(None);billing_dialog("price-dialog",false);},
            Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});
    };
    let edit_price=Callback::new(move|r:Value|{
        let kind=price_row_kind(&r);let legacy=row_currency(&r)!="CNY";
        channel.set(api::text(&r,"channel_id"));model.set(api::text(&r,"model"));on_selection.run(());
        let selected=selected_price_option(&channel.get_untracked(),&model.get_untracked(),&sources.get_untracked(),&objects.get_untracked());
        let target=selected.as_ref().map(|o|price_scheme(o,kind)).unwrap_or_else(||kind.into());let unit_valid=selected.as_ref().is_some_and(|o|compatible_price_unit(&r,o));manual_kind.set(kind.into());
        if target==kind&&unit_valid {
            input.set(if legacy{String::new()}else{r[match kind{"token"=>"input_micro_per_million","unit"=>"micro_per_unit",_=>"micro_per_call"}].as_i64().map(policy_editor::money_input).unwrap_or_default()});
            output.set(if legacy{String::new()}else{r["output_micro_per_million"].as_i64().map(policy_editor::money_input).unwrap_or_default()});
            if selected.as_ref().is_some_and(|o|api::rows(&o.metadata,"allowed_units").iter().any(|u|u==&r["unit"])){unit.set(api::text(&r,"unit"));}
        }
        edit_target.set((!legacy&&price_is_current(&r,&rows.get_untracked())&&target==kind&&unit_valid&&r["deleted"]!=true).then(||(model_price_endpoint(kind),r["id"].as_i64().unwrap_or(0))));
        message.set(if target!=kind{"模型类型已变更，请按当前计价方式重新填写。"}else if !unit_valid{"原计价单位不适用于当前模型，请确认新单位并重新填写。"}else if legacy{"请重新填写人民币价格，旧美元单价不会直接复制。"}else{"修改后生成新版本，后续调用使用新价格，历史账目保留。"}.into());
        billing_dialog("price-dialog",true);
    });
    let change_state=Callback::new(move|(r,enabled):(Value,bool)|{
        if busy.get_untracked(){return;}busy.set(true);error.set(String::new());message.set(String::new());
        let path=format!("{}/{}",model_price_endpoint(price_row_kind(&r)),api::text(&r,"id"));
        leptos::task::spawn_local(async move {
            match api::request("PATCH",&path,Some(json!({"enabled":enabled}))).await {
                Ok(_)=>{match load_model_prices().await {Ok(all)=>rows.set(all),Err(e)=>error.set(format!("状态已保存，刷新列表失败：{}",e.message))}message.set(if enabled{"价格已启用，后续调用按此价格计费。"}else{"价格已停用，后续调用不再使用此价格；历史账目保留。"}.into());},
                Err(e)=>{ctx.note_err(&e);error.set(e.message);}
            }busy.set(false);
        });
    });
    let remove_price=move |_|{
        if busy.get_untracked(){return;}let Some(r)=pending_delete.get_untracked()else{return;};busy.set(true);error.set(String::new());message.set(String::new());
        let path=format!("{}/{}",model_price_endpoint(price_row_kind(&r)),api::text(&r,"id"));
        leptos::task::spawn_local(async move {
            match api::request("DELETE",&path,None).await {
                Ok(_) => {
                    let (prices, metadata) = futures_util::join!(load_model_prices(), refresh_price_metadata(sources));
                    match prices {Ok(all)=>rows.set(all),Err(e)=>error.set(format!("价格已删除，刷新列表失败：{}",e.message))}
                    if let Err(e) = metadata {ctx.note_err(&e);error.set(format!("价格已删除，刷新用量参考值失败，{}", e.message));}
                    message.set("价格已删除，可在“已删除”范围查看；历史账目保留。".into());
                    pending_delete.set(None);
                    billing_dialog("price-delete-dialog", false);
                },
                Err(e)=>{ctx.note_err(&e);error.set(e.message);}
            }busy.set(false);
        });
    };
    view! {
        <section class="card"><header class="card-head"><div><h2>"模型价格"</h2><p class="muted small">"选择模型和来源渠道，自动使用对应计价方式与已配置参数。"</p></div><div class="account-actions"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{edit_target.set(None);channel.set(String::new());model.set(String::new());on_selection.run(());billing_dialog("price-dialog",true);}>"配置人民币价格"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load(())>"刷新价格"</button></div></header><div class="card-body">
            <div class="billing-toolbar"><div class="billing-field"><label for="price-versions">"价格范围"</label><select id="price-versions" prop:value=versions on:change=move|e|versions.set(event_target_value(&e))><option value="current">"当前配置（含停用）"</option><option value="history">"历史版本（含美元）"</option><option value="deleted">"已删除"</option><option value="all">"全部版本"</option></select></div><p class="muted small">"新价格以人民币结算，最多6位小数；0 表示明确免费。"</p></div>
            <div class="table-wrap"><table class="billing-price-table"><thead><tr><th>"模型 / 来源渠道"</th><th>"计价方式 / 价格"</th><th>"用量参考值"</th><th>"版本状态"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().filter(|r|{price_visible(r,&versions.get(),&rows.get())}).map(|r|{
                let source_name=price_source_name(&r,&objects.get());let model_label=price_model_name(&r);let current=price_effective(&r,&rows.get(),&objects.get())&&compatible_price_kind(&r,&sources.get());let legacy=row_currency(&r)!="CNY";let kind=price_row_kind(&r);
                let status=format!("{} · 版本 {}",if r["deleted"]==true {"已删除"}else if legacy {"历史美元"}else if r["enabled"]==false {"已停用"}else if current {"当前有效"}else{"历史 / 配置已变更"},api::text(&r,"id"));
                let amount=match kind {"token"=>format!("输入 {} / 输出 {} / 百万 Token",row_money(&r,"input_micro_per_million"),row_money(&r,"output_micro_per_million")),"unit"=>format!("{} / {}",row_money(&r,"micro_per_unit"),unit_label(r["unit"].as_str().unwrap_or(""))),_=>format!("{} / 次",row_money(&r,"micro_per_call"))};
                let cap=match kind {"token"=>format!("输入 {} / 总输出 {} Token",api::text(&r,"max_input_tokens"),api::text(&r,"max_output_tokens")),"unit"=>{let n=r["max_quantity_milli"].as_i64().map(quantity_milli_input).unwrap_or_else(||"未知".into());if duration_unit(r["unit"].as_str().unwrap_or("")){let task=sources.get().iter().find(|o|r["channel_id"].as_i64()==Some(o.source_id)&&r["model"].as_str()==Some(o.model.as_str())).map(|o|duration_subject(o.metadata["task_type"].as_str().unwrap_or(""))).unwrap_or("媒体");format!("{n} {task}秒")}else{format!("{n} {}",unit_label(r["unit"].as_str().unwrap_or("")))}},_=>"每次调用".into()};
                view!{<tr><td data-label="模型 / 来源"><strong class="billing-price-model">{model_label}</strong><small class="billing-price-source">{source_name}</small></td><td data-label="计价方式 / 价格">{amount}</td><td data-label="用量参考值">{cap}</td><td data-label="版本状态"><span class="tag billing-price-version">{status}</span></td><td data-label="操作">{if service_price(&r) {view!{<button type="button" class="ghost" on:click=move |_|ctx.go(crate::Page::Services)>"管理服务价格"</button>}.into_any()}else if r["deleted"]==true {view!{<span class="muted small">"已归档，历史账目可查询"</span>}.into_any()}else{
                    let edit_row=r.clone();let toggle_row=r.clone();let delete_row=r.clone();let latest=price_is_current(&r,&rows.get());let enabled=r["enabled"]!=false;
                    view!{<div class="billing-price-actions">
                        <button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|edit_price.run(edit_row.clone())>{if legacy{"配置人民币价格"}else if latest{"修改"}else{"复制为新版本"}}</button>
                        {if latest&&!legacy {view!{<button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|change_state.run((toggle_row.clone(),!enabled))>{if enabled{"停用"}else{"启用"}}</button>}.into_any()}else{().into_any()}}
                        <button type="button" class="ghost danger" disabled=move||busy.get() on:click=move |_|{pending_delete.set(Some(delete_row.clone()));error.set(String::new());billing_dialog("price-delete-dialog",true);}>"删除"</button>
                    </div>}.into_any()
                }}</td></tr>}
            }).collect_view()}</tbody></table></div>
            <p role="status">{move||if busy.get(){"正在加载价格…"}else if rows.get().is_empty(){"暂无模型价格，请先选择模型配置。"}else{""}}</p><p role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p>
            <BillingDialog id="price-dialog" title="配置人民币价格" busy=busy><form on:submit=save><fieldset disabled=move||busy.get()><div class="form-grid">
                {move||if edit_target.get().is_some(){view!{<div class="billing-selected-object" style="grid-column:1 / -1"><strong>{model.get()}</strong><p class="muted small">{selected.get().map(|o|o.channel).unwrap_or_default()}</p><p class="muted small">"修改当前模型和来源的价格；保存后生成新版本。"</p></div>}.into_any()}else{view!{<PriceModelSource prefix="price" channel=channel model=model options=sources objects=objects on_selection=on_selection/>}.into_any()}}
                {move||selected.get().map(|option|{
                    let metadata = &option.metadata;
                    let kind = scheme.get();
                    let input_reference = usage_reference(&option, "input_usage_reference");
                    let output_reference = usage_reference(&option, "output_usage_reference");
                    let subject = duration_subject(metadata["task_type"].as_str().unwrap_or(""));
                    let selected_unit = unit.get();
                    view!{
                        <div id="price-scheme-summary" class="billing-selected-object" style="grid-column:1 / -1"><p><strong>{format!("{} · {}",task_label(metadata["task_type"].as_str().unwrap_or("unknown")),pricing_scheme_label(&kind))}</strong></p>{if metadata.is_null(){view!{<p role="alert">"未能读取模型计价配置，请刷新后重试。"</p>}.into_any()}else{().into_any()}}</div>
                        {if metadata["pricing_kind"]=="unknown"{view!{<label for="price-kind">"计价方式（模型类型未配置）"</label><select id="price-kind" required prop:value=manual_kind on:change=move|e|{manual_kind.set(event_target_value(&e));input.set(String::new());output.set(String::new());unit.set(String::new());}><option value="">"请选择供应方实际计价方式"</option><option value="token">"按 Token"</option><option value="unit">"按时长 / 数量"</option><option value="request">"按次"</option></select>}.into_any()}else{().into_any()}}
                        {if kind=="unit"{let units=api::rows(&metadata,"allowed_units");view!{<label for="price-unit">"计价单位"</label><select id="price-unit" required prop:value=unit on:change=move|e|{unit.set(event_target_value(&e));input.set(String::new());}><option value="">"请选择供应方实际使用的单位"</option>{units.into_iter().filter_map(|u|u.as_str().map(str::to_string)).map(|u|{let label=if duration_unit(&u){format!("{subject}{}",unit_label(&u))}else{unit_label(&u).to_string()};view!{<option value=u>{label}</option>}}).collect_view()}</select>}.into_any()}else{().into_any()}}
                        {if !kind.is_empty(){let price_label=match kind.as_str(){"token"=>"输入价格（元 / 百万 Token）".to_string(),"unit"=>format!("价格（元 / {}）",if selected_unit.is_empty(){"所选单位"}else{unit_label(&selected_unit)}),_=>"价格（元 / 次）".into()};view!{<label for="price-in">{price_label}</label><input id="price-in" required type="text" inputmode="decimal" prop:value=input aria-describedby="price-error" on:input=move|e|input.set(event_target_value(&e))/ >}.into_any()}else{().into_any()}}
                        {if kind=="token"{view!{
                            {if output_reference==Some(0){view!{<p class="muted small" style="grid-column:1 / -1">"该模型不生成输出 Token，仅配置输入价格；输出价格为 0。"</p>}.into_any()}else{view!{<label for="price-out">"输出价格（元 / 百万 Token）"</label><input id="price-out" required type="text" inputmode="decimal" prop:value=output aria-describedby="price-error" on:input=move|e|output.set(event_target_value(&e))/>}.into_any()}}
                            <PriceUsageReferences ctx=ctx rows=vec![
                                ("输入用量参考值".into(), input_reference.map(|value| format!("{value} Token")), reference_origin(&metadata["input_bound_source"])),
                                ("输出用量参考值".into(), output_reference.map(|value| format!("{value} Token")), reference_origin(&metadata["output_bound_source"])),
                            ]/>
                        }.into_any()}else if kind=="unit"{view!{
                            {(!selected_unit.is_empty()).then(|| {
                                let (name, label) = if duration_unit(&selected_unit) {
                                    (format!("{subject}时长参考值"), "秒")
                                } else {
                                    ("数量参考值".into(), unit_label(&selected_unit))
                                };
                                let value = metadata["quantity_usage_references"][selected_unit.as_str()]
                                    .as_i64().map(|value| format!("{} {label}", quantity_milli_input(value)));
                                let source = reference_origin(&metadata["quantity_reference_sources"][selected_unit.as_str()]);
                                view! { <PriceUsageReferences ctx=ctx rows=vec![(name,value,source)]/> }
                            })}
                            <p class="muted small" style="grid-column:1 / -1">{if metadata["task_type"]=="asr"{"语音识别按音频实际时长结算，使用秒、分钟或小时单价；不使用 Token 或接口响应耗时。"}else if metadata["task_type"]=="tts"{"请按供应方选择字符或 UTF-8 字节单价，二者不可混用。"}else if metadata["task_type"]=="video"{"请按供应方选择每段视频或视频时长单价；时长参考值始终填写视频秒数。"}else{"计价单位须与供应方实际账单一致。"}}</p>
                            {if metadata["automatic_settlement"]==false{view!{<p class="muted small" style="grid-column:1 / -1">"当前可保存此类价格；对应调用接口尚未接入自动计量和结算。"</p>}.into_any()}else{().into_any()}}
                        }.into_any()}else{().into_any()}}
                    }
                })}
            </div><p id="price-error" role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><div class="module-dialog-actions"><button type="button" class="ghost" on:click=move |_|billing_dialog("price-dialog",false)>"取消"</button><button type="submit" class="primary" disabled=move||busy.get()||scheme.get().is_empty()>{move||if edit_target.get().is_some(){"保存修改"}else{"创建人民币价格版本"}}</button></div></fieldset></form></BillingDialog>
            <BillingDialog id="price-delete-dialog" title="删除价格配置" busy=busy>
                <p>{move||pending_delete.get().map(|r|format!("删除 {} · 版本 {}？",price_model_name(&r),api::text(&r,"id"))).unwrap_or_default()}</p>
                <p class="muted small">"删除后不再用于后续调用，旧版本不会自动恢复。历史账目及原价格快照保留。"</p>
                <p role="alert">{move||error.get()}</p>
                <div class="module-dialog-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|billing_dialog("price-delete-dialog",false)>"取消"</button><button type="button" class="primary danger" disabled=move||busy.get() on:click=remove_price>"确认删除"</button></div>
            </BillingDialog>
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

fn db_diagnostic(row:&Value)->bool {row["auth_snapshot"]["kind"]=="admin_diagnostic" || row["origin"]["kind"]=="admin_diagnostic"}
fn reporting_row_money(row:&Value,field:&str)->String {
    let display=format!("display_{field}");
    if let Some(n)=row[&display].as_i64(){return format!("{} 元",policy_editor::money_input(n));}
    if row[field].is_null(){return "未知 / 未定价".into();}
    if row_currency(row)=="CNY"{return format!("{} 元",policy_editor::money_input(row[field].as_i64().unwrap_or(0)));}
    "汇率暂不可用".into()
}
fn reporting_page_summary(rows:&[Value])->Vec<String> {
    let reporting=rows.iter().cloned().map(|mut row|{for(raw,display)in [("debit_micro","display_debit_micro"),("observed_cost_micro","display_observed_cost_micro"),("reserved_micro","display_reserved_micro")]{row[raw]=row[display].clone();}row["currency"]=json!("CNY");row}).collect::<Vec<_>>();
    api::billing_page_summary(&reporting)
}
fn reporting_hold(row:&Value)->String {if row["status"]=="reserved"&&row["settled_ms"].is_null(){reporting_row_money(row,"reserved_micro")}else{"0 元".into()}}
const DEFAULT_USAGE_STATE: &str = "active";
const DEFAULT_USAGE_LIMIT: &str = "50";

fn default_usage_query() -> String {
    format!("limit={DEFAULT_USAGE_LIMIT}&state={DEFAULT_USAGE_STATE}&currency=all{}", default_period_query())
}

fn usage_query_limit(query:&str)->String {
    query.split('&').find_map(|part|part.strip_prefix("limit=")).unwrap_or(DEFAULT_USAGE_LIMIT).to_string()
}
fn usage_query_with_limit(query:&str, limit:u32)->String {
    let filters=query.split('&').filter(|p|!p.starts_with("limit=")&&!p.starts_with("before_id=")&&!p.is_empty()).collect::<Vec<_>>().join("&");
    format!("limit={limit}&{filters}")
}

/// Cursor batches can overlap while the ledger changes; retain order and one row per identity.
fn append_usage_rows(current:&mut Vec<Value>,incoming:Vec<Value>) {
    let mut ids=current.iter().filter_map(|r|r["id"].as_i64()).collect::<std::collections::HashSet<_>>();
    current.extend(incoming.into_iter().filter(|r|r["id"].as_i64().is_none_or(|id|ids.insert(id))));
}

#[component]
fn Usage(ctx: Ctx,objects:RwSignal<Value>) -> impl IntoView {
    let tasks=crate::page_tasks::PageTasks::new();
    let period=RwSignal::new("today".to_string());let start_time=RwSignal::new(String::new());let end_time=RwSignal::new(String::new());let start=RwSignal::new(String::new());let end=RwSignal::new(String::new());let summary=RwSignal::new(Value::Null);let analytics=RwSignal::new(Value::Null);let advanced=RwSignal::new(false);let applied_time=RwSignal::new(String::new());
    let snapshot=RwSignal::new(String::new());let user=RwSignal::new(String::new());let key=RwSignal::new(String::new());let channel=RwSignal::new(String::new());let model=RwSignal::new(String::new());let state=RwSignal::new(DEFAULT_USAGE_STATE.to_string());let count=RwSignal::new(DEFAULT_USAGE_LIMIT.to_string());
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
                for (group,selection) in [("users",user),("keys",key),("source_groups",channel)] {
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
    let rows=RwSignal::new(Vec::<Value>::new());let cursor=RwSignal::new(None::<i64>);let busy=RwSignal::new(false);let error=RwSignal::new(String::new());let message=RwSignal::new(String::new());let generation=RwSignal::new(0u64);let feed_failed=RwSignal::new(false);let active_query=RwSignal::new(default_usage_query());
    let statistics=RwSignal::new(crate::account_api::UsageStatisticsRequest::default());
    let statistics_busy=RwSignal::new(false);let statistics_error=RwSignal::new(String::new());
    let fetch=move|base:String,before:Option<i64>,refresh_statistics:bool|{
        if busy.get_untracked(){return;}
        let mut report=None;statistics.update(|s|report=s.begin(&base,refresh_statistics));
        if report.is_some(){summary.set(Value::Null);analytics.set(Value::Null);statistics_busy.set(true);statistics_error.set(String::new());}
        generation.update(|g|*g+=1);let current=generation.get_untracked();
        busy.set(true);feed_failed.set(false);error.set(String::new());message.set(if before.is_some(){"正在加载更早记录…".into()}else{"正在查询…".into()});
        let query=if let Some(id)=before{format!("{base}&before_id={id}")}else{base.clone()};
        tasks.spawn(async move{
            let result=api::request("GET",&format!("/api/usage?{query}"),None).await;
            if generation.get_untracked()!=current{return;}
            match result{
                Ok(v)=>{
                    let found=api::rows(&v,"entries");let empty=found.is_empty();
                    let next=v["next_before_id"].as_i64().filter(|id|!empty&&before.is_none_or(|old|*id<old));
                    cursor.set(next);
                    if before.is_some(){rows.update(|items|append_usage_rows(items,found));}
                    else{rows.set(found);count.set(usage_query_limit(&base));applied_time.set(applied_period(&base));active_query.set(base);}
                    message.set(format!("已加载 {} 条记录 · 更新于 {}",rows.with_untracked(|items|items.len()),updated_time()));
                    // Commit the lightweight page before starting the independent full-range scan.
                    if let Some((generation,filter))=report {
                        tasks.spawn(async move{
                            let result=api::request("GET",&format!("/api/usage/analytics?{filter}"),None).await;
                            if !statistics.with_untracked(|s|s.accepts(generation)){return;}
                            match result {Ok(v)=>{summary.set(v["summary"].clone());analytics.set(v["analytics"].clone());},Err(e)=>{ctx.note_err(&e);statistics_error.set(e.message);statistics.update(|s|s.failed(generation));}}
                            statistics_busy.set(false);
                        });
                    }
                },
                Err(e)=>{ctx.note_err(&e);feed_failed.set(true);count.set(usage_query_limit(&active_query.get_untracked()));error.set(e.message);message.set("查询失败，保留上次列表。".into());if let Some((generation,_))=report{statistics.update(|s|s.not_started(generation));statistics_busy.set(false);statistics_error.set("列表查询失败，统计尚未读取。".into());}}
            }
            busy.set(false);
        });
    };
    fetch(default_usage_query(),None,false);
    let run_search=move||{if busy.get_untracked(){return;}let n=match api::integer(&count.get_untracked(),"每批条数",true){Ok(v)if v<=200=>v,_=>{error.set("每批条数必须在1至200之间".into());return;}};let mut q=format!("limit={n}&state={}&currency=all",state.get_untracked());for (value,name) in [(user,"user_id"),(key,"key_id"),(channel,"source_group_id")] {if !value.get_untracked().is_empty(){q.push_str(&format!("&{name}={}",api::encode(&value.get_untracked())));}}if !model.get_untracked().trim().is_empty(){q.push_str(&format!("&model={}",api::encode(model.get_untracked().trim())));}match period_query(&period.get_untracked(),&start.get_untracked(),&end.get_untracked(),&start_time.get_untracked(),&end_time.get_untracked()){Ok(time)=>q.push_str(&time),Err(e)=>{error.set(e);return;}}fetch(q,None,false);};
    let search=move|ev:leptos::ev::SubmitEvent|{ev.prevent_default();run_search();};
    view!{<section class="card billing-usage"><header class="card-head"><div><h2>"用量看板"</h2><p class="muted small">"查看区间消耗、用量趋势与调用明细。"</p></div><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|fetch(active_query.get_untracked(),None,true)>"刷新"</button></header><div class="card-body">
    <form class="usage-filter-bar" on:submit=search><fieldset disabled=move||busy.get()><div class="usage-filter-row">
    <UsagePeriod period=period start=start end=end start_time=start_time end_time=end_time compact=true/>
    <ObjectSelect id="usage-user" label="用户" group="users" value=user objects=objects active_only=current_only/>
    <ObjectSelect id="usage-key" label="API Key" group="keys" value=key objects=objects active_only=current_only owner=user/>
    <div class="billing-field"><label for="usage-model">"模型"</label><input id="usage-model" placeholder="全部模型" prop:value=model on:input=move|e|model.set(event_target_value(&e))/></div>
    <div class="usage-inline-actions"><button type="submit" class="primary">{move||if busy.get(){"查询中…"}else{"查询"}}</button><button type="button" class="ghost" on:click=move |_|{period.set("today".into());start.set(String::new());end.set(String::new());start_time.set(String::new());end_time.set(String::new());user.set(String::new());key.set(String::new());channel.set(String::new());model.set(String::new());state.set(DEFAULT_USAGE_STATE.into());count.set(DEFAULT_USAGE_LIMIT.into());fetch(default_usage_query(),None,false);}>"重置"</button><button type="button" class="ghost" aria-controls="usage-advanced" aria-expanded=move||advanced.get().to_string() on:click=move |_|advanced.update(|open|*open=!*open)>"更多筛选"</button></div>
    </div><div id="usage-advanced" class="usage-advanced" hidden=move||!advanced.get()>

    <ObjectSelect id="usage-channel" label="渠道" group="source_groups" value=channel objects=objects active_only=current_only/>
    <div class="billing-field"><label for="usage-state">"对象状态"</label><select id="usage-state" prop:value=state on:change=change_state><option value="active">"当前有效"</option><option value="historical">"历史对象"</option><option value="all">"全部（含历史）"</option></select></div>
    <span class="muted small">"模型精确匹配；停用或删除的用户、Key 和上游可切换历史对象查询。"</span>
    </div></fieldset></form>
    <p id="usage-error" role="alert">{move||error.get()}</p><p class="billing-query-status" role="status" aria-live="polite">{move||message.get()}</p>
    <p class="usage-applied-range muted small">{move||format!("统计区间：{} · 本地时区",applied_time.get())}</p>
    <p class="muted small usage-reporting-note">"用量覆盖全部币种，金额统一折合人民币；历史美元按参考汇率自动换算。"</p>
    <Show when=move||statistics_busy.get()><p role="status" aria-live="polite">"正在统计所选范围，调用明细可先查看…"</p></Show>
    <Show when=move||!statistics_error.get().is_empty()><p role="alert">{move||statistics_error.get()}</p></Show>
    <Show when=move||!statistics_busy.get()&&statistics_error.get().is_empty()&&summary.get().is_object()><UsageDashboard summary=summary analytics=analytics/></Show>
    <header id="billing-ledger-start" class="usage-ledger-heading scroll-feed-start"><div><h3>"调用明细"</h3><span class="muted small">"所选区间内的逐次账目"</span></div><div class="usage-page-size"><label for="usage-limit">"每批"</label><select id="usage-limit" disabled=move||busy.get() prop:value=count on:change=move |e|{let n=event_target_value(&e).parse::<u32>().unwrap_or(50);fetch(usage_query_with_limit(&active_query.get_untracked(),n),None,false);} ><option value="10">"10 条"</option><option value="50">"50 条"</option><option value="100">"100 条"</option><option value="200">"200 条"</option></select></div></header>
    <div class="table-wrap"><table><thead><tr><th>"用户 / Key"</th><th>"上游接入 / 模型"</th><th>"结果"</th><th>"用量"</th><th>"用户扣款（元）"</th><th>"上游成本（元）"</th><th>"当前占用（元）"</th><th>"详情"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|r|{
        let (source,who,_raw_debit)=ledger_identity(&r);let debit=if db_diagnostic(&r){"不扣用户费用".to_string()}else{reporting_row_money(&r,"debit_micro")};let model_name=api::text(&r,"model");let state_text=usage_status(&r);let basis=usage_basis(&r);let units=if r["billing_unit"]=="request"{format!("{} 次",api::billed_requests(&r))}else if r["billing_unit"]=="token"{format!("输入 {} / 输出 {} token",api::text(&r,"input_tokens"),api::text(&r,"output_tokens"))}else{crate::fmt::quantity(r["billing_unit"].as_str().unwrap_or(""),r["quantity_milli"].as_i64())};let cost=reporting_row_money(&r,"observed_cost_micro");let reserved=reporting_hold(&r);let historical=r["record_state"]=="historical";let detail=api::pretty(&r);
        view!{<tr><td data-label="用户 / Key"><strong>{who}</strong>{if historical{Some(view!{<small class="billing-history">"关联历史对象"</small>})}else{None}}</td><td data-label="上游接入 / 模型">{source}<small>{model_name}</small></td><td data-label="结果">{state_text}<small>{basis}</small></td><td data-label="用量">{units}</td><td data-label="用户扣款">{debit}</td><td data-label="上游成本">{cost}</td><td data-label="当前占用">{reserved}</td><td data-label="详情"><button type="button" class="ghost" on:click=move |_|{snapshot.set(detail.clone());billing_dialog("ledger-snapshot-dialog",true);}>"查看详情"</button></td></tr>}
    }).collect_view()}</tbody></table></div>
    {move||if !busy.get()&&rows.get().is_empty(){Some(view!{<p class="muted">"当前筛选没有账目，可调整条件或重置筛选。"</p>})}else{None}}
    <super::scroll_feed::ScrollFeed busy=Signal::derive(move||busy.get()) has_more=Signal::derive(move||cursor.get().is_some()) failed=Signal::derive(move||feed_failed.get()) on_more=Callback::new(move|()|fetch(active_query.get_untracked(),cursor.get_untracked(),false)) on_latest=Callback::new(move|()|fetch(active_query.get_untracked(),None,false)) label="账目" anchor="billing-ledger-start"/>
    <p class="muted small">"向下滚动加载更早账目；修改筛选后点击“查询”，重新从最新记录开始。"</p>
    <details class="billing-help"><summary>"如何理解账目与费用"</summary><p>"费用按调用用量与已配置价格记录；成本为上游报告或估算，不能直接当作供应商对账结果。本地不再按预算或冻结标记限制调用；历史预留记录保留在详情中。美元金额按参考汇率自动折合人民币展示，原币种仅保留在账目详情中；未知不等于免费。"</p><p>"系统诊断单独标识，不扣用户费用。历史对象包括停用、删除、撤销或到期的用户、Key、上游接入。"</p>{move||reporting_page_summary(&rows.get()).into_iter().map(|line|view!{<p class="muted small">{line}</p>}).collect_view()}</details>
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
 #[test] fn unit_quantity_display_preserves_audio_milliseconds(){assert_eq!(quantity_milli_input(61234),"61.234");}
 #[test] fn legacy_money_is_never_relabelled_as_cny(){let old=json!({"debit_micro":1230000});assert_eq!(row_money(&old,"debit_micro"),"1.23 USD");let new=json!({"currency":"CNY","debit_micro":1230000});assert_eq!(row_money(&new,"debit_micro"),"1.23 元");}

}

#[cfg(test)]
mod usage_paging_tests {
    use super::*;
    #[test]
    fn overlapping_cursor_batches_keep_one_ledger_row_and_stable_order() {
        let mut rows=vec![json!({"id":3,"input_tokens":30}),json!({"id":2,"input_tokens":20})];
        append_usage_rows(&mut rows,vec![json!({"id":2,"input_tokens":99}),json!({"id":1,"input_tokens":10}),json!({"id":1,"input_tokens":10})]);
        assert_eq!(rows.iter().map(|r|r["id"].as_i64().unwrap()).collect::<Vec<_>>(),vec![3,2,1]);
        assert_eq!(rows[1]["input_tokens"],20);
    }
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

#[cfg(test)]mod reporting_display_tests {
 use super::*;
 #[test]fn usage_rows_use_converted_cny_without_relabelling_original_usd(){let row=json!({"currency":"USD","debit_micro":1000000,"display_debit_micro":7000000});assert_eq!(reporting_row_money(&row,"debit_micro"),"7 元");assert_eq!(row["debit_micro"],1000000);assert_eq!(reporting_row_money(&json!({"currency":"USD","debit_micro":1000000,"display_debit_micro":null}),"debit_micro"),"汇率暂不可用");}
 #[test]fn default_usage_does_not_require_currency_choice(){assert_eq!(DEFAULT_USAGE_STATE,"active");let q=usage_query_with_limit("limit=50&state=active&currency=all&source_group_id=credential%3A14&start_at=1&end_at=2",100);assert!(q.contains("currency=all")&&q.contains("source_group_id=credential%3A14"));}
}

#[cfg(test)]
mod automatic_price_form_tests {
    use super::*;
    fn option(metadata:Value)->PriceOption {
        let rows=vec![json!({"id":5,"enabled":true,"models":[{"model":"test-model","upstream":"upstream-model","enabled":true}]})];
        let mut result=price_options_from_channels(&rows,&Default::default()).remove(0);result.metadata=metadata;result
    }
    #[test]
    fn token_price_body_contains_only_rates_and_identity() {
        let p = option(json!({"pricing_kind":"token","input_usage_reference":128000,"output_usage_reference":8192}));
        let (path, _, body) = model_price_payload(&p, "", "1.5", "6", "").unwrap();
        assert_eq!(path, "/api/pricing");
        assert_eq!(body, json!({"channel_id":5,"model":"test model","currency":"CNY",
            "input_micro_per_million":1500000,"output_micro_per_million":6000000}));
    }

    #[test]
    fn input_only_models_submit_zero_output_rate_without_a_usage_reference() {
        let p = option(json!({"task_type":"embeddings","pricing_kind":"token","input_usage_reference":8192,"output_usage_reference":0}));
        let (_, _, body) = model_price_payload(&p, "", "0.2", "", "").unwrap();
        assert_eq!(body["output_micro_per_million"], 0);
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn existing_valid_by_call_scheme_can_be_edited_while_new_defaults_to_tokens() {
        let p = option(json!({"task_type":"chat","pricing_kind":"token","allowed_pricing_kinds":["token","request"]}));
        assert_eq!(price_scheme(&p, ""), "token");
        assert_eq!(model_price_payload(&p, "request", "0.01", "", "").unwrap().0, "/api/request-pricing");
        let row = json!({"channel_id":5,"model":p.model,"micro_per_call":10000});
        assert!(compatible_price_kind(&row, &[p]));
    }

    #[test]
    fn unit_price_body_contains_rate_and_selected_unit_without_quantity() {
        let p = option(json!({"task_type":"asr","pricing_kind":"unit","allowed_units":["second","minute","hour"],
            "quantity_usage_references":{"minute":61234}}));
        let (path, _, body) = model_price_payload(&p, "", "0.12", "", "minute").unwrap();
        assert_eq!(path, "/api/unit-pricing");
        assert_eq!(body, json!({"channel_id":5,"model":"test model","currency":"CNY",
            "unit":"minute","micro_per_unit":120000}));
        assert!(model_price_payload(&p, "", "0.12", "", "character").is_err());
    }

    #[test]
    fn unknown_models_require_explicit_scheme_and_missing_metadata_cannot_submit() {
        let p = option(json!({"task_type":"unknown","pricing_kind":"unknown","allowed_units":["image"],"allowed_pricing_kinds":["token","unit","request"]}));
        assert!(model_price_payload(&p, "", "1", "", "").is_err());
        assert_eq!(model_price_payload(&p, "request", "1", "", "").unwrap().0, "/api/request-pricing");
        assert!(model_price_payload(&option(Value::Null), "token", "1", "2", "").is_err());
    }

    #[test]
    fn obsolete_unit_price_is_historical_and_cannot_seed_a_new_unit_price() {
        let p=option(json!({"task_type":"asr","pricing_kind":"unit","allowed_pricing_kinds":["unit"],"allowed_units":["second","minute","hour"],"recommended_unit":"second"}));
        let old=json!({"id":1,"channel_id":5,"model":p.model,"currency":"CNY","micro_per_unit":5000,"max_quantity_milli":100000,"unit":"character"});
        assert_eq!(price_scheme(&p,price_row_kind(&old)),"unit");
        assert!(!compatible_price_unit(&old,&p));assert!(!compatible_price_kind(&old,&[p.clone()]));
        let mut valid=old.clone();valid["unit"]=json!("minute");
        assert!(compatible_price_unit(&valid,&p));assert!(compatible_price_kind(&valid,&[p]));
    }
    #[test]
    fn price_versions_in_separate_storage_tables_do_not_shadow_each_other() {
        let token=json!({"id":1,"channel_id":5,"model":"test-model","currency":"CNY","input_micro_per_million":1});
        let unit=json!({"id":10,"channel_id":5,"model":"test-model","currency":"CNY","micro_per_unit":1,"unit":"second"});
        assert!(price_is_current(&token,&[token.clone(),unit.clone()]));assert!(price_is_current(&unit,&[token,unit.clone()]));
    }
}

#[cfg(test)]
mod price_lifecycle_tests {
    use super::*;
    #[test]
    fn disabled_or_deleted_latest_price_is_not_effective_and_does_not_revive_history() {
        let objects=json!({"channels":[{"id":1,"state":"active"}]});
        let old=json!({"id":1,"channel_id":1,"model":"test","currency":"CNY","input_micro_per_million":100,"enabled":true,"is_latest":false});
        let mut latest=json!({"id":2,"channel_id":1,"model":"test","currency":"CNY","input_micro_per_million":200,"enabled":false,"is_latest":true});
        assert!(!price_effective(&latest,&[old.clone(),latest.clone()],&objects));
        assert!(!price_effective(&old,&[old.clone(),latest.clone()],&objects));
        latest["enabled"]=json!(true);latest["deleted"]=json!(true);
        assert!(!price_effective(&latest,&[old,latest.clone()],&objects));
    }
    #[test]
    fn backend_latest_identity_prevents_hidden_archive_reviving_an_older_price() {
        let old=json!({"id":1,"channel_id":1,"model":"test","currency":"CNY","input_micro_per_million":100,"is_latest":false});
        assert!(!price_is_current(&old,std::slice::from_ref(&old)));
    }
}
