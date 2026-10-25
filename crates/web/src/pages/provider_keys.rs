//! Credential pool editor. Lists contain metadata; plaintext requires an explicit reveal request.
use crate::{Ctx, admin_api as api};
use super::{family_picker::FamilyPicker, oauth_enrollment::{OAuthEnrollment,account_provider}, connection_parameters::ConnectionParameters};
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
pub(crate) fn family_api_base(family: &str) -> &'static str {
    match family {
        "gpt"=>"https://api.openai.com/v1", "claude"=>"https://api.anthropic.com",
        "deepseek"=>"https://api.deepseek.com/v1", "glm"=>"https://open.bigmodel.cn/api/paas/v4",
        "kimi"=>"https://api.moonshot.cn/v1", "mimo"=>"https://api.xiaomimimo.com/v1",
        "minimax"=>"https://api.minimax.io/v1", "grok"=>"https://api.x.ai/v1",
        "mistral"=>"https://api.mistral.ai/v1", _=>"",
    }
}
fn new_provider_payload(d: &Value) -> Result<Value,String> {
    let name=d["_provider_name"].as_str().unwrap_or("").trim();
    if name.is_empty(){return Err("请填写渠道名称。".into());}
    let local=d["_endpoint_mode"]=="local";
    let base=if d["kind"]=="oauth"{d["_oauth_api_base"].as_str().unwrap_or("")}else{d["_provider_base_url"].as_str().unwrap_or("")}.trim();
    if !local && base.is_empty(){return Err("请填写供应方 API URL；OAuth 线路需先选择有效的授权来源。".into());}
    Ok(json!({"name":name,"enabled":true,"base_url":if local{None}else{Some(base)},"endpoint_mode":if local{"local"}else{"single"},"key_strategy":"ordered","switch_on_429":true,"switch_on_400":false}))
}
/// OAuth authenticates a service identity, which can span several model families.
/// Keep this in step with the backend's egress::oauth_family routing contract.
fn network_policy_family(d: &Value) -> &str {
    if d["kind"]=="oauth" {
        match d["_oauth_provider"].as_str().unwrap_or("") {
            "openai"=>"gpt", "antigravity"=>"gemini", "kimi"=>"kimi",
            "minimax-cn"|"minimax-global"=>"minimax", _=>"other",
        }
    }else{d["_model_family"].as_str().unwrap_or("other")}
}
fn fresh() -> Value {
    json!({"_provider_name":"","_endpoint_mode":"single","_provider_base_url":"https://api.openai.com/v1","_model_family":"gpt","_oauth_provider":"openai","base_url":"","label":"","kind":"api_key","priority":0,"max_concurrent":0,"enabled":true,"models":[]})
}
/// Directory selection changes availability; explicit removal changes membership.
fn upstream_name(m: &Value) -> &str {
    m["upstream"].as_str().filter(|n|!n.is_empty()).unwrap_or_else(||m["model"].as_str().unwrap_or(""))
}
fn choose_model(d: &mut Value, name: &str, checked: bool) -> String {
    let name=name.trim();
    if name.is_empty(){return "请先填写上游模型名。".into();}
    let mut models=api::rows(d,"models");
    let matches=|m:&Value|upstream_name(m)==name||m["_picker_origin"].as_str()==Some(name);
    let message=if checked {
        if models.iter().any(|m|matches(m)&&m["enabled"]!=false) {
            "此上游已有启用映射；如需另一个本地名称，请点击“添加同上游映射”。".into()
        } else if let Some(row)=models.iter_mut().find(|m|matches(m)&&m["conflict"]!=true) {
            row["enabled"]=json!(true);
            format!("已在草稿中重新启用 {}，保存模型配置后生效。",row["model"].as_str().unwrap_or(name))
        } else {
            models.push(json!({"model":"","upstream":name,"enabled":true,"_picker_origin":name}));
            "已添加模型草稿，可填写本地映射名；保存后生效。".into()
        }
    } else {
        models.retain(|m|m["_picker_origin"].as_str()!=Some(name));
        for row in models.iter_mut().filter(|m|matches(m)&&m["conflict"]!=true){row["enabled"]=json!(false);}
        "已在草稿中停用此上游的映射，保存后生效；移除映射请使用下方独立操作。".into()
    };
    d["models"]=json!(models);
    message
}
fn add_model_alias(d:&mut Value,upstream:&str)->String {
    let upstream=upstream.trim();
    if upstream.is_empty(){return "请先填写上游模型名。".into();}
    let mut models=api::rows(d,"models");
    if models.iter().any(|m|upstream_name(m)==upstream&&m["model"].as_str().unwrap_or("").is_empty()){
        return "此上游已有待填写名称的映射，请先填写不同的本地映射名。".into();
    }
    models.push(json!({"model":"","upstream":upstream,"enabled":true}));d["models"]=json!(models);
    "已添加同上游映射草稿，请填写不同的本地映射名，再保存。".into()
}
fn remove_mapping(d:&mut Value,model:&str){
    if let Some(models)=d["models"].as_array_mut(){models.retain(|m|m["model"].as_str()!=Some(model));}
}
fn model_patch(mut body:Value,original:&[Value])->Value {
    let current=api::rows(&body,"models");
    let delta=current.iter().filter(|m|!original.iter().any(|o|tokengo_shared::same_model_name(o["model"].as_str().unwrap_or(""),m["model"].as_str().unwrap_or(""))&&o["upstream"].as_str().unwrap_or("")==m["upstream"].as_str().unwrap_or("")&&(o["enabled"]!=false)==(m["enabled"]!=false))).cloned().collect::<Vec<_>>();
    let removed=original.iter().filter(|o|!current.iter().any(|m|tokengo_shared::same_model_name(m["model"].as_str().unwrap_or(""),o["model"].as_str().unwrap_or("")))).filter_map(|o|o["model"].as_str()).collect::<std::collections::BTreeSet<_>>();
    if delta.is_empty(){body.as_object_mut().unwrap().remove("models");}else{body["models"]=json!(delta);}
    if !removed.is_empty(){body["remove_models"]=json!(removed);}
    body
}
fn focus_model_control(id:String){
    let callback=wasm_bindgen::closure::Closure::once_into_js(move||{
        if let Some(el)=web_sys::window().and_then(|w|w.document()).and_then(|d|d.get_element_by_id(&id)).and_then(|e|e.dyn_into::<web_sys::HtmlElement>().ok()){let _=el.focus();}
    });
    if let Some(window)=web_sys::window(){let _=window.request_animation_frame(callback.unchecked_ref());}
}
fn payload(d: &Value, secret: &str, editing: bool) -> Result<Value, String> {
    let label = d["label"].as_str().unwrap_or("").trim();
    if label.is_empty() {
        return Err("请填写Key备注。".into());
    }
    if !editing && d["kind"] == "oauth" && d["oauth_account_id"].as_i64().is_none_or(|id|id<=0) {
        return Err("请先完成此来源的账号授权，并选择已授权账号。".into());
    }
    if !editing && d["kind"] == "oauth" && d["_oauth_inference_available"]==false {
        return Err("此账号可完成认证，但模型传输尚未接通，当前不能绑定推理接入。".into());
    }
    let mut models = Vec::new();
    for m in api::rows(d, "models") {
        let up = m["upstream"].as_str().unwrap_or("").trim();
        let name = m["model"].as_str().unwrap_or("").trim();
        let upstream = if up.is_empty() { name } else { up };
        if upstream.is_empty() { return Err("请填写上游模型名，或移除未填写的模型。".into()); }
        let model = if name.is_empty() { upstream } else { name };
        if models.iter().any(|m: &Value|tokengo_shared::same_model_name(m["model"].as_str().unwrap_or(""),model)) { return Err(format!("本地映射名 {model} 重复，请为不同上游模型设置不同名称。")); }
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
    if models_only {let models=body.get("models").cloned();let removed=body.get("remove_models").cloned();body=json!({});if let Some(models)=models{body["models"]=models;}if let Some(removed)=removed{body["remove_models"]=removed;}}
    else {body.as_object_mut().unwrap().remove("models");body.as_object_mut().unwrap().remove("remove_models");}
    body
}
#[component]
pub fn ProviderKeys(ctx: Ctx, provider_id: i64, on_changed: Callback<()>, #[prop(default=false)] local: bool, #[prop(default=false)] initially_open: bool, #[prop(optional)] on_open: Option<Callback<bool>>, #[prop(default=false)] create_provider: bool) -> impl IntoView {
    // The dialog keeps a stable DOM identity; requests use the persisted provider only.
    let active_provider=RwSignal::new(provider_id);
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
    let wizard=RwSignal::new(false);
    let parameters_ready=RwSignal::new(false);
    let parameter_sources=RwSignal::new(Vec::<i64>::new());
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
    let is_local=move || local || (create_provider && draft.get()["_endpoint_mode"]=="local");
    let secret = RwSignal::new(String::new());
    let original_models = RwSignal::new(Vec::<Value>::new());
    let removal_pending=RwSignal::new(None::<String>);
    let model_note=RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let save_uncertain=RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let rev = RwSignal::new(0u64);
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        rev.get();
        let provider_id=active_provider.get();
        loading.set(true);
        load_error.set(String::new());
        leptos::task::spawn_local(async move {
            if provider_id>0 {match api::request(
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
        let provider_id=active_provider.get_untracked();
        if provider_id<=0{return;}
        if catalog_busy.get_untracked() || (!force && catalog_attempted.get_untracked()) { return; }
        let d = draft.get_untracked();
        let mut body = json!({});
        let entered = secret.get_untracked();
        if let Some(id) = editing.get_untracked().filter(|_|entered.is_empty()) { body["credential_id"]=json!(id); }
        else { body["kind"]=d["kind"].clone(); if !entered.is_empty(){body["key"]=json!(entered);} }
        if is_local() { let url=api::text(&d,"base_url");if !url.trim().is_empty(){body["base_url"]=json!(url.trim());} }
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
        removal_pending.set(None);model_note.set(String::new());
        reveal_generation.update(|n|*n+=1);revealed.set(String::new());reveal_busy.set(false);secret.set(String::new());copy_note.set(String::new());
        catalog_generation.update(|n|*n+=1);catalog_busy.set(false);catalog_attempted.set(false);catalog.set(vec![]);catalog_loaded.set(false);picker_open.set(false);catalog_query.set(String::new());
    });
    let leave_editor = Callback::new(move |()| {if busy.get_untracked(){return;}if create_provider{modal_command(provider_id,"close");return;}clear_private.run(());form_open.set(false);editing.set(None);modal_view.set("list");error.set(String::new());focus_modal(provider_id);});
    let reveal_key = Callback::new(move |copy: bool| {
        let provider_id=active_provider.get_untracked();
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
        if busy.get_untracked() || save_uncertain.get_untracked() {
            return;
        }
        let id = editing.get_untracked();
        if wizard.get_untracked()&&modal_view.get_untracked()=="models"&&api::rows(&draft.get_untracked(),"models").iter().all(|m|m["enabled"]==false){error.set("请先选择至少一个需要启用的模型，或返回列表保留未启用的凭据。".into());return;}
        let mut input=draft.get_untracked();
        if modal_view.get_untracked()=="key" {input["models"]=json!([]);}
        let body = match payload(
            &input,
            &secret.get_untracked(),
            id.is_some(),
        ) {
            Ok(mut b) => {
                if is_local() {
                    let urls = draft.get_untracked()["base_urls"].as_array().map(|a| a.len()).unwrap_or(0);
                    let url = draft.get_untracked()["base_url"].as_str().unwrap_or("").trim().to_string();
                    if !url.is_empty() {b["base_url"]=json!(url);}
                    else if id.is_none() || urls == 1 {error.set("请填写此模型服务的 API URL。".into());return;}

                }
                if id.is_some()&&modal_view.get_untracked()=="models" {
                    b=model_patch(b,&original_models.get_untracked());
                }
                scoped_payload(b,modal_view.get_untracked()=="models")
            }
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let provider_body=if create_provider&&active_provider.get_untracked()<=0 {
            match new_provider_payload(&draft.get_untracked()){Ok(body)=>Some(body),Err(message)=>{error.set(message);return;}}
        }else{None};
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            let mut provider_id=active_provider.get_untracked();
            if let Some(provider_body)=provider_body {
                match api::request("POST","/api/providers",Some(provider_body)).await {
                    Ok(value)=>if let Some(id)=value["provider"]["id"].as_i64().filter(|id|*id>0){provider_id=id;active_provider.set(id);changed.set(true);}else{error.set("渠道保存响应缺少标识，结果尚未确认；请关闭并刷新列表检查，勿重复创建。".into());save_uncertain.set(true);changed.set(true);busy.set(false);return;},
                    Err(e)=>{ctx.note_err(&e);if e.status==0||e.status>=500{save_uncertain.set(true);changed.set(true);error.set(format!("{}；渠道保存结果尚未确认，请关闭并刷新列表检查，勿重复创建。",e.message));}else{error.set(e.message);}busy.set(false);return;}
                }
            }
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
                Ok(v) => {
                    secret.set(String::new());changed.set(true);
                    if wizard.get_untracked() {
                        let stage=modal_view.get_untracked();
                        let row=if v["credential"].is_object(){Some(v["credential"].clone())}else{
                            match api::request("GET",&format!("/api/providers/{provider_id}/credentials"),None).await {
                                Ok(list)=>api::rows(&list,"credentials").into_iter().find(|r|r["id"].as_i64()==id),
                                Err(e)=>{ctx.note_err(&e);error.set(e.message);None}
                            }
                        };
                        if let Some(mut row)=row {
                            let old=draft.get_untracked();for field in ["_model_family","_oauth_provider","_endpoint_mode","_provider_name","_provider_base_url","_oauth_api_base"]{row[field]=old[field].clone();}
                            let Some(credential_id)=row["id"].as_i64()else{save_uncertain.set(true);error.set("已保存，但账号返回缺少凭据标识，请刷新列表。".into());busy.set(false);return;};
                            editing.set(Some(credential_id));original_models.set(api::rows(&row,"models"));
                            parameter_sources.set(api::rows(&row,"source_ids").into_iter().filter_map(|id|id.as_i64()).collect());
                            if is_local(){row["base_url"]=row["base_urls"].as_array().and_then(|a|a.first()).cloned().unwrap_or(Value::Null);}
                            draft.set(row);clear_private.run(());
                            if stage=="models"{parameters_ready.set(true);modal_view.set("parameters");}else{modal_view.set("models");}
                        }else{save_uncertain.set(true);error.set("配置已保存，但无法确认读回结果；请关闭并刷新列表检查，勿重复创建。".into());}
                    }else{
                        editing.set(None);draft.set(fresh());form_open.set(false);clear_private.run(());modal_view.set("list");
                    }
                    rev.update(|r|*r+=1);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    let mut message=e.message;
                    if create_provider && id.is_none() {
                        changed.set(true);
                        message.push_str("；渠道已保留，请关闭并刷新渠道列表检查凭据保存结果，再从“管理上游 Key”继续，勿重复新建渠道。");
                        // An HTTP failure can follow a committed write. Never compensate
                        // by deleting the provider: deletion can cascade to its sources.
                        if e.status==0||e.status>=500 {save_uncertain.set(true);}
                    }
                    error.set(message);
                }
            }
            busy.set(false);
        });
    };
    view! {<div class="provider-key-pool" class:card-body=!create_provider><button class={if create_provider{"primary"}else{"ghost"}} aria-haspopup="dialog" on:click=move |_|{open.set(true);if create_provider{clear_private.run(());save_uncertain.set(false);active_provider.set(0);draft.set(fresh());editing.set(None);original_models.set(vec![]);wizard.set(true);parameters_ready.set(false);parameter_sources.set(vec![]);form_open.set(true);modal_view.set("key");error.set(String::new());}else{modal_view.set("list");}if let Some(cb)=on_open{cb.run(false);}modal_command(provider_id,"showModal");}>{if create_provider{"新建渠道"}else{"管理上游 Key"}}</button>
    <dialog id=format!("provider-key-dialog-{provider_id}") class="provider-key-modal console-content" aria-labelledby=format!("provider-key-title-{provider_id}") on:cancel=move |e: leptos::ev::Event|{if busy.get_untracked(){e.prevent_default();}} on:close=move |_|{clear_private.run(());open.set(false);form_open.set(false);modal_view.set("list");editing.set(None);if changed.get_untracked(){changed.set(false);on_changed.run(());}}><header class="provider-key-modal-head"><h2 id=format!("provider-key-title-{provider_id}")>{move||match modal_view.get(){"models"=>"选择模型","parameters"=>"参数与转发","key"=>if editing.get().is_some(){"编辑接入凭据"}else if create_provider{"新建渠道"}else{"添加模型接入"},_=>"管理上游 Key"}}</h2><button type="button" class="ghost" aria-label={if create_provider{"关闭新建渠道"}else{"关闭上游 Key 管理"}} data-key-focus="" autofocus disabled=move||busy.get() on:click=move |_|modal_command(provider_id,"close")>"关闭"</button></header><div class="provider-key-modal-body"><Show when=move||open.get()><Show when=move||modal_view.get()=="list"><p class="account-help">"每个Key可配置多个模型。并发策略按用户保持Key粘性；Key顺序数值越小越优先。停用保留账单与历史。"</p><Show when=move||loading.get()><p role="status">"正在读取已配置的上游 Key…"</p></Show><Show when=move||!load_error.get().is_empty()><p role="alert">{move||format!("读取上游 Key 失败：{}",load_error.get())}</p><button type="button" class="ghost" on:click=move |_|rev.update(|v|*v+=1)>"重新读取"</button></Show><Show when=move||!loading.get()&&load_error.get().is_empty()&&rows.get().is_empty()><p class="account-help">"此渠道尚未配置上游 Key。点击添加后填写凭据和模型。"</p></Show><div class="table-wrap"><table><thead><tr><th>"Key备注"</th><th>"类型 / 状态"</th><th>"优先顺序"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().enumerate().map(|(position,row)|{let item=row.clone();let model_item=row.clone();let id=row["id"].as_i64().unwrap_or(0);let is_enabled=row["enabled"]==true;view!{<tr><td data-label="Key备注">{crate::provider_api::credential_label(&row,position)}{is_local().then(||view!{<p class="provider-url muted small">{row["base_urls"].as_array().map(|a|a.iter().filter_map(|v|v.as_str()).collect::<Vec<_>>().join(" / ")).unwrap_or_default()}</p>})}</td><td data-label="类型 / 状态">{format!("{} · {}",match row["kind"].as_str(){Some("oauth")=>"账号授权",Some("anonymous")=>"免认证",_=>"API Key"},if row["enabled"]==true{"启用"}else{"停用"})}<p class="muted small">{match row["kind"].as_str(){Some("oauth")=>if row["oauth_account_id"].is_number(){"已绑定授权账号"}else{"授权账号待确认"},Some("anonymous")=>"无需密钥",_=>if row["has_secret"]==true{"密钥已配置"}else{"密钥待配置"}}}</p></td><td data-label="优先顺序">{row["priority"].to_string()}</td><td data-label="操作"><div class="provider-key-row-actions"><button class="ghost" disabled=move||busy.get()||loading.get()||!load_error.get().is_empty() on:click=move |_|{clear_private.run(());modal_view.set("key");form_open.set(true);catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);catalog_error.set(String::new());catalog_query.set(String::new());wizard.set(false);editing.set(Some(id));original_models.set(api::rows(&item,"models"));let mut d=item.clone();if is_local() {d["base_url"]=d["base_urls"].as_array().and_then(|a|if a.len()==1{a.first().cloned()}else{None}).unwrap_or(json!(""));}draft.set(d);secret.set(String::new());error.set(String::new());}>"编辑 Key"</button><button class="ghost" disabled=move||busy.get()||loading.get()||!load_error.get().is_empty() on:click=move |_|{clear_private.run(());modal_view.set("models");form_open.set(true);catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);catalog_error.set(String::new());catalog_query.set(String::new());wizard.set(false);editing.set(Some(id));original_models.set(api::rows(&model_item,"models"));let mut d=model_item.clone();if is_local() {d["base_url"]=d["base_urls"].as_array().and_then(|a|if a.len()==1{a.first().cloned()}else{None}).unwrap_or(json!(""));}draft.set(d);secret.set(String::new());error.set(String::new());}>"选择模型"</button><button class="ghost danger" disabled=move||busy.get() on:click=move |_|{if busy.get_untracked(){return}busy.set(true);leptos::task::spawn_local(async move{let provider_id=active_provider.get_untracked();match api::request("PATCH",&format!("/api/providers/{provider_id}/credentials/{id}"),Some(json!({"enabled":!is_enabled}))).await{Ok(_)=>{rev.update(|r|*r+=1);changed.set(true);},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});}>{if is_enabled{"停用"}else{"启用"}}</button></div></td></tr>}}).collect_view()}</tbody></table></div>
    <button type="button" class="ghost" disabled=move||busy.get()||loading.get()||!load_error.get().is_empty() on:click=move |_|{clear_private.run(());modal_view.set("key");form_open.set(true);catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);catalog_error.set(String::new());catalog_query.set(String::new());wizard.set(true);parameters_ready.set(false);parameter_sources.set(vec![]);editing.set(None);draft.set(fresh());original_models.set(vec![]);secret.set(String::new());error.set(String::new());}>"添加模型接入"</button></Show>
    <Show when=move||!form_open.get()&&!error.get().is_empty()><p role="alert" class="account-error">{move||error.get()}</p></Show><Show when=move||wizard.get()&&form_open.get()><nav class="provider-enrollment-steps" aria-label="添加接入步骤">{[("key","1 家族与认证"),("models","2 选择模型"),("parameters","3 参数与转发")].into_iter().map(|(stage,label)|view!{<button type="button" class:is-active=move||modal_view.get()==stage disabled=move||stage=="models"&&editing.get().is_none()||stage=="parameters"&&!parameters_ready.get() aria-current=move||if modal_view.get()==stage{Some("step")}else{None} on:click=move |_|modal_view.set(stage)>{label}</button>}).collect_view()}</nav><p class="account-help" role="status">{move||match modal_view.get(){"key"=>if editing.get().is_some(){"凭据已保存；调整设置会作用于此已保存接入。"}else{"先保存真实凭据，随后读取该账号的模型目录；此时尚未启用模型。"},"models"=>"凭据已保存。请选择并保存真实模型映射，再检查参数与转发。",_=>"模型配置已保存。默认值仅在客户端未显式指定时生效。"}}</p></Show><Show when=move||form_open.get()&&modal_view.get()!="parameters"><button type="button" class="ghost" data-key-focus="" on:click=move |_|leave_editor.run(())>{if create_provider{"关闭接入流程"}else{"返回 Key 列表"}}</button><Show when=move||editing.get().is_some()><p class="account-help">{move||format!("当前 Key：{}",api::text(&draft.get(),"label"))}</p></Show><form class="account-form provider-key-editor" class:provider-mapping-editor=move||modal_view.get()=="models" on:submit=save on:keydown=move|e|{if e.key()=="Escape"&&picker_open.get_untracked(){e.prevent_default();e.stop_propagation();picker_open.set(false);}}><fieldset disabled=move||busy.get()><Show when=move||modal_view.get()=="key"><h3>{move||if editing.get().is_some(){"编辑接入凭据"}else{"家族与认证"}}</h3><Show when=move||editing.get().is_none()><FamilyPicker draft=draft on_changed=Callback::new(move |()|{clear_private.run(());if create_provider{draft.update(|d|{let current=d["_provider_base_url"].as_str().unwrap_or("");let is_default=super::family_picker::FAMILIES.iter().any(|(f,_)|family_api_base(f)==current);if current.is_empty()||is_default{d["_provider_base_url"]=json!(family_api_base(d["_model_family"].as_str().unwrap_or("")));}});}})/></Show><p class="label">"认证线路"</p><div class="provider-auth-choices"><button type="button" class="provider-auth-choice" class:is-selected=move||draft.get()["kind"]=="api_key" aria-pressed=move||(draft.get()["kind"]=="api_key").to_string() disabled=move||editing.get().is_some() on:click=move |_|{draft.update(|d|{d["kind"]=json!("api_key");d["oauth_account_id"]=Value::Null;});clear_private.run(());}>"API 接入"<span>"使用供应方 API Key"</span></button><Show when=move||!local><button type="button" class="provider-auth-choice" class:is-selected=move||draft.get()["kind"]=="oauth" aria-pressed=move||(draft.get()["kind"]=="oauth").to_string() disabled=move||editing.get().is_some() on:click=move |_|{draft.update(|d|{d["kind"]=json!("oauth");d["_endpoint_mode"]=json!("single");d["oauth_account_id"]=Value::Null;});clear_private.run(());}>"订阅 OAuth"<span>"使用已授权账号与专属模型目录"</span></button></Show><Show when=move||is_local()><button type="button" class="provider-auth-choice" class:is-selected=move||draft.get()["kind"]=="anonymous" aria-pressed=move||(draft.get()["kind"]=="anonymous").to_string() disabled=move||editing.get().is_some() on:click=move |_|{draft.update(|d|d["kind"]=json!("anonymous"));clear_private.run(());}>"本地免密"<span>"仅用于无需认证的模型服务"</span></button></Show></div><Show when=move||create_provider&&editing.get().is_none()><label class="account-field"><span>"渠道名称"</span><input required maxlength="128" placeholder="如 DeepSeek 官方、团队订阅账号" prop:value=move||api::text(&draft.get(),"_provider_name") on:input=move|e|draft.update(|d|d["_provider_name"]=json!(event_target_value(&e)))/></label><Show when=move||draft.get()["kind"]!="oauth"><p class="label">"服务地址类型"</p><div class="provider-auth-choices"><button type="button" class="provider-auth-choice" class:is-selected=move||!is_local() aria-pressed=move||(!is_local()).to_string() on:click=move |_|draft.update(|d|{d["_endpoint_mode"]=json!("single");if d["kind"]=="anonymous"{d["kind"]=json!("api_key");}})>"在线 API"<span>"供应方或兼容服务地址"</span></button><button type="button" class="provider-auth-choice" class:is-selected=move||is_local() aria-pressed=move||is_local().to_string() on:click=move |_|draft.update(|d|d["_endpoint_mode"]=json!("local"))>"本地模型服务"<span>"同一渠道可包含多个地址"</span></button></div><Show when=move||!is_local()><label class="account-field"><span>"供应方 API URL"</span><input required type="url" placeholder="https://api.example.com/v1" prop:value=move||api::text(&draft.get(),"_provider_base_url") on:input=move|e|draft.update(|d|d["_provider_base_url"]=json!(event_target_value(&e)))/><p class="account-help">"默认地址可以修改；第三方兼容服务请填写其实际地址。"</p></label></Show></Show></Show><div class="policy-limits"><label class="account-field"><span>"Key备注"</span><input required maxlength="128" placeholder="如 主账号、备用账号" prop:value=move||api::text(&draft.get(),"label") on:input=move|e|draft.update(|d|d["label"]=json!(event_target_value(&e)))/></label></div>
    <Show when=move||editing.get().is_none()><p class="account-help">"家族、认证来源、供应方接口协议与具体模型共同确定参数规则。尚未完成授权的来源不能绑定成可用账号。"</p></Show><details><summary>"网络与代理"</summary><p class="account-help">"渠道或来源覆盖模型家族默认代理。多家族授权来源请先保存此渠道的网络家族；未配置时按继承策略执行。"</p>{move||{let d=draft.get();let provider_family=d["_model_family"].as_str().unwrap_or("other").to_string();let family=network_policy_family(&d).to_string();let network_help=if d["kind"]=="oauth"{format!("当前认证来源的默认网络家族：{}。首次授权使用此策略；已保存的渠道或模型接入代理覆盖会优先执行。",super::family_picker::family_label(&family))}else{format!("当前模型家族默认策略：{}。已保存的渠道或模型接入代理覆盖会优先执行。",super::family_picker::family_label(&family))};view!{<p class="account-help">{network_help}</p><Show when=move||{active_provider.get()>0}><super::proxy_settings::ProxySettings ctx=ctx scope="provider".to_string() id=active_provider.get_untracked().to_string() network_family=provider_family.clone()/></Show><Show when=move||{active_provider.get()<=0}><p class="account-help">"保存凭据前可设置家族默认代理；保存后可在此覆盖当前渠道。"</p></Show><super::proxy_settings::ProxySettings ctx=ctx scope="family".to_string() id=family/>}}}</details>
    <Show when=move||is_local()><label class="account-field"><span>"模型服务 API URL"</span><input type="url" required=move||editing.get().is_none() placeholder="http://192.168.1.10:8001/v1" prop:value=move||api::text(&draft.get(),"base_url") on:input=move|e|{draft.update(|d|d["base_url"]=json!(event_target_value(&e)));revealed.set(String::new());reveal_generation.update(|n|*n+=1);copy_note.set(String::new());catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);}/></label><p class="account-help">"同一供应方可添加多个服务地址，每个地址分别设置凭据和模型。IP 相同、端口不同也可以。"</p></Show>
    <Show when=move||draft.get()["kind"]=="api_key"><Show when=move||editing.get().is_some()><p class="account-help">"原密钥已配置。查看或复制不会替换密钥；下面留空保存会继续使用原值。"</p><div class="provider-key-original"><label class="account-field"><span>"原 API Key"</span><input readonly autocomplete="off" type=move||if revealed.get().is_empty(){"password"}else{"text"} prop:value=move||if revealed.get().is_empty(){"••••••••••••••••".to_string()}else{revealed.get()}/></label><div class="provider-key-row-actions"><button type="button" class="ghost" aria-disabled=move||reveal_busy.get().to_string() on:click=move |_|reveal_key.run(false)>{move||if revealed.get().is_empty(){"显示原 Key"}else{"隐藏原 Key"}}</button><button type="button" class="ghost" aria-disabled=move||reveal_busy.get().to_string() on:click=move |_|reveal_key.run(true)>"复制原 Key"</button></div><p role="status">{move||copy_note.get()}</p></div></Show><label class="account-field"><span>{move||if editing.get().is_some(){"替换上游Key（留空保留）"}else{"上游API Key"}}</span><input type="password" autocomplete="new-password" prop:value=secret on:input=move|e|{secret.set(event_target_value(&e));revealed.set(String::new());reveal_generation.update(|n|*n+=1);copy_note.set(String::new());catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_attempted.set(false);catalog_busy.set(false);picker_open.set(false);catalog_query.set(String::new());catalog_loaded.set(false);}/></label></Show>
    <Show when=move||draft.get()["kind"]=="oauth"&&editing.get().is_none()><OAuthEnrollment ctx=ctx provider_id=provider_id draft=draft accounts=accounts on_changed=Callback::new(move |()|{catalog_generation.update(|n|*n+=1);catalog.set(vec![]);catalog_loaded.set(false);})/><label class="account-field"><span>"已授权账号"</span><select required prop:value=move||draft.get()["oauth_account_id"].as_i64().map(|n|n.to_string()).unwrap_or_default() on:change=move|e|draft.update(|d|d["oauth_account_id"]=event_target_value(&e).parse::<i64>().map(|n|json!(n)).unwrap_or(Value::Null))><option value="">"请选择已完成安全授权的账号"</option>{move||accounts.get().into_iter().filter(|a|draft.get()["_oauth_inference_available"]!=false&&account_provider(a)==draft.get()["_oauth_provider"].as_str().unwrap_or("openai")).map(|a|view!{<option value=a["id"].to_string()>{api::text(&a,"label")}</option>}).collect_view()}</select></label><p class="account-help">"仅显示当前认证来源的已授权账号。保存绑定后读取该账号的真实模型目录；首次绑定需设置已验证的输入、输出 Token 边界。"</p><div class="policy-limits">{[ ("max_input_tokens","最大输入Token"),("max_output_tokens","最大输出Token")].into_iter().map(|(k,label)|view!{<label class="account-field"><span>{label}</span><input type="number" min="1" step="1" placeholder="已有验证配置时可留空继承" on:input=move|e|draft.update(|d|d[k]=event_target_value(&e).parse::<u64>().ok().filter(|n|*n>0).map(|n|json!(n)).unwrap_or(Value::Null))/></label>}).collect_view()}</div></Show>
    <Show when=move||editing.get().is_some()><div class="policy-limits provider-key-priority"><label class="account-field"><span>"优先顺序（越小越优先）"</span><input type="number" step="1" prop:value=move||draft.get()["priority"].to_string() on:input=move|e|{if let Ok(n)=event_target_value(&e).parse::<i64>(){draft.update(|d|d["priority"]=json!(n));}}/></label><label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["enabled"]==true on:change=move|e|draft.update(|d|d["enabled"]=json!(event_target_checked(&e)))/>"启用此Key"</label></div></Show>
    <label class="account-field"><span>"此Key最大同时请求数（0不限）"</span><input type="number" min="0" step="1" prop:value=move||draft.get()["max_concurrent"].to_string() on:input=move|e|{if let Ok(n)=event_target_value(&e).parse::<u32>(){draft.update(|d|d["max_concurrent"]=json!(n));}}/></label>
    <Show when=move||editing.get().is_none()&&!is_local()><label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["tools_enabled"]==true on:change=move|e|draft.update(|d|{let checked=event_target_checked(&e);d["tools_enabled"]=json!(checked);if checked&&d["tools"].is_null(){d["tools"]=json!({"search":true,"fetch":false,"account_scope":"","qps":5});}})/>"此入口提供搜索 / 网页提取工具"</label><Show when=move||draft.get()["tools_enabled"]==true><div class="account-actions">{[("search","启用搜索接口"),("fetch","启用网页提取接口")].into_iter().map(|(k,label)|view!{<label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["tools"][k]==true on:change=move|e|draft.update(|d|d["tools"][k]=json!(event_target_checked(&e)))/>{label}</label>}).collect_view()}</div><div class="policy-limits"><label class="account-field"><span>"供应方账户限流分组"</span><input required placeholder="同一上游账户使用相同分组名" prop:value=move||draft.get()["tools"]["account_scope"].as_str().unwrap_or("").to_string() on:input=move|e|draft.update(|d|d["tools"]["account_scope"]=json!(event_target_value(&e)))/></label><label class="account-field"><span>"每秒请求数 QPS"</span><input type="number" min="1" max="10000" required prop:value=move||draft.get()["tools"]["qps"].as_u64().unwrap_or(5).to_string() on:input=move|e|{if let Ok(n)=event_target_value(&e).parse::<u32>(){draft.update(|d|d["tools"]["qps"]=json!(n));}}/></label></div><p class="account-help">"按供应方明确能力设置；同一上游账户的Key应共享限流分组。搜索/提取按次计费需在账务页配置真实来源价格，不会从其他Key猜测价格。"</p></Show></Show>
 </Show><Show when=move||modal_view.get()=="models"><h3>"此Key可用模型"</h3><p class="account-help">{move||if is_local(){"先选择或填写上游模型名。本地映射名是客户端调用的名称，留空沿用上游名称。不同端口分别配置模型。"}else{"先选择或填写上游模型名。本地映射名是客户端调用的名称，留空沿用上游名称。取消勾选可停用已配置模型；移除映射使用下方独立按钮，保存后生效。"}}</p>
    <div class="upstream-model-picker"><label class="account-field"><span>"添加上游模型"</span><input id=format!("provider-model-query-{provider_id}") type="text" autocomplete="off" aria-controls=format!("key-model-options-{provider_id}") aria-expanded=move||picker_open.get().to_string() aria-describedby=format!("key-model-help-{provider_id}") placeholder="选择上游模型，或输入名称后按回车" prop:value=catalog_query on:focus=move |_|{picker_open.set(true);discover.run(false);} on:click=move |_|{picker_open.set(true);discover.run(false);} on:input=move|e|{catalog_query.set(event_target_value(&e));picker_open.set(true);} on:keydown=move|e|{if e.key()=="Enter"{e.prevent_default();let name=catalog_query.get_untracked();draft.update(|d|{model_note.set(choose_model(d,&name,true));});catalog_query.set(String::new());}else if e.key()=="Escape"&&picker_open.get_untracked(){e.prevent_default();e.stop_propagation();picker_open.set(false);}}/></label><p id=format!("key-model-help-{provider_id}") class="account-help">"选择可添加或重新启用模型；也可输入名称后按回车。停用保留映射，明确移除后才能重新配置。所有改动保存后生效，目录探测不发送推理请求。"</p>
    <Show when=move||picker_open.get()><div class="upstream-model-options" role="group" aria-label="上游模型选择" id=format!("key-model-options-{provider_id}")><div class="upstream-model-options-head"><span>{move||if catalog_busy.get(){"正在读取此 Key 的模型…"}else{"可选上游模型（可多选）"}}</span><button type="button" class="ghost" aria-label="关闭上游模型列表" on:click=move |_|picker_open.set(false)>"关闭"</button></div><Show when=move||!catalog_error.get().is_empty()><p role="alert">{move||catalog_error.get()}</p></Show><Show when=move||catalog_loaded.get()&&catalog.get().is_empty()><p role="status">"此 Key 未返回模型目录，可直接输入模型名。"</p></Show><Show when=move||catalog_truncated.get()&&catalog_loaded.get()><p class="account-help">"目录可能不完整，未列出的模型可手工输入。"</p></Show><Show when=move||catalog_loaded.get()&&!catalog.get().is_empty()&&!catalog.get().iter().any(|name|name.to_lowercase().contains(&catalog_query.get().to_lowercase()))><p class="account-help">"未找到匹配项，可按回车添加输入的模型名。"</p></Show><div class="upstream-model-choice-list">{move||{let query=catalog_query.get().to_lowercase();catalog.get().into_iter().filter(|n|n.to_lowercase().contains(&query)).map(|name|{let check=name.clone();let locked=name.clone();let status=name.clone();let update=name.clone();view!{<label class="account-check"><input type="checkbox" prop:checked=move||api::rows(&draft.get(),"models").iter().any(|m|(upstream_name(m)==check||m["_picker_origin"].as_str()==Some(check.as_str()))&&m["enabled"]!=false) disabled=move||api::rows(&draft.get(),"models").iter().any(|m|upstream_name(m)==locked&&m["conflict"]==true) on:change=move|e|{let checked=event_target_checked(&e);draft.update(|d|{model_note.set(choose_model(d,&update,checked));});}/><span>{name}{move||if api::rows(&draft.get(),"models").iter().any(|m|upstream_name(m)==status&&m["_picker_origin"].is_null()){"（已配置）"}else{""}}</span></label>}}).collect_view()}}</div><Show when=move||!catalog_busy.get()><button type="button" class="ghost" on:click=move |_|discover.run(true)>"重新读取模型目录"</button></Show></div></Show></div>
    <p role="status" aria-live="polite">{move||model_note.get()}</p>
    {move||api::rows(&draft.get(),"models").into_iter().enumerate().map(|(i,m)|{
        let existing = original_models.get().iter().any(|o|o["model"]==m["model"]);
        let model=api::text(&m,"model");let up=m["upstream"].as_str().filter(|s|!s.is_empty()).unwrap_or(&model).to_string();
        let mapping=if model==up&&!existing{String::new()}else{model.clone()};
        let remove_name=model.clone();let confirm_name=StoredValue::new(model.clone());let pending_name=model.clone();let note_name=StoredValue::new(model.clone());let alias_up=up.clone();
        view!{<div class="policy-limits provider-model-mapping"><label class="account-field"><span>"上游模型名"</span><input required on:keydown=move|e|{if e.key()=="Enter"{e.prevent_default();}else if e.key()=="Escape"&&picker_open.get_untracked(){e.prevent_default();e.stop_propagation();picker_open.set(false);}} readonly=m["conflict"]==true prop:value=up on:change=move|e|draft.update(|d|d["models"][i]["upstream"]=json!(event_target_value(&e)))/></label><label class="account-field"><span>"本地映射名"</span><input id=format!("model-alias-{provider_id}-{i}") readonly=existing||m["conflict"]==true placeholder="留空沿用上游名称" prop:value=mapping on:change=move|e|draft.update(|d|d["models"][i]["model"]=json!(event_target_value(&e)))/></label><label class="account-check"><input type="checkbox" disabled=m["conflict"]==true prop:checked=m["enabled"]!=false on:change=move|e|draft.update(|d|d["models"][i]["enabled"]=json!(event_target_checked(&e)))/>"模型启用"</label><div class="account-actions"><button type="button" class="ghost" disabled=m["conflict"]==true on:click=move |_|{let mut added=None;draft.update(|d|{model_note.set(add_model_alias(d,&alias_up));added=api::rows(d,"models").iter().position(|row|upstream_name(row)==alias_up&&row["model"].as_str().unwrap_or("").is_empty());});if let Some(index)=added{focus_model_control(format!("model-alias-{provider_id}-{index}"));}}>"添加同上游映射"</button>{if existing {view!{<button id=format!("remove-model-{provider_id}-{i}") type="button" class="ghost danger" disabled=m["conflict"]==true on:click=move |_|{removal_pending.set(Some(remove_name.clone()));focus_model_control(format!("confirm-model-remove-{provider_id}-{i}"));}>"移除映射"</button>}.into_any()}else{view!{<button type="button" class="ghost danger" on:click=move |_|{draft.update(|d|{if let Some(a)=d["models"].as_array_mut(){a.remove(i);}});model_note.set("已移除未保存的模型草稿。".into());focus_model_control(format!("provider-model-query-{provider_id}"));}>"移除此模型"</button>}.into_any()}}</div>
        {existing.then(||view!{<p class="account-help">"已有本地名称不直接改名。停用保留配置；移除只解除当前 Key 的映射，不删除调用与账务历史。保存时会检查共享虚拟模型等引用。"</p>})}
        <Show when=move||removal_pending.get().as_deref()==Some(pending_name.as_str())><div role="group" aria-label="确认移除映射"><p role="alert">{format!("确认从当前 Key 移除映射 {}？保存模型配置后生效；历史记录保留。",note_name.get_value())}</p><div class="account-actions"><button id=format!("confirm-model-remove-{provider_id}-{i}") type="button" class="ghost danger" on:click=move |_|{draft.update(|d|remove_mapping(d,&confirm_name.get_value()));removal_pending.set(None);model_note.set("已从草稿移除映射；保存模型配置后生效，取消将保留原配置。".into());focus_model_control(format!("provider-model-query-{provider_id}"));}>"确认移除映射"</button><button type="button" class="ghost" on:click=move |_|{removal_pending.set(None);focus_model_control(format!("remove-model-{provider_id}-{i}"));}>"保留映射"</button></div></div></Show>{(m["conflict"]==true).then(||view!{<p class="account-help">"旧来源映射存在差异，保留原配置；可先独立编辑 Key，模型差异请在诊断与分组中核对。"</p>})}</div>}}).collect_view()}
    </Show><p role="alert" class="account-error">{move||error.get()}</p><div class="account-actions provider-editor-footer"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|leave_editor.run(())>"取消"</button><button type="submit" class="primary" disabled=move||save_uncertain.get()>{move||if busy.get(){"正在保存…"}else if wizard.get()&&modal_view.get()=="models"{"保存模型并继续"}else if wizard.get(){"保存凭据并继续"}else if modal_view.get()=="models"{"保存模型配置"}else{"保存 Key"}}</button></div></fieldset></form></Show><Show when=move||form_open.get()&&modal_view.get()=="parameters"><ConnectionParameters ctx=ctx source_ids=parameter_sources.get_untracked()/><div class="account-actions provider-editor-footer"><button type="button" class="ghost" on:click=move |_|modal_view.set("models")>"← 返回模型选择"</button><button type="button" class="primary" on:click=move |_|leave_editor.run(())>"完成接入配置"</button></div></Show></Show></div></dialog></div>}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copilot_authorization_uses_multi_family_network_instead_of_selected_model() {
        let mut d=json!({"kind":"oauth","_oauth_provider":"copilot","_model_family":"gpt"});
        assert_eq!(network_policy_family(&d),"other");
        d["_model_family"]=json!("claude");assert_eq!(network_policy_family(&d),"other");
        d["_oauth_provider"]=json!("openai");assert_eq!(network_policy_family(&d),"gpt");
        d["_oauth_provider"]=json!("kimi");assert_eq!(network_policy_family(&d),"kimi");
        d["_oauth_provider"]=json!("minimax-cn");assert_eq!(network_policy_family(&d),"minimax");
        d["kind"]=json!("api_key");assert_eq!(network_policy_family(&d),"claude");
    }
    #[test]
    fn channel_creation_uses_explicit_address_for_each_authentication_line() {
        let mut d=fresh();
        assert!(new_provider_payload(&d).is_err());
        d["_provider_name"]=json!("team");
        d["_provider_base_url"]=json!("https://compatible.example/v1");
        assert_eq!(new_provider_payload(&d).unwrap()["base_url"],"https://compatible.example/v1");
        d["kind"]=json!("oauth");
        assert!(new_provider_payload(&d).is_err());
        d["_oauth_api_base"]=json!("https://api.kimi.com/coding/v1");
        assert_eq!(new_provider_payload(&d).unwrap()["base_url"],"https://api.kimi.com/coding/v1");
        d["kind"]=json!("anonymous");d["_endpoint_mode"]=json!("local");
        let local=new_provider_payload(&d).unwrap();assert_eq!(local["endpoint_mode"],"local");assert!(local["base_url"].is_null());
    }
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
    fn directory_selection_restores_availability_without_renaming_existing() {
        let mut d=json!({"models":[{"model":"existing-local","upstream":"vendor/old","enabled":false}]});
        choose_model(&mut d,"vendor/new",true);
        assert_eq!(d["models"][1]["model"],"");
        assert_eq!(d["models"][1]["upstream"],"vendor/new");
        choose_model(&mut d,"vendor/old",true);
        assert_eq!(api::rows(&d,"models").len(),2);
        assert_eq!(d["models"][0]["enabled"],true);
        assert_eq!(d["models"][0]["model"],"existing-local");
        choose_model(&mut d,"vendor/old",false);
        assert_eq!(api::rows(&d,"models").len(),2);
        assert_eq!(d["models"][0]["enabled"],false);
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
    fn oauth_family_selection_does_not_become_a_model_capability() {
        let mut d=json!({"label":"subscription","kind":"oauth","_model_family":"kimi","_oauth_provider":"kimi","models":[]});
        assert!(payload(&d,"",false).is_err());
        d["oauth_account_id"]=json!(1);
        let body=payload(&d,"",false).unwrap();assert_eq!(body["oauth_account_id"],1);
        assert!(body.get("_model_family").is_none());assert!(body.get("_oauth_provider").is_none());
        assert!(payload(&d,"",true).is_ok());
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
    fn saved_removal_is_an_explicit_patch_and_leaves_unmodified_mappings_out() {
        let original=vec![json!({"model":"old","upstream":"vendor/a","enabled":true}),json!({"model":"keep","upstream":"vendor/b","enabled":true})];
        let mut draft=json!({"label":"primary","kind":"api_key","models":original});
        remove_mapping(&mut draft,"old");
        let body=scoped_payload(model_patch(payload(&draft,"",true).unwrap(),&original),true);
        assert_eq!(body,json!({"remove_models":["old"]}));
        assert_eq!(original.len(),2); // Cancel discards the draft; stored response is untouched.
    }
    #[test]
    fn removal_then_readding_same_name_never_emits_delete_and_update_together() {
        let original=vec![json!({"model":"vendor/a","upstream":"","enabled":false})];
        let mut draft=json!({"label":"primary","kind":"api_key","models":original});
        remove_mapping(&mut draft,"vendor/a");choose_model(&mut draft,"vendor/a",true);
        let body=scoped_payload(model_patch(payload(&draft,"",true).unwrap(),&original),true);
        assert!(body.get("remove_models").is_none());
        assert_eq!(body["models"],json!([{"model":"vendor/a","upstream":"","enabled":true}]));
    }
    #[test]
    fn equivalent_alias_spelling_is_not_both_removed_and_updated() {
        let original=vec![json!({"model":"DeepSeek-V4.1-Flash","upstream":"vendor/model","enabled":false})];
        let draft=json!({"label":"primary","kind":"api_key","models":[{"model":"deepseek_v4.1_flash","upstream":"vendor/model","enabled":true}]});
        let body=model_patch(payload(&draft,"",true).unwrap(),&original);
        assert!(body.get("remove_models").is_none());assert_eq!(api::rows(&body,"models").len(),1);
        assert!(payload(&json!({"label":"k","kind":"anonymous","models":[{"model":"my-model","upstream":"a"},{"model":"MY_model","upstream":"b"}]}),"",false).is_err());
    }
    #[test]
    fn one_upstream_can_have_another_alias_without_changing_saved_alias() {
        let mut draft=json!({"label":"primary","kind":"api_key","models":[{"model":"saved","upstream":"vendor/a","enabled":true}]});
        assert!(add_model_alias(&mut draft,"vendor/a").contains("草稿"));
        assert_eq!(api::rows(&draft,"models").len(),2);
        assert!(add_model_alias(&mut draft,"vendor/a").contains("先填写"));
        assert_eq!(api::rows(&draft,"models").len(),2);
        draft["models"][1]["model"]=json!("another");
        let body=payload(&draft,"",true).unwrap();
        assert_eq!(body["models"][0]["model"],"saved");
        assert_eq!(body["models"][1],json!({"model":"another","upstream":"vendor/a","enabled":true}));
    }
    #[test]
    fn restoring_disabled_model_and_adding_existing_model_both_give_feedback() {
        let mut draft=json!({"models":[{"model":"saved","upstream":"vendor/a","enabled":false}]});
        assert!(choose_model(&mut draft,"vendor/a",true).contains("重新启用"));
        assert_eq!(draft["models"][0]["enabled"],true);
        assert!(choose_model(&mut draft,"vendor/a",true).contains("已有启用映射"));
        assert_eq!(api::rows(&draft,"models").len(),1);
    }
    #[test]
    fn key_metadata_patch_never_contains_model_removals() {
        let body=json!({"label":"k","remove_models":["old"],"models":[]});
        assert_eq!(scoped_payload(body.clone(),true),json!({"models":[],"remove_models":["old"]}));
        assert_eq!(scoped_payload(body,false),json!({"label":"k"}));
    }
    #[test]
    fn new_api_credential_requires_secret() {
        assert!(payload(&json!({"label":"key","kind":"api_key"}), "", false).is_err());
    }
}
