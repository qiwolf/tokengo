//! Incremental SSE framing, native observation, and explicit text/function conversion.
//! Native callers forward their original bytes, not Frame::encode().
//! Messages target usage uses required wire-level zero placeholders when source
//! usage is unknown. Billing MUST observe the source independently; placeholders
//! never change Observer's Option values or establish known financial usage.
use super::Protocol;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
const MAX_FRAME: usize = 1024 * 1024;
const MAX_CONTENT: usize = 16 * MAX_FRAME;
const MAX_BLOCKS: usize = 128;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub event: Option<String>,
    pub data: String,
}
impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = String::new();
        if let Some(event) = &self.event {
            out.push_str("event: ");
            out.extend(event.chars().filter(|c| *c != '\r' && *c != '\n'));
            out.push('\n');
        }
        for line in self.data.split('\n') {
            out.push_str("data: ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
        out.into_bytes()
    }
}
#[derive(Default)]
pub(super) struct Decoder {
    line: Vec<u8>,
    data: Vec<String>,
    event: Option<String>,
    has_data: bool,
    frame_bytes: usize,
    skip_lf: bool,
    finished: bool,
}
impl Decoder {
    fn dispatch(&mut self, out: &mut Vec<Frame>) {
        if self.has_data {
            out.push(Frame {
                event: self.event.take(),
                data: self.data.join("\n"),
            });
        }
        self.data.clear();
        self.event = None;
        self.has_data = false;
        self.frame_bytes = 0;
    }
    fn line(&mut self, out: &mut Vec<Frame>) -> Result<(), String> {
        let bytes = std::mem::take(&mut self.line);
        let text = std::str::from_utf8(&bytes).map_err(|_| "Invalid UTF-8 in SSE frame")?;
        if text.is_empty() {
            self.dispatch(out);
            return Ok(());
        }
        if text.starts_with(':') {
            return Ok(());
        }
        let (field, value) = text.split_once(':').unwrap_or((text, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                self.has_data = true;
                self.data.push(value.to_owned());
            }
            "event" => self.event = Some(value.to_owned()),
            _ => {}
        }
        Ok(())
    }
    pub(super) fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Frame>, String> {
        if self.finished {
            return Err("SSE already finished".into());
        }
        let mut out = Vec::new();
        for &b in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    continue;
                }
            }
            // Count CRLF together at CR; bare CR is conservatively counted twice.
            self.frame_bytes += if b == b'\r' { 2 } else { 1 };
            if self.frame_bytes > MAX_FRAME {
                return Err("SSE frame exceeds 1 MiB".into());
            }
            match b {
                b'\r' => {
                    self.line(&mut out)?;
                    self.skip_lf = true;
                }
                b'\n' => self.line(&mut out)?,
                _ => self.line.push(b),
            }
        }
        Ok(out)
    }
    pub(super) fn finish(&mut self) -> Result<Vec<Frame>, String> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        let mut out = Vec::new();
        if !self.line.is_empty() {
            self.line(&mut out)?;
        }
        self.dispatch(&mut out);
        Ok(out)
    }
}
fn merge_usage(input: &mut Option<i64>, output: &mut Option<i64>, value: &Value) {
    for (slot, names) in [
        (input, ["prompt_tokens", "input_tokens"]),
        (output, ["completion_tokens", "output_tokens"]),
    ] {
        if let Some(n) = names
            .iter()
            .find_map(|k| value.get(*k).and_then(Value::as_i64).filter(|n| *n >= 0))
        {
            *slot = Some(n);
        }
    }
}
fn nonempty(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.trim().is_empty())
}
fn payload(v: &Value) -> bool {
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "function_call" | "tool_use" => true,
        "message" => v["content"]
            .as_array()
            .is_some_and(|a| a.iter().any(payload)),
        "text" | "output_text" => nonempty(&v["text"]),
        "refusal" => nonempty(&v["refusal"]),
        "reasoning" => {
            v["summary"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| nonempty(&v["text"])))
                || nonempty(&v["encrypted_content"])
                || v["content"].as_array().is_some_and(|a| a.iter().any(|p| p["type"] == "reasoning_text" && nonempty(&p["text"])))
        }
        _ => false,
    }
}
fn kind<'a>(frame: &'a Frame, v: &'a Value) -> &'a str {
    v.get("type")
        .and_then(Value::as_str)
        .or(frame.event.as_deref())
        .unwrap_or("")
}
/// The known namespace is opaque presentation telemetry in Codex 0.149.
/// Its contents cannot establish output, usage, terminal state, or control actions.
fn moderation_telemetry(frame: &Frame, value: &Value) -> Result<(), String> {
    let envelope = value
        .as_object()
        .ok_or("Invalid Responses metadata envelope")?;
    if value["type"] != "response.metadata"
        || frame
            .event
            .as_deref()
            .is_some_and(|event| event != "response.metadata")
        || envelope.keys().any(|key| {
            !matches!(
                key.as_str(),
                "type" | "response_id" | "sequence_number" | "metadata"
            )
        })
    {
        return Err("Unsupported Responses metadata envelope or control fields".into());
    }
    if let Some(id) = envelope.get("response_id").filter(|v| !v.is_null()) {
        if !id.as_str().is_some_and(|id| {
            !id.trim().is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
        }) {
            return Err("Invalid Responses metadata response identity".into());
        }
    }
    if envelope
        .get("sequence_number")
        .filter(|v| !v.is_null())
        .is_some_and(|v| !v.as_u64().is_some_and(|n| n <= i64::MAX as u64))
    {
        return Err("Invalid Responses metadata sequence number".into());
    }
    let metadata = value["metadata"]
        .as_object()
        .ok_or("Invalid Responses metadata namespace")?;
    if metadata.len() != 1 || !metadata.contains_key("openai_chatgpt_moderation_metadata") {
        return Err("Unsupported Responses metadata control or unknown namespace".into());
    }
    let opaque = &metadata["openai_chatgpt_moderation_metadata"];
    if serde_json::to_vec(opaque)
        .map_err(|_| "Invalid moderation telemetry")?
        .len()
        > 64 * 1024
    {
        return Err("Moderation telemetry exceeds 64 KiB".into());
    }
    fn bounded(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), String> {
        *nodes += 1;
        if depth > 16 || *nodes > 4096 {
            return Err("Moderation telemetry exceeds depth or node bound".into());
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    bounded(item, depth + 1, nodes)?;
                }
            }
            Value::Object(items) => {
                for item in items.values() {
                    bounded(item, depth + 1, nodes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    bounded(opaque, 0, &mut 0)
}
pub struct Observer {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub meaningful: bool,
    pub done: bool,
    pub failed: Option<String>,
    pub saw_output: bool,
    protocol: Protocol,
    decoder: Decoder,
    choices: BTreeMap<u64, bool>,
    message_usage: [Option<i64>; 3],
}
impl Observer {
    pub fn new(protocol: Protocol) -> Self {
        Self {
            input: None,
            output: None,
            meaningful: false,
            done: false,
            failed: None,
            saw_output: false,
            protocol,
            decoder: Decoder::default(),
            choices: BTreeMap::new(),
            message_usage: [None; 3],
        }
    }
    fn observe(&mut self, frame: &Frame) -> Result<(), String> {
        if frame.data.trim().is_empty() {
            return Ok(());
        }
        if frame.data.trim() == "[DONE]" {
            if self.protocol != Protocol::Chat {
                return Err("Unexpected DONE for this protocol".into());
            }
            self.done = true;
            return Ok(());
        }
        let v: Value = serde_json::from_str(&frame.data).map_err(|_| "Invalid SSE JSON")?;
        if !v.is_object() {
            return Err("SSE JSON must be an object".into());
        }
        if self.protocol != Protocol::Messages {
            merge_usage(&mut self.input, &mut self.output, &v["usage"]);
        }
        match self.protocol {
            Protocol::Chat => {
                if v.get("error").is_some_and(|e| !e.is_null()) {
                    return Err("Upstream Chat stream error".into());
                }
                if let Some(choices) = v["choices"].as_array() {
                    for c in choices {
                        let index = c["index"].as_u64().unwrap_or(0);
                        if self.choices.len() >= MAX_BLOCKS && !self.choices.contains_key(&index) {
                            return Err("Too many Chat choices".into());
                        }
                        let ended = nonempty(&c["finish_reason"]);
                        self.choices
                            .entry(index)
                            .and_modify(|v| *v |= ended)
                            .or_insert(ended);
                        if c.get("delta").is_some() || c.get("message").is_some() {
                            self.saw_output = true;
                        }
                        let d = c
                            .get("delta")
                            .or_else(|| c.get("message"))
                            .unwrap_or(&Value::Null);
                        self.meaningful |= ["content", "reasoning_content", "reasoning", "refusal"]
                            .iter()
                            .any(|k| nonempty(&d[*k]))
                            || d["tool_calls"].as_array().is_some_and(|a| !a.is_empty())
                            || d["function_call"]
                                .as_object()
                                .is_some_and(|m| !m.is_empty())
                            || d["reasoning_details"]
                                .as_array()
                                .is_some_and(|a| !a.is_empty());
                    }
                }
            }
            Protocol::Responses => {
                merge_usage(&mut self.input, &mut self.output, &v["response"]["usage"]);
                let event = kind(frame, &v);
                self.saw_output |= event.starts_with("response.");
                self.meaningful |= matches!(
                    event,
                    "response.output_text.delta"
                        | "response.refusal.delta"
                        | "response.reasoning_text.delta"
                        | "response.reasoning_summary_text.delta"
                        | "response.function_call_arguments.delta"
                ) && nonempty(&v["delta"]);
                self.meaningful |= v.get("item").is_some_and(payload);
                self.meaningful |= v["response"]["output"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(payload));
                let output_limited = event == "response.incomplete"
                    && v["response"]["status"] == "incomplete"
                    && v["response"]["incomplete_details"]["reason"] == "max_output_tokens"
                    && v["response"].get("error").is_none_or(Value::is_null);
                if matches!(event, "error" | "response.failed")
                    || ((event == "response.incomplete" || v["response"]["status"] == "incomplete") && !output_limited)
                    || v["response"]["status"] == "failed"
                    || v.get("error").is_some_and(|e| !e.is_null())
                {
                    return Err(format!("Upstream Responses stream {event}"));
                }
                if event == "response.completed" || v["response"]["status"] == "completed" || output_limited {
                    self.done = true;
                }
            }
            Protocol::Messages => {
                for usage in [&v["message"]["usage"], &v["usage"]] {
                    for (i, key) in [
                        "input_tokens",
                        "cache_read_input_tokens",
                        "cache_creation_input_tokens",
                    ]
                    .iter()
                    .enumerate()
                    {
                        if let Some(value) = usage.get(*key) {
                            let n = value
                                .as_i64()
                                .filter(|n| *n >= 0)
                                .ok_or("Invalid Messages input usage")?;
                            self.message_usage[i] = Some(self.message_usage[i].unwrap_or(0).max(n));
                        }
                    }
                    if let Some(value) = usage.get("output_tokens") {
                        let n = value
                            .as_i64()
                            .filter(|n| *n >= 0)
                            .ok_or("Invalid Messages output usage")?;
                        self.output = Some(self.output.unwrap_or(0).max(n));
                    }
                }
                if self.message_usage[0].is_some() {
                    self.input = Some(
                        self.message_usage
                            .iter()
                            .try_fold(0i64, |sum, n| sum.checked_add(n.unwrap_or(0)))
                            .ok_or("Messages input usage overflow")?,
                    );
                }
                let event = kind(frame, &v);
                self.saw_output |= event != "ping";
                self.meaningful |= v.get("content_block").is_some_and(payload);
                if event == "content_block_delta" {
                    self.meaningful |= ["text", "partial_json", "thinking", "signature"]
                        .iter()
                        .any(|k| nonempty(&v["delta"][*k]));
                }
                if event == "error" || v.get("error").is_some_and(|e| !e.is_null()) {
                    return Err("Upstream Messages stream error".into());
                }
                if event == "message_stop" {
                    self.done = true;
                }
            }
        }
        Ok(())
    }
    fn record_failure<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        if let Err(e) = &result {
            self.failed = Some(e.clone());
        }
        result
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, String> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        let parsed = self.decoder.feed(bytes);
        let frames = self.record_failure(parsed)?;
        for f in &frames {
            let result = self.observe(f);
            self.record_failure(result)?;
        }
        Ok(frames)
    }
    fn end_frames(&mut self) -> Result<Vec<Frame>, String> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        let parsed = self.decoder.finish();
        let frames = self.record_failure(parsed)?;
        for f in &frames {
            let result = self.observe(f);
            self.record_failure(result)?;
        }
        // EOF may confirm all finished Chat choices, but finish_reason alone
        // never finalizes feed(): a later usage-only chunk must still be read.
        if self.protocol == Protocol::Chat
            && !self.choices.is_empty()
            && self.choices.values().all(|v| *v)
        {
            self.done = true;
        }
        if !self.done {
            let error = "SSE ended without a terminal event".to_owned();
            self.failed = Some(error.clone());
            return Err(error);
        }
        Ok(frames)
    }
    pub fn finish_frames(&mut self)->Result<Vec<Frame>,String> {self.end_frames()}
    pub fn finish(&mut self) -> Result<(), String> {
        self.end_frames().map(|_| ())
    }
}
#[derive(Debug)]
struct Tool {
    id: String,
    name: String,
    args: String,
    index: Option<u32>,
    ordinal: u32,
    item_id: String,
    started: bool,
}
pub struct Transcoder {
    pub observer: Observer,
    pub saw_output: bool,
    from: Protocol,
    to: Protocol,
    started: bool,
    ended: bool,
    failed: Option<String>,
    id: String,
    model: String,
    text: String,
    text_index: Option<u32>,
    reasoning: String,
    reasoning_tokens: Option<u64>,
    usage_metadata: super::response::UsageMetadata,
    request_id: Option<String>,
    reasoning_index: Option<u32>,
    reasoning_item: String,
    reasoning_closed: bool,
    source_reasoning: Option<(u64, String)>,
    source_response_id: Option<String>,
    sparse_proof: super::sparse::SparseProof,
    message_item: String,
    next_index: u32,
    next_tool: u32,
    sequence: u64,
    total: usize,
    tools: BTreeMap<String, Tool>,
    item_keys: BTreeMap<String, String>,
    source_text: BTreeMap<(u64, u64), String>,
    closed: BTreeSet<String>,
    stop: String,
}
impl Transcoder {
    pub fn new(from: Protocol, to: Protocol) -> Self {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        Self {
            observer: Observer::new(from),
            saw_output: false,
            from,
            to,
            started: false,
            ended: false,
            failed: None,
            id: format!(
                "{}_{unique}",
                match to {
                    Protocol::Chat => "chatcmpl",
                    Protocol::Responses => "resp",
                    Protocol::Messages => "msg",
                }
            ),
            model: "tokengo".into(),
            text: String::new(),
            text_index: None,
            reasoning: String::new(),
            reasoning_tokens: None,
            usage_metadata: super::response::UsageMetadata::default(),
            request_id: None,
            reasoning_index: None,
            reasoning_item: format!("rs_{unique}"),
            reasoning_closed: false,
            source_reasoning: None,
            source_response_id: None,
            sparse_proof: super::sparse::SparseProof::default(),
            message_item: format!("msg_{unique}"),
            next_index: 0,
            next_tool: 0,
            sequence: 0,
            total: 0,
            tools: BTreeMap::new(),
            item_keys: BTreeMap::new(),
            source_text: BTreeMap::new(),
            closed: BTreeSet::new(),
            stop: "stop".into(),
        }
    }
    fn emit(&mut self, out: &mut Vec<u8>, event: Option<&str>, mut value: Value) {
        if let Some(trace) = &self.request_id {
            match self.to {
                Protocol::Chat => value["request_id"] = json!(trace),
                Protocol::Responses if value.get("response").is_some() => value["response"]["request_id"] = json!(trace),
                _ => {}
            }
        }
        if self.to == Protocol::Responses {
            value["sequence_number"] = json!(self.sequence);
            self.sequence += 1;
        }
        out.extend(
            Frame {
                event: event.map(str::to_owned),
                data: value.to_string(),
            }
            .encode(),
        );
        self.saw_output = true;
    }
    fn usage(&self, chat: bool) -> Value {
        let mut u = serde_json::Map::new();
        if let Some(n) = self.observer.input {
            u.insert(
                if chat {
                    "prompt_tokens"
                } else {
                    "input_tokens"
                }
                .into(),
                json!(n),
            );
        }
        if let Some(n) = self.observer.output {
            u.insert(
                if chat {
                    "completion_tokens"
                } else {
                    "output_tokens"
                }
                .into(),
                json!(n),
            );
        }
        if let (Some(i), Some(o)) = (self.observer.input, self.observer.output) {
            if let Some(n) = i.checked_add(o) {
                u.insert("total_tokens".into(), json!(n));
            }
        }
        if let Some(n)=self.reasoning_tokens {
            u.insert(if chat {"completion_tokens_details"} else {"output_tokens_details"}.into(), json!({"reasoning_tokens":n}));
        }
        let mut usage = Value::Object(u);
        if matches!((self.from,self.to),(Protocol::Chat,Protocol::Responses)|(Protocol::Responses,Protocol::Chat)) {
            self.usage_metadata.apply(if chat { Protocol::Chat } else { Protocol::Responses }, &mut usage);
        }
        usage
    }
    fn messages_wire_usage(&self) -> Value {
        json!({"input_tokens":self.observer.input.unwrap_or(0),"output_tokens":self.observer.output.unwrap_or(0)})
    }
    fn response(&self, status: &str, output: Vec<Value>) -> Value {
        let mut v = json!({"id":self.id,"object":"response","created_at":crate::db::now_ms()/1000,"model":self.model,"status":status,"error":null,"incomplete_details":null,"output":output});
        if status != "in_progress" {
            v["usage"] = self.usage(false);
            if status == "incomplete" {
                v["incomplete_details"] = json!({"reason":"max_output_tokens"});
            }
        }
        v
    }
    fn begin(&mut self, out: &mut Vec<u8>) {
        if self.started {
            return;
        }
        self.started = true;
        match self.to {
            Protocol::Chat=>self.emit(out,None,json!({"id":self.id,"object":"chat.completion.chunk","created":crate::db::now_ms()/1000,"model":self.model,"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]})),
            Protocol::Responses=>{let response=self.response("in_progress",vec![]);self.emit(out,Some("response.created"),json!({"type":"response.created","response":response}));let response=self.response("in_progress",vec![]);self.emit(out,Some("response.in_progress"),json!({"type":"response.in_progress","response":response}));},
            Protocol::Messages=>self.emit(out,Some("message_start"),json!({"type":"message_start","message":{"id":self.id,"type":"message","role":"assistant","model":self.model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":self.messages_wire_usage()}})),
        }
    }
    fn add_size(&mut self, n: usize) -> Result<(), String> {
        self.total = self
            .total
            .checked_add(n)
            .ok_or("Converted content overflow")?;
        if self.total > MAX_CONTENT {
            return Err("Converted response exceeds 16 MiB".into());
        }
        Ok(())
    }
    fn reasoning(&mut self, delta: &str, out: &mut Vec<u8>) -> Result<(), String> {
        if delta.is_empty() {
            return Ok(());
        }
        if !matches!(
            (self.from, self.to),
            (Protocol::Chat, Protocol::Responses) | (Protocol::Responses, Protocol::Chat)
        ) {
            return Err("Plain reasoning conversion requires Chat/Responses".into());
        }
        if self.reasoning_closed || self.text_index.is_some() || !self.tools.is_empty() {
            return Err("Reasoning after text/tools is not representable".into());
        }
        self.add_size(delta.len())?;
        self.begin(out);
        if self.reasoning_index.is_none() {
            let index = self.next_index;
            self.next_index += 1;
            self.reasoning_index = Some(index);
            if self.to == Protocol::Responses {
                self.emit(out,Some("response.output_item.added"),json!({"type":"response.output_item.added","output_index":index,"item":{"id":self.reasoning_item,"type":"reasoning","status":"in_progress","summary":[],"content":[]}}));
            }
        }
        self.reasoning.push_str(delta);
        if self.to == Protocol::Responses {
            self.emit(out,Some("response.reasoning_text.delta"),json!({"type":"response.reasoning_text.delta","item_id":self.reasoning_item,"output_index":self.reasoning_index.unwrap(),"content_index":0,"delta":delta}));
        } else {
            self.emit(out,None,json!({"id":self.id,"object":"chat.completion.chunk","model":self.model,"choices":[{"index":0,"delta":{"reasoning_content":delta},"finish_reason":null}]}));
        }
        Ok(())
    }
    fn reasoning_item(&self) -> Value {
        json!({"id":self.reasoning_item,"type":"reasoning","status":"completed","summary":[],"content":[{"type":"reasoning_text","text":self.reasoning}]})
    }
    fn close_reasoning(&mut self, out: &mut Vec<u8>) {
        if self.reasoning_closed {
            return;
        }
        self.reasoning_closed = true;
        if self.reasoning_index.is_none() {
            return;
        }
        if self.to == Protocol::Responses {
            let index = self.reasoning_index.unwrap();
            self.emit(out,Some("response.reasoning_text.done"),json!({"type":"response.reasoning_text.done","item_id":self.reasoning_item,"output_index":index,"content_index":0,"text":self.reasoning}));
            self.emit(out,Some("response.output_item.done"),json!({"type":"response.output_item.done","output_index":index,"item":self.reasoning_item()}));
        }
    }
    fn response_reasoning(
        &mut self,
        index: u64,
        id: &str,
        text: &str,
        complete: bool,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        if let Some((old_index, old_id)) = &self.source_reasoning {
            if *old_index != index || old_id != id {
                return Err("Multiple or changed reasoning identities".into());
            }
        } else {
            self.source_reasoning = Some((index, id.into()));
        }
        let delta = if complete {
            text.strip_prefix(&self.reasoning)
                .ok_or("Final reasoning disagrees with deltas")?
        } else {
            text
        }
        .to_owned();
        self.reasoning(&delta, out)
    }
    fn text(&mut self, delta: &str, out: &mut Vec<u8>) -> Result<(), String> {
        if delta.is_empty() {
            return Ok(());
        }
        if self.to == Protocol::Messages && !self.tools.is_empty() {
            return Err(
                "Text after tool blocks cannot be converted to Messages without reordering".into(),
            );
        }
        self.close_reasoning(out);
        self.add_size(delta.len())?;
        self.begin(out);
        if self.text_index.is_none() {
            let i = self.next_index;
            self.next_index += 1;
            self.text_index = Some(i);
            match self.to {
                Protocol::Chat=>{},
                Protocol::Responses=>{self.emit(out,Some("response.output_item.added"),json!({"type":"response.output_item.added","output_index":i,"item":{"id":self.message_item,"type":"message","role":"assistant","status":"in_progress","content":[]}}));self.emit(out,Some("response.content_part.added"),json!({"type":"response.content_part.added","item_id":self.message_item,"output_index":i,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}));},
                Protocol::Messages=>self.emit(out,Some("content_block_start"),json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}})),
            }
        }
        self.text.push_str(delta);
        let index = self.text_index.unwrap();
        match self.to {
            Protocol::Chat=>self.emit(out,None,json!({"id":self.id,"object":"chat.completion.chunk","created":crate::db::now_ms()/1000,"model":self.model,"choices":[{"index":0,"delta":{"content":delta},"finish_reason":null}]})),
            Protocol::Responses=>self.emit(out,Some("response.output_text.delta"),json!({"type":"response.output_text.delta","item_id":self.message_item,"output_index":index,"content_index":0,"delta":delta})),
            Protocol::Messages=>self.emit(out,Some("content_block_delta"),json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":delta}})),
        }
        Ok(())
    }
    fn tool(
        &mut self,
        key: &str,
        id: Option<&str>,
        name: Option<&str>,
        args: &str,
        start: bool,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        self.close_reasoning(out);
        let mut tool = if let Some(t) = self.tools.remove(key) {
            t
        } else {
            if self.tools.len() >= MAX_BLOCKS {
                return Err("Too many converted tool calls".into());
            }
            let ordinal = self.next_tool;
            self.next_tool += 1;
            Tool {
                id: String::new(),
                name: String::new(),
                args: String::new(),
                index: None,
                ordinal,
                item_id: format!("fc_{}", uuid::Uuid::new_v4().simple()),
                started: false,
            }
        };
        for (target, part) in [(&mut tool.id, id), (&mut tool.name, name)] {
            if let Some(part) = part {
                if target.is_empty() {
                    target.push_str(part);
                } else if target != part {
                    if tool.started {
                        return Err("Tool identity changed after its first delta".into());
                    }
                    target.push_str(part);
                }
            }
        }
        self.add_size(args.len())?;
        let old_len = tool.args.len();
        tool.args.push_str(args);
        // Messages blocks are sequential. Buffer parallel function deltas until
        // complete, then emit each tool as a closed block at finalization.
        if self.to == Protocol::Messages {
            self.tools.insert(key.to_owned(), tool);
            return Ok(());
        }
        let should_start =
            !tool.id.is_empty() && !tool.name.is_empty() && (start || !tool.args.is_empty());
        let delta = if !tool.started && should_start {
            self.begin(out);
            tool.started = true;
            let index = self.next_index;
            self.next_index += 1;
            tool.index = Some(index);
            match self.to {
                Protocol::Chat=>self.emit(out,None,json!({"id":self.id,"object":"chat.completion.chunk","created":crate::db::now_ms()/1000,"model":self.model,"choices":[{"index":0,"delta":{"tool_calls":[{"index":tool.ordinal,"id":tool.id,"type":"function","function":{"name":tool.name,"arguments":""}}]},"finish_reason":null}]})),
                Protocol::Responses=>self.emit(out,Some("response.output_item.added"),json!({"type":"response.output_item.added","output_index":index,"item":{"id":tool.item_id,"type":"function_call","status":"in_progress","call_id":tool.id,"name":tool.name,"arguments":""}})),
                Protocol::Messages=>self.emit(out,Some("content_block_start"),json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":tool.id,"name":tool.name,"input":{}}})),
            }
            tool.args.clone()
        } else if tool.started {
            tool.args[old_len..].to_owned()
        } else {
            String::new()
        };
        if !delta.is_empty() {
            let index = tool.index.unwrap();
            match self.to{
            Protocol::Chat=>self.emit(out,None,json!({"id":self.id,"object":"chat.completion.chunk","created":crate::db::now_ms()/1000,"model":self.model,"choices":[{"index":0,"delta":{"tool_calls":[{"index":tool.ordinal,"function":{"arguments":delta}}]},"finish_reason":null}]})),
            Protocol::Responses=>self.emit(out,Some("response.function_call_arguments.delta"),json!({"type":"response.function_call_arguments.delta","item_id":tool.item_id,"output_index":index,"delta":delta})),
            Protocol::Messages=>self.emit(out,Some("content_block_delta"),json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":delta}})),
        }
        }
        self.tools.insert(key.to_owned(), tool);
        Ok(())
    }
    fn complete_args(
        &mut self,
        key: &str,
        arguments: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        let previous = self
            .tools
            .get(key)
            .map(|t| t.args.clone())
            .unwrap_or_default();
        let Some(tail) = arguments.strip_prefix(&previous) else {
            return Err("Final tool arguments disagree with deltas".into());
        };
        self.tool(key, None, None, tail, true, out)
    }
    fn unsupported(&self, frame: &Frame, v: &Value) -> Result<(), String> {
        let event = kind(frame, v);
        match self.from {
            Protocol::Chat => {
                super::fields(v, &["id", "request_id", "object", "created", "model", "choices", "usage", "system_fingerprint", "service_tier", "timings"])?;
                if let Some(choices) = v["choices"].as_array() {
                    for c in choices {
                        super::fields(c, &["index", "delta", "finish_reason", "logprobs"])?;
                        if c.get("logprobs").is_some_and(|v| !v.is_null()) { return Err("Chat logprobs cannot be converted".into()); }
                        if c["index"].as_u64().unwrap_or(0) != 0 {
                            return Err("Multiple Chat choices cannot be converted".into());
                        }
                        let d = &c["delta"];
                        if !d.is_null() {
                            super::fields(d, &["role", "content", "tool_calls", "reasoning_content", "reasoning", "reasoning_details", "refusal", "audio", "annotations", "function_call"])?;
                        }
                        if let Some(r)=d.get("reasoning_content").filter(|v| !v.is_null()) {
                            if !r.is_string() || (self.to != Protocol::Responses && r != "") { return Err("Unsupported raw reasoning destination/type".into()); }
                        }
                        for k in [
                            "reasoning",
                            "reasoning_details",
                            "refusal",
                            "audio",
                            "annotations",
                            "function_call",
                        ] {
                            if d.get(k)
                                .is_some_and(|v| !v.is_null() && v != &json!("") && v != &json!([]))
                            {
                                return Err(format!("Unsupported Chat conversion field {k}"));
                            }
                        }
                        if d.get("content")
                            .is_some_and(|v| !v.is_null() && !v.is_string())
                        {
                            return Err(
                                "Non-text Chat content cannot be streamed across protocols".into(),
                            );
                        }
                    }
                }
            }
            Protocol::Responses => {
                if event == "response.metadata" {
                    return moderation_telemetry(frame, v);
                }
                if (event.contains("reasoning") && !matches!(event,"response.reasoning_text.delta"|"response.reasoning_text.done"))
                    || event.contains("refusal")
                    || event.contains("audio")
                    || event.contains("image")
                    || event.contains("web_search")
                    || event.contains("code_interpreter")
                {
                    return Err(format!("Unsupported Responses conversion event {event}"));
                }
                for item in v["response"]["output"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(v.get("item").into_iter())
                {
                    let t = item["type"].as_str().ok_or("Missing Responses item type")?;
                    if t == "reasoning" && self.to == Protocol::Chat {
                        let mut item=item.clone();
                        if event=="response.output_item.added" {
                            if item["status"]=="in_progress" { item.as_object_mut().unwrap().remove("status"); }
                            if item["content"].as_array().is_some_and(Vec::is_empty) { item["content"]=json!([{"type":"reasoning_text","text":""}]); }
                        }
                        super::request::plain_reasoning(&item)?;
                        continue;
                    }
                    if !matches!(t, "message" | "function_call") {
                        return Err(format!("Unsupported Responses output item {t}"));
                    }
                    for part in item["content"].as_array().into_iter().flatten() {
                        if part["type"] != "output_text"
                            || part["annotations"]
                                .as_array()
                                .is_some_and(|a| !a.is_empty())
                        {
                            return Err("Unsupported Responses content or annotations".into());
                        }
                    }
                }
                if let Some(t) = v["part"]["type"].as_str() {
                    if t != "output_text" {
                        return Err("Unsupported Responses content part".into());
                    }
                }
                if let Some(t) = v["item"]["type"].as_str() {
                    if !matches!(t, "message" | "function_call" | "reasoning") {
                        return Err(format!("Unsupported Responses output item {t}"));
                    }
                }
            }
            Protocol::Messages => {
                if v["message"]["content"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
                {
                    return Err("Nonempty Messages initial content cannot be converted".into());
                }
                for usage in [&v["message"]["usage"], &v["usage"]] {
                    for key in ["cache_read_input_tokens", "cache_creation_input_tokens"] {
                        if usage[key].as_i64().is_some_and(|n| n > 0) {
                            return Err("Messages cache usage buckets cannot be converted".into());
                        }
                    }
                }
                if let Some(t) = v["content_block"]["type"].as_str() {
                    if t == "tool_use" && !v["content_block"]["input"].is_object() {
                        return Err("Messages tool input must be an object".into());
                    }
                    if !matches!(t, "text" | "tool_use") {
                        return Err(format!("Unsupported Messages block {t}"));
                    }
                }
                if let Some(t) = v["delta"]["type"].as_str() {
                    if event == "content_block_delta"
                        && !matches!(t, "text_delta" | "input_json_delta")
                    {
                        return Err(format!("Unsupported Messages delta {t}"));
                    }
                }
            }
        }
        Ok(())
    }
    fn response_text(
        &mut self,
        index: u64,
        content: u64,
        value: &str,
        complete: bool,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        let key = (index, content);
        if !self.source_text.contains_key(&key) && self.source_text.len() >= MAX_BLOCKS {
            return Err("Too many text parts".into());
        }
        let previous = self.source_text.entry(key).or_default();
        let delta = if complete {
            value
                .strip_prefix(previous.as_str())
                .ok_or("Final text part disagrees with deltas")?
        } else {
            value
        }
        .to_owned();
        previous.push_str(&delta);
        self.text(&delta, out)
    }
    fn response_snapshot(&mut self, response: &Value, out: &mut Vec<u8>) -> Result<(), String> {
        let items = response["output"].as_array().ok_or("Responses terminal output must be an array")?;
        if items.is_empty() {
            return self.sparse_proof.finish(response, &self.text);
        }
        let mut full_text = String::new();
        for (i, item) in items.iter().enumerate() {
            match item["type"].as_str() {
                Some("message") => {
                    if let Some(parts) = item["content"].as_array() {
                        for part in parts {
                            if part["type"] != "output_text" {
                                return Err("Unsupported Responses snapshot content".into());
                            }
                            full_text
                                .push_str(part["text"].as_str().ok_or("Missing snapshot text")?);
                        }
                    }
                }
                Some("reasoning") => {
                    let raw=super::request::plain_reasoning(item)?;
                    self.response_reasoning(i as u64,item["id"].as_str().ok_or("Missing reasoning id")?,raw,true,out)?;
                }
                Some("function_call") => {
                    let key = format!("r:{i}");
                    if !self.tools.contains_key(&key) {
                        self.tool(
                            &key,
                            item["call_id"].as_str(),
                            item["name"].as_str(),
                            "",
                            true,
                            out,
                        )?;
                    }
                    self.complete_args(
                        &key,
                        item["arguments"]
                            .as_str()
                            .ok_or("Missing snapshot tool arguments")?,
                        out,
                    )?;
                }
                _ => return Err("Unsupported Responses snapshot output".into()),
            }
        }
        let tail = full_text
            .strip_prefix(&self.text)
            .ok_or("Final Responses text disagrees with deltas")?
            .to_owned();
        self.text(&tail, out)
    }
    fn convert(&mut self, frame: &Frame, out: &mut Vec<u8>) -> Result<(), String> {
        if frame.data.trim().is_empty() || frame.data.trim() == "[DONE]" {
            return Ok(());
        }
        let v: Value = serde_json::from_str(&frame.data).map_err(|_| "Invalid SSE JSON")?;
        let payload = if self.from == Protocol::Responses { &v["response"] } else { &v };
        if let Some(trace) = super::response::request_id(payload)? {
            if self.to == Protocol::Messages { return Err("request_id metadata cannot be represented by Messages".into()); }
            if self.request_id.as_ref().is_some_and(|old| old != &trace) {
                return Err("Conflicting request_id metadata in stream".into());
            }
            self.request_id = Some(trace);
        }
        if matches!(self.from, Protocol::Chat | Protocol::Responses) {
            self.usage_metadata.observe(self.from, &payload["usage"], self.observer.input.and_then(|n| u64::try_from(n).ok()))?;
            if self.to == Protocol::Messages && self.usage_metadata.has_cache() {
                return Err("Chat/Responses cache subsets cannot be represented by Messages".into());
            }
        }
        if matches!((self.from,self.to),(Protocol::Chat,Protocol::Responses)|(Protocol::Responses,Protocol::Chat)) {
            for usage in [&v["usage"], &v["response"]["usage"]] {
                if let Some(n)=super::response::reasoning_tokens(self.from,usage)? {self.reasoning_tokens=Some(n);}
            }
            if self.reasoning_tokens.zip(self.observer.output).is_some_and(|(n,total)| n > total as u64) {
                return Err("Reasoning token count exceeds total output".into());
            }
        }
        let model = match self.from {
            Protocol::Chat => v["model"].as_str(),
            Protocol::Responses => v["response"]["model"].as_str(),
            Protocol::Messages => v["message"]["model"].as_str(),
        };
        if let Some(model) = model {
            if !self.started {
                self.model = model.to_owned();
            }
        }
        match self.from {
            Protocol::Chat => {
                if let Some(choices) = v["choices"].as_array() {
                    for c in choices {
                        let d = &c["delta"];
                        if let Some(reasoning)=d["reasoning_content"].as_str() { self.reasoning(reasoning,out)?; }
                        if let Some(text) = d["content"].as_str() {
                            self.text(text, out)?;
                        }
                        if let Some(calls) = d["tool_calls"].as_array() {
                            for call in calls {
                                if call
                                    .get("type")
                                    .and_then(Value::as_str)
                                    .is_some_and(|t| t != "function")
                                {
                                    return Err("Unsupported Chat tool type".into());
                                }
                                let index = call["index"]
                                    .as_u64()
                                    .ok_or("Tool delta index is required")?;
                                let key = format!("c:{index}");
                                self.tool(
                                    &key,
                                    call["id"].as_str(),
                                    call["function"]["name"].as_str(),
                                    call["function"]["arguments"].as_str().unwrap_or(""),
                                    false,
                                    out,
                                )?;
                            }
                        }
                        if let Some(reason) = c["finish_reason"].as_str() {
                            self.stop = reason.to_owned();
                        }
                    }
                }
            }
            Protocol::Responses => {
                let event = kind(frame, &v);
                self.sparse_proof.observe(event, &v);
                if self.source_response_id.is_none() {
                    self.source_response_id = v["response"]["id"].as_str().map(str::to_owned);
                }
                match event {
                    "response.metadata" => {
                        if let (Some(expected), Some(actual)) =
                            (&self.source_response_id, v["response_id"].as_str())
                        {
                            if expected != actual {
                                return Err("Responses moderation telemetry belongs to a different response".into());
                            }
                        }
                        // No destination representation, payload, meter, or terminal action.
                    }
                    "response.reasoning_text.delta" | "response.reasoning_text.done" => {
                        let complete=event.ends_with(".done");
                        if v["content_index"].as_u64()!=Some(0) { return Err("Only one raw reasoning part is supported".into()); }
                        let index=v["output_index"].as_u64().ok_or("Missing reasoning index")?;
                        let id=v["item_id"].as_str().ok_or("Missing reasoning identity")?;
                        if self.source_reasoning.as_ref()!=Some(&(index,id.to_owned())) { return Err("Reasoning delta precedes or changes item identity".into()); }
                        self.response_reasoning(index,id,v[if complete {"text"} else {"delta"}].as_str().ok_or("Missing raw reasoning text")?,complete,out)?;
                        if complete { self.close_reasoning(out); }
                    }
                    "response.output_item.added" => {
                        let item = &v["item"];
                        if item["type"]=="reasoning" {
                            let index=v["output_index"].as_u64().ok_or("Missing reasoning index")?;
                            let id=item["id"].as_str().ok_or("Missing reasoning identity")?;
                            let initial=item["content"].as_array().and_then(|a|a.first()).and_then(|p|p["text"].as_str()).unwrap_or("");
                            self.response_reasoning(index,id,initial,false,out)?;
                        }
                        if item["type"] == "function_call" {
                            let index = v["output_index"]
                                .as_u64()
                                .ok_or("Missing function output index")?;
                            let key = format!("r:{index}");
                            if let Some(id) = item["id"].as_str() {
                                self.item_keys.insert(id.into(), key.clone());
                            }
                            self.tool(
                                &key,
                                item["call_id"].as_str(),
                                item["name"].as_str(),
                                item["arguments"].as_str().unwrap_or(""),
                                true,
                                out,
                            )?;
                        }
                    }
                    "response.output_text.delta" | "response.output_text.done" => {
                        let complete = event.ends_with(".done");
                        self.response_text(
                            v["output_index"].as_u64().unwrap_or(0),
                            v["content_index"].as_u64().unwrap_or(0),
                            v[if complete { "text" } else { "delta" }]
                                .as_str()
                                .ok_or("Missing output text")?,
                            complete,
                            out,
                        )?;
                    }
                    "response.content_part.added" | "response.content_part.done" => {
                        if let Some(text) = v["part"]["text"].as_str() {
                            self.response_text(
                                v["output_index"].as_u64().unwrap_or(0),
                                v["content_index"].as_u64().unwrap_or(0),
                                text,
                                true,
                                out,
                            )?;
                        }
                    }
                    "response.function_call_arguments.delta"
                    | "response.function_call_arguments.done" => {
                        let key = v["item_id"]
                            .as_str()
                            .and_then(|id| self.item_keys.get(id))
                            .cloned()
                            .or_else(|| v["output_index"].as_u64().map(|i| format!("r:{i}")))
                            .ok_or("Missing tool item identity")?;
                        if !self.tools.contains_key(&key) {
                            return Err("Arguments arrived before function identity".into());
                        }
                        if event.ends_with(".done") {
                            self.complete_args(
                                &key,
                                v["arguments"]
                                    .as_str()
                                    .ok_or("Missing final tool arguments")?,
                                out,
                            )?;
                        } else {
                            self.tool(
                                &key,
                                None,
                                None,
                                v["delta"].as_str().ok_or("Missing argument delta")?,
                                true,
                                out,
                            )?;
                        }
                    }
                    "response.output_item.done" => {
                        let item = &v["item"];
                        if item["type"]=="reasoning" {
                            self.response_reasoning(v["output_index"].as_u64().ok_or("Missing reasoning index")?,item["id"].as_str().ok_or("Missing reasoning id")?,super::request::plain_reasoning(item)?,true,out)?;
                            self.close_reasoning(out);
                        }
                        if item["type"] == "message" {
                            for (i, part) in
                                item["content"].as_array().into_iter().flatten().enumerate()
                            {
                                if let Some(text) = part["text"].as_str() {
                                    self.response_text(
                                        v["output_index"].as_u64().unwrap_or(0),
                                        i as u64,
                                        text,
                                        true,
                                        out,
                                    )?;
                                }
                            }
                        }
                        if item["type"] == "function_call" {
                            let index = v["output_index"]
                                .as_u64()
                                .ok_or("Missing function output index")?;
                            let key = format!("r:{index}");
                            if !self.tools.contains_key(&key) {
                                self.tool(
                                    &key,
                                    item["call_id"].as_str(),
                                    item["name"].as_str(),
                                    "",
                                    true,
                                    out,
                                )?;
                            }
                            self.complete_args(
                                &key,
                                item["arguments"]
                                    .as_str()
                                    .ok_or("Missing completed tool arguments")?,
                                out,
                            )?;
                        }
                    }
                    "response.completed" => self.response_snapshot(&v["response"], out)?,
                    "response.incomplete" => {
                        // Observer accepts only a terminal max_output_tokens truncation.
                        self.stop = "length".into();
                        self.response_snapshot(&v["response"], out)?;
                    }
                    "response.created" | "response.in_progress" | "ping" => {}
                    "" => return Err("Responses stream has no event type".into()),
                    _ => return Err(format!("Unsupported Responses conversion event {event}")),
                }
            }
            Protocol::Messages => {
                let event = kind(frame, &v);
                let index = v["index"].as_u64().unwrap_or(0);
                let key = format!("m:{index}");
                match event {
                    "content_block_start" => {
                        let block = &v["content_block"];
                        match block["type"].as_str() {
                            Some("text") => {
                                if let Some(text) = block["text"].as_str() {
                                    self.text(text, out)?;
                                }
                            }
                            Some("tool_use") => {
                                let initial =
                                    if block["input"].as_object().is_some_and(|m| !m.is_empty()) {
                                        block["input"].to_string()
                                    } else {
                                        String::new()
                                    };
                                self.tool(
                                    &key,
                                    block["id"].as_str(),
                                    block["name"].as_str(),
                                    &initial,
                                    true,
                                    out,
                                )?;
                            }
                            _ => return Err("Unsupported Messages content block".into()),
                        }
                    }
                    "content_block_delta" => match v["delta"]["type"].as_str() {
                        Some("text_delta") => self.text(
                            v["delta"]["text"].as_str().ok_or("Missing text delta")?,
                            out,
                        )?,
                        Some("input_json_delta") => {
                            if !self.tools.contains_key(&key) {
                                return Err("Tool arguments arrived before tool_use".into());
                            }
                            self.tool(
                                &key,
                                None,
                                None,
                                v["delta"]["partial_json"]
                                    .as_str()
                                    .ok_or("Missing JSON delta")?,
                                true,
                                out,
                            )?;
                        }
                        _ => return Err("Unsupported Messages delta".into()),
                    },
                    "content_block_stop" => {
                        self.closed.insert(key);
                    }
                    "message_delta" => {
                        if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                            self.stop = match reason {
                                "end_turn" | "stop_sequence" => "stop",
                                "tool_use" => "tool_calls",
                                "max_tokens" => "length",
                                _ => {
                                    return Err(format!(
                                        "Unsupported Messages stop reason {reason}"
                                    ))
                                }
                            }
                            .into();
                        }
                    }
                    "message_start" | "message_stop" | "ping" => {}
                    _ => return Err(format!("Unsupported Messages conversion event {event}")),
                }
            }
        }
        Ok(())
    }
    fn finalize(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        if self.ended {
            return Ok(());
        }
        self.close_reasoning(out);
        let keys: Vec<_> = self.tools.keys().cloned().collect();
        for key in &keys {
            self.tool(key, None, None, "", true, out)?;
        }
        for tool in self.tools.values() {
            if (self.to != Protocol::Messages && !tool.started)
                || tool.id.is_empty()
                || tool.name.is_empty()
            {
                return Err("Incomplete tool identity at stream end".into());
            }
            let arguments = if tool.args.is_empty() {
                "{}"
            } else {
                tool.args.as_str()
            };
            let parsed: Value =
                serde_json::from_str(arguments).map_err(|_| "Incomplete tool JSON arguments")?;
            if !parsed.is_object() {
                return Err("Tool arguments must be a JSON object".into());
            }
        }
        self.begin(out);
        let finish = if self.stop == "length" {
            "length"
        } else if !self.tools.is_empty() {
            "tool_calls"
        } else {
            self.stop.as_str()
        }
        .to_owned();
        match self.to {
            Protocol::Chat => {
                self.emit(out,None,json!({"id":self.id,"object":"chat.completion.chunk","created":crate::db::now_ms()/1000,"model":self.model,"choices":[{"index":0,"delta":{},"finish_reason":finish}]}));
                if self.observer.input.is_some() || self.observer.output.is_some() {
                    self.emit(out,None,json!({"id":self.id,"object":"chat.completion.chunk","created":crate::db::now_ms()/1000,"model":self.model,"choices":[],"usage":self.usage(true)}));
                }
                out.extend(b"data: [DONE]\n\n");
            }
            Protocol::Messages => {
                if let Some(index) = self.text_index {
                    self.emit(
                        out,
                        Some("content_block_stop"),
                        json!({"type":"content_block_stop","index":index}),
                    );
                }
                let mut tools: Vec<_> = self
                    .tools
                    .values()
                    .map(|t| {
                        (
                            t.ordinal,
                            t.id.clone(),
                            t.name.clone(),
                            if t.args.is_empty() {
                                "{}".to_owned()
                            } else {
                                t.args.clone()
                            },
                        )
                    })
                    .collect();
                tools.sort_by_key(|t| t.0);
                for (_, id, name, args) in tools {
                    let index = self.next_index;
                    self.next_index += 1;
                    self.emit(out,Some("content_block_start"),json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}));
                    self.emit(out,Some("content_block_delta"),json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":args}}));
                    self.emit(
                        out,
                        Some("content_block_stop"),
                        json!({"type":"content_block_stop","index":index}),
                    );
                }
                let stop = match finish.as_str() {
                    "length" => "max_tokens",
                    "tool_calls" => "tool_use",
                    "stop" => "end_turn",
                    _ => return Err("Unsupported Chat finish reason for Messages".into()),
                };
                self.emit(out,Some("message_delta"),json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":self.messages_wire_usage()}));
                self.emit(out, Some("message_stop"), json!({"type":"message_stop"}));
            }
            Protocol::Responses => {
                let mut items = Vec::new();
                if let Some(index)=self.reasoning_index { items.push((index,self.reasoning_item())); }
                if let Some(index) = self.text_index {
                    let part = json!({"type":"output_text","text":self.text,"annotations":[]});
                    let item = json!({"id":self.message_item,"type":"message","role":"assistant","status":"completed","content":[part]});
                    self.emit(out,Some("response.output_text.done"),json!({"type":"response.output_text.done","item_id":self.message_item,"output_index":index,"content_index":0,"text":self.text}));
                    self.emit(out,Some("response.content_part.done"),json!({"type":"response.content_part.done","item_id":self.message_item,"output_index":index,"content_index":0,"part":part}));
                    self.emit(out,Some("response.output_item.done"),json!({"type":"response.output_item.done","output_index":index,"item":item}));
                    items.push((index, item));
                }
                let tools: Vec<_> = self
                    .tools
                    .values()
                    .map(|t| {
                        (
                            t.index.unwrap(),
                            t.item_id.clone(),
                            t.id.clone(),
                            t.name.clone(),
                            if t.args.is_empty() {
                                "{}".into()
                            } else {
                                t.args.clone()
                            },
                        )
                    })
                    .collect();
                for (index, item_id, call_id, name, args) in tools {
                    let item = json!({"id":item_id,"type":"function_call","status":"completed","call_id":call_id,"name":name,"arguments":args});
                    self.emit(out,Some("response.function_call_arguments.done"),json!({"type":"response.function_call_arguments.done","item_id":item_id,"output_index":index,"name":name,"arguments":args}));
                    self.emit(out,Some("response.output_item.done"),json!({"type":"response.output_item.done","output_index":index,"item":item}));
                    items.push((index, item));
                }
                items.sort_by_key(|(i, _)| *i);
                let incomplete = finish == "length";
                let status = if incomplete {
                    "incomplete"
                } else {
                    "completed"
                };
                let event = if incomplete {
                    "response.incomplete"
                } else {
                    "response.completed"
                };
                let response = self.response(status, items.into_iter().map(|(_, v)| v).collect());
                self.emit(out, Some(event), json!({"type":event,"response":response}));
            }
        }
        self.ended = true;
        Ok(())
    }
    fn convert_frames(&mut self, frames: Vec<Frame>) -> Result<Vec<u8>, String> {
        // Validate the whole incoming batch before committing any target bytes.
        for frame in &frames {
            if !frame.data.trim().is_empty() && frame.data.trim() != "[DONE]" {
                let v: Value = serde_json::from_str(&frame.data).map_err(|_| "Invalid SSE JSON")?;
                self.unsupported(frame, &v)?;
            }
        }
        let mut out = Vec::new();
        for frame in frames {
            self.convert(&frame, &mut out)?;
        }
        if self.observer.done {
            self.finalize(&mut out)?;
        }
        Ok(out)
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        if self.from == self.to {
            return Err("Native SSE must be forwarded without transcoding".into());
        }
        let result = self
            .observer
            .feed(bytes)
            .and_then(|frames| self.convert_frames(frames));
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }
    pub fn finish(&mut self) -> Result<Vec<u8>, String> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = self
            .observer
            .end_frames()
            .and_then(|frames| self.convert_frames(frames));
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(event: Option<&str>, value: serde_json::Value) -> Vec<u8> {
        Frame {
            event: event.map(str::to_owned),
            data: value.to_string(),
        }
        .encode()
    }
    #[test]
    fn frames_preserve_crlf_multiline_and_chunked_unicode() {
        let mut o = Observer::new(Protocol::Chat);
        let bytes="event: test\r\ndata: {\"choices\":[{\"index\":0,\r\ndata: \"delta\":{\"content\":\"你好\"},\"finish_reason\":null}]}\r\n\r\n".as_bytes();
        let mut frames = Vec::new();
        for b in bytes {
            frames.extend(o.feed(&[*b]).unwrap());
        }
        assert_eq!(frames.len(), 1);
        assert!(frames[0].data.contains("你好"));
        assert!(o.meaningful);
    }
    #[test]
    fn chat_waits_for_post_finish_usage() {
        let mut o = Observer::new(Protocol::Chat);
        o.feed(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n").unwrap();
        assert!(!o.done);
        o.feed(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":4}}\n\ndata: [DONE]\n\n").unwrap();
        o.finish().unwrap();
        assert_eq!((o.input, o.output), (Some(0), Some(4)));
    }
    #[test]
    fn responses_incomplete_and_messages_errors_are_not_success() {
        for(p,s)in [(Protocol::Responses,"event: response.incomplete\ndata: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n"),(Protocol::Messages,"event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}\n\n")]{let mut o=Observer::new(p);assert!(o.feed(s.as_bytes()).is_err());assert!(o.failed.is_some());assert!(o.finish().is_err());}
    }
    #[test]
    fn oversized_frame_and_missing_terminal_are_rejected() {
        let mut o = Observer::new(Protocol::Chat);
        assert!(o.feed(&vec![b'x'; (1 << 20) + 1]).is_err());
        let mut o = Observer::new(Protocol::Messages);
        o.feed(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2}}}\n\n").unwrap();
        assert!(o.finish().is_err());
    }
    #[test]
    fn chat_tool_arguments_transcode_to_both_target_lifecycles() {
        for target in [Protocol::Responses, Protocol::Messages] {
            let mut t = Transcoder::new(Protocol::Chat, target);
            let mut out = Vec::new();
            for v in [
                serde_json::json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"read","arguments":"{\"path\":\""}}]},"finish_reason":null}]}),
                serde_json::json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"资料.txt\"}"}}]},"finish_reason":"tool_calls"}]}),
                serde_json::json!({"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3}}),
            ] {
                out.extend(t.feed(&frame(None, v)).unwrap());
            }
            out.extend(t.feed(b"data: [DONE]\n\n").unwrap());
            out.extend(t.finish().unwrap());
            let s = String::from_utf8(out).unwrap();
            assert!(s.contains("call_a"));
            assert!(s.contains("资料.txt"));
            let mut observer = Observer::new(target);
            observer.feed(s.as_bytes()).unwrap();
            observer.finish().unwrap();
            assert_eq!((observer.input, observer.output), (Some(7), Some(3)));
        }
    }
    #[test]
    fn reasoning_conversion_refuses_before_target_output() {
        let mut t = Transcoder::new(Protocol::Responses, Protocol::Messages);
        assert!(t.feed(b"event: response.reasoning_summary_text.delta\ndata: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"private thought\"}\n\n").is_err());
    }
    #[test]
    fn messages_cache_usage_is_total_monotonic_and_checked() {
        let mut o = Observer::new(Protocol::Messages);
        o.feed(&frame(Some("message_start"), json!({"type":"message_start","message":{"usage":{"input_tokens":2,"cache_read_input_tokens":10,"cache_creation_input_tokens":5,"output_tokens":0}}}))).unwrap();
        assert_eq!(o.input, Some(17));
        o.feed(&frame(
            Some("message_delta"),
            json!({"type":"message_delta","usage":{"output_tokens":4}}),
        ))
        .unwrap();
        assert_eq!((o.input, o.output), (Some(17), Some(4)));
        o.feed(&frame(
            Some("message_delta"),
            json!({"type":"message_delta","usage":{"input_tokens":1,"output_tokens":2}}),
        ))
        .unwrap();
        assert_eq!((o.input, o.output), (Some(17), Some(4)));
        let mut o = Observer::new(Protocol::Messages);
        assert!(o.feed(&frame(None,json!({"type":"message_start","message":{"usage":{"input_tokens":i64::MAX,"cache_read_input_tokens":1}}}))).is_err());
    }
    #[test]
    fn final_responses_snapshot_is_not_silently_dropped() {
        let mut t = Transcoder::new(Protocol::Responses, Protocol::Chat);
        let out=t.feed(&frame(None,json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"snapshot"}]}],"usage":{"input_tokens":3,"output_tokens":1}}}))).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("snapshot"));
    }
    fn source_fixture(source: Protocol) -> Vec<u8> {
        let mut bytes = Vec::new();
        let values = match source {
            Protocol::Chat => vec![
                json!({"model":"m","choices":[{"index":0,"delta":{"content":"你好"}}]}),
                json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"read","arguments":"{\"p\":"}},{"index":1,"id":"call_b","function":{"name":"write","arguments":"{\"x\":"}}]}}]}),
                json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"2}"}},{"index":0,"function":{"arguments":"\"资料\"}"}}]},"finish_reason":"tool_calls"}]}),
                json!({"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":4}}),
            ],
            Protocol::Responses => vec![
                json!({"type":"response.created","response":{"model":"m"}}),
                json!({"type":"response.output_text.delta","output_index":0,"delta":"你好"}),
                json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_a","call_id":"call_a","name":"read","arguments":""}}),
                json!({"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","id":"fc_b","call_id":"call_b","name":"write","arguments":""}}),
                json!({"type":"response.function_call_arguments.delta","item_id":"fc_b","delta":"{\"x\":2}"}),
                json!({"type":"response.function_call_arguments.delta","item_id":"fc_a","delta":"{\"p\":\"资料\"}"}),
                json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":8,"output_tokens":4},"output":[{"type":"message","content":[{"type":"output_text","text":"你好"}]},{"type":"function_call","call_id":"call_a","name":"read","arguments":"{\"p\":\"资料\"}"},{"type":"function_call","call_id":"call_b","name":"write","arguments":"{\"x\":2}"}]}}),
            ],
            Protocol::Messages => vec![
                json!({"type":"message_start","message":{"model":"m","content":[],"usage":{"input_tokens":8,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_a","name":"read","input":{}}}),
                json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"p\":\"资料\"}"}}),
                json!({"type":"content_block_stop","index":1}),
                json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call_b","name":"write","input":{}}}),
                json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"x\":2}"}}),
                json!({"type":"content_block_stop","index":2}),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
                json!({"type":"message_stop"}),
            ],
        };
        for v in values {
            bytes.extend(frame(None, v));
        }
        if source == Protocol::Chat {
            bytes.extend(b"data: [DONE]\n\n");
        }
        bytes
    }
    #[test]
    fn all_six_directions_preserve_text_parallel_tool_identity_json_and_usage() {
        for source in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            for target in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
                if source == target {
                    continue;
                }
                let mut t = Transcoder::new(source, target);
                let mut out = Vec::new();
                for chunk in source_fixture(source).chunks(7) {
                    out.extend(t.feed(chunk).unwrap());
                }
                out.extend(t.finish().unwrap());
                let mut o = Observer::new(target);
                let frames = o.feed(&out).unwrap();
                o.finish().unwrap();
                assert_eq!(
                    (o.input, o.output),
                    (Some(8), Some(4)),
                    "{source:?} -> {target:?}"
                );
                let mut text = String::new();
                let mut calls: BTreeMap<u64, (String, String, String)> = BTreeMap::new();
                let mut active = None;
                for f in frames {
                    if f.data == "[DONE]" {
                        continue;
                    }
                    let v: Value = serde_json::from_str(&f.data).unwrap();
                    match target {
                        Protocol::Chat => {
                            if let Some(cs) = v["choices"].as_array() {
                                for c in cs {
                                    let d = &c["delta"];
                                    if let Some(s) = d["content"].as_str() {
                                        text.push_str(s);
                                    }
                                    for call in d["tool_calls"].as_array().into_iter().flatten() {
                                        let e = calls
                                            .entry(call["index"].as_u64().unwrap())
                                            .or_default();
                                        if let Some(s) = call["id"].as_str() {
                                            e.0.push_str(s);
                                        }
                                        if let Some(s) = call["function"]["name"].as_str() {
                                            e.1.push_str(s);
                                        }
                                        if let Some(s) = call["function"]["arguments"].as_str() {
                                            e.2.push_str(s);
                                        }
                                    }
                                }
                            }
                        }
                        Protocol::Responses => {
                            if v["type"] == "response.output_text.delta" {
                                text.push_str(v["delta"].as_str().unwrap());
                            }
                            if v["type"] == "response.completed" {
                                for item in v["response"]["output"].as_array().unwrap() {
                                    if item["type"] == "function_call" {
                                        calls.insert(
                                            calls.len() as u64,
                                            (
                                                item["call_id"].as_str().unwrap().into(),
                                                item["name"].as_str().unwrap().into(),
                                                item["arguments"].as_str().unwrap().into(),
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                        Protocol::Messages => {
                            let index = v["index"].as_u64().unwrap_or(0);
                            match v["type"].as_str().unwrap() {
                                "content_block_start" => {
                                    assert_eq!(
                                        active, None,
                                        "Messages blocks must close sequentially"
                                    );
                                    active = Some(index);
                                    if v["content_block"]["type"] == "tool_use" {
                                        calls.insert(
                                            index,
                                            (
                                                v["content_block"]["id"].as_str().unwrap().into(),
                                                v["content_block"]["name"].as_str().unwrap().into(),
                                                String::new(),
                                            ),
                                        );
                                    }
                                }
                                "content_block_delta" => {
                                    assert_eq!(active, Some(index));
                                    if let Some(s) = v["delta"]["text"].as_str() {
                                        text.push_str(s);
                                    }
                                    if let Some(s) = v["delta"]["partial_json"].as_str() {
                                        calls.get_mut(&index).unwrap().2.push_str(s);
                                    }
                                }
                                "content_block_stop" => {
                                    assert_eq!(active, Some(index));
                                    active = None;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                assert_eq!(text, "你好");
                assert_eq!(calls.len(), 2);
                let calls: BTreeMap<_, _> = calls
                    .into_values()
                    .map(|(id, name, args)| {
                        (id, (name, serde_json::from_str::<Value>(&args).unwrap()))
                    })
                    .collect();
                assert_eq!(calls["call_a"], ("read".into(), json!({"p":"资料"})));
                assert_eq!(calls["call_b"], ("write".into(), json!({"x":2})));
            }
        }
    }
    #[test]
    fn eof_chat_finish_is_success_but_truncated_tool_json_is_not() {
        let mut o = Observer::new(Protocol::Chat);
        o.feed(&frame(
            None,
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
        ))
        .unwrap();
        assert!(!o.done);
        o.finish().unwrap();
        assert!(o.done);
        let mut t = Transcoder::new(Protocol::Chat, Protocol::Messages);
        t.feed(&frame(None,json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"read","arguments":"{"}}]},"finish_reason":"tool_calls"}]}))).unwrap();
        assert!(t.finish().is_err());
    }
    #[test]
    fn done_only_text_is_preserved_and_structured_tool_input_is_validated() {
        let mut t = Transcoder::new(Protocol::Responses, Protocol::Chat);
        let mut out=t.feed(&frame(None,json!({"type":"response.output_text.done","output_index":0,"content_index":0,"text":"done only"}))).unwrap();
        out.extend(
            t.feed(&frame(
                None,
                json!({"type":"response.completed","response":{"id":"resp_done_only","status":"completed","output":[{"id":"msg_done_only","type":"message","role":"assistant","content":[{"type":"output_text","text":"done only","annotations":[]}]}]}}),
            ))
            .unwrap(),
        );
        assert!(String::from_utf8(out).unwrap().contains("done only"));
        let mut t = Transcoder::new(Protocol::Messages, Protocol::Chat);
        assert!(t.feed(&frame(None,json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"read","input":[]}}))).is_err());
    }
    #[test]
    fn messages_wire_required_usage_does_not_create_source_usage() {
        let mut t = Transcoder::new(Protocol::Chat, Protocol::Messages);
        let mut out = t
            .feed(&frame(
                None,
                json!({"choices":[{"index":0,"delta":{"content":"text"},"finish_reason":"stop"}]}),
            ))
            .unwrap();
        out.extend(t.feed(b"data: [DONE]\n\n").unwrap());
        assert_eq!((t.observer.input, t.observer.output), (None, None));
        let mut decoder = Decoder::default();
        for f in decoder.feed(&out).unwrap() {
            let v: Value = serde_json::from_str(&f.data).unwrap();
            let usage = if v["type"] == "message_start" {
                Some(&v["message"]["usage"])
            } else if v["type"] == "message_delta" {
                Some(&v["usage"])
            } else {
                None
            };
            if let Some(usage) = usage {
                assert_eq!(usage["input_tokens"], 0);
                assert_eq!(usage["output_tokens"], 0);
            }
        }
    }
    #[test]
    fn native_legacy_function_call_is_meaningful_but_conversion_explicitly_refuses() {
        let bytes = frame(
            None,
            json!({"choices":[{"index":0,"delta":{"function_call":{"name":"read","arguments":"{}"}},"finish_reason":"function_call"}]}),
        );
        let mut o = Observer::new(Protocol::Chat);
        o.feed(&bytes).unwrap();
        o.finish().unwrap();
        assert!(o.meaningful);
        let mut t = Transcoder::new(Protocol::Chat, Protocol::Messages);
        assert!(t.feed(&bytes).is_err());
        assert!(!t.saw_output);
    }
}
