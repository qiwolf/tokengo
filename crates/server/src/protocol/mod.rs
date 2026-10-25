//! Explicit wire protocols. Native traffic retains opaque provider fields.
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod request;
mod response;
mod oauth_response;
mod sparse;
pub mod sse;
pub mod collect;
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Chat,
    Responses,
    Messages,
}
impl Protocol {
    pub fn path(self) -> &'static str {
        match self {
            Self::Chat => "chat/completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }
}
pub use request::{convert_request, has_image, normalization_notes};
pub use response::{convert_response, validate_response};
pub use oauth_response::convert_oauth_buffered_response;
/// Only the exact supported plaintext shape may bypass opaque-state ownership.
pub(crate) fn is_plain_reasoning(item: &Value) -> bool {
    request::plain_reasoning(item).is_ok()
}
fn object(v: &Value) -> Result<&serde_json::Map<String, Value>, String> {
    v.as_object().ok_or_else(|| "Expected JSON object".into())
}
fn fields(v: &Value, allowed: &[&str]) -> Result<(), String> {
    for name in object(v)?.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(format!("Unsupported field in conversion: {name}"));
        }
    }
    Ok(())
}
fn string(v: &Value, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("Expected string {key}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn chat_response_format_maps_to_responses_without_changing_schema() {
        let schema = json!({"type":"object","properties":{"label":{"type":"string","enum":["中文","ok"]}},"required":["label"],"additionalProperties":false,"$defs":{"nested":{"type":"integer"}}});
        for strict in [None, Some(json!(false)), Some(json!(true)), Some(Value::Null)] {
            let mut format = json!({"type":"json_schema","json_schema":{"name":"hs_extract","description":"HS extraction","schema":schema}});
            if let Some(strict) = strict { format["json_schema"]["strict"] = strict; }
            let body = json!({"model":"m","messages":[{"role":"user","content":"Extract JSON"}],"response_format":format});
            let converted = convert_request(Protocol::Chat, Protocol::Responses, &body).unwrap();
            let mut expected = format["json_schema"].clone();
            expected["type"] = json!("json_schema");
            assert_eq!(converted["text"]["format"], expected);
            assert!(converted.get("response_format").is_none());
            assert_eq!(body["response_format"], format);
        }
    }
    #[test]
    fn chat_response_format_maps_text_and_json_object() {
        for kind in ["text", "json_object"] {
            let body = json!({"model":"m","messages":[{"role":"user","content":"JSON please"}],"response_format":{"type":kind}});
            let converted = convert_request(Protocol::Chat, Protocol::Responses, &body).unwrap();
            assert_eq!(converted["text"], json!({"format":{"type":kind}}));
        }
    }
    #[test]
    fn chat_response_format_validates_schema_name_boundaries() {
        for (name, valid) in [
            ("a".repeat(64), true),
            ("A0_-".into(), true),
            (String::new(), false),
            ("a".repeat(65), false),
            ("has space".into(), false),
            ("中文".into(), false),
        ] {
            let body = json!({"model":"m","messages":[{"role":"user","content":"JSON"}],"response_format":{"type":"json_schema","json_schema":{"name":name,"schema":{}}}});
            assert_eq!(convert_request(Protocol::Chat, Protocol::Responses, &body).is_ok(), valid, "{name}");
        }
    }
    #[test]
    fn chat_response_format_rejects_lossy_or_malformed_conversion() {
        for format in [
            json!(null), json!({"type":"unknown"}),
            json!({"type":"json_object","schema":{}}),
            json!({"type":"json_schema"}),
            json!({"type":"json_schema","json_schema":{"name":"x"}}),
            json!({"type":"json_schema","json_schema":{"name":7,"schema":{}}}),
            json!({"type":"json_schema","json_schema":{"name":"x","schema":[],"strict":true}}),
            json!({"type":"json_schema","json_schema":{"name":"x","schema":{},"strict":"true"}}),
            json!({"type":"json_schema","json_schema":{"name":"x","schema":{},"description":7}}),
            json!({"type":"json_schema","json_schema":{"name":"x","schema":{},"future_control":true}}),
        ] {
            let body = json!({"model":"m","messages":[{"role":"user","content":"hi"}],"response_format":format});
            assert!(convert_request(Protocol::Chat, Protocol::Responses, &body).is_err(), "{format}");
            // Native traffic must still preserve provider-specific payloads.
            assert_eq!(convert_request(Protocol::Chat, Protocol::Chat, &body).unwrap(), body);
        }
        let body = json!({"model":"m","messages":[{"role":"user","content":"hi"}],"response_format":{"type":"json_schema","json_schema":{"name":"x","schema":{}}}});
        assert!(convert_request(Protocol::Chat, Protocol::Messages, &body).is_err());
    }
    fn parameter_test_request(protocol: Protocol) -> Value {
        match protocol {
            Protocol::Chat => json!({"model":"m","messages":[{"role":"user","content":"hello"}]}),
            Protocol::Responses => json!({"model":"m","input":"hello"}),
            Protocol::Messages => json!({"model":"m","max_tokens":32,"messages":[{"role":"user","content":"hello"}]}),
        }
    }
    #[test]
    fn structured_format_roundtrips_keep_schema_controls_and_explicit_null() {
        let schema = json!({"type":"object","properties":{"a":{"type":["string","null"]}},"required":["a"],"additionalProperties":false,"$defs":{"nested":{"const":"中文"}}});
        for strict in [None, Some(json!(false)), Some(json!(true)), Some(Value::Null)] {
            let mut format = json!({"type":"json_schema","json_schema":{"name":"portable_schema","description":"","schema":schema}});
            if let Some(strict) = strict { format["json_schema"]["strict"] = strict; }
            let mut chat = parameter_test_request(Protocol::Chat);
            chat["response_format"] = format.clone();
            let original = chat.clone();
            let response = convert_request(Protocol::Chat, Protocol::Responses, &chat).unwrap();
            let roundtrip = convert_request(Protocol::Responses, Protocol::Chat, &response).unwrap();
            assert_eq!(roundtrip["response_format"], format);
            assert_eq!(chat, original);
            let repeated = convert_request(Protocol::Chat, Protocol::Responses, &roundtrip).unwrap();
            assert_eq!(repeated["text"], response["text"]);
        }
        for kind in ["text", "json_object"] {
            let mut response = parameter_test_request(Protocol::Responses);
            response["text"] = json!({"format":{"type":kind}});
            let chat = convert_request(Protocol::Responses, Protocol::Chat, &response).unwrap();
            assert_eq!(chat["response_format"], json!({"type":kind}));
            assert_eq!(convert_request(Protocol::Chat, Protocol::Responses, &chat).unwrap()["text"], response["text"]);
        }
    }
    #[test]
    fn responses_text_rejects_unrepresentable_nested_controls_without_mutating_input() {
        for text in [
            Value::Null, json!({"format":null}), json!({"format":{"type":"future"}}),
            json!({"format":{"type":"text","future":1}}),
            json!({"format":{"type":"json_schema","name":"x","schema":{},"strict":"true"}}),
            json!({"format":{"type":"json_schema","name":"x","schema":{},"description":null}}),
            json!({"format":{"type":"json_schema","name":"x","schema":{},"future":true}}),
            json!({"format":{"type":"json_schema","name":"x","schema":[]}}),
            json!({"verbosity":"ultra"}), json!({"future":true}),
        ] {
            let mut body = parameter_test_request(Protocol::Responses);
            body["text"] = text;
            let original = body.clone();
            assert!(convert_request(Protocol::Responses, Protocol::Chat, &body).is_err(), "{body}");
            assert_eq!(body, original);
            assert_eq!(convert_request(Protocol::Responses, Protocol::Responses, &body).unwrap(), original);
        }
    }
    #[test]
    fn verbosity_maps_both_directions_without_inventing_format_or_default() {
        for value in [json!("low"), json!("medium"), json!("high"), Value::Null] {
            let mut chat = parameter_test_request(Protocol::Chat);
            chat["verbosity"] = value.clone();
            let response = convert_request(Protocol::Chat, Protocol::Responses, &chat).unwrap();
            assert_eq!(response["text"].get("verbosity"), Some(&value));
            assert!(response["text"].get("format").is_none());
            let converted = convert_request(Protocol::Responses, Protocol::Chat, &response).unwrap();
            assert_eq!(converted.get("verbosity"), Some(&value));
            assert!(converted.get("response_format").is_none());
        }
        let body = parameter_test_request(Protocol::Chat);
        assert!(convert_request(Protocol::Chat, Protocol::Responses, &body).unwrap().get("text").is_none());
        for value in [json!(false), json!(0), json!("HIGH"), json!({}), json!([])] {
            let mut body = parameter_test_request(Protocol::Chat);
            body["verbosity"] = value.clone();
            assert!(convert_request(Protocol::Chat, Protocol::Responses, &body).is_err());
            let mut response = parameter_test_request(Protocol::Responses);
            response["text"] = json!({"verbosity":value});
            assert!(convert_request(Protocol::Responses, Protocol::Chat, &response).is_err());
        }
    }
    #[test]
    fn format_and_verbosity_coexist_and_invalid_alias_shapes_do_not_override() {
        let mut body = parameter_test_request(Protocol::Chat);
        body["response_format"] = json!({"type":"json_object"});
        body["verbosity"] = json!("low");
        let response = convert_request(Protocol::Chat, Protocol::Responses, &body).unwrap();
        assert_eq!(response["text"], json!({"format":{"type":"json_object"},"verbosity":"low"}));
        body["text"] = json!({"format":{"type":"text"},"verbosity":"high"});
        assert!(convert_request(Protocol::Chat, Protocol::Responses, &body).is_err());
        let mut response = response;
        response["response_format"] = json!({"type":"text"});
        assert!(convert_request(Protocol::Responses, Protocol::Chat, &response).is_err());
    }
    #[test]
    fn same_semantic_identity_and_cache_controls_survive_roundtrip_independently() {
        for controls in [
            json!({"user":"legacy-user","safety_identifier":"safe-hash","prompt_cache_key":"cache-bucket","prompt_cache_retention":"24h","prompt_cache_options":{"mode":"explicit","ttl":"12h"}}),
            json!({"user":null,"safety_identifier":null,"prompt_cache_key":null,"prompt_cache_retention":null,"prompt_cache_options":{}}),
            json!({"prompt_cache_retention":"in_memory","prompt_cache_options":{"mode":"implicit","ttl":"30m"}}),
        ] {
            for (from, to) in [(Protocol::Chat, Protocol::Responses), (Protocol::Responses, Protocol::Chat)] {
                let mut body = parameter_test_request(from);
                for (name, value) in controls.as_object().unwrap() { body[name] = value.clone(); }
                let original = body.clone();
                let converted = convert_request(from, to, &body).unwrap();
                let roundtrip = convert_request(to, from, &converted).unwrap();
                for (name, value) in controls.as_object().unwrap() {
                    assert_eq!(converted.get(name), Some(value));
                    assert_eq!(roundtrip.get(name), Some(value));
                }
                assert_eq!(body, original);
            }
        }
    }
    #[test]
    fn shared_controls_validate_shapes_but_do_not_rename_retention_into_ttl() {
        for controls in [
            json!({"safety_identifier":7}), json!({"user":{}}),
            json!({"prompt_cache_key":"x".repeat(513)}), json!({"prompt_cache_key":"unsafe\nkey"}),
            json!({"prompt_cache_options":null}), json!({"prompt_cache_options":{"ttl":"24h"}}),
            json!({"prompt_cache_options":{"mode":"future"}}), json!({"prompt_cache_options":{"mode":null}}),
            json!({"prompt_cache_options":{"ttl":null}}), json!({"prompt_cache_options":{"prewarm":false}}),
            json!({"prompt_cache_options":{"comparison_response_id":"resp_123"}}),
            json!({"prompt_cache_options":{"future":1}}), json!({"prompt_cache_retention":"12h"}),
        ] {
            for (from,to) in [(Protocol::Chat,Protocol::Responses),(Protocol::Responses,Protocol::Chat)] {
                let mut body = parameter_test_request(from);
                for (key,value) in controls.as_object().unwrap() {body[key]=value.clone();}
                let original = body.clone();
                assert!(convert_request(from,to,&body).is_err(), "{body}");
                assert_eq!(body,original);
                assert_eq!(convert_request(from,from,&body).unwrap(),original);
            }
        }
        let mut body = parameter_test_request(Protocol::Chat);
        body["prompt_cache_retention"] = json!("24h");
        let converted = convert_request(Protocol::Chat,Protocol::Responses,&body).unwrap();
        assert_eq!(converted["prompt_cache_retention"],"24h");
        assert!(converted.get("prompt_cache_options").is_none());
    }
    #[test]
    fn new_controls_have_explicit_dispositions_for_all_six_protocol_pairs() {
        for (from,to) in [
            (Protocol::Chat,Protocol::Responses),(Protocol::Chat,Protocol::Messages),
            (Protocol::Responses,Protocol::Chat),(Protocol::Responses,Protocol::Messages),
            (Protocol::Messages,Protocol::Chat),(Protocol::Messages,Protocol::Responses),
        ] {
            for (name,value) in [("user",json!("u")),("safety_identifier",Value::Null),("prompt_cache_key",json!("k")),("prompt_cache_retention",json!("24h")),("prompt_cache_options",json!({"mode":"explicit","ttl":"30m"}))] {
                let mut body = parameter_test_request(from);
                body[name] = value;
                let converted = convert_request(from,to,&body);
                if from==Protocol::Messages || to==Protocol::Messages {
                    assert!(converted.is_err(),"{from:?}->{to:?}: {name}");
                } else {assert_eq!(converted.unwrap()[name],body[name]);}
            }
            let mut body=parameter_test_request(from);
            match from {
                Protocol::Responses=>body["text"]=json!({"verbosity":"low"}),
                _=>body["verbosity"]=json!("low"),
            }
            assert_eq!(convert_request(from,to,&body).is_ok(),from!=Protocol::Messages&&to!=Protocol::Messages);
        }
    }
    #[test]
    fn native_keeps_opaque_fields() {
        let v = json!({"model":"m","thinking":{"signature":"opaque"},"unknown":[1]});
        for p in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            assert_eq!(convert_request(p, p, &v).unwrap(), v);
            assert_eq!(convert_response(p, p, &v).unwrap(), v);
        }
    }
    #[test]
    fn converted_chat_stream_requests_real_usage_but_native_stream_is_unchanged() {
        for (source,body) in [(Protocol::Responses,json!({"model":"m","input":"hello","stream":true})),(Protocol::Messages,json!({"model":"m","messages":[{"role":"user","content":"hello"}],"max_tokens":31,"stream":true}))] {
            let converted=convert_request(source,Protocol::Chat,&body).unwrap();assert_eq!(converted["stream_options"]["include_usage"],true);
        }
        let native=json!({"model":"m","messages":[{"role":"user","content":"hello"}],"stream":true});assert_eq!(convert_request(Protocol::Chat,Protocol::Chat,&native).unwrap(),native);
    }
    #[test]
    fn text_tools_and_results_round_trip_all_protocol_pairs() {
        let chat = json!({"model":"m","max_tokens":200,"messages":[{"role":"system","content":"rules"},{"role":"user","content":"compute"},{"role":"assistant","content":"working","tool_calls":[{"id":"call_a","type":"function","function":{"name":"sum","arguments":"{\"a\":1}"}},{"id":"call_b","type":"function","function":{"name":"other","arguments":"{}"}}]},{"role":"tool","tool_call_id":"call_a","content":"2"},{"role":"tool","tool_call_id":"call_b","content":"3"},{"role":"assistant","content":"done"}],"tools":[{"type":"function","function":{"name":"sum","parameters":{"type":"object","properties":{}}}}],"tool_choice":"auto"});
        for target in [Protocol::Responses, Protocol::Messages] {
            let converted = convert_request(Protocol::Chat, target, &chat).unwrap();
            let back = convert_request(target, Protocol::Chat, &converted).unwrap();
            assert_eq!(back["messages"], chat["messages"], "{target:?}");
            assert_eq!(back["tools"], chat["tools"]);
        }
    }
    #[test]
    fn unknown_state_images_and_thinking_fail_explicitly() {
        for body in [
            json!({"model":"m","input":"hello","previous_response_id":"resp1"}),
            json!({"model":"m","input":"hello","conversation":"conv"}),
            json!({"model":"m","input":"hello","reasoning":{"summary":"auto"}}),
            json!({"model":"m","input":"hello","surprise":1}),
        ] {
            assert!(convert_request(Protocol::Responses, Protocol::Chat, &body).is_err());
        }
        assert!(convert_request(Protocol::Messages,Protocol::Chat,&json!({"model":"m","messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"secret","signature":"sig"}]}]})).is_err());
        assert!(has_image(
            Protocol::Responses,
            &json!({"input":[{"role":"user","content":[{"type":"input_image","image_url":"https://image"}]}]})
        ));
    }
    #[test]
    fn native_response_shapes_are_protocol_specific() {
        assert!(validate_response(
            Protocol::Messages,
            &json!({"type":"message","role":"assistant","content":[{"type":"text","text":"ok"}]})
        )
        .is_ok());
        assert!(validate_response(Protocol::Responses,&json!({"object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok","annotations":[]}]}]})).is_ok());
        assert!(validate_response(Protocol::Responses, &json!({"choices":[]})).is_err());
    }
    #[test]
    fn nonstream_tools_usage_and_finish_convert() {
        let chat = json!({"id":"r","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_a","type":"function","function":{"name":"sum","arguments":"{\"a\":1}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":3}});
        for p in [Protocol::Responses, Protocol::Messages] {
            let v = convert_response(Protocol::Chat, p, &chat).unwrap();
            validate_response(p, &v).unwrap();
            let back = convert_response(p, Protocol::Chat, &v).unwrap();
            assert_eq!(
                back["choices"][0]["message"]["tool_calls"],
                chat["choices"][0]["message"]["tool_calls"]
            );
            assert_eq!(back["usage"]["prompt_tokens"], 10);
            assert_eq!(back["usage"]["completion_tokens"], 3);
            assert_eq!(back["choices"][0]["finish_reason"], "tool_calls");
        }
    }
}
#[cfg(test)]
mod boundary_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn converted_responses_are_stateless_and_tool_item_ids_stable() {
        let chat = json!({"model":"m","messages":[{"role":"user","content":"hello"}]});
        let converted = convert_request(Protocol::Chat, Protocol::Responses, &chat).unwrap();
        assert_eq!(converted["store"], false);
        let body = json!({"id":"r","model":"m","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]});
        let a = convert_response(Protocol::Chat, Protocol::Responses, &body).unwrap();
        let b = convert_response(Protocol::Chat, Protocol::Responses, &body).unwrap();
        assert_eq!(a, b);
        assert!(a["output"][0]["id"].as_str().unwrap().starts_with("msg_"));
        assert_eq!(a["output"][0]["content"][0]["annotations"], json!([]));
    }
    #[test]
    fn truncated_response_and_usage_zero_keep_their_meaning() {
        let response = json!({"object":"response","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"id":"msg_a","type":"message","status":"incomplete","role":"assistant","content":[{"type":"output_text","text":"partial","annotations":[]}]}],"usage":{"input_tokens":0,"output_tokens":3}});
        let c = convert_response(Protocol::Responses, Protocol::Chat, &response).unwrap();
        assert_eq!(c["choices"][0]["finish_reason"], "length");
        assert_eq!(c["usage"]["prompt_tokens"], 0);
        let m = convert_response(Protocol::Responses, Protocol::Messages, &response).unwrap();
        assert_eq!(m["stop_reason"], "max_tokens");
    }
    #[test]
    fn unsupported_semantics_are_never_silently_dropped() {
        for (source, body) in [
            (
                Protocol::Chat,
                json!({"model":"m","messages":[],"instructions":"must not disappear"}),
            ),
            (
                Protocol::Messages,
                json!({"model":"m","messages":[],"thinking":{"type":"enabled","budget_tokens":1000}}),
            ),
            (
                Protocol::Responses,
                json!({"model":"m","input":"hello","tools":[{"type":"web_search"}]}),
            ),
        ] {
            assert!(convert_request(
                source,
                if source == Protocol::Chat {
                    Protocol::Messages
                } else {
                    Protocol::Chat
                },
                &body
            )
            .is_err());
        }
        let response = json!({"type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":3,"cache_read_input_tokens":100}});
        assert!(convert_response(Protocol::Messages, Protocol::Chat, &response).is_err());
    }
    #[test]
    fn text_and_tools_convert_directly_between_responses_and_messages() {
        let input = json!({"model":"m","max_output_tokens":20,"input":[{"role":"user","content":[{"type":"input_text","text":"hello"}]},{"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"},{"type":"function_call_output","call_id":"call_1","output":"result"}]});
        let m = convert_request(Protocol::Responses, Protocol::Messages, &input).unwrap();
        assert_eq!(m["messages"][1]["content"][0]["id"], "call_1");
        let r = convert_request(Protocol::Messages, Protocol::Responses, &m).unwrap();
        assert_eq!(r["input"][1]["call_id"], "call_1");
        assert_eq!(r["input"][2]["call_id"], "call_1");
    }
}
#[cfg(test)]
mod native_validation_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn chat_preserves_legacy_tools_multimodal_reasoning_and_refusals() {
        for message in [
            json!({"role":"assistant","content":null,"function_call":{"name":"read","arguments":"{}"}}),
            json!({"role":"assistant","content":[{"type":"text","text":"ok"}]}),
            json!({"role":"assistant","content":null,"reasoning":"native reasoning"}),
            json!({"role":"assistant","content":null,"refusal":"cannot comply"}),
            json!({"role":"assistant","content":null,"reasoning_details":[{"type":"encrypted","data":"opaque"}]}),
        ] {
            let body = json!({"choices":[{"message":message}]});
            assert!(validate_response(Protocol::Chat, &body).is_ok(), "{body}");
            assert_eq!(
                convert_response(Protocol::Chat, Protocol::Chat, &body).unwrap(),
                body
            );
        }
    }
    #[test]
    fn native_responses_require_completed_actual_payload() {
        for body in [
            json!({"object":"response","status":"completed","output":[]}),
            json!({"object":"response","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":""}]}]}),
            json!({"object":"response","status":"incomplete","output":[{"type":"message","content":[{"type":"output_text","text":"partial"}]}]}),
        ] {
            assert!(
                validate_response(Protocol::Responses, &body).is_err(),
                "{body}"
            );
        }
        for item in [
            json!({"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}),
            json!({"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"declined"}]}),
            json!({"type":"reasoning","encrypted_content":"opaque"}),
            json!({"type":"future_provider_item","opaque_payload":{"value":"kept"}}),
        ] {
            let body = json!({"object":"response","status":"completed","output":[item]});
            assert!(
                validate_response(Protocol::Responses, &body).is_ok(),
                "{body}"
            );
            assert_eq!(
                convert_response(Protocol::Responses, Protocol::Responses, &body).unwrap(),
                body
            );
        }
    }
    #[test]
    fn native_empty_chat_and_messages_are_not_success() {
        for body in [
            json!({"choices":[{"message":{"content":" "}}]}),
            json!({"choices":[{"message":{"content":[]}}]}),
        ] {
            assert!(validate_response(Protocol::Chat, &body).is_err());
        }
        assert!(validate_response(
            Protocol::Messages,
            &json!({"type":"message","role":"assistant","content":[{"type":"text","text":""}]})
        )
        .is_err());
    }
}
#[cfg(test)]
mod image_bound_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn image_detection_includes_native_tool_results() {
        assert!(has_image(
            Protocol::Messages,
            &json!({"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":[{"type":"image","source":{"type":"url","url":"https://example/image"}}]}]}]})
        ));
        assert!(has_image(
            Protocol::Responses,
            &json!({"input":[{"type":"function_call_output","call_id":"t","output":[{"type":"input_image","image_url":"https://example/image"}]}]})
        ));
        assert!(!has_image(
            Protocol::Chat,
            &json!({"messages":[{"role":"assistant","tool_calls":[{"function":{"arguments":"{\"type\":\"image\"}"}}]}]})
        ));
    }
}
#[cfg(test)]
mod llama_conversion_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn llama_timings_do_not_change_portable_text_or_token_usage() {
        let body = json!({"id":"llama_chat","object":"chat.completion","created":1,"model":"local","timings":{"prompt_n":7,"predicted_n":3,"predicted_ms":24.5},"choices":[{"index":0,"message":{"role":"assistant","content":"本地回复"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}});
        let response = convert_response(Protocol::Chat, Protocol::Responses, &body).unwrap();
        assert_eq!(response["output"][0]["content"][0]["text"], "本地回复");
        assert_eq!(response["usage"]["input_tokens"], 7);
        assert_eq!(response["usage"]["output_tokens"], 3);
        let message = convert_response(Protocol::Chat, Protocol::Messages, &body).unwrap();
        assert_eq!(message["content"][0]["text"], "本地回复");
        let mut thinking = body;
        thinking["choices"][0]["message"]["reasoning_content"] = json!("nonempty native reasoning");
        let converted=convert_response(Protocol::Chat, Protocol::Responses, &thinking).unwrap();
        assert_eq!(converted["output"][0]["content"][0]["text"], "nonempty native reasoning");
        assert!(convert_response(Protocol::Chat, Protocol::Messages, &thinking).is_err());
    }
}

#[cfg(test)]
mod explicit_disabled_reasoning_conversion_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn responses_explicit_none_survives_chat_roundtrip_without_becoming_default() {
        let body = json!({"model":"unknown-source","input":"hello","reasoning":{"effort":"none"}});
        let chat = convert_request(Protocol::Responses, Protocol::Chat, &body).unwrap();
        assert_eq!(chat["reasoning_effort"], "none");
        let responses = convert_request(Protocol::Chat, Protocol::Responses, &chat).unwrap();
        assert_eq!(responses["reasoning"]["effort"], "none");
        let omitted = json!({"model":"unknown-source","input":"hello"});
        assert!(convert_request(Protocol::Responses, Protocol::Chat, &omitted).unwrap().get("reasoning_effort").is_none());
        assert_eq!(convert_request(Protocol::Responses, Protocol::Responses, &body).unwrap(), body);
    }

    #[test]
    fn messages_explicit_none_is_preserved_for_both_targets_and_active_effort_still_rejected() {
        let body = json!({"model":"unknown-source","max_tokens":16,"messages":[{"role":"user","content":"hello"}],"output_config":{"effort":"none","format":{"type":"text"}}});
        let chat = convert_request(Protocol::Messages, Protocol::Chat, &body).unwrap();
        assert_eq!(chat["reasoning_effort"], "none");
        let responses = convert_request(Protocol::Messages, Protocol::Responses, &body).unwrap();
        assert_eq!(responses["reasoning"]["effort"], "none");
        assert_eq!(responses["text"]["format"], json!({"type":"text"}));
        assert_eq!(convert_request(Protocol::Messages, Protocol::Messages, &body).unwrap(), body);
        let mut active = body;
        active["output_config"]["effort"] = json!("high");
        for target in [Protocol::Chat, Protocol::Responses] {
            assert!(convert_request(Protocol::Messages, target, &active).is_err());
        }
    }
}
