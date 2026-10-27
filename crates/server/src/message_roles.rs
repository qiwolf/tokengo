//! Provider-specific adaptation of privileged message roles.
use crate::{parameter_contracts::Context, protocol::Protocol};
use serde_json::Value;

/// DeepSeek Chat has one privileged instruction role (`system`). Preserve the
/// leading instruction sequence while folding `developer` into that role;
/// never downgrade application instructions to `user`, reorder history, or
/// infer this contract from a model name. The target cannot represent distinct
/// system/developer priority levels, so this is explicitly source-specific.
///
/// Validate into a private copy first: a later unsupported item must not leave
/// an earlier instruction rewritten in the caller's body.
pub fn adapt(ctx: &Context<'_>, body: &mut Value) -> Result<bool, String> {
    if ctx.is_oauth || ctx.protocol != Protocol::Chat
        || crate::parameter_contracts::provider(ctx) != "deepseek"
    {
        return Ok(false);
    }
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return Ok(false);
    };
    let mut next = messages.clone();
    let mut conversation_started = false;
    let mut changed = false;
    for message in &mut next {
        match message.get("role").and_then(Value::as_str) {
            Some("system") => {},
            Some("developer") => {
                if conversation_started {
                    return Err("DeepSeek developer instructions must precede conversation messages; refusing to reorder history".into());
                }
                let content = match message.get("content") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Array(parts)) => {
                        let mut text = String::new();
                        for part in parts {
                            let Some(fields) = part.as_object() else {
                                return Err("DeepSeek developer instructions require plain text blocks".into());
                            };
                            if fields.len() != 2 || part.get("type").and_then(Value::as_str) != Some("text") {
                                return Err("DeepSeek developer instructions cannot contain non-text or unknown content fields".into());
                            }
                            let Some(part_text) = part.get("text").and_then(Value::as_str) else {
                                return Err("DeepSeek developer instruction text must be a string".into());
                            };
                            text.push_str(part_text);
                        }
                        text
                    },
                    _ => return Err("DeepSeek developer instructions require a string or plain text blocks".into()),
                };
                message["role"] = Value::String("system".into());
                message["content"] = Value::String(content);
                changed = true;
            },
            _ => conversation_started = true,
        }
    }
    if changed {
        body["messages"] = Value::Array(next);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context(url: &str) -> Context<'_> {
        Context { base_url: url, model: "deepseek-flash", protocol: Protocol::Chat,
            api_version: None, is_oauth: false, profile_override: None }
    }

    #[test]
    fn leading_developer_becomes_system_preserving_order_and_fields() {
        let mut body = json!({"model":"alias","stream":true,"messages":[
            {"role":"system","content":"first"},
            {"role":"developer","content":"second","name":"application"},
            {"role":"developer","content":"third"},
            {"role":"user","content":"hello"},
            {"role":"assistant","content":null,"tool_calls":[{"id":"call1"}]},
            {"role":"tool","tool_call_id":"call1","content":"result"}
        ]});
        let original=body.clone();
        assert!(adapt(&context("https://api.deepseek.com/v1"), &mut body).unwrap());
        let mut expected=original;
        expected["messages"][1]["role"]=json!("system");
        expected["messages"][2]["role"]=json!("system");
        assert_eq!(body,expected);
        assert!(!adapt(&context("https://api.deepseek.com/v1"), &mut body).unwrap());
    }

    #[test]
    fn developer_text_blocks_are_joined_without_separator_or_content_loss() {
        let mut body=json!({"messages":[{"role":"developer","content":[
            {"type":"text","text":"one\n"},{"type":"text","text":"二"}
        ]},{"role":"user","content":"hello"}]});
        assert!(adapt(&context("https://api.deepseek.com/v1"),&mut body).unwrap());
        assert_eq!(body["messages"][0],json!({"role":"system","content":"one\n二"}));
    }

    #[test]
    fn unsupported_developer_content_rejects_without_partial_mutation() {
        for content in [json!([{"type":"image_url","image_url":{"url":"https://example.com/a.png"}}]),
            json!([{"type":"text","text":"rules","unknown":true}]),
            json!([{"type":"text","text":42}]),json!(null),json!({"text":"rules"})] {
            let mut body=json!({"messages":[{"role":"developer","content":"first"},
                {"role":"developer","content":content}]});
            let original=body.clone();
            assert!(adapt(&context("https://api.deepseek.com/v1"),&mut body).is_err());
            assert_eq!(body,original);
        }
    }

    #[test]
    fn late_developer_rejects_instead_of_reordering_or_changing_priority() {
        for role in ["user","assistant","tool"] {
            let mut body=json!({"messages":[{"role":"developer","content":"first"},
                {"role":role,"content":"turn"},{"role":"developer","content":"late"}]});
            let original=body.clone();
            assert!(adapt(&context("https://api.deepseek.com/v1"),&mut body).is_err());
            assert_eq!(body,original);
        }
    }

    #[test]
    fn native_system_and_conversation_are_untouched() {
        let mut body=json!({"messages":[{"role":"system","content":"rules"},
            {"role":"user","content":"hello"},{"role":"assistant","content":"hi"}]});
        let original=body.clone();
        assert!(!adapt(&context("https://api.deepseek.com/v1"),&mut body).unwrap());
        assert_eq!(body,original);
    }

    #[test]
    fn other_providers_unknown_hosts_and_non_chat_are_not_guessed() {
        let body=json!({"messages":[{"role":"developer","content":"rules"}]});
        for url in ["https://api.openai.com/v1","https://api.siliconflow.cn/v1", "https://deepseek.proxy.example/v1"] {
            let mut actual=body.clone();
            assert!(!adapt(&context(url),&mut actual).unwrap());
            assert_eq!(actual,body);
        }
        let mut ctx=context("https://api.deepseek.com/v1");
        ctx.protocol=Protocol::Responses;
        let mut actual=body.clone();
        assert!(!adapt(&ctx,&mut actual).unwrap());
        assert_eq!(actual,body);
    }

    #[test]
    fn managed_oauth_ignores_even_explicit_deepseek_override() {
        let mut ctx=context("https://api.openai.com/v1");
        ctx.is_oauth=true;ctx.profile_override=Some("deepseek");
        let mut body=json!({"messages":[{"role":"developer","content":"rules"}]});
        let original=body.clone();
        assert!(!adapt(&ctx,&mut body).unwrap());
        assert_eq!(body,original);
    }

    #[test]
    fn explicit_deepseek_contract_can_adapt_a_configured_proxy() {
        let mut ctx=context("https://private.proxy.example/v1");
        ctx.profile_override=Some("deepseek");
        let mut body=json!({"messages":[{"role":"developer","content":"rules"}]});
        assert!(adapt(&ctx,&mut body).unwrap());
        assert_eq!(body["messages"][0]["role"],"system");
    }
}
