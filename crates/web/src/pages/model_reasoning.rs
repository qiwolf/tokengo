//! Reasoning defaults follow declared metadata, never model-name suffixes.
use crate::{Ctx, api, model::ReasoningCapability};
use leptos::prelude::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Async results belong to one mounted component and one latest operation.
struct ReasoningEpoch { active:bool, generation:u64 }
impl Default for ReasoningEpoch {fn default()->Self{Self{active:true,generation:0}}}
impl ReasoningEpoch {
    fn begin(&mut self)->Option<u64>{if !self.active{return None}self.generation+=1;Some(self.generation)}
    fn accepts(&self,ticket:u64)->bool{self.active&&self.generation==ticket}
    fn dispose(&mut self){self.active=false;self.generation+=1;}
}

pub(crate) fn confirmed_levels(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|s| s == part) {
            out.push(part.to_string());
        }
    }
    out
}
pub(crate) fn source_levels(c: &ReasoningCapability) -> Vec<String> {
    let mut levels = c.upstream_levels.clone();
    for level in c.verified_levels.iter().chain(c.compatibility_levels.iter()) {
        if !levels.contains(level) { levels.push(level.clone()); }
    }
    if levels.is_empty() && c.manual_levels.is_none() { c.levels.clone() } else { levels }
}
pub(crate) fn effective_levels(c: &ReasoningCapability, mode: &str, text: &str) -> Vec<String> {
    let mut levels = if mode == "replace" { vec![] } else { source_levels(c) };
    if mode != "source" {
        for level in confirmed_levels(text) {
            if !levels.contains(&level) { levels.push(level); }
        }
    }
    levels
}
fn valid_manual_levels(text: &str) -> bool {
    let levels = confirmed_levels(text);
    !levels.is_empty() && levels.len() <= 16 && levels.iter().all(|s|
        s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'))
}
fn level_list(levels: &[String]) -> String {
    if levels.is_empty() { "暂无".into() } else { levels.join(" / ") }
}
pub(crate) fn capability_label(c: &ReasoningCapability) -> String {
    let source = if c.upstream_levels.is_empty() && c.verified_levels.is_empty() && c.compatibility_levels.is_empty() {
        match c.source.as_str() { "upstream"=>"上游声明".to_string(), "upstream_with_compatibility"=>"上游声明 + 内置兼容".into(), "user_confirmed"=>"人工设置".into(), _=>"尚未确认".into() }
    } else {
        let mut labels=Vec::new();
        if !c.upstream_levels.is_empty(){labels.push("上游声明");}
        if !c.verified_levels.is_empty(){labels.push("实测补充");}
        if !c.compatibility_levels.is_empty(){labels.push("内置兼容");}
        if c.manual_levels.is_some(){labels.push(if c.manual_mode.as_deref()==Some("append"){"人工追加"}else{"人工覆盖"});}
        labels.join(" + ")
    };
    let status = match c.status.as_str() {
        "supported" if !c.levels.is_empty() => format!("支持档位：{}", c.levels.join(" / ")),
        "supported" => "支持推理，档位未知".into(),
        "unsupported" => "明确不支持推理强度参数".into(),
        _ => "尚未确认支持哪些推理档位".into(),
    };
    if matches!(c.source.as_str(), "upstream" | "upstream_with_compatibility" | "user_confirmed") {format!("{status} · {source}")}else{status}
}
pub(crate) fn thinking_modes_label(c: &ReasoningCapability) -> String {
    if c.thinking_modes.is_empty() {
        "思考模式：服务商未公布具体模式".into()
    } else {
        format!("思考模式：{}（独立于推理档位）", c.thinking_modes.join(" / "))
    }
}
#[component]
pub fn ReasoningConfig(ctx: Ctx, channel: Option<i64>, model: String, #[prop(optional)] label: Option<String>) -> impl IntoView {
    let diagnostics_open = RwSignal::new(false);
    let lifecycle=Arc::new(Mutex::new(ReasoningEpoch::default()));
    let cleanup=lifecycle.clone();on_cleanup(move||cleanup.lock().unwrap().dispose());
    let load_lifecycle=lifecycle.clone();
    let model = StoredValue::new(model);
    let data = RwSignal::new(ReasoningCapability::default());
    let manual = RwSignal::new(String::new());
    let manual_mode = RwSignal::new("source".to_string());
    let default = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let load=Callback::new(move|()| {
        let epoch=load_lifecycle.clone();let Some(ticket)=epoch.lock().unwrap().begin()else{return};
        loaded.set(false);
        let Some(id)=channel else{return};
        // Resolve owner-bound values before yielding; the original component may
        // be replaced while the real metadata request is still in flight.
        let requested_model=model.get_value();
        leptos::task::spawn_local(async move {
            let response=api::channels().await;
            if !epoch.lock().unwrap().accepts(ticket){return}
            match response {
                Ok(rows)=>{
                    if let Some(m)=rows.into_iter().find(|c|c.id==id).and_then(|c|c.models.into_iter().find(|m|m.model==requested_model)) {
                        let cap=m.reasoning.unwrap_or_default();
                        manual.set(cap.manual_levels.clone().unwrap_or_default().join(", "));
                        manual_mode.set(if cap.manual_levels.is_some() {cap.manual_mode.clone().unwrap_or_else(||"replace".into())} else {"source".into()});
                        default.set(if cap.configured_default{cap.default_effort.clone().unwrap_or_default()}else{String::new()});
                        data.set(cap);loaded.set(true);
                    }else{error.set("当前模型配置不存在，请刷新渠道后重试。".into());}
                },
                Err(e)=>{error.set(e.message.clone());ctx.note_err(&e);}
            }
        });
    });
    load.run(());
    let save_lifecycle=lifecycle.clone();
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(id) = channel else {
            error.set("请先保存此连接的模型映射，再配置推理参数。".into());
            return;
        };
        if busy.get_untracked() {
            return;
        }
        if !loaded.get_untracked() {error.set("请等待模型配置读取成功后再保存。".into());return;}
        let mode = manual_mode.get_untracked();
        let levels = if mode != "source" {
            let levels = confirmed_levels(&manual.get_untracked());
            if !valid_manual_levels(&manual.get_untracked()) {
                error.set("请填写1–16个档位，每项不超过32字节，仅用小写字母、数字、下划线或连字符。".into());
                return;
            }
            Some(levels)
        } else {
            None
        };
        let effort = default.get_untracked();
        let effort = if effort.is_empty() {
            None
        } else {
            Some(effort)
        };
        let available = effective_levels(&data.get_untracked(), &mode, &manual.get_untracked());
        if effort.as_ref().is_some_and(|v| !available.contains(v)) {
            error.set("默认档位已不在最终生效列表中，请重新选择默认值。".into());
            return;
        }
        let epoch=save_lifecycle.clone();let Some(ticket)=epoch.lock().unwrap().begin()else{return};
        let body=json!({"model":model.get_value(),"manual_levels":levels,"manual_mode":if mode == "source" {None::<String>} else {Some(mode)},"default_effort":effort});
        busy.set(true);error.set(String::new());message.set(String::new());
        leptos::task::spawn_local(async move {
            let response=api::call("PUT",&format!("/api/channels/{id}/models/reasoning"),Some(body)).await;
            if !epoch.lock().unwrap().accepts(ticket){return}
            busy.set(false);
            match response {Ok(_)=>{message.set("推理参数已保存，客户端显式参数仍优先。".into());load.run(());},Err(e)=>{error.set(e.message.clone());ctx.note_err(&e);}}
        });
    };
    let refresh_lifecycle=lifecycle.clone();
    let refresh=move |_| {
        let Some(id)=channel else{return};if busy.get_untracked(){return}
        let epoch=refresh_lifecycle.clone();let Some(ticket)=epoch.lock().unwrap().begin()else{return};
        busy.set(true);error.set(String::new());
        leptos::task::spawn_local(async move {
            let response=api::refresh_channel_metadata(id).await;
            if !epoch.lock().unwrap().accepts(ticket){return}
            busy.set(false);
            match response {Ok(r) if r.ok=>{message.set("已从服务商更新模型信息；本次未调用模型。".into());load.run(());},Ok(r)=>error.set(r.note.unwrap_or_else(||"元数据获取失败，现有配置保留。".into())),Err(e)=>{error.set(e.message.clone());ctx.note_err(&e);}}
        });
    };
    let field = move |name: &str| {
        format!(
            "reasoning-{}-{}-{}",
            channel.unwrap_or(0),
            model.get_value(),
            name
        )
    };
    view! {<section class="reasoning-config"><h3>{label.unwrap_or_else(||if channel.is_some(){"推理设置".into()}else{"推理设置（请先保存模型）".into()})}</h3><p>{move||capability_label(&data.get())}</p><dl class="reasoning-sources">
      <dt>"上游声明"</dt><dd>{move||level_list(&data.get().upstream_levels)}</dd>
      <dt>"实测补充"</dt><dd>{move||level_list(&data.get().verified_levels)}</dd>
      <dt>"内置兼容"</dt><dd>{move||level_list(&data.get().compatibility_levels)}</dd>
      <dt>"已保存的人工设置"</dt><dd>{move||{let c=data.get();c.manual_levels.as_ref().map(|v|format!("{}：{}",if c.manual_mode.as_deref()==Some("append"){"追加"}else{"覆盖"},level_list(v))).unwrap_or_else(||"未设置".into())}}</dd>
    </dl><Show when=move||!data.get().verified_levels.is_empty()><p class="account-help">{move||data.get().reason}</p></Show><p class="muted small">{move||thinking_modes_label(&data.get())}</p><p class="muted small">{move||data.get().default_effort.map(|v|format!("当前默认档位：{v}（{}）",if data.get().configured_default{"已配置"}else{"服务商默认"})).unwrap_or_else(||"当前跟随服务商默认设置。".into())}</p><p class="account-help">"档位与默认值一起保存，仅作用于当前模型和接入。客户端显式指定的档位优先；none 是明确关闭推理，与沿用服务商默认不同。"</p><div class="toolbar reasoning-actions"><button type="button" class="ghost" on:click=move |_|diagnostics_open.set(true)>"识别依据与高级说明"</button><super::ModuleDialog open=diagnostics_open title="推理能力识别依据".to_string()><p class="muted small">{move||data.get().reason}</p><p class="muted small">{move||data.get().effort_wire.map(|wire|format!("推理强度的上游参数：{wire}")).unwrap_or_else(||"上游强度参数位置尚未确认；不根据模型名称推断。".into())}</p><p class="muted small">"思考模式与推理强度分别记录。disabled、adaptive 和 between_tools 是模式，不会作为强度档位加入下拉选项；本页默认值只配置强度，思考模式仍由客户端显式请求。"</p><p class="muted small">"更新模型信息不会发送推理请求，识别结果不代表实际调用已验证。具体能力以当前接入协议为准。推理档位使用模型原生参数，不另建模型名；客户端显式指定的 reasoning.effort、reasoning_effort 和 service_tier 优先。"</p></super::ModuleDialog>
    <button type="button" class="ghost" disabled=move||busy.get()||channel.is_none() on:click=refresh>"更新模型信息"</button></div>{channel.map(|id|view!{<super::provider_compatibility::CompatibilityDiagnostics ctx=ctx channel=id model=model.get_value()/>})}<form class="account-form" on:submit=save>
    <div class="account-field"><label for=field("mode")>"人工设置方式"</label>
    <select id=field("mode") prop:value=manual_mode disabled=move||busy.get()||channel.is_none()||!loaded.get() on:change=move|e|{manual_mode.set(event_target_value(&e));error.set(String::new());}>
    <option value="source">"跟随来源（无人工设置）"</option><option value="append">"追加档位"</option><option value="replace">"覆盖支持列表"</option></select>
    <p class="account-help">{move||match manual_mode.get().as_str(){"append"=>"保留当前来源支持的档位，加入你人工确认的值；来源更新后重新合并。","replace"=>"仅允许你填写的档位；未填写的值会被排除，包括实测补充值。",_=>"使用上游声明及当前有效的补充能力；保存后清除人工列表。"}}</p></div>
    <Show when=move||manual_mode.get()!="source"><div class="account-field">
    <label for=field("manual")>"人工确认的档位（逗号分隔）"</label>
    <input id=field("manual") disabled=move||busy.get() prop:value=manual on:input=move|e|{manual.set(event_target_value(&e));error.set(String::new());} aria-describedby=field("help")/>
    <p id=field("help") class="account-help">"仅填写已核实的原生参数值，1–16项，小写字母、数字、下划线或连字符；人工设置不会标记为实测成功。"</p></div></Show>
    <p role="status">"最终生效档位预览："{move||level_list(&effective_levels(&data.get(),&manual_mode.get(),&manual.get()))}</p>
    <div class="account-field reasoning-default-field"><label for=field("default")>"默认推理档位"</label>
    <select id=field("default") prop:value=default aria-describedby=field("default-help") disabled=move||busy.get()||channel.is_none()||!loaded.get() on:change=move|e|{default.set(event_target_value(&e));error.set(String::new());}>
    <option value="" selected=move||default.get().is_empty()>"沿用服务商默认"</option>
    {move||{let levels=effective_levels(&data.get(),&manual_mode.get(),&manual.get());let selected=default.get();let mut options=levels.clone();if !selected.is_empty()&&!levels.contains(&selected){options.push(selected.clone());}options.into_iter().map(|level|{let invalid=!levels.contains(&level);let is_selected=selected==level;view!{<option value=level.clone() selected=is_selected disabled=invalid>{if invalid{format!("{level}（已移除，请重新选择）")}else{level.clone()}}</option>}}).collect_view()}}</select>
    <p id=field("default-help") class="account-help">"选项随上方档位实时更新，可一次保存。沿用服务商默认表示不补充档位参数。"</p></div>
    <p class="account-error" role="alert">{move||error.get()}</p><p class="account-success" role="status">{move||message.get()}</p><button type="submit" class="primary" disabled=move||busy.get()||channel.is_none()||!loaded.get()>{move||if busy.get(){"正在保存…"}else{"保存推理参数"}}</button></form></section>}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draft_modes_and_default_options_use_unsaved_levels() {
        let c = ReasoningCapability { upstream_levels:vec!["low".into(),"medium".into()],verified_levels:vec!["none".into()],..Default::default() };
        assert_eq!(effective_levels(&c,"append","custom, medium"),vec!["low","medium","none","custom"]);
        assert_eq!(effective_levels(&c,"replace","custom"),vec!["custom"]);
        assert_eq!(effective_levels(&c,"source","custom"),vec!["low","medium","none"]);
        assert!(valid_manual_levels("custom, none"));
        assert!(!valid_manual_levels(""));
        assert!(!valid_manual_levels("Medium"));
        assert!(!valid_manual_levels("a/b"));
    }
    #[test]
    fn verified_provenance_is_not_presented_as_builtin_compatibility() {
        let c=ReasoningCapability {status:"supported".into(),source:"upstream_with_compatibility".into(),levels:vec!["low".into(),"none".into()],upstream_levels:vec!["low".into()],verified_levels:vec!["none".into()],..Default::default()};
        assert!(capability_label(&c).contains("上游声明 + 实测补充"));
        assert!(!capability_label(&c).contains("内置兼容"));
    }
    #[test]
    fn detached_or_superseded_reasoning_requests_cannot_publish() {
        let mut epoch=ReasoningEpoch::default();let old=epoch.begin().unwrap();let current=epoch.begin().unwrap();
        assert!(!epoch.accepts(old));assert!(epoch.accepts(current));
        epoch.dispose();assert!(!epoch.accepts(current));assert!(epoch.begin().is_none());
    }
    #[test]
    fn modes_are_displayed_independently_and_do_not_become_strength_options() {
        let c = ReasoningCapability {
            status: "unknown".into(),
            thinking_modes: vec!["adaptive".into(), "between_tools".into()],
            effort_wire: Some("output_config.effort".into()),
            ..Default::default()
        };
        assert!(thinking_modes_label(&c).contains("adaptive / between_tools"));
        assert!(thinking_modes_label(&c).contains("独立"));
        assert!(!capability_label(&c).contains("adaptive"));
        assert!(c.levels.is_empty());
        assert!(thinking_modes_label(&ReasoningCapability::default()).contains("未公布"));
    }
    #[test]
    fn reasoning_dto_accepts_old_and_mode_aware_payloads() {
        let old = serde_json::json!({"status":"unknown","levels":[]});
        let old: ReasoningCapability = serde_json::from_value(old).unwrap();
        assert!(old.thinking_modes.is_empty());
        assert!(old.effort_wire.is_none());
        let current = serde_json::json!({"status":"supported","levels":["high"],"thinking_modes":["disabled","adaptive"],"effort_wire":"output_config.effort"});
        let current: ReasoningCapability = serde_json::from_value(current).unwrap();
        assert_eq!(current.levels, vec!["high"]);
        assert_eq!(current.thinking_modes, vec!["disabled", "adaptive"]);
        assert_eq!(current.effort_wire.as_deref(), Some("output_config.effort"));
    }
    #[test]
    fn boolean_support_never_invents_effort_levels() {
        let c = ReasoningCapability {
            status: "supported".into(),
            source: "upstream".into(),
            ..Default::default()
        };
        assert!(capability_label(&c).contains("档位未知"));
        assert!(!capability_label(&c).contains("low"));
    }
    #[test]
    fn compatibility_is_distinguished_from_upstream_and_manual_claims() {
        let c = ReasoningCapability {
            status: "supported".into(),
            source: "upstream_with_compatibility".into(),
            levels: vec!["none".into(), "high".into()],
            ..Default::default()
        };
        assert_eq!(capability_label(&c), "支持档位：none / high · 上游声明 + 内置兼容");
    }
    #[test]
    fn manual_confirmation_deduplicates_only_explicit_values() {
        assert_eq!(
            confirmed_levels(" light, custom ,light, "),
            vec!["light", "custom"]
        );
        assert!(confirmed_levels(" , ").is_empty());
    }
}
