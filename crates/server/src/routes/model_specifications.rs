use super::{admin::PanelAuth, ApiError};
use crate::{
    db,
    model_specifications::{self, SpecificationInput},
    SharedState,
};
use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;

pub(super) fn router() -> Router<SharedState> {
    Router::new()
        .route("/model-specifications", get(list).put(save).delete(remove))
        .route("/model-specifications/resolve", post(resolve))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecificationQuery {
    model_id: Option<String>,
    provider_id: Option<i64>,
    base_url: Option<String>,
}

fn check_provider(conn: &rusqlite::Connection, provider_id: Option<i64>) -> Result<(), ApiError> {
    if let Some(id) = provider_id {
        if id <= 0 {
            return Err(ApiError::bad_request("供应商编号无效"));
        }
        if !db::provider_channels::exists(conn, id)? {
            return Err(ApiError::not_found("供应商不存在"));
        }
    }
    Ok(())
}

fn lookup_url(
    conn: &rusqlite::Connection,
    provider_id: Option<i64>,
    supplied: Option<String>,
) -> db::Result<Option<String>> {
    if supplied.as_deref().is_some_and(|url| !url.trim().is_empty()) {
        return Ok(supplied);
    }
    let Some(id) = provider_id else { return Ok(None); };
    Ok(conn.query_row(
        "SELECT COALESCE(NULLIF(TRIM(base_url), ''), (
             SELECT CASE WHEN COUNT(DISTINCT TRIM(c.base_url)) = 1 THEN MIN(TRIM(c.base_url)) END
             FROM provider_connections p JOIN channels c ON c.id = p.channel_id
             WHERE p.provider_id = ?1 AND TRIM(c.base_url) <> ''
         ))
         FROM provider_channels WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?)
}

async fn list(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Query(query): Query<SpecificationQuery>,
) -> Result<Json<Value>, ApiError> {
    let conn = state.conn();
    check_provider(&conn, query.provider_id)?;
    let rows = if let Some(id) = query.model_id {
        model_specifications::validate_model_id(&id).map_err(ApiError::bad_request)?;
        let base_url = lookup_url(&conn, query.provider_id, query.base_url)?;
        model_specifications::resolve(&conn, &id, query.provider_id, base_url.as_deref())?
            .into_iter()
            .collect()
    } else {
        model_specifications::list(&conn, query.provider_id)?
    };
    Ok(Json(json!({"specifications": rows})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveInput {
    provider_id: Option<i64>,
    base_url: Option<String>,
    model_ids: Vec<String>,
}

async fn resolve(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(input): Json<ResolveInput>,
) -> Result<Json<Value>, ApiError> {
    if input.model_ids.len() > 250 {
        return Err(ApiError::bad_request("每次最多匹配 250 个模型"));
    }
    let conn = state.conn();
    check_provider(&conn, input.provider_id)?;
    let base_url = lookup_url(&conn, input.provider_id, input.base_url)?;
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for id in &input.model_ids {
        model_specifications::validate_model_id(id).map_err(ApiError::bad_request)?;
        if seen.insert(id) {
            if let Some(row) = model_specifications::resolve(&conn, id, input.provider_id, base_url.as_deref())? {
                rows.push(row);
            }
        }
    }
    Ok(Json(json!({"specifications": rows})))
}

async fn save(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Json(input): Json<SpecificationInput>,
) -> Result<Json<Value>, ApiError> {
    let spec = input.validate().map_err(ApiError::bad_request)?;
    let conn = state.conn();
    check_provider(&conn, spec.provider_id)?;
    db::in_tx(&conn, |conn| {
        model_specifications::save(conn, &spec)?;
        db::identity::audit(
            conn,
            None,
            "model_specification.save",
            "model_specification",
            spec.provider_id.unwrap_or(0),
            json!({"model_id":spec.model_id,"provider_id":spec.provider_id}),
        )
    })?;
    Ok(Json(json!({"specification": spec})))
}

async fn remove(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Query(query): Query<SpecificationQuery>,
) -> Result<Json<Value>, ApiError> {
    let id = query.model_id.ok_or_else(|| ApiError::bad_request("请填写上游模型 ID"))?;
    model_specifications::validate_model_id(&id).map_err(ApiError::bad_request)?;
    let conn = state.conn();
    check_provider(&conn, query.provider_id)?;
    let removed = db::in_tx(&conn, |conn| {
        let removed = model_specifications::delete(conn, &id, query.provider_id)?;
        if removed {
            db::identity::audit(
                conn,
                None,
                "model_specification.delete",
                "model_specification",
                query.provider_id.unwrap_or(0),
                json!({"model_id":id,"provider_id":query.provider_id}),
            )?;
        }
        Ok(removed)
    })?;
    Ok(Json(json!({"removed": removed})))
}
