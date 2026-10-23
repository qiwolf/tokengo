//! Regression for the observed ChatGPT OAuth snapshot, scoped to buffered Chat.
use serde_json::{json, Value};
use tokengo_server::protocol::{convert_oauth_buffered_response, convert_response, Protocol};

fn observed_snapshot() -> Value {
    json!({
        "id":"resp_fixture", "object":"response", "created_at":1792738458,
        "status":"completed", "model":"gpt-6.1-sol",
        "access_programs":{"cyber":"standard"}, "completed_at":1792738459,
        "frequency_penalty":0.0, "presence_penalty":0.0, "moderation":null,
        "top_logprobs":0,
        "tool_usage":{"image_gen":{"input_tokens":0,"input_tokens_details":{"image_tokens":0,"text_tokens":0},"output_tokens":0,"output_tokens_details":{"image_tokens":0,"text_tokens":0},"total_tokens":0},"web_search":{"num_requests":0}},
        "reasoning":{"context":"all_turns","effort":"medium","mode":"standard","summary":null},
        "text":{"format":{"type":"text"},"verbosity":"medium"},
        "error":null, "incomplete_details":null,
        "output":[{"id":"msg_fixture","type":"message","role":"assistant","status":"completed","phase":"final_answer","content":[{"type":"output_text","text":"NATIVE_OK","annotations":[],"logprobs":[]}]}],
        "usage":{"input_tokens":17,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":7,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":24}
    })
}

#[test]
fn observed_metadata_is_preserved_without_inventing_reasoning_or_usage() {
    let body = observed_snapshot();
    let chat = convert_oauth_buffered_response(Protocol::Chat, &body).unwrap();
    assert_eq!(chat["choices"][0]["message"]["content"], "NATIVE_OK");
    assert!(chat["choices"][0]["message"]
        .get("reasoning_content")
        .is_none());
    assert_eq!(chat["usage"]["prompt_tokens"], 17);
    assert_eq!(chat["usage"]["completion_tokens"], 7);
    assert_eq!(chat["usage"]["total_tokens"], 24);
    assert_eq!(
        chat["usage"]["completion_tokens_details"]["reasoning_tokens"],
        0
    );
    let metadata = &chat["provider_metadata"]["openai_responses"];
    for key in [
        "access_programs",
        "completed_at",
        "frequency_penalty",
        "presence_penalty",
        "moderation",
        "top_logprobs",
        "tool_usage",
        "reasoning",
        "text",
    ] {
        assert_eq!(metadata.get(key), body.get(key), "{key}");
    }
    assert_eq!(
        metadata["output_message_phases"],
        json!([{"output_index":0,"id":"msg_fixture","phase":"final_answer"}])
    );
    assert_eq!(body["output"][0]["phase"], "final_answer");
}

#[test]
fn native_response_retains_every_provider_value_and_general_converter_stays_strict() {
    let mut body = observed_snapshot();
    body["future_provider_control"] = json!({"opaque":true});
    assert_eq!(
        convert_oauth_buffered_response(Protocol::Responses, &body).unwrap(),
        body
    );
    assert!(convert_response(Protocol::Responses, Protocol::Chat, &observed_snapshot()).is_err());
    assert!(convert_oauth_buffered_response(Protocol::Chat, &body).is_err());
    assert!(convert_oauth_buffered_response(Protocol::Messages, &observed_snapshot()).is_err());
}

#[test]
fn non_observed_control_or_meter_values_are_rejected() {
    for (pointer, value) in [
        ("/access_programs", json!({"cyber":"elevated"})),
        ("/completed_at", json!("1792738459")),
        ("/frequency_penalty", json!(1)),
        ("/presence_penalty", json!(-1)),
        ("/moderation", json!({"blocked":false})),
        ("/top_logprobs", json!(1)),
        ("/tool_usage/image_gen/input_tokens", json!(1)),
        ("/tool_usage/web_search/num_requests", json!(1)),
        ("/reasoning/context", json!("last_turn")),
        ("/reasoning/mode", json!("unknown")),
        ("/reasoning/summary", json!("auto")),
        ("/reasoning/effort", json!("unknown")),
        ("/text/verbosity", json!("unknown")),
        ("/text/format/type", json!("json_object")),
        ("/output/0/phase", json!("commentary")),
        ("/output/0/phase", Value::Null),
        ("/output/0/status", json!("in_progress")),
    ] {
        let mut body = observed_snapshot();
        *body.pointer_mut(pointer).unwrap() = value;
        assert!(
            convert_oauth_buffered_response(Protocol::Chat, &body).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn supported_generation_dials_are_echoes_without_fabricated_reasoning() {
    for effort in ["low", "medium", "high"] {
        for verbosity in ["low", "medium", "high"] {
            let mut body = observed_snapshot();
            body["reasoning"]["effort"] = json!(effort);
            body["text"]["verbosity"] = json!(verbosity);
            let chat = convert_oauth_buffered_response(Protocol::Chat, &body).unwrap();
            assert!(chat["choices"][0]["message"]
                .get("reasoning_content")
                .is_none());
            assert_eq!(
                chat["provider_metadata"]["openai_responses"]["reasoning"],
                body["reasoning"]
            );
            assert_eq!(
                chat["provider_metadata"]["openai_responses"]["text"],
                body["text"]
            );
        }
    }
}
