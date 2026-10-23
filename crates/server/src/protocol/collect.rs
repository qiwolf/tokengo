//! Collect a stream-only Responses source for a non-streaming downstream caller.
//! Finalized items survive providers that leave terminal `response.output` empty.
use super::{
    sse::{Frame, Observer},
    Protocol,
};
use serde_json::Value;
use std::collections::BTreeMap;

/// Public ChatGPT-plan preview. Unsupported limits are rejected, never silently stripped.
pub fn prepare_request(
    body: &mut Value,
    effort: Option<&str>,
    tier: Option<&str>,
) -> Result<(), String> {
    let map = body
        .as_object_mut()
        .ok_or("Expected OAuth Responses request object")?;
    if map.get("store").is_some_and(|v| v != false) {
        return Err("ChatGPT plan requests require store=false".into());
    }
    for field in [
        "background",
        "conversation",
        "max_output_tokens",
        "max_tokens",
        "max_completion_tokens",
        "max_tool_calls",
        "metadata",
        "moderation",
        "multi_agent",
        "prompt",
        "prompt_cache_retention",
        "safety_identifier",
        "temperature",
        "top_logprobs",
        "top_p",
        "truncation",
        "user",
        "previous_response_id",
    ] {
        if map.contains_key(field) {
            return Err(format!(
                "ChatGPT plan preview does not support {field}; no upstream request was sent"
            ));
        }
    }
    if let Some(input) = map.get("input").and_then(Value::as_str) {
        let text = input.to_owned();
        map.insert(
            "input".into(),
            serde_json::json!([{"role":"user","content":[{"type":"input_text","text":text}]}]),
        );
    }
    let input = map
        .get("input")
        .and_then(Value::as_array)
        .ok_or("ChatGPT plan requests require an input array")?;
    let leading_systems = input
        .iter()
        .take_while(|item| item["role"] == "system")
        .count();
    if input[leading_systems..]
        .iter()
        .any(|item| item["role"] == "system")
    {
        return Err(
            "ChatGPT plan cannot move mid-conversation system messages into instructions".into(),
        );
    }
    if leading_systems > 0 {
        // Chat clients send a system prompt as the first message. The subscription
        // transport accepts it as instructions, retaining its priority over developer
        // messages. Validate everything before removing any source content.
        let mut instructions = Vec::new();
        if let Some(existing) = map.get("instructions") {
            if !existing.is_null() {
                instructions.push(
                    existing
                        .as_str()
                        .ok_or("Expected text instructions")?
                        .to_owned(),
                );
            }
        }
        for item in &input[..leading_systems] {
            super::fields(item, &["type", "role", "content"])?;
            if item.get("type").is_some_and(|kind| kind != "message") {
                return Err("Expected system message item".into());
            }
            let content = &item["content"];
            let text = if let Some(text) = content.as_str() {
                text.to_owned()
            } else {
                let mut text = String::new();
                for part in content.as_array().ok_or("Expected system text content")? {
                    super::fields(part, &["type", "text"])?;
                    if part["type"] != "input_text" {
                        return Err(
                            "ChatGPT plan system instructions require input_text blocks".into()
                        );
                    }
                    text.push_str(part["text"].as_str().ok_or("Expected system text block")?);
                }
                text
            };
            instructions.push(text);
        }
        map.get_mut("input")
            .and_then(Value::as_array_mut)
            .unwrap()
            .drain(..leading_systems);
        map.insert(
            "instructions".into(),
            Value::String(instructions.join("\n\n")),
        );
    }
    // The subscription preview accepts bare client tools inside additional_tools,
    // retaining each name/schema/call identity instead of inventing a namespace.
    if let Some(tools) = map.get("tools") {
        let tools = tools.as_array().ok_or("Expected OAuth tools array")?;
        let mut additional = Vec::new();
        let mut namespaces = Vec::new();
        for tool in tools {
            match tool["type"].as_str() {
                Some("function"|"custom")=>additional.push(tool.clone()),
                Some("namespace")=>namespaces.push(tool.clone()),
                _=>return Err("ChatGPT plan preview requires client function/custom tools in namespaces or additional_tools".into()),
            }
        }
        if !additional.is_empty() {
            map.get_mut("input").and_then(Value::as_array_mut).ok_or("Expected OAuth input array")?
                .insert(0,serde_json::json!({"type":"additional_tools","role":"developer","tools":additional}));
            if namespaces.is_empty() {
                map.remove("tools");
            } else {
                map.insert("tools".into(), Value::Array(namespaces));
            }
        }
    }
    map.insert("store".into(), Value::Bool(false));
    map.insert("stream".into(), Value::Bool(true));
    if let Some(effort) = effort {
        if !["low", "medium", "high"].contains(&effort) {
            return Err("Invalid configured OAuth reasoning effort".into());
        }
        let value = map
            .entry("reasoning")
            .or_insert_with(|| serde_json::json!({}));
        let reasoning = value.as_object_mut().ok_or("Expected reasoning object")?;
        reasoning
            .entry("effort")
            .or_insert_with(|| Value::String(effort.into()));
    }
    if let Some(tier) = tier {
        if !["priority", "auto", "default"].contains(&tier) {
            return Err("Invalid configured OAuth service tier".into());
        }
        map.entry("service_tier")
            .or_insert_with(|| Value::String(tier.into()));
    }
    Ok(())
}

pub struct ResponsesCollector {
    pub observer: Observer,
    items: BTreeMap<u64, Value>,
    terminal: Option<Value>,
    bytes: usize,
}
impl Default for ResponsesCollector {
    fn default() -> Self {
        Self {
            observer: Observer::new(Protocol::Responses),
            items: BTreeMap::new(),
            terminal: None,
            bytes: 0,
        }
    }
}
impl ResponsesCollector {
    fn frames(&mut self, frames: Vec<Frame>) -> Result<(), String> {
        for frame in frames {
            let value: Value =
                serde_json::from_str(&frame.data).map_err(|_| "Invalid Responses event JSON")?;
            let kind = value["type"]
                .as_str()
                .or(frame.event.as_deref())
                .unwrap_or("");
            if kind == "response.output_item.done" {
                let index = value["output_index"]
                    .as_u64()
                    .filter(|i| *i < 128)
                    .ok_or("Invalid Responses output index")?;
                let item = value
                    .get("item")
                    .filter(|i| i.is_object())
                    .ok_or("Missing finalized Responses item")?;
                if self.items.insert(index, item.clone()).is_some() {
                    return Err("Duplicate finalized Responses item".into());
                }
            } else if kind == "response.completed" {
                if self.terminal.is_some() {
                    return Err("Duplicate Responses completion".into());
                }
                self.terminal = Some(value["response"].clone());
            }
        }
        Ok(())
    }
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.bytes = self.bytes.saturating_add(chunk.len());
        if self.bytes > 16 * 1024 * 1024 {
            return Err("Collected Responses stream exceeds 16 MiB".into());
        }
        let frames = self.observer.feed(chunk)?;
        self.frames(frames)
    }
    pub fn finish(&mut self) -> Result<Value, String> {
        let frames = self.observer.finish_frames()?;
        self.frames(frames)?;
        let mut result = self
            .terminal
            .take()
            .ok_or("Responses stream has no completed response")?;
        let output = result
            .get("output")
            .and_then(Value::as_array)
            .ok_or("Completed Responses output must be an array")?;
        if output.is_empty() && !self.items.is_empty() {
            if self.items.keys().copied().ne(0..self.items.len() as u64) {
                return Err("Missing finalized Responses item".into());
            }
            result["output"] =
                Value::Array(std::mem::take(&mut self.items).into_values().collect());
        } else {
            for (index, item) in &self.items {
                if output.get(*index as usize) != Some(item) {
                    return Err("Conflicting Responses terminal item".into());
                }
            }
        }
        super::validate_response(Protocol::Responses, &result)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn frame(v: Value) -> Vec<u8> {
        format!("data: {v}\n\n").into_bytes()
    }
    fn item() -> Value {
        json!({"id":"msg_test","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"你好","annotations":[]}]})
    }
    fn completed(output: Value) -> Vec<u8> {
        frame(
            json!({"type":"response.completed","response":{"id":"resp_test","object":"response","status":"completed","output":output,"usage":{"input_tokens":7,"output_tokens":3}}}),
        )
    }
    #[test]
    fn preserves_final_items_when_terminal_output_is_empty_and_does_not_require_tcp_eof() {
        let mut c = ResponsesCollector::default();
        let mut bytes =
            frame(json!({"type":"response.output_item.done","output_index":0,"item":item()}));
        bytes.extend(completed(json!([])));
        for b in bytes.chunks(1) {
            c.feed(b).unwrap();
        }
        assert!(c.observer.done);
        assert_eq!(c.observer.input, Some(7));
        let v = c.finish().unwrap();
        assert_eq!(v["output"], json!([item()]));
        assert_eq!(v["usage"]["output_tokens"], 3);
    }
    #[test]
    fn native_opaque_reasoning_and_parallel_function_items_survive() {
        let items = json!([{"type":"reasoning","id":"rs_1","encrypted_content":"synthetic-opaque","summary":[]},{"type":"function_call","id":"fc_1","call_id":"call_1","name":"sum","arguments":"{}","status":"completed"},{"type":"function_call","id":"fc_2","call_id":"call_2","name":"other","arguments":"{}","status":"completed"}]);
        let mut c = ResponsesCollector::default();
        c.feed(&completed(items.clone())).unwrap();
        assert_eq!(c.finish().unwrap()["output"], items);
    }
    #[test]
    fn incomplete_failed_missing_terminal_conflicts_and_gaps_fail_closed() {
        for v in [
            json!({"type":"response.failed","response":{"status":"failed"}}),
            json!({"type":"response.incomplete","response":{"status":"incomplete"}}),
        ] {
            assert!(ResponsesCollector::default().feed(&frame(v)).is_err());
        }
        let mut c = ResponsesCollector::default();
        c.feed(&frame(
            json!({"type":"response.output_item.done","output_index":0,"item":item()}),
        ))
        .unwrap();
        assert!(c.finish().is_err());
        for index in [0, 2] {
            let mut c = ResponsesCollector::default();
            c.feed(&frame(
                json!({"type":"response.output_item.done","output_index":index,"item":item()}),
            ))
            .unwrap();
            c.feed(&completed(if index==0 {json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"different"}]}])}else{json!([])})).unwrap();
            assert!(c.finish().is_err());
        }
    }
    #[test]
    fn total_stream_memory_is_bounded_even_with_small_frames() {
        let mut c = ResponsesCollector::default();
        let ping = b": heartbeat\n\n";
        c.bytes = 16 * 1024 * 1024 - ping.len();
        c.feed(ping).unwrap();
        assert!(c.feed(ping).is_err());
    }
    #[test]
    fn oauth_transport_rejects_unsupported_limits_and_respects_native_fields_and_explicit_dials() {
        let mut v = json!({"input":"hello","stream":false,"reasoning":{"effort":"high"},"service_tier":"default","include":["reasoning.encrypted_content"]});
        prepare_request(&mut v, Some("low"), Some("priority")).unwrap();
        assert_eq!(v["stream"], true);
        assert_eq!(v["store"], false);
        assert!(v["input"].is_array());
        assert!(v.get("max_output_tokens").is_none());
        assert_eq!(v["reasoning"]["effort"], "high");
        assert_eq!(v["service_tier"], "default");
        assert_eq!(v["include"], json!(["reasoning.encrypted_content"]));
        let mut v = json!({"input":"hello"});
        prepare_request(&mut v, Some("medium"), Some("priority")).unwrap();
        assert_eq!(v["reasoning"]["effort"], "medium");
        assert_eq!(v["service_tier"], "priority");
        for mut v in [
            json!({"store":true}),
            json!({"background":true}),
            json!({"reasoning":null}),
        ] {
            assert!(prepare_request(&mut v, Some("low"), None).is_err());
        }
        for field in [
            "max_output_tokens",
            "temperature",
            "top_p",
            "metadata",
            "previous_response_id",
        ] {
            let mut v = json!({"input":"hello"});
            v[field] = json!(17);
            assert!(prepare_request(&mut v, None, None).is_err());
            assert_eq!(v[field], 17, "must reject rather than strip");
        }
        let mut v = json!({"input":[{"role":"system","content":"rules"}]});
        prepare_request(&mut v, None, None).unwrap();
        assert_eq!(v["instructions"], "rules");
        assert_eq!(v["input"], json!([]));
    }
    #[test]
    fn subscription_client_tools_keep_names_schemas_and_replay_identity() {
        let tool = json!({"type":"function","name":"exec_command","description":"Run a fixed command","parameters":{"type":"object","properties":{}},"strict":true});
        let call = json!({"type":"function_call","call_id":"call_original","name":"exec_command","arguments":"{}"});
        let mut v = json!({"input":[call.clone(),{"type":"function_call_output","call_id":"call_original","output":"ok"}],"tools":[tool.clone()]});
        prepare_request(&mut v, None, None).unwrap();
        assert!(v.get("tools").is_none());
        assert_eq!(
            v["input"][0],
            json!({"type":"additional_tools","role":"developer","tools":[tool]})
        );
        assert_eq!(v["input"][1], call);
    }
}
