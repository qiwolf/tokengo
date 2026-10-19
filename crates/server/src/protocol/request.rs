use super::{fields, object, string, Protocol};
use serde_json::{json, Value};

fn text(v: &Value) -> Result<String, String> {
    if let Some(s) = v.as_str() {
        return Ok(s.into());
    }
    if v.is_null() {
        return Ok(String::new());
    }
    let parts = v.as_array().ok_or("Expected text or text blocks")?;
    let mut out = String::new();
    for part in parts {
        fields(part, &["type", "text", "annotations", "logprobs"])?;
        if !matches!(
            part["type"].as_str(),
            Some("text" | "input_text" | "output_text")
        ) {
            return Err(
                "Unsupported content block (images/thinking/provider state cannot be converted)"
                    .into(),
            );
        }
        for key in ["annotations", "logprobs"] {
            if part
                .get(key)
                .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
            {
                return Err(format!("Unsupported nonempty {key}"));
            }
        }
        out.push_str(&string(part, "text")?);
    }
    Ok(out)
}
fn message(role: &str, content: String) -> Value {
    json!({"role":role,"content":content})
}
fn call(id: String, name: String, args: String) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":args}})
}
fn valid_status(v: &Value) -> Result<(), String> {
    if v.get("status").is_some_and(|s| s != "completed") {
        Err("Incomplete provider item cannot be converted".into())
    } else {
        Ok(())
    }
}
pub(super) fn plain_reasoning(item: &Value) -> Result<&str, String> {
    fields(
        item,
        &[
            "type",
            "id",
            "status",
            "summary",
            "content",
            "encrypted_content",
        ],
    )?;
    valid_status(item)?;
    if let Some(id) = item.get("id") {
        if !id
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && !s.chars().any(char::is_control))
        {
            return Err("Reasoning id must be nonempty text without controls".into());
        }
    }
    if item["type"] != "reasoning" || item.get("encrypted_content").is_some_and(|v| !v.is_null()) {
        return Err("Opaque reasoning cannot be converted".into());
    }
    if !item["summary"].as_array().is_some_and(Vec::is_empty) {
        return Err("Reasoning summaries are not raw reasoning".into());
    }
    let parts = item["content"]
        .as_array()
        .ok_or("Expected raw reasoning content")?;
    if parts.len() != 1 {
        return Err("Only one exact reasoning_text part is convertible".into());
    }
    fields(&parts[0], &["type", "text"])?;
    if parts[0]["type"] != "reasoning_text" {
        return Err("Expected reasoning_text".into());
    }
    let text = parts[0]["text"]
        .as_str()
        .ok_or("Reasoning text must be a string")?;
    if text.len() > 16 * 1024 * 1024 {
        return Err("Reasoning exceeds 16 MiB".into());
    }
    Ok(text)
}
pub(super) fn decode_messages(protocol: Protocol, body: &Value) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    match protocol {
        Protocol::Chat => {
            for m in body["messages"]
                .as_array()
                .ok_or("Expected messages array")?
            {
                fields(
                    m,
                    &[
                        "role",
                        "content",
                        "tool_calls",
                        "tool_call_id",
                        "name",
                        "reasoning_content",
                    ],
                )?;
                let role = string(m, "role")?;
                if !["system", "developer", "user", "assistant", "tool"].contains(&role.as_str()) {
                    return Err("Unsupported message role".into());
                }
                if m.get("name").is_some() {
                    return Err("Named chat messages cannot be converted".into());
                }
                if m.get("tool_call_id").is_some() && role != "tool" {
                    return Err("tool_call_id requires tool role".into());
                }
                let mut row = message(&role, text(m.get("content").unwrap_or(&Value::Null))?);
                if let Some(reasoning) = m.get("reasoning_content").filter(|v| !v.is_null()) {
                    if role != "assistant" {
                        return Err("reasoning_content requires assistant role".into());
                    }
                    let reasoning = reasoning.as_str().ok_or("reasoning_content must be text")?;
                    if reasoning.len() > 16 * 1024 * 1024 {
                        return Err("Reasoning exceeds 16 MiB".into());
                    }
                    row["reasoning_content"] = json!(reasoning);
                }
                if role == "tool" {
                    row["tool_call_id"] = json!(string(m, "tool_call_id")?);
                }
                if let Some(calls) = m.get("tool_calls") {
                    if role != "assistant" {
                        return Err("Tool calls require assistant role".into());
                    }
                    let calls = calls.as_array().ok_or("Expected tool_calls array")?;
                    for c in calls {
                        fields(c, &["id", "type", "function"])?;
                        if c["type"] != "function" {
                            return Err("Only function tools are convertible".into());
                        }
                        fields(&c["function"], &["name", "arguments"])?;
                        string(c, "id")?;
                        string(&c["function"], "name")?;
                        string(&c["function"], "arguments")?;
                    }
                    row["tool_calls"] = json!(calls);
                    if row["content"] == "" {
                        row["content"] = Value::Null;
                    }
                }
                out.push(row);
            }
        }
        Protocol::Responses => {
            if let Some(instructions) = body.get("instructions") {
                out.push(message(
                    "system",
                    instructions
                        .as_str()
                        .ok_or("instructions must be text")?
                        .into(),
                ));
            }
            if let Some(input) = body.get("input").and_then(Value::as_str) {
                out.push(message("user", input.into()));
                return Ok(out);
            }
            for item in body["input"]
                .as_array()
                .ok_or("Expected Responses input text or array")?
            {
                let kind = item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message");
                match kind {
                    "message" => {
                        fields(item, &["type", "role", "content", "id", "status"])?;
                        valid_status(item)?;
                        let role = string(item, "role")?;
                        if !["system", "developer", "user", "assistant"].contains(&role.as_str()) {
                            return Err("Unsupported Responses role".into());
                        }
                        let content = text(&item["content"])?;
                        if role == "assistant"
                            && out.last().is_some_and(|m| {
                                m.get("reasoning_content").is_some()
                                    && m["content"] == ""
                                    && m.get("tool_calls").is_none()
                            })
                        {
                            out.last_mut().unwrap()["content"] = json!(content);
                        } else {
                            out.push(message(&role, content));
                        }
                    }
                    "reasoning" => {
                        let reasoning = plain_reasoning(item)?;
                        if out.last().is_some_and(|m| m["role"] == "assistant") {
                            return Err("Reasoning must precede its assistant text/tools".into());
                        }
                        out.push(
                            json!({"role":"assistant","content":"","reasoning_content":reasoning}),
                        );
                    }
                    "function_call" => {
                        fields(
                            item,
                            &["type", "call_id", "name", "arguments", "id", "status"],
                        )?;
                        valid_status(item)?;
                        let c = call(
                            string(item, "call_id")?,
                            string(item, "name")?,
                            string(item, "arguments")?,
                        );
                        if let Some(last) = out.last_mut().filter(|m| m["role"] == "assistant") {
                            if last.get("tool_calls").is_none() {
                                last["tool_calls"] = json!([]);
                            }
                            last["tool_calls"].as_array_mut().unwrap().push(c);
                        } else {
                            out.push(json!({"role":"assistant","content":null,"tool_calls":[c]}));
                        }
                    }
                    "function_call_output" => {
                        fields(item, &["type", "call_id", "output", "id", "status"])?;
                        valid_status(item)?;
                        out.push(json!({"role":"tool","tool_call_id":string(item,"call_id")?,"content":text(&item["output"])?}));
                    }
                    _ => return Err(format!("Unsupported Responses input item: {kind}")),
                }
            }
        }
        Protocol::Messages => {
            if let Some(system) = body.get("system") {
                out.push(message("system", text(system)?));
            }
            for m in body["messages"]
                .as_array()
                .ok_or("Expected Messages messages array")?
            {
                fields(m, &["role", "content"])?;
                let role = string(m, "role")?;
                if !["assistant", "user"].contains(&role.as_str()) {
                    return Err("Unsupported Messages role".into());
                }
                if m["content"].is_string() {
                    out.push(message(&role, text(&m["content"])?));
                    continue;
                }
                let mut content = String::new();
                let mut calls = Vec::new();
                for block in m["content"]
                    .as_array()
                    .ok_or("Expected Messages content array")?
                {
                    match block["type"].as_str() {
                        Some("text") => {
                            fields(block, &["type", "text"])?;
                            if !calls.is_empty() {
                                return Err(
                                    "Text after tool-use cannot be represented losslessly".into()
                                );
                            }
                            content.push_str(&string(block, "text")?);
                        }
                        Some("tool_use") => {
                            if role != "assistant" {
                                return Err("tool_use requires assistant".into());
                            }
                            fields(block, &["type", "id", "name", "input"])?;
                            let input = block
                                .get("input")
                                .filter(|v| v.is_object())
                                .ok_or("Tool input must be an object")?;
                            calls.push(call(
                                string(block, "id")?,
                                string(block, "name")?,
                                input.to_string(),
                            ));
                        }
                        Some("tool_result") => {
                            if role != "user" {
                                return Err("tool_result requires user".into());
                            }
                            fields(block, &["type", "tool_use_id", "content", "is_error"])?;
                            if block.get("is_error").is_some_and(|v| v != false) {
                                return Err(
                                    "Error tool-result semantics cannot be converted".into()
                                );
                            }
                            if !content.is_empty() {
                                out.push(message("user", std::mem::take(&mut content)));
                            }
                            out.push(json!({"role":"tool","tool_call_id":string(block,"tool_use_id")?,"content":text(&block["content"])?}));
                        }
                        _ => return Err(
                            "Unsupported Messages content block (thinking/signature/images/cache)"
                                .into(),
                        ),
                    }
                }
                if !calls.is_empty() {
                    out.push(json!({"role":"assistant","content":if content.is_empty(){Value::Null}else{json!(content)},"tool_calls":calls}));
                } else if !content.is_empty() || m["content"].as_array().is_some_and(Vec::is_empty)
                {
                    out.push(message(&role, content));
                }
            }
        }
    }
    Ok(out)
}
pub(super) fn encode_messages(
    protocol: Protocol,
    messages: &[Value],
    body: &mut Value,
) -> Result<(), String> {
    match protocol {
        Protocol::Chat => body["messages"] = json!(messages),
        Protocol::Responses => {
            let mut items = Vec::new();
            for m in messages {
                let role = string(m, "role")?;
                if role == "tool" {
                    items.push(json!({"type":"function_call_output","call_id":string(m,"tool_call_id")?,"output":m["content"]}));
                    continue;
                }
                if let Some(reasoning) = m.get("reasoning_content") {
                    if role != "assistant" {
                        return Err("Reasoning requires assistant".into());
                    }
                    items.push(json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":reasoning}]}));
                }
                if let Some(content) = m["content"]
                    .as_str()
                    .filter(|c| !c.is_empty() || m.get("reasoning_content").is_none())
                {
                    items.push(json!({"type":"message","role":role,"content":[{"type":if role=="assistant"{"output_text"}else{"input_text"},"text":content}]}));
                }
                if let Some(calls) = m["tool_calls"].as_array() {
                    for c in calls {
                        items.push(json!({"type":"function_call","call_id":c["id"],"name":c["function"]["name"],"arguments":c["function"]["arguments"]}));
                    }
                }
            }
            body["input"] = json!(items);
        }
        Protocol::Messages => {
            let mut system = Vec::new();
            let mut items: Vec<Value> = Vec::new();
            for m in messages {
                if m.get("reasoning_content").is_some() {
                    return Err("Plain reasoning has no native Messages signature".into());
                }
                let role = string(m, "role")?;
                if role == "developer" {
                    return Err(
                        "Developer instruction priority cannot be converted to Messages".into(),
                    );
                }
                if role == "system" {
                    if !items.is_empty() {
                        return Err("Mid-conversation system instruction cannot be moved".into());
                    }
                    system.push(json!({"type":"text","text":m["content"]}));
                    continue;
                }
                if role == "tool" {
                    let block = json!({"type":"tool_result","tool_use_id":m["tool_call_id"],"content":m["content"]});
                    if let Some(last) = items
                        .last_mut()
                        .filter(|m| m["role"] == "user" && m["content"].is_array())
                    {
                        last["content"].as_array_mut().unwrap().push(block);
                    } else {
                        items.push(json!({"role":"user","content":[block]}));
                    }
                    continue;
                }
                let mut blocks = Vec::new();
                if let Some(content) = m["content"].as_str() {
                    if !content.is_empty() {
                        blocks.push(json!({"type":"text","text":content}));
                    }
                }
                if let Some(calls) = m["tool_calls"].as_array() {
                    for c in calls {
                        let args: Value = serde_json::from_str(
                            c["function"]["arguments"]
                                .as_str()
                                .ok_or("Tool arguments must be JSON text")?,
                        )
                        .map_err(|_| "Tool arguments are not complete JSON")?;
                        if !args.is_object() {
                            return Err("Tool arguments must be a JSON object".into());
                        }
                        blocks.push(json!({"type":"tool_use","id":c["id"],"name":c["function"]["name"],"input":args}));
                    }
                }
                items.push(json!({"role":role,"content":blocks}));
            }
            if !system.is_empty() {
                body["system"] = json!(system);
            }
            body["messages"] = json!(items);
        }
    }
    Ok(())
}
fn tools(from: Protocol, to: Protocol, v: &Value) -> Result<Value, String> {
    let mut out = Vec::new();
    for tool in v.as_array().ok_or("tools must be an array")? {
        let f = match from {
            Protocol::Chat => {
                fields(tool, &["type", "function"])?;
                if tool["type"] != "function" {
                    return Err("Only function tools can be converted".into());
                }
                &tool["function"]
            }
            _ => tool,
        };
        let allowed = match from {
            Protocol::Messages => vec!["name", "description", "input_schema"],
            Protocol::Chat => vec!["name", "description", "parameters", "strict"],
            Protocol::Responses => vec!["type", "name", "description", "parameters", "strict"],
        };
        fields(f, &allowed)?;
        if from == Protocol::Responses && f["type"] != "function" {
            return Err("Hosted tools cannot be converted".into());
        }
        let name = string(f, "name")?;
        let schema = f
            .get(if from == Protocol::Messages {
                "input_schema"
            } else {
                "parameters"
            })
            .cloned()
            .unwrap_or(json!({"type":"object","properties":{}}));
        if !schema.is_object() {
            return Err("Tool schema must be object".into());
        }
        let mut target = json!({"name":name});
        if let Some(desc) = f.get("description") {
            if !desc.is_string() {
                return Err("Tool description must be text".into());
            }
            target["description"] = desc.clone();
        }
        target[if to == Protocol::Messages {
            "input_schema"
        } else {
            "parameters"
        }] = schema;
        if let Some(strict) = f.get("strict") {
            if !strict.is_boolean() {
                return Err("strict must be boolean".into());
            }
            if to == Protocol::Messages && strict == true {
                return Err("Strict function semantics cannot be converted to Messages".into());
            }
            if to != Protocol::Messages {
                target["strict"] = strict.clone();
            }
        }
        out.push(if to == Protocol::Chat {
            json!({"type":"function","function":target})
        } else {
            if to == Protocol::Responses {
                target["type"] = json!("function");
            }
            target
        });
    }
    Ok(json!(out))
}
fn choice(from: Protocol, to: Protocol, v: &Value) -> Result<Value, String> {
    if from != Protocol::Messages {
        if let Some(kind) = v.as_str() {
            if !["auto", "none", "required"].contains(&kind) {
                return Err("Invalid tool choice for source protocol".into());
            }
        } else if v["type"] != "function" {
            return Err("Only named function choices can be converted".into());
        }
    } else if !matches!(v["type"].as_str(), Some("auto" | "none" | "any" | "tool")) {
        return Err("Invalid Messages tool choice".into());
    }
    let (kind, name) = if from == Protocol::Messages {
        fields(v, &["type", "name", "disable_parallel_tool_use"])?;
        (
            string(v, "type")?,
            v.get("name").and_then(Value::as_str).map(str::to_owned),
        )
    } else if let Some(kind) = v.as_str() {
        (kind.to_string(), None)
    } else if from == Protocol::Chat {
        fields(v, &["type", "function"])?;
        fields(&v["function"], &["name"])?;
        (string(v, "type")?, Some(string(&v["function"], "name")?))
    } else {
        fields(v, &["type", "name"])?;
        (
            string(v, "type")?,
            v.get("name").and_then(Value::as_str).map(str::to_owned),
        )
    };
    let kind = match kind.as_str() {
        "auto" => "auto",
        "none" => "none",
        "required" | "any" => "required",
        "function" | "tool" => "function",
        _ => return Err("Unsupported tool choice".into()),
    };
    if kind == "function" {
        let name = name.ok_or("Named tool choice requires name")?;
        Ok(match to {
            Protocol::Chat => json!({"type":"function","function":{"name":name}}),
            Protocol::Responses => json!({"type":"function","name":name}),
            Protocol::Messages => json!({"type":"tool","name":name}),
        })
    } else {
        Ok(if to == Protocol::Messages {
            json!({"type":if kind=="required"{"any"}else{kind}})
        } else {
            json!(kind)
        })
    }
}

pub fn convert_request(from: Protocol, to: Protocol, body: &Value) -> Result<Value, String> {
    if from == to {
        return Ok(body.clone());
    }
    fields(
        body,
        match from {
            Protocol::Chat => &[
                "model",
                "messages",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "temperature",
                "top_p",
                "stream",
                "metadata",
                "max_tokens",
                "max_completion_tokens",
                "stop",
                "stream_options",
                "n",
            ],
            Protocol::Responses => &[
                "model",
                "input",
                "instructions",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "temperature",
                "top_p",
                "stream",
                "metadata",
                "max_output_tokens",
                "store",
                "client_metadata",
                "prompt_cache_key",
                "reasoning",
                "include",
            ],
            Protocol::Messages => &[
                "model",
                "messages",
                "system",
                "tools",
                "tool_choice",
                "temperature",
                "top_p",
                "stream",
                "metadata",
                "max_tokens",
                "stop_sequences",
                "output_config",
            ],
        },
    )?;
    validate_cli_hints(from, body)?;
    if has_image(from, body) {
        return Err("Image conversion is not supported; use native protocol".into());
    }
    if body.get("n").is_some_and(|v| v != 1) {
        return Err("Multiple choices cannot be converted".into());
    }
    if body.get("store").is_some_and(|v| v != false) {
        return Err("Stored Responses state cannot be converted".into());
    }
    for key in ["temperature", "top_p"] {
        if let Some(v) = body.get(key) {
            let n = v.as_f64().ok_or("Sampling parameter must be numeric")?;
            let max = if key == "temperature" && to != Protocol::Messages {
                2.0
            } else {
                1.0
            };
            if n < 0.0 || n > max {
                return Err("Sampling value cannot be mapped to target protocol".into());
            }
        }
    }
    if body.get("stream").is_some_and(|v| !v.is_boolean()) {
        return Err("stream must be boolean".into());
    }
    let messages = decode_messages(from, body)?;
    let mut result = json!({"model":string(body,"model")?});
    encode_messages(to, &messages, &mut result)?;
    if to == Protocol::Responses {
        result["store"] = json!(false);
    }
    for key in ["temperature", "top_p", "stream", "metadata"] {
        if let Some(v) = body.get(key) {
            if key == "metadata" {
                object(v)?;
                if to == Protocol::Messages {
                    fields(v, &["user_id"])?;
                }
            }
            result[key] = v.clone();
        }
    }
    // Responses and Messages carry stream meters in their native terminal events.
    // Ask a converted Chat source for its real meter instead of relying on target placeholders.
    if to == Protocol::Chat && result["stream"] == true {
        result["stream_options"] = json!({"include_usage":true});
    }
    let mut cap = None;
    for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        if let Some(v) = body.get(key) {
            let n = v
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or("Output cap must be positive integer")?;
            if cap.is_some_and(|old| old != n) {
                return Err("Conflicting output caps".into());
            }
            cap = Some(n);
        }
    }
    if let Some(cap) = cap.or_else(|| (to == Protocol::Messages).then_some(4096)) {
        result[if to == Protocol::Chat {
            "max_tokens"
        } else if to == Protocol::Messages {
            "max_tokens"
        } else {
            "max_output_tokens"
        }] = json!(cap);
    }
    if let Some(v) = body.get("tools") {
        result["tools"] = tools(from, to, v)?;
    }
    if let Some(v) = body.get("tool_choice") {
        result["tool_choice"] = choice(from, to, v)?;
    }
    if body
        .get("tool_choice")
        .and_then(|v| v.get("disable_parallel_tool_use"))
        .is_some_and(|v| !v.is_boolean())
    {
        return Err("disable_parallel_tool_use must be boolean".into());
    }
    let parallel = body.get("parallel_tool_calls").cloned().or_else(|| {
        body.get("tool_choice")
            .and_then(|v| v.get("disable_parallel_tool_use"))
            .map(|v| json!(!v.as_bool().unwrap_or(false)))
    });
    if let Some(v) = parallel {
        let p = v.as_bool().ok_or("parallel_tool_calls must be boolean")?;
        if to == Protocol::Messages {
            if result.get("tool_choice").is_none() {
                result["tool_choice"] = json!({"type":"auto"});
            }
            result["tool_choice"]["disable_parallel_tool_use"] = json!(!p);
        } else {
            result["parallel_tool_calls"] = json!(p);
        }
    }
    if let Some(stop) = body.get("stop").or_else(|| body.get("stop_sequences")) {
        if to == Protocol::Responses {
            return Err("Stop sequences cannot be converted to Responses".into());
        }
        let stops = if stop.is_string() {
            json!([stop])
        } else {
            stop.clone()
        };
        if stops
            .as_array()
            .is_none_or(|a| a.iter().any(|v| !v.is_string()))
        {
            return Err("Stop sequences must contain strings".into());
        }
        result[if to == Protocol::Messages {
            "stop_sequences"
        } else {
            "stop"
        }] = stops;
    }
    if let Some(v) = body.get("stream_options") {
        fields(v, &["include_usage"])?;
        if v.get("include_usage").is_some_and(|v| !v.is_boolean()) {
            return Err("include_usage must be boolean".into());
        }
    }
    Ok(result)
}
pub fn has_image(protocol: Protocol, body: &Value) -> bool {
    fn contains_image(value: &Value) -> bool {
        match value {
            Value::Array(items) => items.iter().any(contains_image),
            Value::Object(object) => {
                matches!(
                    object.get("type").and_then(Value::as_str),
                    Some("image" | "image_url" | "input_image")
                ) || ["content", "output"]
                    .iter()
                    .any(|field| object.get(*field).is_some_and(contains_image))
            }
            _ => false,
        }
    }
    body.get(if protocol == Protocol::Responses {
        "input"
    } else {
        "messages"
    })
    .is_some_and(contains_image)
}

#[cfg(test)]
mod modern_cli_tests {
    use super::*;
    #[test]
    fn disabled_codex_reasoning_and_advisory_hints_convert_without_claiming_cache_or_opaque_state()
    {
        let body = json!({"model":"M","input":"hello","store":false,"reasoning":{},"include":["reasoning.encrypted_content"],"client_metadata":{"session_id":"private-tracing-id"},"prompt_cache_key":"private-cache-hint"});
        let result = convert_request(Protocol::Responses, Protocol::Chat, &body).unwrap();
        assert_eq!(result["messages"][0]["content"], "hello");
        for field in [
            "client_metadata",
            "prompt_cache_key",
            "reasoning",
            "include",
        ] {
            assert!(result.get(field).is_none());
        }
        let notes = normalization_notes(Protocol::Responses, Protocol::Chat, &body).join(";");
        assert!(notes.contains("cache") && notes.contains("opaque"));
        assert!(!notes.contains("private-tracing-id"));
        assert!(!notes.contains("private-cache-hint"));
        assert_eq!(
            convert_request(Protocol::Responses, Protocol::Responses, &body).unwrap(),
            body
        );
    }
    #[test]
    fn active_reasoning_opaque_inputs_and_nonadvisory_include_remain_explicitly_rejected() {
        for reasoning in [
            json!({"effort":"low"}),
            json!({"effort":"high"}),
            json!({"summary":"auto"}),
            json!({"summary":"detailed"}),
            json!({"unknown":false}),
            json!(true),
        ] {
            let body = json!({"model":"M","input":"hello","reasoning":reasoning,"include":["reasoning.encrypted_content"]});
            assert!(
                convert_request(Protocol::Responses, Protocol::Chat, &body).is_err(),
                "{body}"
            );
        }
        for include in [
            json!(["message.output_text.logprobs"]),
            json!(["reasoning.encrypted_content", "reasoning.encrypted_content"]),
            json!(true),
        ] {
            assert!(convert_request(
                Protocol::Responses,
                Protocol::Chat,
                &json!({"model":"M","input":"hi","reasoning":{},"include":include})
            )
            .is_err());
        }
        assert!(convert_request(Protocol::Responses,Protocol::Chat,&json!({"model":"M","input":[{"type":"reasoning","encrypted_content":"opaque"}],"reasoning":{}})).is_err());
        assert!(convert_request(
            Protocol::Responses,
            Protocol::Chat,
            &json!({"model":"M","input":"hi","include":["reasoning.encrypted_content"]})
        )
        .is_err());
    }
    #[test]
    fn claude_plain_text_and_disabled_effort_convert_but_real_effort_or_format_do_not() {
        for config in [
            json!({}),
            json!({"effort":"none"}),
            json!({"format":{"type":"text"}}),
        ] {
            let body = json!({"model":"M","max_tokens":16,"messages":[{"role":"user","content":"hello"}],"output_config":config});
            let converted = convert_request(Protocol::Messages, Protocol::Chat, &body).unwrap();
            assert!(converted.get("output_config").is_none());
        }
        for config in [
            json!({"effort":"high"}),
            json!({"effort":"low"}),
            json!({"format":{"type":"json_schema","schema":{}}}),
            json!({"task_budget":10}),
            json!(false),
        ] {
            assert!(convert_request(Protocol::Messages,Protocol::Chat,&json!({"model":"M","max_tokens":16,"messages":[{"role":"user","content":"hi"}],"output_config":config})).is_err());
        }
    }
}

/// Advisory fields are intentionally normalized only for cross-protocol requests.
/// Empty/disabled reasoning never establishes support for provider reasoning state.
fn validate_cli_hints(from: Protocol, body: &Value) -> Result<(), String> {
    if from == Protocol::Responses {
        if let Some(value) = body.get("client_metadata") {
            object(value)?;
        }
        if let Some(value) = body.get("prompt_cache_key") {
            if value
                .as_str()
                .is_none_or(|s| s.len() > 512 || s.chars().any(char::is_control))
            {
                return Err("prompt_cache_key must be an advisory string up to 512 bytes".into());
            }
        }
        let mut explicitly_disabled = false;
        if let Some(value) = body.get("reasoning") {
            fields(value, &["effort", "summary"])?;
            for parameter in ["effort", "summary"] {
                if value
                    .get(parameter)
                    .is_some_and(|v| v.as_str() != Some("none"))
                {
                    return Err("Requested reasoning effort or summary cannot be converted; disable reasoning and summary for text/function conversion".into());
                }
            }
            explicitly_disabled = true;
        }
        if let Some(include) = body.get("include") {
            let items = include.as_array().ok_or("include must be an array")?;
            if !items.is_empty()
                && (items.len() != 1
                    || items[0] != "reasoning.encrypted_content"
                    || !explicitly_disabled)
            {
                return Err("Opaque reasoning include requires explicitly empty/disabled reasoning; other includes cannot be converted".into());
            }
        }
    }
    if from == Protocol::Messages {
        if let Some(value) = body.get("output_config") {
            fields(value, &["effort", "format"])?;
            if value
                .get("effort")
                .is_some_and(|v| !v.is_null() && v.as_str() != Some("none"))
            {
                return Err("Requested Messages effort cannot be converted; disable effort in the client model capabilities".into());
            }
            if let Some(format) = value.get("format").filter(|v| !v.is_null()) {
                fields(format, &["type"])?;
                if format["type"] != "text" {
                    return Err("Only plain-text output_config.format can be converted".into());
                }
            }
        }
    }
    Ok(())
}

/// Fixed, secret-free notes; gateway records these after successful conversion.
pub fn normalization_notes(from: Protocol, to: Protocol, body: &Value) -> Vec<&'static str> {
    if from == to {
        return vec![];
    }
    let mut notes = vec![];
    if to == Protocol::Chat && body["stream"] == true {
        notes.push(
            "converted Chat stream requests source usage; missing source meters remain unknown",
        );
    }
    if from == Protocol::Responses {
        if body.get("client_metadata").is_some() {
            notes.push("client_metadata tracing omitted; values not forwarded or retained");
        }
        if body.get("prompt_cache_key").is_some() {
            notes.push(
                "prompt_cache_key hint omitted; gateway does not implement or claim cache affinity",
            );
        }
        if body.get("reasoning").is_some() {
            notes.push("empty/disabled reasoning normalized to text/function mode; opaque reasoning is unsupported");
        }
        if body.get("include").is_some() {
            notes.push("empty include or disabled-reasoning encrypted include normalized; no opaque output is promised");
        }
    }
    if from == Protocol::Messages && body.get("output_config").is_some() {
        notes.push("plain-text/disabled-effort output_config normalized; active reasoning effort and structured formats are unsupported");
    }
    notes
}
