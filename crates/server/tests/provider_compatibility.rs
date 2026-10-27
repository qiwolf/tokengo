//! Versioned, synthetic provider shapes. These exercise the production converter;
//! they neither prove a supplier account nor replace HTTP ownership/billing tests.
use serde_json::Value;
use std::path::PathBuf;
use tokengo_server::protocol::{convert_response, sse::Transcoder, Protocol};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider_compatibility/v1")
}
fn events(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes).unwrap().lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok()).collect()
}

#[test]
fn versioned_provider_shapes_are_chunk_boundary_invariant() {
    let manifest: Value = serde_json::from_slice(&std::fs::read(root().join("manifest.json")).unwrap()).unwrap();
    for case in manifest["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let wire = std::fs::read(root().join(case["file"].as_str().unwrap())).unwrap();
        for size in [1, 7, wire.len()] {
            let mut transcoder = Transcoder::new(Protocol::Chat, Protocol::Responses);
            let mut output = Vec::new();
            let mut error = None;
            for chunk in wire.chunks(size) {
                match transcoder.feed(chunk) {
                    Ok(bytes) => output.extend(bytes),
                    Err(reason) => { error = Some(reason); break; }
                }
            }
            if error.is_none() {
                match transcoder.finish() {
                    Ok(bytes) => output.extend(bytes),
                    Err(reason) => error = Some(reason),
                }
            }
            let expected = &case["expected"];
            let label = format!("{id}, chunk={size}");
            let values = events(&output);
            let terminals: Vec<_> = values.iter().filter(|v| matches!(v["type"].as_str(), Some("response.completed" | "response.incomplete"))).collect();
            if expected["result"] == "error" {
                assert!(error.is_some(), "{label}: malformed source became successful");
                assert!(terminals.is_empty(), "{label}: fabricated successful terminal");
                continue;
            }
            assert!(error.is_none(), "{label}: {error:?}");
            if size == wire.len() {
                if let Some(directory) = std::env::var_os("TOKENGO_PROVIDER_FIXTURE_EXPORT") {
                    let directory = PathBuf::from(directory);
                    std::fs::create_dir_all(&directory).unwrap();
                    std::fs::write(directory.join(format!("{id}.responses.sse")), &output).unwrap();
                    let response = &terminals[0]["response"];
                    std::fs::write(directory.join(format!("{id}.responses.json")), serde_json::to_vec(response).unwrap()).unwrap();
                    let mut reverse = Transcoder::new(Protocol::Responses, Protocol::Chat);
                    let mut chat_stream = reverse.feed(&output).unwrap();
                    chat_stream.extend(reverse.finish().unwrap());
                    std::fs::write(directory.join(format!("{id}.chat.sse")), chat_stream).unwrap();
                    let chat = convert_response(Protocol::Responses, Protocol::Chat, response).unwrap();
                    std::fs::write(directory.join(format!("{id}.chat.json")), serde_json::to_vec(&chat).unwrap()).unwrap();
                }
            }
            assert_eq!(terminals.len(), 1, "{label}: terminal count");
            let terminal = &terminals[0]["response"];
            assert_eq!(terminal["status"], expected["result"], "{label}: status");
            let text: String = terminal["output"].as_array().unwrap().iter()
                .filter(|item| item["type"] == "message")
                .flat_map(|item| item["content"].as_array().into_iter().flatten())
                .filter_map(|part| part["text"].as_str()).collect();
            assert_eq!(text, expected["text"].as_str().unwrap(), "{label}: text");
            let tools: Vec<_> = terminal["output"].as_array().unwrap().iter()
                .filter(|item| item["type"] == "function_call")
                .map(|item| serde_json::json!({"id":item["call_id"],"name":item["name"],"arguments":serde_json::from_str::<Value>(item["arguments"].as_str().unwrap()).unwrap()})).collect();
            assert_eq!(serde_json::json!(tools), expected["tools"], "{label}: call identity / arguments");
            assert_eq!(terminal["usage"]["input_tokens"], expected["input"], "{label}: input");
            assert_eq!(terminal["usage"]["output_tokens"], expected["output"], "{label}: output");
            assert_eq!((transcoder.observer.input, transcoder.observer.output), (expected["input"].as_i64(), expected["output"].as_i64()), "{label}: accounting counters");
        }
    }
}
