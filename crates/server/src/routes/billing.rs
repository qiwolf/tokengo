//! Pricing and cost reporting; local monetary budget enforcement is disabled.
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
        .merge(super::model_limits::router())
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
        .route("/api/usage/analytics", get(admin_usage_analytics))
        .route("/v1/usage", get(self_usage))
        .route("/v1/usage/analytics", get(self_usage_analytics))
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
    #[serde(default,deserialize_with="provided_token_bound")] max_input_tokens: Option<i64>,
    #[serde(default,deserialize_with="provided_token_bound")] max_output_tokens: Option<i64>,
}
// Omitted limits use automatic metadata; explicitly supplied null/non-integers
// are invalid instead of silently reverting an administrator's edited value.
fn provided_token_bound<'de,D:serde::Deserializer<'de>>(deserializer:D)->Result<Option<i64>,D::Error>{
    i64::deserialize(deserializer).map(Some)
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
    let non_generation=option.as_ref().is_some_and(|v|matches!(v.task_type.as_str(),"embeddings"|"rerank"));
    let generation=option.as_ref().is_some_and(|v|v.task_type=="chat");
    if p.max_input_tokens.is_some_and(|v|v<=0)||p.max_output_tokens.is_some_and(|v|v<0||(generation&&v==0)) {
        return Err(ApiError::bad_request("输入窗口须为正整数；生成模型的最大输出须为正整数。"));
    }
    if non_generation&&p.max_output_tokens.is_some_and(|v|v!=0) {
        return Err(ApiError::bad_request("向量与重排模型没有生成输出，最大输出须为0。"));
    }
    let inherited_input = option.as_ref().and_then(|v| v.input_usage_reference);
    let inherited_output = option.as_ref().and_then(|v| v.output_usage_reference);
    for (submitted, inherited, source) in [
        (p.max_input_tokens, inherited_input, option.as_ref().and_then(|v| v.input_bound_source)),
        (p.max_output_tokens, inherited_output, option.as_ref().and_then(|v| v.output_bound_source)),
    ] {
        if source == Some("channel_model") && submitted.is_some() && submitted != inherited {
            return Err(ApiError::bad_request("用量参考值由渠道模型维护，请在渠道模型中修改。"));
        }
    }
    // New callers submit rates. Keep the explicit legacy payload compatible where
    // it does not override a reference explicitly maintained on the channel model.
    let max_input_tokens = p.max_input_tokens.or(inherited_input)
        .ok_or_else(|| ApiError::bad_request("请先在渠道模型中维护输入用量参考值。"))?;
    let max_output_tokens = p.max_output_tokens.or(inherited_output)
        .ok_or_else(|| ApiError::bad_request("请先在渠道模型中维护输出用量参考值。"))?;
    if max_input_tokens<=0||max_output_tokens<0||(generation&&max_output_tokens==0) {
        return Err(ApiError::bad_request("模型输入/输出窗口无效，请填写有效的token数量。"));
    }
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
    channel_id:i64,model:String,unit:String,micro_per_unit:i64,
    #[serde(default,deserialize_with="provided_token_bound")]
    max_quantity_milli:Option<i64>,
    #[serde(default="billing::cny_currency")] currency:String,
}
async fn create_unit_price(auth:AdminAuth,State(state):State<SharedState>,payload:Result<Json<NewUnitPrice>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError> {
    let Json(p)=payload.map_err(|_|ApiError::bad_request("Invalid unit pricing body"))?;
    store_unit_price(&state.conn(),p,&auth.actor_label(),None)
}
fn store_unit_price(c:&rusqlite::Connection,p:NewUnitPrice,actor:&str,previous:Option<i64>)->Result<Json<Value>,ApiError> {
    if let Some(id)=previous {billing::require_current_price(c,"unit",id,p.channel_id,&p.model,&p.currency).map_err(mutation_error)?;}
    let option=super::pricing_options::for_pair(c,p.channel_id,&p.model).map_err(mutation_error)?;
    super::pricing_options::require_kind(option.as_ref(),"unit").map_err(mutation_error)?;
    if option.as_ref().is_some_and(|v|!v.allowed_units.contains(&p.unit.as_str())) {
        return Err(ApiError::bad_request("所选计价单位不适用于该模型类型。"));
    }
    let inherited = option.as_ref().and_then(|v| v.quantity_usage_references.get(p.unit.as_str())).copied();
    let source = option.as_ref().and_then(|v| v.quantity_reference_sources.get(p.unit.as_str())).copied();
    if source == Some("channel_model") && p.max_quantity_milli.is_some() && p.max_quantity_milli != inherited {
        return Err(ApiError::bad_request("用量参考值由渠道模型维护，请在渠道模型中修改。"));
    }
    let max_quantity_milli = p.max_quantity_milli.or(inherited)
        .ok_or_else(|| ApiError::bad_request("请先在渠道模型中维护此计价单位的用量参考值。"))?;
    let p=billing::UnitPrice{id:0,channel_id:p.channel_id,model:p.model,currency:p.currency,unit:p.unit,micro_per_unit:p.micro_per_unit,max_quantity_milli};
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
        json!({"budget_enforcement":false,"budgets":db::billing_management::budgets_currency(&state.conn(),q.scope.as_deref(),q.scope_id,q.state.as_deref(),&q.currency).map_err(mutation_error)?}),
    ))
}
async fn update_budget(_auth:AdminAuth)->Result<Json<Value>,ApiError> {
    Err(ApiError::new(axum::http::StatusCode::GONE,"budget_enforcement_disabled","本地预算门限已取消，费用只记录，不再设置限额或冻结。"))
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
        let entries=db::billing_management::enrich_entries_with_catalog(entries,&catalog)?;
        json!({"budget_enforcement":false,"entries":entries,"next_before_id":next})
    };
    apply_reporting(&state,&mut response).await;
    Ok(Json(response))
}
// Full-range statistics have a separate request lifecycle from cursor paging.
// Both functions still read the same filters/catalog and canonical ledger.
async fn admin_usage_analytics(
    _auth: AdminAuth,
    State(state): State<SharedState>,
    Query(q): Query<UsageQuery>,
) -> Result<Json<Value>, ApiError> {
    let mut response=read_usage_report(&state,q,UsageReportAccess::Admin).await?;
    apply_reporting(&state,&mut response).await;
    Ok(Json(response))
}
// Each report owns a read-only WAL snapshot. Slow groupings must not hold the
// shared writer connection or a Tokio worker while a user turns a history page.
pub(super) enum UsageReportAccess { Admin, Browser{session:String,user:i64}, Key{key:i64,user:i64} }
pub(super) async fn read_usage_report(state:&SharedState,q:UsageQuery,access:UsageReportAccess)->Result<Value,ApiError> {
    let state=state.clone();
    tokio::task::spawn_blocking(move|| {
        let report=|conn:&rusqlite::Connection|->Result<Value,ApiError>{
            match &access {
                UsageReportAccess::Browser{session,user}=>{
                    let current=db::accounts::authenticate(conn,session)?;
                    if !current.is_some_and(|a|a.account.role=="user"&&a.account.user_id==Some(*user)) {return Err(ApiError::unauthorized("账号不可用"));}
                },
                UsageReportAccess::Key{key,user}=>{
                    if !db::identity::context(conn,*key)?.is_some_and(|a|a.user_id==*user) {return Err(ApiError::unauthorized("Virtual key is inactive or invalid"));}
                },
                UsageReportAccess::Admin=>{},
            }
            usage_report(conn,&q,None)
        };
        with_usage_read_snapshot(&state,report)
    }).await.map_err(|_|ApiError::internal("用量统计处理失败"))?
}
fn with_usage_read_snapshot(state:&SharedState,report:impl FnOnce(&rusqlite::Connection)->Result<Value,ApiError>)->Result<Value,ApiError> {
    // SQLite's private-memory database cannot be reopened. File-backed
    // production reports use an independent read-only transaction.
    if state.cfg.db_path==std::path::Path::new(":memory:") {return report(&state.conn());}
    let conn=rusqlite::Connection::open_with_flags(&state.cfg.db_path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY|rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(db::DbError::from)?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(db::DbError::from)?;
    conn.execute_batch("BEGIN DEFERRED").map_err(db::DbError::from)?;
    let value=report(&conn)?;
    conn.execute_batch("COMMIT").map_err(db::DbError::from)?;
    Ok(value)
}

pub(super) fn usage_report(conn:&rusqlite::Connection,q:&UsageQuery,catalog:Option<&Value>)->Result<Value,ApiError> {
    let summary=billing::usage_summary_with_catalog(conn,q,catalog).map_err(mutation_error)?;
    let analytics=billing::usage_analytics_with_catalog(conn,q,catalog,&summary).map_err(mutation_error)?;
    Ok(json!({"summary":summary,"analytics":analytics}))
}
pub(super) async fn apply_reporting(state:&SharedState,response:&mut Value) {
    let quote=if crate::reporting_currency::needs_quote(response){crate::reporting_currency::quote(state).await}else{None};
    crate::reporting_currency::apply(response,quote.as_ref());
}
async fn self_usage(
    key: GatewayKey,
    State(state): State<SharedState>,
    Query(mut q): Query<UsageQuery>,
) -> Result<Json<Value>, ApiError> {
    q.user_id=Some(key.auth.user_id);q.key_id=Some(key.auth.key_id);
    let mut response = {
        let conn=state.conn();
        let (entries,next)=billing::usage_page(&conn,&q).map_err(mutation_error)?;
        let currency=billing::usage_currency(&q).map_err(mutation_error)?;
        let currencies=if currency=="all" {vec!["CNY","USD"]}else{vec![currency]};
        let mut budgets=Vec::new();
        for currency in currencies {
            budgets.extend(billing::budgets_currency(&conn,Some("user"),Some(key.auth.user_id),currency)?);
            budgets.extend(billing::budgets_currency(&conn,Some("key"),Some(key.auth.key_id),currency)?);
        }
        json!({"budget_enforcement":false,"entries":entries,"next_before_id":next,"budgets":budgets})
    };
    apply_reporting(&state,&mut response).await;
    Ok(Json(response))
}
async fn self_usage_analytics(
    key: GatewayKey,
    State(state): State<SharedState>,
    Query(mut q): Query<UsageQuery>,
) -> Result<Json<Value>, ApiError> {
    q.user_id=Some(key.auth.user_id);q.key_id=Some(key.auth.key_id);
    let mut response=read_usage_report(&state,q,UsageReportAccess::Key{key:key.auth.key_id,user:key.auth.user_id}).await?;
    apply_reporting(&state,&mut response).await;
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
        assert_eq!(bad_budget,StatusCode::GONE);
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
        assert_eq!(status,StatusCode::OK);assert_eq!(window["entries"].as_array().unwrap().len(),1);assert!(window.get("summary").is_none());
        let (status,window)=call(&state,"GET","/api/usage/analytics?start_at=10&end_at=20&limit=1",&admin,Value::Null).await;assert_eq!(status,StatusCode::OK);
        assert_eq!(window["summary"]["ledger_count"],3);assert_eq!(window["summary"]["unknown_input_entries"],0); // Request billing has no token quantity.
        assert_eq!(window["analytics"]["buckets"][0]["total_tokens"],window["summary"]["total_tokens"]);
        assert_eq!(window["analytics"]["start_at"],10);assert_eq!(window["analytics"]["end_at"],20);
        let (status,own)=call(&state,"GET","/v1/usage/analytics?start_at=10&end_at=20&user_id=999&key_id=999&limit=1",&secret,Value::Null).await;
        assert_eq!(status,StatusCode::OK);assert_eq!(own["summary"]["ledger_count"],3);
        assert_eq!(own["analytics"]["buckets"][0]["total_tokens"],own["summary"]["total_tokens"]);
        assert_eq!(own["analytics"]["models"].as_array().unwrap().len(),1);
        let (_,empty)=call(&state,"GET","/api/usage?start_at=20&end_at=30",&admin,Value::Null).await;
        assert!(empty.get("summary").is_none());assert!(empty["entries"].as_array().unwrap().is_empty());
        let (_,empty)=call(&state,"GET","/api/usage/analytics?start_at=20&end_at=30",&admin,Value::Null).await;assert_eq!(empty["summary"]["ledger_count"],0);
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

#[cfg(test)]
mod usage_snapshot_tests {
    use super::*;
    #[tokio::test]
    async fn read_only_report_snapshot_allows_writer_and_keeps_one_consistent_view() {
        let dir=std::env::temp_dir().join(format!("usage-snapshot-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state=crate::AppState::init(crate::Config{db_path:dir.join("db"),master_key_path:dir.join("key").to_string_lossy().into(),..Default::default()}).unwrap();
        let value=with_usage_read_snapshot(&state,|reader| {
            let before:i64=reader.query_row("SELECT count(*) FROM requests",[],|r|r.get(0)).map_err(db::DbError::from)?;
            assert!(reader.execute("INSERT INTO requests(ts_ms,endpoint,model,status) VALUES(1,'/v1/chat/completions','must-not-write','ok')",[]).is_err());
            // This would deadlock if the reader still owned state.conn().
            state.conn().execute("INSERT INTO requests(ts_ms,endpoint,model,status) VALUES(1,'/v1/chat/completions','concurrent-writer','ok')",[]).unwrap();
            let after:i64=reader.query_row("SELECT count(*) FROM requests",[],|r|r.get(0)).map_err(db::DbError::from)?;
            Ok(json!({"before":before,"after":after}))
        }).unwrap();
        assert_eq!(value,json!({"before":0,"after":0}));
        assert_eq!(state.conn().query_row("SELECT count(*) FROM requests",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        drop(state);std::fs::remove_dir_all(dir).unwrap();
    }
}
