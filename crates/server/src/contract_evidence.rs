//! Versioned documentation evidence. This is descriptive metadata, never an
//! authorization, source-capability switch or replacement for live acceptance.
use crate::{parameter_contracts::{self, Context}, protocol::Protocol};
use serde::Serialize;
use serde_json::{json, Value};

pub const REVISION: &str = "2026-10-28-p1.4";

#[derive(Debug, Serialize)]
pub struct ProviderContractIdentity<'a> {
    pub provider: &'static str,
    pub model: &'a str,
    pub protocol: Protocol,
    pub api_version: Option<&'a str>,
    pub authentication_line: &'static str,
    pub endpoint_family: &'static str,
    pub authentication_product: &'static str,
}
impl<'a> ProviderContractIdentity<'a> {
    pub fn from_context(ctx: &Context<'a>) -> Self {
        Self {
            provider: parameter_contracts::provider(ctx), model: ctx.model,
            protocol: ctx.protocol, api_version: ctx.api_version.filter(|v| valid_api_version(v)),
            // Context does not distinguish external OAuth products; the runtime
            // binding DTO owns that fact. Never infer managed entitlement here.
            authentication_line: if ctx.is_oauth { "oauth_runtime_bound" } else { "externally_configured_credentials" },
            endpoint_family: endpoint_family(ctx),
            authentication_product: match endpoint_family(ctx) {
                "glm_coding_plan"=>"glm_coding_plan_endpoint_contract",
                "glm_public_api"=>"glm_public_api_endpoint_contract",
                "oauth_runtime_bound"=>"oauth_product_requires_runtime_binding",
                _=>"externally_configured_product_unverified",
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ContractEvidence {
    pub rule_id: &'static str,
    pub provider: &'static str,
    pub protocol: Protocol,
    pub field: &'static str,
    pub source: &'static str,
    pub read_at: &'static str,
    pub scope: &'static str,
    pub confidence: &'static str,
    pub review_trigger: &'static str,
}

const RULES: &[ContractEvidence] = &[
    ContractEvidence {
        rule_id:"G10",provider:"moonshot",protocol:Protocol::Chat,
        field:"usage input subsets/raw provenance",source:"https://platform.kimi.com/docs/api/chat",
        read_at:"2026-10-27",scope:"typed cache read/write subsets; no duplicate totals or inferred supplier cost; bounded wire provenance",
        confidence:"documentation_and_static_code",review_trigger:"usage subset or protocol schema change",
    },
    ContractEvidence {
        rule_id:"G12",provider:"zai",protocol:Protocol::Chat,
        field:"request_id",source:"https://docs.bigmodel.cn/api-reference/模型-api/对话补全",
        read_at:"2026-10-27",scope:"bounded trace string only; semantic extensions remain guarded",
        confidence:"documentation_and_static_code",review_trigger:"trace or semantic response schema change",
    },
    ContractEvidence {
        rule_id:"G06-chat",provider:"dashscope",protocol:Protocol::Chat,
        field:"workspace endpoint identity",source:"https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions",
        read_at:"2026-10-27",scope:"exact workspace single-label host in six documented regions; no model-name or arbitrary suffix inference",
        confidence:"documentation_and_static_code",review_trigger:"workspace region or endpoint contract change",
    },
    ContractEvidence {
        rule_id:"G06",provider:"dashscope",protocol:Protocol::Responses,
        field:"workspace endpoint identity",source:"https://help.aliyun.com/en/model-studio/qwen-api-via-openai-responses",
        read_at:"2026-10-27",scope:"six exact documented workspace regions and paths; manual profile wins, no capability grant",
        confidence:"documentation_and_static_code",review_trigger:"workspace region or endpoint contract change",
    },
    ContractEvidence {
        rule_id:"G17-ollama",provider:"ollama",protocol:Protocol::Chat,
        field:"runtime_identity",source:"https://docs.ollama.com/api/openai-compatibility",
        read_at:"2026-10-27",scope:"bounded unverified operator/API claims; template digest changes invalidate source identity; no automatic probing or capability grant",
        confidence:"documentation_and_static_code",review_trigger:"runtime template/parser/config or evidence source change",
    },
    ContractEvidence {
        rule_id:"G17-vllm",provider:"vllm",protocol:Protocol::Chat,
        field:"runtime_identity",source:"https://docs.vllm.ai/en/stable/serving/online_serving/openai_compatible_server/",
        read_at:"2026-10-27",scope:"runtime/model names do not prove parser/template support; declared identity remains unverified",
        confidence:"documentation_and_static_code",review_trigger:"runtime template/parser/config or evidence source change",
    },
    ContractEvidence {
        rule_id:"G16",provider:"minimax",protocol:Protocol::Chat,
        field:"source split thinking codec",source:"https://platform.minimax.io/docs/api-reference/text-openai-api",
        read_at:"2026-10-27",scope:"API-key exact M3/M3.1 split Chat to public Responses; preserve strict intent without asserting enforcement; signed Messages remain unrepresentable; native passthrough unchanged",
        confidence:"documentation_and_static_code",review_trigger:"thinking/signature/split/schema or authentication line change",
    },
    ContractEvidence {
        rule_id: "G01", provider: "dashscope", protocol: Protocol::Responses,
        field: "max_output_tokens",
        source: "https://help.aliyun.com/en/model-studio/qwen-api-via-openai-responses",
        read_at: "2026-10-27", scope: "exact model rules; unknown native budgets use an annotated local reservation estimate, while unrepresentable converted total caps remain guarded",
        confidence: "documentation_and_static_code", review_trigger: "model or output-budget schema change",
    },
    ContractEvidence {
        rule_id: "G02-gemini", provider: "gemini", protocol: Protocol::Chat,
        field: "tool_calls[].extra_content.google.thought_signature",
        source: "https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures",
        read_at: "2026-10-27", scope: "opaque-history ownership only; no new converter or executor support",
        confidence: "documentation_and_static_code", review_trigger: "signature path or replay schema change",
    },
    ContractEvidence {
        rule_id: "G02-ark", provider: "ark", protocol: Protocol::Chat,
        field: "assistant.encrypted_content",
        source: "https://docs.volcengine.com/docs/ark/api-schema-upgrade-notice?lang=zh",
        read_at: "2026-10-27", scope: "opaque-history ownership only; model/version acceptance remains unverified",
        confidence: "documentation_conflict", review_trigger: "resolve schema-versus-guide conflict on exact source version",
    },
    ContractEvidence {
        rule_id: "G04", provider: "openai", protocol: Protocol::Responses,
        field: "tools[].strict", source: "https://developers.openai.com/api/docs/guides/function-calling",
        read_at: "2026-10-27", scope: "client protocol conversion only; proven strict schema subset, ambiguous defaults require native transport",
        confidence: "documentation_and_static_code", review_trigger: "exact model/API schema or source contract change",
    },
    ContractEvidence {
        rule_id: "G05-qwen", provider: "dashscope", protocol: Protocol::Chat,
        field: "messages[].role", source: "https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions",
        read_at: "2026-10-27", scope: "exact Chat model role table; QwQ/QVQ reject privileged instructions",
        confidence: "documentation_and_static_code", review_trigger: "exact model/API schema or source contract change",
    },
    ContractEvidence {
        rule_id: "G05-kimi", provider: "moonshot", protocol: Protocol::Chat,
        field: "messages[].role", source: "https://platform.kimi.ai/docs/api/chat",
        read_at: "2026-10-27", scope: "exact native Chat model role table, not hosted-brand inference",
        confidence: "documentation_and_static_code", review_trigger: "exact model/API schema or source contract change",
    },
    ContractEvidence {
        rule_id: "G05-glm", provider: "zai", protocol: Protocol::Chat,
        field: "messages[].role", source: "https://docs.bigmodel.cn/api-reference/模型-api/对话补全",
        read_at: "2026-10-27", scope: "exact public/Coding Plan model and role contracts",
        confidence: "documentation_and_static_code", review_trigger: "exact model/API schema or source contract change",
    },
    ContractEvidence {
        rule_id: "G05-siliconflow", provider: "siliconflow", protocol: Protocol::Chat,
        field: "messages[].role", source: "https://docs.siliconflow.cn/docs/api/chat-completions-post",
        read_at: "2026-10-27", scope: "independent exact SF Qwen/Qwen3.8-27B, deepseek-ai/DeepSeek-V4-Flash, zai-org/GLM-5.2 and moonshotai/Kimi-K2.7-Code leading-role observations: Qwen/Kimi reject developer, DeepSeek/GLM ignore its unique instruction while system succeeds; pure text single-system priority fold only; no brand or parameter-capability inference",
        confidence: "documentation_and_exact_source_observation", review_trigger: "hosted role schema or exact model change",
    },
    ContractEvidence {
        rule_id: "G14", provider: "anthropic", protocol: Protocol::Messages,
        field: "thinking/tool_choice", source: "https://platform.claude.com/docs/en/build-with-claude/thinking",
        read_at: "2026-10-27", scope: "exact Mythos5.1 always-on/forced-tool/sampling/output limits; no account entitlement grant",
        confidence: "documentation_and_static_code", review_trigger: "exact model/API schema or source contract change",
    },
    ContractEvidence {
        rule_id: "G15-chat", provider: "minimax", protocol: Protocol::Chat,
        field: "thinking", source: "https://platform.minimax.io/docs/api-reference/text-openai-api",
        read_at: "2026-10-27", scope: "exact M3/M3.1 Chat↔Messages defaults/explicit mode and effort preservation",
        confidence: "documentation_and_static_code", review_trigger: "exact model/API schema or source contract change",
    },
    ContractEvidence {
        rule_id: "G15-messages", provider: "minimax", protocol: Protocol::Messages,
        field: "thinking", source: "https://platform.minimax.io/docs/api-reference/text-anthropic-api",
        read_at: "2026-10-27", scope: "native defaults differ; operator mapping defaults resolved before provider omission defaults",
        confidence: "documentation_and_static_code", review_trigger: "exact model/API schema or source contract change",
    },
    ContractEvidence {
        rule_id:"G07-chat", provider:"deepseek", protocol:Protocol::Chat,
        field:"thinking/tool_choice/tools.function.strict", source:"https://api-docs.deepseek.com/guides/tool_calls/",
        read_at:"2026-10-27", scope:"exact Chat models; forced thinking conflicts and documented official Beta strict source only",
        confidence:"documentation_and_static_code", review_trigger:"source model/protocol schema change",
    },
    ContractEvidence {
        rule_id:"G07-responses", provider:"deepseek", protocol:Protocol::Responses,
        field:"native Responses intent controls", source:"https://api-docs.deepseek.com/guides/responses_api/",
        read_at:"2026-10-27", scope:"exact models; ignored state/role/media intent rejected before transport",
        confidence:"documentation_and_static_code", review_trigger:"source model/protocol schema change",
    },
    ContractEvidence {
        rule_id:"G08", provider:"dashscope", protocol:Protocol::Chat,
        field:"tool_choice/parallel_tool_calls", source:"https://help.aliyun.com/en/model-studio/qwen-api-via-openai-chat-completions",
        read_at:"2026-10-27", scope:"exact source/model combinations; hosted Kimi/MiniMax never inherit Qwen off rules",
        confidence:"documentation_and_static_code", review_trigger:"source model/protocol schema change",
    },
    ContractEvidence {
        rule_id:"G09", provider:"moonshot", protocol:Protocol::Responses,
        field:"native request and full summary history", source:"https://platform.kimi.com/docs/api/responses",
        read_at:"2026-10-27", scope:"exact K3 native models; independent complete summary proof, not raw thinking",
        confidence:"documentation_and_static_code", review_trigger:"source model/protocol schema change",
    },
    ContractEvidence {
        rule_id:"G11", provider:"zai", protocol:Protocol::Chat,
        field:"reasoning_effort API line", source:"https://docs.bigmodel.cn/cn/guide/capabilities/thinking",
        read_at:"2026-10-27", scope:"public versus exact official Coding Plan endpoints; no proxy product-line guess",
        confidence:"documentation_and_static_code", review_trigger:"source model/protocol schema change",
    },
    ContractEvidence {
        rule_id:"G13", provider:"zai", protocol:Protocol::Chat,
        field:"tool_choice/response_format/tool_stream", source:"https://docs.bigmodel.cn/api-reference/模型-api/对话补全",
        read_at:"2026-10-27", scope:"exact public schema including Flash subtype; stream constraints and schema/text conflict preserved",
        confidence:"documentation_and_static_code", review_trigger:"source model/protocol schema change",
    },
];

/// Deliberately excludes arbitrary endpoint/header/model payloads from evidence.
fn safe_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b,b'-'|b'_'|b'.'|b'/'|b':'))
}
fn valid_api_version(value: &str) -> bool {
    value.len()==10 && value.bytes().enumerate().all(|(i,b)| if i==4||i==7 {b==b'-'} else {b.is_ascii_digit()})
}
fn endpoint_family(ctx: &Context<'_>) -> &'static str {
    if ctx.is_oauth { return "oauth_runtime_bound"; }
    let Ok(url)=url::Url::parse(ctx.base_url) else { return "unknown"; };
    if url.scheme()!="https" || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || url.port().is_some() { return "configured_proxy_or_unknown"; }
    let p=url.path().trim_end_matches('/');
    match (parameter_contracts::provider(ctx),url.host_str(),p) {
        ("zai",Some("open.bigmodel.cn"|"api.z.ai"),"/api/coding/paas/v4")=>"glm_coding_plan",
        ("zai",Some("open.bigmodel.cn"|"api.z.ai"),"/api/paas/v4")=>"glm_public_api",
        ("deepseek",Some("api.deepseek.com"),"/beta")=>"deepseek_beta",
        ("deepseek",Some("api.deepseek.com"),""|"/v1")=>"deepseek_public_api",
        _ if ctx.profile_override.is_some()=>"explicit_profile_endpoint_unverified",
        _ if parameter_contracts::provider(ctx)!="unknown"=>"exact_official_endpoint",
        _=>"configured_proxy_or_unknown",
    }
}

fn model_independent(rule: &ContractEvidence) -> bool {
    matches!(rule.rule_id,"G02-gemini"|"G02-ark"|"G04"|"G06"|"G06-chat"|"G17-ollama"|"G17-vllm")
}
fn applies(rule: &ContractEvidence,ctx: &Context<'_>) -> bool {
    if ctx.is_oauth || rule.provider!=parameter_contracts::provider(ctx) || rule.protocol!=ctx.protocol {return false;}
    if model_independent(rule) {return true;}
    if rule.provider!="anthropic" && ctx.api_version.is_some() {return false;}
    if rule.rule_id.starts_with("G05-") {return crate::message_roles::documented_model(ctx);}
    if rule.rule_id=="G01" {
        return !matches!(crate::parameter_contracts_china::output_semantics(ctx,&json!({})),crate::parameter_contracts_china::OutputSemantics::Unknown);
    }
    if !parameter_contracts::known_model_for_evidence(ctx) {return false;}
    if rule.rule_id=="G16" {return crate::protocol::minimax_codec::policy(ctx,Protocol::Responses).is_some();}
    if matches!(rule.rule_id,"G15-chat"|"G15-messages") {return matches!(ctx.model,"MiniMax-M3"|"MiniMax-M3.1-Flash");}
    if matches!(rule.rule_id,"G07-chat"|"G07-responses") {return matches!(ctx.model,"deepseek-flash"|"deepseek-v4-pro");}
    if rule.rule_id=="G14" {
        return ctx.model=="claude-mythos-5-1" && matches!(ctx.api_version,None|Some("2023-06-01"|"2026-10-21"));
    }
    if rule.rule_id=="G09" {return crate::parameter_contracts_china::kimi_native_summary(ctx);}
    true
}

fn source_evidence(rule:&ContractEvidence)->Value {
    use sha2::{Digest,Sha256};
    static SOURCES: std::sync::LazyLock<Value> = std::sync::LazyLock::new(||
        serde_json::from_str(include_str!("../../../docs/references/provider-compatibility-20261027/sources.json")).expect("versioned official source metadata"));
    let sources=&*SOURCES;
    let source=sources.as_array().and_then(|items|items.iter().find(|v|v["url"].as_str()==Some(rule.source)));
    let metadata_hash=source.map(|value|hex::encode(Sha256::digest(serde_json::to_vec(value).expect("source record serializes"))));
    json!({"url":rule.source,"read_at":rule.read_at,"source_id":source.and_then(|s|s["id"].as_str()),
        "read_status":source.and_then(|s|s["read_status"].as_str()).unwrap_or("not_in_source_snapshot"),
        "metadata_record_sha256":metadata_hash,
        "document_archive_sha256":source.and_then(|s|s["archive_sha256"].as_str()),
        "hash_scope":"metadata_record_sha256 hashes the archived research record, never claims to hash a missing document body",
        "model_revision":"not_inferred_from_model_id","document_revision":"unknown_unless_source_record_explicit"})
}

/// Full URLs and interpretation are emitted once with the top-level inventory.
pub fn registry() -> Value {
    let rules:Vec<_>=RULES.iter().map(|rule| {
        let mut value=serde_json::to_value(rule).expect("static rule serializes");
        value["applicability"]=json!({"provider":rule.provider,"protocol":rule.protocol,
            "model_scope":if model_independent(rule){"model_independent_protection_or_protocol_rule"}else{"execution_contract_exact_model_table"},
            "api_version_scope":if rule.rule_id=="G14"{"2023-06-01 or 2026-10-21; omitted follows source default"}else{"no_model_version_support_inferred"}});
        value["evidence_layers"]=json!({"documentation":source_evidence(rule),"provider_shape":"separate_test_receipt_required",
            "gateway_mock":"separate_test_receipt_required","live_source":"not_attested_by_registry"});
        value
    }).collect();
    json!({"revision":REVISION,"rules":rules,"runtime_acceptance":"not_inferred",
        "grants_capabilities":false,"mutates_binding_generation":false})
}

/// A bounded reference per catalog model; no source data or secrets are copied.
pub fn describe(ctx: &Context<'_>) -> Value {
    let identity=ProviderContractIdentity::from_context(ctx);
    let rules:Vec<_>=RULES.iter().filter(|r|applies(r,ctx)).map(|r|r.rule_id).collect();
    let mut out=json!({"revision":REVISION,"identity":{
        "provider":identity.provider,"model":if safe_identifier(identity.model){Some(identity.model)}else{None},
        "protocol":identity.protocol,"api_version":identity.api_version,
        "model_revision":"unknown","schema_revision":REVISION,
        "endpoint_family":identity.endpoint_family,"authentication_line":identity.authentication_line,
        "authentication_product":identity.authentication_product,"authentication_verified":false},
        "rules":rules,"live_inference_verified":false,
        "evidence_layers":{"documentation":"versioned_registry_reference","provider_shape":"separate_test_receipt_required",
            "gateway_mock":"separate_test_receipt_required","live_source":"not_verified"}});
    if identity.provider=="ark" && ctx.protocol==Protocol::Chat {
        out["version_verification"]=json!("required_before_new_capability");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ctx<'a>(model:&'a str,version:Option<&'a str>)->Context<'a>{Context{base_url:"https://api.anthropic.com/v1",model,protocol:Protocol::Messages,api_version:version,is_oauth:false,profile_override:None}}
    #[test]
    fn unknown_model_and_api_revision_do_not_borrow_exact_rule_evidence(){
        assert!(describe(&ctx("claude-future",Some("2026-10-21")))["rules"].as_array().unwrap().is_empty());
        assert!(describe(&ctx("claude-mythos-5-1",Some("2099-01-01")))["rules"].as_array().unwrap().is_empty());
        assert_eq!(describe(&ctx("claude-mythos-5-1",Some("2026-10-21")))["rules"],json!(["G14"]));
    }
    #[test]
    fn identity_is_bounded_typed_and_never_echoes_endpoint_or_header_material(){
        let mut c=ctx("https://secret.invalid/?token=hidden",Some("secret-api-value"));
        c.base_url="https://user:password@secret.invalid/v1?key=secret";
        let text=describe(&c).to_string();
        for secret in ["password","secret-api-value","hidden","secret.invalid"] {assert!(!text.contains(secret));}
        assert!(describe(&c)["identity"]["api_version"].is_null());
        assert_eq!(describe(&c)["identity"]["protocol"],"messages");
    }
    #[test]
    fn source_hashes_are_scoped_and_documentary_updates_are_not_runtime_grants(){
        let r=registry();assert_eq!(r["mutates_binding_generation"],false);assert_eq!(r["grants_capabilities"],false);
        for rule in r["rules"].as_array().unwrap(){
            let d=&rule["evidence_layers"]["documentation"];
            if !d["source_id"].is_null(){assert_eq!(d["metadata_record_sha256"].as_str().unwrap().len(),64);}
            assert_eq!(rule["evidence_layers"]["live_source"],"not_attested_by_registry");
        }
    }
    #[test]
    fn minimax_and_deepseek_evidence_never_expand_by_brand_or_future_model(){
        for model in ["MiniMax-M2.5","MiniMax-M3-future","MiniMax-M3.1-Flash-future"] {
            let c=Context{base_url:"https://api.minimax.io/v1",model,protocol:Protocol::Chat,api_version:None,is_oauth:false,profile_override:Some("minimax")};
            assert!(describe(&c)["rules"].as_array().unwrap().is_empty(),"{model}");
        }
        let c=Context{base_url:"https://api.minimax.io/v1",model:"MiniMax-M3",protocol:Protocol::Chat,api_version:None,is_oauth:false,profile_override:Some("minimax")};
        assert!(describe(&c)["rules"].as_array().unwrap().iter().any(|r|r=="G16"));
        let c=Context{base_url:"https://api.deepseek.com/v1",model:"deepseek-future",profile_override:Some("deepseek"),..c};
        assert!(describe(&c)["rules"].as_array().unwrap().is_empty());
    }

}
