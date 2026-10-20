//! Root-panel administration and owner-scoped virtual-key self-service.
use super::{admin::PanelAuth, ApiError};
use crate::{
    db::{
        self,
        identity::{self, AuthContext, Policy},
    },
    SharedState,
};
use axum::{
    extract::{FromRequestParts, Path, State},
    http::{request::Parts, StatusCode},
    routing::{get, patch},
    Json, Router,
};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/users", get(users).post(create_user))
        .route(
            "/api/users/{id}",
            get(read_user)
                .patch(update_user)
                .put(update_user)
                .delete(disable_user),
        )
        .route(
            "/api/users/{id}/keys",
            get(admin_keys).post(admin_create_key),
        )
        .route("/api/vkeys/{id}", patch(update_key))
        .route("/v1/me", get(me))
        .route("/v1/keys", get(own_keys).post(own_create_key))
        .route("/v1/keys/{id}", axum::routing::delete(own_revoke))
}
pub struct VKeyAuth(pub AuthContext);
impl FromRequestParts<SharedState> for VKeyAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, ApiError> {
        let key = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.strip_prefix("Bearer ")
                    .or_else(|| v.strip_prefix("bearer "))
            })
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| ApiError::unauthorized("Virtual key required"))?;
        state
            .authenticate_context(key.to_string())
            .await
            .map(VKeyAuth)
            .ok_or_else(|| ApiError::unauthorized("Virtual key is inactive or invalid"))
    }
}
/// Scoped Route B administrators. Root panel sessions remain a separate authority.
/// An admin role alone (including migrated public gateway credentials) grants nothing.
pub struct AdminAuth(pub Option<AuthContext>);
impl FromRequestParts<SharedState> for AdminAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, ApiError> {
        match PanelAuth::from_request_parts(parts, state).await {
            Ok(_) => return Ok(AdminAuth(None)),
            Err(e) if parts.headers.contains_key(axum::http::header::COOKIE) => return Err(e),
            Err(_) => {}
        }
        let VKeyAuth(auth) = VKeyAuth::from_request_parts(parts, state).await?;
        if auth.role != "admin" || !auth.effective_policy().admin_access {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "admin_scope_required",
                "Admin role and explicit admin_access on user and key are required",
            ));
        }
        Ok(AdminAuth(Some(auth)))
    }
}
impl AdminAuth {
    pub fn actor_label(&self) -> String {
        self.0
            .as_ref()
            .map(|auth| format!("user:{}/key:{}", auth.user_id, auth.key_id))
            .unwrap_or_else(|| "panel_admin".into())
    }
}

fn bad(e: db::DbError) -> ApiError {
    ApiError::bad_request(e.to_string())
}
fn missing_user(c: &rusqlite::Connection, id: i64) -> Result<identity::User, ApiError> {
    identity::user(c, id)?.ok_or_else(|| ApiError::not_found("User not found"))
}
fn missing_key(c: &rusqlite::Connection, id: i64) -> Result<identity::VKey, ApiError> {
    identity::key(c, id)?.ok_or_else(|| ApiError::not_found("Key not found"))
}
async fn users(
    _admin: AdminAuth,
    State(state): State<SharedState>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({"users":identity::users(&state.conn())?})))
}
async fn read_user(
    _admin: AdminAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({"user":missing_user(&state.conn(),id)?})))
}
fn user_role() -> String {
    "user".into()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewUser {
    name: String,
    #[serde(default = "user_role")]
    role: String,
    #[serde(default)]
    policy: Policy,
}
async fn create_user(
    _admin: AdminAuth,
    State(state): State<SharedState>,
    Json(body): Json<NewUser>,
) -> Result<Json<Value>, ApiError> {
    identity::validate_name(&body.name).map_err(bad)?;
    identity::validate_role(&body.role).map_err(bad)?;
    body.policy.validate().map_err(bad)?;
    let c = state.conn();
    let row = db::in_tx(&c, |c| {
        let row = identity::create_user(c, &body.name, &body.role, &body.policy)?;
        identity::audit(
            c,
            _admin.0.as_ref(),
            "user.create",
            "user",
            row.id,
            json!({"role":row.role}),
        )?;
        Ok(row)
    })?;
    Ok(Json(json!({"user":row})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserPatch {
    name: Option<String>,
    role: Option<String>,
    enabled: Option<bool>,
    policy: Option<Policy>,
}
async fn update_user(
    _admin: AdminAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<UserPatch>,
) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    let mut row = missing_user(&c, id)?;
    if let Some(v) = body.name {
        identity::validate_name(&v).map_err(bad)?;
        row.name = v.trim().into();
    }
    if let Some(v) = body.role {
        identity::validate_role(&v).map_err(bad)?;
        row.role = v;
    }
    if let Some(v) = body.policy {
        v.validate().map_err(bad)?;
        row.policy = v;
    }
    if let Some(v) = body.enabled {
        row.enabled = v;
    }
    db::in_tx(&c, |c| {
        identity::update_user(c, &row)?;
        identity::audit(
            c,
            _admin.0.as_ref(),
            "user.update",
            "user",
            id,
            json!({"enabled":row.enabled,"role":row.role}),
        )
    })?;
    Ok(Json(json!({"user":row})))
}
async fn disable_user(
    _admin: AdminAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    let mut row = missing_user(&c, id)?;
    row.enabled = false;
    db::in_tx(&c, |c| {
        identity::update_user(c, &row)?;
        identity::audit(c, _admin.0.as_ref(), "user.disable", "user", id, json!({}))
    })?;
    Ok(Json(json!({"user":row})))
}
async fn admin_keys(
    _admin: AdminAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    missing_user(&c, id)?;
    Ok(Json(json!({"keys":identity::keys(&c,id)?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewKey {
    name: String,
    #[serde(default)]
    policy: Option<Policy>,
    #[serde(default)]
    expires_at: Option<i64>,
}
async fn admin_create_key(
    _admin: AdminAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<NewKey>,
) -> Result<Json<Value>, ApiError> {
    identity::validate_name(&body.name).map_err(bad)?;
    let policy = body.policy.unwrap_or_default();
    policy.validate().map_err(bad)?;
    if body.expires_at.is_some_and(|t| t <= db::now_ms()) {
        return Err(ApiError::bad_request("Expiry must be in the future"));
    }
    let c = state.conn();
    let owner = missing_user(&c, id)?;
    if !owner.enabled {
        return Err(ApiError::bad_request("User is disabled"));
    }
    let (row, secret) = db::in_tx(&c, |c| {
        let (row, secret) =
            identity::create_key(c, id, &body.name, &policy, body.expires_at, None)?;
        identity::audit(
            c,
            _admin.0.as_ref(),
            "key.create",
            "key",
            row.id,
            json!({"user_id":id}),
        )?;
        Ok((row, secret))
    })?;
    Ok(Json(json!({"key":row,"secret":secret})))
}
fn present_nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<i64>>, D::Error> {
    Option::<i64>::deserialize(d).map(Some)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyPatch {
    name: Option<String>,
    policy: Option<Policy>,
    enabled: Option<bool>,
    #[serde(default)]
    revoke: bool,
    #[serde(default, deserialize_with = "present_nullable")]
    expires_at: Option<Option<i64>>,
}
async fn update_key(
    _admin: AdminAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<KeyPatch>,
) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    let mut row = missing_key(&c, id)?;
    if let Some(v) = body.name {
        identity::validate_name(&v).map_err(bad)?;
        row.name = v.trim().into();
    }
    if let Some(v) = body.policy {
        v.validate().map_err(bad)?;
        row.policy = v;
    }
    if let Some(v) = body.expires_at {
        if v.is_some_and(|t| t < 0) {
            return Err(ApiError::bad_request(
                "Expiry must be a nonnegative timestamp",
            ));
        }
        row.expires_at = v;
    }
    if let Some(v) = body.enabled {
        if v && row.revoked_at.is_some() {
            return Err(ApiError::bad_request("Revoked keys cannot be re-enabled"));
        }
        row.enabled = v;
    }
    if body.revoke {
        row.enabled = false;
        row.revoked_at.get_or_insert_with(db::now_ms);
    }
    db::in_tx(&c, |c| {
        identity::update_key(c, &row)?;
        identity::audit(
            c,
            _admin.0.as_ref(),
            if body.revoke {
                "key.revoke"
            } else {
                "key.update"
            },
            "key",
            id,
            json!({"user_id":row.user_id,"enabled":row.enabled}),
        )
    })?;
    Ok(Json(json!({"key":row})))
}
async fn me(VKeyAuth(auth): VKeyAuth) -> Json<Value> {
    Json(json!({"identity":auth}))
}
async fn own_keys(
    VKeyAuth(auth): VKeyAuth,
    State(state): State<SharedState>,
) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    let auth = identity::context(&c, auth.key_id)?
        .ok_or_else(|| ApiError::unauthorized("Key is inactive"))?;
    let mut visible = Vec::new();
    for row in identity::keys(&c, auth.user_id)? {
        if identity::key_in_subtree(&c, row.id, auth.key_id, auth.user_id)? {
            visible.push(row);
        }
    }
    Ok(Json(json!({"keys":visible})))
}
async fn own_create_key(
    VKeyAuth(auth): VKeyAuth,
    State(state): State<SharedState>,
    Json(body): Json<NewKey>,
) -> Result<Json<Value>, ApiError> {
    identity::validate_name(&body.name).map_err(bad)?;
    let c = state.conn();
    let auth = identity::context(&c, auth.key_id)?
        .ok_or_else(|| ApiError::unauthorized("Key is inactive"))?;
    let effective = auth.effective_policy();
    let policy = body.policy.unwrap_or_else(|| effective.clone());
    policy.validate().map_err(bad)?;
    if !policy.no_broader_than(&effective) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "policy_broadening",
            "Child policy cannot broaden its parent",
        ));
    }
    let mut expiry = body.expires_at;
    let parent_ids = std::iter::once(auth.key_id).chain(auth.ancestor_key_ids.iter().copied());
    for id in parent_ids {
        if let Some(limit) = missing_key(&c, id)?.expires_at {
            if expiry.is_some_and(|t| t > limit) {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "expiry_broadening",
                    "Child expiry cannot exceed parent expiry",
                ));
            }
            expiry = Some(expiry.map_or(limit, |t| t.min(limit)));
        }
    }
    if expiry.is_some_and(|t| t <= db::now_ms()) {
        return Err(ApiError::bad_request("Expiry must be in the future"));
    }
    let (row, secret) = db::in_tx(&c, |c| {
        let (row, secret) = identity::create_key(
            c,
            auth.user_id,
            &body.name,
            &policy,
            expiry,
            Some(auth.key_id),
        )?;
        identity::audit(
            c,
            Some(&auth),
            "key.create",
            "key",
            row.id,
            json!({"parent_key_id":auth.key_id}),
        )?;
        Ok((row, secret))
    })?;
    Ok(Json(json!({"key":row,"secret":secret})))
}
async fn own_revoke(
    VKeyAuth(auth): VKeyAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    let auth = identity::context(&c, auth.key_id)?
        .ok_or_else(|| ApiError::unauthorized("Key is inactive"))?;
    let row = missing_key(&c, id)?;
    if row.user_id != auth.user_id {
        return Err(ApiError::not_found("Key not found"));
    }
    if !identity::key_in_subtree(&c, id, auth.key_id, auth.user_id)? {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "key_scope_required",
            "Only this key and its descendants may be revoked",
        ));
    }
    // Parent lifetime/quota authority cannot be removed by deleting a child: records
    // are retained permanently and descendants authenticate against the parent chain.
    db::in_tx(&c, |c| {
        identity::revoke(c, id)?;
        identity::audit(c, Some(&auth), "key.revoke", "key", id, json!({}))
    })?;
    Ok(Json(json!({"revoked":true,"id":id})))
}
