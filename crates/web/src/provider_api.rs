//! Explicit provider identities; connection IDs and credentials remain separate.
use crate::api::{ApiErr, call, typed};
use serde::Deserialize;
use serde_json::json;
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ProviderModel {
    #[serde(default = "yes")]
    pub visible: bool,
    pub name: String,
    pub global_enabled: bool,
    pub declared_connection_ids: Vec<i64>,
    pub enabled_connection_ids: Vec<i64>,
    pub routable_connection_ids: Vec<i64>,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Provider {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub connection_ids: Vec<i64>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub canonical_state: String,
    #[serde(default)]
    pub key_strategy: String,
    #[serde(default = "yes")]
    pub switch_on_429: bool,
    #[serde(default)]
    pub switch_on_400: bool,
    pub models: Vec<ProviderModel>,
}
#[derive(Clone, Deserialize)]
pub struct Providers {
    pub providers: Vec<Provider>,
    pub ungrouped_connection_ids: Vec<i64>,
}
pub async fn list() -> Result<Providers, ApiErr> {
    typed("GET", "/api/providers", None).await
}
pub async fn save(id: Option<i64>, name: &str, enabled: bool) -> Result<(), ApiErr> {
    let path = id
        .map(|i| format!("/api/providers/{i}"))
        .unwrap_or_else(|| "/api/providers".into());
    call(
        if id.is_some() { "PUT" } else { "POST" },
        &path,
        Some(json!({"name":name,"enabled":enabled})),
    )
    .await
    .map(|_| ())
}
pub async fn assign(connection: i64, provider: Option<i64>) -> Result<(), ApiErr> {
    call(
        "PUT",
        &format!("/api/channels/{connection}/provider"),
        Some(json!({"provider_id":provider})),
    )
    .await
    .map(|_| ())
}
pub async fn toggle(provider: i64, model: &str, enabled: bool, ids: &[i64]) -> Result<(), ApiErr> {
    call(
        "PUT",
        &format!("/api/providers/{provider}/models/enabled"),
        Some(json!({"model":model,"enabled":enabled,"connection_ids":ids})),
    )
    .await
    .map(|_| ())
}
pub async fn toggle_connection(connection: i64, model: &str, enabled: bool) -> Result<(), ApiErr> {
    call(
        "PUT",
        &format!("/api/channels/{connection}/models/enabled"),
        Some(json!({"model":model,"enabled":enabled})),
    )
    .await
    .map(|_| ())
}

/// Only empty providers can be removed. Sources and historical identities never cascade.
pub async fn remove(id: i64) -> Result<(), ApiErr> {
    call("DELETE", &format!("/api/providers/{id}"), None)
        .await
        .map(|_| ())
}

pub async fn save_config(id: Option<i64>, body: serde_json::Value) -> Result<(), ApiErr> {
    let path = id
        .map(|i| format!("/api/providers/{i}"))
        .unwrap_or_else(|| "/api/providers".into());
    call(if id.is_some() { "PUT" } else { "POST" }, &path, Some(body))
        .await
        .map(|_| ())
}
