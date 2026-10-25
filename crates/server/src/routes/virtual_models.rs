//! Cookie-owned private route editor; no arbitrary upstream addresses or credentials.
use super::{accounts::Browser, ApiError};
use crate::{
    db::{self, user_virtual_routes as vr},
    SharedState,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/account/virtual-models", get(list).post(create))
        .route(
            "/api/account/virtual-models/{id}",
            patch(update).delete(disable),
        )
        .route("/api/account/virtual-models/{id}/verify",post(verify))
        .route("/api/account/virtual-models/{id}/targets/{position}/verify",post(verify_target))
        .route("/api/account/virtual-models/{id}/trash",post(trash))
        .route("/api/account/virtual-models/{id}/restore",post(restore))
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
fn owner(b: &Browser, c: &rusqlite::Connection) -> Result<i64, ApiError> {
    b.owner(c)?.ok_or_else(|| {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "user_required",
            "请在普通用户空间配置私有模型",
        )
    })
}
#[derive(Default,Deserialize)]
struct ListQuery {#[serde(default)] trash:bool}
async fn list(b: Browser, State(s): State<SharedState>, Query(query):Query<ListQuery>) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    let user = owner(&b, &c)?;
    let auth = vr::owner_auth(&c, user).map_err(api)?;
    let mut options = Vec::new();
    for route in db::model_routes::servable(&c)? {
        if !db::model_routes::visible(&c, &route.name)? {
            continue;
        }
        let mut providers = std::collections::BTreeSet::new();
        for source in &route.declared_by {
            if let Some((provider_id, provider_name, _)) =
                db::provider_channels::membership(&c, source.0)?
            {
                let target = vr::Target {
                    provider_id,
                    logical_model: route.name.clone(),
                    switch_on_429: true,
                    switch_on_400: false,
                };
                if providers.insert(provider_id)
                    && !vr::allowed_sources(&c, &auth, &target)
                        .map_err(api)?
                        .is_empty()
                {
                    options.push(json!({"provider_id":provider_id,"provider_name":provider_name,"logical_model":route.name,"kind":route.kind}));
                }
            }
        }
    }
    Ok(Json(
        json!({"virtual_models":vr::list_all(&c,user)?.into_iter().filter(|r|r.deleted_at_ms.is_some()==query.trash).map(|r|{let status=crate::vector_renewal::saved_status(&s,&c,user,r.id,r.version,&r.config);let mut v=serde_json::to_value(r).unwrap_or_default();v["compatibility"]=status;v}).collect::<Vec<_>>(),"options":options}),
    ))
}
async fn create(
    b: Browser,
    State(s): State<SharedState>,
    Json(body): Json<vr::Config>,
) -> Result<Json<Value>, ApiError> {
    let user = owner(&b, &s.conn())?;
    let c = s.conn();
    if !b.valid(&c)? || owner(&b, &c)? != user {
        return Err(ApiError::unauthorized("登录状态已变化，请重新登录后保存"));
    }
    Ok(Json(
        json!({"virtual_model":vr::create(&c,user,&body).map_err(api)?}),
    ))
}
#[derive(Deserialize)]
struct Edit {
    version: i64,
    #[serde(flatten)]
    config: vr::Config,
}
async fn update(
    b: Browser,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<Edit>,
) -> Result<Json<Value>, ApiError> {
    let user = owner(&b, &s.conn())?;
    // Reject stale edits before replacing the saved configuration.
    let old = vr::list(&s.conn(), user)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "virtual_route_not_found",
                "虚拟模型不存在",
            )
        })?;
    if old.version != body.version {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "virtual_route_changed",
            "路由已修改，请刷新后重试",
        ));
    }
    let c = s.conn();
    if !b.valid(&c)? || owner(&b, &c)? != user {
        return Err(ApiError::unauthorized("登录状态已变化，请重新登录后保存"));
    }
    Ok(Json(
        json!({"virtual_model":vr::replace(&c,user,id,body.version,&body.config).map_err(api)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    version: i64,
}
fn verification_config(
    s: &SharedState,
    browser: &Browser,
    id: i64,
    version: i64,
) -> Result<(i64, vr::Config), ApiError> {
    let c = s.conn();
    if !browser.valid(&c)? {
        return Err(ApiError::unauthorized("登录状态已变化，请重新登录"));
    }
    let user = owner(browser, &c)?;
    let row = vr::get(&c, user, id)?.ok_or_else(|| ApiError::not_found("虚拟模型不存在"))?;
    if row.version != version {
        return Err(ApiError::new(StatusCode::CONFLICT, "virtual_route_changed", "路由已修改，请刷新后验证"));
    }
    Ok((user, row.config))
}
async fn verify(
    browser: Browser,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<Version>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (user, config) = verification_config(&s, &browser, id, body.version)?;
    let status = crate::vector_renewal::start_saved(
        &s, user, id, body.version, config, None,
    ).map_err(api)?;
    Ok((StatusCode::ACCEPTED, Json(json!({"compatibility":status}))))
}
async fn verify_target(
    browser: Browser,
    State(s): State<SharedState>,
    Path((id, position)): Path<(i64, usize)>,
    Json(body): Json<Version>,
) -> Result<Json<Value>, ApiError> {
    let (user, config) = verification_config(&s, &browser, id, body.version)?;
    let guard = crate::vector_renewal::RouteGuard {
        user, id, version: body.version, allow_disabled: true,
    };
    let result = crate::vector_compat::verify_target(
        &s, user, &config, position, None, &guard,
    ).await;
    verification_config(&s, &browser, id, body.version)?;
    Ok(Json(result.map_err(api)?))
}
async fn disable(
    b: Browser,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<Version>,
) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    let user = owner(&b, &c)?;
    Ok(Json(
        json!({"virtual_model":vr::disable(&c,user,id,body.version).map_err(api)?}),
    ))
}

async fn trash(b:Browser,State(s):State<SharedState>,Path(id):Path<i64>,Json(body):Json<Version>)->Result<Json<Value>,ApiError>{
    let c=s.conn();let user=owner(&b,&c)?;Ok(Json(json!({"virtual_model":vr::trash(&c,user,id,body.version).map_err(api)?})))
}
async fn restore(b:Browser,State(s):State<SharedState>,Path(id):Path<i64>,Json(body):Json<Version>)->Result<Json<Value>,ApiError>{
    let c=s.conn();let user=owner(&b,&c)?;Ok(Json(json!({"virtual_model":vr::restore(&c,user,id,body.version).map_err(api)?})))
}
