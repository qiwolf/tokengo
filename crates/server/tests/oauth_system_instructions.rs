use serde_json::json;
use tokengo_server::protocol::{collect::prepare_request, convert_request, Protocol};

#[test]
fn chatbox_system_prompt_survives_chat_to_subscription_conversion() {
    let chat = json!({"model":"gpt-6.1-sol","stream":true,"messages":[
        {"role":"system","content":"用中文回答，保留公式。"},
        {"role":"developer","content":"Be concise."},
        {"role":"user","content":"你好"}
    ]});
    for stream in [false, true] {
        let mut chat = chat.clone();
        chat["stream"] = json!(stream);
        let mut body = convert_request(Protocol::Chat, Protocol::Responses, &chat).unwrap();
        prepare_request(&mut body, None, None).unwrap();
        assert_eq!(body["instructions"], "用中文回答，保留公式。");
        assert_eq!(body["input"][0]["role"], "developer");
        assert_eq!(body["input"][1]["content"][0]["text"], "你好");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
    }
}

#[test]
fn leading_system_messages_keep_order_and_existing_instructions() {
    let user = json!({"role":"user","content":"hello"});
    let mut body = json!({"instructions":"existing","input":[
        {"role":"system","content":"first"},
        {"type":"message","role":"system","content":[{"type":"input_text","text":"second"},{"type":"input_text","text":" part"}]},
        user.clone()
    ]});
    prepare_request(&mut body, None, None).unwrap();
    assert_eq!(body["instructions"], "existing\n\nfirst\n\nsecond part");
    assert_eq!(body["input"], json!([user]));
}

#[test]
fn system_normalization_rejects_lossy_or_reordered_inputs() {
    for mut body in [
        json!({"input":[{"role":"user","content":"hello"},{"role":"system","content":"late"}]}),
        json!({"input":[{"role":"system","content":[{"type":"input_image","image_url":"https://example.com/image"}]}]}),
        json!({"input":[{"role":"system","content":"rules","unknown":"preserve me"}]}),
        json!({"instructions":17,"input":[{"role":"system","content":"rules"}]}),
    ] {
        let before = body.clone();
        assert!(prepare_request(&mut body, None, None).is_err());
        assert_eq!(body, before);
    }
}

#[test]
fn native_developer_and_opaque_items_remain_unchanged() {
    let input = json!([
        {"role":"developer","content":"rules"},
        {"type":"reasoning","encrypted_content":"opaque"},
        {"role":"user","content":"hello"}
    ]);
    let mut body = json!({"instructions":"native","input":input.clone()});
    prepare_request(&mut body, None, None).unwrap();
    assert_eq!(body["instructions"], "native");
    assert_eq!(body["input"], input);
}
