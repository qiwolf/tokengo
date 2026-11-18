//! Step 3 uses saved sources/models; client protocol is an inspection choice, not source identity.
use crate::{Ctx, admin_api as api};
use leptos::prelude::*;
use serde_json::{Value, json};

const PROTOCOLS: [&str; 3] = ["chat", "responses", "messages"];

fn native_protocol_state(raw: &str, protocol: &str) -> (bool, bool) {
    let Ok(config) = serde_json::from_str::<Value>(raw) else { return (false, false); };
    let Some(native) = config["native"].as_array() else { return (false, false); };
    let checked = native.iter().any(|entry| entry.as_str() == Some(protocol));
    let chat_required = protocol == "chat"
        && (config["native_input_audio"] == true || config["chat_reasoning_replay"] == true);
    (checked, PROTOCOLS.contains(&protocol) && (!checked || (native.len() > 1 && !chat_required)))
}

fn toggle_native_protocol(raw: &str, protocol: &str, enable: bool) -> String {
    let (checked, changeable) = native_protocol_state(raw, protocol);
    if checked == enable || !changeable {
        return raw.into();
    }
    let Ok(mut config) = serde_json::from_str::<Value>(raw) else { return raw.into(); };
    let Some(native) = config["native"].as_array_mut() else { return raw.into(); };
    if enable {
        native.push(json!(protocol));
    } else {
        native.retain(|entry| entry.as_str() != Some(protocol));
    }
    config.to_string()
}

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
fn protocol_save_payload(row: &Value, protocols: Value) -> Result<Value, String> {
    let expected_generation = row["binding_generation"].as_i64()
        .ok_or_else(|| "缺少来源版本，已重新读取配置，请确认后再保存。".to_string())?;
    let expected_protocols_sha256 = row["protocols_sha256"].as_str()
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| "服务端未提供有效协议版本，已重新读取配置；请确认版本支持后再保存。".to_string())?;
    Ok(json!({"protocols": protocols, "expected_binding_generation": expected_generation,
        "expected_protocols_sha256": expected_protocols_sha256}))
}
fn protocol_label(protocol: &str) -> &str {match protocol{"chat"=>"Chat Completions","responses"=>"Responses","messages"=>"Messages",_=>"尚未确认"}}
fn protocol_path(protocol: &str) -> &'static str {
    match protocol {
        "chat" => "/v1/chat/completions",
        "responses" => "/v1/responses",
        "messages" => "/v1/messages",
        _ => "",
    }
}
fn supplier_protocol(raw: &str, client: &str) -> Option<String> {
    let config: Value = serde_json::from_str(raw).ok()?;
    let native = config["native"].as_array()?;
    if native.iter().any(|entry| entry.as_str() == Some(client)) {
        return Some(client.to_owned());
    }
    if config["allow_conversion"] == true {
        return native.first().and_then(Value::as_str).map(str::to_owned);
    }
    None
}
fn client_protocol_status(raw: &str, protocol: &str) -> (&'static str, bool) {
    match supplier_protocol(raw, protocol) {
        Some(native) if native == protocol => ("原生", true),
        Some(_) => ("经转换", true),
        None => ("不可用", false),
    }
}
fn effort_key(protocol: &str) -> &str {match protocol{"chat"=>"reasoning_effort","responses"=>"reasoning.effort","messages"=>"output_config.effort",_=>"由来源确认"}}
fn cap_key(protocol: &str) -> &str {match protocol{"chat"=>"max_tokens / max_completion_tokens","responses"=>"max_output_tokens","messages"=>"max_tokens",_=>"由来源确认"}}
fn capacity_value(raw:&str,key:&str)->String {
    serde_json::from_str::<Value>(raw).ok().and_then(|v|v.get(key).cloned()).map(|v|v.as_str().map(str::to_owned).unwrap_or_else(||if v.is_null(){String::new()}else{v.to_string()})).unwrap_or_default()
}
fn edit_capacity(raw:&str,key:&str,text:&str)->String {
    let Ok(mut value)=serde_json::from_str::<Value>(raw)else{return raw.into();};
    let Some(map)=value.as_object_mut()else{return raw.into();};
    if text.trim().is_empty(){map.remove(key);}else{map.insert(key.into(),text.parse::<i64>().map(|n|json!(n)).unwrap_or_else(|_|json!(text)));}value.to_string()
}
fn fill_discovered_limits(raw:&str,limits:&Value,replace:bool)->String {
    let Ok(mut value)=serde_json::from_str::<Value>(raw)else{return raw.into();};
    if !value.is_object(){return raw.into();}
    for key in ["max_input_tokens","max_output_tokens"] {
        if let Some(n)=limits[key].as_i64().filter(|n|*n>0) {
            if replace||value.get(key).is_none_or(|v|v.is_null()){value[key]=json!(n);}
        }
    }value.to_string()
}
#[component]
pub fn ConnectionParameters(ctx: Ctx, source_ids: Vec<i64>) -> impl IntoView {
    let sources=StoredValue::new(source_ids);
    let channels=RwSignal::new(Vec::<Value>::new());
    let selected=RwSignal::new(None::<i64>);
    let model=RwSignal::new(String::new());
    let protocols=RwSignal::new(String::new());
    let client=RwSignal::new("chat".to_string());
    let oauth_bound = Memo::new(move |_| {
        let id = selected.get();
        channels.get().iter().any(|row| {
            row["id"].as_i64() == id && row["authentication_line"]["kind"] == "oauth"
        })
    });
    let supplier = Memo::new(move |_| supplier_protocol(&protocols.get(), &client.get()).unwrap_or_default());
    let loading=RwSignal::new(false);let busy=RwSignal::new(false);
    let error=RwSignal::new(String::new());let note=RwSignal::new(String::new());
    let load_generation=RwSignal::new(0u64);let discovery_generation=RwSignal::new(0u64);let discovering=RwSignal::new(false);let declaration=RwSignal::new(None::<Value>);let discovery_note=RwSignal::new(String::new());let last_discovered=RwSignal::new(None::<(i64,String)>);
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
    let discover=Callback::new(move|replace:bool|{
        let Some(id)=selected.get_untracked()else{return;};let name=model.get_untracked();if name.is_empty(){return;}
        let pair=(id,name.clone());last_discovered.set(Some(pair.clone()));discovery_generation.update(|v|*v+=1);let sequence=discovery_generation.get_untracked();let starting_limits=(capacity_value(&protocols.get_untracked(),"max_input_tokens"),capacity_value(&protocols.get_untracked(),"max_output_tokens"));discovering.set(true);discovery_note.set("正在读取供应方模型目录…".into());
        leptos::task::spawn_local(async move {
            let result=api::request("POST",&format!("/api/channels/{id}/limits/discover"),Some(json!({"model":name}))).await;
            if discovery_generation.try_get_untracked()!=Some(sequence)||selected.try_get_untracked()!=Some(Some(id))||model.try_get_untracked()!=Some(pair.1){return;}
            match result {
                Ok(value)=>{
                    protocols.update(|raw|{let unchanged=(capacity_value(raw,"max_input_tokens"),capacity_value(raw,"max_output_tokens"))==starting_limits;*raw=fill_discovered_limits(raw,&value["limits"],replace&&unchanged);});
                    let time=value["discovered_at_ms"].as_f64().map(|ms|js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms)).to_locale_string("zh-CN",&wasm_bindgen::JsValue::UNDEFINED).as_string().unwrap_or_default()).unwrap_or_default();
                    discovery_note.set(if value["status"]=="supplier_declared"{format!("供应方目录声明 · {time}；可调整后保存，已有手工配置优先。")}else{"供应方目录未声明这些上限，请保留现有配置或手工补填。".into()});declaration.set(Some(value));
                },
                Err(e)=>{ctx.note_err(&e);discovery_note.set(format!("未能探测：{}；现有值保持，可手工填写。",e.message));}
            }discovering.set(false);
        });
    });
    Effect::new(move |_|{
        let Some(id)=selected.get()else{return;};let name=model.get();if name.is_empty()||loading.get(){return;}
        if last_discovered.get_untracked()!=Some((id,name)){discover.run(false);}
    });
    let save=move |event:leptos::ev::SubmitEvent|{
        event.prevent_default();if busy.get_untracked(){return;}
        let Some(id)=selected.get_untracked()else{error.set("请先选择已保存的来源。".into());return;};
        let value=match api::protocols(&protocols.get_untracked()){Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        let row=channels.get_untracked().into_iter().find(|r|r["id"].as_i64()==Some(id)).unwrap_or(Value::Null);
        let payload=match protocol_save_payload(&row,value){Ok(v)=>v,Err(e)=>{load.run(());error.set(e);return;}};
        busy.set(true);error.set(String::new());note.set(String::new());
        leptos::task::spawn_local(async move{
            let result=api::request("PUT",&format!("/api/channels/{id}/protocols"),Some(payload)).await;
            if busy.try_get_untracked().is_none(){return;}
            match result{Ok(_)=>{note.set("供应方接口协议设置已保存；未发送模型推理请求。".into());load.run(());},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);
        });
    };
    view!{<section class="connection-parameters"><h3>"参数与转发"</h3><p class="account-help">"这里显示已保存的真实来源和模型；供应方协议与容量设置作用于所选接入来源及其全部模型。客户端接入协议是用户调用本系统的协议；供应方接口协议是本系统请求供应方时使用的协议。"</p>
        <Show when=move||loading.get()><p role="status">"正在读取已保存的来源与模型…"</p></Show>
        <Show when=move||!loading.get()&&channels.get().is_empty()><p role="alert">"没有可配置的已保存来源，请返回列表重新打开此凭据。"</p></Show>
        <Show when=move||{channels.get().len()>1}><label class="account-field"><span>"当前来源"</span><select aria-label="当前来源" prop:value=move||selected.get().map(|id|id.to_string()).unwrap_or_default() on:change=move|e|selected.set(event_target_value(&e).parse().ok())>{move||channels.get().into_iter().map(|c|view!{<option value=c["id"].to_string()>{api::text(&c,"name")}</option>}).collect_view()}</select><span class="account-help">"本页只修改所选来源，不批量覆盖同凭据的其他来源。"</span></label></Show>
        <div class="policy-limits"><label class="account-field"><span>"当前模型"</span><select aria-label="当前模型" prop:value=model on:change=move|e|model.set(event_target_value(&e))>{move||channels.get().into_iter().find(|r|r["id"].as_i64()==selected.get()).map(|r|api::rows(&r,"models").into_iter().map(|m|{let name=api::text(&m,"model");view!{<option value=name.clone()>{name.clone()}</option>}}).collect_view())}</select></label>
        <label class="account-field"><span>"客户端接入协议"</span>
            <select aria-label="客户端接入协议" prop:value=client on:change=move |e| client.set(event_target_value(&e))>
                {move || PROTOCOLS.into_iter().map(|kind| {
                    let (status, available) = client_protocol_status(&protocols.get(), kind);
                    view! { <option value=kind disabled=!available>
                        {format!("{} {} · {status}", protocol_label(kind), protocol_path(kind))}
                    </option> }
                }).collect_view()}
            </select>
            <span class="account-help">"选择要预览的客户端请求路径。"</span>
        </label></div>
        <form class="account-form" on:submit=save><fieldset disabled=move||busy.get()||loading.get()>
        <Show when=move || !oauth_bound.get()>
            <div class="account-field" role="group" aria-labelledby="source-native-protocols" aria-describedby="source-native-protocols-help">
                <span id="source-native-protocols">"供应方原生协议"</span>
                <div class="policy-limits">
                    {PROTOCOLS.into_iter().map(|kind| view! {
                        <label class="account-check">
                            <input type="checkbox"
                                prop:checked=move || native_protocol_state(&protocols.get(), kind).0
                                disabled=move || !native_protocol_state(&protocols.get(), kind).1
                                on:change=move |e| protocols.update(|raw| {
                                    *raw = toggle_native_protocol(raw, kind, event_target_checked(&e));
                                }) />
                            <span>{protocol_label(kind)}</span>
                        </label>
                    }).collect_view()}
                </div>
                <span class="account-help" id="source-native-protocols-help">"勾选供应方实际支持的协议，至少保留一个。启用 Chat 音频输入或推理回放时须保留 Chat Completions。"</span>
            </div>
            <label class="account-field"><span>"转换优先协议"</span>
                <select prop:value=move || primary(&protocols.get())
                    on:change=move |e| protocols.update(|raw| *raw = choose_primary(raw, &event_target_value(&e)))>
                    {move || PROTOCOLS.into_iter().filter(|kind| native_protocol_state(&protocols.get(), kind).0)
                        .map(|kind| view! { <option value=kind>{protocol_label(kind)}</option> }).collect_view()}
                </select>
                <span class="account-help">"客户端协议由供应方原生支持时直接转发，需要转换时使用此协议。"</span>
            </label>
        </Show>
        <Show when=move || oauth_bound.get()>
            <p class="account-help">{move || format!("OAuth 账号固定使用 {}。", protocol_label(&primary(&protocols.get())))}</p>
        </Show>
        <div class="policy-limits"><label class="account-field"><span>"来源输入窗口（Token）"</span><input id="source-max-input" type="text" inputmode="numeric" prop:value=move||capacity_value(&protocols.get(),"max_input_tokens") on:input=move|e|protocols.update(|raw|*raw=edit_capacity(raw,"max_input_tokens",&event_target_value(&e)))/><span class="account-help">"按当前模型的目录声明自动补缺；保存来源设置会作用于该来源全部模型。"</span></label>
        <label class="account-field"><span>"来源最大输出（Token）"</span><input id="source-max-output" type="text" inputmode="numeric" prop:value=move||capacity_value(&protocols.get(),"max_output_tokens") on:input=move|e|protocols.update(|raw|*raw=edit_capacity(raw,"max_output_tokens",&event_target_value(&e)))/><span class="account-help">"保留未知，不把默认值当作供应方声明。"</span></label></div>
        <div class="account-actions"><button type="button" class="ghost" disabled=move||discovering.get() on:click=move |_|discover.run(true)>"重新探测并填入"</button></div><p role="status" class="account-help">{move||discovery_note.get()}</p>{move||declaration.get().map(|v|view!{<p class="account-help">{format!("探测来源：{}",api::text(&v,"source_url"))}</p>})}
        <label class="account-check"><input type="checkbox" prop:checked=move||{serde_json::from_str::<Value>(&protocols.get()).ok().is_some_and(|v|v["allow_conversion"]==true)} on:change=move|e|protocols.update(|raw|*raw=patch_flag(raw,"allow_conversion",event_target_checked(&e)))/>"允许在可无损表达的协议之间转换"</label>
        <div class="table-wrap"><table><thead><tr><th>"参数"</th><th>"客户端请求"</th><th>"供应方接收"</th><th>"处理规则"</th></tr></thead><tbody>
        <tr><td>"推理强度"</td><td><code>{move||effort_key(&client.get()).to_string()}</code></td><td><code>{move||effort_key(&supplier.get()).to_string()}</code></td><td>"保留显式档位；按来源能力校验，不自动降档"</td></tr>
        <tr><td>"结构化输出"</td><td>{move||if client.get()=="chat"{"response_format"}else if client.get()=="responses"{"text.format"}else{"Messages 原生格式"}}</td><td>{move||match supplier.get().as_str(){"chat"=>"response_format","responses"=>"text.format",_=>"来源原生格式"}}</td><td>"仅转换可等价表达的格式；不模拟或弱化 Schema"</td></tr>
        <tr><td>"输出上限"</td><td>{move||cap_key(&client.get()).to_string()}</td><td>{move||cap_key(&supplier.get()).to_string()}</td><td>"核对是否包含推理；不静默缩小客户端显式上限"</td></tr>
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
    #[test]
    fn native_checkbox_keeps_order_and_unrelated_source_fields() {
        let original = json!({
            "native":["chat"], "allow_conversion":false,
            "parameter_profile":"openai", "max_input_tokens":1000, "max_output_tokens":400,
        }).to_string();
        assert_eq!(toggle_native_protocol(&original, "chat", false), original);
        assert_eq!(toggle_native_protocol(&original, "invalid", true), original);
        let both = toggle_native_protocol(&original, "responses", true);
        assert_eq!(toggle_native_protocol(&both, "responses", true), both);
        let responses_first = choose_primary(&both, "responses");
        assert_eq!(supplier_protocol(&responses_first, "chat").as_deref(), Some("chat"));
        assert_eq!(supplier_protocol(&responses_first, "responses").as_deref(), Some("responses"));
        let only_responses = toggle_native_protocol(&responses_first, "chat", false);
        assert_eq!(client_protocol_status(&only_responses, "chat"), ("不可用", false));
        let value: Value = serde_json::from_str(&only_responses).unwrap();
        assert_eq!(value, json!({
            "native":["responses"], "allow_conversion":false,
            "parameter_profile":"openai", "max_input_tokens":1000, "max_output_tokens":400,
        }));
        assert_eq!(supplier_protocol(&patch_flag(&only_responses, "allow_conversion", true), "chat").as_deref(), Some("responses"));
    }

    #[test]
    fn chat_native_checkbox_stays_enabled_for_required_chat_features() {
        for flag in ["native_input_audio", "chat_reasoning_replay"] {
            let mut original = json!({"native":["chat","responses"]});
            original[flag] = json!(true);
            let raw = original.to_string();
            assert_eq!(native_protocol_state(&raw, "chat"), (true, false));
            assert_eq!(toggle_native_protocol(&raw, "chat", false), raw);
            assert_eq!(native_protocol_state(&raw, "responses"), (true, true));
        }
    }

    #[test] fn save_payload_copies_both_versions_from_the_read_row() {
        let row=json!({"binding_generation":7,"protocols_sha256":"ab".repeat(32),"protocols":{"max_input_tokens":100,"max_output_tokens":100}});
        let edited=json!({"native":["responses"],"max_input_tokens":272000,"max_output_tokens":272000});
        let payload=protocol_save_payload(&row,edited.clone()).unwrap();
        assert_eq!(payload["protocols"],edited);
        assert_eq!(payload["expected_binding_generation"],7);
        assert_eq!(payload["expected_protocols_sha256"],row["protocols_sha256"]);
        // Capacity edits do not produce a replacement read hash or require a binding change.
        assert_eq!(row["protocols"]["max_input_tokens"],100);
    }
    #[test] fn save_payload_rejects_missing_or_invalid_protocol_version_without_a_fallback() {
        for hash in [Value::Null,json!(""),json!("abc"),json!("z".repeat(64)),json!(7)] {
            assert!(protocol_save_payload(&json!({"binding_generation":7,"protocols_sha256":hash}),json!({})).is_err());
        }
        assert!(protocol_save_payload(&json!({"protocols_sha256":"ab".repeat(32)}),json!({})).is_err());
    }
    #[test] fn runtime_identity_is_not_lost_when_changing_protocol_or_conversion(){
        let identity=json!({"runtime":"ollama","evidence":"operator_declared","models":[{"model":"qwen3:8b","template_sha256":"a".repeat(64)}]});
        let raw=json!({"native":["chat","responses"],"parameter_profile":"ollama","allow_conversion":true,"runtime_identity":identity}).to_string();
        let chosen=choose_primary(&raw,"responses");
        let edited=patch_flag(&chosen,"allow_conversion",false);
        let value=api::protocols(&edited).unwrap();
        assert_eq!(value["runtime_identity"],identity);
        assert_eq!(value["native"],json!(["responses","chat"]));
        assert_eq!(value["allow_conversion"],false);
    }
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

#[cfg(test)] mod discovery_input_tests {
 use super::*;
 #[test] fn automatic_fill_preserves_admin_decisions_and_unrelated_protocol_fields(){
  let raw=json!({"native":["chat"],"allow_conversion":true,"max_input_tokens":500000}).to_string();let limits=json!({"max_input_tokens":1000000,"max_output_tokens":384000});
  let value:Value=serde_json::from_str(&fill_discovered_limits(&raw,&limits,false)).unwrap();assert_eq!(value["max_input_tokens"],500000);assert_eq!(value["max_output_tokens"],384000);assert_eq!(value["allow_conversion"],true);assert_eq!(value["native"],json!(["chat"]));
  let value:Value=serde_json::from_str(&fill_discovered_limits(&raw,&limits,true)).unwrap();assert_eq!(value["max_input_tokens"],1000000);
 }
 #[test] fn unknown_or_invalid_declarations_do_not_invent_zero_bounds(){
  for limits in [json!({}),json!({"max_input_tokens":0,"max_output_tokens":-1}),json!({"max_input_tokens":"1M","max_output_tokens":null})] {let value:Value=serde_json::from_str(&fill_discovered_limits(r#"{"native":["chat"]}"#,&limits,false)).unwrap();assert!(value.get("max_input_tokens").is_none());assert!(value.get("max_output_tokens").is_none());}
 }
}
