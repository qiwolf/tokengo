//! Provider-specific adaptation of privileged message roles.
use crate::{parameter_contracts::Context, protocol::Protocol};
use serde_json::Value;

// Native Chat schema snapshots, 2026-10-27:
// DashScope: https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions
// Kimi: https://platform.kimi.ai/docs/api/chat
// GLM: https://docs.bigmodel.cn/api-reference/模型-api/对话补全
// Their privileged message variant is system, not developer (the Flash visual
// schema lists user/system/assistant only). Explicit exact IDs keep
// unknown future models untouched. Hosted providers need their own evidence.
const QWEN_SYSTEM_MODELS: &[&str] = &[
    "qwen3.8-max", "qwen3.8-max-0902", "qwen3.8-flash", "qwen3.8-2.4t-a95b", "qwen3.8-27b",
    "qwen3.7-max", "qwen3.7-plus", "qwen3.7-flash", "qwen3.6-plus", "qwen3.6-flash",
    "qwen3.5-plus", "qwen3.5-flash", "qwen3-vl-plus", "qwen3.8-omni-flash",
    "qwen3.5-omni-plus", "qwen3.5-omni-flash",
];
const KIMI_SYSTEM_MODELS: &[&str] = &[
    "kimi-k3.1-pro", "kimi-k3.1-pro-highspeed", "kimi-k3", "kimi-k2.7-code",
    "kimi-k2.7-code-highspeed", "kimi-k2.6",
];
// Independent SiliconFlow contract, not inherited from the Qwen brand. The
// official Chat guide documents system instructions; a 2026-10-27 exact-model
// Qwen probe rejected developer (code20015), then a second system message.
// 2026-10-28 exact-source controls: DeepSeek V4 Flash and GLM5.2 accepted
// developer structurally but ignored its unique instruction; Kimi Code rejected
// developer. All three honored the same instruction as system. Their pure
// leading controls use the same documented single-priority text fold.
// https://docs.siliconflow.cn/docs/api/chat-completions-post
// https://docs.siliconflow.cn/docs/userguide/capabilities/text-generation
const SILICONFLOW_SYSTEM_MODELS: &[&str] = &[
    "Qwen/Qwen3.8-27B", "deepseek-ai/DeepSeek-V4-Flash",
    "zai-org/GLM-5.2", "moonshotai/Kimi-K2.7-Code",
];
const GLM_SYSTEM_MODELS: &[&str] = &[
    "glm-5.3", "glm-5.3-flash", "glm-5.3-flashx", "glm-5.2", "glm-5.1", "glm-5-turbo", "glm-5", "glm-4.7",
    "glm-4.7-flash", "glm-4.7-flashx", "glm-4.6", "glm-4.5-air", "glm-4.5-airx",
    "glm-4.5-flash", "glm-4-flash-250414", "glm-4-flashx-250414",
];

pub(crate) fn documented_model(ctx:&Context<'_>)->bool {
    if ctx.is_oauth || ctx.protocol!=Protocol::Chat{return false;}
    match crate::parameter_contracts::provider(ctx) {
        "deepseek"=>matches!(ctx.model,"deepseek-flash"|"deepseek-v4-pro"),
        "dashscope"=>QWEN_SYSTEM_MODELS.contains(&ctx.model)||matches!(ctx.model,"qwq-plus"|"qwq-32b"|"qvq-max"|"qvq-plus"),
        "moonshot"=>KIMI_SYSTEM_MODELS.contains(&ctx.model),
        "zai"=>GLM_SYSTEM_MODELS.contains(&ctx.model),
        "siliconflow"=>SILICONFLOW_SYSTEM_MODELS.contains(&ctx.model),
        _=>false,
    }
}

/// Fold leading textual developer instructions into the target's single
/// privileged system role. This intentionally collapses the distinct OpenAI
/// system/developer priority levels: it preserves instruction text, sequence,
/// and message fields, but cannot represent their original priority distinction.
/// Never downgrade instructions to user or reorder history. QwQ forbids system
/// and QVQ ignores it, so either privileged role is rejected for those models.
///
/// Validate into a private copy first: a later unsupported item must not leave
/// an earlier instruction rewritten in the caller's body. Native OpenAI/OAuth,
/// unknown source/model pairs, and unverified hosting contracts are unchanged.
pub fn adapt(ctx: &Context<'_>, body: &mut Value) -> Result<bool, String> {
    if ctx.is_oauth || ctx.protocol != Protocol::Chat {
        return Ok(false);
    }
    let (label, system_supported) = match crate::parameter_contracts::provider(ctx) {
        "deepseek" => ("DeepSeek", true),
        "dashscope" if matches!(ctx.model, "qwq-plus" | "qwq-32b" | "qvq-max" | "qvq-plus") => ("DashScope QwQ/QVQ", false),
        "dashscope" if QWEN_SYSTEM_MODELS.contains(&ctx.model) => ("DashScope Qwen", true),
        "moonshot" if KIMI_SYSTEM_MODELS.contains(&ctx.model) => ("Moonshot Kimi", true),
        "zai" if GLM_SYSTEM_MODELS.contains(&ctx.model) => ("GLM", true),
        "siliconflow" if SILICONFLOW_SYSTEM_MODELS.contains(&ctx.model) => return adapt_siliconflow_prefix(body),
        _ => return Ok(false),
    };
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return Ok(false);
    };
    let mut next = messages.clone();
    let mut conversation_started = false;
    let mut changed = false;
    for message in &mut next {
        match message.get("role").and_then(Value::as_str) {
            Some("system") if !system_supported => {
                return Err(format!("{label} model {} cannot honor system instructions; refusing to discard or downgrade them", ctx.model));
            },
            Some("system") => {},
            Some("developer") => {
                if !system_supported {
                    return Err(format!("{label} model {} cannot honor developer instructions as system; refusing to discard or downgrade them", ctx.model));
                }
                if conversation_started {
                    return Err(format!("{label} developer instructions must precede conversation messages; refusing to reorder history"));
                }
                let content = match message.get("content") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Array(parts)) => {
                        let mut text = String::new();
                        for part in parts {
                            let Some(fields) = part.as_object() else {
                                return Err(format!("{label} developer instructions require plain text blocks"));
                            };
                            if fields.len() != 2 || part.get("type").and_then(Value::as_str) != Some("text") {
                                return Err(format!("{label} developer instructions cannot contain non-text or unknown content fields"));
                            }
                            let Some(part_text) = part.get("text").and_then(Value::as_str) else {
                                return Err(format!("{label} developer instruction text must be a string"));
                            };
                            text.push_str(part_text);
                        }
                        text
                    },
                    _ => return Err(format!("{label} developer instructions require a string or plain text blocks")),
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

// SiliconFlow's observed single-system shape is a separate hosting contract.
// Collapse only text-only, metadata-free prefix messages; fields on a single
// message remain intact. Validate everything before replacing the caller body.
fn adapt_siliconflow_prefix(body: &mut Value) -> Result<bool, String> {
    let Some(messages)=body.get("messages").and_then(Value::as_array) else {return Ok(false)};
    let privileged=|message:&Value|matches!(message["role"].as_str(),Some("system"|"developer"));
    let prefix=messages.iter().take_while(|message|privileged(message)).count();
    if messages[prefix..].iter().any(privileged) {
        return Err("SiliconFlow system/developer instructions must form the leading prefix; refusing to reorder history".into());
    }
    if prefix==0 {return Ok(false)};
    if prefix==1 && messages[0]["role"]=="system" {return Ok(false)};
    let mut texts=Vec::with_capacity(prefix);
    for message in &messages[..prefix] {
        if prefix>1 && message.as_object().is_none_or(|fields|fields.len()!=2 || !fields.contains_key("role") || !fields.contains_key("content")) {
            return Err("SiliconFlow system/developer prefix merging cannot preserve message metadata; refusing to discard name or unknown fields".into());
        }
        let text=match message.get("content") {
            Some(Value::String(text))=>text.clone(),
            Some(Value::Array(parts))=>{
                let mut text=String::new();
                for part in parts {
                    if part.as_object().is_none_or(|fields|fields.len()!=2) || part["type"]!="text" {
                        return Err("SiliconFlow system/developer prefix requires plain text blocks without unknown fields".into());
                    }
                    text.push_str(part["text"].as_str().ok_or("SiliconFlow system/developer prefix text must be a string")?);
                }
                text
            },
            _=>return Err("SiliconFlow system/developer prefix requires a string or plain text blocks".into()),
        };
        texts.push(text);
    }
    let mut first=messages[0].clone();
    first["role"]=Value::String("system".into());
    // Within each block list retain exact order; separate original messages so
    // adjacent instructions are not accidentally concatenated into one word.
    first["content"]=Value::String(texts.join("\n\n"));
    let mut next=Vec::with_capacity(messages.len()-prefix+1);
    next.push(first);next.extend_from_slice(&messages[prefix..]);
    body["messages"]=Value::Array(next);
    Ok(true)
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
    fn documented_provider_models_fold_privileged_text_atomically() {
        for (profile, model) in [("dashscope", "qwen3.8-max"), ("moonshot", "kimi-k3.1-pro"), ("zai", "glm-5.3")] {
            let mut ctx = context("https://fixture.invalid/v1");
            ctx.profile_override = Some(profile); ctx.model = model;
            let original = json!({"messages":[{"role":"system","content":"first"},
                {"role":"developer","name":"app","content":[{"type":"text","text":"one\n"},{"type":"text","text":"二"}]},
                {"role":"user","content":"hi"},{"role":"assistant","content":null,"tool_calls":[{"id":"one"}]},
                {"role":"tool","tool_call_id":"one","content":"1"}]});
            let mut body = original.clone();
            assert!(adapt(&ctx, &mut body).unwrap(), "{profile}/{model}");
            let mut expected = original;
            expected["messages"][1]["role"] = json!("system");
            expected["messages"][1]["content"] = json!("one\n二");
            assert_eq!(body, expected);
            for invalid in [json!({"role":"developer","content":[{"type":"text","text":"x","extra":1}]}),
                json!({"role":"developer","content":[{"type":"image_url","image_url":{"url":"x"}}]}),
                json!({"role":"developer","content":null})] {
                let mut rejected = json!({"messages":[{"role":"developer","content":"first"},invalid]});
                let before = rejected.clone();
                assert!(adapt(&ctx, &mut rejected).is_err()); assert_eq!(rejected,before);
            }
            let mut late = json!({"messages":[{"role":"developer","content":"first"},{"role":"user","content":"hi"},{"role":"developer","content":"late"}]});
            let before = late.clone();
            assert!(adapt(&ctx,&mut late).is_err()); assert_eq!(late,before);
        }
    }

    #[test]
    fn system_disabled_models_reject_privileged_roles_without_downgrade() {
        for model in ["qwq-plus", "qwq-32b", "qvq-max", "qvq-plus"] {
            let mut ctx = context("https://dashscope.aliyuncs.com/compatible-mode/v1"); ctx.model=model;
            for role in ["system", "developer"] {
                let mut body=json!({"messages":[{"role":role,"content":"important"},{"role":"user","content":"hi"}]});
                let before=body.clone(); assert!(adapt(&ctx,&mut body).is_err(), "{model}/{role}"); assert_eq!(body,before);
            }
            let mut body=json!({"messages":[{"role":"user","content":"hi"}]});
            assert!(!adapt(&ctx,&mut body).unwrap());
        }
    }

    #[test]
    fn roles_require_exact_source_model_and_chat_and_leave_oauth_untouched() {
        for (profile,model) in [("dashscope","qwen-future"),("dashscope","kimi-k3"),("moonshot","qwen3.8-max"),
            ("zai","glm-future"),("siliconflow","Qwen/qwen3.8-max"),("openai","glm-5.3")] {
            let mut ctx=context("https://fixture.invalid/v1");ctx.profile_override=Some(profile);ctx.model=model;
            let mut body=json!({"messages":[{"role":"developer","content":"rules"}]});let before=body.clone();
            assert!(!adapt(&ctx,&mut body).unwrap());assert_eq!(body,before);
        }
        for (url,model) in [("https://dashscope.proxy.invalid/compatible-mode/v1","qwen3.8-max"),
            ("https://api.moonshot.ai/v1/extra","kimi-k3.1-pro"),("https://open.bigmodel.cn.evil/api/paas/v4","glm-5.3")] {
            let mut ctx=context(url);ctx.model=model;
            let mut body=json!({"messages":[{"role":"developer","content":"rules"}]});let before=body.clone();
            assert!(!adapt(&ctx,&mut body).unwrap());assert_eq!(body,before);
        }
        for profile in ["dashscope","moonshot","zai"] {
            let mut ctx=context("https://fixture.invalid/v1");ctx.profile_override=Some(profile);ctx.model="qwen3.8-max";
            ctx.is_oauth=true;
            let mut body=json!({"messages":[{"role":"developer","content":"rules"}]});let before=body.clone();
            assert!(!adapt(&ctx,&mut body).unwrap());assert_eq!(body,before);
        }
    }

    #[test]
    fn known_models_non_chat_and_native_system_are_untouched() {
        for (profile,model) in [("dashscope","qwen3.8-max"),("moonshot","kimi-k3.1-pro"),("zai","glm-5.3")] {
            let mut ctx=context("https://fixture.invalid/v1");ctx.profile_override=Some(profile);ctx.model=model;
            let mut native=json!({"messages":[{"role":"system","content":"rules"},{"role":"user","content":"hi"}]});
            let before=native.clone();assert!(!adapt(&ctx,&mut native).unwrap());assert_eq!(native,before);
            for protocol in [Protocol::Responses,Protocol::Messages] {
                ctx.protocol=protocol;
                let mut body=json!({"messages":[{"role":"developer","content":"rules"}]});let before=body.clone();
                assert!(!adapt(&ctx,&mut body).unwrap());assert_eq!(body,before);
            }
        }
    }

    #[test]
    fn glm53_flash_exact_models_fold_text_and_reject_late_instructions_atomically() {
        for model in ["glm-5.3-flash", "glm-5.3-flashx"] {
            let mut ctx=context("https://open.bigmodel.cn/api/paas/v4");ctx.model=model;
            let mut body=json!({"messages":[{"role":"system","content":"root"},
                {"role":"developer","content":[{"type":"text","text":"rules"}]},{"role":"user","content":"hi"}]});
            assert!(adapt(&ctx,&mut body).unwrap(), "{model}");
            assert_eq!(body["messages"],json!([{"role":"system","content":"root"},{"role":"system","content":"rules"},{"role":"user","content":"hi"}]));
            let mut late=json!({"messages":[{"role":"developer","content":"first"},{"role":"user","content":"hi"},{"role":"developer","content":"late"}]});
            let before=late.clone();assert!(adapt(&ctx,&mut late).is_err());assert_eq!(late,before);
        }
    }

    #[test]
    fn siliconflow_observed_role_contract_is_exact_and_atomic() {
        for (url,profile) in [("https://api.siliconflow.cn/v1",None),("https://fixture.invalid/v1",Some("siliconflow"))] {
            let mut ctx=context(url);ctx.profile_override=profile;ctx.model="Qwen/Qwen3.8-27B";
            let mut body=json!({"messages":[{"role":"system","content":"root"},{"role":"developer","content":"rules"},{"role":"user","content":"hi"}]});
            assert!(adapt(&ctx,&mut body).unwrap());assert_eq!(body["messages"],json!([{"role":"system","content":"root\n\nrules"},{"role":"user","content":"hi"}]));
            let mut invalid=json!({"messages":[{"role":"developer","content":"first"},{"role":"developer","content":[{"type":"image_url","image_url":{"url":"x"}}]}]});
            let before=invalid.clone();assert!(adapt(&ctx,&mut invalid).is_err());assert_eq!(invalid,before);
        }
        for (url,profile,model) in [("https://api.siliconflow.cn/v1",None,"Qwen/Qwen3.8-unknown"),
            ("https://api.siliconflow.cn.evil/v1",None,"Qwen/Qwen3.8-27B"),
            ("https://fixture.invalid/v1",Some("openai"),"Qwen/Qwen3.8-27B")] {
            let mut ctx=context(url);ctx.profile_override=profile;ctx.model=model;
            let mut body=json!({"messages":[{"role":"developer","content":"rules"}]});let before=body.clone();
            assert!(!adapt(&ctx,&mut body).unwrap());assert_eq!(body,before);
        }
    }

    #[test]
    fn siliconflow_single_system_prefix_merges_text_in_order_and_rejects_loss_atomically() {
        let mut ctx=context("https://api.siliconflow.cn/v1");ctx.model="Qwen/Qwen3.8-27B";
        for roles in [["system","developer","system"],["system","system","system"],["developer","developer","system"]] {
            let mut body=json!({"messages":[{"role":roles[0],"content":"root"},{"role":roles[1],"content":[{"type":"text","text":"one"},{"type":"text","text":"two"}]},{"role":roles[2],"content":"tail"},{"role":"user","content":"hi"}]});
            assert!(adapt(&ctx,&mut body).unwrap());assert_eq!(body["messages"],json!([{"role":"system","content":"root\n\nonetwo\n\ntail"},{"role":"user","content":"hi"}]));
        }
        for message in [json!({"role":"system","content":"root","name":"app"}),json!({"role":"developer","content":"root","unknown":1}),
            json!({"role":"system","content":[{"type":"image_url","image_url":{"url":"x"}}]}),json!({"role":"system","content":null})] {
            let mut body=json!({"messages":[message,{"role":"developer","content":"rules"},{"role":"user","content":"hi"}]});let before=body.clone();
            assert!(adapt(&ctx,&mut body).is_err());assert_eq!(body,before);
        }
        for role in ["system","developer"] {
            let mut body=json!({"messages":[{"role":"developer","content":"root"},{"role":"user","content":"hi"},{"role":role,"content":"late"}]});let before=body.clone();
            assert!(adapt(&ctx,&mut body).is_err());assert_eq!(body,before);
        }
        let mut single=json!({"messages":[{"role":"developer","content":"rules","name":"app","extension":42},{"role":"user","content":"hi"}]});
        let mut expected=single.clone();expected["messages"][0]["role"]=json!("system");assert!(adapt(&ctx,&mut single).unwrap());assert_eq!(single,expected);
        let mut native=json!({"messages":[{"role":"system","content":"rules","name":"app","extension":42},{"role":"user","content":"hi"}]});
        let before=native.clone();assert!(!adapt(&ctx,&mut native).unwrap());assert_eq!(native,before);
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
    #[test]
    fn siliconflow_observed_three_models_fold_leading_text_without_brand_expansion(){
        for model in ["deepseek-ai/DeepSeek-V4-Flash","zai-org/GLM-5.2","moonshotai/Kimi-K2.7-Code"] {
            let c=Context{base_url:"https://api.siliconflow.cn/v1",model,protocol:Protocol::Chat,api_version:None,is_oauth:false,profile_override:Some("siliconflow")};
            let mut body=serde_json::json!({"messages":[{"role":"system","content":"first"},{"role":"developer","content":"second"},{"role":"user","content":"Proceed"}]});
            assert!(documented_model(&c));assert!(adapt(&c,&mut body).unwrap());
            assert_eq!(body["messages"],serde_json::json!([{"role":"system","content":"first\n\nsecond"},{"role":"user","content":"Proceed"}]));
            let mut single=serde_json::json!({"messages":[{"role":"developer","content":"text","name":"must retain"}]});
            assert!(adapt(&c,&mut single).unwrap());assert_eq!(single["messages"][0]["name"],"must retain");
            for messages in [serde_json::json!([{"role":"user","content":"Proceed"},{"role":"developer","content":"late"}]),serde_json::json!([{"role":"system","content":"first"},{"role":"developer","content":"text","name":"must retain"}]),serde_json::json!([{"role":"developer","content":[{"type":"image_url","image_url":"private"}]}])] {
                let mut bad=serde_json::json!({"messages":messages});let before=bad.clone();assert!(adapt(&c,&mut bad).is_err());assert_eq!(bad,before);
            }
        }
        for model in ["deepseek-ai/DeepSeek-V4.1-Flash","deepseek-ai/DeepSeek-V4-Pro","Pro/zai-org/GLM-5.2","zai-org/GLM-5.3","Pro/moonshotai/Kimi-K2.6","moonshotai/Kimi-future"] {
            let c=Context{base_url:"https://api.siliconflow.cn/v1",model,protocol:Protocol::Chat,api_version:None,is_oauth:false,profile_override:Some("siliconflow")};
            let mut body=serde_json::json!({"messages":[{"role":"developer","content":"keep"}]});let before=body.clone();assert!(!documented_model(&c));assert!(!adapt(&c,&mut body).unwrap());assert_eq!(body,before);
        }
    }

}
