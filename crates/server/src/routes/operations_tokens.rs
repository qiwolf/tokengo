//! Browser-root-only operations credential management. Machine credentials cannot delegate.
use super::{
    accounts::{self, Browser},
    ApiError,
};
use crate::{
    db::{self, operations_tokens as tokens},
    SharedState,
};
use axum::{
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{request::Parts, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/ops-tokens", get(list).post(create))
        .route("/api/ops-tokens/{id}", delete(revoke))
        .route("/api/ops-tokens/revoke-all", post(revoke_all))
        .layer(DefaultBodyLimit::max(8192))
        .layer(axum::middleware::map_response(accounts::json_errors))
}
struct BrowserRoot(Browser);
impl FromRequestParts<SharedState> for BrowserRoot {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, ApiError> {
        let browser = Browser::from_request_parts(parts, state).await?;
        if browser.credential.account.role != "root" {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "仅部署者可以管理运维令牌",
            ));
        }
        // Browser already enforces CSRF for unsafe methods. Read-only same-origin
        // list requests use the normal cookie transport without a write-CSRF header.
        Ok(Self(browser))
    }
}
fn current(browser: &Browser, c: &rusqlite::Connection) -> db::Result<()> {
    if !browser.valid(c)? {
        return Err(db::DbError::Other("root会话或凭据已变化".into()));
    }
    Ok(())
}
fn lifecycle_error(e: db::DbError) -> ApiError {
    if matches!(&e, db::DbError::Other(s) if s == "root会话或凭据已变化") {
        ApiError::unauthorized("root会话或凭据已变化")
    } else {
        e.into()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    before_id: Option<i64>,
    limit: Option<i64>,
}
async fn list(
    BrowserRoot(browser): BrowserRoot,
    State(state): State<SharedState>,
    Query(q): Query<Page>,
) -> Result<Json<Value>, ApiError> {
    let limit = q.limit.unwrap_or(50);
    if q.before_id.is_some_and(|id| id <= 0) || !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request("分页参数无效"));
    }
    let c = state.conn();
    current(&browser, &c).map_err(lifecycle_error)?;
    let mut rows = tokens::list(&c, q.before_id, limit + 1)?;
    let more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next = if more {
        rows.last().map(|r| r.id)
    } else {
        None
    };
    Ok(Json(json!({"tokens":rows,"next_before_id":next})))
}
fn default_days() -> i64 {
    90
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct New {
    name: String,
    #[serde(default = "default_days")]
    expires_in_days: i64,
    password: String,
}
async fn stepup(state: &SharedState, browser: &Browser, password: String) -> Result<(), ApiError> {
    state
        .panel_login_gate
        .check(&browser.credential.account.username)?;
    if !accounts::verify(state, password, browser.credential.hash.clone()).await? {
        return Err(ApiError::new(StatusCode::FORBIDDEN,"step_up_failed","当前密码错误，请重试"));
    }
    Ok(())
}
async fn create(
    BrowserRoot(browser): BrowserRoot,
    State(state): State<SharedState>,
    Json(body): Json<New>,
) -> Result<Json<Value>, ApiError> {
    tokens::validate(&body.name, body.expires_in_days)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    stepup(&state, &browser, body.password).await?;
    let c = state.conn();
    let created = db::in_tx(&c, |c| {
        current(&browser, c)?;
        tokens::create(
            c,
            &body.name,
            body.expires_in_days,
            browser.credential.account.id,
        )
    })
    .map_err(|e| match &e {
        db::DbError::Other(s) if s.contains("20把") => {
            ApiError::new(StatusCode::CONFLICT, "operations_token_limit", s)
        }
        _ => lifecycle_error(e),
    })?;
    state
        .panel_login_gate
        .success(&browser.credential.account.username);
    Ok(Json(json!({"token":created.token,"secret":created.secret})))
}
async fn revoke(
    BrowserRoot(browser): BrowserRoot,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    if id <= 0 {
        return Err(ApiError::bad_request("令牌id无效"));
    }
    let c = state.conn();
    let found = db::in_tx(&c, |c| {
        current(&browser, c)?;
        tokens::revoke(c, id, browser.credential.account.id)
    })
    .map_err(lifecycle_error)?;
    if !found {
        return Err(ApiError::not_found("令牌不存在"));
    }
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Password {
    password: String,
}
async fn revoke_all(
    BrowserRoot(browser): BrowserRoot,
    State(state): State<SharedState>,
    Json(body): Json<Password>,
) -> Result<Json<Value>, ApiError> {
    stepup(&state, &browser, body.password).await?;
    let c = state.conn();
    let n = db::in_tx(&c, |c| {
        current(&browser, c)?;
        tokens::revoke_all(c, browser.credential.account.id)
    })
    .map_err(lifecycle_error)?;
    state
        .panel_login_gate
        .success(&browser.credential.account.username);
    Ok(Json(json!({"ok":true,"revoked_count":n})))
}
