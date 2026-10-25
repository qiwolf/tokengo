//! Root-panel administration only. Browser loopback callbacks are relayed by a local authenticated tool.
use crate::{
    oauth,
    routes::{admin::PanelAuth, ApiError},
    SharedState,
};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/oauth/providers", get(providers))
        .route("/api/oauth/device/start", post(device_start))
        .route("/api/oauth/device/{id}/poll", post(device_poll))
        .route("/api/oauth/device/{id}/cancel", post(device_cancel))
        .route("/api/oauth/start", post(start))
        .route("/api/oauth/complete", post(complete))
        .route("/api/oauth/cancel", post(cancel))
        .route("/api/oauth/accounts", get(accounts))
        .route(
            "/api/oauth/channels/{id}",
            get(binding).put(bind).delete(unbind),
        )
        .route("/api/oauth/accounts/{id}/revoke", post(revoke))
        .route("/api/oauth/accounts/{id}/models", get(models))
}
fn error(e: oauth::OAuthError) -> ApiError {
    let status = if matches!(
        e.0,
        "oauth_unavailable"
            | "oauth_storage_unavailable"
            | "oauth_refresh_too_early"
            | "oauth_reauthorization_required"
            | "oauth_refresh_in_progress"
    ) {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    } else {
        axum::http::StatusCode::BAD_REQUEST
    };
    ApiError::new(status, e.0, e.0)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    label: String,
    redirect_uri: String,
    account_id: Option<i64>,
    #[serde(default)]
    provider_id: Option<i64>,
}
async fn start(
    _root: PanelAuth,
    State(state): State<SharedState>,
    Json(body): Json<Start>,
) -> Result<Json<oauth::Started>, ApiError> {
    oauth::start_with_origin(&state, body.label, body.redirect_uri, body.account_id,body.provider_id)
        .map(Json)
        .map_err(error)
}
async fn complete(
    _root: PanelAuth,
    State(state): State<SharedState>,
    Json(body): Json<oauth::Completion>,
) -> Result<Json<Value>, ApiError> {
    oauth::complete(&state, body)
        .await
        .map(|a| Json(json!({"account":a})))
        .map_err(error)
}
async fn accounts(
    _root: PanelAuth,
    State(state): State<SharedState>,
) -> Result<Json<Value>, ApiError> {
    oauth::accounts(&state)
        .map(|a| Json(json!({"accounts":a})))
        .map_err(error)
}
async fn bind(
    _root: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<oauth::Binding>,
) -> Result<Json<Value>, ApiError> {
    oauth::bind(&state, id, &body).map_err(error)?;
    Ok(Json(
        json!({"bound":true,"channel_id":id,"account_id":body.account_id}),
    ))
}
async fn unbind(
    _root: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    oauth::unbind(&state, id).map_err(error)?;
    Ok(Json(json!({"bound":false,"channel_id":id})))
}
async fn revoke(
    _root: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<oauth::Revoked>, ApiError> {
    oauth::revoke(&state, id).await.map(Json).map_err(error)
}

async fn binding(
    _root: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let row = oauth::binding_metadata(&state, id).map_err(error)?;
    Ok(Json(
        json!({"channel_id":id,"binding":row.as_ref().map(|(b,_)|b),"binding_generation":row.as_ref().map(|(_,g)|g)}),
    ))
}

async fn models(
    _root: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    oauth::models(&state, id).await.map(Json).map_err(error)
}

async fn providers(_root: PanelAuth, State(state): State<SharedState>) -> Result<Json<Value>,ApiError> {
    crate::oauth_external::cleanup_pending(&state).map_err(error)?;
    Ok(Json(crate::oauth_external::providers()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceStart { provider: String, label: String, #[serde(default)] provider_id: Option<i64> }
async fn device_start(_root: PanelAuth, State(state): State<SharedState>, Json(body): Json<DeviceStart>) -> Result<Json<crate::oauth_external::DeviceStarted>, ApiError> {
    crate::oauth_external::start(&state,&body.provider,body.label,body.provider_id).await.map(Json).map_err(error)
}
async fn device_poll(_root: PanelAuth, State(state): State<SharedState>, Path(id): Path<String>) -> Result<Json<crate::oauth_external::DevicePoll>, ApiError> {
    crate::oauth_external::poll(&state,id).await.map(Json).map_err(error)
}
async fn device_cancel(_root: PanelAuth, State(state): State<SharedState>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    crate::oauth_external::cancel(&state,&id).map_err(error)?;Ok(Json(json!({"canceled":true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel { attempt_id:String }
async fn cancel(_root:PanelAuth,State(state):State<SharedState>,Json(body):Json<Cancel>)->Result<Json<Value>,ApiError> {
    oauth::cancel(&state,&body.attempt_id).map_err(error)?;Ok(Json(json!({"canceled":true})))
}
