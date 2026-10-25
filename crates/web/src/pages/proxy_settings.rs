//! Root-managed outbound network policy. Proxy credentials are write-only.
use crate::{admin_api as api,Ctx};
use leptos::prelude::*;
use serde_json::{json,Value};
use wasm_bindgen::JsCast;

#[component]
pub fn ProxySettings(ctx:Ctx,scope:String,id:String,#[prop(optional)]network_family:Option<String>)->impl IntoView {
    let scope=StoredValue::new(scope);let id=StoredValue::new(id);let initial_family=StoredValue::new(network_family);
    let policy=RwSignal::new(json!({"version":0,"mode":"inherit","has_credentials":false}));
    let mode=RwSignal::new("inherit".to_owned());let url=RwSignal::new(String::new());
    let family=RwSignal::new(initial_family.get_value().unwrap_or_default());
    let action=RwSignal::new("keep".to_owned());let username=RwSignal::new(String::new());let password=RwSignal::new(String::new());
    let busy=RwSignal::new(false);let loaded=RwSignal::new(false);let error=RwSignal::new(String::new());let message=RwSignal::new(String::new());let revision=RwSignal::new(0u64);let generation=RwSignal::new(0u64);
    Effect::new(move |_|{let _=revision.get();generation.update(|n|*n+=1);let request_generation=generation.get_untracked();loaded.set(false);error.set(String::new());leptos::task::spawn_local(async move {
        let response=api::request("GET","/api/network/proxy-policies",None).await;
        if generation.try_get_untracked()!=Some(request_generation){return;}
        match response {
            Ok(value)=>{let row=value["policies"].as_array().and_then(|rows|rows.iter().find(|p|p["scope"].as_str()==Some(scope.get_value().as_str())&&p["id"].as_str()==Some(id.get_value().as_str()))).cloned().unwrap_or_else(||json!({"version":0,"mode":"inherit","has_credentials":false}));mode.set(row["mode"].as_str().unwrap_or("inherit").into());url.set(row["proxy_url"].as_str().unwrap_or("").into());family.set(row["network_family"].as_str().map(str::to_owned).or(initial_family.get_value()).unwrap_or_default());policy.set(row);loaded.set(true);},
            Err(e)=>{ctx.note_err(&e);error.set("读取代理策略失败；未修改配置。".into());}
        }
    });});
    let save=move |e:leptos::ev::MouseEvent|{e.prevent_default();e.stop_propagation();if busy.get_untracked()||!loaded.get_untracked(){return;}busy.set(true);error.set(String::new());message.set(String::new());
        let save_generation=generation.get_untracked();let mut body=json!({"mode":mode.get_untracked(),"proxy_url":if mode.get_untracked()=="proxy"{Some(url.get_untracked())}else{None},"credential_action":action.get_untracked(),"expected_version":policy.get_untracked()["version"].as_u64().unwrap_or(0)});
        if scope.get_value()!="family"{body["network_family"]=if family.get_untracked().is_empty(){Value::Null}else{json!(family.get_untracked())};}
        if action.get_untracked()=="replace"{body["username"]=json!(username.get_untracked());body["password"]=json!(password.get_untracked());}
        username.set(String::new());password.set(String::new());
        let path=format!("/api/network/proxy-policies/{}/{}",scope.get_value(),id.get_value());leptos::task::spawn_local(async move {let response=api::request("PUT",&path,Some(body)).await;if generation.try_get_untracked()!=Some(save_generation){return;}match response {
            Ok(row)=>{policy.set(row);action.set("keep".into());message.set("代理策略已保存；后续授权、刷新、目录和推理请求使用新策略。".into());},
            Err(e)=>{ctx.note_err(&e);error.set(format!("保存失败：{}",e.message));}
        }busy.set(false);});
    };
    view!{<section class="network-proxy-settings" on:keydown=move |e|{if e.key()=="Enter"{e.stop_propagation();if e.target().is_some_and(|target|target.dyn_into::<web_sys::HtmlInputElement>().is_ok()){e.prevent_default();}}}><h4>{move||match scope.get_value().as_str(){"family"=>"家族默认代理","source"=>"此模型接入的代理",_=>"此渠道的代理"}}</h4><p class="account-help">"优先级：模型接入 → 渠道 → 家族默认 → 系统网络。继承会使用下一层策略；直连会关闭环境代理。"</p><div class="account-form"><fieldset disabled=move||busy.get()||!loaded.get()><label class="account-field"><span>"连接方式"</span><select prop:value=mode on:change=move|e|mode.set(event_target_value(&e))><option value="inherit">"继承上层策略"</option><option value="direct">"直连（不使用代理）"</option><option value="proxy">"指定 HTTP / HTTPS 代理"</option></select></label><Show when=move||scope.get_value()!="family"><label class="account-field"><span>"网络家族"</span><select prop:value=family on:change=move|e|family.set(event_target_value(&e))><option value="">"按官方服务地址识别 / 不指定"</option>{[("gpt","GPT"),("claude","Claude"),("gemini","Gemini"),("deepseek","DeepSeek"),("qwen","Qwen"),("glm","GLM"),("kimi","Kimi"),("mimo","MiMo"),("minimax","MiniMax"),("doubao","Doubao"),("grok","Grok"),("mistral","Mistral"),("llama","Meta / Llama"),("cohere","Cohere"),("other","其他 / 多家族")].into_iter().map(|(value,label)|view!{<option value=value>{label}</option>}).collect_view()}</select><p class="account-help">"兼容代理和多家族授权来源请明确保存网络家族；不会根据模型别名猜测。"</p></label></Show><Show when=move||mode.get()=="proxy"><label class="account-field"><span>"代理地址"</span><input type="url" required placeholder="http://127.0.0.1:7890" prop:value=url on:input=move|e|url.set(event_target_value(&e))/><p class="account-help">"只支持 HTTP / HTTPS。地址中不要填写用户名、密码、查询参数或路径。"</p></label></Show><Show when=move||mode.get()=="proxy"||policy.get()["has_credentials"]==true><label class="account-field"><span>"代理认证"</span><select prop:value=action on:change=move|e|action.set(event_target_value(&e))><option value="keep">"保留现有认证"</option><option value="replace">"设置 / 替换认证"</option><option value="clear">"清除认证"</option></select><p class="account-help">{move||if policy.get()["has_credentials"]==true{"已保存认证信息，原值不会回显；更换代理地址或切换直连时请选择清除或替换。"}else{"当前未保存代理认证，可直接连接无需认证的代理。"}}</p></label><Show when=move||action.get()=="replace"><div class="policy-limits"><label class="account-field"><span>"代理用户名"</span><input required autocomplete="off" prop:value=username on:input=move|e|username.set(event_target_value(&e))/></label><label class="account-field"><span>"代理密码"</span><input type="password" autocomplete="new-password" prop:value=password on:input=move|e|password.set(event_target_value(&e))/></label></div></Show></Show></fieldset><p class="account-error" role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p><div class="account-actions"><button type="button" class="primary" on:click=save disabled=move||busy.get()||!loaded.get()>{move||if busy.get(){"正在保存…"}else{"保存代理策略"}}</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|revision.update(|v|*v+=1)>"重新读取"</button></div></div></section>}
}
