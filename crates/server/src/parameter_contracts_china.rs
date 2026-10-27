//! Documentary provider contracts. Native extensions are retained, not filtered.
//! These rules do not establish account availability or live-inference acceptance.
use crate::{parameter_contracts::{provider, Context}, protocol::Protocol};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSemantics {
    Total,
    /// No documented total/answer limit for this exact source/model/protocol.
    Unknown,
    AnswerOnly { reasoning_budget: Option<i64> },
}

const QWEN_HYBRID: &[&str] = &[
    "qwen3.8-max", "qwen3.8-max-0902", "qwen3.8-flash", "qwen3.8-27b",
    "qwen3.7-max", "qwen3.7-plus", "qwen3.7-flash", "qwen3.6-plus", "qwen3.6-flash",
    "qwen3.5-plus", "qwen3.5-flash", "qwen3-vl-plus", "qwen3.8-omni-flash",
    "qwen3.5-omni-plus", "qwen3.5-omni-flash",
];
const QWEN_EFFORT_BUDGET: &[&str] = &[
    "qwen3.8-max", "qwen3.8-max-0902", "qwen3.8-flash", "qwen3.8-27b",
];
const QWEN_TOTAL_CAP: &[&str] = &[
    "deepseek-v4.1-flash", "deepseek-v4-pro", "deepseek-v4-pro-0813", "deepseek-v4-flash-0731",
    "glm-5.3",
];
const QWEN_INCLUSIVE_CAP: &[&str] = &[
    "qwen3.8-max", "qwen3.8-max-0902", "qwen3.8-flash", "qwen3.7-max", "qwen3.7-plus",
    "qwen3.7-flash", "qwen3.6-plus", "qwen3.6-flash", "qwen3.5-plus", "qwen3.5-flash",
    "glm-5.2", "glm-5.3", "deepseek-v4.1-flash", "deepseek-v4-pro", "deepseek-v4-pro-0813", "deepseek-v4-flash-0731",
];
// 2026-10-27 native Responses reference. These are exact deployed IDs,
// not Chat max_completion_tokens capabilities or model-name prefix guesses.
const QWEN_RESPONSES_TOTAL_CAP: &[&str] = &[
    "qwen3.8-max", "qwen3.8-max-0902", "qwen3.8-flash", "qwen3.8-2.4t-a95b",
    "qwen3.8-27b", "qwen3.8-omni-flash",
];
const QWEN_RESPONSES_ANSWER_CAP: &[&str] = &[
    "qwen3.7-max", "qwen3.7-max-2026-05-20", "qwen3.7-max-2026-06-08", "qwen3.7-max-2026-05-17",
    "qwen3.7-max-preview", "qwen3.7-plus", "qwen3.7-plus-2026-05-26",
    "qwen3.6-plus", "qwen3.6-plus-2026-04-02", "qwen3.5-plus", "qwen3.5-plus-2026-04-20", "qwen3.5-plus-2026-02-15",
    "qwen3.7-flash", "qwen3.7-flash-2026-07-15", "qwen3.6-flash", "qwen3.6-flash-2026-04-16",
    "qwen3.5-flash", "qwen3.5-flash-2026-02-23", "qwen3.6-35b-a3b", "qwen3.5-397b-a17b",
    "qwen3.5-122b-a10b", "qwen3.5-27b", "qwen3.5-35b-a3b",
    "deepseek-v4.1-flash", "deepseek-v4.1-flash-1017", "deepseek-v4-pro", "deepseek-v4-pro-0813", "deepseek-v4-flash-0731",
    "glm-5.3", "glm-5.2", "kimi-k3",
];
fn known_dashscope_responses(model: &str) -> bool {
    QWEN_RESPONSES_TOTAL_CAP.contains(&model) || QWEN_RESPONSES_ANSWER_CAP.contains(&model)
}
fn dashscope_responses_thinking_disabled(ctx: &Context<'_>, body: &Value) -> bool {
    // GLM-5.3 is thinking-only; Kimi's off control is not documented here.
    if matches!(ctx.model, "glm-5.3" | "kimi-k3") { return false; }
    match effort(ctx, body) {
        Some("none") => matches!(ctx.model, "deepseek-v4-pro" | "deepseek-v4-pro-0813" | "deepseek-v4-flash-0731" | "deepseek-v4.1-flash" | "glm-5.2"),
        Some(_) => false, // effort takes precedence over enable_thinking.
        None => body.get("enable_thinking") == Some(&json!(false)),
    }
}

const KIMI_K3: &[&str] = &["kimi-k3.1-pro", "kimi-k3.1-pro-highspeed", "kimi-k3"];
/// Exact native Responses summary replay contract, independent of model branding.
pub(crate) fn kimi_native_summary(ctx: &Context<'_>) -> bool {
    !ctx.is_oauth && ctx.protocol == Protocol::Responses
        && super::parameter_contracts::provider(ctx) == "moonshot"
        && KIMI_K3.contains(&ctx.model)
}
const KIMI_CODE: &[&str] = &["kimi-k2.7-code", "kimi-k2.7-code-highspeed"];
// Exact IDs in the public Chat schema, including the Flash schema branch.
const GLM_MODELS: &[&str] = &["glm-5.3", "glm-5.3-flash", "glm-5.3-flashx", "glm-5.2", "glm-5.1", "glm-5-turbo", "glm-5", "glm-4.7", "glm-4.6"];
const DEEPSEEK_MODELS: &[&str] = &["deepseek-flash", "deepseek-v4-pro"];
fn glm53(model: &str) -> bool { ["glm-5.3", "glm-5.3-flash", "glm-5.3-flashx"].contains(&model) }
fn glm_coding_plan(ctx: &Context<'_>) -> bool {
    let Ok(url) = url::Url::parse(ctx.base_url) else { return false; };
    url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
        && url.port().is_none() && url.query().is_none() && url.fragment().is_none()
        && matches!(url.host_str(), Some("open.bigmodel.cn" | "api.z.ai"))
        && url.path().trim_end_matches('/') == "/api/coding/paas/v4"
}
const ARK_SEED: &[&str] = &[
    "doubao-seed-evolving", "doubao-seed-2-1-pro-260915", "doubao-seed-2-1-lite-260915",
    "doubao-seed-2-1-pro-260628", "doubao-seed-2-1-turbo-260628",
    "doubao-seed-2-0-lite-260428", "doubao-seed-2-0-mini-260428", "doubao-seed-character-260628",
];
const SF_MODELS: &[&str] = &[
    "deepseek-ai/DeepSeek-V4-Flash", "Pro/zai-org/GLM-5.2", "moonshotai/Kimi-K2.7-Code", "Qwen/Qwen3.8-27B",
];
const SF_SWITCH_MODELS: &[&str] = &["Qwen/Qwen3.8-27B"];
const MIMO_MODELS: &[&str] = &["mimo-v2.6-flash", "mimo-v2.6-pro", "mimo-v2.6-pro-ultraspeed"];

fn present<'a>(body: &'a Value, path: &str) -> Option<&'a Value> {
    body.pointer(path).filter(|v| !v.is_null())
}
fn invalid(path: &str, message: &str) -> String {
    format!("Parameter {path}: {message}")
}
fn number(body: &Value, path: &str, min: f64, max: f64) -> Result<(), String> {
    if let Some(value) = present(body, path) {
        let value = value.as_f64().ok_or_else(|| invalid(path, "must be a number"))?;
        if !value.is_finite() || value < min || value > max {
            return Err(invalid(path, &format!("must be in [{min}, {max}]")));
        }
    }
    Ok(())
}
fn integer(body: &Value, path: &str, min: i64, max: i64) -> Result<(), String> {
    if let Some(value) = present(body, path) {
        let value = value.as_i64().ok_or_else(|| invalid(path, "must be an integer"))?;
        if value < min || value > max { return Err(invalid(path, &format!("must be in [{min}, {max}]"))); }
    }
    Ok(())
}
fn boolean(body: &Value, path: &str) -> Result<(), String> {
    if present(body, path).is_some_and(|v| !v.is_boolean()) { return Err(invalid(path, "must be boolean")); }
    Ok(())
}
fn object(body: &Value, path: &str) -> Result<(), String> {
    if present(body, path).is_some_and(|v| !v.is_object()) { return Err(invalid(path, "must be an object")); }
    Ok(())
}
fn one_of(body: &Value, path: &str, values: &[&str]) -> Result<(), String> {
    if let Some(value) = present(body, path) {
        if value.as_str().is_none_or(|value| !values.contains(&value)) {
            return Err(invalid(path, &format!("must be one of {}", values.join(", "))));
        }
    }
    Ok(())
}
fn fixed(body: &Value, path: &str, expected: f64) -> Result<(), String> {
    if let Some(value) = present(body, path) {
        if value.as_f64() != Some(expected) {
            return Err(invalid(path, &format!("is fixed at {expected} for this exact model")));
        }
    }
    Ok(())
}
fn effort_path(protocol: Protocol) -> &'static str {
    match protocol { Protocol::Chat => "/reasoning_effort", Protocol::Responses => "/reasoning/effort", Protocol::Messages => "/output_config/effort" }
}
fn effort<'a>(ctx: &Context<'_>, body: &'a Value) -> Option<&'a str> {
    body.pointer(effort_path(ctx.protocol)).and_then(Value::as_str)
}
// Keep off adaptation independent of registry membership: adding a hosted
// model must not grant it Qwen's enable_thinking alias or shrink its budget.
fn dashscope_off_adapter(model: &str) -> bool {
    QWEN_HYBRID.contains(&model)
        || (QWEN_TOTAL_CAP.contains(&model) && model != "glm-5.3")
        || model == "glm-5.2"
}
fn disabled(ctx: &Context<'_>, body: &Value) -> bool {
    let toggle = body.get("enable_thinking") == Some(&json!(false))
        || body.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled");
    let off_effort = effort(ctx, body) == Some("none");
    match provider(ctx) {
        "siliconflow" => SF_SWITCH_MODELS.contains(&ctx.model) && (toggle || off_effort),
        "dashscope" => dashscope_off_adapter(ctx.model) && (toggle || off_effort),
        "moonshot" => ctx.model == "kimi-k2.6" && (toggle || off_effort),
        "minimax" => ctx.model == "MiniMax-M3" && (toggle || off_effort),
        "ark" => ARK_SEED.contains(&ctx.model) && (toggle || off_effort || effort(ctx, body) == Some("minimal")),
        "zai" => GLM_MODELS.contains(&ctx.model) && !glm53(ctx.model)
            && (toggle || off_effort || (ctx.model == "glm-5.2" && effort(ctx, body) == Some("minimal"))),
        _ => false,
    }
}
fn known(ctx: &Context<'_>) -> bool {
    match provider(ctx) {
        "dashscope" => (ctx.protocol == Protocol::Responses && known_dashscope_responses(ctx.model)) || QWEN_HYBRID.contains(&ctx.model) || QWEN_TOTAL_CAP.contains(&ctx.model) || ["glm-5.2", "kimi-k3", "kimi/kimi-k3.1-pro", "kimi/kimi-k3.1-pro-highspeed", "MiniMax/MiniMax-M3.1-Flash", "deepseek-v4.1-flash-1017"].contains(&ctx.model),
        "deepseek" => DEEPSEEK_MODELS.contains(&ctx.model),
        "moonshot" => KIMI_K3.contains(&ctx.model) || KIMI_CODE.contains(&ctx.model) || ctx.model == "kimi-k2.6",
        "zai" => GLM_MODELS.contains(&ctx.model),
        "minimax" => ["MiniMax-M3", "MiniMax-M3.1-Flash"].contains(&ctx.model),
        "mimo" => MIMO_MODELS.contains(&ctx.model),
        "ark" => ARK_SEED.contains(&ctx.model),
        "siliconflow" => SF_MODELS.contains(&ctx.model),
        "ollama" | "vllm" => true, // Explicit profile; model capabilities remain runtime-defined.
        _ => false,
    }
}

/// Checks only documented controls for a known source/model pair. It does not
/// remove unknown native parameters, normalize defaults or clean thinking history.
pub fn validate(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    if !known(ctx) { return Ok(()); }
    match provider(ctx) {
        "deepseek" => validate_deepseek(ctx, body),
        "mimo" => validate_mimo(ctx, body),
        "moonshot" if ctx.protocol == Protocol::Responses && KIMI_K3.contains(&ctx.model) => validate_kimi_responses(ctx, body),
        "moonshot" if ctx.protocol == Protocol::Chat => validate_kimi(ctx, body),
        "zai" if ctx.protocol == Protocol::Chat => validate_glm(ctx, body),
        "minimax" if matches!(ctx.protocol, Protocol::Chat | Protocol::Messages) => validate_minimax(ctx, body),
        "dashscope" if ctx.protocol == Protocol::Responses && known_dashscope_responses(ctx.model) => {
            integer(body, "/max_output_tokens", 16, i64::MAX)?;
            boolean(body, "/enable_thinking")?;
            Ok(())
        }
        "dashscope" if matches!(ctx.protocol, Protocol::Chat | Protocol::Messages) => validate_dashscope(ctx, body),
        "ark" => validate_ark(ctx, body),
        "siliconflow" if ctx.protocol == Protocol::Chat => validate_siliconflow(ctx, body),
        "ollama" => validate_ollama(ctx, body),
        "vllm" => validate_vllm(body),
        _ => Ok(()),
    }
}
// CN-DS-02: native Responses silently ignores unsupported intent; reject it
// before transport. These checks do not change the configured Chat conversion.
fn validate_deepseek(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    if ctx.protocol == Protocol::Chat {
        if let Some(tools)=body.get("tools").and_then(Value::as_array) {
            if tools.iter().any(|tool|tool.pointer("/function/strict")==Some(&json!(true))) {
                let beta=url::Url::parse(ctx.base_url).is_ok_and(|url|url.scheme()=="https"
                    && url.host_str()==Some("api.deepseek.com") && url.path().trim_end_matches('/')=="/beta"
                    && url.port().is_none() && url.username().is_empty() && url.password().is_none()
                    && url.query().is_none() && url.fragment().is_none());
                if !beta {
                    return Err(invalid("tools.function.strict", "strict mode requires the documented official DeepSeek Beta source; configure that source or a native Responses route in TokenGo, never downgrade strict"));
                }
                if tools.iter().any(|tool|tool["type"]!="function" || tool.pointer("/function/strict")!=Some(&json!(true))) {
                    return Err(invalid("tools.function.strict", "DeepSeek Beta requires every function to be strict; mixed strict semantics are not rewritten"));
                }
            }
        }
        object(body, "/thinking")?;
        one_of(body, "/thinking/type", &["enabled", "disabled"])?;
        if body.pointer("/thinking/type").and_then(Value::as_str) == Some("enabled") && effort(ctx, body) == Some("none") {
            return Err(invalid("thinking", "enabled conflicts with explicit reasoning_effort=none"));
        }
        let off = body.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled")
            || effort(ctx, body) == Some("none");
        if !off && present(body, "/tool_choice").is_some_and(|v| v == "required" || v.is_object()) {
            return Err(invalid("tool_choice", "DeepSeek required/named tools require explicitly disabled thinking"));
        }
        one_of(body, "/response_format/type", &["text", "json_object"])?;
        return Ok(());
    }
    if ctx.protocol != Protocol::Responses { return Ok(()); }
    for path in ["/previous_response_id", "/conversation", "/metadata", "/include", "/prompt", "/truncation", "/service_tier", "/safety_identifier", "/prompt_cache_key", "/prompt_cache_retention", "/context_management", "/stream_options", "/max_tool_calls", "/text/verbosity"] {
        if present(body, path).is_some() { return Err(invalid(path, "not supported by native DeepSeek Responses; upstream would ignore this intent")); }
    }
    for path in ["/store", "/background"] {
        boolean(body, path)?;
        if present(body, path) == Some(&json!(true)) { return Err(invalid(path, "native DeepSeek Responses is stateless and synchronous")); }
    }
    boolean(body, "/parallel_tool_calls")?;
    if body.get("parallel_tool_calls") == Some(&json!(false)) { return Err(invalid("parallel_tool_calls", "DeepSeek always enables parallel tool calling")); }
    if present(body, "/reasoning/summary").is_some_and(|v| v != "none") { return Err(invalid("reasoning.summary", "DeepSeek generates raw reasoning, not a summary")); }
    if let Some(items) = present(body, "/input").and_then(Value::as_array) {
        for item in items {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("message");
            if !["message", "function_call", "function_call_output", "reasoning", "custom_tool_call", "custom_tool_call_output", "web_search_call"].contains(&kind) {
                return Err(invalid("input", "native DeepSeek Responses ignores this input item type"));
            }
            if kind == "message" && item.get("role").is_some_and(|v| v == "developer") {
                return Err(invalid("input.role", "native DeepSeek Responses treats developer as user and cannot preserve instruction priority"));
            }
            if kind == "message" {
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    for part in parts {
                        if !["input_text", "output_text", "input_image"].contains(&part["type"].as_str().unwrap_or("")) {
                            return Err(invalid("input.content", "native DeepSeek Responses does not support this content type, including file inputs"));
                        }
                        if part["type"] == "input_image" && !matches!(item["role"].as_str(), Some("user")) {
                            return Err(invalid("input.content", "native DeepSeek Responses images require a user message"));
                        }
                    }
                }
            }
            if kind == "reasoning" && (present(item, "/summary").is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty)) || present(item, "/encrypted_content").is_some()) {
                return Err(invalid("input.reasoning", "DeepSeek accepts raw reasoning content, not summary or encrypted content"));
            }
        }
    }
    if let Some(tools) = present(body, "/tools").and_then(Value::as_array) {
        for tool in tools {
            let supported = tool["type"] == "function" || (tool["type"] == "custom" && tool["name"] == "apply_patch") || (tool["type"] == "web_search" && ctx.model == "deepseek-v4-pro");
            if !supported { return Err(invalid("tools", "this tool type/model combination is ignored or unsupported by native DeepSeek Responses")); }
        }
    }
    Ok(())
}
// CN-KM-02: K3 native Responses has its own effort/tool schema. Summary events
// returned by Kimi remain native; no summary is synthesized into raw reasoning.
fn validate_kimi_responses(_ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    one_of(body, "/tool_choice", &["auto"])?;
    object(body, "/reasoning")?;
    one_of(body, "/reasoning/effort", &["low", "high", "max"])?;
    if present(body, "/reasoning/summary").is_some() { return Err(invalid("reasoning.summary", "not a documented Kimi native Responses request control")); }
    for path in ["/previous_response_id", "/conversation", "/context_management"] {
        if present(body, path).is_some() { return Err(invalid(path, "Kimi native Responses is stateless")); }
    }
    for path in ["/store", "/background"] {
        boolean(body, path)?;
        if present(body, path) == Some(&json!(true)) { return Err(invalid(path, "Kimi native Responses is stateless and synchronous")); }
    }
    integer(body, "/max_output_tokens", 1, 1048576)?;
    one_of(body, "/text/format/type", &["json_schema"])?;
    Ok(())
}
fn validate_mimo(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    number(body, "/temperature", 0.0, 1.5)?;
    number(body, "/top_p", 0.01, 1.0)?;
    boolean(body, "/stream")?;
    let cap = match ctx.protocol {
        Protocol::Chat => "/max_completion_tokens",
        Protocol::Responses => "/max_output_tokens",
        Protocol::Messages => "/max_tokens",
    };
    integer(body, cap, 1, 131072)?;
    if ctx.protocol == Protocol::Responses {
        object(body, "/reasoning")?;
        one_of(body, "/reasoning/effort", &["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"])?;
        for path in ["/background", "/previous_response_id", "/context_management"] {
            if present(body, path).is_some() { return Err(invalid(path, "MiMo Responses does not support this stateful/background control")); }
        }
        one_of(body, "/text/format/type", &["text", "json_object"])?;
    } else {
        object(body, "/thinking")?;
        one_of(body, "/thinking/type", &["enabled", "disabled"])?;
        if present(body, effort_path(ctx.protocol)).is_some() {
            return Err(invalid(effort_path(ctx.protocol), "this MiMo protocol uses thinking.type; only explicit none has a lossless adapter"));
        }
    }
    let choice = if ctx.protocol == Protocol::Messages { "/tool_choice/type" } else { "/tool_choice" };
    if present(body, choice).is_some_and(|v| v.as_str() != Some("auto")) {
        return Err(invalid(choice, "MiMo silently converts non-auto choices to auto; the gateway rejects this loss of tool-selection intent"));
    }
    if ctx.protocol == Protocol::Messages {
        boolean(body, "/tool_choice/disable_parallel_tool_use")?;
    }
    if ctx.protocol == Protocol::Chat {
        number(body, "/frequency_penalty", -2.0, 2.0)?;
        number(body, "/presence_penalty", -2.0, 2.0)?;
        one_of(body, "/response_format/type", &["text", "json_object"])?;
        if let Some(stop) = present(body, "/stop") {
            if !stop.is_string() && !stop.as_array().is_some_and(|a| a.len() <= 4 && a.iter().all(Value::is_string)) {
                return Err(invalid("stop", "must be a string or at most four strings"));
            }
        }
    }
    Ok(())
}
fn validate_kimi(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    let k3 = KIMI_K3.contains(&ctx.model);
    let code = KIMI_CODE.contains(&ctx.model);
    if k3 {
        one_of(body, "/reasoning_effort", &["low", "high", "max"])?;
        if present(body, "/thinking").is_some() { return Err(invalid("thinking", "K3 uses reasoning_effort, not the K2 thinking control")); }
    } else {
        if present(body, "/reasoning_effort").is_some() { return Err(invalid("reasoning_effort", "not a native parameter for this K2 model; use thinking")); }
        object(body, "/thinking")?;
        one_of(body, "/thinking/type", if code { &["enabled"] } else { &["enabled", "disabled"] })?;
        one_of(body, "/thinking/keep", &["all"])?;
        if code && present(body, "/thinking").is_some()
            && (body.pointer("/thinking/type").and_then(Value::as_str) != Some("enabled")
                || body.pointer("/thinking/keep").and_then(Value::as_str) != Some("all")) {
            return Err(invalid("thinking", "K2.7 Code accepts only enabled with keep=all"));
        }
    }
    fixed(body, "/temperature", if !k3 && !code && disabled(ctx, body) { 0.6 } else { 1.0 })?;
    fixed(body, "/top_p", 0.95)?;
    fixed(body, "/n", 1.0)?;
    fixed(body, "/presence_penalty", 0.0)?;
    fixed(body, "/frequency_penalty", 0.0)?;
    if !k3 && body.get("tool_choice").and_then(Value::as_str) == Some("required") {
        return Err(invalid("tool_choice", "required is not supported by this exact K2 model"));
    }
    integer(body, "/top_logprobs", 0, 20)?;
    if present(body, "/top_logprobs").is_some() && body.get("logprobs") != Some(&json!(true)) {
        return Err(invalid("top_logprobs", "requires logprobs=true"));
    }
    if let Some(stop) = present(body, "/stop") {
        if let Some(items) = stop.as_array() {
            if items.len() > 5 || items.iter().any(|v| v.as_str().is_none_or(|s| s.len() > 32)) {
                return Err(invalid("stop", "accepts at most five strings of at most 32 bytes"));
            }
        } else if stop.as_str().is_none_or(|s| s.len() > 32) {
            return Err(invalid("stop", "must be a string or array of strings"));
        }
    }
    object(body, "/prompt_cache_options")?;
    one_of(body, "/prompt_cache_options/mode", &["implicit"])?;
    one_of(body, "/prompt_cache_options/ttl", &["5m", "1h"])?;
    Ok(())
}
fn validate_glm(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    object(body, "/thinking")?;
    one_of(body, "/thinking/type", if matches!(ctx.model, "glm-5.3" | "glm-5.3-flash") { &["enabled"] } else { &["enabled", "disabled"] })?;
    boolean(body, "/thinking/clear_thinking")?;
    boolean(body, "/do_sample")?;
    boolean(body, "/tool_stream")?;
    number(body, "/temperature", 0.0, 1.0)?;
    number(body, "/top_p", 0.01, 1.0)?;
    integer(body, "/max_tokens", 1, 131072)?;
    one_of(body, "/tool_choice", &["auto"])?;
    object(body, "/response_format")?;
    one_of(body, "/response_format/type", &["text", "json_object"])?;
    if body.get("tool_stream") == Some(&json!(true)) && !["glm-5.3", "glm-5.2", "glm-5.1", "glm-5", "glm-5-turbo", "glm-4.7", "glm-4.6"].contains(&ctx.model) {
        return Err(invalid("tool_stream", "not documented for this exact GLM model"));
    }
    if body.get("tool_stream") == Some(&json!(true)) && body.get("stream") != Some(&json!(true)) {
        return Err(invalid("tool_stream", "requires stream=true"));
    }
    if glm53(ctx.model) {
        one_of(body, "/reasoning_effort", if glm_coding_plan(ctx) { &["none", "minimal", "low", "medium", "high", "xhigh", "max"] } else { &["low", "high", "max"] })?;
    } else if ctx.model == "glm-5.2" {
        one_of(body, "/reasoning_effort", &["none", "minimal", "low", "medium", "high", "xhigh", "max"])?;
    }
    if let Some(items) = present(body, "/stop") {
        if items.as_array().is_none_or(|items| items.len() > 4 || items.iter().any(|v| !v.is_string())) {
            return Err(invalid("stop", "must be an array of at most four strings"));
        }
    }
    Ok(())
}
fn validate_minimax(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    object(body, "/thinking")?;
    one_of(body, "/thinking/type", &["adaptive", "disabled"])?;
    let flash = ctx.model == "MiniMax-M3.1-Flash";
    if flash {
        if body.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled") {
            return Err(invalid("thinking.type", "MiniMax-M3.1-Flash requires thinking; it cannot be disabled"));
        }
        one_of(body, effort_path(ctx.protocol), &["low", "medium", "high", "xhigh", "max"])?;
        if ctx.protocol == Protocol::Chat && body.get("reasoning_split") == Some(&json!(false)) {
            return Err(invalid("reasoning_split", "false is not supported by MiniMax-M3.1-Flash"));
        }
        if present(body, "/service_tier").is_some() {
            return Err(invalid("service_tier", "not supported by MiniMax-M3.1-Flash"));
        }
    } else {
        one_of(body, "/service_tier", &["standard", "priority"])?;
    }
    // Temperature is accepted but ignored; preserving it is more honest than
    // rejecting it as if it were a fixed-value schema. top_p still has a range.
    if let Some(value) = present(body, "/top_p") {
        if value.as_f64().is_none_or(|n| n <= 0.0 || n > 1.0) { return Err(invalid("top_p", "must be in (0, 1]")); }
    }
    Ok(())
}
fn validate_dashscope(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    // CN-QW-03: Qwen required is not guaranteed even with thinking disabled.
    if ctx.protocol == Protocol::Chat {
        boolean(body, "/parallel_tool_calls")?;
        if ["kimi-k3", "kimi/kimi-k3.1-pro", "kimi/kimi-k3.1-pro-highspeed", "MiniMax/MiniMax-M3.1-Flash", "deepseek-v4.1-flash-1017"].contains(&ctx.model) && effort(ctx, body) == Some("none") {
            return Err(invalid("reasoning_effort", "this exact deployed model has no documented none alias; the gateway does not invent a thinking-off mapping"));
        }
        if ["kimi/kimi-k3.1-pro", "kimi/kimi-k3.1-pro-highspeed", "MiniMax/MiniMax-M3.1-Flash"].contains(&ctx.model)
            && (body.get("enable_thinking") == Some(&json!(false)) || body.pointer("/thinking/type").is_some_and(|v| v == "disabled")) {
            return Err(invalid("thinking", "this exact hosted model does not declare a thinking-off control"));
        }
        if QWEN_HYBRID.contains(&ctx.model) {
            if body.get("tool_choice").is_some_and(|v| v == "required") {
                return Err(invalid("tool_choice", "this exact Qwen model does not guarantee required tool calls"));
            }
            let explicit_off = effort(ctx, body) == Some("none")
                || (effort(ctx, body).is_none() && body.get("enable_thinking") == Some(&json!(false)));
            if body.get("tool_choice").is_some_and(Value::is_object) && !explicit_off {
                return Err(invalid("tool_choice", "named tools require explicitly disabled thinking for this hybrid model"));
            }
        }
        if ["kimi-k3", "MiniMax/MiniMax-M3.1-Flash"].contains(&ctx.model) && body.get("parallel_tool_calls") == Some(&json!(false)) {
            return Err(invalid("parallel_tool_calls", "this deployed model cannot guarantee sequential tools"));
        }
        if ["kimi/kimi-k3.1-pro", "kimi/kimi-k3.1-pro-highspeed"].contains(&ctx.model) && present(body, "/parallel_tool_calls").is_some() {
            return Err(invalid("parallel_tool_calls", "not supported by this deployed model"));
        }
        if ctx.model == "MiniMax/MiniMax-M3.1-Flash" { one_of(body, "/tool_choice", &["auto"])?; }
    }
    boolean(body, "/enable_thinking")?;
    object(body, "/thinking")?;
    boolean(body, "/tool_stream")?;
    if QWEN_EFFORT_BUDGET.contains(&ctx.model) || ctx.model == "qwen3.8-omni-flash" {
        integer(body, if ctx.protocol == Protocol::Messages { "/thinking/budget_tokens" } else { "/thinking_budget" }, 0,
            if QWEN_EFFORT_BUDGET.contains(&ctx.model) { 262144 } else { i64::MAX })?;
        if ctx.protocol == Protocol::Chat && present(body, "/reasoning_effort").is_some() && present(body, "/thinking_budget").is_some() {
            return Err(invalid("reasoning_effort", "cannot be combined with thinking_budget for this Qwen3.8 model"));
        }
        if ctx.protocol == Protocol::Chat {
            one_of(body, "/reasoning_effort", &["none", "minimal", "low", "medium", "high", "xhigh", "max"])?;
        }
    }
    if ctx.protocol == Protocol::Chat {
        integer(body, "/seed", 0, 2_147_483_647)?;
        integer(body, "/top_logprobs", 0, 5)?;
        if present(body, "/top_logprobs").is_some() && body.get("logprobs") != Some(&json!(true)) {
            return Err(invalid("top_logprobs", "requires logprobs=true"));
        }
        if let Some(stop) = present(body, "/stop").and_then(Value::as_array) {
            let strings = stop.iter().any(Value::is_string);
            let integers = stop.iter().any(Value::is_i64);
            if strings && integers { return Err(invalid("stop", "must not mix strings and token IDs")); }
        }
    }
    if ctx.model == "glm-5.3" {
        if body.get("enable_thinking") == Some(&json!(false)) || body.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled") {
            return Err(invalid("thinking", "glm-5.3 is a thinking-only model"));
        }
        one_of(body, effort_path(ctx.protocol), &["low", "high", "max"])?;
    }
    // Both output-cap fields may represent different scopes: never compare them
    // as aliases or rewrite one into the other in this provider contract.
    Ok(())
}
fn validate_ark(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    object(body, "/thinking")?;
    one_of(body, "/thinking/type", &["enabled", "disabled"])?;
    one_of(body, effort_path(ctx.protocol), &["none", "minimal", "low", "medium", "high", "xhigh", "max"])?;
    if body.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled")
        && effort(ctx, body).is_some_and(|v| !["none", "minimal"].contains(&v)) {
        return Err(invalid(effort_path(ctx.protocol), "disabled thinking accepts only the documented off effort (minimal/none)"));
    }
    // enabled+minimal is explicitly a provider off configuration, not a conflict.
    Ok(())
}
fn validate_siliconflow(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    boolean(body, "/enable_thinking")?;
    integer(body, "/thinking_budget", 128, 32768)?;
    if ["deepseek-ai/DeepSeek-V4-Flash", "Pro/zai-org/GLM-5.2"].contains(&ctx.model) {
        one_of(body, "/reasoning_effort", &["low", "medium", "high", "xhigh", "max"])?;
    }
    number(body, "/temperature", 0.0, 2.0)?;
    number(body, "/top_p", 0.0, 1.0)?;
    number(body, "/frequency_penalty", -2.0, 2.0)?;
    if let Some(v) = present(body, "/top_k") {
        if v.as_f64().is_none_or(|n| n > 100.0) { return Err(invalid("top_k", "must be numeric and at most 100")); }
    }
    if let Some(stop) = present(body, "/stop").and_then(Value::as_array) {
        if stop.len() > 4 || stop.iter().any(|v| !v.is_string()) { return Err(invalid("stop", "accepts at most four strings")); }
    }
    Ok(())
}
fn validate_ollama(ctx: &Context<'_>, body: &Value) -> Result<(), String> {
    if ctx.protocol == Protocol::Responses {
        for path in ["/previous_response_id", "/conversation"] {
            if present(body, path).is_some() { return Err(invalid(path, "Ollama Responses does not support stateful continuation")); }
        }
    }
    if let Some(think) = present(body, "/think") {
        if !think.is_boolean() && !think.is_string() { return Err(invalid("think", "must be boolean, a model-defined string, or null")); }
    }
    Ok(())
}
fn validate_vllm(body: &Value) -> Result<(), String> {
    for path in ["/chat_template_kwargs", "/structured_outputs", "/vllm_xargs", "/kv_transfer_params", "/ec_transfer_params"] { object(body, path)?; }
    if body.get("continue_final_message") == Some(&json!(true)) && body.get("add_generation_prompt") == Some(&json!(true)) {
        return Err(invalid("continue_final_message", "cannot be combined with add_generation_prompt=true"));
    }
    if let Some(salt) = present(body, "/cache_salt") {
        if salt.as_str().is_none_or(|s| s.is_empty() || s.len() > 1024) { return Err(invalid("cache_salt", "must be a nonempty string of at most 1024 bytes")); }
    }
    Ok(())
}

fn budget(body: &Value, protocol: Protocol) -> Option<i64> {
    let path = if protocol == Protocol::Messages { "/thinking/budget_tokens" } else { "/thinking_budget" };
    body.pointer(path).and_then(Value::as_i64).filter(|v| *v > 0)
}
/// An inclusive Chat field is a versioned provider/model capability, not a
/// blanket DashScope convention. OAuth uses its own authenticated transport.
pub fn has_inclusive_chat_cap(ctx: &Context<'_>) -> bool {
    !ctx.is_oauth && ctx.protocol == Protocol::Chat
        && ((provider(ctx) == "dashscope" && QWEN_INCLUSIVE_CAP.contains(&ctx.model))
            || (provider(ctx) == "mimo" && MIMO_MODELS.contains(&ctx.model)))
}
/// No provider default is invented. Answer-only budgets without an explicit,
/// verified reasoning ceiling require the caller's conservative total reservation.
pub fn output_semantics(ctx: &Context<'_>, body: &Value) -> OutputSemantics {
    if !ctx.is_oauth && provider(ctx) == "dashscope" && ctx.protocol == Protocol::Responses {
        if QWEN_RESPONSES_TOTAL_CAP.contains(&ctx.model) { return OutputSemantics::Total; }
        if QWEN_RESPONSES_ANSWER_CAP.contains(&ctx.model) {
            return if dashscope_responses_thinking_disabled(ctx, body) { OutputSemantics::Total }
                else { OutputSemantics::AnswerOnly { reasoning_budget: None } };
        }
        return OutputSemantics::Unknown;
    }
    if disabled(ctx, body) || !matches!(ctx.protocol, Protocol::Chat | Protocol::Messages) {
        return OutputSemantics::Total;
    }
    match provider(ctx) {
        "siliconflow" => OutputSemantics::AnswerOnly {
            reasoning_budget: if SF_SWITCH_MODELS.contains(&ctx.model) { budget(body, ctx.protocol) } else { None },
        },
        "dashscope" if ctx.protocol == Protocol::Chat && QWEN_INCLUSIVE_CAP.contains(&ctx.model)
            && present(body, "/max_completion_tokens").is_some() => OutputSemantics::Total,
        "dashscope" if ctx.model == "glm-5.2" => {
            if budget(body, ctx.protocol).is_some() { OutputSemantics::AnswerOnly { reasoning_budget: budget(body, ctx.protocol) } }
            else { OutputSemantics::Total }
        }
        "dashscope" if QWEN_TOTAL_CAP.contains(&ctx.model) => OutputSemantics::Total,
        "dashscope" if ctx.model == "qwen3-vl-plus" && present(body, "/enable_thinking").is_none() => OutputSemantics::Total,
        "dashscope" => {
            let explicit = (if QWEN_HYBRID.contains(&ctx.model) { budget(body, ctx.protocol) } else { None }).or_else(|| {
                if ctx.protocol != Protocol::Chat || !QWEN_EFFORT_BUDGET.contains(&ctx.model) { return None; }
                match effort(ctx, body) {
                    Some("minimal" | "low") => Some(4096),
                    Some("medium") => Some(16384),
                    Some("high" | "xhigh" | "max") => Some(262144),
                    _ => None,
                }
            });
            OutputSemantics::AnswerOnly { reasoning_budget: explicit }
        },
        _ => OutputSemantics::Total,
    }
}

fn remove_effort(protocol: Protocol, body: &mut Value) {
    let key = match protocol { Protocol::Chat => "reasoning_effort", Protocol::Responses => "reasoning", Protocol::Messages => "output_config" };
    if protocol == Protocol::Chat { body.as_object_mut().unwrap().remove(key); }
    else if let Some(object) = body.get_mut(key).and_then(Value::as_object_mut) {
        object.remove("effort");
        if object.is_empty() { body.as_object_mut().unwrap().remove(key); }
    }
}
fn thinking_off(body: &mut Value) -> Result<(), String> {
    if let Some(old) = body.get("thinking") {
        if old != &json!({"type":"disabled"}) { return Err(invalid("thinking", "explicit none conflicts with existing thinking controls; no fields were discarded")); }
    }
    body["thinking"] = json!({"type":"disabled"});
    Ok(())
}
/// Applies only lossless, explicit off controls with a documented source contract.
/// Unknown fields and history are never scrubbed. On error the body is unchanged.
pub fn adapt(ctx: &Context<'_>, body: &mut Value) -> Result<bool, String> {
    if !known(ctx) || effort(ctx, body) != Some("none") { return Ok(false); }
    if provider(ctx) == "dashscope" && ctx.protocol == Protocol::Chat
        && (QWEN_EFFORT_BUDGET.contains(&ctx.model) || ctx.model == "qwen3.8-omni-flash")
        && present(body, "/reasoning_effort").is_some() && present(body, "/thinking_budget").is_some()
    {
        return Err(invalid("reasoning_effort", "cannot be combined with thinking_budget; none adaptation does not erase this conflict"));
    }
    let mut changed = body.clone();
    match provider(ctx) {
        "dashscope" if dashscope_off_adapter(ctx.model) && matches!(ctx.protocol, Protocol::Chat | Protocol::Messages) => {
            if ctx.protocol == Protocol::Chat {
                if present(&changed, "/enable_thinking").is_some_and(|v| v != &json!(false)) {
                    return Err(invalid("enable_thinking", "explicit none conflicts with enabled thinking"));
                }
                changed["enable_thinking"] = json!(false);
            } else { thinking_off(&mut changed)?; }
            remove_effort(ctx.protocol, &mut changed);
        }
        "siliconflow" if ctx.protocol == Protocol::Chat && SF_SWITCH_MODELS.contains(&ctx.model) => {
            if present(&changed, "/enable_thinking").is_some_and(|v| v != &json!(false)) {
                return Err(invalid("enable_thinking", "explicit none conflicts with enabled thinking"));
            }
            changed["enable_thinking"] = json!(false);
            remove_effort(ctx.protocol, &mut changed);
        }
        "mimo" if matches!(ctx.protocol, Protocol::Chat | Protocol::Messages) => {
            thinking_off(&mut changed)?;
            remove_effort(ctx.protocol, &mut changed);
        }
        "moonshot" if ctx.protocol == Protocol::Chat && ctx.model == "kimi-k2.6" => {
            thinking_off(&mut changed)?;
            remove_effort(ctx.protocol, &mut changed);
        }
        "minimax" if ctx.model == "MiniMax-M3" && matches!(ctx.protocol, Protocol::Chat | Protocol::Messages) => {
            thinking_off(&mut changed)?;
            remove_effort(ctx.protocol, &mut changed);
        }
        "ark" => {
            // This exact Seed contract defines minimal as off (not low effort).
            match ctx.protocol {
                Protocol::Chat => changed["reasoning_effort"] = json!("minimal"),
                Protocol::Responses => changed["reasoning"]["effort"] = json!("minimal"),
                Protocol::Messages => changed["output_config"]["effort"] = json!("minimal"),
            }
        }
        _ => return Ok(false),
    }
    validate(ctx, &changed)?;
    *body = changed;
    Ok(true)
}

pub fn describe(ctx: &Context<'_>) -> Value {
    let p = provider(ctx);
    let runtime_profile = matches!(p, "ollama" | "vllm");
    let specific = known(ctx) && !runtime_profile;
    let urls: &[&str] = match p {
        "mimo" => &["https://mimo.mi.com/docs/en-US/api/chat/openai-api", "https://mimo.mi.com/docs/en-US/api/chat/responses", "https://mimo.mi.com/docs/en-US/api/chat/anthropic-api"],
        "dashscope" => &["https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions", "https://help.aliyun.com/en/model-studio/anthropic-api-messages", "https://help.aliyun.com/en/model-studio/qwen-api-via-openai-responses"],
        "moonshot" => &["https://platform.kimi.ai/docs/models", "https://platform.kimi.ai/docs/api/models-overview", "https://platform.kimi.ai/docs/api/chat"],
        "zai" => &["https://docs.bigmodel.cn/api-reference/模型-api/对话补全", "https://docs.z.ai/api-reference/llm/chat-completion"],
        "minimax" => &["https://platform.minimax.io/docs/api-reference/text-openai-api", "https://platform.minimax.io/docs/api-reference/text-anthropic-api"],
        "ark" => &["https://docs.volcengine.com/docs/ark/model-deprecation-migration-guide?lang=en", "https://docs.volcengine.com/docs/ark/context-management?lang=en"],
        "siliconflow" => &["https://docs.siliconflow.cn/docs/api/chat-completions-post", "https://docs.siliconflow.cn/docs/api/messages-post", "https://docs.siliconflow.cn/docs/usercases/use-siliconcloud-in-yan"],
        "ollama" => &["https://docs.ollama.com/api/openai-compatibility"],
        "vllm" => &["https://docs.vllm.ai/en/stable/serving/online_serving/openai_compatible_server/"],
        _ => &[],
    };
    let notes = match p {
        "mimo" => "V2.6 defaults to thinking enabled. Responses none disables it; every other valid effort enables the same reasoning behavior. Chat and Messages use thinking.type. Thinking overrides sampling to temperature=1/top_p=.95; retained values need not be effective. Non-auto tool choice and JSON Schema output are rejected rather than promising unsupported behavior. Preserve full reasoning/tool history, multimodal usage and cache breakdown. Chat/Responses output caps include reasoning; Messages uses its documented generation maximum.",
        "dashscope" => "Exact model and supplying vendor matter. max_completion_tokens includes thinking; max_tokens may count answer only and is not a universal alias. No automatic budget default is inserted.",
        "moonshot" => "K3 always thinks; K2.7 Code only supports preserved thinking; K2.6 has an off mode. Fixed sampling values are errors when changed. Preserve full assistant/reasoning/tool history and cache read/write breakdown.",
        "zai" => "GLM-5.3 cannot disable thinking. clear_thinking controls historical reasoning only, not current thinking. do_sample=false is not a temperature=0 rewrite. Native MCP/retrieval and usage stay intact.",
        "minimax" => "M3.1-Flash cannot turn thinking off. M3 Chat defaults to thinking on, but M3 Messages defaults off: omission is not cross-protocol equivalence. Temperature and several Messages controls may be ignored, not effective. Preserve whole thinking/tool history.",
        "ark" => "For the listed Seed versions minimal means off, unlike OpenAI minimal. Opaque thinking and stateful continuation are not plaintext equivalents; gateway ownership rules remain separate.",
        "siliconflow" => "Platform contract differs from original-model vendor. max_tokens excludes chain of thought; thinking_budget is separate. Missing budget remains unknown and requires a conservative verified total ceiling.",
        "ollama" => "Thinking levels/defaults are runtime model metadata from /api/show. Only a subset of OpenAI is supported; stateless Responses only. Do not infer deployment identity from a model name or localhost port.",
        "vllm" => "Server version, generation_config, chat template and parsers determine capabilities/defaults. Preserve native extensions/cache_salt; cache salt is not authorization identity. Profile must be explicit for self-hosted deployments.",
        _ => "No Chinese-provider or local-engine contract selected; native fields are retained for the source to decide.",
    };
    json!({"provider":p,"model":ctx.model,"known_model":specific,"runtime_profile":runtime_profile,"model_capabilities":if runtime_profile{"runtime_metadata_required"}else if specific{"documented_exact_model"}else{"unknown"},"evidence":"official_documentation","observed_date":"2026-10-25","native_unknown_fields":"preserved","automatic_drop_params":false,"budget_scope_confidence":if runtime_profile{"runtime_model_unknown"}else if p=="dashscope"||p=="siliconflow"{"documented_provider_contract_model_dependent"}else if specific{"documented_exact_model_or_runtime_profile"}else{"unknown_legacy_default"},"notes":notes,"references":urls,"live_inference_verified":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ctx<'a>(profile: &'a str, model: &'a str, protocol: Protocol) -> Context<'a> {
        Context { base_url: "https://proxy.example.test/v1", model, protocol, api_version: None, is_oauth: false, profile_override: Some(profile) }
    }
    #[test]
    fn p1_deepseek_strict_requires_documented_beta_source_and_uniform_tools() {
        let mut c=ctx("deepseek","deepseek-flash",Protocol::Chat);
        let body=json!({"tools":[{"type":"function","function":{"name":"f","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}}]});
        c.base_url="https://api.deepseek.com/v1";
        assert!(validate(&c,&body).is_err());
        c.base_url="https://api.deepseek.com/beta";
        assert!(validate(&c,&body).is_ok());
        let mut mixed=body.clone();mixed["tools"].as_array_mut().unwrap().push(json!({"type":"function","function":{"name":"g","strict":false}}));
        assert!(validate(&c,&mixed).is_err());
        c.base_url="https://proxy.invalid/beta";assert!(validate(&c,&body).is_err());
        let nonstrict=json!({"tools":[{"type":"function","function":{"name":"f","strict":false}}]});
        assert!(validate(&c,&nonstrict).is_ok());
    }
    #[test]
    fn unknown_model_and_unknown_native_extensions_are_not_removed_or_rejected() {
        let c = ctx("moonshot", "future-model", Protocol::Chat);
        let b = json!({"thinking":{"type":"future"},"future_vendor_field":{"x":false}});
        assert!(validate(&c,&b).is_ok());
        assert!(!adapt(&c,&mut b.clone()).unwrap());
    }
    #[test]
    fn kimi_k3_fixed_sampling_and_thinking_only_are_not_silently_downgraded() {
        let c = ctx("moonshot", "kimi-k3.1-pro", Protocol::Chat);
        for b in [json!({"reasoning_effort":"none"}),json!({"thinking":{"type":"disabled"}}),json!({"temperature":0.7}),json!({"top_p":1.0}),json!({"n":2})] { assert!(validate(&c,&b).is_err()); }
        assert!(validate(&c,&json!({"reasoning_effort":"max","temperature":1.0,"top_p":0.95,"unknown":{"keep":1}})).is_ok());
    }
    #[test]
    fn kimi_code_preserved_thinking_is_not_an_off_switch() {
        let c = ctx("moonshot", "kimi-k2.7-code", Protocol::Chat);
        assert!(validate(&c,&json!({"thinking":{"type":"enabled","keep":"all"}})).is_ok());
        assert!(validate(&c,&json!({"thinking":{"type":"disabled"}})).is_err());
        assert!(validate(&c,&json!({"tool_choice":"required"})).is_err());
    }
    #[test]
    fn kimi_k26_none_maps_only_to_its_documented_off_control() {
        let c = ctx("moonshot", "kimi-k2.6", Protocol::Chat);
        let mut b = json!({"reasoning_effort":"none","temperature":0.6,"other":7});
        assert!(adapt(&c,&mut b).unwrap());
        assert_eq!(b,json!({"thinking":{"type":"disabled"},"temperature":0.6,"other":7}));
        let mut conflict = json!({"reasoning_effort":"none","thinking":{"type":"enabled","keep":"all"}});
        let before = conflict.clone(); assert!(adapt(&c,&mut conflict).is_err()); assert_eq!(before,conflict);
    }
    #[test]
    fn glm_history_switch_is_preserved_and_53_cannot_turn_thinking_off() {
        let c = ctx("zai", "glm-5.3", Protocol::Chat);
        assert!(validate(&c,&json!({"thinking":{"type":"enabled","clear_thinking":false},"reasoning_effort":"high"})).is_ok());
        assert!(validate(&c,&json!({"thinking":{"type":"disabled"}})).is_err());
        assert!(validate(&c,&json!({"reasoning_effort":"none"})).is_err());
        let c = ctx("zai", "glm-5.2", Protocol::Chat);
        assert!(validate(&c,&json!({"reasoning_effort":"none","thinking":{"clear_thinking":false}})).is_ok());
    }
    #[test]
    fn minimax_models_and_protocol_defaults_are_not_collapsed() {
        for protocol in [Protocol::Chat,Protocol::Messages] {
            let c = ctx("minimax", "MiniMax-M3.1-Flash", protocol);
            assert!(validate(&c,&json!({"thinking":{"type":"disabled"}})).is_err());
            assert!(validate(&c,&json!({"service_tier":"priority"})).is_err());
            let c = ctx("minimax", "MiniMax-M3", protocol);
            assert!(validate(&c,&json!({"temperature":0.2,"service_tier":"priority"})).is_ok());
            let mut b = if protocol==Protocol::Chat {json!({"reasoning_effort":"none"})}else{json!({"output_config":{"effort":"none","format":{"type":"text"}}})};
            assert!(adapt(&c,&mut b).unwrap()); assert_eq!(b["thinking"],json!({"type":"disabled"}));
            if protocol==Protocol::Messages { assert_eq!(b["output_config"]["format"],json!({"type":"text"})); }
        }
    }
    #[test]
    fn dashscope_caps_have_distinct_scopes_and_missing_budget_is_unknown() {
        let c = ctx("dashscope", "qwen3.8-max", Protocol::Chat);
        let b = json!({"max_tokens":32,"max_completion_tokens":128,"thinking_budget":64});
        assert!(validate(&c,&b).is_ok()); assert_eq!(output_semantics(&c,&b),OutputSemantics::Total);
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32,"thinking_budget":64})),OutputSemantics::AnswerOnly{reasoning_budget:Some(64)});
        assert_eq!(output_semantics(&c,&json!({})),OutputSemantics::AnswerOnly{reasoning_budget:None});
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32})),OutputSemantics::AnswerOnly{reasoning_budget:None});
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32,"enable_thinking":false})),OutputSemantics::Total);
    }
    #[test]
    fn qwen_explicit_effort_budget_is_documented_but_omission_is_not_invented() {
        let c=ctx("dashscope","qwen3.8-max",Protocol::Chat);
        assert!(validate(&c,&json!({"reasoning_effort":"low","thinking_budget":4096})).is_err());
        let mut conflict=json!({"reasoning_effort":"none","thinking_budget":4096});
        let before=conflict.clone(); assert!(adapt(&c,&mut conflict).is_err()); assert_eq!(before,conflict);
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32,"reasoning_effort":"low"})),OutputSemantics::AnswerOnly{reasoning_budget:Some(4096)});
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32,"reasoning_effort":"medium"})),OutputSemantics::AnswerOnly{reasoning_budget:Some(16384)});
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32})),OutputSemantics::AnswerOnly{reasoning_budget:None});
    }
    #[test]
    fn siliconflow_unknown_switches_and_undocumented_aliases_do_not_shrink_reservation() {
        let c=ctx("siliconflow","moonshotai/Kimi-K2.7-Code",Protocol::Chat);
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32,"enable_thinking":false})),OutputSemantics::AnswerOnly{reasoning_budget:None});
        let c=ctx("siliconflow","deepseek-ai/DeepSeek-V4-Flash",Protocol::Chat);
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32,"max_completion_tokens":64})),OutputSemantics::AnswerOnly{reasoning_budget:None});
    }
    #[test]
    fn dashscope_glm52_changes_scope_only_with_explicit_budget() {
        let c = ctx("dashscope", "glm-5.2", Protocol::Chat);
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32})),OutputSemantics::Total);
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32,"thinking_budget":64})),OutputSemantics::AnswerOnly{reasoning_budget:Some(64)});
        let c = ctx("dashscope", "glm-5.3", Protocol::Chat);
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32,"thinking_budget":64})),OutputSemantics::Total);
    }
    #[test]
    fn siliconflow_answer_only_limits_are_not_total_caps() {
        let c = ctx("siliconflow", "Qwen/Qwen3.8-27B", Protocol::Chat);
        assert_eq!(output_semantics(&c,&json!({})),OutputSemantics::AnswerOnly{reasoning_budget:None});
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32})),OutputSemantics::AnswerOnly{reasoning_budget:None});
        assert_eq!(output_semantics(&c,&json!({"max_tokens":32,"thinking_budget":128})),OutputSemantics::AnswerOnly{reasoning_budget:Some(128)});
        assert!(validate(&c,&json!({"thinking_budget":127})).is_err());
        let mut b=json!({"reasoning_effort":"none","other":"keep"}); assert!(adapt(&c,&mut b).unwrap());
        assert_eq!(b,json!({"enable_thinking":false,"other":"keep"}));
    }
    #[test]
    fn ark_minimal_is_off_not_low_and_explicit_off_constraints_are_checked() {
        let c = ctx("ark", "doubao-seed-evolving", Protocol::Responses);
        let mut b=json!({"reasoning":{"effort":"none","summary":"auto"},"thinking":{"type":"enabled"}});
        assert!(adapt(&c,&mut b).unwrap()); assert_eq!(b["reasoning"],json!({"effort":"minimal","summary":"auto"}));
        assert!(validate(&c,&json!({"reasoning":{"effort":"high"},"thinking":{"type":"disabled"}})).is_err());
    }
    #[test]
    fn local_serving_capabilities_are_not_inferred_from_model_brand() {
        let c=ctx("ollama","qwen3:8b",Protocol::Responses);
        assert!(validate(&c,&json!({"previous_response_id":"resp_x"})).is_err());
        assert!(validate(&c,&json!({"think":"model-defined","future":true})).is_ok());
        let c=ctx("vllm","custom",Protocol::Chat);
        assert!(validate(&c,&json!({"continue_final_message":true,"add_generation_prompt":true})).is_err());
        assert!(validate(&c,&json!({"cache_salt":"owner-random-salt","chat_template_kwargs":{"enable_thinking":false}})).is_ok());
    }
    #[test]
    fn explicit_nulls_are_never_rewritten_as_defaults() {
        let c=ctx("minimax","MiniMax-M3",Protocol::Messages);
        let b=json!({"temperature":null,"thinking":null,"other":null});
        assert!(validate(&c,&b).is_ok()); assert!(!adapt(&c,&mut b.clone()).unwrap());
    }
    #[test]
    fn p0_dashscope_responses_caps_are_protocol_and_model_specific() {
        let old = ctx("dashscope", "qwen3.7-plus", Protocol::Responses);
        assert_eq!(output_semantics(&old, &json!({"max_output_tokens":32})), OutputSemantics::AnswerOnly { reasoning_budget: None });
        assert_eq!(output_semantics(&old, &json!({"max_output_tokens":32,"thinking_budget":64})), OutputSemantics::AnswerOnly { reasoning_budget: None });
        for model in ["deepseek-v4-pro", "glm-5.3", "kimi-k3"] {
            let c = ctx("dashscope", model, Protocol::Responses);
            assert_eq!(output_semantics(&c, &json!({"max_output_tokens":32})), OutputSemantics::AnswerOnly { reasoning_budget: None });
        }
        let new = ctx("dashscope", "qwen3.8-max", Protocol::Responses);
        assert_eq!(output_semantics(&new, &json!({"max_output_tokens":32})), OutputSemantics::Total);
        assert!(validate(&new, &json!({"max_output_tokens":15})).is_err());
        assert!(validate(&new, &json!({"max_output_tokens":16})).is_ok());
        let hosted = ctx("siliconflow", "Qwen/Qwen3.8-27B", Protocol::Responses);
        assert_eq!(output_semantics(&hosted, &json!({"max_output_tokens":32})), OutputSemantics::Total);
        // Reasoning.effort wins over enable_thinking on native Responses.
        let old = ctx("dashscope", "deepseek-v4-pro", Protocol::Responses);
        assert_eq!(output_semantics(&old, &json!({"reasoning":{"effort":"high"},"enable_thinking":false})), OutputSemantics::AnswerOnly { reasoning_budget: None });
        assert_eq!(output_semantics(&old, &json!({"reasoning":{"effort":"none"}})), OutputSemantics::Total);
    }

    #[test]
    fn unknown_dashscope_budget_is_not_a_verified_reasoning_ceiling() {
        let c = ctx("dashscope", "future-model", Protocol::Chat);
        assert_eq!(output_semantics(&c,&json!({"thinking_budget":64})),OutputSemantics::AnswerOnly{reasoning_budget:None});
        let c = ctx("siliconflow", "Qwen/Qwen3.8-27B", Protocol::Messages);
        assert_eq!(output_semantics(&c,&json!({"thinking":{"budget_tokens":128}})),OutputSemantics::AnswerOnly{reasoning_budget:Some(128)});
    }
    #[test]
    fn mimo_none_is_off_but_other_efforts_are_not_differentiated() {
        let c = ctx("mimo", "mimo-v2.6-pro", Protocol::Chat);
        let mut b = json!({"reasoning_effort":"none","max_completion_tokens":128,"temperature":0.2,"messages":[{"role":"assistant","reasoning_content":"original"}]});
        assert_eq!(adapt(&c,&mut b),Ok(true));
        assert_eq!(b["thinking"],json!({"type":"disabled"}));
        assert_eq!(b["temperature"],json!(0.2));
        assert_eq!(b["messages"][0]["reasoning_content"],json!("original"));
        assert!(validate(&c,&json!({"tool_choice":"required"})).is_err());
        assert!(validate(&c,&json!({"response_format":{"type":"json_schema"}})).is_err());
        assert!(validate(&c,&json!({"max_completion_tokens":131073})).is_err());
        let r = ctx("mimo", "mimo-v2.6-pro", Protocol::Responses);
        assert!(validate(&r,&json!({"reasoning":{"effort":"none"}})).is_ok());
        assert!(validate(&r,&json!({"reasoning":{"effort":"ultra"}})).is_ok());
        assert!(validate(&r,&json!({"previous_response_id":"abc"})).is_err());
        assert_eq!(output_semantics(&r,&json!({"max_output_tokens":32})),OutputSemantics::Total);
    }

}

#[cfg(test)]
mod p1_combinations {
    use super::*;
    fn ctx<'a>(p: &'a str, m: &'a str, protocol: Protocol) -> Context<'a> {
        Context { base_url:"https://proxy.test/v1", model:m, protocol, api_version:None, is_oauth:false, profile_override:Some(p) }
    }
    #[test]
    fn deepseek_native_responses_rejects_ignored_intent() {
        let c=ctx("deepseek","deepseek-flash",Protocol::Responses);
        for bad in [json!({"parallel_tool_calls":false}),json!({"reasoning":{"summary":"auto"}}),json!({"text":{"verbosity":"low"}}),json!({"tools":[{"type":"mcp"}]}),json!({"previous_response_id":"r"}),json!({"store":true}),json!({"input":[{"role":"developer","content":"rules"}]}),json!({"input":[{"type":"reasoning","summary":[{"type":"summary_text","text":"summary"}]}]}),json!({"input":[{"type":"unknown_item"}]}),json!({"input":[{"role":"user","content":[{"type":"input_file","file_id":"f"}]}]})] {
            assert!(validate(&c,&bad).is_err(),"{bad}");
        }
        for good in [json!({"input":[{"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"original"}]}]}),json!({}),json!({"parallel_tool_calls":true}),json!({"store":false}),json!({"reasoning":{"effort":"none"},"tool_choice":"required"}),json!({"text":{"format":{"type":"json_schema","schema":{}}}})] { assert!(validate(&c,&good).is_ok(),"{good}"); }
    }
    #[test]
    fn deepseek_chat_forcing_requires_explicit_off() {
        let c=ctx("deepseek","deepseek-v4-pro",Protocol::Chat);
        for choice in [json!("required"),json!({"type":"function","function":{"name":"f"}})] {
            assert!(validate(&c,&json!({"tool_choice":choice})).is_err());
            assert!(validate(&c,&json!({"tool_choice":choice,"thinking":{"type":"disabled"}})).is_ok());
        }
        assert!(validate(&c,&json!({"tool_choice":"auto"})).is_ok());
        assert!(validate(&c,&json!({"response_format":{"type":"json_schema"}})).is_err());
    }
    #[test]
    fn qwen_tool_controls_are_model_and_mode_specific() {
        let c=ctx("dashscope","qwen3.8-max",Protocol::Chat);
        for off in [true,false] { assert!(validate(&c,&json!({"enable_thinking":off,"tool_choice":"required"})).is_err()); }
        assert!(validate(&c,&json!({"tool_choice":{"type":"function","function":{"name":"f"}}})).is_err());
        assert!(validate(&c,&json!({"enable_thinking":false,"tool_choice":{"type":"function","function":{"name":"f"}}})).is_ok());
        assert!(validate(&c,&json!({"enable_thinking":false,"reasoning_effort":"high","tool_choice":{"type":"function","function":{"name":"f"}}})).is_err());
        assert!(validate(&ctx("dashscope","kimi-k3",Protocol::Chat),&json!({"parallel_tool_calls":false})).is_err());
        assert!(validate(&ctx("dashscope","deepseek-v4.1-flash-1017",Protocol::Chat),&json!({"tool_choice":"required"})).is_ok());
    }
    #[test]
    fn kimi_responses_is_separate_from_chat_k3_and_k2() {
        let c=ctx("moonshot","kimi-k3",Protocol::Responses);
        for bad in [json!({"tool_choice":"required"}),json!({"reasoning":{"effort":"none"}}),json!({"reasoning":{"summary":"auto"}}),json!({"previous_response_id":"r"}),json!({"store":true}),json!({"background":true})] { assert!(validate(&c,&bad).is_err(),"{bad}"); }
        assert!(validate(&c,&json!({"tool_choice":"auto","reasoning":{"effort":"max"}})).is_ok());
        assert!(validate(&ctx("moonshot","kimi-k3",Protocol::Chat),&json!({"tool_choice":"required"})).is_ok());
        assert!(validate(&ctx("moonshot","kimi-k2.6",Protocol::Chat),&json!({"tool_choice":"required"})).is_err());
    }
    #[test]
    fn glm_format_tools_and_product_line_are_explicit() {
        for m in ["glm-5.3","glm-5.3-flash","glm-5.3-flashx","glm-5-turbo"] {
            let c=ctx("zai",m,Protocol::Chat);
            for bad in [json!({"tool_choice":"required"}),json!({"response_format":{"type":"json_schema"}}),json!({"tool_stream":true})] { assert!(validate(&c,&bad).is_err(),"{m}: {bad}"); }
            let stream=validate(&c,&json!({"tool_stream":true,"stream":true,"tool_choice":"auto","response_format":{"type":"json_object"}}));
            assert_eq!(stream.is_ok(), ["glm-5.3","glm-5-turbo"].contains(&m));
            assert!(validate(&c,&json!({"tool_choice":"auto","response_format":{"type":"json_object"}})).is_ok());
        }
        let mut c=ctx("zai","glm-5.3",Protocol::Chat);
        assert!(validate(&c,&json!({"reasoning_effort":"none"})).is_err());
        c.base_url="https://open.bigmodel.cn/api/coding/paas/v4";
        assert!(validate(&c,&json!({"reasoning_effort":"none"})).is_ok());
        assert!(validate(&c,&json!({"thinking":{"type":"disabled"}})).is_err());
    }
}

#[cfg(test)]
mod p1_hosted_none {
    use super::*;
    #[test]
    fn hosted_models_do_not_inherit_qwen_none_adapter() {
        for model in ["kimi-k3", "kimi/kimi-k3.1-pro", "kimi/kimi-k3.1-pro-highspeed", "MiniMax/MiniMax-M3.1-Flash", "deepseek-v4.1-flash-1017"] {
            let c=Context {base_url:"https://dashscope.aliyuncs.com/compatible-mode/v1",model,protocol:Protocol::Chat,api_version:None,is_oauth:false,profile_override:None};
            let mut body=json!({"reasoning_effort":"none","future":"preserve"});let before=body.clone();
            assert_eq!(adapt(&c,&mut body),Ok(false),"{model}");assert_eq!(body,before,"{model}");
            assert!(validate(&c,&body).is_err(),"{model}: unsupported off must be explicit rejection");
            assert_eq!(output_semantics(&c,&body),output_semantics(&c,&json!({})),"{model}: unsupported off must not shrink reservation");
        }
    }
}
