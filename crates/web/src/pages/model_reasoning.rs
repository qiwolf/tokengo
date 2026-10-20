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
        "user_confirmed" => "人工确认",
        _ => "来源未知",
    };
    let status = match c.status.as_str() {
        "supported" if !c.levels.is_empty() => format!("支持档位：{}", c.levels.join(" / ")),
        "supported" => "支持推理，档位未知".into(),
        "unsupported" => "明确不支持推理".into(),
        _ => "推理档位未知，待确认".into(),
    };
    format!("{status} · {source}")
}
#[component]
pub fn ReasoningConfig(ctx: Ctx, channel: Option<i64>, model: String) -> impl IntoView {
    let model = StoredValue::new(model);
    let data = RwSignal::new(ReasoningCapability::default());
    let manual = RwSignal::new(String::new());
    let manual_on = RwSignal::new(false);
    let default = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let load =
        Callback::new(move |()| {
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
    let refresh=move |_|{let Some(id)=channel else{return;};if busy.get_untracked(){return;}busy.set(true);error.set(String::new());leptos::task::spawn_local(async move{match api::refresh_channel_metadata(id).await{Ok(r) if r.ok=>{message.set("已刷新真实目录元数据，未发送推理请求。".into());load.run(());},Ok(r)=>error.set(r.note.unwrap_or_else(||"元数据获取失败，现有配置保留。".into())),Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});};
    let field = move |name: &str| {
        format!(
            "reasoning-{}-{}-{}",
            channel.unwrap_or(0),
            model.get_value(),
            name
        )
    };
    view! {<section class="reasoning-config"><h3>{channel.map(|id|format!("连接 #{id} 的原生推理参数")).unwrap_or_else(||"新模型推理参数（请先保存）".into())}</h3><p>{move||capability_label(&data.get())}</p><p class="muted small">{move||data.get().reason}</p><p class="muted small">{move||data.get().default_effort.map(|v|format!("当前默认：{v}（{}）",if data.get().configured_default{"本模型配置"}else{"服务商声明"})).unwrap_or_else(||if data.get().configured_default{"使用服务商默认（不注入档位）".into()}else{"目录未声明默认档位".into()})}</p><p class="muted small">"来自上游目录，未做推理实测；跨协议支持以接入协议为准。目录识别不发付费推理，没有声明具体档位时保持未知；推理档位不另建模型名。客户端 reasoning.effort / reasoning_effort 与 service_tier 显式值优先。"</p>
    <button type="button" class="ghost" disabled=move||busy.get()||channel.is_none() on:click=refresh>"刷新能力元数据"</button><form class="account-form" on:submit=save><div class="account-field"><label for=field("default")>"默认推理档位"</label><select id=field("default") prop:value=default disabled=move||busy.get()||channel.is_none() on:change=move|e|default.set(event_target_value(&e))><option value="">"使用服务商默认"</option>{move||data.get().levels.into_iter().map(|level|view!{<option value=level.clone()>{level.clone()}</option>}).collect_view()}</select></div>
    <label class="pick"><input type="checkbox" prop:checked=manual_on on:change=move|e|manual_on.set(event_target_checked(&e))/>{"人工确认支持档位"}</label><Show when=move||manual_on.get()><div class="account-field"><label for=field("manual")>"已明确确认的档位（逗号分隔）"</label><input id=field("manual") prop:value=manual on:input=move|e|manual.set(event_target_value(&e)) aria-describedby=field("help")/><p id=field("help") class="account-help">"仅填写你已核实支持的值，来源会标为人工确认。先保存档位，再选择默认；关闭后恢复目录判断。"</p></div></Show>
    <p class="account-error" role="alert">{move||error.get()}</p><p class="account-success" role="status">{move||message.get()}</p><button type="submit" class="ghost" disabled=move||busy.get()||channel.is_none()>{move||if busy.get(){"正在保存…"}else{"保存推理参数"}}</button></form></section>}
}
#[cfg(test)]
mod tests {
    use super::*;
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
    fn manual_confirmation_deduplicates_only_explicit_values() {
        assert_eq!(
            confirmed_levels(" light, custom ,light, "),
            vec!["light", "custom"]
        );
        assert!(confirmed_levels(" , ").is_empty());
    }
}
