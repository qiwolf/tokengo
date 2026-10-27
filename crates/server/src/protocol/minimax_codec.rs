//! Exact-source MiniMax native Chat split codec for a public Responses client.
//! Same-protocol traffic and signed Messages conversion never enter this codec.
use super::{Protocol, sse::{Transcoder, Decoder}};
use crate::parameter_contracts::Context;
use serde_json::Value;

fn protocol_fault(error: impl std::fmt::Display) -> String {
    format!("Unsupported MiniMax split conversion: {error}")
}


#[derive(Clone, Debug)]
pub struct Policy { model: String }

pub fn policy(ctx: &Context<'_>, client: Protocol) -> Option<Policy> {
    (!ctx.is_oauth && ctx.protocol == Protocol::Chat && client == Protocol::Responses
        && crate::parameter_contracts::provider(ctx) == "minimax"
        && matches!(ctx.model, "MiniMax-M3" | "MiniMax-M3.1-Flash"))
        .then(|| Policy { model: ctx.model.to_owned() })
}
impl Policy {
    pub fn convert_request(&self, body: &Value) -> Result<Value, String> {
        let mut input = body.clone();
        validate_control_messages(&input)?;
        let map = input.as_object_mut().ok_or("Unsupported MiniMax request: expected object")?;
        if let Some(split) = map.remove("reasoning_split").filter(|v| !v.is_null()) {
            if split != Value::Bool(true) {
                return Err("Unsupported MiniMax conversion requires reasoning_split=true; no-split tags have no lossless grammar".into());
            }
        }
        let thinking = map.remove("thinking").filter(|v| !v.is_null());
        if let Some(value) = &thinking {
            super::fields(value, &["type"])?;
            if !matches!(value["type"].as_str(), Some("adaptive" | "disabled")) {
                return Err("Unsupported MiniMax thinking requires a documented adaptive/disabled type".into());
            }
        }
        let off = input.pointer("/reasoning/effort").and_then(Value::as_str) == Some("none");
        if self.model == "MiniMax-M3.1-Flash"
            && (off || thinking.as_ref().is_some_and(|v| v["type"] == "disabled")) {
            return Err("Unsupported MiniMax-M3.1-Flash cannot disable thinking".into());
        }
        if off && thinking.as_ref().is_some_and(|v| v["type"] != "disabled") {
            return Err("Unsupported MiniMax explicit none conflicts with thinking control".into());
        }
        if off {
            if let Some(reasoning) = input.get_mut("reasoning").and_then(Value::as_object_mut) {
                reasoning.remove("effort");
            }
        }
        let mut outgoing = super::convert_request(Protocol::Responses, Protocol::Chat, &input).map_err(protocol_fault)?;
        // A closed schema can establish the caller's strict intent, but cannot
        // establish that this provider enforces it. Never downgrade that intent.
        if outgoing["tools"].as_array().is_some_and(|tools| tools.iter().any(|tool|
            tool["type"] == "function" && tool["function"]["strict"] == true)) {
            return Err("Unsupported MiniMax strict guarantee: this source has no verified enforcement contract; select a source with a verified guarantee or a suitable native route".into());
        }
        fold_control_messages(&mut outgoing)?;
        outgoing["reasoning_split"] = Value::Bool(true);
        if let Some(thinking) = thinking {
            outgoing["thinking"] = thinking;
        } else if off {
            outgoing["thinking"] = serde_json::json!({"type":"disabled"});
        }
        Ok(outgoing)
    }
    pub fn convert_response(&self, body: &Value) -> Result<Value, String> {
        let mut metadata = Metadata::default();
        let normalized = metadata.normalize(body)?;
        let mut converted = super::convert_response(Protocol::Chat, Protocol::Responses, &normalized).map_err(protocol_fault)?;
        metadata.attach(&mut converted);
        Ok(converted)
    }
    pub fn stream(&self) -> Stream {
        Stream { inner: Transcoder::new(Protocol::Chat, Protocol::Responses),
            decoder: Decoder::default(), metadata: Metadata::default(), failed: None }
    }
}


/// Documented semantic fold: the target has one system control layer. This
/// preserves ordered text, not OpenAI's independent system/developer hierarchy.
fn validate_control_messages(body: &Value) -> Result<(), String> {
    let Some(items) = body.get("input").and_then(Value::as_array) else { return Ok(()); };
    let mut conversation_started = false;
    for item in items {
        if !matches!(item["role"].as_str(), Some("system" | "developer")) {
            conversation_started = true;
            continue;
        }
        if conversation_started {
            return Err("Unsupported MiniMax control message after conversation content".into());
        }
        super::fields(item, &["type", "role", "content"])?;
        if item.get("type").is_some_and(|v| v != "message") {
            return Err("Unsupported MiniMax control item type".into());
        }
        if !item["content"].is_string() {
            let parts = item["content"].as_array().ok_or("Unsupported MiniMax control content must be plain text")?;
            for part in parts {
                super::fields(part, &["type", "text"])?;
                if !matches!(part["type"].as_str(), Some("text" | "input_text")) || !part["text"].is_string() {
                    return Err("Unsupported MiniMax control content must be plain text".into());
                }
            }
        }
    }
    Ok(())
}
fn fold_control_messages(body: &mut Value) -> Result<(), String> {
    let messages = body["messages"].as_array_mut().ok_or("Unsupported MiniMax messages shape")?;
    let count = messages.iter().take_while(|m| matches!(m["role"].as_str(), Some("system" | "developer"))).count();
    if count == 0 { return Ok(()); }
    let mut text = Vec::new();
    for message in &messages[..count] {
        super::fields(message, &["role", "content"])?;
        text.push(message["content"].as_str().ok_or("Unsupported MiniMax control text shape")?.to_owned());
    }
    messages.splice(0..count, [serde_json::json!({"role":"system","content":text.join("\n\n")})]);
    Ok(())
}

/// Only neutral, explicitly documented diagnostics are representable as metadata.
/// Policy flags and upstream error codes are never treated as inert decoration.
#[derive(Default)]
struct Metadata { fields: serde_json::Map<String, Value> }
impl Metadata {
    fn normalize(&mut self, original: &Value) -> Result<Value, String> {
        let mut normalized = original.clone();
        let object = normalized.as_object_mut().ok_or("Unsupported MiniMax response: expected object")?;
        for key in ["input_sensitive", "output_sensitive"] {
            if let Some(value) = object.remove(key) {
                if value != Value::Bool(false) {
                    return Err("Unsupported MiniMax policy semantics: sensitive flag is active or invalid".into());
                }
                self.fields.insert(key.into(), value);
            }
        }
        for key in ["input_sensitive_type", "output_sensitive_type", "output_sensitive_int"] {
            if let Some(value) = object.remove(key) {
                if value.as_u64() != Some(0) {
                    return Err("Unsupported MiniMax policy semantics: sensitive classification is non-neutral or invalid".into());
                }
                self.fields.insert(key.into(), value);
            }
        }
        if let Some(status) = object.remove("base_resp") {
            super::fields(&status, &["status_code", "status_msg"])?;
            match status["status_code"].as_u64() {
                Some(0) => {},
                Some(_) => return Err("MiniMax base_resp reports an upstream error".into()),
                None => return Err("Unsupported MiniMax base_resp has an invalid status code".into()),
            }
            if let Some(message) = status.get("status_msg") {
                if message.as_str().is_none_or(|s| s.len() > 1024) {
                    return Err("Unsupported MiniMax status message is invalid or exceeds its bound".into());
                }
            }
            self.fields.insert("base_resp".into(), status);
        }
        if serde_json::to_vec(&self.fields).map_err(|_| "Unsupported MiniMax metadata")?.len() > 65536 {
            return Err("Unsupported MiniMax provider metadata exceeds 64 KiB".into());
        }
        Ok(normalized)
    }
    fn attach(&self, target: &mut Value) {
        if !self.fields.is_empty() {
            target["tokengo_provider_metadata"] = serde_json::json!({
                "provider":"minimax", "protocol":"chat", "fields":self.fields,
            });
        }
    }
    fn annotate(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        if self.fields.is_empty() || bytes.is_empty() { return Ok(bytes.to_vec()); }
        // Transcoder output consists of complete encoded frames; user text is
        // JSON escaped and cannot be confused with an SSE frame separator.
        let text = std::str::from_utf8(bytes).map_err(|_| "Unsupported MiniMax converted SSE UTF-8")?;
        let mut out = Vec::new();
        for encoded in text.split("\n\n").filter(|s| !s.is_empty()) {
            let event = encoded.lines().find_map(|l| l.strip_prefix("event: ")).map(str::to_owned);
            let data = encoded.lines().filter_map(|l| l.strip_prefix("data: ")).collect::<Vec<_>>().join("\n");
            let mut value: Value = serde_json::from_str(&data).map_err(|_| "Unsupported MiniMax converted SSE JSON")?;
            self.attach(&mut value);
            if let Some(response) = value.get_mut("response") { self.attach(response); }
            out.extend(super::sse::Frame { event, data: value.to_string() }.encode());
        }
        Ok(out)
    }
}

pub struct Stream {
    inner: Transcoder,
    decoder: Decoder,
    metadata: Metadata,
    failed: Option<String>,
}
impl Stream {
    fn frames(&mut self, frames: Vec<super::sse::Frame>) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        for mut frame in frames {
            if !frame.data.trim().is_empty() && frame.data.trim() != "[DONE]" {
                let value: Value = serde_json::from_str(&frame.data).map_err(|_| "Unsupported MiniMax SSE JSON")?;
                frame.data = self.metadata.normalize(&value)?.to_string();
            }
            let converted = self.inner.feed(&frame.encode()).map_err(protocol_fault)?;
            out.extend(self.metadata.annotate(&converted)?);
        }
        Ok(out)
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        if let Some(error) = &self.failed { return Err(error.clone()); }
        let result = self.decoder.feed(bytes).map_err(protocol_fault).and_then(|frames| self.frames(frames));
        if let Err(error) = &result { self.failed = Some(error.clone()); }
        result
    }
    pub fn finish(&mut self) -> Result<Vec<u8>, String> {
        if let Some(error) = &self.failed { return Err(error.clone()); }
        let result: Result<Vec<u8>, String> = (|| {
            let frames = self.decoder.finish().map_err(protocol_fault)?;
            let mut out = self.frames(frames)?;
            let tail = self.inner.finish().map_err(protocol_fault)?;
            out.extend(self.metadata.annotate(&tail)?);
            Ok(out)
        })();
        if let Err(error) = &result { self.failed = Some(error.clone()); }
        result
    }
}
#[cfg(test)]
#[path = "minimax_codec_tests.rs"]
mod tests;
