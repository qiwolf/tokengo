//! Pricing and budget administration; key-authenticated usage is always key-scoped.
use super::{identity::AdminAuth, proxy::GatewayKey, ApiError};
use crate::{
    db::{
        self,
        billing::{self, Price, UsageQuery},
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
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/pricing", get(prices).post(create))
        .route("/api/budgets", get(budgets).put(update_budget))
        .route("/api/usage", get(admin_usage))
        .route("/v1/usage", get(self_usage))
}
fn mutation_error(e: db::DbError) -> ApiError {
    match e {
        db::DbError::Other(s) => ApiError::bad_request(s),
        e => e.into(),
    }
}
async fn prices(
    _auth: AdminAuth,
    State(state): State<SharedState>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({"prices":billing::list_prices(&state.conn())?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewPrice {
    channel_id: i64,
    model: String,
    input_micro_per_million: i64,
    output_micro_per_million: i64,
    max_input_tokens: i64,
    max_output_tokens: i64,
}
async fn create(
    _auth: AdminAuth,
    State(state): State<SharedState>,
    payload: Result<Json<NewPrice>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(p) = payload.map_err(|_| ApiError::bad_request("Invalid pricing body"))?;
    let p = Price {
        id: 0,
        channel_id: p.channel_id,
        model: p.model,
        input_micro_per_million: p.input_micro_per_million,
        output_micro_per_million: p.output_micro_per_million,
        max_input_tokens: p.max_input_tokens,
        max_output_tokens: p.max_output_tokens,
    };
    let result =
        billing::create_price(&state.conn(), &p, &_auth.actor_label()).map_err(mutation_error)?;
    Ok(Json(json!({"price":result})))
}
#[derive(Deserialize)]
struct BudgetQuery {
    scope: Option<String>,
    scope_id: Option<i64>,
}
async fn budgets(
    _auth: AdminAuth,
    State(state): State<SharedState>,
    Query(q): Query<BudgetQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        json!({"budgets":billing::budgets(&state.conn(),q.scope.as_deref(),q.scope_id).map_err(mutation_error)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetUpdate {
    #[serde(deserialize_with = "required_limit")]
    limit_micro: Option<i64>,
}
fn required_limit<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Option::<i64>::deserialize(d)
}
async fn update_budget(
    _auth: AdminAuth,
    State(state): State<SharedState>,
    Query(q): Query<BudgetQuery>,
    payload: Result<Json<BudgetUpdate>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) = payload
        .map_err(|_| ApiError::bad_request("Invalid budget body; limit_micro is required"))?;
    let scope = q
        .scope
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("scope is required"))?;
    let id = q
        .scope_id
        .ok_or_else(|| ApiError::bad_request("scope_id is required"))?;
    billing::set_budget(
        &state.conn(),
        scope,
        id,
        b.limit_micro,
        &_auth.actor_label(),
    )
    .map_err(mutation_error)?;
    Ok(Json(
        json!({"budgets":billing::budgets(&state.conn(),Some(scope),Some(id))?}),
    ))
}
async fn admin_usage(
    _auth: AdminAuth,
    State(state): State<SharedState>,
    Query(q): Query<UsageQuery>,
) -> Result<Json<Value>, ApiError> {
    let entries = billing::usage(&state.conn(), &q)?;
    let next = entries.last().map(|e| e.id);
    Ok(Json(json!({"entries":entries,"next_before_id":next})))
}
async fn self_usage(
    key: GatewayKey,
    State(state): State<SharedState>,
    Query(mut q): Query<UsageQuery>,
) -> Result<Json<Value>, ApiError> {
    q.user_id = Some(key.auth.user_id);
    q.key_id = Some(key.auth.key_id);
    let conn = state.conn();
    let entries = billing::usage(&conn, &q)?;
    let next = entries.last().map(|e| e.id);
    let mut budgets = billing::budgets(&conn, Some("user"), Some(key.auth.user_id))?;
    budgets.extend(billing::budgets(&conn, Some("key"), Some(key.auth.key_id))?);
    Ok(Json(
        json!({"entries":entries,"next_before_id":next,"budgets":budgets}),
    ))
}
