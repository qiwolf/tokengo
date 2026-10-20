//! Credential pool editor. Credentials are never returned by the API.
use crate::{Ctx, admin_api as api};
use leptos::prelude::*;
use serde_json::{Value, json};
fn fresh() -> Value {
    json!({"label":"","kind":"api_key","priority":0,"max_concurrent":0,"enabled":true,"models":[]})
}
fn payload(d: &Value, secret: &str, editing: bool) -> Result<Value, String> {
    let label = d["label"].as_str().unwrap_or("").trim();
    if label.is_empty() {
        return Err("请填写Key备注。".into());
    }
    let models=api::rows(d,"models").into_iter().map(|m|json!({"model":m["model"],"upstream":m["upstream"].as_str().unwrap_or(""),"enabled":m["enabled"]!=false})).collect::<Vec<_>>();
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
#[component]
pub fn ProviderKeys(ctx: Ctx, provider_id: i64, on_changed: Callback<()>) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Value>::new());
    let accounts = RwSignal::new(Vec::<Value>::new());
    let open = RwSignal::new(false);
    let editing = RwSignal::new(None::<i64>);
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
                    error.set(e.message);
                }
            }
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
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let id = editing.get_untracked();
        let body = match payload(
            &draft.get_untracked(),
            &secret.get_untracked(),
            id.is_some(),
        ) {
            Ok(mut b) => {
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
                b
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
                    rev.update(|r| *r += 1);
                    on_changed.run(());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<div class="card-body provider-key-pool"><button class="ghost" aria-expanded=move||open.get().to_string() on:click=move |_|open.update(|v|*v=!*v)>{move||if open.get(){"收起上游Key"}else{"管理上游Key"}}</button><Show when=move||open.get()><p class="account-help">"每个Key可配置多个模型。并发策略按用户保持Key粘性；Key顺序数值越小越优先。停用保留账单与历史。"</p><div class="table-wrap"><table><thead><tr><th>"Key备注"</th><th>"类型 / 状态"</th><th>"优先顺序"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|row|{let item=row.clone();let id=row["id"].as_i64().unwrap_or(0);view!{<tr><td data-label="Key备注">{api::text(&row,"label")}</td><td data-label="类型 / 状态">{format!("{} · {}",api::text(&row,"kind"),if row["enabled"]==true{"启用"}else{"停用"})}</td><td data-label="优先顺序">{row["priority"].to_string()}</td><td data-label="操作"><button class="ghost" disabled=move||busy.get() on:click=move |_|{editing.set(Some(id));original_models.set(api::rows(&item,"models"));draft.set(item.clone());secret.set(String::new());error.set(String::new());}>"编辑Key与模型"</button><button class="ghost danger" disabled=move||busy.get() on:click=move |_|{if busy.get_untracked(){return}busy.set(true);leptos::task::spawn_local(async move{match api::request("DELETE",&format!("/api/providers/{provider_id}/credentials/{id}"),None).await{Ok(_)=>{rev.update(|r|*r+=1);on_changed.run(());},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});}>"停用"</button></td></tr>}}).collect_view()}</tbody></table></div>
    <button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{editing.set(None);draft.set(fresh());original_models.set(vec![]);secret.set(String::new());error.set(String::new());}>"添加上游Key"</button>
    <form class="account-form" on:submit=save><fieldset disabled=move||busy.get()><h3>{move||if editing.get().is_some(){"编辑上游Key"}else{"新增上游Key"}}</h3><div class="policy-limits"><label class="account-field"><span>"Key备注"</span><input required maxlength="128" placeholder="如 主账号、备用账号" prop:value=move||api::text(&draft.get(),"label") on:input=move|e|draft.update(|d|d["label"]=json!(event_target_value(&e)))/></label><label class="account-field"><span>"凭据类型"</span><select disabled=move||editing.get().is_some() prop:value=move||api::text(&draft.get(),"kind") on:change=move|e|{draft.update(|d|d["kind"]=json!(event_target_value(&e)));secret.set(String::new());}><option value="api_key">"API Key"</option><option value="oauth">"已授权GPT账号"</option><option value="anonymous">"本地免密服务"</option></select></label></div>
    <Show when=move||draft.get()["kind"]=="api_key"><label class="account-field"><span>{move||if editing.get().is_some(){"替换上游Key（留空保留）"}else{"上游API Key"}}</span><input type="password" autocomplete="new-password" prop:value=secret on:input=move|e|secret.set(event_target_value(&e))/></label></Show>
    <Show when=move||draft.get()["kind"]=="oauth"&&editing.get().is_none()><label class="account-field"><span>"已授权账号"</span><select required prop:value=move||draft.get()["oauth_account_id"].as_i64().map(|n|n.to_string()).unwrap_or_default() on:change=move|e|draft.update(|d|d["oauth_account_id"]=event_target_value(&e).parse::<i64>().map(|n|json!(n)).unwrap_or(Value::Null))><option value="">"请选择已完成安全授权的账号"</option>{move||accounts.get().into_iter().map(|a|view!{<option value=a["id"].to_string()>{api::text(&a,"label")}</option>}).collect_view()}</select></label><p class="account-help">"仅绑定已完成OAuth授权的账号；仅适用于官方GPT OAuth入口。首次绑定需设置模型输入、输出Token上限；留空仅可继承此账号已验证的边界。"</p><div class="policy-limits">{[ ("max_input_tokens","最大输入Token"),("max_output_tokens","最大输出Token")].into_iter().map(|(k,label)|view!{<label class="account-field"><span>{label}</span><input type="number" min="1" step="1" placeholder="已有验证配置时可留空继承" on:input=move|e|draft.update(|d|d[k]=event_target_value(&e).parse::<u64>().ok().filter(|n|*n>0).map(|n|json!(n)).unwrap_or(Value::Null))/></label>}).collect_view()}</div></Show>
    <Show when=move||editing.get().is_some()><div class="policy-limits"><label class="account-field"><span>"优先顺序（越小越优先）"</span><input type="number" step="1" prop:value=move||draft.get()["priority"].to_string() on:input=move|e|{if let Ok(n)=event_target_value(&e).parse::<i64>(){draft.update(|d|d["priority"]=json!(n));}}/></label><label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["enabled"]==true on:change=move|e|draft.update(|d|d["enabled"]=json!(event_target_checked(&e)))/>"启用此Key"</label></div></Show>
    <label class="account-field"><span>"此Key最大同时请求数（0不限）"</span><input type="number" min="0" step="1" prop:value=move||draft.get()["max_concurrent"].to_string() on:input=move|e|{if let Ok(n)=event_target_value(&e).parse::<u32>(){draft.update(|d|d["max_concurrent"]=json!(n));}}/></label>
    <Show when=move||editing.get().is_none()><label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["tools_enabled"]==true on:change=move|e|draft.update(|d|{let checked=event_target_checked(&e);d["tools_enabled"]=json!(checked);if checked&&d["tools"].is_null(){d["tools"]=json!({"search":true,"fetch":false,"account_scope":"","qps":5});}})/>"此入口提供搜索 / 网页提取工具"</label><Show when=move||draft.get()["tools_enabled"]==true><div class="account-actions">{[("search","启用搜索接口"),("fetch","启用网页提取接口")].into_iter().map(|(k,label)|view!{<label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["tools"][k]==true on:change=move|e|draft.update(|d|d["tools"][k]=json!(event_target_checked(&e)))/>{label}</label>}).collect_view()}</div><div class="policy-limits"><label class="account-field"><span>"供应方账户限流分组"</span><input required placeholder="同一上游账户使用相同分组名" prop:value=move||draft.get()["tools"]["account_scope"].as_str().unwrap_or("").to_string() on:input=move|e|draft.update(|d|d["tools"]["account_scope"]=json!(event_target_value(&e)))/></label><label class="account-field"><span>"每秒请求数 QPS"</span><input type="number" min="1" max="10000" required prop:value=move||draft.get()["tools"]["qps"].as_u64().unwrap_or(5).to_string() on:input=move|e|{if let Ok(n)=event_target_value(&e).parse::<u32>(){draft.update(|d|d["tools"]["qps"]=json!(n));}}/></label></div><p class="account-help">"按供应方明确能力设置；同一上游账户的Key应共享限流分组。搜索/提取按次计费需在账务页配置真实来源价格，不会从其他Key猜测价格。"</p></Show></Show>
 <h3>"此Key可用模型"</h3><p class="account-help">"直接添加标准模型名及可选上游映射。模型停用请取消勾选；省略已有模型不会删除历史配置。新Key留空将继承渠道已有模型。"</p>
    {move||api::rows(&draft.get(),"models").into_iter().enumerate().map(|(i,m)|view!{<div class="policy-limits"><label class="account-field"><span>"标准模型名"</span><input required readonly=m["conflict"]==true prop:value=api::text(&m,"model") on:input=move|e|draft.update(|d|d["models"][i]["model"]=json!(event_target_value(&e)))/></label><label class="account-field"><span>"上游模型名（留空使用标准名）"</span><input readonly=m["conflict"]==true prop:value=m["upstream"].as_str().unwrap_or("").to_string() on:input=move|e|draft.update(|d|d["models"][i]["upstream"]=json!(event_target_value(&e)))/></label><label class="account-check"><input type="checkbox" disabled=m["conflict"]==true prop:checked=m["enabled"]!=false on:change=move|e|draft.update(|d|d["models"][i]["enabled"]=json!(event_target_checked(&e)))/>"模型启用"</label>{(m["conflict"]==true).then(||view!{<p class="account-help">"旧来源映射存在差异，保留原配置；可先独立编辑Key，模型差异请在高级诊断中核对。"</p>})}</div>}).collect_view()}
    <button type="button" class="ghost" on:click=move |_|draft.update(|d|{let mut models=api::rows(d,"models");models.push(json!({"model":"","upstream":"","enabled":true}));d["models"]=json!(models);})>"添加模型"</button><p role="alert">{move||error.get()}</p><button type="submit" class="primary">{move||if busy.get(){"正在保存…"}else{"保存上游Key"}}</button></fieldset></form></Show></div>}
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
    fn new_api_credential_requires_secret() {
        assert!(payload(&json!({"label":"key","kind":"api_key"}), "", false).is_err());
    }
}
