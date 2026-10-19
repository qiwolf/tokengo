//! Administrator-only logical routing policy API, independent of panel layout.
use super::{admin::PanelAuth, ApiError};
use crate::{
    db::{
        self, channels, model_routes,
        route_policy::{self, RoutePolicy},
    },
    SharedState,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokengo_shared::{normalize_model_name, same_model_name};

pub fn router() -> Router<SharedState> {
    Router::new().route("/api/model-routing", get(read).put(write).delete(remove))
}

#[derive(Deserialize)]
struct ModelQuery {
    model: String,
}

fn model_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty()
        || name.chars().count() > 256
        || name.chars().any(char::is_control)
        || normalize_model_name(name).is_empty()
        || model_routes::is_auto(name)
    {
        return Err(ApiError::bad_request(
            "A non-reserved logical model name is required",
        ));
    }
    Ok(name.to_string())
}

async fn read(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Query(query): Query<ModelQuery>,
) -> Result<Json<Value>, ApiError> {
    let name = model_name(&query.model)?;
    let conn = state.conn();
    Ok(Json(
        json!({"model": name, "policy": route_policy::load(&conn, &name)?}),
    ))
}

async fn write(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Query(query): Query<ModelQuery>,
    body: Result<Json<RoutePolicy>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(policy) =
        body.map_err(|_| ApiError::bad_request("Invalid model routing policy JSON"))?;
    let name = model_name(&query.model)?;
    if policy.retry_times.is_some_and(|v| v > 2) {
        return Err(ApiError::bad_request(
            "retry_times must be 0..2 (at most three attempts per target)",
        ));
    }
    if policy
        .reprobe_interval_ms
        .is_some_and(|v| !(0..=86_400_000).contains(&v))
    {
        return Err(ApiError::bad_request(
            "reprobe_interval_ms must be 0..86400000",
        ));
    }
    let conn = state.conn();
    let route = model_routes::routes_for(&conn, &name)?
        .ok_or_else(|| ApiError::not_found("Logical model is not configured"))?;
    if let Some(ids) = &policy.channel_ids {
        if ids.is_empty()
            || ids.len() > 256
            || ids
                .iter()
                .enumerate()
                .any(|(i, id)| *id <= 0 || ids[..i].contains(id))
        {
            return Err(ApiError::bad_request(
                "channel_ids must contain 1..256 distinct positive IDs",
            ));
        }
        let declared = channels::list(&conn)?;
        if ids.iter().any(|id| {
            !declared.iter().any(|c| {
                c.id.0 == *id
                    && c.models
                        .iter()
                        .any(|m| same_model_name(&m.model, &route.name))
            })
        }) {
            return Err(ApiError::bad_request(
                "Every target must already declare this logical model",
            ));
        }
    }
    let reset = tokengo_shared::routing::RouteState::default();
    db::in_tx(&conn, |c| {
        route_policy::save(c, &route.name, &policy)?;
        db::routing::save_state(c, &route.name, &reset)
    })?;
    // Never wait for the routing mutex while owning the database mutex.
    drop(conn);
    state.routing().restore(&route.name, reset);
    Ok(Json(json!({"model":route.name,"policy":policy})))
}

async fn remove(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Query(query): Query<ModelQuery>,
) -> Result<Json<Value>, ApiError> {
    let name = model_name(&query.model)?;
    let conn = state.conn();
    let canonical = model_routes::routes_for(&conn, &name)?
        .map(|r| r.name)
        .unwrap_or(name);
    let reset = tokengo_shared::routing::RouteState::default();
    db::in_tx(&conn, |c| {
        route_policy::delete(c, &canonical)?;
        db::routing::save_state(c, &canonical, &reset)
    })?;
    drop(conn);
    state.routing().restore(&canonical, reset);
    Ok(Json(
        json!({"model":canonical,"policy":RoutePolicy::default()}),
    ))
}
