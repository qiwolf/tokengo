//! Admin-only versioned diagnostics. Reads never create verification jobs.
use crate::{api::{self, ApiErr}, model::ReasoningCapability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Identity {
    pub channel_id: i64,
    pub model: String,
    pub upstream_model: String,
    pub source_fingerprint: String,
    pub binding_generation: i64,
    #[serde(default)] pub native: Vec<String>,
    #[serde(default)] pub authentication_line: String,
    #[serde(default)] pub api_version: Option<String>,
    pub contract_revision: String,
    pub credential_revision: Option<i64>,
    pub provider_version: Option<i64>,
}
#[derive(Clone, Deserialize)]
pub struct VerificationLane {
    pub kind: String,
    pub management_page: String,
    pub management_hash: String,
    pub source_id: i64,
    pub model: String,
    pub recommended_kind: String,
    pub direct_native_inference: bool,
    pub note: String,
    pub access: String,
}
#[derive(Clone, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub contract_revision: String,
    pub identity: Identity,
    pub purpose: Value,
    pub reasoning: ReasoningCapability,
    pub budget: Value,
    pub verification_lane: VerificationLane,
    #[serde(default)] pub contracts: Vec<Value>,
    #[serde(default)] pub runtime_identity: Value,
    #[serde(default)] pub fields: Vec<Value>,
    #[serde(default)] pub evidence: Value,
    #[serde(default)] pub account_availability: Value,
    pub availability: Value,
    #[serde(default)] pub verification: Option<Job>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Job {
    pub schema_version: u32,
    pub id: String,
    pub identity: Identity,
    pub mode: String,
    #[serde(default)] pub native: Option<String>,
    pub state: String,
    pub started_at_ms: i64,
    #[serde(default)] pub finished_at_ms: Option<i64>,
    pub deadline_at_ms: i64,
    #[serde(default)] pub http_status: Option<u16>,
    #[serde(default)] pub note: String,
    #[serde(default)] pub candidate: Value,
}
fn base(channel: i64) -> String { format!("/api/channels/{channel}/models/compatibility") }
fn query_model(model: &str) -> String { js_sys::encode_uri_component(model).as_string().unwrap_or_default() }
pub async fn read(channel: i64, model: &str) -> Result<Snapshot, ApiErr> {
    api::typed("GET", &format!("{}?model={}",base(channel),query_model(model)),None).await
}
pub async fn verify(channel: i64, snapshot: &Snapshot, mode: &str, native: &str) -> Result<Job, ApiErr> {
    let identity = &snapshot.identity;
    api::typed("POST", &format!("{}/verify",base(channel)),Some(json!({
        "model": identity.model, "mode": mode, "native": native,
        "expected_identity": identity
    }))).await
}
pub async fn job(channel: i64, id: &str) -> Result<Job, ApiErr> {
    api::typed("GET",&format!("{}/jobs/{}",base(channel),query_model(id)),None).await
}
pub async fn cancel(channel: i64, id: &str) -> Result<(), ApiErr> {
    api::call("DELETE",&format!("{}/jobs/{}",base(channel),query_model(id)),None).await.map(|_|())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expected_identity_preserves_contract_and_credential_epochs() {
        let value=json!({"channel_id":7,"model":"alias","upstream_model":"upstream","source_fingerprint":"redacted-safe-hash","binding_generation":3,"native":["responses"],"authentication_line":"api_key","api_version":null,"contract_revision":"v1","credential_revision":11,"provider_version":9});
        let identity:Identity=serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&identity).unwrap(),value);
        let mut changed=identity.clone();changed.credential_revision=Some(12);
        assert_ne!(identity,changed);
        changed=identity.clone();changed.contract_revision="v2".into();assert_ne!(identity,changed);
    }
}
