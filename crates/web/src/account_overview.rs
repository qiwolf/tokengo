//! Personal overview transport and an explicit user-visible response shape.
//! Never reuse the administrator LogRow or fetch administrator directories here.
use crate::api::{typed, ApiErr};
use crate::model::{UsageCoverage,KindStat,ModelStat,ClientStat};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Stats {
    pub total:i64,
    pub ok:i64,
    pub failed:i64,
    pub canceled:i64,
    pub client_errors:i64,
    pub input_tokens:i64,
    pub output_tokens:i64,
    #[serde(default)] pub usage:UsageCoverage,
    #[serde(default)] pub by_kind:Vec<KindStat>,
    #[serde(default)] pub by_model:Vec<ModelStat>,
    #[serde(default)] pub by_client:Vec<ClientStat>,
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StatsResp {
    pub window_hours:i64,
    pub stats:Stats,
}
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RequestRow {
    pub id: i64,
    pub ts_ms: i64,
    pub endpoint: String,
    pub model: String,
    #[serde(default)] pub served_model: String,
    pub status: String,
    pub http_status: Option<i64>,
    pub latency_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default)] pub input_tokens_known: bool,
    #[serde(default)] pub output_tokens_known: bool,
    #[serde(default)] pub usage_source: String,
    #[serde(default)] pub key_id: Option<i64>,
    #[serde(default)] pub key_display_name: Option<String>,
    #[serde(default)] pub request_path: Option<String>,
    #[serde(default)] pub first_byte_ms: Option<i64>,
    #[serde(default)] pub streamed: bool,
    #[serde(default)] pub has_image: bool,
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Logs {
    #[serde(default)] pub logs: Vec<RequestRow>,
    pub next_before_id: Option<i64>,
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Attempt {
    pub ordinal: i64,
    pub outcome: String,
    pub http_status: Option<i64>,
    pub duration_ms: i64,
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Detail {
    pub request: RequestRow,
    #[serde(default)] pub attempts: Vec<Attempt>,
}
pub async fn stats(hours: i64) -> Result<StatsResp, ApiErr> {
    typed("GET",&format!("/api/account/overview/stats?hours={hours}"),None).await
}
pub async fn logs(before: Option<i64>) -> Result<Logs, ApiErr> {
    let cursor=before.map(|id|format!("&before_id={id}")).unwrap_or_default();
    typed("GET",&format!("/api/account/overview/logs?limit=12{cursor}"),None).await
}
pub async fn detail(id: i64) -> Result<Detail, ApiErr> {
    typed("GET",&format!("/api/account/overview/requests/{id}"),None).await
}
