//! Narrow adapter for observed ChatGPT OAuth terminal snapshots buffered for Chat.
//! Native Responses remains opaque; the general cross-protocol converter stays strict.
use super::{convert_response, Protocol};
use serde_json::{json, Map, Value};

fn observed_zero_tools() -> Value {
    json!({"image_gen":{"input_tokens":0,"input_tokens_details":{"image_tokens":0,"text_tokens":0},"output_tokens":0,"output_tokens_details":{"image_tokens":0,"text_tokens":0},"total_tokens":0},"web_search":{"num_requests":0}})
}

fn supported_text_echo(value: &Value) -> bool {
    if super::fields(value, &["format", "verbosity"]).is_err()
        || !["low", "medium", "high"].iter().any(|level| value["verbosity"] == *level)
    {
        return false;
    }
    let format = &value["format"];
    let chat = if format["type"] == "json_schema" {
        if super::fields(format, &["type", "name", "description", "schema", "strict"]).is_err() {
            return false;
        }
        let mut schema = format.clone();
        let Some(map) = schema.as_object_mut() else { return false; };
        map.remove("type");
        // Responses may echo an absent description as null. Preserve the original
        // metadata; only the validation projection omits that empty optional value.
        if map.get("description").is_some_and(Value::is_null) { map.remove("description"); }
        json!({"type":"json_schema", "json_schema":schema})
    } else {
        format.clone()
    };
    super::request::chat_response_format(&chat).is_ok()
}

/// Convert only the observed inert OAuth metadata, preserving it outside Chat semantics.
pub fn convert_oauth_buffered_response(client: Protocol, body: &Value) -> Result<Value, String> {
    if client != Protocol::Chat {
        return convert_response(Protocol::Responses, client, body);
    }
    let output_limited = body["status"] == "incomplete"
        && body["incomplete_details"]["reason"] == "max_output_tokens"
        && body.get("error").is_none_or(Value::is_null);
    let mut normalized = body.clone();
    let object = normalized
        .as_object_mut()
        .ok_or("Expected OAuth response object")?;
    let mut metadata = Map::new();
    for key in [
        "access_programs",
        "completed_at",
        "frequency_penalty",
        "presence_penalty",
        "moderation",
        "top_logprobs",
        "tool_usage",
        "reasoning",
        "text",
    ] {
        let Some(value) = object.get(key) else {
            continue;
        };
        let accepted = match key {
            "access_programs" => *value == json!({"cyber":"standard"}),
            "completed_at" => value.as_u64().is_some() || (output_limited && value.is_null()),
            "frequency_penalty" | "presence_penalty" => value.as_f64() == Some(0.0),
            "moderation" => value.is_null(),
            "top_logprobs" => value.as_u64() == Some(0),
            "tool_usage" => *value == observed_zero_tools(),
            "reasoning" => ["low", "medium", "high"].iter().any(|effort| *value == json!({"context":"all_turns","effort":effort,"mode":"standard","summary":null})),
            "text" => supported_text_echo(value),
            _ => unreachable!(),
        };
        if !accepted {
            // Existing strict semantics still handle older standard reasoning/text shapes.
            if matches!(key, "reasoning" | "text") {
                continue;
            }
            return Err(format!("Unsupported OAuth response metadata: {key}"));
        }
        metadata.insert(key.to_owned(), object.remove(key).unwrap());
    }
    let completed = object.get("status").and_then(Value::as_str) == Some("completed");
    let mut phases = Vec::new();
    if let Some(items) = object.get_mut("output").and_then(Value::as_array_mut) {
        for (index, item) in items.iter_mut().enumerate() {
            if let Some(phase) = item.get("phase") {
                if !(completed || output_limited)
                    || item["type"] != "message"
                    || item["role"] != "assistant"
                    || !(item["status"] == "completed" || (output_limited && item["status"] == "incomplete"))
                    || phase != "final_answer"
                {
                    return Err("Unsupported OAuth output message phase".into());
                }
                phases.push(json!({"output_index":index,"id":item["id"],"phase":phase}));
                item.as_object_mut()
                    .ok_or("Invalid OAuth output item")?
                    .remove("phase");
            }
        }
    }
    if !phases.is_empty() {
        metadata.insert("output_message_phases".into(), json!(phases));
    }
    let mut converted = convert_response(Protocol::Responses, client, &normalized)?;
    if !metadata.is_empty() {
        converted["provider_metadata"] = json!({"openai_responses":metadata});
    }
    Ok(converted)
}
