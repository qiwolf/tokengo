//! Synthetic lifecycle fixtures matching the safely observed sparse terminal shape.
use serde_json::{json, Value};
use tokengo_server::protocol::{sse::Transcoder, Protocol};
fn start() -> Value {
    json!({"type":"response.created","response":{"id":"resp_sparse","status":"in_progress","model":"source","output":[]}})
}
fn terminal() -> Value {
    json!({"type":"response.completed","response":{"id":"resp_sparse","status":"completed","output":[],"usage":{"input_tokens":11,"output_tokens":5}}})
}
fn message(parts: &[&str]) -> Vec<Value> {
    let mut v = vec![
        start(),
        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_sparse","type":"message","role":"assistant","content":[]}}),
    ];
    for (i, text) in parts.iter().enumerate() {
        v.extend([json!({"type":"response.content_part.added","output_index":0,"item_id":"msg_sparse","content_index":i,"part":{"type":"output_text","text":"","annotations":[]}}),json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_sparse","content_index":i,"delta":text}),json!({"type":"response.output_text.done","output_index":0,"item_id":"msg_sparse","content_index":i,"text":text}),json!({"type":"response.content_part.done","output_index":0,"item_id":"msg_sparse","content_index":i,"part":{"type":"output_text","text":text,"annotations":[]}})]);
    }
    v.push(json!({"type":"response.output_item.done","output_index":0,"item":{"id":"msg_sparse","type":"message","role":"assistant","content":parts.iter().map(|text|json!({"type":"output_text","text":text,"annotations":[]})).collect::<Vec<_>>()}}));
    v.push(terminal());
    v
}
fn tools() -> Vec<Value> {
    let mut v = vec![start()];
    for i in 0..2 {
        let id = format!("fc_{i}");
        let call = format!("call_{i}");
        v.extend([json!({"type":"response.output_item.added","output_index":i,"item":{"id":id,"type":"function_call","call_id":call,"name":"sum","arguments":""}}),json!({"type":"response.function_call_arguments.delta","output_index":i,"item_id":id,"delta":"{\"a\":2,\"b\":3}"}),json!({"type":"response.function_call_arguments.done","output_index":i,"item_id":id,"arguments":"{\"a\":2,\"b\":3}"}),json!({"type":"response.output_item.done","output_index":i,"item":{"id":id,"type":"function_call","call_id":call,"name":"sum","arguments":"{\"a\":2,\"b\":3}"}})]);
    }
    v.push(terminal());
    v
}
fn run(events: &[Value], target: Protocol) -> Result<(String, Transcoder), String> {
    let mut t = Transcoder::new(Protocol::Responses, target);
    let mut out = vec![];
    for v in events {
        out.extend(t.feed(format!("data: {v}\n\n").as_bytes())?);
    }
    out.extend(t.finish()?);
    Ok((String::from_utf8(out).unwrap(), t))
}
#[test]
fn closed_text_parts_allow_sparse_terminal_and_exact_usage() {
    for target in [Protocol::Chat, Protocol::Messages] {
        let (out, t) = run(&message(&["O", "K"]), target).unwrap();
        assert!(out.contains(if target == Protocol::Chat {
            "[DONE]"
        } else {
            "message_stop"
        }));
        assert_eq!(t.observer.input, Some(11));
        assert_eq!(t.observer.output, Some(5));
    }
}
#[test]
fn closed_parallel_tools_allow_sparse_terminal_preserving_ids_and_json() {
    for target in [Protocol::Chat, Protocol::Messages] {
        let (out, _) = run(&tools(), target).unwrap();
        assert!(out.contains("call_0") && out.contains("call_1"));
        assert!(out.contains("sum"));
    }
}
#[test]
fn sparse_text_requires_matching_closed_identity_and_all_part_lifecycles() {
    let base = message(&["OK"]);
    let mut variants = vec![];
    for event in [
        "response.output_item.added",
        "response.content_part.added",
        "response.output_text.done",
        "response.content_part.done",
        "response.output_item.done",
    ] {
        variants.push(
            base.iter()
                .filter(|v| v["type"] != event)
                .cloned()
                .collect::<Vec<_>>(),
        );
    }
    for event in [
        "response.output_text.delta",
        "response.output_text.done",
        "response.content_part.done",
    ] {
        let mut v = base.clone();
        v.iter_mut().find(|v| v["type"] == event).unwrap()["item_id"] = json!("wrong");
        variants.push(v);
    }
    let mut v = base.clone();
    v.iter_mut()
        .find(|v| v["type"] == "response.output_item.done")
        .unwrap()["item"]["id"] = json!("wrong");
    variants.push(v);
    let mut v = base.clone();
    v.iter_mut()
        .find(|v| v["type"] == "response.output_item.done")
        .unwrap()["item"]["status"] = json!("in_progress");
    variants.push(v);
    let mut v = base.clone();
    v.last_mut().unwrap()["response"]["id"] = json!("wrong");
    variants.push(v);
    for v in variants {
        assert!(
            run(&v, Protocol::Chat).is_err(),
            "accepted incomplete lifecycle: {v:?}"
        );
    }
}
#[test]
fn sparse_tools_require_closed_arguments_item_identity_and_call_identity() {
    let base = tools();
    let mut variants = vec![];
    for event in [
        "response.function_call_arguments.done",
        "response.output_item.done",
    ] {
        variants.push(
            base.iter()
                .filter(|v| v["type"] != event)
                .cloned()
                .collect::<Vec<_>>(),
        );
    }
    for field in ["id", "call_id", "name", "arguments"] {
        let mut v = base.clone();
        v.iter_mut()
            .find(|v| v["type"] == "response.output_item.done")
            .unwrap()["item"][field] = json!("changed");
        variants.push(v);
    }
    let mut v = base.clone();
    let last = v.len() - 1;
    v.insert(last,json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_0","delta":" "}));
    variants.push(v);
    for v in variants {
        assert!(
            run(&v, Protocol::Chat).is_err(),
            "accepted unclosed or changed tool: {v:?}"
        );
    }
}
#[test]
fn nonempty_aggregate_contradiction_remains_an_error() {
    let mut v = message(&["OK"]);
    v.last_mut().unwrap()["response"]["output"] = json!([{"id":"msg_sparse","type":"message","role":"assistant","content":[{"type":"output_text","text":"different"}]}]);
    assert!(run(&v, Protocol::Chat).is_err());
}
#[test]
fn complete_aggregate_keeps_existing_snapshot_only_path() {
    let events = vec![
        start(),
        json!({"type":"response.completed","response":{"id":"resp_sparse","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"snapshot only"}]}],"usage":{"input_tokens":11,"output_tokens":5}}}),
    ];
    assert!(run(&events, Protocol::Chat)
        .unwrap()
        .0
        .contains("snapshot only"));
}
#[test]
fn sparse_empty_or_unclosed_response_cannot_finish_as_success() {
    for mut events in [vec![start(), terminal()], message(&["OK"])] {
        if events.len() > 2 {
            events.last_mut().unwrap()["response"]["status"] = json!("in_progress");
        }
        assert!(run(&events, Protocol::Chat).is_err());
    }
}

fn target_text_and_tools(
    out: &str,
    to: Protocol,
) -> (
    String,
    std::collections::BTreeMap<u64, (String, String, String)>,
    Option<String>,
) {
    let mut text = String::new();
    let mut tools = std::collections::BTreeMap::<u64, (String, String, String)>::new();
    let mut finish = None;
    for line in out
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|line| *line != "[DONE]")
    {
        let v: Value = serde_json::from_str(line).unwrap();
        if to == Protocol::Chat {
            if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
                text.push_str(t);
            }
            for call in v["choices"][0]["delta"]["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let entry = tools.entry(call["index"].as_u64().unwrap()).or_default();
                if let Some(id) = call["id"].as_str() {
                    if !entry.0.is_empty() {
                        assert_eq!(entry.0, id);
                    }
                    entry.0 = id.into();
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    entry.1 = name.into();
                }
                if let Some(args) = call["function"]["arguments"].as_str() {
                    entry.2.push_str(args);
                }
            }
            if let Some(reason) = v["choices"][0]["finish_reason"].as_str() {
                finish = Some(reason.into());
            }
        } else {
            let index = v["index"].as_u64().unwrap_or(0);
            if v["type"] == "content_block_start" && v["content_block"]["type"] == "tool_use" {
                tools.insert(
                    index,
                    (
                        v["content_block"]["id"].as_str().unwrap().into(),
                        v["content_block"]["name"].as_str().unwrap().into(),
                        String::new(),
                    ),
                );
            }
            if v["type"] == "content_block_delta" {
                if let Some(t) = v["delta"]["text"].as_str() {
                    text.push_str(t);
                }
                if let Some(args) = v["delta"]["partial_json"].as_str() {
                    tools.get_mut(&index).unwrap().2.push_str(args);
                }
            }
            if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                finish = Some(reason.into());
            }
        }
    }
    (text, tools, finish)
}
#[test]
fn sparse_target_text_and_parallel_tool_arguments_are_exact_without_replay() {
    for to in [Protocol::Chat, Protocol::Messages] {
        let (out, _) = run(&message(&["O", "K"]), to).unwrap();
        assert_eq!(target_text_and_tools(&out, to).0, "OK");
        let (out, _) = run(&tools(), to).unwrap();
        let (_, calls, finish) = target_text_and_tools(&out, to);
        assert_eq!(calls.len(), 2);
        for (i, (id, name, args)) in calls {
            assert_eq!(id, format!("call_{i}"));
            assert_eq!(name, "sum");
            assert_eq!(
                serde_json::from_str::<Value>(&args).unwrap(),
                json!({"a":2,"b":3})
            );
        }
        assert_eq!(
            finish.as_deref(),
            Some(if to == Protocol::Chat {
                "tool_calls"
            } else {
                "tool_use"
            })
        );
    }
}
#[test]
fn sparse_exception_is_not_missing_null_or_nonarray_output() {
    for absent in [true, false] {
        for value in [Value::Null, json!({}), json!("not array")] {
            let mut v = message(&["OK"]);
            if absent {
                v.last_mut().unwrap()["response"]
                    .as_object_mut()
                    .unwrap()
                    .remove("output");
            } else {
                v.last_mut().unwrap()["response"]["output"] = value;
            }
            assert!(run(&v, Protocol::Chat).is_err());
        }
    }
}
#[test]
fn sparse_missing_usage_does_not_invent_source_zero() {
    let mut v = message(&["OK"]);
    v.last_mut().unwrap()["response"]
        .as_object_mut()
        .unwrap()
        .remove("usage");
    let (_, t) = run(&v, Protocol::Chat).unwrap();
    assert_eq!(t.observer.input, None);
    assert_eq!(t.observer.output, None);
}
#[test]
fn sparse_rejects_duplicate_or_late_parts_and_bounds() {
    let base = message(&["OK"]);
    let mut variants = vec![];
    let mut v = base.clone();
    let item = v
        .iter()
        .find(|v| v["type"] == "response.output_item.added")
        .unwrap()
        .clone();
    v.insert(2, item);
    variants.push(v);
    let mut v = base.clone();
    v.iter_mut()
        .find(|v| v["type"] == "response.content_part.done")
        .unwrap()["content_index"] = json!(1);
    variants.push(v);
    for before_item_done in [true, false] {
        let mut v = base.clone();
        let pos = if before_item_done {
            v.iter()
                .position(|v| v["type"] == "response.output_item.done")
                .unwrap()
        } else {
            v.len() - 1
        };
        v.insert(pos,json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"msg_sparse","delta":"late"}));
        variants.push(v);
    }
    let mut v = base.clone();
    v.iter_mut()
        .find(|v| v["type"] == "response.output_item.added")
        .unwrap()["output_index"] = json!(128);
    variants.push(v);
    let mut v = base.clone();
    v.iter_mut()
        .find(|v| v["type"] == "response.output_item.added")
        .unwrap()["item"]["id"] = json!("x".repeat(513));
    variants.push(v);
    for v in variants {
        assert!(run(&v, Protocol::Chat).is_err());
    }
}
