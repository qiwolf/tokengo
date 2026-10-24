use super::{fields, object, Protocol};
use serde_json::{json, Value};

fn message(protocol: Protocol, body: &Value) -> Result<Value, String> {
    let canonical = match protocol {
        Protocol::Chat => {
            let choices = body["choices"].as_array().ok_or("Expected choices")?;
            if choices.len() != 1 {
                return Err("Multiple response choices cannot be converted".into());
            }
            let c = &choices[0];
            fields(c, &["index", "message", "finish_reason", "logprobs"])?;
            if c.get("logprobs").is_some_and(|v| !v.is_null()) {
                return Err("Response logprobs cannot be converted".into());
            }
            let v = super::request::decode_messages(
                Protocol::Chat,
                &json!({"messages":[c["message"]]}),
            )?;
            v
        }
        Protocol::Responses => {
            let mut output = body["output"].clone();
            if body["status"] == "incomplete"
                && body["incomplete_details"]["reason"] == "max_output_tokens"
            {
                if let Some(items) = output.as_array_mut() {
                    for item in items {
                        if item["status"] == "incomplete" {
                            item.as_object_mut()
                                .ok_or("Invalid response item")?
                                .remove("status");
                        }
                    }
                }
            }
            super::request::decode_messages(protocol, &json!({"input":output}))?
        }
        Protocol::Messages => super::request::decode_messages(
            protocol,
            &json!({"messages":[{"role":"assistant","content":body["content"]}]}),
        )?,
    };
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut reasoning: Option<String> = None;
    for row in canonical {
        if row["role"] != "assistant" {
            return Err("Response contains non-assistant item".into());
        }
        if let Some(raw) = row.get("reasoning_content") {
            if reasoning.is_some() {
                return Err("Multiple reasoning segments cannot be merged".into());
            }
            reasoning = Some(raw.as_str().ok_or("Reasoning must be text")?.to_owned());
        }
        if let Some(s) = row["content"].as_str() {
            text.push_str(s);
        }
        if let Some(items) = row["tool_calls"].as_array() {
            calls.extend(items.iter().cloned());
        }
    }
    let mut result = json!({"role":"assistant","content":if text.is_empty()&&!calls.is_empty(){Value::Null}else{json!(text)}});
    if let Some(reasoning) = reasoning {
        result["reasoning_content"] = json!(reasoning);
    }
    if !calls.is_empty() {
        result["tool_calls"] = json!(calls);
    }
    Ok(result)
}
fn reason(protocol: Protocol, body: &Value, tools: bool) -> Result<&'static str, String> {
    match protocol {
        Protocol::Chat => match body["choices"][0]["finish_reason"].as_str() {
            Some("stop") => Ok("stop"),
            Some("tool_calls") => Ok("tool"),
            Some("length") => Ok("length"),
            _ => Err("Unsupported or missing Chat finish reason".into()),
        },
        Protocol::Messages => match body["stop_reason"].as_str() {
            Some("end_turn") => Ok("stop"),
            Some("tool_use") => Ok("tool"),
            Some("max_tokens") => Ok("length"),
            _ => Err("Unsupported Messages stop reason".into()),
        },
        Protocol::Responses => match body["status"].as_str() {
            Some("completed") => Ok(if tools { "tool" } else { "stop" }),
            Some("incomplete") if body["incomplete_details"]["reason"] == "max_output_tokens" => {
                Ok("length")
            }
            _ => Err("Failed, stored or unsupported Responses status".into()),
        },
    }
}
fn usage(protocol: Protocol, body: &Value) -> Result<Option<(u64, u64)>, String> {
    let Some(v) = body.get("usage").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    object(v)?;
    let read = |key: &str| {
        v.get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("Missing/nonnegative {key} usage"))
    };
    let (input, output) = if protocol == Protocol::Chat {
        (read("prompt_tokens")?, read("completion_tokens")?)
    } else {
        (read("input_tokens")?, read("output_tokens")?)
    };
    if protocol == Protocol::Messages {
        // Cache buckets use distinct pricing semantics; silently adding/dropping them
        // would make a conversion claim a different financial observation.
        for key in ["cache_creation_input_tokens", "cache_read_input_tokens"] {
            if v.get(key).is_some_and(|v| v.as_u64() != Some(0)) {
                return Err(
                    "Cached Messages usage cannot be represented by the target protocol".into(),
                );
            }
        }
    }
    Ok(Some((input, output)))
}
/// The two protocols define this same output-token subset. Missing is unknown, not zero.
pub(super) fn reasoning_tokens(protocol: Protocol, usage: &Value) -> Result<Option<u64>, String> {
    let key = if protocol == Protocol::Chat {
        "completion_tokens_details"
    } else {
        "output_tokens_details"
    };
    let Some(details) = usage.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    object(details)?;
    let Some(value) = details.get("reasoning_tokens").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let n = value
        .as_u64()
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or("Invalid reasoning token count")?;
    let total = usage.get(if protocol == Protocol::Chat {
        "completion_tokens"
    } else {
        "output_tokens"
    });
    if total.and_then(Value::as_u64).is_some_and(|total| n > total) {
        return Err("Reasoning token count exceeds total output".into());
    }
    Ok(Some(n))
}
pub fn convert_response(from: Protocol, to: Protocol, body: &Value) -> Result<Value, String> {
    if from == to {
        return Ok(body.clone());
    }
    fields(
        body,
        match from {
            Protocol::Chat => &[
                "id",
                "object",
                "created",
                "model",
                "choices",
                "usage",
                "system_fingerprint",
                "service_tier",
                // llama.cpp diagnostics do not change text, tool or usage semantics.
                "timings",
            ],
            Protocol::Responses => &[
                "id",
                "object",
                "created_at",
                "model",
                "status",
                "output",
                "usage",
                "incomplete_details",
                "error",
                "metadata",
                "instructions",
                "max_output_tokens",
                "parallel_tool_calls",
                "temperature",
                "top_p",
                "tools",
                "tool_choice",
                "store",
                "previous_response_id",
                "conversation",
                "reasoning",
                "text",
                "truncation",
                "background",
                "service_tier",
                "max_tool_calls",
                "user",
                "safety_identifier",
                "prompt_cache_key",
                "prompt_cache_retention",
            ],
            Protocol::Messages => &[
                "id",
                "type",
                "role",
                "model",
                "content",
                "stop_reason",
                "stop_sequence",
                "usage",
            ],
        },
    )?;
    if body.get("error").is_some_and(|v| !v.is_null()) {
        return Err("Error response requires protocol-specific error handling".into());
    }
    if from == Protocol::Responses {
        for key in ["previous_response_id", "conversation"] {
            if body.get(key).is_some_and(|v| !v.is_null()) {
                return Err("Provider conversation state cannot be converted".into());
            }
        }
        if let Some(reasoning) = body.get("reasoning").filter(|v| !v.is_null()) {
            fields(reasoning, &["effort", "summary"])?;
            if object(reasoning)?.values().any(|v| !v.is_null()) {
                return Err("Reasoning response semantics cannot be converted".into());
            }
        }
        if let Some(text) = body.get("text").filter(|v| !v.is_null()) {
            fields(text, &["format"])?;
            fields(&text["format"], &["type"])?;
            if text["format"]["type"] != "text" {
                return Err("Structured output semantics cannot be converted".into());
            }
        }
    }

    if from == Protocol::Messages && body.get("stop_sequence").is_some_and(|v| !v.is_null()) {
        return Err("Custom stop sequence result cannot be represented losslessly".into());
    }
    let msg = message(from, body)?;
    let finish = reason(from, body, msg.get("tool_calls").is_some())?;
    let counts = usage(from, body)?;
    let reasoning_count = if matches!(
        (from, to),
        (Protocol::Chat, Protocol::Responses) | (Protocol::Responses, Protocol::Chat)
    ) {
        reasoning_tokens(from, &body["usage"])?
    } else {
        None
    };
    let id = body.get("id").cloned().unwrap_or(json!("converted"));
    let model = body.get("model").cloned().unwrap_or(json!("unknown"));
    let mut out = match to {
        Protocol::Chat => {
            json!({"id":id,"object":"chat.completion","created":body.get("created_at").or_else(||body.get("created")).cloned().unwrap_or(json!(0)),"model":model,"choices":[{"index":0,"message":msg,"finish_reason":match finish{"tool"=>"tool_calls","length"=>"length",_=>"stop"}}]})
        }
        Protocol::Messages => {
            let mut encoded = json!({});
            super::request::encode_messages(Protocol::Messages, &[msg], &mut encoded)?;
            json!({"id":id,"type":"message","role":"assistant","model":model,"content":encoded["messages"][0]["content"],"stop_reason":match finish{"tool"=>"tool_use","length"=>"max_tokens",_=>"end_turn"},"stop_sequence":null})
        }
        Protocol::Responses => {
            let mut encoded = json!({});
            super::request::encode_messages(Protocol::Responses, &[msg], &mut encoded)?;
            for (index, item) in encoded["input"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                let prefix = if item["type"] == "function_call" {
                    "fc"
                } else if item["type"] == "reasoning" {
                    "rs"
                } else {
                    "msg"
                };
                item["id"] = json!(format!(
                    "{prefix}_{}",
                    tokengo_shared::short_hash(&format!("{id}:{index}"))
                ));
                item["status"] = json!(if finish == "length" {
                    "incomplete"
                } else {
                    "completed"
                });
                if let Some(blocks) = item.get_mut("content").and_then(Value::as_array_mut) {
                    for block in blocks.iter_mut().filter(|b| b["type"] == "output_text") {
                        block["annotations"] = json!([]);
                    }
                }
            }
            let mut out = json!({"id":id,"object":"response","created_at":body.get("created").cloned().unwrap_or(json!(0)),"model":model,"status":if finish=="length"{"incomplete"}else{"completed"},"output":encoded["input"],"error":null,"incomplete_details":null});
            if finish == "length" {
                out["incomplete_details"] = json!({"reason":"max_output_tokens"});
            }
            out
        }
    };
    if let Some((input, output)) = counts {
        out["usage"] = if to == Protocol::Chat {
            json!({"prompt_tokens":input,"completion_tokens":output,"total_tokens":input.checked_add(output).ok_or("Usage total overflow")?})
        } else if to == Protocol::Messages {
            json!({"input_tokens":input,"output_tokens":output})
        } else {
            json!({"input_tokens":input,"output_tokens":output,"total_tokens":input.checked_add(output).ok_or("Usage total overflow")?})
        };
    }
    if let Some(n) = reasoning_count {
        out["usage"][if to == Protocol::Chat {
            "completion_tokens_details"
        } else {
            "output_tokens_details"
        }] = json!({"reasoning_tokens":n});
    }
    Ok(out)
}
// Native validation recognizes real payload while preserving unknown fields.
// Do not deserialize native blocks into the deliberately narrower conversion IR.
fn payload(value: &Value) -> bool {
    match value {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(items) => items.iter().any(payload),
        Value::Object(object) => object.iter().any(|(name, value)| {
            ![
                "type",
                "id",
                "status",
                "role",
                "index",
                "annotations",
                "logprobs",
            ]
            .contains(&name.as_str())
                && payload(value)
        }),
        Value::Null => false,
        Value::Bool(_) | Value::Number(_) => true,
    }
}
fn native_block(block: &Value) -> bool {
    if !block.is_object() {
        return false;
    }
    match block.get("type").and_then(Value::as_str) {
        Some("text" | "input_text" | "output_text") => block.get("text").is_some_and(payload),
        Some("refusal") => block
            .get("refusal")
            .or_else(|| block.get("text"))
            .is_some_and(payload),
        Some("message") => block.get("content").is_some_and(|content| {
            content
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(native_block))
        }),
        Some("function_call") => {
            block.get("call_id").is_some_and(payload)
                && block.get("name").is_some_and(payload)
                && block.get("arguments").is_some_and(Value::is_string)
        }
        Some("tool_use") => {
            block.get("id").is_some_and(payload) && block.get("name").is_some_and(payload)
        }
        // Native reasoning, server tools and opaque future output are intentionally
        // not translated or erased. A type/id/status shell alone is never payload.
        _ => payload(block),
    }
}
pub fn validate_response(protocol: Protocol, body: &Value) -> Result<(), String> {
    object(body)?;
    if body.get("error").is_some_and(|v| !v.is_null()) {
        return Err("Upstream returned an error object".into());
    }
    let valid = match protocol {
        Protocol::Chat => body
            .get("choices")
            .and_then(Value::as_array)
            .is_some_and(|choices| {
                choices.iter().any(|choice| {
                    let Some(message) = choice.get("message").and_then(Value::as_object) else {
                        return false;
                    };
                    message.get("content").is_some_and(|content| {
                        if let Some(blocks) = content.as_array() {
                            blocks.iter().any(native_block)
                        } else {
                            content.as_str().is_some_and(|s| !s.trim().is_empty())
                        }
                    }) || [
                        "reasoning_content",
                        "reasoning",
                        "reasoning_details",
                        "thinking",
                        "refusal",
                        "function_call",
                    ]
                    .iter()
                    .any(|field| message.get(*field).is_some_and(payload))
                        || message
                            .get("tool_calls")
                            .and_then(Value::as_array)
                            .is_some_and(|calls| calls.iter().any(payload))
                })
            }),
        Protocol::Messages => {
            body["type"] == "message"
                && body["role"] == "assistant"
                && body
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|blocks| blocks.iter().any(native_block))
        }
        Protocol::Responses => {
            body["object"] == "response"
                && (body["status"] == "completed"
                    || (body["status"] == "incomplete"
                        && body["incomplete_details"]["reason"] == "max_output_tokens"))
                && body
                    .get("output")
                    .and_then(Value::as_array)
                    .is_some_and(|items| items.iter().any(native_block))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "Invalid, incomplete or empty {protocol:?} response"
        ))
    }
}
