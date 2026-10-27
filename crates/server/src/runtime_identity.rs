//! Bounded, unverified runtime evidence. No network access and no capability inference.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime { Ollama, Vllm }
impl Runtime {
    fn profile(self) -> &'static str { match self { Self::Ollama => "ollama", Self::Vllm => "vllm" } }
}
/// Both values are claims, including data supplied to a pure standard-API parser.
/// Only a future authorized collector can establish origin; neither is verified here.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence { OperatorDeclared, StandardApiClaim }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelIdentity {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_revision_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_parser_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_parser_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_config_sha256: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeIdentity {
    pub runtime: Runtime,
    pub evidence: Evidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub models: Vec<ModelIdentity>,
}
fn identifier(value: &str, max: usize, model: bool) -> bool {
    !value.is_empty() && value.len() <= max && !value.contains("..")
        && !value.starts_with('/') && !value.contains("://")
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.'|b'_'|b'-'|b'+') || model && matches!(b,b'/'|b':'))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || matches!(b,b'a'..=b'f'))
}
impl RuntimeIdentity {
    pub fn validate(&self, profile: Option<&str>) -> Result<(), String> {
        if profile != Some(self.runtime.profile()) {
            return Err("Runtime identity requires the matching explicit runtime parameter profile".into());
        }
        if self.version.as_deref().is_some_and(|v| !identifier(v,64,false)) {
            return Err("Invalid bounded runtime version identifier".into());
        }
        if self.models.is_empty() || self.models.len() > 128 {
            return Err("Runtime identity requires one to 128 exact model records".into());
        }
        for (i, model) in self.models.iter().enumerate() {
            if !identifier(&model.model,256,true) || self.models[..i].iter().any(|old|old.model==model.model) {
                return Err("Invalid or duplicate runtime model identifier".into());
            }
            for digest in [&model.model_revision_sha256,&model.template_sha256,&model.reasoning_parser_sha256,
                &model.tool_parser_sha256,&model.generation_config_sha256].into_iter().flatten() {
                if !valid_digest(digest) { return Err("Runtime evidence requires lowercase SHA-256 digests".into()); }
            }
        }
        Ok(())
    }
    /// Descriptions never echo versions, identifiers, raw templates or configuration.
    /// Exact model comparison prevents evidence for aliases or other models leaking in.
    pub fn describe(&self, model: &str) -> Value {
        let record = self.models.iter().find(|row|row.model == model);
        json!({"status":if record.is_some(){"unverified"}else{"unknown"},"runtime":self.runtime,
            "evidence":self.evidence,"origin_verified":false,"capabilities":"unknown",
            "authentication":"not_established","version_known":self.version.is_some(),
            "model_revision_known":record.is_some_and(|r|r.model_revision_sha256.is_some()),
            "template_known":record.is_some_and(|r|r.template_sha256.is_some()),
            "reasoning_parser_known":record.is_some_and(|r|r.reasoning_parser_sha256.is_some()),
            "tool_parser_known":record.is_some_and(|r|r.tool_parser_sha256.is_some()),
            "generation_config_known":record.is_some_and(|r|r.generation_config_sha256.is_some()),
            "identity_sha256":record.and_then(|row|serde_json::to_vec(&(self.runtime,&self.version,row)).ok())
                .map(|bytes|hex::encode(Sha256::digest(bytes)))})
    }
}
fn empty_model(model: &str) -> ModelIdentity {
    ModelIdentity { model:model.into(), model_revision_sha256:None,template_sha256:None,
        reasoning_parser_sha256:None,tool_parser_sha256:None,generation_config_sha256:None }
}
fn version_claim(value: &Value) -> Result<Option<String>,String> {
    match value.get("version") {
        None => Ok(None),
        Some(Value::String(v)) if identifier(v,64,false) => Ok(Some(v.clone())),
        _ => Err("Invalid standard API runtime version claim".into()),
    }
}
fn text_digest(value: Option<&Value>) -> Result<Option<String>,String> {
    match value {
        None => Ok(None),
        Some(Value::String(v)) if !v.is_empty() && v.len()<=262_144 => Ok(Some(hex::encode(Sha256::digest(v.as_bytes())))),
        _ => Err("Invalid or oversized standard API identity component".into()),
    }
}
/// Parses only documented /api/version and /api/show fields. The model is supplied
/// by the authorized request context, never guessed from show output or capabilities.
/// No raw template/parameter/modelfile survives, and parameters are not falsely
/// labeled as the full generation_config (that remains unknown).
pub fn ollama_standard_api_claim(model: &str, version: &Value, show: &Value) -> Result<RuntimeIdentity,String> {
    if !show.is_object() || !version.is_object() { return Err("Invalid standard API object".into()); }
    let mut row=empty_model(model);
    row.template_sha256=text_digest(show.get("template"))?;
    let result=RuntimeIdentity {runtime:Runtime::Ollama,evidence:Evidence::StandardApiClaim,
        version:version_claim(version)?,models:vec![row]};
    result.validate(Some("ollama"))?;
    Ok(result)
}
/// /v1/models identifies model names only. owned_by, capabilities and arbitrary
/// template/parser keys in that response are ignored; none proves deployment flags.
pub fn vllm_standard_api_claim(version: &Value, models: &Value) -> Result<RuntimeIdentity,String> {
    if !version.is_object() { return Err("Invalid standard API object".into()); }
    let rows=models.get("data").and_then(Value::as_array)
        .filter(|rows|!rows.is_empty() && rows.len()<=128)
        .ok_or("Invalid bounded standard model list")?;
    let mut records=Vec::with_capacity(rows.len());
    for row in rows {
        let model=row.get("id").and_then(Value::as_str).ok_or("Invalid standard model identifier")?;
        records.push(empty_model(model));
    }
    let result=RuntimeIdentity {runtime:Runtime::Vllm,evidence:Evidence::StandardApiClaim,
        version:version_claim(version)?,models:records};
    result.validate(Some("vllm"))?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_api_claims_are_partial_unverified_and_redacted() {
        let source=json!({"template":"secret template /private/model/path", "modelfile":"api_key=secret",
            "capabilities":["thinking","tools"],"reasoning_effort_levels":["high"]});
        let identity=ollama_standard_api_claim("qwen3:8b",&json!({"version":"0.13.3"}),&source).unwrap();
        let encoded=serde_json::to_string(&identity).unwrap();
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("private"));
        let description=identity.describe("qwen3:8b");
        assert_eq!(description["status"],"unverified");
        assert_eq!(description["capabilities"],"unknown");
        assert_eq!(description["generation_config_known"],false);
        assert_eq!(identity.describe("Qwen3:8b")["status"],"unknown");
        let other=ollama_standard_api_claim("qwen3:8b",&json!({"version":"0.13.3"}),&json!({"template":"different"})).unwrap();
        assert_ne!(description["identity_sha256"],other.describe("qwen3:8b")["identity_sha256"]);
        let vllm=vllm_standard_api_claim(&json!({"version":"0.11.0"}),&json!({"data":[{"id":"qwen3:8b","owned_by":"vllm","template":"invented","reasoning_effort_levels":["high"]}]})).unwrap();
        assert_eq!(vllm.describe("qwen3:8b")["template_known"],false);
        assert_eq!(vllm.describe("qwen3:8b")["capabilities"],"unknown");
        assert_ne!(description["identity_sha256"],vllm.describe("qwen3:8b")["identity_sha256"]);
    }
    #[test]
    fn invalid_identifiers_or_oversized_evidence_do_not_echo_input() {
        for bad in ["https://example.test/?key=secret", "/private/secret", "qwen\nsecret"] {
            let error=ollama_standard_api_claim(bad,&json!({}),&json!({})).unwrap_err();
            assert!(!error.contains("secret"));
        }
        assert!(ollama_standard_api_claim("qwen",&json!({"version":"https://secret"}),&json!({})).is_err());
        assert!(ollama_standard_api_claim("qwen",&json!({}),&json!({"template":"x".repeat(262145)})).is_err());
        assert!(vllm_standard_api_claim(&json!({}),&json!({"data":[{"id":"same"},{"id":"same"}]})).is_err());
    }
}
