//! Step 3 uses saved sources/models; client protocol is an inspection choice, not source identity.
use crate::{Ctx, admin_api as api};
use leptos::prelude::*;
use serde_json::{Value, json};

fn primary(raw: &str) -> String {
    serde_json::from_str::<Value>(raw).ok().and_then(|v|v["native"].as_array().and_then(|v|v.first()).and_then(Value::as_str).map(str::to_owned)).unwrap_or_default()
}
fn choose_primary(raw: &str, selected: &str) -> String {
    let Ok(mut value)=serde_json::from_str::<Value>(raw)else{return raw.into();};
    let Some(native)=value["native"].as_array_mut()else{return raw.into();};
    if let Some(index)=native.iter().position(|v|v.as_str()==Some(selected)) {let item=native.remove(index);native.insert(0,item);}
    value.to_string()
}
fn patch_flag(raw: &str, flag: &str, enabled: bool) -> String {
    let Ok(mut value)=serde_json::from_str::<Value>(raw)else{return raw.into();};
    if value.is_object(){value[flag]=json!(enabled);}value.to_string()
}
fn protocol_label(protocol: &str) -> &str {match protocol{"chat"=>"Chat Completions","responses"=>"Responses","messages"=>"Messages",_=>"尚未确认"}}
fn client_protocol_status(raw:&str, protocol:&str)->(&'static str,bool) {
    let Ok(value)=serde_json::from_str::<Value>(raw)else{return ("不可用",false);};
    if value["native"].as_array().is_some_and(|rows|rows.iter().any(|v|v.as_str()==Some(protocol))){("原生",true)}
    else if value["allow_conversion"]==true{("经转换",true)}else{("不可用",false)}
}
fn effort_key(protocol: &str) -> &str {match protocol{"chat"=>"reasoning_effort","responses"=>"reasoning.effort","messages"=>"output_config.effort",_=>"由来源确认"}}
fn cap_key(protocol: &str) -> &str {match protocol{"chat"=>"max_tokens / max_completion_tokens","responses"=>"max_output_tokens","messages"=>"max_tokens",_=>"由来源确认"}}
#[component]
pub fn ConnectionParameters(ctx: Ctx, source_ids: Vec<i64>) -> impl IntoView {
    let sources=StoredValue::new(source_ids);
    let channels=RwSignal::new(Vec::<Value>::new());
    let selected=RwSignal::new(None::<i64>);
    let model=RwSignal::new(String::new());
    let protocols=RwSignal::new(String::new());
    let client=RwSignal::new("chat".to_string());
    let loading=RwSignal::new(false);let busy=RwSignal::new(false);
    let error=RwSignal::new(String::new());let note=RwSignal::new(String::new());
    let load_generation=RwSignal::new(0u64);
    let load=Callback::new(move |()| {
        load_generation.update(|v|*v+=1);let load_id=load_generation.get_untracked();
        loading.set(true);error.set(String::new());
        leptos::task::spawn_local(async move{
            let result=api::request("GET","/api/channels",None).await;
            if load_generation.try_get_untracked()!=Some(load_id){return;}
            match result{Ok(v)=>{let wanted=sources.get_value();let rows=api::rows(&v,"channels").into_iter().filter(|r|r["id"].as_i64().is_some_and(|id|wanted.contains(&id))).collect::<Vec<_>>();
                if !rows.iter().any(|r|r["id"].as_i64()==selected.get_untracked()){selected.set(rows.first().and_then(|r|r["id"].as_i64()));}channels.set(rows);},
                Err(e)=>{ctx.note_err(&e);error.set(e.message);}}
            loading.set(false);
        });
    });load.run(());
    Effect::new(move |_|{
        let Some(row)=channels.get().into_iter().find(|r|r["id"].as_i64()==selected.get())else{return;};
        let p=row.get("protocols").filter(|v|v.is_object()).cloned().unwrap_or_else(||json!({"native":["chat"],"allow_conversion":false}));
        protocols.set(serde_json::to_string_pretty(&p).unwrap_or_default());
        let models=api::rows(&row,"models");let current=model.get_untracked();
        if !models.iter().any(|m|m["model"].as_str()==Some(&current)) {model.set(models.first().map(|m|api::text(m,"model")).unwrap_or_default());}
    });
    Effect::new(move |_|{
        let raw=protocols.get();let current=client.get();
        if !client_protocol_status(&raw,&current).1 {
            let next=serde_json::from_str::<Value>(&raw).ok().and_then(|v|v["native"].as_array().and_then(|rows|rows.iter().filter_map(Value::as_str).find(|kind|matches!(*kind,"chat"|"responses"|"messages"))).map(str::to_owned));
            if let Some(next)=next.filter(|next|*next!=current){client.set(next);}
        }
    });
    let save=move |event:leptos::ev::SubmitEvent|{
        event.prevent_default();if busy.get_untracked(){return;}
        let Some(id)=selected.get_untracked()else{error.set("请先选择已保存的来源。".into());return;};
        let value=match api::protocols(&protocols.get_untracked()){Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        let Some(expected_generation)=channels.get_untracked().iter().find(|r|r["id"].as_i64()==Some(id)).and_then(|r|r["binding_generation"].as_i64())else{error.set("缺少来源版本，请重新读取配置后保存。".into());return;};
        busy.set(true);error.set(String::new());note.set(String::new());
        leptos::task::spawn_local(async move{
            let result=api::request("PUT",&format!("/api/channels/{id}/protocols"),Some(json!({"protocols":value,"expected_binding_generation":expected_generation}))).await;
            if busy.try_get_untracked().is_none(){return;}
            match result{Ok(_)=>{note.set("供应方接口协议设置已保存；未发送模型推理请求。".into());load.run(());},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);
        });
    };
    view!{<section class="connection-parameters"><h3>"参数与转发"</h3><p class="account-help">"这里显示刚保存的真实来源和模型。客户端接入协议是用户调用本系统的协议；供应方接口协议是本系统请求供应方时使用的协议。"</p>
        <Show when=move||loading.get()><p role="status">"正在读取已保存的来源与模型…"</p></Show>
        <Show when=move||!loading.get()&&channels.get().is_empty()><p role="alert">"没有可配置的已保存来源，请返回列表重新打开此凭据。"</p></Show>
        <Show when=move||{channels.get().len()>1}><label class="account-field"><span>"当前来源"</span><select prop:value=move||selected.get().map(|id|id.to_string()).unwrap_or_default() on:change=move|e|selected.set(event_target_value(&e).parse().ok())>{move||channels.get().into_iter().map(|c|view!{<option value=c["id"].to_string()>{api::text(&c,"name")}</option>}).collect_view()}</select><span class="account-help">"本页只修改所选来源，不批量覆盖同凭据的其他来源。"</span></label></Show>
        <div class="policy-limits"><label class="account-field"><span>"当前模型"</span><select prop:value=model on:change=move|e|model.set(event_target_value(&e))>{move||channels.get().into_iter().find(|r|r["id"].as_i64()==selected.get()).map(|r|api::rows(&r,"models").into_iter().map(|m|{let name=api::text(&m,"model");view!{<option value=name.clone()>{name.clone()}</option>}}).collect_view())}</select></label>
        <label class="account-field"><span>"客户端接入协议"</span><select prop:value=client on:change=move|e|client.set(event_target_value(&e))>{move||["chat","responses","messages"].into_iter().map(|kind|{let (status,available)=client_protocol_status(&protocols.get(),kind);view!{<option value=kind disabled=!available>{format!("{} · {status}",protocol_label(kind))}</option>}}).collect_view()}</select><span class="account-help">"仅预览转发方式，例如 /v1/chat/completions；不会修改来源或客户端配置。"</span></label></div>
        <form class="account-form" on:submit=save><fieldset disabled=move||busy.get()||loading.get()><label class="account-field"><span>"供应方接口协议"</span><select prop:value=move||primary(&protocols.get()) on:change=move|e|protocols.update(|raw|*raw=choose_primary(raw,&event_target_value(&e)))>{move||serde_json::from_str::<Value>(&protocols.get()).ok().map(|v|api::rows(&v,"native").into_iter().filter_map(|v|v.as_str().map(str::to_owned)).map(|p|view!{<option value=p.clone()>{protocol_label(&p).to_string()}</option>}).collect_view())}</select><span class="account-help">"优先使用已声明的原生协议，例如供应方 /responses；OAuth 线路仍受服务端固定传输约束。"</span></label>
        <label class="account-check"><input type="checkbox" prop:checked=move||{serde_json::from_str::<Value>(&protocols.get()).ok().is_some_and(|v|v["allow_conversion"]==true)} on:change=move|e|protocols.update(|raw|*raw=patch_flag(raw,"allow_conversion",event_target_checked(&e)))/>"允许在可无损表达的协议之间转换"</label>
        <div class="table-wrap"><table><thead><tr><th>"参数"</th><th>"客户端请求"</th><th>"供应方接收"</th><th>"处理规则"</th></tr></thead><tbody>
        <tr><td>"推理强度"</td><td><code>{move||effort_key(&client.get()).to_string()}</code></td><td><code>{move||effort_key(&primary(&protocols.get())).to_string()}</code></td><td>"保留显式档位；按来源能力校验，不自动降档"</td></tr>
        <tr><td>"结构化输出"</td><td>{move||if client.get()=="chat"{"response_format"}else if client.get()=="responses"{"text.format"}else{"Messages 原生格式"}}</td><td>{move||match primary(&protocols.get()).as_str(){"chat"=>"response_format","responses"=>"text.format",_=>"来源原生格式"}}</td><td>"仅转换可等价表达的格式；不模拟或弱化 Schema"</td></tr>
        <tr><td>"输出上限"</td><td>{move||cap_key(&client.get()).to_string()}</td><td>{move||cap_key(&primary(&protocols.get())).to_string()}</td><td>"核对是否包含推理；不静默缩小客户端显式上限"</td></tr>
        <tr><td>"无等价参数"</td><td>"客户端显式字段"</td><td>"无等价字段"</td><td>"明确报错并说明原因"</td></tr>
        </tbody></table></div><p class="account-help">{move||{let p=serde_json::from_str::<Value>(&protocols.get()).unwrap_or(Value::Null);if p["native"].as_array().is_some_and(|rows|rows.iter().any(|v|v.as_str()==Some(client.get().as_str()))){"当前客户端协议已在来源原生协议中声明；仍需模型和参数校验。"}else if p["allow_conversion"]==true{"当前组合将尝试协议转换；表中的字段位置不是所有取值均支持的承诺。"}else{"当前来源未声明此客户端协议且未允许转换，此组合会被拒绝。"}}}</p>
        <details><summary>"输出边界与完整协议配置"</summary><label class="account-field"><span>"协议配置 JSON"</span><textarea rows="9" spellcheck="false" maxlength="65536" prop:value=protocols on:input=move|e|protocols.set(event_target_value(&e))/><span class="account-help">"保留所有已有字段；max_input_tokens 与 max_output_tokens 必须使用成对的已验证边界。人工参数适配不授予认证能力。"</span></label></details>
        <div class="account-actions"><button type="submit" class="ghost">"保存供应方协议设置"</button><button type="button" class="ghost" on:click=move |_|load.run(())>"重新读取已保存配置"</button></div></fieldset></form>
        <details><summary>"此来源的网络与代理"</summary>{move||selected.get().map(|id|view!{<super::proxy_settings::ProxySettings ctx=ctx scope="source".to_string() id=id.to_string()/>})}</details>
        {move||{let name=model.get();selected.get().filter(|_|!name.is_empty()).map(|id|view!{<super::model_reasoning::ReasoningConfig ctx=ctx channel=Some(id) model=name label="当前模型的推理参数".to_string()/>})}}
        <p role="alert" class="account-error">{move||error.get()}</p><p role="status" class="account-help">{move||note.get()}</p>
    </section>}
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn protocol_convenience_controls_preserve_other_contract_fields(){
        let raw=r#"{"native":["chat","responses"],"parameter_profile":"mimo","max_input_tokens":200,"max_output_tokens":100,"allow_conversion":false}"#;
        let chosen=choose_primary(raw,"responses");let value:Value=serde_json::from_str(&chosen).unwrap();
        assert_eq!(value["native"],json!(["responses","chat"]));assert_eq!(value["parameter_profile"],"mimo");assert_eq!(value["max_output_tokens"],100);
        let value:Value=serde_json::from_str(&patch_flag(&chosen,"allow_conversion",true)).unwrap();assert_eq!(value["allow_conversion"],true);assert_eq!(value["max_input_tokens"],200);
        assert_eq!(choose_primary("bad","chat"),"bad");
        assert_eq!(client_protocol_status(raw,"chat"),("原生",true));
        assert_eq!(client_protocol_status(raw,"messages"),("不可用",false));
        assert_eq!(client_protocol_status(&patch_flag(raw,"allow_conversion",true),"messages"),("经转换",true));
    }
}
