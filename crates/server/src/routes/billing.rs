//! Pricing and budget administration; key-authenticated usage is always key-scoped.
use super::{identity::AdminAuth, proxy::GatewayKey, ApiError};
use crate::{
    db::{
        self,
        billing::{self, Price, RequestPrice, UsageQuery},
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
        .route(
            "/api/request-pricing",
            get(request_prices).post(create_request_price),
        )
        .route("/api/budgets", get(budgets).put(update_budget))
        .route("/api/billing/objects", get(objects))
        .route("/api/usage", get(admin_usage))
        .route("/v1/usage", get(self_usage))
}
fn mutation_error(e: db::DbError) -> ApiError {
    match e {
        db::DbError::Other(s) => ApiError::bad_request(s),
        e => e.into(),
    }
}
async fn objects(_auth: AdminAuth, State(state): State<SharedState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(db::billing_management::objects(&state.conn())?))
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
async fn request_prices(
    _auth: AdminAuth,
    State(state): State<SharedState>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        json!({"request_prices":billing::list_request_prices(&state.conn())?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewRequestPrice {
    channel_id: i64,
    model: String,
    micro_per_call: i64,
}
async fn create_request_price(
    auth: AdminAuth,
    State(state): State<SharedState>,
    payload: Result<Json<NewRequestPrice>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(p) = payload.map_err(|_| ApiError::bad_request("Invalid request pricing body"))?;
    let price = RequestPrice {
        id: 0,
        channel_id: p.channel_id,
        model: p.model,
        micro_per_call: p.micro_per_call,
    };
    let result = billing::create_request_price(&state.conn(), &price, &auth.actor_label())
        .map_err(mutation_error)?;
    Ok(Json(json!({"request_price":result})))
}
#[derive(Deserialize)]
struct BudgetQuery {
    scope: Option<String>,
    scope_id: Option<i64>,
    state: Option<String>,
}
async fn budgets(
    _auth: AdminAuth,
    State(state): State<SharedState>,
    Query(q): Query<BudgetQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        json!({"budgets":db::billing_management::budgets(&state.conn(),q.scope.as_deref(),q.scope_id,q.state.as_deref()).map_err(mutation_error)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetUpdate {
    #[serde(deserialize_with = "required_limit")]
    limit_micro: Option<i64>,
    #[serde(default)]
    frozen: Option<bool>,
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
    db::billing_management::update_budget(
        &state.conn(),
        scope,
        id,
        b.limit_micro,
        b.frozen,
        &_auth.actor_label(),
    )
    .map_err(mutation_error)?;
    Ok(Json(
        json!({"budgets":db::billing_management::budgets(&state.conn(),Some(scope),Some(id),Some("all"))?}),
    ))
}
async fn admin_usage(
    _auth: AdminAuth,
    State(state): State<SharedState>,
    Query(q): Query<UsageQuery>,
) -> Result<Json<Value>, ApiError> {
    let conn = state.conn();
    let entries = billing::usage(&conn, &q).map_err(mutation_error)?;
    let next = billing::usage_next_before(&conn, &q, &entries).map_err(mutation_error)?;
    let entries=db::billing_management::enrich_entries(&conn,entries)?;
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
    let entries = billing::usage(&conn, &q).map_err(mutation_error)?;
    let next = billing::usage_next_before(&conn, &q, &entries).map_err(mutation_error)?;
    let mut budgets = billing::budgets(&conn, Some("user"), Some(key.auth.user_id))?;
    budgets.extend(billing::budgets(&conn, Some("key"), Some(key.auth.key_id))?);
    Ok(Json(
        json!({"entries":entries,"next_before_id":next,"budgets":budgets}),
    ))
}

#[cfg(test)]
mod request_pricing_tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn request_pricing_is_admin_only_versioned_and_ledger_preserves_units() {
        let dir =
            std::env::temp_dir().join(format!("request-pricing-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::AppState::init(crate::Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        let admin = db::sessions::create_session(&state.conn(), None).unwrap();
        let (key, secret) = db::identity::create_key(
            &state.conn(),
            1,
            "request-pricing-test",
            &db::identity::Policy::default(),
            None,
            None,
        )
        .unwrap();
        async fn call(
            state: &SharedState,
            method: &str,
            path: &str,
            token: &str,
            body: Value,
        ) -> (StatusCode, Value) {
            let response = crate::app(state.clone())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }
        assert_eq!(
            call(&state, "GET", "/api/request-pricing", "", Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&state, "GET", "/api/request-pricing", &secret, Value::Null)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(call(&state,"GET","/api/billing/objects","",Value::Null).await.0,StatusCode::UNAUTHORIZED);
        assert_eq!(call(&state,"GET","/api/billing/objects",&secret,Value::Null).await.0,StatusCode::FORBIDDEN);
        let (object_status, objects)=call(&state,"GET","/api/billing/objects",&admin,Value::Null).await;
        assert_eq!(object_status,StatusCode::OK);
        assert!(objects["keys"].as_array().unwrap().iter().any(|v|v["name"]=="request-pricing-test"));
        assert!(!objects.to_string().contains(&secret));
        let (bad_budget,_)=call(&state,"PUT","/api/budgets?scope=user&scope_id=999999",&admin,json!({"limit_micro":1})).await;
        assert_eq!(bad_budget,StatusCode::BAD_REQUEST);
        let (bad_state,_)=call(&state,"GET","/api/usage?state=invalid",&admin,Value::Null).await;
        assert_eq!(bad_state,StatusCode::BAD_REQUEST);
        let body = json!({"channel_id":1,"model":"web-search","micro_per_call":750});
        let (status, first) =
            call(&state, "POST", "/api/request-pricing", &admin, body.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["request_price"]["micro_per_call"], 750);
        let first_id = first["request_price"]["id"].as_i64().unwrap();
        let p = billing::request_price(&state.conn(), "web-search", 1)
            .unwrap()
            .unwrap();
        for (attempt, units) in [("one", Some(1)), ("zero", Some(0)), ("unknown", None)] {
            let id = billing::reserve_request(&state.conn(), 1, key.id, 1, attempt, &p).unwrap();
            billing::settle_request(&state.conn(), id, units, "ok").unwrap();
        }
        let mut next = body.clone();
        next["micro_per_call"] = json!(1000);
        assert_eq!(
            call(&state, "POST", "/api/request-pricing", &admin, next)
                .await
                .0,
            StatusCode::OK
        );
        let (_, list) = call(&state, "GET", "/api/request-pricing", &admin, Value::Null).await;
        assert_eq!(list["request_prices"].as_array().unwrap().len(), 2);
        assert_eq!(list["request_prices"][0]["micro_per_call"], 1000);
        let (_, usage) = call(&state, "GET", "/api/usage", &admin, Value::Null).await;
        let entries = usage["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|r| r["billing_unit"] == "request"
            && r["input_tokens"].is_null()
            && r["output_tokens"].is_null()
            && r["request_price_id"] == first_id
            && r["request_price_snapshot"]["micro_per_call"] == 750));
        assert!(entries
            .iter()
            .any(|r| r["request_units"] == 1 && r["debit_micro"] == 750));
        assert!(entries
            .iter()
            .any(|r| r["request_units"] == 0 && r["debit_micro"] == 0));
        assert!(entries
            .iter()
            .any(|r| r["request_units"].is_null() && r["observed_cost_micro"].is_null()));
        let mut invalid = body.clone();
        invalid["micro_per_call"] = json!(-1);
        assert_eq!(
            call(&state, "POST", "/api/request-pricing", &admin, invalid)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let mut invalid = body;
        invalid["input_tokens"] = json!(1);
        assert_eq!(
            call(&state, "POST", "/api/request-pricing", &admin, invalid)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }
}
