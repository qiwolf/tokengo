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
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/pricing", get(prices).post(create))
        .route("/api/pricing/{id}",axum::routing::put(update_token_price).patch(token_price_state).delete(delete_token_price))
        .route("/api/pricing/options",get(pricing_options))
        .route("/api/unit-pricing",get(unit_prices).post(create_unit_price))
        .route("/api/unit-pricing/{id}",axum::routing::put(update_unit_price).patch(unit_price_state).delete(delete_unit_price))
        .route(
            "/api/request-pricing",
            get(request_prices).post(create_request_price),
        )
        .route("/api/request-pricing/{id}",axum::routing::put(update_request_price).patch(request_price_state).delete(delete_request_price))
        .route("/api/budgets", get(budgets).put(update_budget))
        .route("/api/billing/objects", get(objects))
        .route("/api/usage", get(admin_usage))
        .route("/v1/usage", get(self_usage))
}
fn mutation_error(e: db::DbError) -> ApiError {
    match e {
        db::DbError::Other(s) if s.starts_with("price_conflict:")=>ApiError::new(axum::http::StatusCode::CONFLICT,"price_conflict",s),
        db::DbError::Other(s) if s.starts_with("unknown_price:")=>ApiError::not_found(s),
        db::DbError::Other(s) => ApiError::bad_request(s),
        e => e.into(),
    }
}
async fn objects(_auth: AdminAuth, State(state): State<SharedState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(db::billing_management::objects(&state.conn())?))
}
#[derive(Deserialize,Default)]
struct PriceListQuery {#[serde(default)] include_deleted:bool}
async fn prices(_auth:AdminAuth,State(state):State<SharedState>,Query(q):Query<PriceListQuery>)->Result<Json<Value>,ApiError> {
    Ok(Json(json!({"prices":billing::price_lifecycle_rows(&state.conn(),"token",q.include_deleted).map_err(mutation_error)?})))
}
async fn pricing_options(_auth:AdminAuth,State(state):State<SharedState>)->Result<Json<Value>,ApiError> {
    Ok(Json(json!({"options":super::pricing_options::list(&state.conn())?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewPrice {
    #[serde(default="billing::cny_currency")] currency:String,
    channel_id: i64,
    model: String,
    input_micro_per_million: i64,
    output_micro_per_million: i64,
    #[serde(default)] max_input_tokens: Option<i64>,
    #[serde(default)] max_output_tokens: Option<i64>,
}
async fn create(
    _auth: AdminAuth,
    State(state): State<SharedState>,
    payload: Result<Json<NewPrice>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(p) = payload.map_err(|_| ApiError::bad_request("Invalid pricing body"))?;
    store_token_price(&state.conn(),p,&_auth.actor_label(),None)
}
fn store_token_price(c:&rusqlite::Connection,p:NewPrice,actor:&str,previous:Option<i64>)->Result<Json<Value>,ApiError> {

    if let Some(id)=previous {billing::require_current_price(c,"token",id,p.channel_id,&p.model,&p.currency).map_err(mutation_error)?;}
    let option=super::pricing_options::for_pair(&c,p.channel_id,&p.model).map_err(mutation_error)?;
    super::pricing_options::require_kind(option.as_ref(),"token").map_err(mutation_error)?;
    let max_input_tokens=option.as_ref().and_then(|v|v.max_input_tokens).or(p.max_input_tokens).ok_or_else(||ApiError::bad_request("未找到模型的单次输入上限，请补充 token 数量。"))?;
    let max_output_tokens=option.as_ref().and_then(|v|v.max_output_tokens).or(p.max_output_tokens).ok_or_else(||ApiError::bad_request("未找到模型的单次总输出上限，请补充 token 数量。"))?;
    let p = Price {
        currency:p.currency,
        id: 0,
        channel_id: p.channel_id,
        model: p.model,
        input_micro_per_million: p.input_micro_per_million,
        output_micro_per_million: p.output_micro_per_million,
        max_input_tokens,
        max_output_tokens,
    };
    let result =
        billing::create_price(c, &p, actor).map_err(mutation_error)?;
    Ok(Json(json!({"price":billing::enrich_price(c,"token",serde_json::to_value(result).map_err(|_|ApiError::internal("price serialization failed"))?).map_err(mutation_error)?})))
}
async fn update_token_price(auth:AdminAuth,State(state):State<SharedState>,Path(id):Path<i64>,payload:Result<Json<NewPrice>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError> {
    let Json(p)=payload.map_err(|_|ApiError::bad_request("Invalid pricing body"))?;
    store_token_price(&state.conn(),p,&auth.actor_label(),Some(id))
}

async fn request_prices(_auth:AdminAuth,State(state):State<SharedState>,Query(q):Query<PriceListQuery>)->Result<Json<Value>,ApiError> {
    Ok(Json(json!({"request_prices":billing::price_lifecycle_rows(&state.conn(),"request",q.include_deleted).map_err(mutation_error)?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewRequestPrice {
    #[serde(default="billing::cny_currency")] currency:String,
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
    store_request_price(&state.conn(),p,&auth.actor_label(),None)
}
fn store_request_price(c:&rusqlite::Connection,p:NewRequestPrice,actor:&str,previous:Option<i64>)->Result<Json<Value>,ApiError> {
    if let Some(id)=previous {billing::require_current_price(c,"request",id,p.channel_id,&p.model,&p.currency).map_err(mutation_error)?;}
    if !p.model.starts_with("passthrough:") {
        let option=super::pricing_options::for_pair(&c,p.channel_id,&p.model).map_err(mutation_error)?;
        super::pricing_options::require_kind(option.as_ref(),"request").map_err(mutation_error)?;
    }
    let price = RequestPrice {
        currency:p.currency,
        id: 0,
        channel_id: p.channel_id,
        model: p.model,
        micro_per_call: p.micro_per_call,
    };
    let result = billing::create_request_price(c, &price, actor)
        .map_err(mutation_error)?;
    Ok(Json(json!({"request_price":billing::enrich_price(c,"request",serde_json::to_value(result).map_err(|_|ApiError::internal("price serialization failed"))?).map_err(mutation_error)?})))
}
async fn update_request_price(auth:AdminAuth,State(state):State<SharedState>,Path(id):Path<i64>,payload:Result<Json<NewRequestPrice>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError> {
    let Json(p)=payload.map_err(|_|ApiError::bad_request("Invalid request pricing body"))?;
    store_request_price(&state.conn(),p,&auth.actor_label(),Some(id))
}

async fn unit_prices(_auth:AdminAuth,State(state):State<SharedState>,Query(q):Query<PriceListQuery>)->Result<Json<Value>,ApiError> {
    Ok(Json(json!({"unit_prices":billing::price_lifecycle_rows(&state.conn(),"unit",q.include_deleted).map_err(mutation_error)?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewUnitPrice {
    channel_id:i64,model:String,unit:String,micro_per_unit:i64,max_quantity_milli:i64,
    #[serde(default="billing::cny_currency")] currency:String,
}
async fn create_unit_price(auth:AdminAuth,State(state):State<SharedState>,payload:Result<Json<NewUnitPrice>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError> {
    let Json(p)=payload.map_err(|_|ApiError::bad_request("Invalid unit pricing body"))?;
    store_unit_price(&state.conn(),p,&auth.actor_label(),None)
}
fn store_unit_price(c:&rusqlite::Connection,p:NewUnitPrice,actor:&str,previous:Option<i64>)->Result<Json<Value>,ApiError> {
    if let Some(id)=previous {billing::require_current_price(c,"unit",id,p.channel_id,&p.model,&p.currency).map_err(mutation_error)?;}
    {
        let option=super::pricing_options::for_pair(&c,p.channel_id,&p.model).map_err(mutation_error)?;
        super::pricing_options::require_kind(option.as_ref(),"unit").map_err(mutation_error)?;
        if option.as_ref().is_some_and(|v|!v.allowed_units.contains(&p.unit.as_str())) {return Err(ApiError::bad_request("所选计价单位不适用于该模型类型。"));}
    }
    let p=billing::UnitPrice{id:0,channel_id:p.channel_id,model:p.model,currency:p.currency,unit:p.unit,micro_per_unit:p.micro_per_unit,max_quantity_milli:p.max_quantity_milli};
    let result=billing::create_unit_price(c,&p,actor).map_err(mutation_error)?;
    Ok(Json(json!({"unit_price":billing::enrich_price(c,"unit",serde_json::to_value(result).map_err(|_|ApiError::internal("price serialization failed"))?).map_err(mutation_error)?})))
}
async fn update_unit_price(auth:AdminAuth,State(state):State<SharedState>,Path(id):Path<i64>,payload:Result<Json<NewUnitPrice>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError> {
    let Json(p)=payload.map_err(|_|ApiError::bad_request("Invalid unit pricing body"))?;
    store_unit_price(&state.conn(),p,&auth.actor_label(),Some(id))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PriceStateUpdate {enabled:bool}
fn lifecycle_response(state:&SharedState,auth:&AdminAuth,kind:&str,item:&str,id:i64,enabled:Option<bool>)->Result<Json<Value>,ApiError> {
    let row=billing::change_price_state(&state.conn(),kind,id,enabled,&auth.actor_label()).map_err(mutation_error)?;
    let mut value=json!({});value[item]=row;Ok(Json(value))
}

#[derive(Deserialize)]
struct BudgetQuery {
    #[serde(default="billing::cny_currency")] currency:String,
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
        json!({"budgets":db::billing_management::budgets_currency(&state.conn(),q.scope.as_deref(),q.scope_id,q.state.as_deref(),&q.currency).map_err(mutation_error)?}),
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
    db::billing_management::update_budget_currency(
        &state.conn(),
        scope,
        id,
        b.limit_micro,
        b.frozen,
        &_auth.actor_label(),
        &q.currency,
    )
    .map_err(mutation_error)?;
    Ok(Json(
        json!({"budgets":db::billing_management::budgets_currency(&state.conn(),Some(scope),Some(id),Some("all"),&q.currency)?}),
    ))
}
async fn admin_usage(
    _auth: AdminAuth,
    State(state): State<SharedState>,
    Query(q): Query<UsageQuery>,
) -> Result<Json<Value>, ApiError> {
    let mut response = {
    let conn = state.conn();
    let lifecycle_state=db::billing_management::validated_state(q.state.as_deref(),"all").map_err(mutation_error)?;
    let catalog=if lifecycle_state=="active" {db::billing_management::current_objects(&conn)?}else{db::billing_management::objects(&conn)?};
    let (entries,next)=billing::usage_page_with_catalog(&conn,&q,Some(&catalog)).map_err(mutation_error)?;
    let summary=billing::usage_summary_with_catalog(&conn,&q,Some(&catalog)).map_err(mutation_error)?;
    let analytics=billing::usage_analytics_with_catalog(&conn,&q,Some(&catalog),&summary).map_err(mutation_error)?;
    let entries=db::billing_management::enrich_entries_with_catalog(entries,&catalog)?;
    json!({"entries":entries,"next_before_id":next,"summary":summary,"analytics":analytics})
    };
    let quote=if crate::reporting_currency::needs_quote(&response){crate::reporting_currency::quote(&state).await}else{None};
    crate::reporting_currency::apply(&mut response,quote.as_ref());
    Ok(Json(response))
}
async fn self_usage(
    key: GatewayKey,
    State(state): State<SharedState>,
    Query(mut q): Query<UsageQuery>,
) -> Result<Json<Value>, ApiError> {
    q.user_id = Some(key.auth.user_id);
    q.key_id = Some(key.auth.key_id);
    let mut response = {
    let conn = state.conn();
    let (entries,next)=billing::usage_page(&conn,&q).map_err(mutation_error)?;
    let summary=billing::usage_summary(&conn,&q).map_err(mutation_error)?;
    let analytics=billing::usage_analytics(&conn,&q,&summary).map_err(mutation_error)?;
    let currency=billing::usage_currency(&q).map_err(mutation_error)?;
    let currencies=if currency=="all" {vec!["CNY","USD"]}else{vec![currency]};
    let mut budgets=Vec::new();
    for currency in currencies {
        budgets.extend(billing::budgets_currency(&conn,Some("user"),Some(key.auth.user_id),currency)?);
        budgets.extend(billing::budgets_currency(&conn,Some("key"),Some(key.auth.key_id),currency)?);
    }
    json!({"entries":entries,"next_before_id":next,"budgets":budgets,"summary":summary,"analytics":analytics})
    };
    let quote=if crate::reporting_currency::needs_quote(&response){crate::reporting_currency::quote(&state).await}else{None};
    crate::reporting_currency::apply(&mut response,quote.as_ref());
    Ok(Json(response))
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
        // Both API entry points validate the range and aggregate all matching
        // attempts, independently of the requested page size.
        state.conn().execute("UPDATE billing_ledger SET created_ms=15000",[]).unwrap();
        let (status,window)=call(&state,"GET","/api/usage?start_at=10&end_at=20&limit=1",&admin,Value::Null).await;
        assert_eq!(status,StatusCode::OK);assert_eq!(window["entries"].as_array().unwrap().len(),1);
        assert_eq!(window["summary"]["ledger_count"],3);assert_eq!(window["summary"]["unknown_input_entries"],0); // Request billing has no token quantity.
        assert_eq!(window["analytics"]["buckets"][0]["total_tokens"],window["summary"]["total_tokens"]);
        assert_eq!(window["analytics"]["start_at"],10);assert_eq!(window["analytics"]["end_at"],20);
        let (status,own)=call(&state,"GET","/v1/usage?start_at=10&end_at=20&user_id=999&key_id=999&limit=1",&secret,Value::Null).await;
        assert_eq!(status,StatusCode::OK);assert_eq!(own["summary"]["ledger_count"],3);
        assert_eq!(own["analytics"]["buckets"][0]["total_tokens"],own["summary"]["total_tokens"]);
        assert_eq!(own["analytics"]["models"].as_array().unwrap().len(),1);
        let (_,empty)=call(&state,"GET","/api/usage?start_at=20&end_at=30",&admin,Value::Null).await;
        assert_eq!(empty["summary"]["ledger_count"],0);assert!(empty["entries"].as_array().unwrap().is_empty());
        for path in ["/api/usage?start_at=20&end_at=10","/v1/usage?start_at=10&end_at=10"] {
            let token=if path.starts_with("/api/"){&admin}else{&secret};
            assert_eq!(call(&state,"GET",path,token,Value::Null).await.0,StatusCode::BAD_REQUEST);
        }
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

async fn token_price_state(auth:AdminAuth,State(state):State<SharedState>,Path(id):Path<i64>,payload:Result<Json<PriceStateUpdate>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError> {
    let Json(p)=payload.map_err(|_|ApiError::bad_request("Invalid price state; enabled is required"))?;
    lifecycle_response(&state,&auth,"token","price",id,Some(p.enabled))
}
async fn delete_token_price(auth:AdminAuth,State(state):State<SharedState>,Path(id):Path<i64>)->Result<Json<Value>,ApiError> {
    lifecycle_response(&state,&auth,"token","price",id,None)
}

async fn unit_price_state(auth:AdminAuth,State(state):State<SharedState>,Path(id):Path<i64>,payload:Result<Json<PriceStateUpdate>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError> {
    let Json(p)=payload.map_err(|_|ApiError::bad_request("Invalid price state; enabled is required"))?;
    lifecycle_response(&state,&auth,"unit","unit_price",id,Some(p.enabled))
}
async fn delete_unit_price(auth:AdminAuth,State(state):State<SharedState>,Path(id):Path<i64>)->Result<Json<Value>,ApiError> {
    lifecycle_response(&state,&auth,"unit","unit_price",id,None)
}

async fn request_price_state(auth:AdminAuth,State(state):State<SharedState>,Path(id):Path<i64>,payload:Result<Json<PriceStateUpdate>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError> {
    let Json(p)=payload.map_err(|_|ApiError::bad_request("Invalid price state; enabled is required"))?;
    lifecycle_response(&state,&auth,"request","request_price",id,Some(p.enabled))
}
async fn delete_request_price(auth:AdminAuth,State(state):State<SharedState>,Path(id):Path<i64>)->Result<Json<Value>,ApiError> {
    lifecycle_response(&state,&auth,"request","request_price",id,None)
}
