//! Root-only explicit provider/source grouping; no credential or connection identity changes.
use super::{admin::PanelAuth, ApiError};
use crate::{
    db::{self, channels, model_routes, provider_channels as providers},
    SharedState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokengo_shared::{same_model_name, ChannelId};
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/providers", get(list).post(create))
        .route("/api/providers/{id}", put(update).delete(delete))
        .route("/api/channels/{id}/provider", put(assign))
        .route("/api/providers/{id}/models/enabled", put(group_model))
        .route("/api/channels/{id}/models/enabled", put(connection_model))
        .route("/api/model-visibility", put(visibility))
        .route("/api/channels/{id}/models/reasoning", put(reasoning))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderBody {
    name: String,
    #[serde(default = "yes")]
    enabled: bool,
}
fn yes() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Assignment {
    provider_id: Option<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSwitch {
    model: String,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupSwitch {
    model: String,
    enabled: bool,
    connection_ids: Vec<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelQuery {
    model: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Visibility {
    visible: bool,
}
fn model(raw: &str) -> Result<String, ApiError> {
    let s = raw.trim();
    if s.is_empty()
        || s.chars().count() > 256
        || s.chars().any(char::is_control)
        || model_routes::is_auto(s)
    {
        return Err(ApiError::bad_request(
            "A declared non-reserved model is required",
        ));
    }
    Ok(s.into())
}
fn require(c: &rusqlite::Connection, id: i64) -> Result<(), ApiError> {
    if id <= 0 || !providers::exists(c, id)? {
        return Err(ApiError::not_found("Provider not found"));
    }
    Ok(())
}
fn connection(c: &rusqlite::Connection, id: i64) -> Result<channels::ChannelRow, ApiError> {
    channels::get(c, ChannelId(id))?.ok_or_else(|| ApiError::not_found("Connection not found"))
}
fn has_model(ch: &channels::ChannelRow, n: &str) -> bool {
    ch.models.iter().any(|m| same_model_name(&m.model, n))
}
/// Never acquire routing mutex until after the database guard is dropped.
fn changed(state: &SharedState, names: Vec<String>) {
    let names = {
        let c = state.conn();
        names
            .into_iter()
            .map(|name| {
                model_routes::routes_for(&c, &name)
                    .ok()
                    .flatten()
                    .map(|r| r.name)
                    .unwrap_or(name)
            })
            .collect::<Vec<_>>()
    };
    let mut routing = state.routing();
    for name in names {
        routing.restore(&name, Default::default());
    }
}
fn reset(c: &rusqlite::Connection, names: &[String]) -> db::Result<()> {
    for n in names {
        let canonical = model_routes::routes_for(c, n)?
            .map(|r| r.name)
            .unwrap_or_else(|| n.clone());
        db::routing::save_state(c, &canonical, &Default::default())?;
    }
    Ok(())
}
async fn list(_root: PanelAuth, State(s): State<SharedState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(providers::list(&s.conn())?))
}
async fn create(
    _root: PanelAuth,
    State(s): State<SharedState>,
    body: Result<Json<ProviderBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) = body.map_err(|_| ApiError::bad_request("Invalid provider JSON"))?;
    let n = providers::name(&b.name).map_err(ApiError::bad_request)?;
    let c = s.conn();
    let duplicate: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_channels WHERE name=?1 COLLATE NOCASE)",
            [&n],
            |r| r.get(0),
        )
        .map_err(db::DbError::from)?;
    if duplicate {
        return Err(ApiError::bad_request("Provider name already exists"));
    }
    let id = db::in_tx(&c, |c| {
        c.execute(
            "INSERT INTO provider_channels(name,enabled,created_ms) VALUES(?1,?2,?3)",
            rusqlite::params![n, b.enabled as i64, db::now_ms()],
        )?;
        Ok(c.last_insert_rowid())
    })?;
    Ok(Json(json!({"provider":providers::one(&c,id)?})))
}
async fn update(
    _root: PanelAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    body: Result<Json<ProviderBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) = body.map_err(|_| ApiError::bad_request("Invalid provider JSON"))?;
    let n = providers::name(&b.name).map_err(ApiError::bad_request)?;
    let c = s.conn();
    require(&c, id)?;
    let duplicate:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM provider_channels WHERE name=?1 COLLATE NOCASE AND id<>?2)",rusqlite::params![n,id],|r|r.get(0)).map_err(db::DbError::from)?;
    if duplicate {
        return Err(ApiError::bad_request("Provider name already exists"));
    }
    let names: Vec<_> = channels::list(&c)?
        .into_iter()
        .filter(|x| x.provider_id == Some(id))
        .flat_map(|x| x.models.into_iter().map(|m| m.model))
        .collect();
    db::in_tx(&c, |c| {
        c.execute(
            "UPDATE provider_channels SET name=?2,enabled=?3 WHERE id=?1",
            rusqlite::params![id, n, b.enabled as i64],
        )?;
        reset(c, &names)
    })?;
    let row = providers::one(&c, id)?;
    drop(c);
    changed(&s, names);
    Ok(Json(json!({"provider":row})))
}
async fn delete(
    _root: PanelAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    require(&c, id)?;
    // One guarded statement: a concurrent membership can never be cascaded away.
    let deleted = db::in_tx(&c, |c| {
        Ok(c.execute("DELETE FROM provider_channels WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM provider_connections WHERE provider_id=?1)", [id])?)
    })?;
    if deleted == 0 {
        require(&c, id)?;
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "provider_not_empty",
            "服务商仍有关联连接，请先移动/删除连接；不会级联删除连接或历史",
        ));
    }
    Ok(Json(json!({"deleted":true})))
}
async fn assign(
    _root: PanelAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    body: Result<Json<Assignment>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) = body.map_err(|_| ApiError::bad_request("Invalid assignment JSON"))?;
    let c = s.conn();
    let ch = connection(&c, id)?;
    if let Some(p) = b.provider_id {
        require(&c, p)?;
    }
    let names: Vec<_> = ch.models.into_iter().map(|m| m.model).collect();
    db::in_tx(&c, |c| {
        providers::set_membership(c, id, b.provider_id)?;
        reset(c, &names)
    })?;
    drop(c);
    changed(&s, names);
    Ok(Json(json!({"ok":true})))
}
async fn group_model(
    _root: PanelAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    body: Result<Json<GroupSwitch>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) = body.map_err(|_| ApiError::bad_request("Invalid model switch JSON"))?;
    let name = model(&b.model)?;
    if b.connection_ids.is_empty()
        || b.connection_ids.len() > 256
        || b.connection_ids
            .iter()
            .enumerate()
            .any(|(i, x)| *x <= 0 || b.connection_ids[..i].contains(x))
    {
        return Err(ApiError::bad_request(
            "Explicit distinct connection_ids scope is required",
        ));
    }
    let c = s.conn();
    require(&c, id)?;
    for target in &b.connection_ids {
        let ch = channels::get(&c, ChannelId(*target))?
            .ok_or_else(|| ApiError::bad_request("Invalid connection scope"))?;
        if ch.provider_id != Some(id) || !has_model(&ch, &name) {
            return Err(ApiError::bad_request(
                "Every scoped connection must belong to provider and declare this model",
            ));
        }
    }
    db::in_tx(&c, |c| {
        providers::model_switch(c, &b.connection_ids, &name, b.enabled)?;
        reset(c, std::slice::from_ref(&name))
    })?;
    drop(c);
    changed(&s, vec![name]);
    Ok(Json(
        json!({"ok":true,"updated_connection_ids":b.connection_ids}),
    ))
}
async fn connection_model(
    _root: PanelAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    body: Result<Json<ModelSwitch>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) = body.map_err(|_| ApiError::bad_request("Invalid model switch JSON"))?;
    let name = model(&b.model)?;
    let c = s.conn();
    let ch = connection(&c, id)?;
    if !has_model(&ch, &name) {
        return Err(ApiError::bad_request("Model not declared by connection"));
    }
    db::in_tx(&c, |c| {
        providers::model_switch(c, &[id], &name, b.enabled)?;
        reset(c, std::slice::from_ref(&name))
    })?;
    drop(c);
    changed(&s, vec![name]);
    Ok(Json(json!({"ok":true})))
}
async fn visibility(
    _root: PanelAuth,
    State(s): State<SharedState>,
    Query(q): Query<ModelQuery>,
    body: Result<Json<Visibility>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) = body.map_err(|_| ApiError::bad_request("Invalid visibility JSON"))?;
    let name = model(&q.model)?;
    let c = s.conn();
    if model_routes::routes_for(&c, &name)?.is_none() {
        return Err(ApiError::not_found("Model is not declared"));
    }
    db::in_tx(&c, |c| {
        model_routes::set_visible(c, &name, b.visible)?;
        reset(c, std::slice::from_ref(&name))
    })?;
    drop(c);
    changed(&s, vec![name.clone()]);
    Ok(Json(json!({"ok":true,"model":name,"visible":b.visible})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReasoningBody {
    model: String,
    manual_levels: Option<Vec<String>>,
    default_effort: Option<String>,
}
async fn reasoning(
    _root: PanelAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    body: Result<Json<ReasoningBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) =
        body.map_err(|_| ApiError::bad_request("Invalid reasoning configuration JSON"))?;
    let c = s.conn();
    connection(&c, id)?;
    let name = model(&b.model)?;
    let caps = db::in_tx(&c, |c| {
        crate::reasoning_capabilities::configure(
            c,
            id,
            &name,
            b.manual_levels.as_deref(),
            b.default_effort.as_deref(),
        )
    })
    .map_err(|_| ApiError::bad_request("Invalid model reasoning configuration"))?;
    Ok(Json(json!({"reasoning":caps})))
}
