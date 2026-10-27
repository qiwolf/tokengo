//! Versioned documentation evidence. This is descriptive metadata, never an
//! authorization, source-capability switch or replacement for live acceptance.
use crate::{parameter_contracts::{self, Context}, protocol::Protocol};
use serde::Serialize;
use serde_json::{json, Value};

pub const REVISION: &str = "2026-10-27-p1.2";

#[derive(Debug, Serialize)]
pub struct ProviderContractIdentity<'a> {
    pub provider: &'static str,
    pub model: &'a str,
    pub protocol: Protocol,
    pub api_version: Option<&'a str>,
    pub authentication_line: &'static str,
}
impl<'a> ProviderContractIdentity<'a> {
    pub fn from_context(ctx: &Context<'a>) -> Self {
        Self {
            provider: parameter_contracts::provider(ctx), model: ctx.model,
            protocol: ctx.protocol, api_version: ctx.api_version,
            authentication_line: if ctx.is_oauth { "managed_openai_oauth" } else { "externally_configured_credentials" },
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
        read_at:"2026-10-27",scope:"API-key exact M3/M3.1 split Chat to public Responses; signed Messages and unproved strict guarantees remain guarded; native passthrough unchanged",
        confidence:"documentation_and_static_code",review_trigger:"thinking/signature/split/schema or authentication line change",
    },
    ContractEvidence {
        rule_id: "G01", provider: "dashscope", protocol: Protocol::Responses,
        field: "max_output_tokens",
        source: "https://help.aliyun.com/en/model-studio/qwen-api-via-openai-responses",
        read_at: "2026-10-27", scope: "exact model rules; unknown models require a verified total bound",
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
        read_at: "2026-10-27", scope: "independent hosted Qwen/Qwen3.8-27B role/single-system-prefix observations; no official-Qwen or other hosted-model inference",
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

/// Full URLs and interpretation are emitted once with the top-level inventory.
pub fn registry() -> Value {
    json!({"revision": REVISION, "rules": RULES, "runtime_acceptance": "not_inferred",
        "grants_capabilities": false, "mutates_binding_generation": false})
}

/// A bounded reference per catalog model; do not duplicate the source corpus.
pub fn describe(ctx: &Context<'_>) -> Value {
    let identity = ProviderContractIdentity::from_context(ctx);
    let rules: Vec<_> = RULES.iter().filter(|r| !ctx.is_oauth && r.provider == identity.provider && r.protocol == ctx.protocol)
        .map(|r| r.rule_id).collect();
    // Never echo secrets/query strings from arbitrary endpoints, or unbounded
    // remote model/version metadata into every catalog entry.
    let mut out = json!({"revision": REVISION,
        "identity": {"provider": identity.provider, "model": if identity.model.len() <= 256 { Some(identity.model) } else { None },
            "authentication_line": identity.authentication_line},
        "rules": rules, "live_inference_verified": false});
    if identity.provider == "ark" && ctx.protocol == Protocol::Chat {
        out["version_verification"] = json!("required_before_new_capability");
    }
    out
}
