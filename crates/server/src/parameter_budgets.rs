//! Output ceilings are a source contract, not a spelling alias. No wire mutation.
use crate::{parameter_contracts::{self, Context}, parameter_contracts_china::{self, OutputSemantics}, protocol::Protocol};
use serde_json::Value;

/// Nullable optional output limits have the same semantics as omission, never zero.
pub fn cap_value<'a>(body: &'a Value, key: &str) -> Option<&'a Value> {
    body.get(key).filter(|value| !value.is_null())
}
pub fn normalize_null_caps(body: &mut Value) {
    if let Some(map) = body.as_object_mut() {
        for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
            if map.get(key).is_some_and(Value::is_null) { map.remove(key); }
        }
    }
}

fn integer(body: &Value, key: &str, zero: bool) -> Result<Option<i64>, String> {
    cap_value(body,key).map(|v| v.as_i64().filter(|n| if zero { *n >= 0 } else { *n > 0 })
        .ok_or_else(|| format!("{key} must be a {}integer", if zero { "non-negative " } else { "positive " }))).transpose()
}

/// A zero output cap is only an input-billed native Anthropic cache-warming call.
/// This does not loosen OAuth's minimum or other providers' positive cap checks.
pub fn allows_zero(ctx: &Context<'_>) -> bool {
    !ctx.is_oauth && ctx.protocol == Protocol::Messages && parameter_contracts::provider(ctx) == "anthropic"
}

pub fn validate_warmup(ctx: &Context<'_>, client: Protocol, body: &Value) -> Result<(),String> {
    if ctx.protocol==Protocol::Messages && cap_value(body,"max_tokens").and_then(Value::as_i64)==Some(0) {
        if client!=Protocol::Messages || !allows_zero(ctx) {
            return Err("Zero-output cache warming requires native Anthropic Messages; it cannot be represented by this source/conversion".into());
        }
        if body.get("stream")==Some(&Value::Bool(true)) {
            return Err("Streaming zero-output cache warming requires a dedicated empty-success stream contract; use non-streaming native Messages".into());
        }
    }
    Ok(())
}

pub fn valid_warmup_response(body: &Value) -> bool {
    body.get("type").and_then(Value::as_str)==Some("message")
        && body.get("id").and_then(Value::as_str).is_some_and(|v|!v.is_empty())
        && body.get("model").and_then(Value::as_str).is_some_and(|v|!v.is_empty())
        && body.get("role").and_then(Value::as_str)==Some("assistant")
        && body.get("content").and_then(Value::as_array).is_some_and(Vec::is_empty)
        && body.get("stop_reason").and_then(Value::as_str)==Some("max_tokens")
        && body.pointer("/usage/output_tokens").and_then(Value::as_u64)==Some(0)
        && body.pointer("/usage/input_tokens").and_then(Value::as_u64).is_some()
        && ["cache_creation_input_tokens","cache_read_input_tokens"].iter().all(|key|
            body.get("usage").and_then(|v|v.get(*key)).is_none_or(|v|v.as_u64().is_some()))
}

pub fn default_output_key(ctx: &Context<'_>) -> &'static str {
    match ctx.protocol {
        Protocol::Responses => "max_output_tokens",
        Protocol::Messages => "max_tokens",
        Protocol::Chat if parameter_contracts::provider(ctx)=="mimo"
            || (parameter_contracts::provider(ctx)=="openai" && matches!(ctx.model,"gpt-6-astra"|"gpt-6.1-sol"|"gpt-6-sol"|"gpt-6-luna")) => "max_completion_tokens",
        Protocol::Chat => "max_tokens",
    }
}

/// A provider-native switch is explicit client intent, not an absent effort.
pub fn native_thinking_override(ctx: &Context<'_>, body: &Value) -> bool {
    if ctx.is_oauth { return false; }
    match parameter_contracts::provider(ctx) {
        "deepseek" | "moonshot" | "zai" | "minimax" | "mimo" => body.get("thinking").is_some(),
        "anthropic" => body.pointer("/thinking/type").and_then(Value::as_str)==Some("disabled"),
        "dashscope" | "siliconflow" => body.get("enable_thinking").is_some() || body.get("thinking").is_some(),
        "gemini" => body.pointer("/extra_body/google/thinking_config").is_some(),
        "vllm" | "ollama" => body.get("think").is_some() || body.pointer("/chat_template_kwargs/enable_thinking").is_some(),
        _ => false,
    }
}

pub fn explicit_cap(body: &Value) -> bool {
    ["max_tokens", "max_completion_tokens", "max_output_tokens"].iter().any(|k| cap_value(body,*k).is_some())
}

/// `ceiling` is an independently verified source output bound, never a price cap.
/// An answer-only cap never stands in for an unknown amount of paid reasoning.
pub fn output_bound(ctx: &Context<'_>, body: &Value, ceiling: Option<i64>, choices: i64) -> Result<i64, String> {
    if choices <= 0 { return Err("n must be positive".into()); }
    let zero = allows_zero(ctx);
    if ctx.protocol == Protocol::Chat && parameter_contracts::provider(ctx) == "mimo"
        && cap_value(body,"max_tokens").is_some() {
        return Err("MiMo Chat requires max_completion_tokens; max_tokens has no verified enforcement contract".into());
    }
    if ctx.protocol == Protocol::Chat && parameter_contracts::provider(ctx) == "dashscope"
        && cap_value(body,"max_completion_tokens").is_some() && !parameter_contracts_china::has_inclusive_chat_cap(ctx) {
        return Err("This DashScope model has no verified inclusive max_completion_tokens contract".into());
    }
    let keys: &[&str] = match ctx.protocol {
        Protocol::Chat => &["max_tokens", "max_completion_tokens"],
        Protocol::Responses => &["max_output_tokens"],
        Protocol::Messages => &["max_tokens"],
    };
    for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        if !keys.contains(&key) && cap_value(body,key).is_some() {
            return Err(format!("{key} does not belong to this native protocol"));
        }
    }
    // Determine the meaning of max_tokens independently of the separate inclusive
    // DashScope control, rather than requiring two different quantities to agree.
    let answer_shape = if ctx.protocol==Protocol::Chat && parameter_contracts::provider(ctx)=="dashscope"
        && cap_value(body,"max_tokens").is_some() && cap_value(body,"max_completion_tokens").is_some() {
        let mut shape=body.clone(); shape.as_object_mut().unwrap().remove("max_completion_tokens"); Some(shape)
    } else { None };
    let semantics = parameter_contracts_china::output_semantics(ctx, answer_shape.as_ref().unwrap_or(body));
    let per_choice = match semantics {
        OutputSemantics::Total => {
            let mut value = None;
            for key in keys {
                if let Some(n) = integer(body, key, zero)? {
                    if value.is_some_and(|old| old != n) { return Err("Conflicting total-output limits".into()); }
                    value = Some(n);
                }
            }
            value.unwrap_or(4096)
        }
        OutputSemantics::Unknown => {
            for key in keys { integer(body, key, zero)?; }
            ceiling.filter(|n| *n > 0)
                .ok_or("Unknown source output-limit semantics require verified total output bounds")?
        }
        OutputSemantics::AnswerOnly { reasoning_budget } => {
            let answer_key = if ctx.protocol == Protocol::Responses { "max_output_tokens" } else { "max_tokens" };
            let answer = integer(body, answer_key, false)?;
            // DashScope documents this separate inclusive control. It is not an
            // alias of answer-only max_tokens, so unequal values are not merged.
            let inclusive = if parameter_contracts_china::has_inclusive_chat_cap(ctx) {
                integer(body, "max_completion_tokens", false)?
            } else {
                if cap_value(body,"max_completion_tokens").is_some() {
                    return Err("Source does not declare an inclusive max_completion_tokens contract".into());
                }
                None
            };
            if let Some(total) = inclusive { total }
            else if let Some(budget) = reasoning_budget {
                if budget < 0 { return Err("Unknown/unbounded reasoning budget requires verified total output bounds".into()); }
                answer.unwrap_or(4096).checked_add(budget).ok_or("Total reasoning/output reservation overflow")?
            } else {
                ceiling.filter(|n| *n > 0).ok_or_else(||format!("Answer-only {answer_key} requires verified total output bounds when reasoning budget is unknown"))?
            }
        }
    };
    per_choice.checked_mul(choices).ok_or_else(|| "Total output reservation overflow".into())
}

/// Conversion starts with an inclusive client cap. Use a documented inclusive
/// wire field or refuse; never send the total as an answer-only source limit.
pub fn preserve_converted_total_cap(ctx: &Context<'_>, from: Protocol, body: &mut Value) -> Result<(), String> {
    if from == ctx.protocol || !explicit_cap(body) { return Ok(()); }
    // Work on a copy: a failed conversion cannot remove the caller's cap.
    let mut converted = body.clone();
    let needs_inclusive = matches!(parameter_contracts_china::output_semantics(ctx, body), OutputSemantics::AnswerOnly { .. } | OutputSemantics::Unknown);
    let mimo_chat = ctx.protocol == Protocol::Chat && parameter_contracts::provider(ctx) == "mimo";
    if needs_inclusive || mimo_chat || (parameter_contracts_china::has_inclusive_chat_cap(ctx) && cap_value(body,"max_tokens").is_some()) {
        if parameter_contracts_china::has_inclusive_chat_cap(ctx) {
            if let Some(cap) = cap_value(&converted,"max_tokens").cloned() {
                if cap_value(&converted,"max_completion_tokens").is_some_and(|v| *v != cap) {
                    return Err("Conflicting inclusive output caps after conversion".into());
                }
                converted.as_object_mut().ok_or("Request must be an object")?.remove("max_tokens");
                converted["max_completion_tokens"] = cap;
            }
        } else {
            return Err("Inclusive output cap cannot be converted to this source's answer-only limit; use its native protocol and declare the reasoning budget".into());
        }
    }
    *body = converted;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn context(profile: &'static str, model: &'static str, protocol: Protocol) -> Context<'static> {
        Context { base_url: "https://compatible.example.test/v1", model, protocol, api_version: None, is_oauth: false, profile_override: Some(profile) }
    }
    #[test]
    fn zero_is_only_an_explicit_native_cache_warm_contract() {
        let c = context("anthropic", "claude-sonnet-5-5", Protocol::Messages);
        assert_eq!(output_bound(&c, &json!({"max_tokens":0}), None, 1).unwrap(), 0);
        let mut other = context("openai", "gpt-6-luna", Protocol::Responses);
        assert!(output_bound(&other, &json!({"max_output_tokens":0}), None, 1).is_err());
        other.protocol = Protocol::Messages; other.is_oauth = true;
        assert!(!allows_zero(&other));
    }
    #[test]
    fn ordinary_total_limits_reject_conflicts_and_overflow_without_mutation() {
        let c = context("openai", "gpt-6-luna", Protocol::Chat);
        let body = json!({"max_tokens":32,"max_completion_tokens":31});
        assert!(output_bound(&c, &body, Some(128), 1).is_err());
        assert!(output_bound(&c, &json!({"max_completion_tokens":i64::MAX}), None, 2).is_err());
        assert_eq!(output_bound(&c, &json!({"max_completion_tokens":16}), None, 2).unwrap(), 32);
        assert_eq!(body, json!({"max_tokens":32,"max_completion_tokens":31}));
    }
    #[test]
    fn siliconflow_answer_caps_never_underreserve_reasoning() {
        let c = context("siliconflow", "Qwen/Qwen3.8-27B", Protocol::Chat);
        let body = json!({"max_tokens":64,"enable_thinking":true,"thinking_budget":128});
        assert_eq!(output_bound(&c, &body, Some(1024), 1).unwrap(), 192);
        let unknown = json!({"max_tokens":64,"enable_thinking":true});
        assert_eq!(output_bound(&c, &unknown, Some(1024), 1).unwrap(), 1024);
        assert!(output_bound(&c, &unknown, None, 1).is_err());
    }
    #[test]
    fn p0_dashscope_responses_never_use_answer_cap_as_total() {
        let c = context("dashscope", "qwen3.7-plus", Protocol::Responses);
        let original = json!({"max_output_tokens":32,"thinking_budget":64});
        assert!(output_bound(&c, &original, None, 1).is_err());
        assert_eq!(output_bound(&c, &original, Some(1024), 1).unwrap(),1024);
        let unknown = context("dashscope", "future-model", Protocol::Responses);
        assert!(output_bound(&unknown, &original, None, 1).is_err());
        let mut converted = json!({"max_output_tokens":32});
        assert!(preserve_converted_total_cap(&c, Protocol::Chat, &mut converted).is_err());
        assert_eq!(converted, json!({"max_output_tokens":32}));
        let new = context("dashscope", "qwen3.8-max", Protocol::Responses);
        assert_eq!(output_bound(&new, &original, None, 1).unwrap(),32);
        // The protocol-native answer field is max_output_tokens, not max_tokens.
        let off = json!({"max_output_tokens":32,"enable_thinking":false});
        assert_eq!(output_bound(&c, &off, None, 1).unwrap(),32);
    }

    #[test]
    fn inclusive_conversion_is_exact_model_and_atomic() {
        let c=context("dashscope","qwen3.8-max",Protocol::Chat);
        let mut body=json!({"max_tokens":64,"max_completion_tokens":32});
        let original=body.clone();
        assert!(preserve_converted_total_cap(&c,Protocol::Messages,&mut body).is_err());
        assert_eq!(body,original);
        let unknown=context("dashscope","future-qwen",Protocol::Chat);
        let mut body=json!({"max_tokens":64});
        assert!(preserve_converted_total_cap(&unknown,Protocol::Messages,&mut body).is_err());
        assert_eq!(body,json!({"max_tokens":64}));
        assert!(output_bound(&unknown,&json!({"max_completion_tokens":32}),Some(1024),1).is_err());
        let mimo=context("mimo","mimo-v2.6-pro",Protocol::Chat);
        assert!(output_bound(&mimo,&body,Some(131072),1).is_err());
        preserve_converted_total_cap(&mimo,Protocol::Responses,&mut body).unwrap();
        assert_eq!(body,json!({"max_completion_tokens":64}));
    }
    #[test]
    fn oauth_never_uses_native_provider_thinking_override() {
        let mut c=context("mimo","mimo-v2.6-pro",Protocol::Responses);
        c.is_oauth=true;
        assert!(!native_thinking_override(&c,&json!({"thinking":{"type":"disabled"}})));
    }

    #[test]
    fn zero_warmup_keeps_input_usage_and_rejects_invalid_success_shapes() {
        let c=context("anthropic","claude-sonnet-5-5",Protocol::Messages);
        assert!(validate_warmup(&c,Protocol::Messages,&json!({"max_tokens":0})).is_ok());
        assert!(validate_warmup(&c,Protocol::Chat,&json!({"max_tokens":0})).is_err());
        assert!(validate_warmup(&c,Protocol::Messages,&json!({"max_tokens":0,"stream":true})).is_err());
        let valid=json!({"type":"message","id":"warm","model":"claude-sonnet-5-5","role":"assistant","content":[],"stop_reason":"max_tokens","usage":{"input_tokens":100,"output_tokens":0,"cache_creation_input_tokens":90,"cache_read_input_tokens":0}});
        assert!(valid_warmup_response(&valid));
        let mut invalid=valid.clone(); invalid["usage"]["output_tokens"]=Value::from(1);
        assert!(!valid_warmup_response(&invalid));
        let mut invalid=valid.clone(); invalid["usage"].as_object_mut().unwrap().remove("input_tokens");
        assert!(!valid_warmup_response(&invalid));
        let mut invalid=valid; invalid["content"]=json!([{"type":"text","text":"unexpected"}]);
        assert!(!valid_warmup_response(&invalid));
    }

    #[test]
    fn nullable_caps_are_omitted_and_never_zero_or_unbounded() {
        let c=context("openai","gpt-6-luna",Protocol::Chat);
        let mut b=json!({"max_tokens":null,"max_completion_tokens":null});
        assert!(!explicit_cap(&b));
        assert_eq!(output_bound(&c,&b,Some(8192),1).unwrap(),4096);
        normalize_null_caps(&mut b); assert_eq!(b,json!({}));
        assert_eq!(output_bound(&c,&json!({"max_tokens":null,"max_completion_tokens":32}),Some(8192),1).unwrap(),32);
        let c=context("siliconflow","Qwen/Qwen3.8-27B",Protocol::Chat);
        assert!(output_bound(&c,&json!({"max_tokens":null}),None,1).is_err());
        assert_eq!(output_bound(&c,&json!({"max_tokens":null}),Some(8192),1).unwrap(),8192);
    }

}
