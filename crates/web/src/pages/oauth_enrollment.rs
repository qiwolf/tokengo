//! Real administrator-initiated account authorization; credentials never enter browser storage.
use crate::{Ctx, admin_api as api};
use leptos::prelude::*;
use serde_json::{Value, json};

pub fn account_provider(account: &Value) -> &str {
    account["provider"].as_str().unwrap_or("openai")
}
pub fn provider_matches_family(provider: &Value, family: &str) -> bool {
    provider["families"].as_array().is_some_and(|rows|rows.iter().any(|v|v.as_str()==Some(family)))
}
fn provider_reason(provider:&Value)->String {
    match provider["reason"].as_str().unwrap_or("") {
        "oauth_client_registration_required"=>format!("请先为 TokenGo 注册此供应方的 OAuth 客户端，并在服务器配置 {}。",provider["configuration_key"].as_str().unwrap_or("相应客户端标识")),
        "oauth_transport_not_available"=>"可以完成账号认证，但模型服务传输尚未接通；不能将此账号绑定为推理来源。".into(),
        "registered_device_flow"=>"使用 TokenGo 自有客户端的设备授权流程，成功后绑定当前来源的专属模型目录。".into(),
        "independent_registration_and_transport_required"=>"此来源还需要独立的客户端注册和模型传输实现，目前不能开始授权。".into(),
        "public_oauth_contract_not_confirmed"=>"尚未确认可供本系统使用的公开账号授权协议，请使用已支持的 API 接入。".into(),
        "official_client_only_not_gateway_binding"=>"此订阅授权用于官方客户端，不能直接绑定为本系统的第三方模型来源。".into(),
        "upstream_oauth_retired"=>"供应方已停用旧 OAuth 线路，请使用当前 API / Coding Plan Key。".into(),
        "existing_independent_registration"=>"使用独立客户端注册与 PKCE 授权；不会复用其他应用的令牌。".into(),
        other=>other.replace('_'," "),
    }
}
fn device_method(provider: &Value) -> bool {
    matches!(provider["method"].as_str(),Some("device"|"device_code"|"device-code"))
}
fn safe_link(raw: &str) -> Option<String> {
    if raw.len()>8192{return None;}
    let url=web_sys::Url::new(raw).ok()?;
    (url.protocol()=="https:" && url.username().is_empty() && url.password().is_empty() && raw.len()<=8192).then(||url.href())
}
fn callback_result(raw: &str, redirect: &str, attempt: &str) -> Result<Value,String> {
    if raw.len()>16384||raw.chars().any(char::is_control){return Err("回调地址过长或包含无效控制字符。".into());}
    let url=web_sys::Url::new(raw).map_err(|_|"请粘贴浏览器最终显示的完整回调地址。")?;
    let expected=web_sys::Url::new(redirect).map_err(|_|"回调地址配置无效，请重新开始授权。")?;
    if url.origin()!=expected.origin() || url.pathname()!=expected.pathname() || !url.username().is_empty() || !url.password().is_empty() || !url.hash().is_empty() {
        return Err("返回地址与本次授权的回调地址不一致，请不要提交其他授权结果。".into());
    }
    let search=url.search();
    let mut out=json!({"attempt_id":attempt});
    for pair in search.trim_start_matches('?').split('&').filter(|s|!s.is_empty()) {
        let (key,value)=pair.split_once('=').unwrap_or((pair,""));
        let key=js_sys::decode_uri_component(&key.replace('+'," ")).map_err(|_|"回调参数编码无效。")?.as_string().ok_or("回调参数编码无效。")?;
        if !matches!(key.as_str(),"state"|"code"|"client_id"|"error") {continue;}
        if out.get(&key).is_some() {return Err("回调包含重复的授权参数。".into());}
        let value=js_sys::decode_uri_component(&value.replace('+'," ")).map_err(|_|"回调参数编码无效。")?.as_string().ok_or("回调参数编码无效。")?;
        out[&key]=json!(value);
    }
    if out["state"].as_str().is_none_or(str::is_empty) || (out["code"].as_str().is_none_or(str::is_empty)&&out["error"].as_str().is_none_or(str::is_empty)) {
        return Err("回调缺少 state 或授权结果；不要粘贴访问令牌。".into());
    }
    Ok(out)
}
async fn invalidate_attempt(id:String, device:bool) {
    let (path,body)=if device {(format!("/api/oauth/device/{id}/cancel"),json!({}))}
        else {("/api/oauth/cancel".to_string(),json!({"attempt_id":id}))};
    let _=api::request("POST",&path,Some(body)).await;
}
#[component]
pub fn OAuthEnrollment(ctx: Ctx, provider_id: i64, draft: RwSignal<Value>, accounts: RwSignal<Vec<Value>>, on_changed: Callback<()>) -> impl IntoView {
    let providers=RwSignal::new(Vec::<Value>::new());
    let loading=RwSignal::new(true);
    let error=RwSignal::new(String::new());
    let note=RwSignal::new(String::new());
    let flow=RwSignal::new(Value::Null);
    let busy=RwSignal::new(false);
    let generation=RwSignal::new(0u64);
    let next_poll=RwSignal::new(0i64);
    let interval=RwSignal::new(5i64);
    let redirect=RwSignal::new("http://127.0.0.1:1455/auth/callback".to_string());
    let returned=RwSignal::new(String::new());
    let load_generation=RwSignal::new(0u64);
    let clock=RwSignal::new(crate::now_ms());
    let load=Callback::new(move |()| {
        load_generation.update(|v|*v+=1);let load_id=load_generation.get_untracked();
        loading.set(true);error.set(String::new());
        leptos::task::spawn_local(async move {
            let response=api::request("GET","/api/oauth/providers",None).await;
            if load_generation.try_get_untracked()!=Some(load_id){return;}
            match response {
                Ok(v)=>providers.set(api::rows(&v,"providers")),
                Err(e)=>{ctx.note_err(&e);error.set(e.message);}
            }
            loading.set(false);
        });
    });
    load.run(());
    let cancel=Callback::new(move |()| {
        generation.update(|v|*v+=1);busy.set(false);returned.set(String::new());
        let old=flow.get_untracked();flow.set(Value::Null);next_poll.set(0);
        if let Some(id)=old["attempt_id"].as_str().map(str::to_owned) {
            let device=old["_device"]==true;
            leptos::task::spawn_local(async move{invalidate_attempt(id,device).await;});
        }
    });
    let accept_account=Callback::new(move |account:Value| {
        let Some(id)=account["id"].as_i64().filter(|id|*id>0)else{error.set("授权响应缺少有效账号，请刷新账号列表。".into());return;};
        if account["status"]!="active" {error.set("账号尚未处于可绑定状态。".into());return;}
        let selected=draft.get_untracked()["_oauth_provider"].as_str().unwrap_or("openai").to_owned();
        if account_provider(&account)!=selected {error.set("授权来源已变化，请重新选择账号。".into());return;}
        let inference=providers.get_untracked().into_iter().find(|p|p["id"].as_str()==Some(selected.as_str())).is_some_and(|p|p["inference_available"]==true);
        accounts.update(|rows|{rows.retain(|a|a["id"].as_i64()!=Some(id));rows.push(account);});
        flow.set(Value::Null);returned.set(String::new());
        if inference {draft.update(|d|d["oauth_account_id"]=json!(id));note.set("授权完成，已选中此账号。确认来源边界后保存绑定。".into());}
        else {draft.update(|d|d["oauth_account_id"]=Value::Null);note.set("账号认证已完成；此来源的模型传输尚未接通，当前不能绑定模型接入。".into());}
        on_changed.run(());
    });
    let poll=Callback::new(move |()| {
        let current=flow.get_untracked();
        if current["_device"]!=true || busy.get_untracked() || crate::now_ms()<next_poll.get_untracked(){return;}
        if current["expires_at"].as_i64().is_some_and(|expires|expires<=crate::now_ms()) {flow.set(Value::Null);note.set("本次授权已过期，请重新开始。".into());return;}
        let Some(id)=current["attempt_id"].as_str().map(str::to_owned)else{return;};
        let g=generation.get_untracked();busy.set(true);next_poll.set(crate::now_ms()+interval.get_untracked()*1000);
        leptos::task::spawn_local(async move {
            let response=api::request("POST",&format!("/api/oauth/device/{id}/poll"),Some(json!({}))).await;
            if generation.try_get_untracked()!=Some(g){return;}
            match response {
                Ok(v)=>match v["status"].as_str() {
                    Some("complete")=>{if let Some(a)=v.get("account"){accept_account.run(a.clone());}else{error.set("授权完成响应缺少账号。".into());flow.set(Value::Null);}},
                    Some("slow_down")=>{let wait=v["interval"].as_i64().unwrap_or(interval.get_untracked()+5).clamp(1,3600);interval.set(wait);next_poll.set(crate::now_ms()+wait*1000);note.set("供应方要求降低检查频率，稍后自动继续。".into());},
                    Some("pending")=>{let wait=v["interval"].as_i64().unwrap_or(interval.get_untracked()).clamp(1,3600);interval.set(wait);next_poll.set(crate::now_ms()+wait*1000);},
                    Some("expired"|"denied"|"canceled")=>{flow.set(Value::Null);note.set("授权已过期或被拒绝，请重新开始。".into());},
                    _=>{flow.set(Value::Null);error.set("无法识别供应方授权状态，请重新开始。".into());}
                },
                Err(e)=>{ctx.note_err(&e);error.set(e.message);next_poll.set(crate::now_ms()+interval.get_untracked()*1000);}
            }
            busy.set(false);
        });
    });
    let timer=crate::every(1000,move||{clock.set(crate::now_ms());poll.run(());});
    on_cleanup(move||{crate::stop_timer(timer);cancel.run(());});
    // Family or provider changes invalidate pending work before it can select an old account.
    let previous=RwSignal::new((String::new(),String::new()));
    Effect::new(move |_| {
        let d=draft.get();let family=d["_model_family"].as_str().unwrap_or("").to_owned();
        let selected=d["_oauth_provider"].as_str().unwrap_or("").to_owned();
        let identity=(family.clone(),selected.clone());
        if previous.get_untracked()!=identity {cancel.run(());note.set(String::new());error.set(String::new());previous.set(identity);}
        let compatible=providers.get().into_iter().filter(|p|provider_matches_family(p,&family)).collect::<Vec<_>>();
        let inference=compatible.iter().find(|p|p["id"].as_str()==Some(selected.as_str())).is_some_and(|p|p["inference_available"]==true);
        if draft.get_untracked()["_oauth_inference_available"]!=json!(inference){draft.update(|d|d["_oauth_inference_available"]=json!(inference));}
        if !compatible.iter().any(|p|p["id"].as_str()==Some(selected.as_str())) {
            let first=compatible.iter().find(|p|p["available"]==true).or_else(||compatible.first()).and_then(|p|p["id"].as_str()).map(str::to_owned);
            if let Some(first)=first {draft.update(|d|{d["_oauth_provider"]=json!(first);d["oauth_account_id"]=Value::Null;});}
        }
    });
    let start=move |_| {
        let d=draft.get_untracked();let selected=d["_oauth_provider"].as_str().unwrap_or("").to_owned();
        let Some(provider)=providers.get_untracked().into_iter().find(|p|p["id"].as_str()==Some(&selected)&&p["available"]==true)else{error.set("此认证线路尚不可开始，请查看来源说明。".into());return;};
        let label=d["label"].as_str().unwrap_or("").trim().to_string();
        if label.is_empty(){error.set("请先填写接入备注，作为授权账号的备注。".into());return;}
        cancel.run(());let g=generation.get_untracked();busy.set(true);error.set(String::new());note.set(String::new());
        let device=device_method(&provider);let redirect_uri=redirect.get_untracked();
        leptos::task::spawn_local(async move {
            let (path,body)=if device{("/api/oauth/device/start",json!({"provider":selected,"label":label,"provider_id":provider_id}))}else{("/api/oauth/start",json!({"label":label,"redirect_uri":redirect_uri,"account_id":null,"provider_id":provider_id}))};
            let result=api::request("POST",path,Some(body)).await;
            if generation.try_get_untracked()!=Some(g){
                if let Ok(v)=result {if let Some(id)=v["attempt_id"].as_str(){invalidate_attempt(id.to_owned(),device).await;}}
                return;
            }
            match result {
                Ok(mut v)=>{
                    v["_device"]=json!(device);
                    let link=if device{v["verification_uri_complete"].as_str().or_else(||v["verification_uri"].as_str())}else{v["authorization_url"].as_str()};
                    if link.and_then(safe_link).is_none(){if let Some(id)=v["attempt_id"].as_str(){invalidate_attempt(id.to_owned(),device).await;}error.set("供应方返回的授权链接无效；没有打开浏览器。".into());}
                    else{let wait=v["interval"].as_i64().unwrap_or(5).clamp(1,3600);interval.set(wait);next_poll.set(crate::now_ms()+wait*1000);flow.set(v);note.set("请主动打开官方授权页，在供应方页面完成登录与确认。".into());}
                },
                Err(e)=>{ctx.note_err(&e);error.set(e.message);}
            }
            busy.set(false);
        });
    };
    let complete=move |_| {
        let current=flow.get_untracked();let Some(id)=current["attempt_id"].as_str()else{return;};
        let body=match callback_result(&returned.get_untracked(),&redirect.get_untracked(),id){Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        let g=generation.get_untracked();busy.set(true);error.set(String::new());
        leptos::task::spawn_local(async move {
            let result=api::request("POST","/api/oauth/complete",Some(body)).await;
            if generation.try_get_untracked()!=Some(g){return;}
            returned.set(String::new());
            match result{Ok(v)=>{if let Some(a)=v.get("account"){accept_account.run(a.clone());}else{error.set("授权响应缺少账号。".into());}},Err(e)=>{ctx.note_err(&e);error.set(e.message);flow.set(Value::Null);}}
            busy.set(false);
        });
    };
    view!{<section class="oauth-enrollment" aria-label="账号授权">
        <Show when=move||loading.get()><p role="status">"正在读取可用认证线路…"</p></Show>
        <Show when=move||!loading.get()&&providers.get().iter().all(|p|!provider_matches_family(p,draft.get()["_model_family"].as_str().unwrap_or("")))><p class="account-help">"此家族暂未接通可用的账号授权，请使用 API 接入。"</p></Show>
        <div class="oauth-source-list">{move||{let family=draft.get()["_model_family"].as_str().unwrap_or("").to_owned();providers.get().into_iter().filter(|p|provider_matches_family(p,&family)).map(|p|{
            let id=api::text(&p,"id");let choose=id.clone();let selected=id.clone();let label=api::text(&p,"label");let available=p["available"]==true;
            let docs=p["docs_url"].as_str().and_then(safe_link);let reason=provider_reason(&p);let inference=p["inference_available"]==true;let api_base=api::text(&p,"api_base");let native=api::text(&p,"native_protocol");
            view!{<div class="oauth-source" class:is-selected=move||draft.get()["_oauth_provider"].as_str()==Some(&selected)><label class="oauth-source-head"><input type="radio" name="oauth-provider" prop:checked=move||draft.get()["_oauth_provider"].as_str()==Some(&id) on:change=move |_|draft.update(|d|{d["_oauth_provider"]=json!(choose);d["oauth_account_id"]=Value::Null;})/><strong>{label}</strong><span class="status-pill">{if available&&inference{"可开始授权"}else if available{"可认证 · 传输待接入"}else{"尚不可用"}}</span></label><p class="account-help">{reason}</p>{(!api_base.is_empty()).then(||view!{<p class="account-help">{format!("供应方接口：{api_base} · {native}；保存绑定时会核对当前渠道地址。")}</p>)}{docs.map(|url|view!{<a href=url target="_blank" rel="noopener noreferrer">"官方说明 ↗"</a>})}</div>}
        }).collect_view()}}</div>
        <Show when=move||draft.get()["_oauth_provider"]=="openai"><label class="account-field"><span>"本机回调地址"</span><input type="url" prop:value=redirect on:input=move|e|redirect.set(event_target_value(&e)) disabled=move||!flow.get().is_null()/><span class="account-help">"授权结束后将浏览器最终回调地址粘贴到下方。服务端会校验 PKCE、state 和账号身份；不接收访问令牌。"</span></label></Show>
        <div class="account-actions"><button type="button" class="ghost" on:click=start disabled=move||busy.get()||!flow.get().is_null()||!providers.get().iter().any(|p|p["id"]==draft.get()["_oauth_provider"]&&p["available"]==true)>"开始新账号授权"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load.run(())>"刷新认证线路"</button></div>
        <Show when=move||!flow.get().is_null()><div class="oauth-pending"><p>"本次账号授权进行中"</p><Show when=move||flow.get()["_device"]==true><p class="account-help">"在官方页面输入设备验证码："</p><code>{move||api::text(&flow.get(),"user_code")}</code></Show>
            {move||{let f=flow.get();let raw=if f["_device"]==true{f["verification_uri_complete"].as_str().or_else(||f["verification_uri"].as_str())}else{f["authorization_url"].as_str()};raw.and_then(safe_link).map(|url|view!{<a class="oauth-open" href=url target="_blank" rel="noopener noreferrer">"打开官方授权页 ↗"</a>})}}
            <Show when=move||flow.get()["_device"]==true><p class="account-help">"按供应方要求的间隔自动检查。此页面关闭或切换来源后，本次待授权流程会取消。"</p><button type="button" class="ghost" disabled=move||busy.get()||clock.get()<next_poll.get() on:click=move |_|poll.run(())>"我已完成授权，检查状态"</button></Show>
            <Show when=move||flow.get()["_device"]!=true><label class="account-field"><span>"浏览器最终回调地址"</span><textarea autocomplete="off" spellcheck="false" maxlength="16384" prop:value=returned on:input=move|e|returned.set(event_target_value(&e)) placeholder="http://127.0.0.1:1455/auth/callback?code=…&state=…"/></label><button type="button" class="ghost" disabled=move||busy.get()||returned.get().trim().is_empty() on:click=complete>"验证并完成授权"</button></Show>
            <button type="button" class="ghost" on:click=move |_|{cancel.run(());note.set("已取消本次待授权流程。".into());}>"取消本次授权"</button>
        </div></Show>
        <p class="account-error" role="alert">{move||error.get()}</p><p role="status" class="account-help">{move||note.get()}</p>
    </section>}
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn legacy_accounts_are_openai_and_provider_filter_is_explicit(){
        assert_eq!(account_provider(&json!({"id":1})),"openai");
        assert_eq!(account_provider(&json!({"provider":"kimi"})),"kimi");
        let p=json!({"families":["gpt","claude"]});assert!(provider_matches_family(&p,"claude"));assert!(!provider_matches_family(&p,"kimi"));
        assert!(device_method(&json!({"method":"device_code"})));assert!(!device_method(&json!({"method":"browser"})));
    }
}
