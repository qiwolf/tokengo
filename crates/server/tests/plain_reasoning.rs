use serde_json::{json, Value};
use tokengo_server::protocol::{
    convert_request, convert_response,
    sse::{Frame, Transcoder},
    Protocol,
};
fn chat() -> Value {
    json!({"id":"c","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":null,"reasoning_content":"先检查工具。","tool_calls":[{"id":"call_exact","type":"function","function":{"name":"sum","arguments":"{\"a\":2}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":8,"completion_tokens":5,"completion_tokens_details":{"reasoning_tokens":2}}})
}
#[test]
fn plain_reasoning_and_tools_round_trip_without_summary_or_encryption() {
    let r = convert_response(Protocol::Chat, Protocol::Responses, &chat()).unwrap();
    assert_eq!(r["output"][0]["type"], "reasoning");
    assert_eq!(r["output"][0]["summary"], json!([]));
    assert_eq!(
        r["output"][0]["content"],
        json!([{"type":"reasoning_text","text":"先检查工具。"}])
    );
    assert!(r["output"][0].get("encrypted_content").is_none());
    let back = convert_response(Protocol::Responses, Protocol::Chat, &r).unwrap();
    assert_eq!(
        back["choices"][0]["message"]["reasoning_content"],
        "先检查工具。"
    );
    assert_eq!(
        back["choices"][0]["message"]["tool_calls"][0]["id"],
        "call_exact"
    );
    assert_eq!(
        back["usage"],
        json!({"prompt_tokens":8,"completion_tokens":5,"total_tokens":13,"completion_tokens_details":{"reasoning_tokens":2},"tokengo_raw_usage":{"protocol":"chat","usage":chat()["usage"]}})
    );
    let mut input = r["output"].as_array().unwrap().clone();
    input.push(json!({"type":"function_call_output","call_id":"call_exact","output":"2"}));
    let next = convert_request(
        Protocol::Responses,
        Protocol::Chat,
        &json!({"model":"m","input":input}),
    )
    .unwrap();
    assert_eq!(next["messages"][0]["reasoning_content"], "先检查工具。");
    assert_eq!(next["messages"][0]["tool_calls"][0]["id"], "call_exact");
    assert_eq!(next["messages"][1]["tool_call_id"], "call_exact");
}
#[test]
fn streaming_plain_reasoning_keeps_final_item_and_reverse_deltas() {
    let mut bytes = Vec::new();
    for delta in [
        json!({"reasoning_content":"先"}),
        json!({"reasoning_content":"思考"}),
        json!({"tool_calls":[{"index":0,"id":"call_exact","type":"function","function":{"name":"sum","arguments":"{}"}}]}),
    ] {
        bytes.extend(
            Frame {
                event: None,
                data:
                    json!({"model":"m","choices":[{"index":0,"delta":delta,"finish_reason":null}]})
                        .to_string(),
            }
            .encode(),
        );
    }
    bytes.extend(Frame{event:None,data:json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":8,"completion_tokens":5,"completion_tokens_details":{"reasoning_tokens":2}}}).to_string()}.encode());
    bytes.extend(b"data: [DONE]\n\n");
    let mut t = Transcoder::new(Protocol::Chat, Protocol::Responses);
    let mut out = Vec::new();
    for part in bytes.chunks(7) {
        out.extend(t.feed(part).unwrap());
    }
    out.extend(t.finish().unwrap());
    let text = String::from_utf8(out.clone()).unwrap();
    assert!(text.contains("response.reasoning_text.delta"));
    assert!(text.contains("response.reasoning_text.done"));
    assert!(text.contains("reasoning_tokens"));
    assert!(!text.contains("summary_text"));
    assert!(!text.contains("encrypted_content"));
    let terminal: Value = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .find(|v| v["type"] == "response.completed")
        .unwrap();
    assert_eq!(
        terminal["response"]["output"][0]["content"][0]["text"],
        "先思考"
    );
    let mut reverse = Transcoder::new(Protocol::Responses, Protocol::Chat);
    let mut back = Vec::new();
    for part in out.chunks(5) {
        back.extend(reverse.feed(part).unwrap());
    }
    back.extend(reverse.finish().unwrap());
    let wire = String::from_utf8(back).unwrap();
    assert!(wire.contains("reasoning_content"));
    assert!(wire.contains("call_exact"));
    assert_eq!(
        (reverse.observer.input, reverse.observer.output),
        (Some(8), Some(5))
    );
}
#[test]
fn opaque_summary_ambiguous_or_misordered_reasoning_stays_rejected() {
    for item in [
        json!({"type":"reasoning","summary":[],"encrypted_content":"opaque","content":[{"type":"reasoning_text","text":"x"}]}),
        json!({"type":"reasoning","summary":[{"type":"summary_text","text":"summary"}],"content":[{"type":"reasoning_text","text":"x"}]}),
    ] {
        assert!(convert_request(
            Protocol::Responses,
            Protocol::Chat,
            &json!({"model":"m","input":[item]})
        )
        .is_err());
    }
    let mut c = chat();
    c["choices"][0]["message"]["reasoning"] = json!("different");
    assert!(convert_response(Protocol::Chat, Protocol::Responses, &c).is_err());
    assert!(convert_response(Protocol::Chat, Protocol::Messages, &chat()).is_err());
}

#[tokio::test]
async fn real_http_reasoning_tool_result_replay_preserves_source_and_ledger() {
    use axum::{
        extract::State,
        response::{IntoResponse, Response},
        routing::post,
        Json, Router,
    };
    use std::sync::{Arc, Mutex};
    use tokengo_server::{app, db, AppState, Config};
    async fn source(
        State(seen): State<Arc<Mutex<Vec<Value>>>>,
        Json(body): Json<Value>,
    ) -> Response {
        seen.lock().unwrap().push(body.clone());
        if body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool")
        {
            assert_eq!(body["messages"][0]["reasoning_content"], "先检查工具。");
            assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "call_exact");
            assert_eq!(body["messages"][1]["tool_call_id"], "call_exact");
            assert_eq!(body["messages"][1]["content"], "2");
            return Json(json!({"id":"c2","object":"chat.completion","model":"physical","choices":[{"index":0,"message":{"role":"assistant","content":"result 2","reasoning_content":"已核实。"},"finish_reason":"stop"}],"usage":{"prompt_tokens":13,"completion_tokens":4}})).into_response();
        }
        if body["stream"] == true {
            let mut out = String::new();
            for (delta, stop) in [
                (json!({"reasoning_content":"先检查工具。"}), Value::Null),
                (
                    json!({"tool_calls":[{"index":0,"id":"call_exact","type":"function","function":{"name":"sum","arguments":"{\"a\":2}"}}]}),
                    json!("tool_calls"),
                ),
            ] {
                out.push_str(&format!("data: {}\n\n",json!({"id":"c","model":"physical","choices":[{"index":0,"delta":delta,"finish_reason":stop}]})));
            }
            out.push_str(&format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":5,"completion_tokens_details":{"reasoning_tokens":2}}})
            ));
            return ([("content-type", "text/event-stream")], out).into_response();
        }
        Json(chat()).into_response()
    }
    async fn server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        (
            url,
            tokio::spawn(async move { axum::serve(l, router).await.unwrap() }),
        )
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (base, source_task) = server(
        Router::new()
            .route("/v1/chat/completions", post(source))
            .with_state(seen.clone()),
    )
    .await;
    let dir = std::env::temp_dir().join(format!("plain-reasoning-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = AppState::init(Config {
        listen: "127.0.0.1:0".into(),
        db_path: dir.join("test.db"),
        master_key_path: dir.join("master").to_string_lossy().into(),
        web_dir: None,
        log_dir: None,
    })
    .unwrap();
    let (key, cid) = {
        let c = state.conn();
        let u = db::identity::create_user(&c, "reasoning", "user", &Default::default()).unwrap();
        let (_, key) =
            db::identity::create_key(&c, u.id, "reasoning", &Default::default(), None, None)
                .unwrap();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "only".into(),
                base_url: format!("{base}/v1"),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        db::channels::set_models(
            &c,
            id,
            &[db::channels::ModelMapping::new(
                "logical",
                Some("physical".into()),
                false,
            )],
        )
        .unwrap();
        tokengo_server::protocol_config::save(
            &c,
            id.0,
            &tokengo_server::protocol_config::Capabilities {
                allow_conversion: true,
                chat_reasoning_replay: true,
                ..Default::default()
            },
        )
        .unwrap();
        db::route_policy::save(
            &c,
            "logical",
            &db::route_policy::RoutePolicy {
                channel_ids: Some(vec![id.0]),
                retry_times: Some(0),
                probe_enabled: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        (key, id)
    };
    let (url, gateway) = server(app(state.clone())).await;
    let client = reqwest::Client::new();
    for streaming in [false, true] {
        let first=client.post(format!("{url}/v1/responses")).bearer_auth(&key).json(&json!({"model":"logical","input":"compute","stream":streaming,"max_output_tokens":32})).send().await.unwrap();
        assert_eq!(first.status(), 200);
        let result: Value = if streaming {
            let text = first.text().await.unwrap();
            text.lines()
                .filter_map(|l| l.strip_prefix("data: "))
                .filter_map(|s| serde_json::from_str::<Value>(s).ok())
                .find(|v| v["type"] == "response.completed")
                .unwrap()["response"]
                .clone()
        } else {
            first.json().await.unwrap()
        };
        let mut input = result["output"].as_array().unwrap().clone();
        input.push(json!({"type":"function_call_output","call_id":"call_exact","output":"2"}));
        let second = client
            .post(format!("{url}/v1/responses"))
            .bearer_auth(&key)
            .json(&json!({"model":"logical","input":input,"max_output_tokens":32}))
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), 200);
        let final_body: Value = second.json().await.unwrap();
        assert_eq!(final_body["output"][0]["content"][0]["text"], "已核实。");
        assert_eq!(final_body["usage"]["input_tokens"], 13);
    }
    tokengo_server::billing_writer::replay(&state.conn(), &dir.join("test.db")).unwrap();
    {
        let c = state.conn();
        let attempts: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM attempts WHERE channel_id=?1 AND outcome='success'",
                [cid.0],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 4);
        let n:i64=c.query_row("SELECT COUNT(*) FROM billing_ledger WHERE settled_ms IS NOT NULL AND input_tokens IN (8,13) AND output_tokens IN (4,5)",[],|r|r.get(0)).unwrap();
        assert_eq!(n, 4);
    }
    assert_eq!(seen.lock().unwrap().len(), 4);
    assert!(seen
        .lock()
        .unwrap()
        .iter()
        .all(|v| v["model"] == "physical"));
    gateway.abort();
    source_task.abort();
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn malformed_id_late_reasoning_and_conflicting_sse_snapshot_are_rejected() {
    for id in [
        json!(false),
        json!(1),
        json!(""),
        json!("  "),
        json!("bad\nidentity"),
    ] {
        let item = json!({"id":id,"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"raw"}]});
        assert!(tokengo_server::protocol_sessions::opaque_history(
            Protocol::Responses,
            &json!({"input":[item.clone()]})
        ));
        assert!(convert_request(
            Protocol::Responses,
            Protocol::Chat,
            &json!({"model":"m","input":[item]})
        )
        .is_err());
    }
    let mut late = Transcoder::new(Protocol::Chat, Protocol::Responses);
    late.feed(
        &Frame {
            event: None,
            data: json!({"choices":[{"index":0,"delta":{"content":"answer"}}]}).to_string(),
        }
        .encode(),
    )
    .unwrap();
    assert!(late
        .feed(
            &Frame {
                event: None,
                data: json!({"choices":[{"index":0,"delta":{"reasoning_content":"late"}}]})
                    .to_string()
            }
            .encode()
        )
        .is_err());
    let mut reverse = Transcoder::new(Protocol::Responses, Protocol::Chat);
    for (name, v) in [
        (
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,"item":{"id":"rs_x","type":"reasoning","status":"in_progress","summary":[],"content":[]}}),
        ),
        (
            "response.reasoning_text.delta",
            json!({"type":"response.reasoning_text.delta","output_index":0,"content_index":0,"item_id":"rs_x","delta":"first"}),
        ),
    ] {
        reverse
            .feed(
                &Frame {
                    event: Some(name.into()),
                    data: v.to_string(),
                }
                .encode(),
            )
            .unwrap();
    }
    assert!(reverse.feed(&Frame{event:Some("response.reasoning_text.done".into()),data:json!({"type":"response.reasoning_text.done","output_index":0,"content_index":0,"item_id":"rs_x","text":"different"}).to_string()}.encode()).is_err());
}

#[test]
fn completed_snapshot_only_raw_reasoning_is_meaningful_and_collectable() {
    let item = json!({"id":"rs_snapshot","type":"reasoning","status":"completed","summary":[],"content":[{"type":"reasoning_text","text":"原样推理"}]});
    let terminal = json!({"type":"response.completed","response":{"id":"resp_s","object":"response","status":"completed","model":"m","output":[item],"usage":{"input_tokens":8,"output_tokens":5}}});
    let bytes = Frame {
        event: Some("response.completed".into()),
        data: terminal.to_string(),
    }
    .encode();
    let mut observer = tokengo_server::protocol::sse::Observer::new(Protocol::Responses);
    observer.feed(&bytes).unwrap();
    observer.finish().unwrap();
    assert!(observer.meaningful);
    let mut collector = tokengo_server::protocol::collect::ResponsesCollector::default();
    collector.feed(&bytes).unwrap();
    assert_eq!(
        collector.finish().unwrap()["output"][0]["content"][0]["text"],
        "原样推理"
    );
    let mut t = Transcoder::new(Protocol::Responses, Protocol::Chat);
    let mut out = t.feed(&bytes).unwrap();
    out.extend(t.finish().unwrap());
    assert!(String::from_utf8(out).unwrap().contains("原样推理"));
}

#[test]
fn reasoning_usage_known_zero_unknown_and_invalid_remain_distinct() {
    for value in [json!(0), json!(2)] {
        let mut c = chat();
        c["usage"]["completion_tokens_details"] = json!({"reasoning_tokens":value});
        let r = convert_response(Protocol::Chat, Protocol::Responses, &c).unwrap();
        assert_eq!(
            r["usage"]["output_tokens_details"]["reasoning_tokens"],
            value
        );
        let back = convert_response(Protocol::Responses, Protocol::Chat, &r).unwrap();
        assert_eq!(
            back["usage"]["completion_tokens_details"]["reasoning_tokens"],
            value
        );
    }
    let mut c = chat();
    c["usage"]
        .as_object_mut()
        .unwrap()
        .remove("completion_tokens_details");
    assert!(
        convert_response(Protocol::Chat, Protocol::Responses, &c).unwrap()["usage"]
            .get("output_tokens_details")
            .is_none()
    );
    for bad in [
        json!(-1),
        json!(6),
        json!("2"),
        json!(1.5),
        json!(18446744073709551615u64),
    ] {
        c["usage"]["completion_tokens_details"] = json!({"reasoning_tokens":bad});
        assert!(convert_response(Protocol::Chat, Protocol::Responses, &c).is_err());
    }
}

#[test]
fn reasoning_part_done_seals_deltas_but_allows_identical_final_snapshots() {
    fn wire(kind: &str, value: Value) -> Vec<u8> {
        Frame {
            event: Some(kind.into()),
            data: value.to_string(),
        }
        .encode()
    }
    fn started() -> Transcoder {
        let mut t = Transcoder::new(Protocol::Responses, Protocol::Chat);
        t.feed(&wire("response.output_item.added",json!({"type":"response.output_item.added","output_index":0,"item":{"id":"rs_closed","type":"reasoning","status":"in_progress","summary":[],"content":[]}}))).unwrap();
        t.feed(&wire("response.reasoning_text.delta",json!({"type":"response.reasoning_text.delta","output_index":0,"content_index":0,"item_id":"rs_closed","delta":"sealed"}))).unwrap();
        t.feed(&wire("response.reasoning_text.done",json!({"type":"response.reasoning_text.done","output_index":0,"content_index":0,"item_id":"rs_closed","text":"sealed"}))).unwrap();
        t
    }
    let mut empty = Transcoder::new(Protocol::Responses, Protocol::Chat);
    empty.feed(&wire("response.output_item.added",json!({"type":"response.output_item.added","output_index":0,"item":{"id":"rs_empty","type":"reasoning","status":"in_progress","summary":[],"content":[]}}))).unwrap();
    empty.feed(&wire("response.reasoning_text.done",json!({"type":"response.reasoning_text.done","output_index":0,"content_index":0,"item_id":"rs_empty","text":""}))).unwrap();
    assert!(empty.feed(&wire("response.reasoning_text.delta",json!({"type":"response.reasoning_text.delta","output_index":0,"content_index":0,"item_id":"rs_empty","delta":"late"}))).is_err());
    let mut invalid = started();
    assert!(invalid.feed(&wire("response.reasoning_text.delta",json!({"type":"response.reasoning_text.delta","output_index":0,"content_index":0,"item_id":"rs_closed","delta":"late"}))).is_err(),"A completed reasoning part accepted another text delta");
    let mut valid = started();
    assert!(valid.feed(&wire("response.reasoning_text.delta",json!({"type":"response.reasoning_text.delta","output_index":0,"content_index":0,"item_id":"rs_closed","delta":""}))).unwrap().is_empty());
    let item = json!({"id":"rs_closed","type":"reasoning","status":"completed","summary":[],"content":[{"type":"reasoning_text","text":"sealed"}]});
    assert!(valid
        .feed(&wire(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":0,"item":item})
        ))
        .unwrap()
        .is_empty());
    let terminal = wire(
        "response.completed",
        json!({"type":"response.completed","response":{"id":"resp_closed","object":"response","status":"completed","model":"m","output":[item],"usage":{"input_tokens":3,"output_tokens":2}}}),
    );
    let mut tail = valid.feed(&terminal).unwrap();
    tail.extend(valid.finish().unwrap());
    assert!(
        !String::from_utf8(tail)
            .unwrap()
            .contains("reasoning_content"),
        "Final snapshots duplicated already-emitted reasoning"
    );
    let mut conflict = started();
    assert!(conflict.feed(&wire("response.output_item.done",json!({"type":"response.output_item.done","output_index":0,"item":{"id":"rs_closed","type":"reasoning","status":"completed","summary":[],"content":[{"type":"reasoning_text","text":"sealed suffix"}]}}))).is_err());
}
