//! Credential pool editor. Lists contain metadata; plaintext requires an explicit reveal request.
use crate::{Ctx, admin_api as api};
use leptos::prelude::*;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
fn modal_command(provider: i64, method: &str) {
    if let Some(el)=web_sys::window().and_then(|w|w.document()).and_then(|d|d.get_element_by_id(&format!("provider-key-dialog-{provider}"))) {
        if let Ok(f)=js_sys::Reflect::get(el.as_ref(),&method.into()).and_then(|v|v.dyn_into::<js_sys::Function>()) {let _=f.call0(el.as_ref());}
    }
}
fn focus_modal(provider: i64) {
    if let Some(el)=web_sys::window().and_then(|w|w.document()).and_then(|d|d.query_selector(&format!("#provider-key-dialog-{provider} [data-key-focus]")).ok().flatten()).and_then(|e|e.dyn_into::<web_sys::HtmlElement>().ok()){let _=el.focus();}
}
fn fresh() -> Value {
    json!({"base_url":"","label":"","kind":"api_key","priority":0,"max_concurrent":0,"enabled":true,"models":[]})
}
/// A picker owns only rows that it created; stored and manually configured mappings
/// are never removed by clearing a directory checkbox.
fn upstream_name(m: &Value) -> &str {
    m["upstream"].as_str().filter(|n|!n.is_empty()).unwrap_or_else(||m["model"].as_str().unwrap_or(""))
}
fn choose_model(d: &mut Value, name: &str, checked: bool) {
    let name=name.trim();
    if name.is_empty(){return;}
    let mut models=api::rows(d,"models");
    if checked {
        if !models.iter().any(|m|upstream_name(m)==name||m["_picker_origin"].as_str()==Some(name)) {
            models.push(json!({"model":"","upstream":name,"enabled":true,"_picker_origin":name}));
        }
    } else {
        models.retain(|m|m["_picker_origin"].as_str()!=Some(name));
    }
    d["models"]=json!(models);
}
fn payload(d: &Value, secret: &str, editing: bool) -> Result<Value, String> {
    let label = d["label"].as_str().unwrap_or("").trim();
    if label.is_empty() {
        return Err("请填写Key备注。".into());
    }
    let mut models = Vec::new();
    for m in api::rows(d, "models") {
        let up = m["upstream"].as_str().unwrap_or("").trim();
        let name = m["model"].as_str().unwrap_or("").trim();
        let upstream = if up.is_empty() { name } else { up };
        if upstream.is_empty() { return Err("请填写上游模型名，或移除未填写的模型。".into()); }
        let model = if name.is_empty() { upstream } else { name };
        if models.iter().any(|m: &Value| m["model"] == model) { return Err(format!("本地映射名 {model} 重复，请为不同上游模型设置不同名称。")); }
        models.push(json!({"model":model,"upstream":if model == upstream { "" } else { upstream },"enabled":m["enabled"]!=false}));
    }
    let mut b = json!({"label":label,"max_concurrent":d["max_concurrent"],"models":models});
    if !editing && d["kind"] == "oauth" {
        for k in ["max_input_tokens", "max_output_tokens"] {
            if let Some(v) = d.get(k).filter(|v| v.is_u64()) {
                b[k] = v.clone();
            }
        }
    }
    if editing {
        b["enabled"] = d["enabled"].clone();
        b["priority"] = d["priority"].clone();
    } else {
        b["inherit_models"] = json!(false);
        b["kind"] = d["kind"].clone();
        if d["kind"] == "oauth" {
            b["oauth_account_id"] = d["oauth_account_id"].clone();
        }
    }
    if !editing && d["tools_enabled"]==true {b["tools"]=json!({"search":d["tools"]["search"]==true,"fetch":d["tools"]["fetch"]==true,"account_scope":d["tools"]["account_scope"].as_str().unwrap_or(""),"qps":d["tools"]["qps"].as_u64().unwrap_or(5)});}
    if !secret.is_empty() {
        b["key"] = json!(secret)
    } else if !editing && d["kind"] == "api_key" {
        return Err("请填写上游API Key。".into());
    }
    Ok(b)
}
fn scoped_payload(mut body: Value, models_only: bool) -> Value {
    if models_only {let models=body.get("models").cloned();body=json!({});if let Some(models)=models{body["models"]=models;}}
    else {body.as_object_mut().unwrap().remove("models");}
    body
}
#[component]
pub fn ProviderKeys(ctx: Ctx, provider_id: i64, on_changed: Callback<()>, #[prop(default=false)] local: bool, #[prop(default=false)] initially_open: bool, #[prop(optional)] on_open: Option<Callback<bool>>) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Value>::new());
    let accounts = RwSignal::new(Vec::<Value>::new());
    let _ = initially_open;
    let open = RwSignal::new(false);
    let modal_view = RwSignal::new("list");
    let changed = RwSignal::new(false);
    let revealed = RwSignal::new(String::new());
    let reveal_generation = RwSignal::new(0u64);
    let reveal_busy = RwSignal::new(false);
    let copy_note = RwSignal::new(String::new());
    let editing = RwSignal::new(None::<i64>);
    let form_open = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let load_error = RwSignal::new(String::new());
    let catalog = RwSignal::new(Vec::<String>::new());
    let catalog_query = RwSignal::new(String::new());
    let catalog_busy = RwSignal::new(false);
    let catalog_error = RwSignal::new(String::new());
    let catalog_loaded = RwSignal::new(false);
    let catalog_truncated = RwSignal::new(false);
    let catalog_generation = RwSignal::new(0u64);
    let picker_open = RwSignal::new(false);
    let catalog_attempted = RwSignal::new(false);
    let draft = RwSignal::new(fresh());
    let secret = RwSignal::new(String::new());
    let original_models = RwSignal::new(Vec::<Value>::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let rev = RwSignal::new(0u64);
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        rev.get();
        loading.set(true);
        load_error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request(
                "GET",
                &format!("/api/providers/{provider_id}/credentials"),
                None,
            )
            .await
            {
                Ok(v) => rows.set(api::rows(&v, "credentials")),
                Err(e) => {
                    ctx.note_err(&e);
                    load_error.set(e.message);
                }
            }
            loading.set(false);
            match api::request("GET", "/api/oauth/accounts", None).await {
                Ok(v) => accounts.set(
                    api::rows(&v, "accounts")
                        .into_iter()
                        .filter(|a| a["status"] == "active")
                        .collect(),
                ),
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
        });
    });
    let discover = Callback::new(move |force: bool| {
        if catalog_busy.get_untracked() || (!force && catalog_attempted.get_untracked()) { return; }
        let d = draft.get_untracked();
        let mut body = json!({});
        let entered = secret.get_untracked();
        if let Some(id) = editing.get_untracked().filter(|_|entered.is_empty()) { body["credential_id"]=json!(id); }
        else { body["kind"]=d["kind"].clone(); if !entered.is_empty(){body["key"]=json!(entered);} }
        if local { let url=api::text(&d,"base_url");if !url.trim().is_empty(){body["base_url"]=json!(url.trim());} }
        catalog_generation.update(|n|*n+=1);
        let generation=catalog_generation.get_untracked();
        catalog_busy.set(true);catalog_attempted.set(true);catalog_error.set(String::new());catalog_loaded.set(false);catalog.set(vec![]);
        leptos::task::spawn_local(async move {
            let result=api::request("POST",&format!("/api/providers/{provider_id}/models/discover"),Some(body)).await;
            if catalog_generation.try_get_untracked()==Some(generation) {
                match result {
                    Ok(v)=>{catalog_truncated.set(v["truncated"]==true);catalog.set(v["models"].as_array().map(|a|a.iter().filter_map(|v|v.as_str().map(str::to_owned)).collect()).unwrap_or_default());catalog_loaded.set(true);},
                    Err(e)=>{ctx.note_err(&e);catalog_error.set(e.message);}
                }
            }
            if catalog_generation.try_get_untracked()==Some(generation) {catalog_busy.set(false);}
        });
    });
    let clear_private = Callback::new(move |()| {
        reveal_generation.update(|n|*n+=1);revealed.set(String::new());reveal_busy.set(false);secret.set(String::new());copy_note.set(String::new());
        catalog_generation.update(|n|*n+=1);catalog_busy.set(false);catalog_attempted.set(false);catalog.set(vec![]);catalog_loaded.set(false);picker_open.set(false);catalog_query.set(String::new());
    });
    let leave_editor = Callback::new(move |()| {clear_private.run(());form_open.set(false);editing.set(None);modal_view.set("list");error.set(String::new());focus_modal(provider_id);});
    let reveal_key = Callback::new(move |copy: bool| {
        if reveal_busy.get_untracked(){return;}
        let Some(id)=editing.get_untracked() else{return;};
        if !copy && !revealed.get_untracked().is_empty(){revealed.set(String::new());copy_note.set(String::new());reveal_generation.update(|n|*n+=1);return;}
        let generation=reveal_generation.get_untracked();reveal_busy.set(true);copy_note.set(String::new());error.set(String::new());
        leptos::task::spawn_local(async move {
            let result=api::request("POST",&format!("/api/providers/{provider_id}/credentials/{id}/reveal"),None).await;
            if reveal_generation.try_get_untracked()!=Some(generation){return;}
            match result {
                Ok(v)=>{let key=api::text(&v,"key");if copy {let copied=super::settings::copy_text(&key).await;if reveal_generation.try_get_untracked()==Some(generation){copy_note.set(if copied{"已复制原密钥".into()}else{"复制失败，请使用显示按钮后手工复制。".into()});}}else{revealed.set(key);}},
                Err(e)=>{ctx.note_err(&e);revealed.set(String::new());error.set(e.message);}
            }
            if reveal_generation.try_get_untracked()==Some(generation){reveal_busy.set(false);}
        });
    });
    // Switching views removes the clicked control. Restore focus only after the
    // new view has rendered; input edits must not retrigger this effect.
    Effect::new(move |_| {
        let opened=open.get();
        let view=modal_view.get();
        form_open.get();
        if !opened {return;}
        let callback=wasm_bindgen::closure::Closure::once_into_js(move || {
            if open.try_get_untracked()==Some(true) && modal_view.try_get_untracked()==Some(view) {
                focus_modal(provider_id);
            }
        });
        if let Some(window)=web_sys::window(){let _=window.request_animation_frame(callback.unchecked_ref());}
    });
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let id = editing.get_untracked();
        let mut input=draft.get_untracked();
        if modal_view.get_untracked()=="key" {input["models"]=json!([]);}
        let body = match payload(
            &input,
            &secret.get_untracked(),
            id.is_some(),
        ) {
            Ok(mut b) => {
                if local {
                    let urls = draft.get_untracked()["base_urls"].as_array().map(|a| a.len()).unwrap_or(0);
                    let url = draft.get_untracked()["base_url"].as_str().unwrap_or("").trim().to_string();
                    if !url.is_empty() {b["base_url"]=json!(url);}
                    else if id.is_none() || urls == 1 {error.set("请填写此模型服务的 API URL。".into());return;}

                }
                if id.is_some() {
                    let original = original_models.get_untracked();
                    let delta = api::rows(&b, "models")
                        .into_iter()
                        .filter(|m| {
                            !original.iter().any(|o| {
                                o["model"] == m["model"]
                                    && o["upstream"].as_str().unwrap_or("")
                                        == m["upstream"].as_str().unwrap_or("")
                                    && (o["enabled"] != false) == (m["enabled"] != false)
                            })
                        })
                        .collect::<Vec<_>>();
                    if delta.is_empty() {
                        b.as_object_mut().unwrap().remove("models");
                    } else {
                        b["models"] = json!(delta);
                    }
                }
                scoped_payload(b,modal_view.get_untracked()=="models")
            }
            Err(e) => {
                error.set(e);
                return;
            }
        };
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            let path = id
                .map(|i| format!("/api/providers/{provider_id}/credentials/{i}"))
                .unwrap_or_else(|| format!("/api/providers/{provider_id}/credentials"));
            match api::request(
                if id.is_some() { "PATCH" } else { "POST" },
                &path,
                Some(body),
            )
            .await
            {
                Ok(_) => {
                    secret.set(String::new());
                    editing.set(None);
                    draft.set(fresh());
                    form_open.set(false);catalog_generation.update(|n|*n+=1);catalog_busy.set(false);catalog_attempted.set(false);picker_open.set(false);
                    rev.update(|r| *r += 1);
                    clear_private.run(());modal_view.set("list");changed.set(true);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<div class="card-body provider-key-pool"><button class="ghost" aria-haspopup="dialog" on:click=move |_|{open.set(true);modal_view.set("list");if let Some(cb)=on_open{cb.run(false);}modal_command(provider_id,"showModal");}>"管理上游 Key"</button>
    <dialog id=format!("provider-key-dialog-{provider_id}") class="provider-key-modal console-content" aria-labelledby=format!("provider-key-title-{provider_id}") on:close=move |_|{clear_private.run(());open.set(false);form_open.set(false);modal_view.set("list");editing.set(None);if changed.get_untracked(){changed.set(false);on_changed.run(());}}><header class="provider-key-modal-head"><h2 id=format!("provider-key-title-{provider_id}")>{move||match modal_view.get(){"models"=>"选择上游模型","key"=>if editing.get().is_some(){"编辑 Key"}else{"添加上游 Key"},_=>"管理上游 Key"}}</h2><button type="button" class="ghost" aria-label="关闭上游 Key 管理" data-key-focus="" autofocus on:click=move |_|modal_command(provider_id,"close")>"关闭"</button></header><div class="provider-key-modal-body"><Show when=move||open.get()><Show when=move||modal_view.get()=="list"><p class="account-help">"每个Key可配置多个模型。并发策略按用户保持Key粘性；Key顺序数值越小越优先。停用保留账单与历史。"</p><Show when=move||loading.get()><p role="status">"正在读取已配置的上游 Key…"</p></Show><Show when=move||!load_error.get().is_empty()><p role="alert">{move||format!("读取上游 Key 失败：{}",load_error.get())}</p><button type="button" class="ghost" on:click=move |_|rev.update(|v|*v+=1)>"重新读取"</button></Show><Show when=move||!loading.get()&&load_error.get().is_empty()&&rows.get().is_empty()><p class="account-help">"此渠道尚未配置上游 Key。点击添加后填写凭据和模型。"</p></Show><div class="table-wrap"><table><thead><tr><th>"Key备注"</th><th>"类型 / 状态"</th><th>"优先顺序"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().enumerate().map(|(position,row)|{let item=row.clone();let model_item=row.clone();let id=row["id"].as_i64().unwrap_or(0);let is_enabled=row["enabled"]==true;view!{<tr><td data-label="Key备注">{crate::provider_api::credential_label(&row,position)}{local.then(||view!{<p class="provider-url muted small">{row["base_urls"].as_array().map(|a|a.iter().filter_map(|v|v.as_str()).collect::<Vec<_>>().join(" / ")).unwrap_or_default()}</p>})}</td><td data-label="类型 / 状态">{format!("{} · {}",match row["kind"].as_str(){Some("oauth")=>"GPT 授权",Some("anonymous")=>"免认证",_=>"API Key"},if row["enabled"]==true{"启用"}else{"停用"})}<p class="muted small">{match row["kind"].as_str(){Some("oauth")=>if row["oauth_account_id"].is_number(){"已绑定授权账号"}else{"授权账号待确认"},Some("anonymous")=>"无需密钥",_=>if row["has_secret"]==true{"密钥已配置"}else{"密钥待配置"}}}</p></td><td data-label="优先顺序">{row["priority"].to_string()}</td><td data-label="操作"><div class="provider-key-row-actions"><button class="ghost" disabled=move||busy.get()||loading.get()||!load_error.get().is_empty() on:click=move |_|{clear_private.run(());modal_view.set("key");form_open.set(true);catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);catalog_error.set(String::new());catalog_query.set(String::new());editing.set(Some(id));original_models.set(api::rows(&item,"models"));let mut d=item.clone();if local {d["base_url"]=d["base_urls"].as_array().and_then(|a|if a.len()==1{a.first().cloned()}else{None}).unwrap_or(json!(""));}draft.set(d);secret.set(String::new());error.set(String::new());}>"编辑 Key"</button><button class="ghost" disabled=move||busy.get()||loading.get()||!load_error.get().is_empty() on:click=move |_|{clear_private.run(());modal_view.set("models");form_open.set(true);catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);catalog_error.set(String::new());catalog_query.set(String::new());editing.set(Some(id));original_models.set(api::rows(&model_item,"models"));let mut d=model_item.clone();if local {d["base_url"]=d["base_urls"].as_array().and_then(|a|if a.len()==1{a.first().cloned()}else{None}).unwrap_or(json!(""));}draft.set(d);secret.set(String::new());error.set(String::new());}>"选择模型"</button><button class="ghost danger" disabled=move||busy.get() on:click=move |_|{if busy.get_untracked(){return}busy.set(true);leptos::task::spawn_local(async move{match api::request("PATCH",&format!("/api/providers/{provider_id}/credentials/{id}"),Some(json!({"enabled":!is_enabled}))).await{Ok(_)=>{rev.update(|r|*r+=1);changed.set(true);},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});}>{if is_enabled{"停用"}else{"启用"}}</button></div></td></tr>}}).collect_view()}</tbody></table></div>
    <button type="button" class="ghost" disabled=move||busy.get()||loading.get()||!load_error.get().is_empty() on:click=move |_|{clear_private.run(());modal_view.set("key");form_open.set(true);catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);catalog_error.set(String::new());catalog_query.set(String::new());editing.set(None);draft.set(fresh());original_models.set(vec![]);secret.set(String::new());error.set(String::new());}>"添加上游 Key"</button></Show>
    <Show when=move||!form_open.get()&&!error.get().is_empty()><p role="alert" class="account-error">{move||error.get()}</p></Show><Show when=move||form_open.get()><button type="button" class="ghost" data-key-focus="" on:click=move |_|leave_editor.run(())>"返回 Key 列表"</button><Show when=move||editing.get().is_some()><p class="account-help">{move||format!("当前 Key：{}",api::text(&draft.get(),"label"))}</p></Show><form class="account-form provider-key-editor" on:submit=save on:keydown=move|e|{if e.key()=="Escape"&&picker_open.get_untracked(){e.prevent_default();e.stop_propagation();picker_open.set(false);}}><fieldset disabled=move||busy.get()><Show when=move||modal_view.get()=="key"><h3>{move||if editing.get().is_some(){"编辑上游Key"}else{"新增上游Key"}}</h3><div class="policy-limits"><label class="account-field"><span>"Key备注"</span><input required maxlength="128" placeholder="如 主账号、备用账号" prop:value=move||api::text(&draft.get(),"label") on:input=move|e|draft.update(|d|d["label"]=json!(event_target_value(&e)))/></label><label class="account-field"><span>"凭据类型"</span><select disabled=move||editing.get().is_some() prop:value=move||api::text(&draft.get(),"kind") on:change=move|e|{draft.update(|d|d["kind"]=json!(event_target_value(&e)));secret.set(String::new());catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);}><option value="api_key">"API Key"</option>{(!local).then(||view!{<option value="oauth">"已授权GPT账号"</option>})}<option value="anonymous">"本地免密服务"</option></select></label></div>
    <Show when=move||local><label class="account-field"><span>"模型服务 API URL"</span><input type="url" required=move||editing.get().is_none() placeholder="http://192.168.1.10:8001/v1" prop:value=move||api::text(&draft.get(),"base_url") on:input=move|e|{draft.update(|d|d["base_url"]=json!(event_target_value(&e)));revealed.set(String::new());reveal_generation.update(|n|*n+=1);copy_note.set(String::new());catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);}/></label><p class="account-help">"同一供应方可添加多个服务地址，每个地址分别设置凭据和模型。IP 相同、端口不同也可以。"</p></Show>
    <Show when=move||draft.get()["kind"]=="api_key"><Show when=move||editing.get().is_some()><p class="account-help">"原密钥已配置。查看或复制不会替换密钥；下面留空保存会继续使用原值。"</p><div class="provider-key-original"><label class="account-field"><span>"原 API Key"</span><input readonly autocomplete="off" type=move||if revealed.get().is_empty(){"password"}else{"text"} prop:value=move||if revealed.get().is_empty(){"••••••••••••••••".to_string()}else{revealed.get()}/></label><div class="provider-key-row-actions"><button type="button" class="ghost" aria-disabled=move||reveal_busy.get().to_string() on:click=move |_|reveal_key.run(false)>{move||if revealed.get().is_empty(){"显示原 Key"}else{"隐藏原 Key"}}</button><button type="button" class="ghost" aria-disabled=move||reveal_busy.get().to_string() on:click=move |_|reveal_key.run(true)>"复制原 Key"</button></div><p role="status">{move||copy_note.get()}</p></div></Show><label class="account-field"><span>{move||if editing.get().is_some(){"替换上游Key（留空保留）"}else{"上游API Key"}}</span><input type="password" autocomplete="new-password" prop:value=secret on:input=move|e|{secret.set(event_target_value(&e));revealed.set(String::new());reveal_generation.update(|n|*n+=1);copy_note.set(String::new());catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);}/></label></Show>
    <Show when=move||draft.get()["kind"]=="oauth"&&editing.get().is_none()><label class="account-field"><span>"已授权账号"</span><select required prop:value=move||draft.get()["oauth_account_id"].as_i64().map(|n|n.to_string()).unwrap_or_default() on:change=move|e|draft.update(|d|d["oauth_account_id"]=event_target_value(&e).parse::<i64>().map(|n|json!(n)).unwrap_or(Value::Null))><option value="">"请选择已完成安全授权的账号"</option>{move||accounts.get().into_iter().map(|a|view!{<option value=a["id"].to_string()>{api::text(&a,"label")}</option>}).collect_view()}</select></label><p class="account-help">"选择已授权 GPT 账号，保存绑定后即可重新编辑并探测该账号的官方模型目录。首次绑定需设置输入、输出 Token 上限；留空仅可沿用该账号已验证的边界。"</p><div class="policy-limits">{[ ("max_input_tokens","最大输入Token"),("max_output_tokens","最大输出Token")].into_iter().map(|(k,label)|view!{<label class="account-field"><span>{label}</span><input type="number" min="1" step="1" placeholder="已有验证配置时可留空继承" on:input=move|e|draft.update(|d|d[k]=event_target_value(&e).parse::<u64>().ok().filter(|n|*n>0).map(|n|json!(n)).unwrap_or(Value::Null))/></label>}).collect_view()}</div></Show>
    <Show when=move||editing.get().is_some()><div class="policy-limits provider-key-priority"><label class="account-field"><span>"优先顺序（越小越优先）"</span><input type="number" step="1" prop:value=move||draft.get()["priority"].to_string() on:input=move|e|{if let Ok(n)=event_target_value(&e).parse::<i64>(){draft.update(|d|d["priority"]=json!(n));}}/></label><label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["enabled"]==true on:change=move|e|draft.update(|d|d["enabled"]=json!(event_target_checked(&e)))/>"启用此Key"</label></div></Show>
    <label class="account-field"><span>"此Key最大同时请求数（0不限）"</span><input type="number" min="0" step="1" prop:value=move||draft.get()["max_concurrent"].to_string() on:input=move|e|{if let Ok(n)=event_target_value(&e).parse::<u32>(){draft.update(|d|d["max_concurrent"]=json!(n));}}/></label>
    <Show when=move||editing.get().is_none()&&!local><label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["tools_enabled"]==true on:change=move|e|draft.update(|d|{let checked=event_target_checked(&e);d["tools_enabled"]=json!(checked);if checked&&d["tools"].is_null(){d["tools"]=json!({"search":true,"fetch":false,"account_scope":"","qps":5});}})/>"此入口提供搜索 / 网页提取工具"</label><Show when=move||draft.get()["tools_enabled"]==true><div class="account-actions">{[("search","启用搜索接口"),("fetch","启用网页提取接口")].into_iter().map(|(k,label)|view!{<label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["tools"][k]==true on:change=move|e|draft.update(|d|d["tools"][k]=json!(event_target_checked(&e)))/>{label}</label>}).collect_view()}</div><div class="policy-limits"><label class="account-field"><span>"供应方账户限流分组"</span><input required placeholder="同一上游账户使用相同分组名" prop:value=move||draft.get()["tools"]["account_scope"].as_str().unwrap_or("").to_string() on:input=move|e|draft.update(|d|d["tools"]["account_scope"]=json!(event_target_value(&e)))/></label><label class="account-field"><span>"每秒请求数 QPS"</span><input type="number" min="1" max="10000" required prop:value=move||draft.get()["tools"]["qps"].as_u64().unwrap_or(5).to_string() on:input=move|e|{if let Ok(n)=event_target_value(&e).parse::<u32>(){draft.update(|d|d["tools"]["qps"]=json!(n));}}/></label></div><p class="account-help">"按供应方明确能力设置；同一上游账户的Key应共享限流分组。搜索/提取按次计费需在账务页配置真实来源价格，不会从其他Key猜测价格。"</p></Show></Show>
 </Show><Show when=move||modal_view.get()=="models"><h3>"此Key可用模型"</h3><p class="account-help">{if local{"先选择或填写上游模型名。本地映射名是客户端调用的名称，留空沿用上游名称。不同端口分别配置模型。"}else{"先选择或填写上游模型名。本地映射名是客户端调用的名称，留空沿用上游名称。取消勾选可停用已配置模型。"}}</p>
    <div class="upstream-model-picker"><label class="account-field"><span>"添加上游模型"</span><input type="text" autocomplete="off" aria-controls=format!("key-model-options-{provider_id}") aria-expanded=move||picker_open.get().to_string() aria-describedby=format!("key-model-help-{provider_id}") placeholder="选择上游模型，或输入名称后按回车" prop:value=catalog_query on:focus=move |_|{picker_open.set(true);discover.run(false);} on:click=move |_|{picker_open.set(true);discover.run(false);} on:input=move|e|{catalog_query.set(event_target_value(&e));picker_open.set(true);} on:keydown=move|e|{if e.key()=="Enter"{e.prevent_default();let name=catalog_query.get_untracked();draft.update(|d|choose_model(d,&name,true));catalog_query.set(String::new());}else if e.key()=="Escape"&&picker_open.get_untracked(){e.prevent_default();e.stop_propagation();picker_open.set(false);}}/></label><p id=format!("key-model-help-{provider_id}") class="account-help">"勾选即添加模型配置；也可输入名称后按回车添加。已有映射会保留，目录探测不发送推理请求。"</p>
    <Show when=move||picker_open.get()><div class="upstream-model-options" role="group" aria-label="上游模型选择" id=format!("key-model-options-{provider_id}")><div class="upstream-model-options-head"><span>{move||if catalog_busy.get(){"正在读取此 Key 的模型…"}else{"可选上游模型（可多选）"}}</span><button type="button" class="ghost" aria-label="关闭上游模型列表" on:click=move |_|picker_open.set(false)>"关闭"</button></div><Show when=move||!catalog_error.get().is_empty()><p role="alert">{move||catalog_error.get()}</p></Show><Show when=move||catalog_loaded.get()&&catalog.get().is_empty()><p role="status">"此 Key 未返回模型目录，可直接输入模型名。"</p></Show><Show when=move||catalog_truncated.get()&&catalog_loaded.get()><p class="account-help">"目录可能不完整，未列出的模型可手工输入。"</p></Show><Show when=move||catalog_loaded.get()&&!catalog.get().is_empty()&&!catalog.get().iter().any(|name|name.to_lowercase().contains(&catalog_query.get().to_lowercase()))><p class="account-help">"未找到匹配项，可按回车添加输入的模型名。"</p></Show><div class="upstream-model-choice-list">{move||{let query=catalog_query.get().to_lowercase();catalog.get().into_iter().filter(|n|n.to_lowercase().contains(&query)).map(|name|{let check=name.clone();let locked=name.clone();let status=name.clone();let update=name.clone();view!{<label class="account-check"><input type="checkbox" prop:checked=move||api::rows(&draft.get(),"models").iter().any(|m|upstream_name(m)==check||m["_picker_origin"].as_str()==Some(check.as_str())) disabled=move||api::rows(&draft.get(),"models").iter().any(|m|upstream_name(m)==locked&&m["_picker_origin"].is_null()) on:change=move|e|{let checked=event_target_checked(&e);draft.update(|d|choose_model(d,&update,checked));}/><span>{name}{move||if api::rows(&draft.get(),"models").iter().any(|m|upstream_name(m)==status&&m["_picker_origin"].is_null()){"（已配置）"}else{""}}</span></label>}}).collect_view()}}</div><Show when=move||!catalog_busy.get()><button type="button" class="ghost" on:click=move |_|discover.run(true)>"重新读取模型目录"</button></Show></div></Show></div>
    {move||api::rows(&draft.get(),"models").into_iter().enumerate().map(|(i,m)|{
        let existing = original_models.get().iter().any(|o|o["model"]==m["model"]);
        let model=api::text(&m,"model");let up=m["upstream"].as_str().filter(|s|!s.is_empty()).unwrap_or(&model).to_string();
        let mapping=if model==up&&!existing{String::new()}else{model};
        view!{<div class="policy-limits provider-model-mapping"><label class="account-field"><span>"上游模型名"</span><input required on:keydown=move|e|{if e.key()=="Enter"{e.prevent_default();}else if e.key()=="Escape"&&picker_open.get_untracked(){e.prevent_default();e.stop_propagation();picker_open.set(false);}} readonly=m["conflict"]==true prop:value=up on:change=move|e|draft.update(|d|d["models"][i]["upstream"]=json!(event_target_value(&e)))/></label><label class="account-field"><span>"本地映射名"</span><input readonly=existing||m["conflict"]==true placeholder="留空沿用上游名称" prop:value=mapping on:change=move|e|draft.update(|d|d["models"][i]["model"]=json!(event_target_value(&e)))/></label><label class="account-check"><input type="checkbox" disabled=m["conflict"]==true prop:checked=m["enabled"]!=false on:change=move|e|draft.update(|d|d["models"][i]["enabled"]=json!(event_target_checked(&e)))/>"模型启用"</label>{(!existing).then(||view!{<button type="button" class="ghost danger" on:click=move |_|draft.update(|d|{if let Some(a)=d["models"].as_array_mut(){a.remove(i);}})>"移除此模型"</button>})}{existing.then(||view!{<p class="account-help">"已有本地映射名保持不变，以保留授权和调用历史。如需新名称，请另外添加映射，再按需停用原模型。"</p>})}{(m["conflict"]==true).then(||view!{<p class="account-help">"旧来源映射存在差异，保留原配置；可先独立编辑 Key，模型差异请在诊断与分组中核对。"</p>})}</div>}}).collect_view()}
    </Show><p role="alert" class="account-error">{move||error.get()}</p><div class="account-actions"><button type="button" class="ghost" on:click=move |_|leave_editor.run(())>"取消"</button><button type="submit" class="primary">{move||if busy.get(){"正在保存…"}else if modal_view.get()=="models"{"保存模型配置"}else{"保存 Key"}}</button></div></fieldset></form></Show></Show></div></dialog></div>}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_payload_drops_response_metadata_and_blank_rotation() {
        let d = json!({"label":"primary","kind":"api_key","priority":1,"max_concurrent":2,"enabled":true,"revision":8,"models":[{"model":"m","upstream":"x","enabled":false,"conflict":false,"source_ids":[1]}]});
        let b = payload(&d, "", true).unwrap();
        assert!(b.get("key").is_none());
        assert!(b.get("revision").is_none());
        assert_eq!(
            b["models"][0],
            json!({"model":"m","upstream":"x","enabled":false})
        );
    }
    #[test]
    fn upstream_name_is_used_when_mapping_is_blank() {
        let b=payload(&json!({"label":"k","kind":"anonymous","models":[{"model":"","upstream":"vendor/model","enabled":true}]}),"",false).unwrap();
        assert_eq!(b["models"][0],json!({"model":"vendor/model","upstream":"","enabled":true}));
    }
    #[test]
    fn explicit_local_mapping_keeps_upstream_name() {
        let b=payload(&json!({"label":"k","kind":"anonymous","models":[{"model":"my-model","upstream":"vendor/model"}]}),"",false).unwrap();
        assert_eq!(b["models"][0]["model"],"my-model");
        assert_eq!(b["models"][0]["upstream"],"vendor/model");
    }
    #[test]
    fn empty_and_duplicate_local_mappings_are_rejected() {
        assert!(payload(&json!({"label":"k","kind":"anonymous","models":[{"model":"","upstream":""}]}),"",false).is_err());
        assert!(payload(&json!({"label":"k","kind":"anonymous","models":[{"model":"same","upstream":"a"},{"model":"same","upstream":"b"}]}),"",false).is_err());
    }
    #[test]
    fn directory_selection_adds_immediately_without_renaming_existing() {
        let mut d=json!({"models":[{"model":"existing-local","upstream":"vendor/old","enabled":false}]});
        choose_model(&mut d,"vendor/new",true);
        assert_eq!(d["models"][1]["model"],"");
        assert_eq!(d["models"][1]["upstream"],"vendor/new");
        choose_model(&mut d,"vendor/old",true);
        assert_eq!(api::rows(&d,"models").len(),2);
        assert_eq!(d["models"][0]["enabled"],false);
        choose_model(&mut d,"vendor/old",false);
        assert_eq!(api::rows(&d,"models").len(),2);
    }
    #[test]
    fn deselect_uses_stable_origin_after_mapping_edits() {
        let mut d=json!({"models":[{"model":"old","upstream":"vendor/new"}]});
        choose_model(&mut d,"other",true);
        d["models"][1]["model"]=json!("my-alias");
        d["models"][1]["upstream"]=json!("edited-upstream");
        choose_model(&mut d,"other",true);
        assert_eq!(api::rows(&d,"models").len(),2);
        choose_model(&mut d,"other",false);
        assert_eq!(api::rows(&d,"models").len(),1);
        assert_eq!(d["models"][0]["model"],"old");
    }
    #[test]
    fn manual_entry_trims_and_deduplicates_without_sending_picker_metadata() {
        let mut d=json!({"label":"k","kind":"anonymous","models":[]});
        choose_model(&mut d,"  manual-model  ",true);
        choose_model(&mut d,"manual-model",true);
        choose_model(&mut d,"  ",true);
        assert_eq!(api::rows(&d,"models").len(),1);
        let b=payload(&d,"",false).unwrap();
        assert_eq!(b["models"][0],json!({"model":"manual-model","upstream":"","enabled":true}));
        assert_eq!(b["inherit_models"],false);
    }
    #[test]
    fn separate_views_write_only_their_own_fields() {
        let body=json!({"label":"retained","key":"replacement","base_url":"http://service/v1","enabled":true,"models":[{"model":"a"}]});
        let keys=scoped_payload(body.clone(),false);
        assert!(keys.get("models").is_none());assert_eq!(keys["key"],"replacement");
        let models=scoped_payload(body,true);
        assert_eq!(models,json!({"models":[{"model":"a"}]}));
        assert_eq!(scoped_payload(json!({"label":"x"}),true),json!({}));
    }
    #[test]
    fn new_api_credential_requires_secret() {
        assert!(payload(&json!({"label":"key","kind":"api_key"}), "", false).is_err());
    }
}
