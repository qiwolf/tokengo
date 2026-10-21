//! Root-panel-only management. No credential reveal and no operations-token grant.
use super::{ApiError, admin::PanelAuth};
use crate::{
    SharedState,
    db::{
        self,
        passthrough::{self, Route, RouteInput},
    },
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, put},
};
use serde::Deserialize;
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/passthrough-routes", get(list).post(create))
        .route("/api/passthrough-routes/{id}", put(update).delete(delete))
}
fn error(e: db::DbError) -> ApiError {
    match e {
        db::DbError::Other(s) if s == "route revision conflict" => {
            ApiError::new(StatusCode::CONFLICT, "revision_conflict", s)
        }
        db::DbError::Other(s) if s == "route not found" => ApiError::not_found(s),
        db::DbError::Other(s) => ApiError::bad_request(s),
        db::DbError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            ApiError::bad_request("Route prefix already exists or configuration conflicts")
        }
        _ => ApiError::internal("Passthrough storage operation failed"),
    }
}
async fn list(
    _auth: PanelAuth,
    State(s): State<SharedState>,
) -> Result<Json<Vec<Route>>, ApiError> {
    Ok(Json(passthrough::list(&s.conn()).map_err(error)?))
}
async fn create(
    _auth: PanelAuth,
    State(s): State<SharedState>,
    body: Result<Json<RouteInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Route>, ApiError> {
    let Json(v) = body.map_err(|_| ApiError::bad_request("Invalid passthrough route JSON"))?;
    Ok(Json(
        passthrough::create(&s.conn(), &s.secrets, &v).map_err(error)?,
    ))
}
async fn update(
    _auth: PanelAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    body: Result<Json<RouteInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Route>, ApiError> {
    let Json(v) = body.map_err(|_| ApiError::bad_request("Invalid passthrough route JSON"))?;
    let revision = v
        .revision
        .ok_or_else(|| ApiError::bad_request("revision is required"))?;
    Ok(Json(
        passthrough::update(&s.conn(), &s.secrets, id, revision, &v).map_err(error)?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteQuery {
    revision: i64,
}
async fn delete(
    _auth: PanelAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    Query(q): Query<DeleteQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    passthrough::delete(&s.conn(), id, q.revision).map_err(error)?;
    Ok(Json(serde_json::json!({"ok":true})))
}
