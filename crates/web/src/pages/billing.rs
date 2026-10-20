use super::PageHead;
use crate::{admin_api as api, Ctx};
use leptos::prelude::*;
use serde_json::{json, Value};
use wasm_bindgen::JsCast;

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

#[component]
pub fn Billing(ctx: Ctx) -> impl IntoView {
    view! {<div class="page admin-page"><PageHead title="账务" desc="管理用户和密钥额度、模型价格，并查询每次调用的扣费记录。"/><Budgets ctx=ctx/><Pricing ctx=ctx/><RequestPricing ctx=ctx/><Usage ctx=ctx/></div>}
}
#[component]
fn Budgets(ctx: Ctx) -> impl IntoView {
    let scope = RwSignal::new("user".to_string());
    let id = RwSignal::new(String::new());
    let limit = RwSignal::new(String::new());
    let rows = RwSignal::new(Vec::<Value>::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let load = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        message.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("GET", "/api/budgets", None).await {
                Ok(v) => {
                    rows.set(api::rows(&v, "budgets"));
                    message.set("预算已刷新".into());
                }
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
        let i = match api::integer(&id.get_untracked(), "对象 ID", true) {
            Ok(v) => v,
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let l = if limit.get_untracked().trim().is_empty() {
            None
        } else {
            match api::integer(&limit.get_untracked(), "预算", false) {
                Ok(v) => Some(v),
                Err(e) => {
                    error.set(e);
                    return;
                }
            }
        };
        let s = scope.get_untracked();
        error.set(String::new());
        message.set(String::new());
        busy.set(true);
        leptos::task::spawn_local(async move {
            match api::request(
                "PUT",
                &format!("/api/budgets?scope={s}&scope_id={i}"),
                Some(json!({"limit_micro":l})),
            )
            .await
            {
                Ok(v) => {
                    rows.update(|all| {
                        all.retain(|r| !(r["scope"] == s && r["scope_id"] == i));
                        all.extend(api::rows(&v, "budgets"));
                    });
                    message.set("预算已保存".into());
                    billing_dialog("budget-dialog", false);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<section class="card"><header class="card-head"><h2>"用户 / 密钥预算"</h2><div class="account-actions"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{scope.set("user".into());id.set(String::new());limit.set(String::new());error.set(String::new());message.set(String::new());billing_dialog("budget-dialog",true);}>"新建预算"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load(())>"刷新预算"</button></div></header><div class="card-body">
    <p class="muted small">"用户与各级密钥预算同时约束请求。留空表示无限额；0 表示无可用付费额度。冻结状态独立于限额，当前接口不提供解冻操作。列表仅展示已建立的预算记录。"</p>
    <div class="table-wrap"><table><thead><tr><th>"对象"</th><th>"限额 microUSD"</th><th>"已扣 / 预占 microUSD"</th><th>"状态"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|r|{let who=format!("{} #{}",api::text(&r,"scope"),api::text(&r,"scope_id"));let l=if r["limit_micro"].is_null(){"无限额".into()}else{api::text(&r,"limit_micro")};let amount=format!("{} / {}",api::text(&r,"spent_micro"),api::text(&r,"reserved_micro"));let state=if r["frozen"]==true{"冻结"}else{"可用（仍受其他预算约束）"};view!{<tr><td data-label="对象">{who}</td><td data-label="限额">{l}</td><td data-label="已扣 / 预占">{amount}</td><td data-label="状态">{state}</td><td data-label="操作"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{scope.set(api::text(&r,"scope"));id.set(api::text(&r,"scope_id"));limit.set(r["limit_micro"].as_i64().map(|n|n.to_string()).unwrap_or_default());error.set(String::new());message.set(String::new());billing_dialog("budget-dialog",true);}>"编辑预算"</button></td></tr>}}).collect_view()}</tbody></table></div>
    <p role="status">{move||if busy.get(){"正在请求预算…"}else if rows.get().is_empty(){"暂无预算记录，点击“新建预算”设置。"}else{""}}</p>
    <p role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><BillingDialog id="budget-dialog" title="设置预算" busy=busy><form on:submit=save ><fieldset disabled=move||busy.get()><div class="form-grid"><label for="budget-scope">"预算对象"</label><select id="budget-scope" prop:value=scope on:change=move|e|scope.set(event_target_value(&e))><option value="user">"用户"</option><option value="key">"API Key"</option></select>
    <label for="budget-id">"对象 ID"</label><input id="budget-id" required type="text" inputmode="numeric" prop:value=id aria-describedby="budget-error" on:input=move|e|id.set(event_target_value(&e))/>
    <label for="budget-limit">"总限额（整数 microUSD）"</label><input id="budget-limit" type="text" inputmode="numeric" prop:value=limit aria-describedby="budget-error" placeholder="留空 = 无限额" on:input=move|e|limit.set(event_target_value(&e))/></div>
    <p id="budget-error" role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><div class="module-dialog-actions"><button type="button" class="ghost" on:click=move |_|billing_dialog("budget-dialog",false)>"取消"</button><button class="primary" type="submit">"保存预算"</button></div></fieldset></form></BillingDialog></div></section>}
}
#[component]
fn Pricing(ctx: Ctx) -> impl IntoView {
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
        let values = [
            (&channel, "渠道 ID", true),
            (&input, "输入价格", false),
            (&output, "输出价格", false),
            (&max_in, "输入 token 上限", false),
            (&max_out, "输出 token 上限", false),
        ]
        .into_iter()
        .map(|(s, l, p)| api::integer(&s.get_untracked(), l, p))
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
    view! {<section class="card"><header class="card-head"><h2>"价格版本"</h2><div class="account-actions"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{channel.set(String::new());model.set(String::new());input.set("0".into());output.set("0".into());max_in.set(String::new());max_out.set(String::new());error.set(String::new());message.set(String::new());billing_dialog("price-dialog",true);}>"新建价格版本"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load(())>"刷新价格"</button></div></header><div class="card-body">
    <p class="muted small">"价格单位：每百万 token 的整数 microUSD。按渠道 ID + 逻辑模型匹配，最新版本生效；缺少价格表示未知成本，不等于免费。0 / 0 价格表示明确零 token 费用。上下限用于请求预占与安全封顶，不代表上游实测上下文。最多展示最近 1000 个版本。"</p>
    <div class="table-wrap"><table><thead><tr><th>"版本 / 渠道 / 模型"</th><th>"输入 / 输出价格"</th><th>"输入 / 输出上限"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|r|{let label=format!("v{} / #{} / {}",api::text(&r,"id"),api::text(&r,"channel_id"),api::text(&r,"model"));let price=format!("{} / {}",api::text(&r,"input_micro_per_million"),api::text(&r,"output_micro_per_million"));let caps=format!("{} / {}",api::text(&r,"max_input_tokens"),api::text(&r,"max_output_tokens"));view!{<tr><td data-label="版本 / 来源">{label}</td><td data-label="价格">{price}</td><td data-label="token 上限">{caps}</td><td data-label="操作"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{channel.set(api::text(&r,"channel_id"));model.set(api::text(&r,"model"));input.set(api::text(&r,"input_micro_per_million"));output.set(api::text(&r,"output_micro_per_million"));max_in.set(api::text(&r,"max_input_tokens"));max_out.set(api::text(&r,"max_output_tokens"));error.set(String::new());message.set("已填入旧值，保存会创建新版本。".into());billing_dialog("price-dialog",true);}>"复制为新版本"</button></td></tr>}}).collect_view()}</tbody></table></div>
    <p role="status">{move||if busy.get(){"正在请求价格…"}else if rows.get().is_empty(){"暂无价格配置。"}else{""}}</p>
    <p role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><BillingDialog id="price-dialog" title="新建价格版本" busy=busy><form on:submit=save ><fieldset disabled=move||busy.get()><div class="form-grid">
    <label for="price-channel">"渠道 ID"</label><input id="price-channel" required type="text" inputmode="numeric" prop:value=channel aria-describedby="price-error" on:input=move|e|channel.set(event_target_value(&e))/>
    <label for="price-model">"逻辑模型名"</label><input id="price-model" required type="text" prop:value=model aria-describedby="price-error" on:input=move|e|model.set(event_target_value(&e))/>
    <label for="price-in">"输入价格（microUSD / 百万 token）"</label><input id="price-in" required type="text" inputmode="numeric" prop:value=input aria-describedby="price-error" on:input=move|e|input.set(event_target_value(&e))/>
    <label for="price-out">"输出价格（microUSD / 百万 token）"</label><input id="price-out" required type="text" inputmode="numeric" prop:value=output aria-describedby="price-error" on:input=move|e|output.set(event_target_value(&e))/>
    <label for="price-max-in">"输入 token 预占上限"</label><input id="price-max-in" required type="text" inputmode="numeric" prop:value=max_in aria-describedby="price-error" on:input=move|e|max_in.set(event_target_value(&e))/>
    <label for="price-max-out">"输出 token 总上限（含 n）"</label><input id="price-max-out" required type="text" inputmode="numeric" prop:value=max_out aria-describedby="price-error" on:input=move|e|max_out.set(event_target_value(&e))/>
    </div><p id="price-error" role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><div class="module-dialog-actions"><button type="button" class="ghost" on:click=move |_|billing_dialog("price-dialog",false)>"取消"</button><button class="primary" type="submit">"创建价格版本"</button></div></fieldset></form></BillingDialog></div></section>}
}
#[component]
fn RequestPricing(ctx: Ctx) -> impl IntoView {
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
        let values = api::integer(&channel.get_untracked(), "渠道 ID", true)
            .and_then(|c| api::integer(&price.get_untracked(), "按次价格", false).map(|p| (c, p)));
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
    view! {<section class="card"><header class="card-head"><h2>"按请求收费价格版本"</h2><div class="account-actions"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{channel.set(String::new());model.set(String::new());price.set(String::new());error.set(String::new());message.set(String::new());billing_dialog("request-price-dialog",true);}>"新建按次价格"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load(())>"刷新按次价格"</button></div></header><div class="card-body">
    <p class="muted small">"Search / Fetch 按请求计费，单位为整数 microUSD / 次；独立于 token 价格。按渠道 ID + 逻辑模型匹配最新版本，0 表示明确免费。未知用量仍显示未知；预占或估算扣款不表示已确认收费次数。最多展示最近 1000 个版本。"</p>
    <div class="table-wrap"><table><thead><tr><th>"版本 / 渠道 / 模型"</th><th>"价格 microUSD / 次"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|r|{
        let label=format!("v{} / #{} / {}",api::text(&r,"id"),api::text(&r,"channel_id"),api::text(&r,"model"));let amount=api::text(&r,"micro_per_call");
        view!{<tr><td data-label="版本 / 来源">{label}</td><td data-label="microUSD / 次">{amount}</td><td data-label="操作"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{channel.set(api::text(&r,"channel_id"));model.set(api::text(&r,"model"));price.set(api::text(&r,"micro_per_call"));error.set(String::new());message.set("已填入旧值，保存会创建新版本。".into());billing_dialog("request-price-dialog",true);}>"复制为新版本"</button></td></tr>}
    }).collect_view()}</tbody></table></div>
    <p role="status">{move||if busy.get(){"正在请求按次价格…"}else if rows.get().is_empty(){"暂无按次价格，点击“新建按次价格”设置。"}else{""}}</p>
    <p role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><BillingDialog id="request-price-dialog" title="新建按次价格版本" busy=busy><form on:submit=save ><fieldset disabled=move||busy.get()><div class="form-grid">
    <label for="request-price-channel">"渠道 ID"</label><input id="request-price-channel" required type="text" inputmode="numeric" prop:value=channel aria-describedby="request-price-error" on:input=move|e|channel.set(event_target_value(&e))/>
    <label for="request-price-model">"逻辑模型名"</label><input id="request-price-model" required type="text" prop:value=model aria-describedby="request-price-error" on:input=move|e|model.set(event_target_value(&e))/>
    <label for="request-price-micro">"价格（microUSD / 次）"</label><input id="request-price-micro" required type="text" inputmode="numeric" prop:value=price aria-describedby="request-price-error" on:input=move|e|price.set(event_target_value(&e))/>
    </div><p id="request-price-error" role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><div class="module-dialog-actions"><button type="button" class="ghost" on:click=move |_|billing_dialog("request-price-dialog",false)>"取消"</button><button class="primary" type="submit">"创建按次价格版本"</button></div></fieldset></form></BillingDialog>
    </div></section>}
}
#[component]
fn Usage(ctx: Ctx) -> impl IntoView {
    let snapshot = RwSignal::new(String::new());
    let user = RwSignal::new(String::new());
    let key = RwSignal::new(String::new());
    let channel = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let count = RwSignal::new("50".to_string());
    let rows = RwSignal::new(Vec::<Value>::new());
    let cursor = RwSignal::new(None::<i64>);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let loaded = RwSignal::new(false);
    let active_query = RwSignal::new("limit=50".to_string());
    let fetch = move |query: String, older: bool| {
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("GET", &format!("/api/usage?{query}"), None).await {
                Ok(v) => {
                    rows.set(api::rows(&v, "entries"));
                    cursor.set(v["next_before_id"].as_i64());
                    loaded.set(true);
                    if !older {
                        active_query.set(query);
                    }
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    fetch("limit=50".into(), false);
    let search = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let n = match api::integer(&count.get_untracked(), "每页条数", true) {
            Ok(v) if v <= 200 => v,
            _ => {
                error.set("每页条数必须在 1..200 之间".into());
                return;
            }
        };
        let mut q = format!("limit={n}");
        for (s, label, name) in [
            (user, "用户 ID", "user_id"),
            (key, "密钥 ID", "key_id"),
            (channel, "渠道 ID", "channel_id"),
        ] {
            if !s.get_untracked().trim().is_empty() {
                match api::integer(&s.get_untracked(), label, true) {
                    Ok(v) => q.push_str(&format!("&{name}={v}")),
                    Err(e) => {
                        error.set(e);
                        return;
                    }
                }
            }
        }
        if !model.get_untracked().trim().is_empty() {
            q.push_str(&format!(
                "&model={}",
                api::encode(model.get_untracked().trim())
            ));
        }
        fetch(q, false);
    };
    view! {<section class="card"><header class="card-head"><h2>"持久化用量账本"</h2></header><div class="card-body">
    <p class="muted small">"每行是一笔上游尝试，同一请求的 fallback 可能有多行；按 ID 从新到旧。扣款、上游报告或估算成本与预占分别列出；未知值不计为 0。reported = 上游报告，estimated = 上限估算，unpriced = 未定价，not_sent = 未发送。此处不是供应商账单核对结果。"</p>
    <form on:submit=search><fieldset disabled=move||busy.get()><div class="form-grid">
    <label for="usage-user">"用户 ID（留空全部）"</label><input id="usage-user" type="text" inputmode="numeric" prop:value=user aria-describedby="usage-error" on:input=move|e|user.set(event_target_value(&e))/>
    <label for="usage-key">"API Key ID（留空全部）"</label><input id="usage-key" type="text" inputmode="numeric" prop:value=key aria-describedby="usage-error" on:input=move|e|key.set(event_target_value(&e))/>
    <label for="usage-channel">"渠道 ID（留空全部）"</label><input id="usage-channel" type="text" inputmode="numeric" prop:value=channel aria-describedby="usage-error" on:input=move|e|channel.set(event_target_value(&e))/>
    <label for="usage-model">"逻辑模型（精确匹配）"</label><input id="usage-model" type="text" prop:value=model on:input=move|e|model.set(event_target_value(&e))/>
    <label for="usage-limit">"每页条数（1..200）"</label><input id="usage-limit" required type="text" inputmode="numeric" prop:value=count aria-describedby="usage-error" on:input=move|e|count.set(event_target_value(&e))/>
    </div><button class="primary" type="submit">"查询 / 返回最新页"</button></fieldset></form>
    <p id="usage-error" role="alert">{move||error.get()}</p><p role="status">{move||if busy.get(){"正在加载账本…"}else if loaded.get()&&rows.get().is_empty(){"当前条件下没有记录。"}else{""}}</p>
    <div aria-live="polite">{move||api::billing_page_summary(&rows.get()).into_iter().map(|line|view!{<p class="muted small">{line}</p>}).collect_view()}</div>
    <p class="muted small">"汇总仅针对当前筛选的本页尝试，非全账户总额。按次收费 0 / 1 / 未知分别保留；token 列对按次项目不适用。"</p>
    <div class="table-wrap"><table><thead><tr><th>"ID / 请求 / 来源"</th><th>"身份 / 模型"</th><th>"状态 / 用量依据"</th><th>"计费单位 / 收费次数"</th><th>"输入 / 输出 token"</th><th>"扣款 / 成本 / 预占 microUSD"</th><th>"快照"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|r|{
        let origin=format!("{} / 请求 {} / 渠道 {}",api::text(&r,"id"),api::text(&r,"request_id"),api::text(&r,"channel_id"));let who=format!("用户 {} / key {} / {}",api::text(&r,"user_id"),api::text(&r,"key_id"),api::text(&r,"model"));let state=format!("{} / {} / {}{}",api::text(&r,"status"),api::text(&r,"outcome"),api::text(&r,"usage_certainty"),if r["overrun"]==true{" / 超额"}else{""});let tokens=api::billed_tokens(&r);let units=format!("{} / {}",api::billing_unit_label(&r),api::billed_requests(&r));let money=format!("{} / {} / {}",api::text(&r,"debit_micro"),api::text(&r,"observed_cost_micro"),api::text(&r,"reserved_micro"));let detail=api::pretty(&r);
        view!{<tr><td data-label="记录">{origin}</td><td data-label="身份 / 模型">{who}</td><td data-label="状态">{state}</td><td data-label="计费单位 / 收费次数">{units}</td><td data-label="token">{tokens}</td><td data-label="扣款 / 成本 / 预占">{money}</td><td data-label="快照"><button type="button" class="ghost" on:click=move |_|{snapshot.set(detail.clone());billing_dialog("ledger-snapshot-dialog",true);}>"查看账本快照"</button></td></tr>}
    }).collect_view()}</tbody></table></div>
    <button type="button" class="ghost" disabled=move||busy.get()||cursor.get().is_none()||rows.get().is_empty() on:click=move |_|{if let Some(id)=cursor.get_untracked(){fetch(format!("{}&before_id={id}",active_query.get_untracked()),true);}}>"较早一页"</button><p class="muted small">"翻页沿用最近一次查询条件；修改筛选后请点查询。空页表示已到末尾。"</p>
    <BillingDialog id="ledger-snapshot-dialog" title="账本快照"><p class="muted small">"这笔上游尝试的原始记账数据，用于核对与排障。"</p><pre class="admin-json">{move||snapshot.get()}</pre></BillingDialog>
    </div></section>}
}
