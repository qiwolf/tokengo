//! Cookie-owned private route editor; no arbitrary upstream addresses or credentials.
use super::{accounts::Browser, ApiError};
use crate::{
    db::{self, user_virtual_routes as vr},
    SharedState,
};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{get, patch},
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
        .layer(DefaultBodyLimit::max(65536))
        .layer(axum::middleware::map_response(super::accounts::json_errors))
}
fn api(e: vr::VirtualError) -> ApiError {
    let status = match e.code {
        "virtual_route_changed" | "virtual_route_conflict" => StatusCode::CONFLICT,
        "virtual_route_not_found" => StatusCode::NOT_FOUND,
        "virtual_route_forbidden" | "virtual_target_forbidden" => StatusCode::FORBIDDEN,
        "virtual_route_storage_error" => StatusCode::INTERNAL_SERVER_ERROR,
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
async fn list(b: Browser, State(s): State<SharedState>) -> Result<Json<Value>, ApiError> {
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
        json!({"virtual_models":vr::list(&c,user)?,"options":options}),
    ))
}
async fn create(
    b: Browser,
    State(s): State<SharedState>,
    Json(body): Json<vr::Config>,
) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    let user = owner(&b, &c)?;
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
    let c = s.conn();
    let user = owner(&b, &c)?;
    Ok(Json(
        json!({"virtual_model":vr::replace(&c,user,id,body.version,&body.config).map_err(api)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    version: i64,
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
