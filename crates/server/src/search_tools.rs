//! DeepSeek standalone JSON tools; request units are never represented as tokens.
//! Contract: https://api-docs.deepseek.com/guides/search/
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Search,
    Fetch,
}
impl ToolKind {
    pub fn path(self) -> &'static str {
        match self {
            Self::Search => "/v1/tools/search",
            Self::Fetch => "/v1/tools/fetch",
        }
    }
}
fn text<'a>(body: &'a Value, field: &str, max: usize, nonblank: bool) -> Result<&'a str, String> {
    let value = body
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{field} must be a string"))?;
    let len = value.chars().count();
    if len == 0 || len > max || (nonblank && value.trim().is_empty()) {
        return Err(format!("{field} must contain 1..{max} Unicode characters"));
    }
    Ok(value)
}
fn http_url(value: &str) -> Result<(), String> {
    if value.chars().count() > 8192 || value.chars().any(char::is_control) || value.trim() != value
    {
        return Err("invalid URL".into());
    }
    let parsed = reqwest::Url::parse(value).map_err(|_| "invalid URL")?;
    // Require an explicit absolute http(s) authority; do not silently normalize
    // backslashes or empty userinfo, both accepted by browser-style URL parsers.
    let authority = value
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""));
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || authority.is_none_or(|a| a.is_empty() || a.contains('@') || a.contains('\\'))
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("URL must be HTTP(S), contain a host and no credentials".into());
    }
    Ok(())
}
/// Apply documented defaults and discard unknown fields, as the official API does.
/// Logical route/model and gateway auth metadata must never be sent as tool params.
pub fn normalize_request(kind: ToolKind, body: &Value) -> Result<Value, String> {
    if !body.is_object() {
        return Err("tool request must be a JSON object".into());
    }
    match kind {
        ToolKind::Search => {
            let query = text(body, "query", 128, true)?;
            let count = match body.get("count") {
                None => 10,
                Some(v) => v
                    .as_u64()
                    .filter(|n| (1..=50).contains(n))
                    .ok_or("count must be an integer in 1..50")?,
            };
            let need = match body.get("need_content") {
                None => false,
                Some(v) => v.as_bool().ok_or("need_content must be boolean")?,
            };
            let localization = match body.get("localization") {
                None => "",
                Some(v) => v.as_str().ok_or("localization must be an IP string")?,
            };
            if !localization.is_empty() && localization.parse::<IpAddr>().is_err() {
                return Err("localization must be an IP address or empty".into());
            }
            let mut out = json!({"query":query,"count":count,"need_content":need,"localization":localization});
            if body.get("ori_question").is_some_and(|v| !v.is_null()) {
                out["ori_question"] = Value::String(text(body, "ori_question", 128, false)?.into());
            }
            Ok(out)
        }
        ToolKind::Fetch => {
            let url = text(body, "url", 8192, false)?;
            http_url(url)?;
            Ok(json!({"url":url}))
        }
    }
}
/// Only a validated, complete HTTP200 body can establish a billable result.
/// Errors/non200 return known zero upstream units. A missing/truncated/invalid
/// 200 body is an error, so callers must retain UNKNOWN rather than invent zero.
pub fn response_units(kind: ToolKind, status: u16, body: &Value) -> Result<u8, String> {
    if status != 200 {
        return Ok(0);
    }
    if !body.is_object() || body.get("error").is_some_and(|v| !v.is_null()) {
        return Err("invalid tool success response".into());
    }
    match kind {
        ToolKind::Search => {
            let rows = body
                .get("results")
                .and_then(Value::as_array)
                .ok_or("search response requires results array")?;
            for row in rows {
                if !row.is_object() {
                    return Err("search result must be an object".into());
                }
                for field in ["title", "url", "snippet", "summary", "content"] {
                    if !row.get(field).is_some_and(Value::is_string) {
                        return Err(format!("search result requires {field} string"));
                    }
                }
                let score = row
                    .get("rerank_score")
                    .and_then(Value::as_f64)
                    .filter(|n| n.is_finite() && (0.0..=3.0).contains(n))
                    .ok_or("invalid rerank_score")?;
                let _ = score;
                let authority = row
                    .get("authority")
                    .and_then(Value::as_i64)
                    .ok_or("invalid authority")?;
                if !(-1..=4).contains(&authority) {
                    return Err("invalid authority".into());
                }
                for field in ["site_name", "site_icon", "date"] {
                    if !row.get(field).is_some_and(|v| v.is_null() || v.is_string()) {
                        return Err(format!("search result requires nullable {field}"));
                    }
                }
            }
            Ok(u8::from(!rows.is_empty()))
        }
        ToolKind::Fetch => {
            if !body.get("title").is_some_and(Value::is_string) {
                return Err("fetch response requires title string".into());
            }
            let content = body
                .get("content")
                .and_then(Value::as_str)
                .ok_or("fetch response requires content string")?;
            Ok(u8::from(!content.trim().is_empty()))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_defaults_and_unknown_fields() {
        let s = "中".repeat(128);
        let p = normalize_request(
            ToolKind::Search,
            &json!({"query":s,"model":"route","unknown":true,"ori_question":null}),
        )
        .unwrap();
        assert_eq!(p["count"], 10);
        assert_eq!(p["need_content"], false);
        assert!(p.get("model").is_none());
        assert!(normalize_request(ToolKind::Search, &json!({"query":"中".repeat(129)})).is_err());
    }
    #[test]
    fn rejects_wrong_types_ranges_and_ip() {
        for x in [
            json!({"query":" "}),
            json!({"query":"x","count":true}),
            json!({"query":"x","count":51}),
            json!({"query":"x","need_content":1}),
            json!({"query":"x","localization":"not-ip"}),
            json!({"query":"x","ori_question":""}),
        ] {
            assert!(normalize_request(ToolKind::Search, &x).is_err());
        }
        assert!(
            normalize_request(ToolKind::Search, &json!({"query":"x","localization":"::1"})).is_ok()
        );
    }
    #[test]
    fn fetch_rejects_credentials_and_nonhttp() {
        for url in [
            "file:///tmp/x",
            "https://user@example.com",
            "http://@example.com",
            "http://user:pass@example.com",
            "https:\\example.com",
            "https://",
            " https://example.com",
        ] {
            assert!(
                normalize_request(ToolKind::Fetch, &json!({"url":url})).is_err(),
                "{url}"
            );
        }
        assert!(
            normalize_request(ToolKind::Fetch, &json!({"url":"https://example.com/a?q=1"})).is_ok()
        );
    }
    #[test]
    fn valid_empty_and_invalid_are_distinct() {
        assert_eq!(
            response_units(ToolKind::Search, 200, &json!({"results":[]})).unwrap(),
            0
        );
        assert_eq!(
            response_units(ToolKind::Fetch, 200, &json!({"title":"","content":" hi "})).unwrap(),
            1
        );
        assert_eq!(
            response_units(ToolKind::Fetch, 200, &json!({"title":"","content":""})).unwrap(),
            0
        );
        assert!(response_units(ToolKind::Search, 200, &json!({"results":[{}]})).is_err());
        assert_eq!(
            response_units(ToolKind::Search, 429, &Value::Null).unwrap(),
            0
        );
    }
    #[test]
    fn request_count_not_result_count() {
        let row = json!({"rerank_score":2.4,"title":"title","url":"https://example.com","snippet":"s","summary":"s","content":"","site_name":null,"site_icon":null,"date":null,"authority":-1});
        assert_eq!(
            response_units(ToolKind::Search, 200, &json!({"results":[row.clone(),row]})).unwrap(),
            1
        );
    }
}

/// Explicit tool declarations, separate from Chat/vision/protocol capabilities.
/// account_scope is an administrator-chosen opaque grouping label, not a guess
/// from API-key bytes or channel name. A shared label shares each tool's QPS.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ToolCapabilities {
    #[serde(default)]
    pub search: bool,
    #[serde(default)]
    pub fetch: bool,
    #[serde(default)]
    pub account_scope: String,
    #[serde(default = "default_qps")]
    pub qps: u32,
}
fn default_qps() -> u32 {
    5
}
impl Default for ToolCapabilities {
    fn default() -> Self {
        Self {
            search: false,
            fetch: false,
            account_scope: String::new(),
            qps: 5,
        }
    }
}
impl ToolCapabilities {
    pub fn permits(&self, kind: ToolKind) -> bool {
        match kind {
            ToolKind::Search => self.search,
            ToolKind::Fetch => self.fetch,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.qps == 0 || self.qps > 10_000 {
            return Err("tool qps must be 1..10000".into());
        }
        if self.account_scope.chars().count() > 128
            || self.account_scope.chars().any(char::is_control)
            || ((self.search || self.fetch) && self.account_scope.trim().is_empty())
        {
            return Err("enabled tools require an explicit account_scope grouping label".into());
        }
        Ok(())
    }
}
pub fn load(conn: &rusqlite::Connection, channel_id: i64) -> crate::db::Result<ToolCapabilities> {
    match crate::db::settings::get(conn, &format!("channel_tools:{channel_id}"))? {
        None => Ok(ToolCapabilities::default()),
        Some(raw) => {
            let value: ToolCapabilities = serde_json::from_str(&raw).map_err(|_| {
                crate::db::DbError::Other("invalid tool capability configuration".into())
            })?;
            value.validate().map_err(crate::db::DbError::Other)?;
            Ok(value)
        }
    }
}
pub fn save(
    conn: &rusqlite::Connection,
    channel_id: i64,
    cap: &ToolCapabilities,
) -> crate::db::Result<()> {
    cap.validate().map_err(crate::db::DbError::Other)?;
    if channel_id <= 0 {
        return Err(crate::db::DbError::Other("invalid channel id".into()));
    }
    let key = format!("channel_tools:{channel_id}");
    let raw = serde_json::to_string(cap)
        .map_err(|_| crate::db::DbError::Other("cannot serialize tool capabilities".into()))?;
    crate::db::in_tx(conn, |c| {
        let old = crate::db::settings::get(c, &key)?;
        if old.as_deref() != Some(&raw) {
            let changed = c.execute(
                "UPDATE channels SET binding_generation=binding_generation+1 WHERE id=?1",
                [channel_id],
            )?;
            if changed != 1 {
                return Err(crate::db::DbError::Other("channel does not exist".into()));
            }
            crate::db::settings::set(c, &key, &raw)?;
        }
        Ok(())
    })
}
pub fn router() -> axum::Router<crate::SharedState> {
    axum::Router::new().route(
        "/api/channels/{id}/tools",
        axum::routing::get(read_tools).put(write_tools),
    )
}
async fn read_tools(
    _auth: crate::routes::operations::ManagementAuth,
    axum::extract::State(state): axum::extract::State<crate::SharedState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<axum::Json<Value>, crate::routes::ApiError> {
    let c = state.conn();
    if crate::db::channels::get(&c, tokengo_shared::ChannelId(id))?.is_none() {
        return Err(crate::routes::ApiError::not_found("channel does not exist"));
    }
    Ok(axum::Json(json!({"tools":load(&c,id)?})))
}
async fn write_tools(
    _auth: crate::routes::operations::ManagementAuth,
    axum::extract::State(state): axum::extract::State<crate::SharedState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    axum::Json(body): axum::Json<ToolCapabilities>,
) -> Result<axum::Json<Value>, crate::routes::ApiError> {
    body.validate()
        .map_err(crate::routes::ApiError::bad_request)?;
    let c = state.conn();
    if crate::db::channels::get(&c, tokengo_shared::ChannelId(id))?.is_none() {
        return Err(crate::routes::ApiError::not_found("channel does not exist"));
    }
    save(&c, id, &body)?;
    Ok(axum::Json(json!({"tools":body})))
}
#[cfg(test)]
mod capability_tests {
    use super::*;
    #[test]
    fn capabilities_explicit_and_binding_generation_changes_only_with_config() {
        let c = crate::db::open_in_memory().unwrap();
        let id = crate::db::channels::create(
            &c,
            &crate::db::channels::NewChannel {
                name: "DeepSeek Search is only a name".into(),
                base_url: "https://example.invalid".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap()
        .0;
        assert!(!load(&c, id).unwrap().search);
        let before: i64 = c
            .query_row(
                "SELECT binding_generation FROM channels WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        let cap = ToolCapabilities {
            search: true,
            fetch: false,
            account_scope: "account-A".into(),
            qps: 5,
        };
        save(&c, id, &cap).unwrap();
        save(&c, id, &cap).unwrap();
        assert_eq!(load(&c, id).unwrap(), cap);
        let after: i64 = c
            .query_row(
                "SELECT binding_generation FROM channels WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, before + 1);
        let invalid = ToolCapabilities {
            search: true,
            account_scope: String::new(),
            ..Default::default()
        };
        assert!(save(&c, id, &invalid).is_err());
        assert!(save(&c, 999, &cap).is_err());
    }
}
