//! Reasoning defaults follow declared metadata, never model-name suffixes.
use crate::{Ctx, api, model::ReasoningCapability};
use leptos::prelude::*;
use serde_json::json;
pub(crate) fn confirmed_levels(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|s| s == part) {
            out.push(part.to_string());
        }
    }
    out
}
pub(crate) fn capability_label(c: &ReasoningCapability) -> String {
    let source = match c.source.as_str() {
        "upstream" => "上游声明",
        "upstream_with_compatibility" => "上游声明 + 内置兼容",
        "user_confirmed" => "人工确认",
        _ => "尚未确认",
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
    let model = StoredValue::new(model);
    let data = RwSignal::new(ReasoningCapability::default());
    let manual = RwSignal::new(String::new());
    let manual_on = RwSignal::new(false);
    let default = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let load =
        Callback::new(move |()| {
            loaded.set(false);
            let Some(id) = channel else {
                return;
            };
            leptos::task::spawn_local(async move {
                match api::channels().await {
                    Ok(rows) => {
                        if let Some(m) = rows.into_iter().find(|c| c.id == id).and_then(|c| {
                            c.models.into_iter().find(|m| m.model == model.get_value())
                        }) {
                            let cap = m.reasoning.unwrap_or_default();
                            manual.set(cap.manual_levels.clone().unwrap_or_default().join(", "));
                            manual_on.set(cap.manual_levels.is_some());
                            default.set(if cap.configured_default {
                                cap.default_effort.clone().unwrap_or_default()
                            } else {
                                String::new()
                            });
                            data.set(cap);
                            loaded.set(true);
                        } else {
                            error.set("当前模型配置不存在，请刷新渠道后重试。".into());
                        }
                    }
                    Err(e) => {
                        ctx.note_err(&e);
                        error.set(e.message);
                    }
                }
            });
        });
    load.run(());
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
        let levels = if manual_on.get_untracked() {
            let levels = confirmed_levels(&manual.get_untracked());
            if levels.is_empty() {
                error.set("请填写明确确认的档位，或关闭人工确认。".into());
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
        busy.set(true);
        error.set(String::new());
        message.set(String::new());
        leptos::task::spawn_local(async move {
            match api::call("PUT",&format!("/api/channels/{id}/models/reasoning"),Some(json!({"model":model.get_value(),"manual_levels":levels,"default_effort":effort}))).await{Ok(_)=>{message.set("推理参数已保存，客户端显式参数仍优先。".into());load.run(());},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}
            busy.set(false);
        });
    };
    let refresh=move |_|{let Some(id)=channel else{return;};if busy.get_untracked(){return;}busy.set(true);error.set(String::new());leptos::task::spawn_local(async move{match api::refresh_channel_metadata(id).await{Ok(r) if r.ok=>{message.set("已从服务商更新模型信息；本次未调用模型。".into());load.run(());},Ok(r)=>error.set(r.note.unwrap_or_else(||"元数据获取失败，现有配置保留。".into())),Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});};
    let field = move |name: &str| {
        format!(
            "reasoning-{}-{}-{}",
            channel.unwrap_or(0),
            model.get_value(),
            name
        )
    };
    view! {<section class="reasoning-config"><h3>{label.unwrap_or_else(||if channel.is_some(){"推理设置".into()}else{"推理设置（请先保存模型）".into()})}</h3><p>{move||capability_label(&data.get())}</p><p class="muted small">{move||thinking_modes_label(&data.get())}</p><p class="muted small">{move||data.get().default_effort.map(|v|format!("当前默认档位：{v}（{}）",if data.get().configured_default{"已配置"}else{"服务商默认"})).unwrap_or_else(||"当前跟随服务商默认设置。".into())}</p><p class="account-help">"先更新模型信息，查看服务商公布的档位，再选择默认值。未公布档位时保持默认；客户端指定的推理档位优先生效。"</p><div class="toolbar reasoning-actions"><button type="button" class="ghost" on:click=move |_|diagnostics_open.set(true)>"识别依据与高级说明"</button><super::ModuleDialog open=diagnostics_open title="推理能力识别依据".to_string()><p class="muted small">{move||data.get().reason}</p><p class="muted small">{move||data.get().effort_wire.map(|wire|format!("推理强度的上游参数：{wire}")).unwrap_or_else(||"上游强度参数位置尚未确认；不根据模型名称推断。".into())}</p><p class="muted small">"思考模式与推理强度分别记录。disabled、adaptive 和 between_tools 是模式，不会作为强度档位加入下拉选项；本页默认值只配置强度，思考模式仍由客户端显式请求。"</p><p class="muted small">"更新模型信息不会发送推理请求，识别结果不代表实际调用已验证。具体能力以当前接入协议为准。推理档位使用模型原生参数，不另建模型名；客户端显式指定的 reasoning.effort、reasoning_effort 和 service_tier 优先。"</p></super::ModuleDialog>
    <button type="button" class="ghost" disabled=move||busy.get()||channel.is_none() on:click=refresh>"更新模型信息"</button></div><form class="account-form" on:submit=save><div class="account-field reasoning-default-field"><label for=field("default")>"默认推理档位"</label><select id=field("default") prop:value=default disabled=move||busy.get()||channel.is_none()||!loaded.get() on:change=move|e|default.set(event_target_value(&e))><option value="">"使用服务商默认"</option>{move||data.get().levels.into_iter().map(|level|view!{<option value=level.clone()>{level.clone()}</option>}).collect_view()}</select></div>
    <label class="pick"><input type="checkbox" disabled=move||busy.get()||!loaded.get() prop:checked=manual_on on:change=move|e|manual_on.set(event_target_checked(&e))/>{"人工确认支持档位"}</label><Show when=move||manual_on.get()><div class="account-field"><label for=field("manual")>"已明确确认的档位（逗号分隔）"</label><input id=field("manual") prop:value=manual on:input=move|e|manual.set(event_target_value(&e)) aria-describedby=field("help")/><p id=field("help") class="account-help">"仅填写你已核实支持的值，来源会标为人工确认。先保存档位，再选择默认；关闭后恢复目录判断。"</p></div></Show>
    <p class="account-error" role="alert">{move||error.get()}</p><p class="account-success" role="status">{move||message.get()}</p><button type="submit" class="primary" disabled=move||busy.get()||channel.is_none()||!loaded.get()>{move||if busy.get(){"正在保存…"}else{"保存推理参数"}}</button></form></section>}
}
#[cfg(test)]
mod tests {
    use super::*;
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
