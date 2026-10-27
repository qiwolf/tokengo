//! Versioned documentation evidence. This is descriptive metadata, never an
//! authorization, source-capability switch or replacement for live acceptance.
use crate::{parameter_contracts::{self, Context}, protocol::Protocol};
use serde::Serialize;
use serde_json::{json, Value};

pub const REVISION: &str = "2026-10-27-p0.1";

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
