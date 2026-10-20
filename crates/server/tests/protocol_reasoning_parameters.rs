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
fn disabled_responses_reasoning_keeps_existing_cli_normalization_contract() {
    for reasoning in [
        json!({}),
        json!({"effort":"none"}),
        json!({"summary":"none"}),
    ] {
        let mut body = responses();
        body["reasoning"] = reasoning;
        body["include"] = json!(["reasoning.encrypted_content"]);
        let result = convert_request(Protocol::Responses, Protocol::Chat, &body).unwrap();
        assert!(result.get("reasoning_effort").is_none());
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
