//! Shadow proof for explicitly empty Responses aggregate output only.
//! Invalid evidence never changes the legacy nonempty-snapshot conversion path.
use serde_json::Value;
use std::collections::BTreeMap;
const MAX_ITEMS: usize = 128;
const MAX_BYTES: usize = 16 * 1024 * 1024;
#[derive(Default)]
pub(super) struct SparseProof {
    items: BTreeMap<u64, Item>,
    response_id: Option<String>,
    sequence: Option<u64>,
    bytes: usize,
    parts: usize,
    invalid: Option<String>,
}
struct Item {
    id: String,
    kind: String,
    call_id: String,
    name: String,
    args: String,
    args_done: bool,
    parts: BTreeMap<u64, Part>,
    closed: bool,
}
struct Part {
    text: String,
    text_done: bool,
    closed: bool,
}
fn identity(v: &Value) -> Result<&str, String> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
        .ok_or_else(|| "Invalid sparse item identity".into())
}
fn index(v: &Value) -> Result<u64, String> {
    v.as_u64()
        .filter(|n| *n < MAX_ITEMS as u64)
        .ok_or_else(|| "Invalid sparse item or content index".into())
}
fn text(v: &Value) -> Result<&str, String> {
    v.as_str()
        .ok_or_else(|| "Missing sparse final text or arguments".into())
}
fn text_part(v: &Value) -> Result<&str, String> {
    if v["type"] != "output_text"
        || v.get("annotations")
            .is_some_and(|a| !a.is_null() && !a.as_array().is_some_and(Vec::is_empty))
    {
        return Err("Unsupported sparse text content".into());
    }
    text(&v["text"])
}
impl SparseProof {
    pub(super) fn observe(&mut self, event: &str, v: &Value) {
        if self.invalid.is_none() {
            if let Err(e) = self.track(event, v) {
                self.invalid = Some(e);
            }
        }
    }
    fn track(&mut self, event: &str, v: &Value) -> Result<(), String> {
        if let Some(number) = v.get("sequence_number").filter(|number| !number.is_null()) {
            let n = number.as_u64().ok_or("Invalid sparse sequence")?;
            if self.sequence.is_some_and(|old| n <= old) {
                return Err("Sparse sequence changed order".into());
            }
            self.sequence = Some(n);
        }
        if let Some(id) = v["response"].get("id") {
            let id = identity(id)?;
            if self.response_id.as_ref().is_some_and(|old| old != id) {
                return Err("Sparse response identity changed".into());
            }
            self.response_id = Some(id.into());
        }
        if event == "response.output_item.added" {
            let i = index(&v["output_index"])?;
            let item = &v["item"];
            let id = identity(&item["id"])?;
            if self.items.len() >= MAX_ITEMS
                || self.items.contains_key(&i)
                || self.items.values().any(|old| old.id == id)
            {
                return Err("Duplicate or excessive sparse item".into());
            }
            let kind = item["type"].as_str().ok_or("Missing sparse item type")?;
            if !matches!(kind, "message" | "function_call") {
                return Err("Sparse aggregate requires closed text/function items".into());
            }
            let (call_id, name, args) = if kind == "function_call" {
                let call = identity(&item["call_id"])?;
                if self.items.values().any(|old| old.call_id == call) {
                    return Err("Duplicate sparse function call identity".into());
                }
                (
                    call.to_owned(),
                    identity(&item["name"])?.to_owned(),
                    text(&item["arguments"])?.to_owned(),
                )
            } else {
                if item["content"]
                    .as_array()
                    .is_none_or(|parts| !parts.is_empty())
                    || item.get("role").is_some_and(|role| role != "assistant")
                {
                    return Err("Sparse message requires initial empty assistant content".into());
                }
                (String::new(), String::new(), String::new())
            };
            self.bytes = self
                .bytes
                .checked_add(args.len())
                .filter(|n| *n <= MAX_BYTES)
                .ok_or("Sparse content exceeds bound")?;
            self.items.insert(
                i,
                Item {
                    id: id.into(),
                    kind: kind.into(),
                    call_id,
                    name,
                    args,
                    args_done: false,
                    parts: BTreeMap::new(),
                    closed: false,
                },
            );
            return Ok(());
        }
        if !matches!(
            event,
            "response.content_part.added"
                | "response.content_part.done"
                | "response.output_text.delta"
                | "response.output_text.done"
                | "response.function_call_arguments.delta"
                | "response.function_call_arguments.done"
                | "response.output_item.done"
        ) {
            return Ok(());
        }
        let i = match v.get("output_index") {
            Some(value) => index(value)?,
            None => {
                let id = identity(&v["item_id"])?;
                *self
                    .items
                    .iter()
                    .find(|(_, item)| item.id == id)
                    .map(|(i, _)| i)
                    .ok_or("Sparse event has no item identity")?
            }
        };
        let item = self
            .items
            .get_mut(&i)
            .ok_or("Sparse event precedes item identity")?;
        if item.closed {
            return Err("Sparse event follows closed item".into());
        }
        let id = if event == "response.output_item.done" {
            identity(&v["item"]["id"])?
        } else {
            identity(&v["item_id"])?
        };
        if item.id != id {
            return Err("Sparse item identity changed".into());
        }
        if event == "response.output_item.done" {
            let final_item = &v["item"];
            if final_item["type"] != item.kind
                || final_item
                    .get("status")
                    .is_some_and(|status| status != "completed")
            {
                return Err("Sparse final item type/status changed".into());
            }
            if item.kind == "message" {
                if final_item
                    .get("role")
                    .is_some_and(|role| role != "assistant")
                {
                    return Err("Sparse message role changed".into());
                }
                let parts = final_item["content"]
                    .as_array()
                    .ok_or("Sparse item lacks final content")?;
                if parts.is_empty() || parts.len() != item.parts.len() {
                    return Err("Sparse final content count changed".into());
                }
                for (i, part) in parts.iter().enumerate() {
                    let observed = item
                        .parts
                        .get(&(i as u64))
                        .ok_or("Sparse text indices are not contiguous")?;
                    if !observed.closed || !observed.text_done || text_part(part)? != observed.text
                    {
                        return Err("Sparse final part is unclosed or contradictory".into());
                    }
                }
            } else if !item.args_done
                || final_item["call_id"] != item.call_id
                || final_item["name"] != item.name
                || text(&final_item["arguments"])? != item.args
            {
                return Err("Sparse final tool is unclosed or contradictory".into());
            }
            item.closed = true;
            return Ok(());
        }
        if event.starts_with("response.function_call_arguments.") {
            if item.kind != "function_call" || item.args_done {
                return Err("Sparse function arguments changed after done".into());
            }
            if event.ends_with(".done") {
                if text(&v["arguments"])? != item.args {
                    return Err("Sparse final arguments disagree with deltas".into());
                }
                item.args_done = true;
            } else {
                let delta = text(&v["delta"])?;
                self.bytes = self
                    .bytes
                    .checked_add(delta.len())
                    .filter(|n| *n <= MAX_BYTES)
                    .ok_or("Sparse content exceeds bound")?;
                item.args.push_str(delta);
            }
            return Ok(());
        }
        if item.kind != "message" {
            return Err("Sparse text event does not belong to message".into());
        }
        let p = index(&v["content_index"])?;
        if event == "response.content_part.added" {
            if self.parts >= MAX_ITEMS || item.parts.contains_key(&p) {
                return Err("Duplicate or excessive sparse text part".into());
            }
            let initial = text_part(&v["part"])?;
            self.bytes = self
                .bytes
                .checked_add(initial.len())
                .filter(|n| *n <= MAX_BYTES)
                .ok_or("Sparse content exceeds bound")?;
            self.parts += 1;
            item.parts.insert(
                p,
                Part {
                    text: initial.into(),
                    text_done: false,
                    closed: false,
                },
            );
            return Ok(());
        }
        let part = item
            .parts
            .get_mut(&p)
            .ok_or("Sparse text precedes content identity")?;
        if part.closed {
            return Err("Sparse text event follows closed content".into());
        }
        match event {
            "response.output_text.delta" => {
                if part.text_done {
                    return Err("Sparse text delta follows final text".into());
                }
                let delta = text(&v["delta"])?;
                self.bytes = self
                    .bytes
                    .checked_add(delta.len())
                    .filter(|n| *n <= MAX_BYTES)
                    .ok_or("Sparse content exceeds bound")?;
                part.text.push_str(delta);
            }
            "response.output_text.done" => {
                if part.text_done || text(&v["text"])? != part.text {
                    return Err("Sparse final text contradicts deltas".into());
                }
                part.text_done = true;
            }
            "response.content_part.done" => {
                if !part.text_done || text_part(&v["part"])? != part.text {
                    return Err("Sparse part closes without matching final text".into());
                }
                part.closed = true;
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn finish(&self, response: &Value, converted_text: &str) -> Result<(), String> {
        if let Some(error) = &self.invalid {
            return Err(format!("Invalid sparse Responses terminal proof: {error}"));
        }
        if response
            .get("status")
            .is_some_and(|status| status != "completed")
            || identity(&response["id"])?
                != self
                    .response_id
                    .as_deref()
                    .ok_or("Missing sparse response identity")?
        {
            return Err("Sparse response terminal identity/status changed".into());
        }
        if self.items.is_empty() {
            return Err("Sparse terminal has no closed output items".into());
        }
        let mut text = String::new();
        for (expected, (i, item)) in self.items.iter().enumerate() {
            if *i != expected as u64 || !item.closed {
                return Err("Sparse terminal has missing or unclosed output items".into());
            }
            for part in item.parts.values() {
                text.push_str(&part.text);
            }
        }
        if text != converted_text {
            return Err("Sparse final text order disagrees with converted output".into());
        }
        Ok(())
    }
}
