use serde_json::{json, Value};
use tokengo_server::protocol::{convert_response, sse::Transcoder, Protocol};

fn chat() -> Value {
    json!({"id":"c1","object":"chat.completion","model":"glm-5.3","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]})
}
fn kimi_usage() -> Value {
    json!({"prompt_tokens":100,"completion_tokens":20,"total_tokens":120,"cached_tokens":30,"prompt_tokens_details":{"cached_tokens":30,"cache_write_tokens":40},"completion_tokens_details":{"reasoning_tokens":5},"vendor_observation":"keep"})
}
fn frame(value: Value) -> Vec<u8> { format!("data: {value}\n\n").into_bytes() }
fn events(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes).lines().filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok()).collect()
}
#[test]
fn glm_request_id_is_typed_metadata_and_semantic_extensions_stay_rejected() {
    let mut body = chat(); body["request_id"] = json!("req-glm");
    let converted = convert_response(Protocol::Chat, Protocol::Responses, &body).unwrap();
    assert_eq!(converted["request_id"], "req-glm");
    assert_eq!(convert_response(Protocol::Responses, Protocol::Chat, &converted).unwrap()["request_id"], "req-glm");
    for extension in ["content_filter", "web_search", "video_result", "future_semantics"] {
        let mut bad = body.clone(); bad[extension] = json!([]);
        assert!(convert_response(Protocol::Chat, Protocol::Responses, &bad).is_err());
    }
    for invalid in [json!(7), json!({}), Value::Null] {
        body["request_id"] = invalid;
        assert!(convert_response(Protocol::Chat, Protocol::Responses, &body).is_err());
    }
}
#[test]
fn kimi_cache_subsets_json_are_conserved_and_raw_usage_retained() {
    let mut body = chat(); body["usage"] = kimi_usage();
    let result = convert_response(Protocol::Chat, Protocol::Responses, &body).unwrap();
    assert_eq!(tokengo_server::gateway::usage_from_value(&body), (100, 20));
    assert_eq!(result["usage"]["input_tokens"], 100);
    assert_eq!(result["usage"]["output_tokens"], 20);
    assert_eq!(result["usage"]["total_tokens"], 120);
    assert_eq!(result["usage"]["input_tokens_details"]["cached_tokens"], 30);
    assert_eq!(result["usage"]["input_tokens_details"]["cache_write_tokens"], 40);
    assert_eq!(result["usage"]["output_tokens_details"]["reasoning_tokens"], 5);
    assert_eq!(result["usage"]["tokengo_raw_usage"]["usage"], body["usage"]);
    assert_eq!(tokengo_server::gateway::usage_from_value(&result), (100, 20));
    let roundtrip = convert_response(Protocol::Responses, Protocol::Chat, &result).unwrap();
    assert_eq!(roundtrip["usage"]["prompt_tokens_details"]["cache_write_tokens"], 40);
    assert_eq!(roundtrip["usage"]["total_tokens"], 120);
    body["usage"]["prompt_tokens_details"]["cache_write_tokens"] = json!(71);
    assert!(convert_response(Protocol::Chat, Protocol::Responses, &body).is_err());
}
#[test]
fn partial_sse_usage_keeps_unknown_buckets_and_validates_against_known_input() {
    let mut converter = Transcoder::new(Protocol::Chat, Protocol::Responses);
    converter.feed(&frame(json!({"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":"stop"}]}))).unwrap();
    converter.feed(&frame(json!({"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20}}))).unwrap();
    converter.feed(&frame(json!({"choices":[],"usage":{"prompt_tokens_details":{"cache_write_tokens":40}}}))).unwrap();
    let output = converter.feed(b"data: [DONE]\n\n").unwrap();
    let values = events(&output);
    let usage = &values.iter().find(|v| v["type"] == "response.completed").unwrap()["response"]["usage"];
    assert_eq!(usage["input_tokens"], 100);
    assert_eq!(usage["output_tokens"], 20);
    assert_eq!(usage["input_tokens_details"]["cache_write_tokens"], 40);
    assert!(usage["input_tokens_details"].get("cached_tokens").is_none());
}
#[test]
fn kimi_sse_final_usage_duplicates_preserve_subsets_without_double_counting() {
    let mut frames=vec![
        frame(json!({"model":"kimi-k3.1-pro","request_id":"req-stream","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]})),
        frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})),
        frame(json!({"choices":[],"usage":kimi_usage()})),
        frame(json!({"choices":[],"usage":kimi_usage()})),
    ];
    frames.push(b"data: [DONE]\n\n".to_vec());
    // Network chunking must not change final counters or the last observed raw usage.
    for together in [false,true] {
        let mut converter=Transcoder::new(Protocol::Chat,Protocol::Responses);
        let mut output=Vec::new();
        if together {output.extend(converter.feed(&frames.concat()).unwrap());}
        else {for bytes in &frames {output.extend(converter.feed(bytes).unwrap());}}
        let values=events(&output);
        let terminal=&values.iter().find(|v|v["type"]=="response.completed").unwrap()["response"];
        assert_eq!(terminal["request_id"],"req-stream");
        assert_eq!(terminal["usage"]["input_tokens"],100);
        assert_eq!(terminal["usage"]["output_tokens"],20);
        assert_eq!(terminal["usage"]["input_tokens_details"]["cache_write_tokens"],40);
        assert_eq!(terminal["usage"]["tokengo_raw_usage"]["usage"],kimi_usage());
        assert_eq!((converter.observer.input,converter.observer.output),(Some(100),Some(20)));
    }
}
#[test]
fn glm_sse_semantic_extensions_are_rejected() {
    for key in ["content_filter", "web_search", "video_result", "future_semantics"] {
        let mut value = json!({"choices":[{"index":0,"delta":{"content":"hello"}}]});
        value[key] = json!([]);
        assert!(Transcoder::new(Protocol::Chat, Protocol::Responses).feed(&frame(value)).is_err(), "{key}");
    }
}
#[test]
fn metadata_boundaries_missing_buckets_and_roundtrip_do_not_invent_usage() {
    let mut body = chat();
    body["request_id"] = json!("r".repeat(1025));
    assert!(convert_response(Protocol::Chat, Protocol::Responses, &body).is_err());
    body.as_object_mut().unwrap().remove("request_id");
    for details in [json!({"cached_tokens":30}), json!({"cache_write_tokens":40}), json!({})] {
        body["usage"] = json!({"prompt_tokens":100,"completion_tokens":20,"prompt_tokens_details":details});
        let converted = convert_response(Protocol::Chat, Protocol::Responses, &body).unwrap();
        for key in ["cached_tokens", "cache_write_tokens"] {
            assert_eq!(converted["usage"]["input_tokens_details"].get(key), details.get(key));
        }
        let roundtrip = convert_response(Protocol::Responses, Protocol::Chat, &converted).unwrap();
        assert_eq!(roundtrip["usage"]["tokengo_raw_usage"], converted["usage"]["tokengo_raw_usage"]);
    }
    for usage in [
        json!({"prompt_tokens":100,"completion_tokens":20,"cached_tokens":20,"prompt_tokens_details":{"cached_tokens":30}}),
        json!({"prompt_tokens":100,"completion_tokens":20,"prompt_tokens_details":{"cache_write_tokens":-1}}),
        json!({"prompt_tokens":100,"completion_tokens":20,"tokengo_raw_usage":{"protocol":"chat","usage":{"tokengo_raw_usage":{}}}}),
        json!({"prompt_tokens":100,"completion_tokens":20,"extra":"x".repeat(65536)}),
    ] {
        body["usage"] = usage;
        assert!(convert_response(Protocol::Chat, Protocol::Responses, &body).is_err());
        assert_eq!(convert_response(Protocol::Chat, Protocol::Chat, &body).unwrap(), body);
    }
    let mut nested = json!(0);
    for _ in 0..17 { nested = json!({"nested":nested}); }
    body["usage"] = json!({"prompt_tokens":100,"completion_tokens":20,"extra":nested});
    assert!(convert_response(Protocol::Chat, Protocol::Responses, &body).is_err());
    for raw in [json!({"protocol":"future","usage":{}}), json!({"protocol":"chat","usage":[]}), json!({"protocol":"chat","usage":{},"future":true})] {
        body["usage"] = json!({"prompt_tokens":100,"completion_tokens":20,"tokengo_raw_usage":raw});
        assert!(convert_response(Protocol::Chat, Protocol::Responses, &body).is_err());
    }
}
#[test]
fn sse_partial_subset_conflicts_and_trace_changes_fail_closed() {
    for tail in [
        json!({"choices":[],"request_id":"changed"}),
        json!({"choices":[],"usage":{"prompt_tokens_details":{"cached_tokens":60,"cache_write_tokens":41}}}),
        json!({"choices":[],"usage":{"completion_tokens_details":{"reasoning_tokens":21}}}),
    ] {
        let mut converter = Transcoder::new(Protocol::Chat, Protocol::Responses);
        converter.feed(&frame(json!({"request_id":"original","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":20}}))).unwrap();
        assert!(converter.feed(&frame(tail)).is_err());
        assert!(converter.finish().is_err());
    }
}
#[test]
fn raw_usage_size_and_depth_limits_are_stable_across_repeated_conversion() {
    let mut body = chat();
    let mut usage = kimi_usage(); usage["padding"] = json!("");
    let overhead = serde_json::to_vec(&usage).unwrap().len();
    usage["padding"] = json!("x".repeat(60 * 1024 - 64 - overhead));
    body["usage"] = usage.clone();
    let first = convert_response(Protocol::Chat, Protocol::Responses, &body).unwrap();
    let mut converted = first.clone();
    for (from,to) in [(Protocol::Responses,Protocol::Chat),(Protocol::Chat,Protocol::Responses),(Protocol::Responses,Protocol::Chat)] {
        converted = convert_response(from,to,&converted).unwrap();
        assert_eq!(converted["usage"]["tokengo_raw_usage"], first["usage"]["tokengo_raw_usage"]);
        assert!(serde_json::to_vec(&converted["usage"]).unwrap().len() < 65536);
    }
    body["usage"]["padding"] = json!(format!("{}x", usage["padding"].as_str().unwrap()));
    assert!(convert_response(Protocol::Chat,Protocol::Responses,&body).unwrap_err().contains("60 KiB"));
    let mut nested = json!(0);
    for _ in 0..11 { nested = json!({"nested":nested}); }
    body["usage"] = kimi_usage(); body["usage"]["deep"] = nested;
    let first = convert_response(Protocol::Chat,Protocol::Responses,&body).unwrap();
    assert!(convert_response(Protocol::Responses,Protocol::Chat,&first).is_ok());
    body["usage"]["deep"] = json!({"nested":body["usage"]["deep"].clone()});
    assert!(convert_response(Protocol::Chat,Protocol::Responses,&body).unwrap_err().contains("12"));
}
