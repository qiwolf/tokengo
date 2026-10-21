//! Explicit machine authority. Root/session authorities remain separate.
use super::{ApiError, accounts::RootGrant};
use crate::{SharedState, db};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{FromRequestParts, MatchedPath, Request, State},
    http::{StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct OperationsContext {
    pub token_id: i64,
    pub request_id: String,
    pub method: String,
    pub path: String,
}
tokio::task_local! { pub static CONTEXT: OperationsContext; }
pub fn actor_id() -> Option<i64> {
    CONTEXT.try_with(|v| v.token_id).ok()
}

/// Registry is also the capabilities response. No prefix grants or implicit new routes.
const ACTIONS: &[(&str, &str)] = &[
    ("GET", "/livez"),
    ("GET", "/readyz"),
    ("GET", "/api/ping"),
    ("GET", "/api/ops/me"),
    ("GET", "/api/ops/capabilities"),
    ("GET", "/api/ops/audit"),
    ("GET", "/api/channels"),
    ("POST", "/api/channels"),
    ("PUT", "/api/channels/{id}"),
    ("DELETE", "/api/channels/{id}"),
    ("PUT", "/api/channels/{id}/enabled"),
    ("POST", "/api/channels/{id}/probe"),
    ("POST", "/api/channels/{id}/models"),
    ("GET", "/api/channels/{id}/catalog"),
    ("GET", "/api/channels/{id}/registry"),
    ("PUT", "/api/channels/{id}/registry"),
    ("GET", "/api/channels/{id}/context"),
    ("PUT", "/api/channels/{id}/context"),
    ("POST", "/api/channels/{id}/vision-test"),
    ("POST", "/api/channels/{id}/move"),
    ("GET", "/api/providers"),
    ("POST", "/api/providers"),
    ("PUT", "/api/providers/{id}"),
    ("DELETE", "/api/providers/{id}"),
    ("GET", "/api/providers/{id}/credentials"),
    ("POST", "/api/providers/{id}/credentials"),
    ("PATCH", "/api/providers/{id}/credentials/{credential}"),
    ("DELETE", "/api/providers/{id}/credentials/{credential}"),
    ("POST", "/api/providers/{id}/models/discover"),
    ("PUT", "/api/providers/{id}/models/enabled"),
    ("PUT", "/api/channels/{id}/provider"),
    ("PUT", "/api/channels/{id}/models/enabled"),
    ("PUT", "/api/channels/{id}/models/reasoning"),
    ("PUT", "/api/model-visibility"),
    ("GET", "/api/channels/{id}/tools"),
    ("PUT", "/api/channels/{id}/tools"),
    ("GET", "/api/model-kinds"),
    ("GET", "/api/auto-policy"),
    ("PUT", "/api/auto-policy"),
    ("POST", "/api/model-order/move"),
    ("PUT", "/api/model-enabled"),
    ("POST", "/api/channel-faults/retry"),
    ("GET", "/api/model-cooldowns"),
    ("POST", "/api/model-cooldown/reset"),
    ("PUT", "/api/models/{name}/kind"),
    ("GET", "/api/model-routes"),
    ("GET", "/api/routing"),
    ("PUT", "/api/routing/config"),
    ("POST", "/api/routing/{model}/reprobe"),
    ("GET", "/api/model-routing"),
    ("PUT", "/api/model-routing"),
    ("DELETE", "/api/model-routing"),
    ("GET", "/api/stats"),
    ("GET", "/api/logs"),
    ("GET", "/api/requests/{id}"),
    ("GET", "/api/logs/live"),
    ("GET", "/api/logs/level"),
    ("PUT", "/api/logs/level"),
    ("GET", "/api/settings"),
    ("GET", "/api/maintenance"),
    ("POST", "/api/maintenance"),
    ("GET", "/api/gateway-keys"),
    ("POST", "/api/gateway-keys"),
    ("DELETE", "/api/gateway-keys/{id}"),
    ("GET", "/api/users"),
    ("POST", "/api/users"),
    ("GET", "/api/users/{id}"),
    ("PATCH", "/api/users/{id}"),
    ("PUT", "/api/users/{id}"),
    ("DELETE", "/api/users/{id}"),
    ("GET", "/api/users/{id}/keys"),
    ("POST", "/api/users/{id}/keys"),
    ("PATCH", "/api/vkeys/{id}"),
    ("GET", "/api/managed-users"),
    ("POST", "/api/managed-users"),
    ("PATCH", "/api/managed-users/{id}"),
    ("DELETE", "/api/managed-users/{id}"),
    ("POST", "/api/managed-users/{id}/login-credential"),
    ("GET", "/api/pricing"),
    ("POST", "/api/pricing"),
    ("GET", "/api/request-pricing"),
    ("POST", "/api/request-pricing"),
    ("GET", "/api/budgets"),
    ("PUT", "/api/budgets"),
    ("GET", "/api/usage"),
    ("GET", "/api/custom-probes"),
    ("POST", "/api/custom-probes"),
    ("PUT", "/api/custom-probes/{id}"),
    ("DELETE", "/api/custom-probes/{id}"),
    ("POST", "/api/custom-probes/{id}/run"),
];
fn matches_path(template: &str, path: &str) -> bool {
    let a: Vec<_> = template.split('/').collect();
    let b: Vec<_> = path.split('/').collect();
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            if x.starts_with('{') {
                !y.is_empty()
            } else {
                *x == y
            }
        })
}
pub fn allowed(method: &str, path: &str) -> bool {
    ACTIONS
        .iter()
        .any(|(m, p)| *m == method && matches_path(p, path))
}
static AUTH_FAILURES: [std::sync::atomic::AtomicU64; 4] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 4];
static AUTH_LAST_LOG: [std::sync::atomic::AtomicI64; 4] =
    [const { std::sync::atomic::AtomicI64::new(0) }; 4];
fn auth_failure(category: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    let count = AUTH_FAILURES[category].fetch_add(1, Relaxed) + 1;
    let now = db::now_ms();
    let previous = AUTH_LAST_LOG[category].load(Relaxed);
    if now - previous >= 30_000
        && AUTH_LAST_LOG[category]
            .compare_exchange(previous, now, Relaxed, Relaxed)
            .is_ok()
    {
        let reason = [
            "invalid_or_inactive",
            "mixed_credentials",
            "url_credentials",
            "storage_unavailable",
        ][category];
        tracing::warn!(reason, count, "Operations authentication refused");
    }
}
fn invalid() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_operations_token",
        "Operations credential is invalid",
    )
}
fn forbidden() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "operations_action_forbidden",
        "This action is not permitted for operations tokens",
    )
}
fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

pub async fn transport_gate(request: Request, next: Next) -> Response {
    if request.uri().query().is_some_and(|q| {
        url::form_urlencoded::parse(q.as_bytes()).any(|(k, _)| {
            matches!(
                k.as_ref(),
                "token" | "access_token" | "ops_token" | "operations_token"
            )
        })
    }) {
        auth_failure(2);
        return no_store(invalid().into_response());
    }
    next.run(request).await
}

pub async fn middleware(
    State(state): State<SharedState>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let values = headers
        .get_all(header::AUTHORIZATION)
        .iter()
        .collect::<Vec<_>>();
    if values.len() > 1 {
        let machine = values.iter().filter_map(|v| v.to_str().ok()).any(|v| {
            v.split_once(' ').is_some_and(|(scheme, raw)| {
                scheme.eq_ignore_ascii_case("bearer") && raw.trim().starts_with("tgo_ops_")
            })
        });
        if machine || request.uri().path().starts_with("/api/") {
            auth_failure(1);
            return no_store(invalid().into_response());
        }
        // Native inference protocols retain their own duplicate-header error contract.
        return next.run(request).await;
    }
    let raw = values
        .first()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, s)| s.trim());
    let Some(raw) = raw.filter(|s| s.starts_with("tgo_ops_")) else {
        return next.run(request).await;
    };
    if headers.contains_key(header::COOKIE) || raw.len() > 256 {
        auth_failure(1);
        return no_store(invalid().into_response());
    }
    let token = match db::operations_tokens::authenticate(&state.conn(), raw) {
        Ok(Some(v)) => v,
        Ok(None) => {
            auth_failure(0);
            return no_store(invalid().into_response());
        }
        Err(_) => {
            return no_store({
                auth_failure(3);
                ApiError::internal("Operations credential storage unavailable").into_response()
            });
        }
    };
    let path = request.uri().path().to_owned();
    let method = request.method().as_str().to_owned();
    // MatchedPath distinguishes actual routes from the SPA fallback, including parameterized routes.
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str())
        .unwrap_or("");
    if !allowed(&method, &path) {
        let status = if matched.is_empty() || matched.contains("{*") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::FORBIDDEN
        };
        let mut error = forbidden();
        error.status = status;
        let _ = db::operations_tokens::audit(
            &state.conn(),
            "operations_token",
            token.id,
            "operations.denied",
            "route",
            None,
            None,
            false,
        );
        return no_store(error.into_response());
    }
    // A token in a URL is never accepted even if Authorization is also supplied.
    if request.uri().query().is_some_and(|q| {
        url::form_urlencoded::parse(q.as_bytes()).any(|(k, _)| {
            matches!(
                k.as_ref(),
                "token" | "access_token" | "ops_token" | "operations_token"
            )
        })
    }) {
        return no_store(invalid().into_response());
    }
    // Refuse body credential aliases as well; reinsert exactly the same bytes for handlers.
    let (request_parts, request_body) = request.into_parts();
    let body_bytes = match to_bytes(request_body, 2 * 1024 * 1024).await {
        Ok(v) => v,
        Err(_) => {
            return no_store(
                ApiError::bad_request("Operations request body too large").into_response(),
            );
        }
    };
    if let Ok(Value::Object(ref object)) = serde_json::from_slice::<Value>(&body_bytes) {
        if object.keys().any(|key| {
            matches!(
                key.as_str(),
                "token" | "access_token" | "ops_token" | "operations_token"
            )
        }) {
            return no_store(invalid().into_response());
        }
        if method == "POST"
            && path.ends_with("/credentials")
            && (object.get("kind").and_then(Value::as_str) == Some("oauth")
                || object.get("oauth_account_id").is_some_and(|v| !v.is_null()))
        {
            return no_store(forbidden().into_response());
        }
    }
    let mut request = Request::from_parts(request_parts, Body::from(body_bytes));
    let context = OperationsContext {
        token_id: token.id,
        request_id: uuid::Uuid::new_v4().to_string(),
        method: method.clone(),
        path: path.clone(),
    };
    request.extensions_mut().insert(context.clone());
    let result = CONTEXT
        .scope(context.clone(), async {
            // Memory-only mutations need a durable intent record before touching state.
            if method != "GET" && method != "HEAD" {
                if db::operations_tokens::audit(
                    &state.conn(),
                    "operations_token",
                    token.id,
                    "operations.intent",
                    "route",
                    None,
                    Some(&context.request_id),
                    true,
                )
                .is_err()
                {
                    return ApiError::internal("Operations audit unavailable").into_response();
                }
            }
            next.run(request).await
        })
        .await;
    // Fail-closed response projection: never return arbitrary diagnostic/error text.
    let status = result.status();
    let (parts, body) = result.into_parts();
    let bytes = match to_bytes(body, 4 * 1024 * 1024).await {
        Ok(v) => v,
        Err(_) => {
            return no_store(ApiError::internal("Operations response unavailable").into_response());
        }
    };
    let mut value: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) if !status.is_success() => Value::Null,
        Err(_) => {
            return no_store(ApiError::internal("Operations response is not JSON").into_response());
        }
    };
    let final_status = if status.is_success() {
        project(&path, &method, &mut value);
        if reject_stored_credential_echo(&state, &value).is_err() {
            value = json!({"error":{"type":"unsafe_operations_response","message":"Response contains unsafe or unavailable credential-derived data"}});
            StatusCode::BAD_GATEWAY
        } else {
            status
        }
    } else {
        let inactive = bytes
            .windows(b"operations_token_inactive".len())
            .any(|w| w == b"operations_token_inactive");
        let code = if inactive {
            StatusCode::UNAUTHORIZED
        } else {
            status
        };
        value = json!({"error":{"type":if inactive {"invalid_operations_token"} else {"operations_request_failed"},"message":format!("Operations request failed (HTTP {})",code.as_u16())}});
        code
    };
    let audit_result = db::operations_tokens::audit(
        &state.conn(),
        "operations_token",
        token.id,
        "operations.request",
        "route",
        None,
        Some(&context.request_id),
        final_status.is_success(),
    );
    if audit_result.is_err() {
        return no_store(ApiError::internal("Operations audit unavailable").into_response());
    }
    let mut response = (final_status, Json(value)).into_response();
    if let Some(id) = parts.headers.get("x-request-id") {
        response.headers_mut().insert("x-request-id", id.clone());
    }
    no_store(response)
}

/// Accept root only through the original extractor, or an already classified machine request.
pub struct ManagementAuth;
impl FromRequestParts<SharedState> for ManagementAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, ApiError> {
        if let Some(ctx) = parts.extensions.get::<OperationsContext>() {
            if db::operations_tokens::active(&state.conn(), ctx.token_id)? {
                return Ok(Self);
            }
            return Err(invalid());
        }
        super::admin::PanelAuth::from_request_parts(parts, state)
            .await
            .map(|_| Self)
    }
}
pub enum ManagementGrant {
    Root(RootGrant),
    Operations(i64),
}
impl ManagementGrant {
    pub fn valid(&self, c: &Connection) -> db::Result<bool> {
        match self {
            Self::Root(r) => r.valid(c),
            Self::Operations(id) => db::operations_tokens::active(c, *id),
        }
    }
    // Old business helpers take an account id; the atomic SQL audit projector replaces this sentinel
    // with the distinct machine actor, never with its owner account or a fabricated root identity.
    pub fn actor_account_id(&self) -> i64 {
        match self {
            Self::Root(r) => r.actor_account_id(),
            Self::Operations(_) => 0,
        }
    }
}
impl FromRequestParts<SharedState> for ManagementGrant {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, ApiError> {
        if let Some(ctx) = parts.extensions.get::<OperationsContext>() {
            if db::operations_tokens::active(&state.conn(), ctx.token_id)? {
                return Ok(Self::Operations(ctx.token_id));
            }
            return Err(invalid());
        }
        RootGrant::from_request_parts(parts, state)
            .await
            .map(Self::Root)
    }
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/ops/me", get(me))
        .route("/api/ops/capabilities", get(capabilities))
        .route("/api/ops/audit", get(audit_index))
}
async fn me(
    State(state): State<SharedState>,
    parts: axum::http::request::Parts,
) -> Result<Json<Value>, ApiError> {
    let id = parts
        .extensions
        .get::<OperationsContext>()
        .map(|v| v.token_id)
        .ok_or_else(invalid)?;
    let c = state.conn();
    let row = db::operations_tokens::by_id(&c, id)?.ok_or_else(invalid)?;
    Ok(Json(
        json!({"instance_id":db::operations_tokens::instance_id(&c)?,"token":row,"permission":"site_operations","contract_version":1}),
    ))
}
async fn capabilities(parts: axum::http::request::Parts) -> Result<Json<Value>, ApiError> {
    if parts.extensions.get::<OperationsContext>().is_none() {
        return Err(invalid());
    }
    Ok(Json(
        json!({"contract_version":1,"permission":"site_operations","actions":ACTIONS.iter().map(|(m,p)|json!({"method":m,"path":p})).collect::<Vec<_>>(),"documentation":"docs/operations-tokens.md"}),
    ))
}
#[derive(serde::Deserialize)]
struct AuditQuery {
    before_id: Option<i64>,
    limit: Option<i64>,
}
async fn audit_index(
    State(state): State<SharedState>,
    axum::extract::Query(q): axum::extract::Query<AuditQuery>,
    parts: axum::http::request::Parts,
) -> Result<Json<Value>, ApiError> {
    if parts.extensions.get::<OperationsContext>().is_none() {
        return Err(invalid());
    }
    let c = state.conn();
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let mut stmt=c.prepare("SELECT id,ts_ms,action,target_type,target_id,metadata_json FROM audit_events WHERE (?1 IS NULL OR id<?1) ORDER BY id DESC LIMIT ?2").map_err(db::DbError::from)?;
    let rows = stmt
        .query_map(params![q.before_id, limit + 1], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .map_err(db::DbError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db::DbError::from)?;
    let more = rows.len() > limit as usize;
    let events:Vec<Value>=rows.into_iter().take(limit as usize).map(|(id,ts,action,target,target_id,raw)|{let meta:Value=serde_json::from_str(&raw).unwrap_or(Value::Null);json!({"id":id,"ts_ms":ts,"action":action,"target_type":target,"target_id":target_id,"actor_type":meta.get("actor_type"),"actor_id":meta.get("actor_id"),"request_id":meta.get("request_id"),"success":meta.get("success")})}).collect();
    let next = if more {
        events.last().and_then(|v| v["id"].as_i64())
    } else {
        None
    };
    Ok(Json(json!({"events":events,"next_before_id":next})))
}

fn pick(v: &mut Value, keys: &[&str]) {
    if let Some(map) = v.as_object_mut() {
        map.retain(|k, _| keys.contains(&k.as_str()));
    }
}
fn request_projection(v: &mut Value) {
    pick(
        v,
        &[
            "id",
            "ts_ms",
            "endpoint",
            "model",
            "status",
            "http_status",
            "channel_id",
            "latency_ms",
            "input_tokens",
            "output_tokens",
            "input_tokens_known",
            "output_tokens_known",
            "usage_source",
            "streamed",
            "served_model",
            "first_byte_ms",
            "ordinal",
            "outcome",
            "duration_ms",
            "upstream_model",
        ],
    );
}
fn project(path: &str, method: &str, v: &mut Value) {
    if method == "POST"
        && (path.ends_with("/probe")
            || path.ends_with("/vision-test")
            || path.ends_with("/models")
            || path.ends_with("/models/discover"))
    {
        pick(
            v,
            &[
                "ok",
                "status",
                "http_status",
                "channel_id",
                "channel_name",
                "model",
                "model_sent",
                "latency_ms",
                "applied",
                "max_tokens",
                "retried_with_more_tokens",
                "models",
                "catalog",
                "count",
                "seen",
                "info",
                "fetched_at",
                "credential_id",
            ],
        );
    }
    if method == "POST" && path.starts_with("/api/custom-probes/") && path.ends_with("/run") {
        if let Some(result) = v.get_mut("result") {
            pick(result, &["ok", "status", "latency_ms", "at_ms", "billing"]);
        }
    }
    if path == "/api/logs" {
        if let Some(rows) = v["logs"].as_array_mut() {
            for row in rows {
                request_projection(row);
            }
        }
        pick(v, &["logs"]);
    }
    if path.starts_with("/api/requests/") {
        if let Some(obj) = v.as_object_mut() {
            for (key, value) in obj.iter_mut() {
                if key == "request" {
                    request_projection(value);
                } else if key == "attempts" {
                    if let Some(rows) = value.as_array_mut() {
                        for row in rows {
                            request_projection(row);
                        }
                    }
                }
            }
        }
        pick(v, &["request", "attempts"]);
    }
    if path == "/api/logs/live" {
        let n = v["lines"].as_array().map_or(0, Vec::len);
        *v = json!({"line_count":n,"level":v["level"],"revert_at_ms":v["revert_at_ms"],"message":"Raw runtime log lines are restricted to the root console"});
    }
    if path == "/api/settings" {
        pick(v, &["routing_config", "reprobe_interval_minutes"]);
    }
    if path == "/api/managed-users" {
        v.as_object_mut().map(|m| m.remove("root_accounts"));
    }
    if path == "/api/usage" {
        if let Some(rows) = v["entries"].as_array_mut() {
            for row in rows {
                pick(
                    row,
                    &[
                        "id",
                        "request_id",
                        "user_id",
                        "key_id",
                        "channel_id",
                        "model",
                        "status",
                        "outcome",
                        "created_ms",
                        "settled_ms",
                        "input_tokens",
                        "output_tokens",
                        "debit_micro",
                        "known_debit_micro",
                        "usage_certainty",
                        "estimated",
                        "billing_unit",
                    ],
                );
            }
        }
    }
    scrub(
        v,
        method == "POST" && (path == "/api/gateway-keys" || path.ends_with("/keys")),
    );
}
fn scrub(v: &mut Value, allow_new_secret: bool) {
    match v {
        Value::Array(rows) => {
            for row in rows {
                scrub(row, allow_new_secret)
            }
        }
        Value::Object(map) => {
            map.retain(|k, _| {
                !matches!(
                    k.to_ascii_lowercase().as_str(),
                    "password"
                        | "password_hash"
                        | "token_hash"
                        | "api_key"
                        | "gateway_key"
                        | "access_token"
                        | "refresh_token"
                        | "authorization"
                        | "cookie"
                        | "auth_snapshot"
                        | "source_snapshot"
                        | "price_snapshot"
                        | "request_body"
                        | "response_body"
                        | "raw"
                        | "raw_response"
                        | "error"
                        | "preview"
                        | "reply"
                        | "note"
                        | "input"
                        | "output"
                        | "ori_question"
                        | "reason"
                        | "account_status"
                        | "last_error"
                        | "last_note"
                        | "client_ip"
                        | "user_agent"
                ) && (k != "secret" || allow_new_secret)
            });
            for (k, val) in map.iter_mut() {
                if k == "base_url" || k == "url" {
                    if let Some(raw) = val.as_str() {
                        *val = json!(safe_url(raw));
                    }
                }
                if k == "base_urls" {
                    if let Some(items) = val.as_array_mut() {
                        for item in items {
                            if let Some(raw) = item.as_str() {
                                *item = json!(safe_url(raw));
                            } else {
                                *item = Value::Null;
                            }
                        }
                    }
                }
                if k == "endpoint" {
                    if let Some(raw) = val.as_str() {
                        if raw.contains("://") {
                            *val = json!(safe_url(raw));
                        }
                    }
                }
                scrub(val, allow_new_secret);
            }
        }
        _ => {}
    }
}

fn safe_url(raw: &str) -> String {
    if let Ok(mut url) = url::Url::parse(raw) {
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return String::new();
        }
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        url.to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sensitive_diagnostics_and_legacy_urls_are_projected() {
        let mut v = json!({"ok":false,"http_status":400,"note":"UPSTREAM_SECRET","reply":"MODEL_SECRET","error":"ERROR_SECRET"});
        project("/api/channels/1/probe", "POST", &mut v);
        assert_eq!(v, json!({"ok":false,"http_status":400}));
        let mut v = json!({"base_url":"https://u:password@localhost/v1?api_key=SECRET#secret","base_urls":["https://u:password@localhost/api?x=SECRET","invalid secret"],"url":"bad secret","endpoint":"chat"});
        project("/api/providers", "GET", &mut v);
        assert_eq!(v["base_url"], "https://localhost/v1");
        assert_eq!(v["base_urls"], json!(["https://localhost/api", ""]));
        assert_eq!(v["endpoint"], "chat");
        assert!(!v.to_string().contains("SECRET"));
        assert_eq!(v["url"], "");
    }
    #[test]
    fn exactly_registered_methods_are_allowed() {
        assert!(allowed("PUT", "/api/channels/7/tools"));
        assert!(!allowed("GET", "/api/channels/7/reveal"));
        assert!(!allowed("PUT", "/api/settings"));
        assert!(!allowed("POST", "/api/ops-tokens"));
        assert!(!allowed("GET", "/v1/models"));
    }
}

/// Model IDs, catalog metadata and old stored errors are untrusted strings too.
/// Reject exact credential echoes anywhere in string values, not just secret-named fields.
pub(crate) fn reject_credential_echo(value: &Value, secrets: &[String]) -> Result<(), ApiError> {
    fn contains(v: &Value, secrets: &[String]) -> bool {
        match v {
            Value::String(s) => secrets
                .iter()
                .filter(|s| !s.is_empty())
                .any(|secret| s.contains(secret)),
            Value::Array(a) => a.iter().any(|v| contains(v, secrets)),
            Value::Object(m) => m.iter().any(|(key, v)| {
                secrets
                    .iter()
                    .filter(|s| !s.is_empty())
                    .any(|secret| key.contains(secret))
                    || contains(v, secrets)
            }),
            _ => false,
        }
    }
    if contains(value, secrets) {
        Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "unsafe_operations_response",
            "Upstream returned unsafe credential-derived data",
        ))
    } else {
        Ok(())
    }
}
fn reject_stored_credential_echo(state: &SharedState, value: &Value) -> Result<(), ApiError> {
    let c = state.conn();
    let ids = c
        .prepare("SELECT id FROM channels")
        .map_err(db::DbError::from)?
        .query_map([], |r| r.get::<_, i64>(0))
        .map_err(db::DbError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db::DbError::from)?;
    let mut secrets = Vec::new();
    for id in ids {
        if let Some(secret) =
            db::channels::secret(&c, &state.secrets, tokengo_shared::ChannelId(id))?
        {
            if !secret.is_empty() {
                secrets.push(secret);
            }
        }
        for (_, value) in db::channels::headers(&c, &state.secrets, tokengo_shared::ChannelId(id))?
        {
            if !value.is_empty() {
                secrets.push(value);
            }
        }
    }
    reject_credential_echo(value, &secrets)
}
