//! Service presets are connection shortcuts, never model families or entitlements.
use leptos::prelude::*;
use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub(crate) struct ProviderPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub base_url: &'static str,
}
pub(crate) const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset { id: "opencode-go", label: "OpenCode Go", icon: "opencode", base_url: "https://opencode.ai/inference/go/openai/v1" },
    ProviderPreset { id: "opencode", label: "OpenCode 按量", icon: "opencode", base_url: "https://opencode.ai/inference/openai/v1" },
    ProviderPreset { id: "commandgo", label: "CommandGo", icon: "commandcode", base_url: "https://api.commandcode.ai/provider/v1" },
    ProviderPreset { id: "opendesign", label: "OpenDesign", icon: "opendesign", base_url: "https://amr-link.open-design.ai/v1" },
];
fn apply_preset(draft: &mut Value, preset: ProviderPreset) {
    let name=draft["_provider_name"].as_str().unwrap_or("");
    if name.trim().is_empty() || PROVIDER_PRESETS.iter().any(|p|p.label==name) {
        draft["_provider_name"]=json!(preset.label);
    }
    draft["_provider_preset"]=json!(preset.id);
    draft["_provider_base_url"]=json!(preset.base_url);
    draft["_model_family"]=json!("other");
    draft["_endpoint_mode"]=json!("single");
    draft["kind"]=json!("api_key");
    draft["oauth_account_id"]=Value::Null;
    draft["_oauth_provider"]=Value::Null;
}

pub(crate) fn is_preset_api_base(base: &str) -> bool {
    PROVIDER_PRESETS.iter().any(|p|p.base_url==base)
}

#[component]
pub fn ProviderPresetPicker(draft: RwSignal<Value>, on_changed: Callback<()>) -> impl IntoView {
    view! {
        <fieldset class="family-picker provider-preset-picker"><legend>"常用服务商"</legend>
        <div class="family-grid">
        {PROVIDER_PRESETS.iter().map(|&preset|view!{
            <button type="button" class="family-card desktop-visible mobile-visible"
                class:is-selected=move||draft.get()["_provider_preset"]==preset.id
                aria-pressed=move||(draft.get()["_provider_preset"]==preset.id).to_string()
                aria-label=format!("选择 {}",preset.label)
                on:click=move |_|{draft.update(|d|apply_preset(d,preset));on_changed.run(());}>
                <img src=format!("/ui/family-logos/{}.svg",preset.icon) alt="" width="30" height="30"/>
                <span>{preset.label}</span>
                <span class="family-selected-mark" aria-hidden="true">{move||if draft.get()["_provider_preset"]==preset.id{"✓"}else{""}}</span>
            </button>
        }).collect_view()}
        </div>
        <p class="account-help">"选择后自动填写 API 地址，仍可修改。各服务商支持的模型与套餐权限以账号实际情况为准。"</p>
        </fieldset>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn selecting_a_service_fills_its_current_api_url_and_uses_api_authentication() {
        for preset in PROVIDER_PRESETS {
            let mut draft=json!({"_provider_name":"","_provider_base_url":"https://api.openai.com/v1","kind":"oauth","_endpoint_mode":"local","_model_family":"gpt","oauth_account_id":42,"_oauth_provider":"openai","models":[]});
            apply_preset(&mut draft,*preset);
            assert_eq!(draft["_provider_base_url"],preset.base_url);
            assert_eq!(draft["_provider_name"],preset.label);
            assert_eq!(draft["_provider_preset"],preset.id);
            assert_eq!(draft["kind"],"api_key");
            assert_eq!(draft["_endpoint_mode"],"single");
            assert_eq!(draft["_model_family"],"other");
            assert!(draft["oauth_account_id"].is_null());
            assert!(draft["_oauth_provider"].is_null());
            assert_eq!(draft["models"],json!([]));
        }
    }
    #[test]
    fn changing_services_replaces_the_default_name_but_preserves_a_custom_name() {
        let mut draft=json!({"_provider_name":"OpenCode Go"});
        apply_preset(&mut draft,PROVIDER_PRESETS[2]);
        assert_eq!(draft["_provider_name"],"CommandGo");
        draft["_provider_name"]=json!("团队备用");
        apply_preset(&mut draft,PROVIDER_PRESETS[3]);
        assert_eq!(draft["_provider_name"],"团队备用");
    }
}
