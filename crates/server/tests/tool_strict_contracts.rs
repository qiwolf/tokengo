//! Conversion must preserve the source's function strict semantics, including omission.
use serde_json::{json, Value};
use tokengo_server::protocol::{convert_request, Protocol};

fn request(protocol: Protocol, schema: &Value, strict: Option<Value>) -> Value {
    let mut function = json!({"name":"lookup","parameters":schema});
    if let Some(strict) = strict { function["strict"] = strict; }
    match protocol {
        Protocol::Chat => json!({"model":"m","messages":[{"role":"user","content":"hi"}],"tools":[{"type":"function","function":function}]}),
        Protocol::Responses => { function["type"] = json!("function"); json!({"model":"m","input":"hi","tools":[function]}) }
        Protocol::Messages => unreachable!(),
    }
}
fn schemas() -> Vec<Value> {
    vec![
        json!({"type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":false}),
        json!({"type":"object","properties":{"optional":{"type":"string"}}}),
        // A schema outside a proven strict subset must not be guessed to fall back.
        json!({"type":"object","patternProperties":{"^x":{"type":"string"}},"additionalProperties":true}),
    ]
}
#[test]
fn omitted_chat_strict_is_explicitly_non_strict_in_responses_without_schema_changes() {
    for schema in schemas() {
        let body = request(Protocol::Chat, &schema, None);
        let before = body.clone();
        let converted = convert_request(Protocol::Chat, Protocol::Responses, &body).unwrap();
        assert_eq!(converted["tools"][0]["strict"], false);
        assert_eq!(converted["tools"][0]["parameters"], schema);
        assert_eq!(body, before);
    }
}
#[test]
fn omitted_responses_strict_requires_a_provably_strict_schema() {
    for schema in schemas() {
        let body = request(Protocol::Responses, &schema, None);
        let before = body.clone();
        if schema.get("required").is_some() {
            let converted = convert_request(Protocol::Responses, Protocol::Chat, &body).unwrap();
            assert_eq!(converted["tools"][0]["function"]["strict"], true);
            assert_eq!(converted["tools"][0]["function"]["parameters"], schema);
        } else {
            let error = convert_request(Protocol::Responses, Protocol::Chat, &body).unwrap_err();
            assert!(error.contains("strict"), "{error}");
        }
        assert!(convert_request(Protocol::Responses, Protocol::Messages, &body).is_err());
        assert_eq!(body, before);
    }
}
#[test]
fn explicit_boolean_strict_survives_both_directions_for_every_schema() {
    for schema in schemas() {
        for strict in [false, true] {
            for (from, to) in [(Protocol::Chat, Protocol::Responses), (Protocol::Responses, Protocol::Chat)] {
                let body = request(from, &schema, Some(json!(strict)));
                let converted = convert_request(from, to, &body).unwrap();
                let function = if to == Protocol::Chat { &converted["tools"][0]["function"] } else { &converted["tools"][0] };
                assert_eq!(function["strict"], strict);
                assert_eq!(function["parameters"], schema);
            }
        }
    }
}
#[test]
fn native_tools_preserve_all_three_strict_states_and_unknown_values() {
    for protocol in [Protocol::Chat, Protocol::Responses] {
        for strict in [None, Some(json!(true)), Some(json!(false)), Some(Value::Null), Some(json!("future"))] {
            let body = request(protocol, &schemas()[1], strict);
            assert_eq!(convert_request(protocol, protocol, &body).unwrap(), body);
        }
    }
}
#[test]
fn malformed_explicit_strict_is_rejected_cross_protocol() {
    for (from, to) in [(Protocol::Chat, Protocol::Responses), (Protocol::Responses, Protocol::Chat)] {
        for strict in [Value::Null, json!(1), json!("false")] {
            assert!(convert_request(from, to, &request(from, &schemas()[0], Some(strict))).is_err());
        }
    }
}
#[test]
fn messages_non_strict_tools_do_not_become_strict_in_responses() {
    let schema = schemas()[1].clone();
    let body = json!({"model":"m","max_tokens":32,"messages":[{"role":"user","content":"hi"}],"tools":[{"name":"lookup","input_schema":schema}]});
    let converted = convert_request(Protocol::Messages, Protocol::Responses, &body).unwrap();
    assert_eq!(converted["tools"][0]["strict"], false);
    assert_eq!(converted["tools"][0]["parameters"], schema);
}
#[test]
fn responses_omitted_strict_preserves_nested_nullable_objects_arrays_and_anyof() {
    let schema = json!({"type":"object","properties":{
        "nested":{"type":["object","null"],"properties":{"n":{"type":"integer"}},"required":["n"],"additionalProperties":false},
        "list":{"type":"array","items":{"anyOf":[{"type":"string"},{"type":"number"}]}},
        "choice":{"type":["string","null"],"enum":["a",null]}
    },"required":["nested","list","choice"],"additionalProperties":false});
    let converted = convert_request(Protocol::Responses, Protocol::Chat, &request(Protocol::Responses, &schema, None)).unwrap();
    assert_eq!(converted["tools"][0]["function"]["strict"], true);
    assert_eq!(converted["tools"][0]["function"]["parameters"], schema);
}
#[test]
fn responses_omitted_strict_never_guesses_unknown_fallback_or_rewrites_optionals() {
    for child in [
        json!({"type":"object","properties":{"optional":{"type":"string"}},"required":[],"additionalProperties":false}),
        json!({"type":"object","properties":{},"required":[],"additionalProperties":true}),
        json!({"type":"array"}), json!({"$ref":"#/$defs/item"}),
        json!({"type":"string","futureKeyword":true}), json!({"anyOf":[]}),
        json!({"type":"string","enum":[7]}), json!({"type":"integer","enum":[1.5]}),
        json!({"type":"string","enum":[null]}),
        json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x","x"],"additionalProperties":false}),
    ] {
        let schema = json!({"type":"object","properties":{"child":child},"required":["child"],"additionalProperties":false});
        let body = request(Protocol::Responses, &schema, None);
        assert!(convert_request(Protocol::Responses, Protocol::Chat, &body).is_err(), "{schema}");
        assert_eq!(body["tools"][0]["parameters"], schema);
    }
}
