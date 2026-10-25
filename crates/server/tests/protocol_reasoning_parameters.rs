use serde_json::{json, Value};
use tokengo_server::protocol::{convert_request, normalization_notes, Protocol};

fn chat() -> Value {
    json!({"model":"standard-model","messages":[{"role":"user","content":"hello"}]})
}
fn responses() -> Value {
    json!({"model":"standard-model","input":"hello"})
}

#[test]
fn active_effort_and_tier_round_trip_without_model_alias_or_case_changes() {
    for effort in [
        "low",
        "medium",
        "high",
        "xhigh",
        "max",
        "ultra",
        "vendor_future-2",
    ] {
        let mut body = chat();
        body["reasoning_effort"] = json!(effort);
        body["service_tier"] = json!("priority_future-2");
        let response = convert_request(Protocol::Chat, Protocol::Responses, &body).unwrap();
        assert_eq!(response["model"], "standard-model");
        assert_eq!(response["reasoning"]["effort"], effort);
        assert_eq!(response["service_tier"], "priority_future-2");
        let back = convert_request(Protocol::Responses, Protocol::Chat, &response).unwrap();
        assert_eq!(back["reasoning_effort"], effort);
        assert_eq!(back["service_tier"], "priority_future-2");
        assert_eq!(back["model"], "standard-model");
        assert!(
            !normalization_notes(Protocol::Responses, Protocol::Chat, &response)
                .iter()
                .any(|note| note.contains("disabled reasoning"))
        );
    }
}

#[test]
fn responses_active_effort_with_disabled_summary_is_representable() {
    let mut body = responses();
    body["reasoning"] = json!({"effort":"xhigh","summary":"none"});
    body["service_tier"] = json!("priority");
    body["include"] = json!([]);
    let result = convert_request(Protocol::Responses, Protocol::Chat, &body).unwrap();
    assert_eq!(result["reasoning_effort"], "xhigh");
    assert_eq!(result["service_tier"], "priority");
    assert!(
        !normalization_notes(Protocol::Responses, Protocol::Chat, &body)
            .iter()
            .any(|note| note.contains("disabled"))
    );
}

#[test]
fn disabled_responses_reasoning_preserves_explicit_none_without_inventing_an_effort() {
    for reasoning in [
        json!({}),
        json!({"effort":"none"}),
        json!({"summary":"none"}),
    ] {
        let mut body = responses();
        let explicit_none=reasoning.get("effort").and_then(Value::as_str)==Some("none");
        body["reasoning"] = reasoning;
        body["include"] = json!(["reasoning.encrypted_content"]);
        let result = convert_request(Protocol::Responses, Protocol::Chat, &body).unwrap();
        if explicit_none {assert_eq!(result["reasoning_effort"],"none","explicit off must survive protocol conversion");}
        else {assert!(result.get("reasoning_effort").is_none(),"an absent effort must not acquire a manufactured value");}
        assert!(result.get("include").is_none());
        assert!(
            normalization_notes(Protocol::Responses, Protocol::Chat, &body)
                .iter()
                .any(|note| note.contains("disabled reasoning"))
        );
    }
}

#[test]
fn opaque_reasoning_and_active_summary_still_cannot_cross_protocols() {
    for reasoning in [
        json!({"effort":"high","summary":"auto"}),
        json!({"summary":"detailed"}),
        json!({"effort":"high","unknown":1}),
    ] {
        let mut body = responses();
        body["reasoning"] = reasoning;
        assert!(convert_request(Protocol::Responses, Protocol::Chat, &body).is_err());
    }
    let mut body = responses();
    body["reasoning"] = json!({"effort":"high"});
    body["include"] = json!(["reasoning.encrypted_content"]);
    assert!(convert_request(Protocol::Responses, Protocol::Chat, &body).is_err());
    body["include"] = json!([]);
    body["input"] = json!([{"type":"reasoning","encrypted_content":"opaque"}]);
    assert!(convert_request(Protocol::Responses, Protocol::Chat, &body).is_err());
}

#[test]
fn messages_effort_tier_signature_and_budget_boundaries_remain_closed() {
    for field in ["reasoning_effort", "service_tier"] {
        let mut body = chat();
        body[field] = json!("high");
        assert!(convert_request(Protocol::Chat, Protocol::Messages, &body).is_err());
    }
    let mut body = responses();
    body["reasoning"] = json!({"effort":"high"});
    assert!(convert_request(Protocol::Responses, Protocol::Messages, &body).is_err());
    body = responses();
    body["service_tier"] = json!("priority");
    assert!(convert_request(Protocol::Responses, Protocol::Messages, &body).is_err());
    for extra in [
        json!({"output_config":{"effort":"high"}}),
        json!({"thinking":{"type":"enabled","budget_tokens":512}}),
    ] {
        let mut body =
            json!({"model":"m","messages":[{"role":"user","content":"hi"}],"max_tokens":16});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(convert_request(Protocol::Messages, Protocol::Responses, &body).is_err());
    }
}

#[test]
fn converted_parameter_tokens_are_bounded_and_native_payload_is_untouched() {
    for bad in [
        json!(""),
        json!("a".repeat(33)),
        json!("high value"),
        json!("HIGH"),
        json!("high\n"),
        json!("高"),
        json!(true),
        json!(null),
    ] {
        for field in ["reasoning_effort", "service_tier"] {
            let mut body = chat();
            body[field] = bad.clone();
            assert!(convert_request(Protocol::Chat, Protocol::Responses, &body).is_err());
            assert_eq!(
                convert_request(Protocol::Chat, Protocol::Chat, &body).unwrap(),
                body
            );
        }
        for reasoning in [true, false] {
            let mut body = responses();
            if reasoning {
                body["reasoning"] = json!({"effort":bad});
            } else {
                body["service_tier"] = bad.clone();
            }
            assert!(convert_request(Protocol::Responses, Protocol::Chat, &body).is_err());
            assert_eq!(
                convert_request(Protocol::Responses, Protocol::Responses, &body).unwrap(),
                body
            );
        }
    }
    let mut body = chat();
    body["reasoning_effort"] = json!("a".repeat(32));
    body["service_tier"] = json!("b".repeat(32));
    assert!(convert_request(Protocol::Chat, Protocol::Responses, &body).is_ok());
}

#[test]
fn tier_maps_independently_and_chat_none_is_not_rewritten() {
    for (from, to, mut body) in [
        (Protocol::Chat, Protocol::Responses, chat()),
        (Protocol::Responses, Protocol::Chat, responses()),
    ] {
        body["service_tier"] = json!("default");
        let converted = convert_request(from, to, &body).unwrap();
        assert_eq!(converted["service_tier"], "default");
        assert!(converted.get("reasoning").is_none());
        assert!(converted.get("reasoning_effort").is_none());
    }
    let mut body = chat();
    body["reasoning_effort"] = json!("none");
    assert_eq!(
        convert_request(Protocol::Chat, Protocol::Responses, &body).unwrap()["reasoning"]["effort"],
        "none"
    );
}


#[test]
fn chat_effort_object_alias_is_wire_equivalent_only_when_converting_to_responses() {
    for effort in ["none","low","medium","high","xhigh","vendor_future-2"] {
        let mut canonical=chat();canonical["reasoning_effort"]=json!(effort);
        let mut alias=chat();alias["reasoning"]=json!({"effort":effort});
        let original=alias.clone();
        let expected=convert_request(Protocol::Chat,Protocol::Responses,&canonical).unwrap();
        let actual=convert_request(Protocol::Chat,Protocol::Responses,&alias).unwrap();
        assert_eq!(actual,expected);
        assert_eq!(actual["reasoning"],json!({"effort":effort}));
        assert!(actual.get("reasoning_effort").is_none());
        assert_eq!(alias,original,"conversion must not modify the caller input");
        assert_eq!(convert_request(Protocol::Chat,Protocol::Chat,&alias).unwrap(),alias,"native Chat stays untouched");
        assert!(convert_request(Protocol::Chat,Protocol::Messages,&alias).is_err());
        assert!(normalization_notes(Protocol::Chat,Protocol::Responses,&alias).iter().any(|note|note.contains("compatibility alias")));
    }
}

#[test]
fn chat_effort_object_alias_merges_identical_canonical_values_and_rejects_conflicts() {
    let mut body=chat();body["reasoning"]=json!({"effort":"low"});body["reasoning_effort"]=json!("low");
    assert_eq!(convert_request(Protocol::Chat,Protocol::Responses,&body).unwrap()["reasoning"],json!({"effort":"low"}));
    body["reasoning_effort"]=json!("high");
    assert!(convert_request(Protocol::Chat,Protocol::Responses,&body).unwrap_err().contains("Conflicting reasoning_effort"));
    assert_eq!(convert_request(Protocol::Chat,Protocol::Chat,&body).unwrap(),body);
}

#[test]
fn chat_effort_alias_never_silently_discards_unknown_or_malformed_semantics() {
    let invalid=vec![
        json!(null),json!(true),json!("high"),json!([]),json!({}),
        json!({"effort":null}),json!({"effort":1}),json!({"effort":false}),
        json!({"effort":""}),json!({"effort":"HIGH"}),json!({"effort":"high value"}),
        json!({"effort":"high\n"}),json!({"effort":"x".repeat(33)}),
        json!({"effort":"high","summary":"none"}),json!({"effort":"high","summary":"auto"}),
        json!({"effort":"high","context":"all_turns"}),json!({"effort":"high","budget_tokens":1024}),
    ];
    for value in invalid {
        let mut body=chat();body["reasoning"]=value;
        assert!(convert_request(Protocol::Chat,Protocol::Responses,&body).is_err(),"{body}");
        assert_eq!(convert_request(Protocol::Chat,Protocol::Chat,&body).unwrap(),body,"native Chat passthrough is unchanged");
    }
    let mut native=responses();native["reasoning"]=json!({"effort":"high","summary":"auto"});
    native["include"]=json!(["reasoning.encrypted_content"]);
    assert_eq!(convert_request(Protocol::Responses,Protocol::Responses,&native).unwrap(),native);
    assert!(convert_request(Protocol::Responses,Protocol::Chat,&native).is_err(),"Responses summary/opaque protections remain closed");
}
