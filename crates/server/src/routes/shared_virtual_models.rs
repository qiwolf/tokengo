//! Authenticated root CRUD. Compatibility probes use a separate system identity.
use super::{accounts::RootGrant, ApiError};
use crate::{
    db::{self, shared_virtual_routes as shared, user_virtual_routes as vr},
    SharedState,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/shared-virtual-models", get(list).post(create))
        .route(
            "/api/shared-virtual-models/{id}",
            patch(update).delete(disable),
        )
        .route("/api/shared-virtual-models/{id}/trash",post(trash))
        .route("/api/shared-virtual-models/{id}/restore",post(restore))
        .route("/api/model-access", put(model_access))
        .route("/api/model-presentation", put(model_presentation))
        .route("/api/channels/{id}/presentation", put(channel_presentation))
        .route("/api/providers/{id}/presentation", put(provider_presentation))
        .layer(DefaultBodyLimit::max(65536))
        .layer(axum::middleware::map_response(super::accounts::json_errors))
}
fn api(e: vr::VirtualError) -> ApiError {
    let status = match e.code {
        "virtual_route_changed" | "virtual_route_conflict" => StatusCode::CONFLICT,
        "virtual_route_not_found" => StatusCode::NOT_FOUND,
        "virtual_route_forbidden" | "virtual_target_forbidden" => StatusCode::FORBIDDEN,
        "virtual_route_storage_error" => StatusCode::INTERNAL_SERVER_ERROR,
        "vector_compatibility_renewal_failed" | "vector_compatibility_renewing" => StatusCode::SERVICE_UNAVAILABLE,
        "vector_fallback_rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_REQUEST,
    };
    ApiError::new(status, e.code, e.message)
}
fn valid(root: &RootGrant, c: &rusqlite::Connection) -> Result<i64, ApiError> {
    if !root.valid(c)? {
        return Err(ApiError::unauthorized("管理员登录已失效"));
    }
    let actor = root.actor_account_id();
    shared::root_actor(c, actor).map_err(api)?;
    Ok(actor)
}
#[derive(Default,Deserialize)]
struct ListQuery {#[serde(default)] trash:bool}
async fn list(root: RootGrant, State(s): State<SharedState>,Query(query):Query<ListQuery>) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    valid(&root, &c)?;
    let auth = vr::owner_auth(&c, 0).map_err(api)?;
    let mut options = Vec::new();
    for route in db::model_routes::servable(&c)? {
        let mut seen = std::collections::HashSet::new();
        for source in &route.declared_by {
            if let Some((id, name, _)) = db::provider_channels::membership(&c, source.0)? {
                let target = vr::Target {
                    provider_id: id,
                    logical_model: route.name.clone(),
                    switch_on_429: true,
                    switch_on_400: false,
                };
                if seen.insert(id)
                    && !vr::allowed_sources_shared(&c, &auth, &target)
                        .map_err(api)?
                        .is_empty()
                {
                    options.push(json!({"provider_id":id,"provider_name":name,"logical_model":route.name,"kind":route.kind,"internal_only":db::model_access::internal_only(&c,&route.name)?}));
                }
            }
        }
    }
    Ok(Json(
        json!({"virtual_models":shared::list_all(&c)?.into_iter().filter(|r|r.deleted_at_ms.is_some()==query.trash).map(|r|{let status=crate::vector_renewal::status(&c,0,&r.config);let mut v=serde_json::to_value(r).unwrap_or_default();v["compatibility"]=status;v}).collect::<Vec<_>>(),"options":options}),
    ))
}
async fn create(
    root: RootGrant,
    State(s): State<SharedState>,
    Json(v): Json<vr::Config>,
) -> Result<Json<Value>, ApiError> {
    let actor = {
        let c = s.conn();
        let actor = valid(&root, &c)?;
        shared::preflight(&c, &v, None).map_err(api)?;
        actor
    };
    crate::vector_compat::verify_shared(&s, &v, actor)
        .await
        .map_err(api)?;
    let c = s.conn();
    valid(&root, &c)?;
    Ok(Json(
        json!({"virtual_model":shared::create(&c,&v,actor).map_err(api)?}),
    ))
}
#[derive(Deserialize)]
struct Edit {
    version: i64,
    #[serde(flatten)]
    config: vr::Config,
}
async fn update(
    root: RootGrant,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    Json(v): Json<Edit>,
) -> Result<Json<Value>, ApiError> {
    let actor = {
        let c = s.conn();
        let actor = valid(&root, &c)?;
        let old = shared::list(&c)?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| ApiError::not_found("共享模型不存在"))?;
        if old.version != v.version {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "virtual_route_changed",
                "共享模型已修改，请刷新",
            ));
        }
        shared::preflight(&c, &v.config, Some(id)).map_err(api)?;
        actor
    };
    crate::vector_compat::verify_shared(&s, &v.config, actor)
        .await
        .map_err(api)?;
    let c = s.conn();
    valid(&root, &c)?;
    Ok(Json(
        json!({"virtual_model":shared::replace(&c,id,v.version,&v.config,actor).map_err(api)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    version: i64,
}
async fn disable(
    root: RootGrant,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    Json(v): Json<Version>,
) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    let actor = valid(&root, &c)?;
    Ok(Json(
        json!({"virtual_model":shared::disable(&c,id,v.version,actor).map_err(api)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelQuery {
    model: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Access {
    internal_only: bool,
}
async fn model_access(
    root: RootGrant,
    State(s): State<SharedState>,
    Query(q): Query<ModelQuery>,
    Json(v): Json<Access>,
) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    let actor = valid(&root, &c)?;
    if db::model_routes::routes_for(&c, &q.model)?.is_none() {
        return Err(ApiError::not_found("标准模型不存在"));
    }
    let tx = c.unchecked_transaction().map_err(db::DbError::from)?;
    db::model_access::set_internal_only(&tx, &q.model, v.internal_only)?;
    tx.execute("INSERT INTO audit_events(ts_ms,action,target_type,metadata_json) VALUES(?1,'model_access.update','model',?2)",rusqlite::params![db::now_ms(),json!({"actor_account_id":actor,"model":q.model,"internal_only":v.internal_only}).to_string()]).map_err(db::DbError::from)?;
    tx.commit().map_err(db::DbError::from)?;
    Ok(Json(
        json!({"model":q.model,"internal_only":v.internal_only}),
    ))
}

async fn trash(root:RootGrant,State(s):State<SharedState>,Path(id):Path<i64>,Json(body):Json<Version>)->Result<Json<Value>,ApiError>{
    let c=s.conn();let actor=valid(&root,&c)?;Ok(Json(json!({"virtual_model":shared::trash(&c,id,body.version,actor).map_err(api)?})))
}
async fn restore(root:RootGrant,State(s):State<SharedState>,Path(id):Path<i64>,Json(body):Json<Version>)->Result<Json<Value>,ApiError>{
    let c=s.conn();let actor=valid(&root,&c)?;Ok(Json(json!({"virtual_model":shared::restore(&c,id,body.version,actor).map_err(api)?})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Presentation { display_name: String }
async fn model_presentation(root: RootGrant, State(s): State<SharedState>, Query(q): Query<ModelQuery>, Json(v): Json<Presentation>) -> Result<Json<Value>, ApiError> {
    let name=db::model_access::presentation_name(&v.display_name).map_err(ApiError::bad_request)?;
    let c=s.conn();
    let actor=valid(&root,&c)?;
    let route=db::model_routes::routes_for(&c,&q.model)?.ok_or_else(||ApiError::not_found("标准模型不存在"))?;
    let tx=c.unchecked_transaction().map_err(db::DbError::from)?;
    db::model_access::set_display_name(&tx,&route.name,&name)?;
    tx.execute("INSERT INTO audit_events(ts_ms,action,target_type,metadata_json) VALUES(?1,'model_presentation.update','model',?2)",rusqlite::params![db::now_ms(),json!({"actor_account_id":actor,"model":route.name,"display_name":name}).to_string()]).map_err(db::DbError::from)?;
    tx.commit().map_err(db::DbError::from)?;
    Ok(Json(json!({"model":route.name,"display_name":name})))
}
async fn channel_presentation(root: RootGrant, State(s): State<SharedState>, Path(id): Path<i64>, Json(v): Json<Presentation>) -> Result<Json<Value>, ApiError> {
    rename_presentation(&s,&root,id,&v.display_name,false)
}
async fn provider_presentation(root: RootGrant, State(s): State<SharedState>, Path(id): Path<i64>, Json(v): Json<Presentation>) -> Result<Json<Value>, ApiError> {
    rename_presentation(&s,&root,id,&v.display_name,true)
}
/// Name-only updates deliberately avoid credential versions, routing resets and timestamps.
fn rename_presentation(s:&SharedState,root:&RootGrant,id:i64,raw:&str,provider:bool)->Result<Json<Value>,ApiError> {
    let name=db::model_access::presentation_name(raw).map_err(ApiError::bad_request)?;
    if name.is_empty() { return Err(ApiError::bad_request("渠道和供应商显示名称不能为空")); }
    let c=s.conn();let actor=valid(root,&c)?;
    let tx=c.unchecked_transaction().map_err(db::DbError::from)?;
    if provider {
        if !db::provider_channels::exists(&tx,id)? { return Err(ApiError::not_found("供应商不存在")); }
        let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM provider_channels WHERE name=?1 COLLATE NOCASE AND id<>?2)",rusqlite::params![name,id],|r|r.get(0)).map_err(db::DbError::from)?;
        if duplicate { return Err(ApiError::bad_request("供应商显示名称已存在")); }
    }
    let sql=if provider {"UPDATE provider_channels SET name=?2 WHERE id=?1"}else{"UPDATE channels SET name=?2 WHERE id=?1 AND transport_kind<>'passthrough'"};
    if tx.execute(sql,rusqlite::params![id,name]).map_err(db::DbError::from)?==0 { return Err(ApiError::not_found("模型渠道不存在")); }
    tx.execute("INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json) VALUES(?1,'presentation.update',?2,?3,?4)",rusqlite::params![db::now_ms(),if provider {"provider"}else{"channel"},id,json!({"actor_account_id":actor,"display_name":name}).to_string()]).map_err(db::DbError::from)?;
    tx.commit().map_err(db::DbError::from)?;
    Ok(Json(json!({"id":id,"display_name":name})))
}
