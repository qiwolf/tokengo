//! Explicit wire protocols. Native traffic retains opaque provider fields.
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod request;
mod response;
pub mod sse;
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
    fn native_keeps_opaque_fields() {
        let v = json!({"model":"m","thinking":{"signature":"opaque"},"unknown":[1]});
        for p in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
            assert_eq!(convert_request(p, p, &v).unwrap(), v);
            assert_eq!(convert_response(p, p, &v).unwrap(), v);
        }
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
            json!({"model":"m","input":"hello","reasoning":{"effort":"high"}}),
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
        assert!(convert_response(Protocol::Chat, Protocol::Responses, &thinking).is_err());
    }
}
