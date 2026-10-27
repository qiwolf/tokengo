//! Provider/model/API-version request contracts. No parameter is silently dropped.
//!
//! Inventory is documentation-backed, not a promise of model availability, real
//! inference acceptance, native transport implementation or cross-protocol parity.
use crate::protocol::Protocol;
use serde_json::{json, Value};

pub struct Context<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub protocol: Protocol,
    pub api_version: Option<&'a str>,
    pub is_oauth: bool,
    pub profile_override: Option<&'a str>,
}

pub fn profile_supported(name: &str) -> bool {
    matches!(name, "openai" | "anthropic" | "gemini" | "deepseek" | "dashscope"
        | "moonshot" | "zai" | "minimax" | "ark" | "siliconflow" | "mimo" | "ollama" | "vllm" | "native")
}

/// Explicit source configuration wins. Automatic matching is exact official URL,
/// never an owned_by value, model-name substring, local port or DNS suffix guess.
pub fn provider(ctx: &Context<'_>) -> &'static str {
    if let Some(name) = ctx.profile_override {
        return match name {
            "openai" => "openai", "anthropic" => "anthropic", "gemini" => "gemini",
            "deepseek" => "deepseek", "dashscope" => "dashscope", "moonshot" => "moonshot",
            "zai" => "zai", "minimax" => "minimax", "ark" => "ark",
            "siliconflow" => "siliconflow", "mimo" => "mimo", "ollama" => "ollama", "vllm" => "vllm",
            _ => "unknown",
        };
    }
    // The separately authenticated OAuth executor fixes the OpenAI transport;
    // this flag comes from its source binding, never from the client JSON body.
    if ctx.is_oauth { return "openai"; }
    let Ok(url) = url::Url::parse(ctx.base_url) else { return "unknown"; };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || url.query().is_some() || url.fragment().is_some()
    { return "unknown"; }
    match (url.host_str(), url.path().trim_end_matches('/')) {
        (Some("api.openai.com"), "/v1") => "openai",
        (Some("api.anthropic.com"), "" | "/v1") => "anthropic",
        (Some("generativelanguage.googleapis.com"), "/v1beta/openai" | "/v1/openai" | "/v1beta" | "/v1") => "gemini",
        (Some("api.deepseek.com"), "" | "/v1" | "/beta" | "/anthropic/v1") => "deepseek",
        (Some("dashscope.aliyuncs.com"), "/compatible-mode/v1" | "/apps/anthropic/v1")
        | (Some("dashscope-intl.aliyuncs.com"), "/compatible-mode/v1") => "dashscope",
        (Some("api.moonshot.ai"), "/v1") => "moonshot",
        (Some("open.bigmodel.cn"), "/api/paas/v4") | (Some("api.z.ai"), "/api/paas/v4") => "zai",
        (Some("api.minimax.io"), "/v1" | "/anthropic/v1") => "minimax",
        (Some("ark.cn-beijing.volces.com"), "/api/v3") => "ark",
        (Some("api.siliconflow.cn"), "/v1") => "siliconflow",
        (Some("api.xiaomimimo.com"), "/v1" | "/anthropic/v1") => "mimo",
        _ => "unknown",
    }
}

fn value<'a>(body: &'a Value, key: &str) -> Option<&'a Value> {
    body.get(key).filter(|v| !v.is_null())
}
fn token<'a>(body: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    value(body, key).map(|v| v.as_str().ok_or_else(|| format!("{key} must be a string"))).transpose()
}
fn number(body: &Value, key: &str) -> Result<Option<f64>, String> {
    value(body, key).map(|v| v.as_f64().filter(|n| n.is_finite())
        .ok_or_else(|| format!("{key} must be a finite number"))).transpose()
}
fn integer(body: &Value, key: &str, minimum: u64) -> Result<Option<u64>, String> {
    value(body, key).map(|v| v.as_u64().filter(|n| *n >= minimum)
        .ok_or_else(|| format!("{key} must be an integer of at least {minimum}"))).transpose()
}
fn object<'a>(body: &'a Value, key: &str) -> Result<Option<&'a Value>, String> {
    value(body, key).map(|v| v.as_object().map(|_| v)
        .ok_or_else(|| format!("{key} must be an object"))).transpose()
}
fn bounded_number(body: &Value, key: &str, min: f64, max: f64) -> Result<(), String> {
    if number(body, key)?.is_some_and(|n| n < min || n > max) {
        return Err(format!("{key} must be between {min} and {max}"));
    }
    Ok(())
}
fn nonempty_array(body: &Value, key: &str) -> Result<bool, String> {
    value(body, key).map(|v| v.as_array().map(|a| !a.is_empty())
        .ok_or_else(|| format!("{key} must be an array"))).transpose().map(|v| v.unwrap_or(false))
}
fn gpt6(model: &str) -> bool {
    matches!(model, "gpt-6-astra" | "gpt-6.1-sol" | "gpt-6-sol" | "gpt-6-luna")
}
fn claude_current(model: &str) -> bool {
    matches!(model, "claude-fable-5-1" | "claude-opus-5-5" | "claude-sonnet-5-5" | "claude-haiku-5-5")
}
fn claude_extended(model: &str) -> bool {
    matches!(model, "claude-opus-4-5" | "claude-sonnet-4-5" | "claude-haiku-4-5")
}

fn validate_openai(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    // Managed OAuth is an independent authenticated Responses transport. Its
    // account-bound catalog, prepare_request and admission rules are authoritative;
    // a public API model page is not proof of that account's accepted parameters.
    if ctx.is_oauth {
        if ctx.protocol != Protocol::Responses {
            return Err("Managed OpenAI OAuth requires its source-bound Responses transport".into());
        }
        return Ok(());
    }
    if ctx.protocol == Protocol::Messages {
        return Err("Official OpenAI does not provide a native Messages endpoint; declare a supported source protocol".into());
    }
    bounded_number(body, "temperature", 0.0, 2.0)?;
    bounded_number(body, "top_p", 0.0, 1.0)?;
    for key in ["frequency_penalty", "presence_penalty"] { bounded_number(body, key, -2.0, 2.0)?; }
    for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] { integer(body, key, 1)?; }
    integer(body, "n", 1)?;
    if value(body, "conversation").is_some() && value(body, "previous_response_id").is_some() {
        return Err("conversation and previous_response_id are mutually exclusive".into());
    }
    if value(body, "stream_options").is_some() && body.get("stream") != Some(&Value::Bool(true)) {
        return Err("stream_options requires stream:true".into());
    }
    if let Some(options) = object(body, "prompt_cache_options")? {
        // Retention maximum and TTL minimum are independent policies; preserving
        // both is not the same as treating them as interchangeable aliases.
        if let Some(mode) = token(options, "mode")? {
            if !matches!(mode, "implicit" | "explicit") { return Err("Unknown prompt_cache_options.mode".into()); }
        }
        if let Some(ttl) = token(options, "ttl")? {
            let accepted = matches!(ttl, "30m" | "12h");
            if !accepted { return Err("This endpoint does not support the requested prompt cache TTL".into()); }
        }
    }
    if !gpt6(ctx.model) { return Ok(()); }
    let requested = if ctx.protocol == Protocol::Chat {
        token(body, "reasoning_effort")?
    } else {
        object(body, "reasoning")?.map(|r| token(r, "effort")).transpose()?.flatten()
    };
    let effort = requested.unwrap_or("medium");
    if matches!(ctx.model, "gpt-6-astra" | "gpt-6.1-sol") && matches!(effort, "none" | "minimal") {
        return Err("This exact OpenAI model does not support none/minimal reasoning; choose a supported value explicitly".into());
    }
    if matches!(ctx.model, "gpt-6-sol" | "gpt-6-luna") && effort == "minimal" {
        return Err("This exact OpenAI model does not declare minimal reasoning".into());
    }
    // Do not invent support or locally narrow a future native effort enum. For
    // known active settings only, enforce the current documented restrictions.
    let active = matches!(effort, "low" | "medium" | "high" | "xhigh" | "max");
    if active {
        for key in ["temperature", "top_p", "top_logprobs"] {
            if value(body, key).is_some() { return Err(format!("{key} is incompatible with active reasoning for this exact OpenAI model")); }
        }
        if ctx.protocol == Protocol::Chat && value(body, "logprobs").is_some() {
            return Err("Chat logprobs is incompatible with active reasoning for this exact OpenAI model".into());
        }
        if body.get("include").and_then(Value::as_array).is_some_and(|items| items.iter().any(|item| item == "message.output_text.logprobs")) {
            return Err("Output text logprobs cannot be requested with active reasoning for this exact OpenAI model".into());
        }
    }
    if ctx.protocol == Protocol::Chat {
        let has_tools = nonempty_array(body, "tools")? || nonempty_array(body, "functions")?;
        let selected_tool = match value(body, "tool_choice").or_else(|| value(body, "function_call")) {
            Some(Value::String(s)) => !matches!(s.as_str(), "none" | "auto"),
            Some(_) => true,
            None => false,
        };
        if has_tools || selected_tool {
            if matches!(ctx.model, "gpt-6-astra" | "gpt-6.1-sol") || effort != "none" {
                return Err("Use Responses for this model/effort tool request; changing effort or removing tools automatically would change intent".into());
            }
        }
    }
    for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        if integer(body, key, 1)?.is_some_and(|n| n > 128_000) {
            return Err(format!("{key} exceeds this exact model's documented output ceiling; account/source limits may be smaller"));
        }
    }
    Ok(())
}

fn recent_anthropic_version(version: Option<&str>) -> bool {
    version.is_some_and(|v| v.len() == 10 && v.as_bytes()[4] == b'-' && v.as_bytes()[7] == b'-'
        && v.bytes().enumerate().all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit())
        && v >= "2026-10-21")
}
fn validate_anthropic(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    if ctx.protocol != Protocol::Messages {
        return Err("Official Anthropic requires native Messages transport; convert before applying its request contract".into());
    }
    // Zero is a native cache warm-up request, not an ordinary generated reply.
    // The gateway admission/billing layer must handle that mode independently.
    integer(body, "max_tokens", 0)?;
    if recent_anthropic_version(ctx.api_version) {
        for key in ["temperature", "top_p", "top_k"] {
            if body.get(key).is_some() { return Err(format!("{key} was removed in Anthropic API version 2026-10-21; it is not silently discarded")); }
        }
    }
    let current = claude_current(ctx.model);
    let effort = object(body, "output_config")?.map(|v| token(v, "effort")).transpose()?.flatten();
    let thinking = object(body, "thinking")?;
    let thinking_type = thinking.map(|t| token(t, "type")).transpose()?.flatten();
    let choice = object(body, "tool_choice")?.map(|t| token(t, "type")).transpose()?.flatten();
    let forced = matches!(choice, Some("any" | "tool"));
    if current {
        if matches!(effort, Some("none" | "adaptive")) {
            return Err("Claude output_config.effort is distinct from a thinking mode; none/adaptive are not native effort values".into());
        }
        if matches!(ctx.model, "claude-opus-5-5" | "claude-fable-5-1" | "claude-sonnet-5-5") && forced {
            return Err("This exact Claude model rejects forced tool use; keep native intent rather than silently changing tool_choice".into());
        }
        match (ctx.model, thinking_type) {
            (_, Some("enabled")) => return Err("This exact Claude model is adaptive-only; do not invent a manual thinking budget".into()),
            ("claude-opus-5-5" | "claude-fable-5-1", Some("disabled" | "between_tools")) => return Err("Thinking cannot be disabled on this exact Claude model".into()),
            ("claude-sonnet-5-5", Some("disabled")) => return Err("Claude Sonnet 5.5 does not support disabled; between_tools is a different, not fully-off mode".into()),
            ("claude-haiku-5-5", Some("between_tools")) => return Err("Claude Haiku 5.5 does not support between_tools".into()),
            _ => {}
        }
        if matches!((ctx.model, thinking_type), ("claude-sonnet-5-5", Some("between_tools")) | ("claude-haiku-5-5", Some("disabled")))
            && matches!(effort, Some("xhigh" | "max"))
        { return Err("This thinking-off mode requires Claude effort high or below; effort will not be lowered automatically".into()); }
        if !recent_anthropic_version(ctx.api_version) {
            if number(body, "temperature")?.is_some_and(|n| n != 1.0)
                || number(body, "top_p")?.is_some_and(|n| n != 0.99)
            { return Err("This exact Claude model accepts only its documented sampling defaults".into()); }
            if body.get("temperature").is_some() && body.get("top_p").is_some() {
                return Err("This exact Claude model rejects temperature and top_p together, including their defaults".into());
            }
        }
        if integer(body, "max_tokens", 0)?.is_some_and(|n| n > 128_000) {
            return Err("max_tokens exceeds this exact Claude model's ordinary Messages ceiling; batch beta limits are a separate endpoint contract".into());
        }
    }
    if thinking_type == Some("enabled") {
        let thinking = thinking.expect("type came from this object");
        if let Some(budget) = value(thinking, "budget_tokens") {
            let budget = budget.as_u64().filter(|n| *n >= 1024).ok_or("Manual thinking budget_tokens must be an integer of at least 1024")?;
            if integer(body, "max_tokens", 0)?.is_some_and(|cap| cap > 0 && budget >= cap) {
                return Err("Manual thinking budget_tokens must be below the generation max_tokens cap".into());
            }
        } else if claude_extended(ctx.model) {
            return Err("This exact extended-thinking model requires budget_tokens when thinking is enabled".into());
        }
        if forced { return Err("Forced tools are incompatible with manual extended thinking".into()); }
    }
    Ok(())
}

fn validate_gemini(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    if ctx.protocol != Protocol::Chat {
        return Err("This Gemini profile is for its OpenAI-compatible Chat endpoint; native generateContent is a different transport".into());
    }
    if ctx.profile_override.is_none() {
        if let Ok(url) = url::Url::parse(ctx.base_url) {
            if !matches!(url.path().trim_end_matches('/'), "/v1beta/openai" | "/v1/openai") {
                return Err("Use Gemini's OpenAI-compatible base for Chat; native generateContent cannot be advertised as a Chat/Responses/Messages adapter".into());
            }
        }
    }
    let effort = token(body, "reasoning_effort")?;
    let thinking = body.pointer("/extra_body/google/thinking_config").filter(|v| !v.is_null());
    if let Some(thinking) = thinking {
        if !thinking.is_object() { return Err("extra_body.google.thinking_config must be an object".into()); }
        if value(thinking, "thinking_level").is_some_and(|v| !v.is_string()) {
            return Err("Gemini thinking_level must be a native string value".into());
        }
        if value(thinking, "thinking_budget").is_some_and(|v| v.as_i64().is_none_or(|n| n < -1)) {
            return Err("Gemini thinking_budget must be an integer at least -1".into());
        }
        if value(thinking, "include_thoughts").is_some_and(|v| !v.is_boolean()) {
            return Err("Gemini include_thoughts must be boolean".into());
        }
        if effort.is_some() && ["thinking_level", "thinking_budget"].iter().any(|key| value(thinking, key).is_some()) {
            return Err("Gemini reasoning_effort overlaps thinking_level/thinking_budget; choose one explicit control".into());
        }
    }
    if matches!(ctx.model, "gemini-2.5-pro" | "gemini-3-pro-preview" | "gemini-3-flash-preview" | "gemini-4-argon" | "gemini-3.8-flash")
        && (effort == Some("none") || thinking.and_then(|v| v.get("thinking_budget")).and_then(Value::as_i64) == Some(0))
    { return Err("This exact Gemini model does not declare thinking-off through none or a zero thinking budget".into()); }
    if matches!(ctx.model, "gemini-4-argon" | "gemini-3.8-flash") {
        if thinking.and_then(|v| v.get("thinking_level")).and_then(Value::as_str) == Some("minimal") {
            return Err("This exact Gemini native thinking_level does not support minimal; compatibility reasoning_effort aliases are a separate contract".into());
        }
        // On Argon the compatibility API documents minimal -> native low. Do
        // not incorrectly reuse the native enum to reject that accepted alias.
        // The corresponding 3.8 compatibility alias is left to the provider.
    }
    if ctx.model == "gemini-4-argon" && value(body, "extra_body").and_then(|v| v.pointer("/google/cached_content")).is_some_and(|v| !v.is_null()) {
        return Err("Gemini 4 Argon supports implicit caching, not an explicit cached_content resource".into());
    }
    Ok(())
}

/// Resource/state controls require execution lifecycle and final metering, not
/// just a provider schema. Call this on the original request before conversion as
/// well, so an explicit native profile or a transform cannot bypass the gate.
pub fn validate_executor_controls(body: &Value) -> Result<(), String> {
    if body.get("background").is_some_and(|v| !v.is_null() && v != false) {
        return Err("Background execution is not enabled: async polling, cancellation and final usage settlement require a dedicated executor".into());
    }
    if body.get("prompt").is_some_and(|v| !v.is_null()) {
        return Err("Provider prompt templates are not enabled: template identity and hidden-input admission require a dedicated executor".into());
    }
    if body.get("context_management").is_some_and(|v| !v.is_null()) {
        return Err("Provider context management is not enabled: compaction/state output and cost handling require a dedicated executor".into());
    }
    if body.pointer("/prompt_cache_options/prewarm").is_some_and(|v| !v.is_null() && v != false) {
        return Err("Responses cache prewarm is not enabled: its zero-output lifecycle and input/cache billing require a dedicated executor".into());
    }
    Ok(())
}

/// Validate only documented source contracts. Unknown native fields remain
/// untouched; cross-protocol matching and execution support are separate gates.
pub fn validate(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    if !body.is_object() { return Err("Request must be an object".into()); }
    validate_executor_controls(body)?;
    if ctx.profile_override.is_some_and(|name| !profile_supported(name)) {
        return Err("Unknown explicit parameter profile".into());
    }
    if ctx.is_oauth && ctx.profile_override.is_some_and(|name| name != "openai") {
        return Err("Independent OpenAI OAuth cannot use another provider parameter profile".into());
    }
    match provider(ctx) {
        "openai" => validate_openai(ctx, body),
        "anthropic" => validate_anthropic(ctx, body),
        "gemini" => validate_gemini(ctx, body),
        // China/local provider rules are supplied by the separate registry.
        _ => Ok(()),
    }
}

fn inventory_ref() -> &'static Value {
    static INVENTORY: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    INVENTORY.get_or_init(|| serde_json::from_str(include_str!("parameter_contracts_inventory.json"))
        .expect("bundled parameter inventory must be valid JSON"))
}
pub fn inventory() -> Value {
    let mut inventory = inventory_ref().clone();
    inventory["contract_evidence"] = crate::contract_evidence::registry();
    inventory
}

/// An administrator's legacy drop list is not a lossless parameter adapter.
/// Refuse to erase known control/content/cost/state fields supplied by a client.
pub fn protect_explicit_controls(body: &Value, dropped: &[String]) -> Result<(), String> {
    for field in dropped {
        if body.get(field).is_none() { continue; }
        let known = inventory_ref()["parameters"].as_array().is_some_and(|items|
            items.iter().any(|item|item["name"].as_str()==Some(field.as_str())))
            || matches!(field.as_str(), "thinking" | "thinking_budget" | "enable_thinking" | "clear_thinking"
                | "chat_template_kwargs" | "extra_body" | "output_config" | "reasoning" | "stream_options"
                | "num_predict" | "options" | "keep_alive" | "cache_salt" | "structured_outputs");
        if known { return Err(format!("Configured drop_fields would erase explicit parameter {field}; choose a matching native protocol or provider contract instead")); }
    }
    Ok(())
}

/// Bounded per-model summary: the full inventory belongs once at API top-level,
/// not once per model in a catalog with thousands of entries.
pub fn describe(ctx: &Context<'_>) -> Value {
    let p = provider(ctx);
    let known = !ctx.is_oauth && match p { "openai" => gpt6(ctx.model), "anthropic" => claude_current(ctx.model) || claude_extended(ctx.model),
        "gemini" => matches!(ctx.model, "gemini-4-argon" | "gemini-3.8-flash" | "gemini-2.5-pro" | "gemini-2.5-flash" | "gemini-2.5-flash-lite"), _ => false };
    let protocol = match ctx.protocol { Protocol::Chat => "chat", Protocol::Responses => "responses", Protocol::Messages => "messages" };
    json!({"provider":p,"selection":if ctx.profile_override.is_some(){"manual_profile"}else if ctx.is_oauth{"authenticated_oauth_transport"}else if p=="unknown"{"unknown"}else{"exact_official_endpoint"},
        "model_rules":if ctx.is_oauth{"source_bound_oauth_catalog"}else if known{"exact_documented_model"}else{"unknown_or_separate_provider_registry"},"protocol":protocol,
        "authentication_line":if ctx.is_oauth && ctx.profile_override.is_some_and(|name|name!="openai"){"invalid_oauth_profile"}else if ctx.is_oauth{"managed_openai_oauth"}else{"externally_configured_credentials"},
        "managed_oauth_implementation":"openai_only","parameter_profile_grants_authentication":false,
        "identity_scope":"source/account/credential/protocol/generation; native continuation also key/model/session",
        "authentication_availability":"not_inferred_from_parameter_description",
        "api_version":ctx.api_version.filter(|v|v.len()==10 && v.bytes().enumerate().all(|(i,b)|if i==4||i==7{b==b'-'}else{b.is_ascii_digit()})),"inventory_ref":protocol,"unknown_native_parameters":"preserved",
        "automatic_parameter_dropping":false,"automatic_effort_downgrade":false,"source":"documented_contract_not_live_inference",
        "schema_observed_at":"2026-10-25","gemini_native_generate_content_transport":false,
        "contract_evidence":crate::contract_evidence::describe(ctx),
        "oauth_profile_compatible":!ctx.is_oauth || ctx.profile_override.is_none_or(|name|name=="openai")})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ctx<'a>(url: &'a str, model: &'a str, protocol: Protocol) -> Context<'a> {
        Context { base_url:url, model, protocol, api_version:None, is_oauth:false, profile_override:None }
    }
    #[test]
    fn versioned_evidence_identity_does_not_grant_runtime_capabilities() {
        let a = describe(&ctx("https://ark.cn-beijing.volces.com/api/v3", "doubao-seed-2-0-pro", Protocol::Chat));
        assert_eq!(a["contract_evidence"]["revision"], "2026-10-27-p0.1");
        assert_eq!(a["contract_evidence"]["identity"]["model"], "doubao-seed-2-0-pro");
        assert_eq!(a["contract_evidence"]["identity"]["authentication_line"], "externally_configured_credentials");
        assert_eq!(a["contract_evidence"]["version_verification"], "required_before_new_capability");
        assert_eq!(a["contract_evidence"]["live_inference_verified"], false);
        let unknown = describe(&ctx("https://example.invalid/v1", "doubao-seed-2-0-pro", Protocol::Chat));
        assert_eq!(unknown["contract_evidence"]["identity"]["provider"], "unknown");
        assert_eq!(unknown["contract_evidence"]["rules"], json!([]));
        let oauth = Context {is_oauth:true, ..ctx("https://api.openai.com/v1", "gpt-6.1-terra", Protocol::Responses)};
        assert_eq!(describe(&oauth)["contract_evidence"]["identity"]["authentication_line"], "managed_openai_oauth");
    }
    #[test]
    fn endpoint_and_manual_profiles_are_explicit_and_do_not_infer_from_names_or_ports() {
        assert_eq!(provider(&ctx("https://api.openai.com/v1", "anything", Protocol::Chat)), "openai");
        assert_eq!(provider(&ctx("https://api.openai.com.attacker.test/v1", "gpt-6-luna", Protocol::Chat)), "unknown");
        assert_eq!(provider(&ctx("http://localhost:11434/v1", "Qwen", Protocol::Chat)), "unknown");
        let mut c=ctx("http://private.test/proxy", "gpt-6-luna", Protocol::Chat); c.profile_override=Some("openai");
        assert_eq!(provider(&c), "openai"); assert_eq!(describe(&c)["selection"], "manual_profile");
        for name in ["openai","anthropic","gemini","deepseek","dashscope","moonshot","zai","minimax","ark","siliconflow","mimo","ollama","vllm"] { assert!(profile_supported(name)); }
        assert!(!profile_supported("auto_guess"));
    }
    #[test]
    fn oauth_profile_cannot_bypass_its_source_contract() {
        let mut c=ctx("https://api.openai.com/v1", "gpt-6-luna", Protocol::Responses); c.is_oauth=true; c.profile_override=Some("ollama");
        assert!(validate(&c,&json!({})).is_err());
        c.profile_override=Some("openai"); assert!(validate(&c,&json!({"reasoning":{"effort":"none"}})).is_ok());
    }
    #[test]
    fn gpt6_tools_sampling_and_exact_model_rules_preserve_intent() {
        let lunar=ctx("https://api.openai.com/v1", "gpt-6-luna", Protocol::Chat);
        let body=json!({"reasoning_effort":"none","temperature":0.2,"tools":[{"type":"function","function":{"name":"x"}}],"future_native_field":{"x":1}});
        let before=body.clone(); assert!(validate(&lunar,&body).is_ok()); assert_eq!(body,before);
        assert!(validate(&lunar,&json!({"tools":[{}]})).is_err());
        let sol=ctx("https://api.openai.com/v1", "gpt-6.1-sol", Protocol::Responses);
        assert!(validate(&sol,&json!({"reasoning":{"effort":"none"}})).is_err());
        assert!(validate(&sol,&json!({"temperature":1})).is_err());
        let unknown=ctx("https://proxy.test/v1", "gpt-6.1-sol", Protocol::Chat);
        assert!(validate(&unknown,&json!({"temperature":0.3,"future_native_field":1})).is_ok());
    }
    #[test]
    fn cache_policies_are_not_renamed_and_stream_options_require_streaming() {
        let c=ctx("https://api.openai.com/v1", "gpt-6-luna", Protocol::Chat);
        assert!(validate(&c,&json!({"reasoning_effort":"none","prompt_cache_options":{"mode":"explicit","ttl":"30m"}})).is_ok());
        assert!(validate(&c,&json!({"prompt_cache_options":{"ttl":"12h"}})).is_ok(),"conflicting official TTL documentation is not a proven model rejection");
        assert!(validate(&c,&json!({"prompt_cache_options":{"ttl":"30m"},"prompt_cache_retention":"24h"})).is_ok());
        assert!(validate(&c,&json!({"stream_options":{"include_usage":true}})).is_err());
    }
    #[test]
    fn anthropic_sampling_rules_use_actual_api_version_and_preserve_native_extras() {
        let mut c=ctx("https://api.anthropic.com/v1", "claude-sonnet-4-6", Protocol::Messages);
        c.api_version=Some("2023-06-01"); assert!(validate(&c,&json!({"temperature":0.2,"future":{"x":1}})).is_ok());
        c.api_version=Some("2026-10-21");
        for key in ["temperature","top_p","top_k"] { let mut body=json!({}); body[key]=Value::Null; assert!(validate(&c,&body).is_err()); }
        assert!(validate(&c,&json!({"max_tokens":0})).is_ok(),"native cache warm-up is not normal positive-output generation");
    }
    #[test]
    fn anthropic_modes_budgets_and_forced_tools_are_not_equivalent() {
        let sonnet=ctx("https://api.anthropic.com/v1", "claude-sonnet-5-5", Protocol::Messages);
        assert!(validate(&sonnet,&json!({"thinking":{"type":"disabled"}})).is_err());
        assert!(validate(&sonnet,&json!({"thinking":{"type":"between_tools"},"output_config":{"effort":"high"}})).is_ok());
        assert!(validate(&sonnet,&json!({"thinking":{"type":"between_tools"},"output_config":{"effort":"max"}})).is_err());
        assert!(validate(&sonnet,&json!({"tool_choice":{"type":"any"}})).is_err());
        let legacy=ctx("https://api.anthropic.com/v1", "claude-sonnet-4-5", Protocol::Messages);
        assert!(validate(&legacy,&json!({"max_tokens":4096,"thinking":{"type":"enabled","budget_tokens":1024}})).is_ok());
        assert!(validate(&legacy,&json!({"max_tokens":1024,"thinking":{"type":"enabled","budget_tokens":1024}})).is_err());
        assert!(validate(&legacy,&json!({"thinking":{"type":"enabled","budget_tokens":1024},"tool_choice":{"type":"any"}})).is_err());
    }
    #[test]
    fn gemini_native_and_compatibility_effort_fields_are_not_confused() {
        let c=ctx("https://generativelanguage.googleapis.com/v1beta/openai", "gemini-4-argon", Protocol::Chat);
        assert!(validate(&c,&json!({"reasoning_effort":"minimal"})).is_ok(),"compatibility minimal maps to low; native enum is different");
        assert!(validate(&c,&json!({"reasoning_effort":"none"})).is_err());
        assert!(validate(&c,&json!({"extra_body":{"google":{"thinking_config":{"thinking_budget":0}}}})).is_err());
        assert!(validate(&c,&json!({"extra_body":{"google":{"thinking_config":{"thinking_budget":true}}}})).is_err());
        assert!(validate(&c,&json!({"reasoning_effort":"low","extra_body":{"google":{"thinking_config":{"thinking_level":"low"}}}})).is_err());
        assert!(validate(&c,&json!({"extra_body":{"google":{"thinking_config":{"thinking_level":"minimal"}}}})).is_err());
        assert!(validate(&ctx("https://generativelanguage.googleapis.com/v1beta", "gemini-4-argon", Protocol::Chat),&json!({})).is_err());
    }
    #[test]
    fn public_api_model_restrictions_do_not_override_managed_oauth_catalog() {
        let mut c=ctx("https://api.openai.com/v1", "gpt-6-astra", Protocol::Responses);
        let body=json!({"reasoning":{"effort":"none"}});
        assert!(validate(&c,&body).is_err(), "public API model contract declares none unavailable");
        c.is_oauth=true;
        assert!(validate(&c,&body).is_ok(), "actual OAuth source catalog is validated by its own gate");
        assert!(validate(&c,&json!({"reasoning":{"effort":"minimal"}})).is_ok());
        assert_eq!(describe(&c)["authentication_line"], "managed_openai_oauth");
        assert_eq!(describe(&c)["model_rules"], "source_bound_oauth_catalog");
        c.protocol=Protocol::Chat;
        assert!(validate(&c,&body).is_err(), "managed OAuth does not create native Chat support");
        c.protocol=Protocol::Responses; c.profile_override=Some("native");
        assert!(validate(&c,&body).is_err(), "native override cannot bypass managed OAuth identity");
    }
    #[test]
    fn resource_execution_is_not_enabled_by_a_native_or_vendor_profile() {
        let mut c=ctx("https://custom.example.test/v1", "unknown-model", Protocol::Responses);
        c.profile_override=Some("native");
        for body in [json!({"background":true}),json!({"prompt":{"id":"p"}}),json!({"context_management":[]}),json!({"prompt_cache_options":{"prewarm":true}})] {
            let before=body.clone(); assert!(validate(&c,&body).is_err()); assert_eq!(body,before);
        }
        assert!(validate(&c,&json!({"background":false,"prompt_cache_options":{"prewarm":false},"future_native_extension":1})).is_ok());
        assert_eq!(describe(&c)["authentication_line"], "externally_configured_credentials");
        assert_eq!(describe(&c)["managed_oauth_implementation"], "openai_only");
        assert_eq!(describe(&c)["parameter_profile_grants_authentication"], false);
    }
    #[test]
    fn descriptor_is_bounded_and_inventory_is_not_a_support_claim() {
        let c=ctx("https://api.openai.com/v1","gpt-6-luna",Protocol::Responses);
        assert!(describe(&c).to_string().len()<2048); assert!(describe(&c).get("parameters").is_none());
        let i=inventory(); assert!(i["parameters"].as_array().unwrap().len()>=127); assert_eq!(i["runtime_acceptance"],"not_executed");
    }
}
