use super::*;
use serde_json::json;
fn p(model: &str) -> Policy {
    policy(&Context { base_url: "https://api.minimax.io/v1", model, protocol: Protocol::Chat,
        api_version: None, is_oauth: false, profile_override: None }, Protocol::Responses).unwrap()
}
fn reply() -> Value { json!({"id":"c1","object":"chat.completion","created":1,"model":"MiniMax-M3","choices":[{"index":0,"message":{"role":"assistant","content":"答<think>字面</think>","reasoning_content":"原始思考"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":5,"total_tokens":8},"input_sensitive":false,"output_sensitive":false,"input_sensitive_type":0,"output_sensitive_type":0,"output_sensitive_int":0,"base_resp":{"status_code":0,"status_msg":""}}) }
#[test]
fn w09_policy_is_exact_source_model_protocol_not_brand_guess() {
    for (url,model,oauth,target,expected) in [
        ("https://api.minimax.io/v1","MiniMax-M3",false,Protocol::Responses,true),
        ("https://api.minimax.io/v1","MiniMax-M3.1-Flash",false,Protocol::Responses,true),
        ("https://api.minimax.io/v1","MiniMax-M3",false,Protocol::Chat,false),
        ("https://api.minimax.io/v1","MiniMax-M3",false,Protocol::Messages,false),
        ("https://api.minimax.io/v1","MiniMax-M3",true,Protocol::Responses,false),
        ("https://example.invalid/v1","MiniMax-M3",false,Protocol::Responses,false),
        ("https://api.minimax.io/v1","MiniMax-M2.7",false,Protocol::Responses,false),
    ] { assert_eq!(policy(&Context{base_url:url,model,protocol:Protocol::Chat,api_version:None,is_oauth:oauth,profile_override:None},target).is_some(),expected); }
}
#[test]
fn w09_request_selects_split_and_preserves_raw_tool_history() {
    let body=json!({"model":"alias","input":[{"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"原始"}]},{"type":"function_call","call_id":"tool1","name":"weather","arguments":"{}"},{"type":"function_call_output","call_id":"tool1","output":"sunny"}],"max_output_tokens":37,"thinking":{"type":"adaptive"}});
    let before=body.clone(); let converted=p("MiniMax-M3").convert_request(&body).unwrap();
    assert_eq!(body,before); assert_eq!(converted["reasoning_split"],true); assert_eq!(converted["thinking"]["type"],"adaptive");
    assert_eq!(converted["max_tokens"],37); assert_eq!(converted["messages"][0]["reasoning_content"],"原始");
    assert_eq!(converted["messages"][0]["tool_calls"][0]["id"],"tool1"); assert_eq!(converted["messages"][1]["tool_call_id"],"tool1");
}
#[test]
fn w09_request_rejects_false_opaque_summary_and_always_on_off() {
    for (extra,expected) in [(json!({"reasoning_split":false}),"reasoning_split=true"),(json!({"reasoning_split":"true"}),"reasoning_split=true"),(json!({"thinking":{"type":"adaptive","budget_tokens":20}}),"Unsupported field in conversion: budget_tokens")] {
        let mut body=json!({"model":"m","input":"hi"});for (k,v) in extra.as_object().unwrap(){body[k]=v.clone();}let error=p("MiniMax-M3").convert_request(&body).unwrap_err();assert!(error.contains(expected),"{error}");
    }
    let error=p("MiniMax-M3.1-Flash").convert_request(&json!({"model":"m","input":"hi","reasoning":{"effort":"none"}})).unwrap_err();assert!(error.contains("MiniMax-M3.1-Flash cannot disable thinking"),"{error}");
    for (item,expected) in [(json!({"type":"reasoning","summary":[{"type":"summary_text","text":"summary"}]}),"Reasoning summaries are not raw reasoning"),(json!({"type":"reasoning","encrypted_content":"sig"}),"Opaque reasoning cannot be converted"),(json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"r","signature":"sig"}]}),"Unsupported field in conversion: signature")] {
        let error=p("MiniMax-M3").convert_request(&json!({"model":"m","input":[item]})).unwrap_err();assert!(error.contains(expected),"{error}");
    }
}
#[test]
fn w09_off_intent_maps_only_on_m3_and_missing_thought_stays_missing() {
    let out=p("MiniMax-M3").convert_request(&json!({"model":"m","input":"hi","reasoning":{"effort":"none"}})).unwrap();
    assert_eq!(out["thinking"]["type"],"disabled"); assert!(out.get("reasoning_effort").is_none());
    let mut body=reply();body["choices"][0]["message"].as_object_mut().unwrap().remove("reasoning_content");
    let out=p("MiniMax-M3").convert_response(&body).unwrap();assert!(out["output"].as_array().unwrap().iter().all(|v|v["type"]!="reasoning"));
}
#[test]
fn w09_official_split_reply_preserves_diagnostics_raw_and_literal_answer() {
    let out=p("MiniMax-M3").convert_response(&reply()).unwrap();
    assert_eq!(out["output"][0]["type"],"reasoning");assert_eq!(out["output"][0]["content"][0]["text"],"原始思考");
    assert_eq!(out["output"][1]["content"][0]["text"],"答<think>字面</think>");
    assert_eq!(out["tokengo_provider_metadata"]["fields"]["input_sensitive"],false);
    assert_eq!(out["tokengo_provider_metadata"]["fields"]["base_resp"]["status_code"],0);
}
#[test]
fn w09_semantic_flags_provider_errors_and_unknown_state_are_not_metadata() {
    for (key,value) in [("input_sensitive",json!(true)),("output_sensitive",json!("false")),("input_sensitive_type",json!(1)),("base_resp",json!({"status_code":1008,"status_msg":"balance"})),("base_resp",json!({"status_code":0,"unknown":"state"})),("reasoning_details",json!([{"text":"x"}]))] {
        let mut body=reply();body[key]=value;assert!(p("MiniMax-M3").convert_response(&body).is_err());
    }
}
fn frame(value: Value)->Vec<u8>{format!("data: {}\n\n",value).into_bytes()}
#[test]
fn w09_stream_is_incremental_bounded_and_carries_provider_diagnostics() {
    let mut data=Vec::new();
    for thought in ["a","ab","ab"] {data.extend(frame(json!({"id":"c1","model":"MiniMax-M3","choices":[{"index":0,"delta":{"reasoning_content":thought},"finish_reason":null}],"input_sensitive":false})));}
    data.extend(frame(json!({"id":"c1","model":"MiniMax-M3","choices":[{"index":0,"delta":{"content":"答<think>字面</think>"},"finish_reason":null}]})));
    data.extend(frame(json!({"id":"c1","model":"MiniMax-M3","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":5,"total_tokens":8},"base_resp":{"status_code":0,"status_msg":""}})));
    data.extend_from_slice(b"data: [DONE]\n\n");
    let mut stream=p("MiniMax-M3").stream();let mut out=Vec::new();for bytes in data.chunks(1){out.extend(stream.feed(bytes).unwrap());}out.extend(stream.finish().unwrap());
    let text=String::from_utf8(out).unwrap();let mut thought=String::new();let mut completed=None;
    for line in text.lines().filter_map(|l|l.strip_prefix("data: ")) {let v:Value=serde_json::from_str(line).unwrap();if v["type"]=="response.reasoning_text.delta"{thought.push_str(v["delta"].as_str().unwrap());}if v["type"]=="response.completed"{completed=Some(v);}}
    assert_eq!(thought,"aabab");let completed=completed.unwrap();assert_eq!(completed["response"]["tokengo_provider_metadata"]["fields"]["base_resp"]["status_code"],0);
}
#[test]
fn w09_stream_late_sensitive_and_truncated_or_reordered_thought_fail_closed() {
    let mut stream=p("MiniMax-M3").stream();stream.feed(&frame(json!({"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}))).unwrap();
    assert!(stream.feed(&frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"output_sensitive":true}))).is_err());assert!(stream.finish().is_err());
    let mut stream=p("MiniMax-M3").stream();stream.feed(&frame(json!({"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}))).unwrap();
    assert!(stream.feed(&frame(json!({"choices":[{"index":0,"delta":{"reasoning_content":"late"},"finish_reason":null}]}))).is_err());
    let mut stream=p("MiniMax-M3").stream();stream.feed(b"data: {\"choices\":").unwrap();assert!(stream.finish().is_err());
}
#[test]
fn w09_provider_metadata_does_not_accept_opaque_nested_or_unbounded_values() {
    for (key,value) in [("base_resp",json!({"status_code":0,"status_msg":"x".repeat(65537)})),("output_sensitive_type",json!({"opaque":"x"})),("input_sensitive",Value::Null)] {
        let mut body=reply();body[key]=value;assert!(p("MiniMax-M3").convert_response(&body).is_err());
    }
    let mut body=reply();body["choices"][0]["message"]["signature"]=json!("opaque");assert!(p("MiniMax-M3").convert_response(&body).is_err());
}
#[test]
fn w09_controls_fold_to_single_system_as_documented_semantic_fold() {
    for content in [json!("high priority"),json!([{"type":"input_text","text":"high priority"}])] {
        let input=json!({"model":"m","input":[{"role":"developer","content":content},{"role":"user","content":"hi"}]});
        let out=p("MiniMax-M3").convert_request(&input).unwrap();assert_eq!(out["messages"][0],json!({"role":"system","content":"high priority"}));assert_eq!(input["input"][0]["role"],"developer");
    }
    let folded=p("MiniMax-M3").convert_request(&json!({"model":"m","instructions":"first","input":[{"role":"system","content":"second"},{"role":"developer","content":"third"},{"role":"developer","content":"fourth"},{"role":"user","content":"hi"}]})).unwrap();
    assert_eq!(folded["messages"][0],json!({"role":"system","content":"first\n\nsecond\n\nthird\n\nfourth"}));assert_eq!(folded["messages"].as_array().unwrap().len(),2);
    for (body,expected) in [
        (json!({"model":"m","input":[{"role":"user","content":"hi"},{"role":"developer","content":"late"}]}),"control message after conversation content"),
        (json!({"model":"m","input":[{"role":"developer","content":"d","id":"state"}]}),"Unsupported field in conversion: id"),
        (json!({"model":"m","input":[{"role":"developer","content":[{"type":"input_text","text":"d","annotations":[]}]}]}),"Unsupported field in conversion: annotations"),
        (json!({"model":"m","input":[{"role":"developer","content":[{"type":"input_image","image_url":"x"}]}]}),"Unsupported field in conversion: image_url"),
        (json!({"model":"m","input":[{"role":"developer","content":42}]}),"control content must be plain text"),
    ] {let error=p("MiniMax-M3").convert_request(&body).unwrap_err();assert!(error.starts_with("Unsupported ")&&error.contains(expected),"{error}");}
}
#[test]
fn w09_semantic_and_schema_errors_are_protocol_faults_but_actual_provider_error_is_not() {
    for (key,value) in [("output_sensitive",json!(true)),("input_sensitive_type",json!(9)),("output_sensitive",json!(0)),("base_resp",json!({"status_code":"0"})),("base_resp",json!({"status_code":0,"status_msg":[]}))] {
        let mut body=reply();body[key]=value;let error=p("MiniMax-M3").convert_response(&body).unwrap_err();assert!(error.starts_with("Unsupported "),"{error}");
    }
    let mut body=reply();body["base_resp"]=json!({"status_code":1008,"status_msg":"balance"});assert!(!p("MiniMax-M3").convert_response(&body).unwrap_err().starts_with("Unsupported "));
}
#[test]
fn w09_masked_empty_or_error_shapes_do_not_hide_semantic_policy_flags() {
    for masked in [json!({"choices":[],"output_sensitive":true}),json!({"error":{"type":"filtered"},"input_sensitive":true})] {
        let error=p("MiniMax-M3").convert_response(&masked).unwrap_err();assert!(error.starts_with("Unsupported MiniMax policy semantics:"),"{error}");
        let mut stream=p("MiniMax-M3").stream();let error=stream.feed(&frame(masked)).unwrap_err();assert!(error.starts_with("Unsupported MiniMax policy semantics:"),"{error}");assert_eq!(stream.finish().unwrap_err(),error);
    }
}
#[test]
fn w09_strict_intent_is_preserved_without_claiming_upstream_enforcement() {
    for strict in [Some(true),None,Some(false)] {
        let mut tool=json!({"type":"function","name":"weather","parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}});
        if let Some(strict)=strict {tool["strict"]=json!(strict);}
        let body=json!({"model":"m","input":"hi","tools":[tool]});let before=body.clone();
        let result=p("MiniMax-M3").convert_request(&body).unwrap();
        assert_eq!(result["tools"][0]["function"]["strict"],strict.unwrap_or(true));
        assert_eq!(result["tools"][0]["function"]["parameters"],body["tools"][0]["parameters"]);
        assert_eq!(body,before);
    }
    // Missing strict still cannot justify rewriting an open/optional schema.
    let ambiguous=json!({"model":"m","input":"hi","tools":[{"type":"function","name":"weather","parameters":{"type":"object","properties":{"optional":{"type":"string"}}}}]});
    assert!(p("MiniMax-M3").convert_request(&ambiguous).is_err());
}
