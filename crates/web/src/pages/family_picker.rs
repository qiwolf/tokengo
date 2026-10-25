//! Approved family cards: presentation only, never source entitlement or capability inference.
use leptos::prelude::*;
use serde_json::{Value, json};

pub const FAMILIES: &[(&str, &str)] = &[
    ("gpt", "GPT"), ("claude", "Claude"), ("gemini", "Gemini"), ("deepseek", "DeepSeek"),
    ("qwen", "Qwen"), ("glm", "GLM"), ("kimi", "Kimi"), ("mimo", "MiMo"),
    ("minimax", "MiniMax"), ("doubao", "Doubao"), ("grok", "Grok"), ("mistral", "Mistral"),
    ("llama", "Meta / Llama"), ("cohere", "Cohere"), ("other", "其他 / 多家族"),
];
pub fn family_label(id: &str) -> &str {
    FAMILIES.iter().find(|(key, _)| *key == id).map(|(_, label)| *label).unwrap_or("其他 / 多家族")
}
/// Keep the selected extra family visible after collapse without adding a third row.
fn collapsed_ids(selected: &str, count: usize) -> Vec<&'static str> {
    let mut ids = FAMILIES.iter().take(count).map(|(id, _)| *id).collect::<Vec<_>>();
    if let Some((id, _)) = FAMILIES.iter().find(|(id, _)| *id == selected) {
        if !ids.contains(id) { ids.pop(); ids.push(id); }
    }
    ids
}
#[component]
pub fn FamilyPicker(draft: RwSignal<Value>, on_changed: Callback<()>) -> impl IntoView {
    let expanded = RwSignal::new(false);
    view! {
        <fieldset class="family-picker"><legend>"模型家族"</legend>
        <div class="family-grid" class:is-expanded=move||expanded.get()>
        {FAMILIES.iter().map(|&(id,label)| {
            view!{<button type="button" class="family-card"
                class:is-selected=move||draft.get()["_model_family"]==id
                class:desktop-visible=move||expanded.get()||collapsed_ids(draft.get()["_model_family"].as_str().unwrap_or(""),8).contains(&id)
                class:mobile-visible=move||expanded.get()||collapsed_ids(draft.get()["_model_family"].as_str().unwrap_or(""),4).contains(&id)
                aria-pressed=move||(draft.get()["_model_family"]==id).to_string()
                aria-label=format!("选择 {label}")
                on:click=move |_|{draft.update(|d|{d["_model_family"]=json!(id);d["oauth_account_id"]=Value::Null;d["_oauth_provider"]=Value::Null;});on_changed.run(());}>
                {if id=="other" {view!{<span class="family-other" aria-hidden="true"><i></i><i></i><i></i><i></i></span>}.into_any()}
                else {view!{<img src=format!("/ui/family-logos/{id}.svg") alt="" width="30" height="30"/>}.into_any()}}
                <span>{label}</span><span class="family-selected-mark" aria-hidden="true">{move||if draft.get()["_model_family"]==id{"✓"}else{""}}</span>
            </button>}
        }).collect_view()}
        </div>
        <button type="button" class="family-more" aria-expanded=move||expanded.get().to_string() on:click=move |_|expanded.update(|v|*v=!*v)>
            <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" class:is-expanded=move||expanded.get()><path d="m6 9 6 6 6-6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            {move||if expanded.get(){"收起模型"}else{"更多模型"}}
        </button>
        <p class="account-help">"同一凭据可以包含多个家族；这里的选择用于匹配接入方式，不会删减模型目录。"</p>
        </fieldset>
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn selected_extra_family_stays_inside_two_rows() {
        assert_eq!(collapsed_ids("gpt",8).len(),8);
        let ids=collapsed_ids("minimax",8);assert_eq!(ids.len(),8);assert!(ids.contains(&"minimax"));
        let ids=collapsed_ids("minimax",4);assert_eq!(ids.len(),4);assert!(ids.contains(&"minimax"));
        assert_eq!(family_label("llama"),"Meta / Llama");
    }
}
