//! Root-only explicit provider/source grouping; no credential or connection identity changes.
use super::{admin::PanelAuth, operations::ManagementAuth, ApiError};
use crate::{
    db::{self, channels, model_routes, provider_channels as providers, provider_pool as pool},
    SharedState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, put},
    Json, Router,
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{json, Value};
use tokengo_shared::{same_model_name, ChannelId};
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/providers", get(list).post(create))
        .route("/api/providers/{id}", put(update).delete(delete))
        .route(
            "/api/providers/{id}/credentials",
            get(credentials_list).post(credentials_create),
        )
        .route(
            "/api/providers/{id}/credentials/{credential}",
            axum::routing::patch(credentials_update).delete(credentials_disable),
        )
        .route("/api/providers/{id}/models/discover", axum::routing::post(credential_catalog))
        .route(
            "/api/providers/{id}/credentials/{credential}/reveal",
            axum::routing::post(credentials_reveal).layer(axum::middleware::map_response(secret_no_store)),
        )
        .route("/api/channels/{id}/provider", put(assign))
        .route("/api/providers/{id}/models/enabled", put(group_model))
        .route("/api/channels/{id}/models/enabled", put(connection_model))
        .route("/api/model-visibility", put(visibility))
        .route("/api/channels/{id}/models/reasoning", put(reasoning))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderBody {
    endpoint_mode: Option<String>,
    name: String,
    #[serde(default = "yes")]
    enabled: bool,
    base_url: Option<String>,
    key_strategy: Option<String>,
    switch_on_429: Option<bool>,
    switch_on_400: Option<bool>,
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
async fn list(_root: ManagementAuth, State(s): State<SharedState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(providers::list(&s.conn())?))
}
async fn create(
    _root: ManagementAuth,
    State(s): State<SharedState>,
    body: Result<Json<ProviderBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) = body.map_err(|_| ApiError::bad_request("Invalid provider JSON"))?;
    let n = providers::name(&b.name).map_err(ApiError::bad_request)?;
    if b.endpoint_mode.as_deref().is_some_and(|m| !matches!(m,"single"|"local")) { return Err(ApiError::bad_request("供应商地址模式必须是single或local")); }
    let url = b
        .base_url
        .as_deref()
        .map(pool::canonical_url)
        .transpose()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if let Some(strategy) = &b.key_strategy {
        pool::validate_strategy(strategy).map_err(|e| ApiError::bad_request(e.to_string()))?;
    }
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
        let id = c.last_insert_rowid();
        c.execute("UPDATE provider_channels SET base_url=?2,canonical_state=?3,key_strategy=?4,switch_on_429=?5,switch_on_400=?6 WHERE id=?1",rusqlite::params![id,url,if url.is_some(){"canonical"}else{"attention"},b.key_strategy.unwrap_or_else(||"ordered".into()),b.switch_on_429.unwrap_or(true),b.switch_on_400.unwrap_or(false)])?;
        if b.endpoint_mode.as_deref()==Some("local") {
            if url.is_some() { return Err(db::DbError::Other("本地供应商请在每个模型入口设置API URL".into())); }
            c.execute("UPDATE provider_channels SET endpoint_mode='local',base_url=NULL,canonical_state='canonical' WHERE id=?1",[id])?;
        }
        Ok(id)
    }).map_err(|e|ApiError::bad_request(e.to_string()))?;
    Ok(Json(json!({"provider":providers::one(&c,id)?})))
}
async fn update(
    _root: ManagementAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    body: Result<Json<ProviderBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(b) = body.map_err(|_| ApiError::bad_request("Invalid provider JSON"))?;
    let n = providers::name(&b.name).map_err(ApiError::bad_request)?;
    if b.endpoint_mode.as_deref().is_some_and(|m| !matches!(m,"single"|"local")) { return Err(ApiError::bad_request("供应商地址模式必须是single或local")); }
    let url = b
        .base_url
        .as_deref()
        .map(pool::canonical_url)
        .transpose()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if let Some(strategy) = &b.key_strategy {
        pool::validate_strategy(strategy).map_err(|e| ApiError::bad_request(e.to_string()))?;
    }
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
        let was_local=pool::local(c,id)?;
        let is_local=b.endpoint_mode.as_deref().map(|m|m=="local").unwrap_or(was_local);
        if is_local {
            if url.is_some() { return Err(db::DbError::Other("本地供应商请在每个模型入口设置API URL".into())); }
            let non_api:bool=c.query_row("SELECT adapter_kind<>'api' FROM provider_channels WHERE id=?1",[id],|r|r.get(0))?;
            if non_api { return Err(db::DbError::Other("OAuth或工具供应商不能切换为本地模型".into())); }
            c.execute("UPDATE provider_channels SET endpoint_mode='local',base_url=NULL,canonical_state='canonical' WHERE id=?1",[id])?;
        } else if was_local {
            let mut urls=channels::list(c)?.into_iter().filter(|ch|ch.provider_id==Some(id)).map(|ch|pool::canonical_url(&ch.base_url)).collect::<db::Result<Vec<_>>>()?;
            urls.sort();urls.dedup();
            if urls.len()>1 { return Err(db::DbError::Other("此本地供应商存在多个API URL，不能切换为单地址".into())); }
            let single=urls.first();
            c.execute("UPDATE provider_channels SET endpoint_mode='single',base_url=?2,canonical_state=?3 WHERE id=?1",rusqlite::params![id,single,if single.is_some(){"canonical"}else{"attention"}])?;
        }
        let old_url:Option<String>=c.query_row("SELECT base_url FROM provider_channels WHERE id=?1",[id],|r|r.get(0))?;
        if let Some(url)=&url {
            let oauth:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings o JOIN provider_connections p ON p.channel_id=o.channel_id WHERE p.provider_id=?1)",[id],|r|r.get(0))?;
            if oauth && url!=crate::oauth::API_BASE{return Err(db::DbError::Other("OAuth渠道必须保留官方API入口".into()));}
            if old_url.as_ref()!=Some(url){c.execute("UPDATE channels SET base_url=?2 WHERE id IN (SELECT channel_id FROM provider_connections WHERE provider_id=?1)",rusqlite::params![id,url])?;}
            c.execute("UPDATE provider_channels SET base_url=?2,canonical_state='canonical' WHERE id=?1",rusqlite::params![id,url])?;
        }
        c.execute("UPDATE provider_channels SET key_strategy=COALESCE(?2,key_strategy),switch_on_429=COALESCE(?3,switch_on_429),switch_on_400=COALESCE(?4,switch_on_400),config_version=config_version+1 WHERE id=?1",rusqlite::params![id,b.key_strategy,b.switch_on_429,b.switch_on_400])?;
        reset(c, &names)
    }).map_err(|e|ApiError::bad_request(e.to_string()))?;
    let row = providers::one(&c, id)?;
    drop(c);
    changed(&s, names);
    Ok(Json(json!({"provider":row})))
}
async fn delete(
    _root: ManagementAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    require(&c, id)?;
    let names = db::in_tx(&c, |c| db::lifecycle::delete_provider(c,id))
        .map_err(|e| ApiError::new(axum::http::StatusCode::CONFLICT,"provider_delete_blocked",e.to_string()))?;
    drop(c);
    changed(&s,names);
    Ok(Json(json!({"deleted":true,"history_preserved":true})))
}
async fn assign(
    _root: ManagementAuth,
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
        if let Some(provider) = b.provider_id {
            pool::adopt_source(c, &s.secrets, id, provider)?;
        }
        reset(c, &names)
    })
    .map_err(|e| {
        ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "provider_url_conflict",
            e.to_string(),
        )
    })?;
    drop(c);
    changed(&s, names);
    Ok(Json(json!({"ok":true})))
}
async fn group_model(
    _root: ManagementAuth,
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
    _root: ManagementAuth,
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
    _root: ManagementAuth,
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
    _root: ManagementAuth,
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

async fn credentials_list(
    _root: ManagementAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    require(&c, id)?;
    Ok(Json(
        json!({"provider":providers::one(&c,id)?,"credentials":pool::credentials(&c,id)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialModel {
    model: String,
    upstream: Option<String>,
    enabled: Option<bool>,
    vision: Option<bool>,
    drop_fields: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewCredential {
    #[serde(default = "yes")]
    inherit_models: bool,
    base_url: Option<String>,
    tools: Option<crate::search_tools::ToolCapabilities>,
    label: String,
    kind: Option<String>,
    key: Option<String>,
    #[serde(default)]
    models: Vec<CredentialModel>,
    #[serde(default)]
    max_concurrent: u32,
    oauth_account_id: Option<i64>,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
}
async fn credentials_create(
    _root: ManagementAuth,
    State(s): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<NewCredential>,
) -> Result<Json<Value>, ApiError> {
    let label = providers::name(&body.label).map_err(ApiError::bad_request)?;
    let kind = body.kind.as_deref().unwrap_or("api_key");
    if !matches!(kind, "api_key" | "oauth" | "anonymous")
        || body
            .key
            .as_ref()
            .is_some_and(|k| k.len() > 65536 || k.trim().is_empty())
        || kind == "api_key" && body.key.is_none()
        || kind != "api_key" && body.key.is_some()
    {
        return Err(ApiError::bad_request("凭据类型或密钥无效"));
    }
    let c = s.conn();
    require(&c, id)?;
    let url: Option<String> = c
        .query_row(
            "SELECT base_url FROM provider_channels WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(db::DbError::from)?;
    let is_local=pool::local(&c,id)?;
    let url = if is_local {
        if kind=="oauth" || body.tools.is_some() { return Err(ApiError::bad_request("本地模型入口只支持API Key或免认证")); }
        pool::canonical_url(body.base_url.as_deref().ok_or_else(||ApiError::bad_request("请填写本地模型入口API URL"))?).map_err(|e|ApiError::bad_request(e.to_string()))?
    } else {
        if body.base_url.is_some() { return Err(ApiError::bad_request("普通供应商只允许渠道设置中的唯一API URL")); }
        url.ok_or_else(|| ApiError::bad_request("请先设置此渠道唯一API URL"))?
    };
    if let Some(tools) = &body.tools {
        tools.validate().map_err(ApiError::bad_request)?;
        if kind == "oauth" || !(tools.search || tools.fetch) {
            return Err(ApiError::bad_request(
                "工具凭据必须明确启用search/fetch，且不能与OAuth混用",
            ));
        }
    }
    let requested_adapter = if body.tools.is_some() {
        "tools"
    } else if kind == "oauth" {
        "oauth"
    } else {
        "api"
    };
    let existing_adapter: String = c
        .query_row(
            "SELECT adapter_kind FROM provider_channels WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(db::DbError::from)?;
    if !pool::credentials(&c, id)?.is_empty() && existing_adapter != requested_adapter {
        return Err(ApiError::bad_request(
            "认证或工具协议语义不同，请创建另一个渠道；工具Key必须显式填写独立account_scope",
        ));
    }

    let mut models = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for m in body.models {
        let name = model(&m.model)?;
        if !seen.insert(tokengo_shared::normalize_model_name(&name)) {
            return Err(ApiError::bad_request("模型不能重复"));
        }
        models.push(channels::ModelMapping {
            model: name,
            upstream: m.upstream.filter(|s| !s.trim().is_empty()),
            enabled: m.enabled.unwrap_or(true),
            vision: m.vision.unwrap_or(false),
            drop_fields: channels::ModelMapping::drop_fields_from_text(
                m.drop_fields.as_deref().unwrap_or(""),
            ),
        });
    }
    // Empty local credentials are intentionally unroutable until models are selected;
    // never inherit declarations from another local port.
    if models.is_empty() && !is_local && body.inherit_models {
        for ch in channels::list(&c)?
            .into_iter()
            .filter(|row| row.provider_id == Some(id))
        {
            for m in ch.models {
                if seen.insert(tokengo_shared::normalize_model_name(&m.model)) {
                    models.push(m);
                }
            }
        }
    }
    if models.len() > 256 {
        return Err(ApiError::bad_request("最多配置256个模型"));
    }
    let account = if kind == "oauth" {
        if url != crate::oauth::API_BASE || models.iter().any(|m| !m.drop_fields.is_empty()) {
            return Err(ApiError::bad_request(
                "OAuth凭据必须使用官方入口且不能丢弃字段",
            ));
        }
        let account = body
            .oauth_account_id
            .ok_or_else(|| ApiError::bad_request("请选择已授权OAuth账号"))?;
        let active:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_accounts WHERE id=?1 AND status='active' AND cipher IS NOT NULL)",[account],|r|r.get(0)).map_err(db::DbError::from)?;
        if !active {
            return Err(ApiError::bad_request("OAuth账号不可用"));
        }
        Some(account)
    } else {
        if body.oauth_account_id.is_some() {
            return Err(ApiError::bad_request("此凭据类型不能绑定OAuth账号"));
        }
        None
    };
    let bounds = if let Some(account) = account {
        let previous:Option<(Option<i64>,Option<i64>)>=c.query_row("SELECT o.max_input_tokens,o.max_output_tokens FROM oauth_channel_bindings o WHERE o.account_id=?1 AND o.max_input_tokens IS NOT NULL AND o.max_output_tokens IS NOT NULL ORDER BY o.channel_id LIMIT 1",[account],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db::DbError::from)?;
        let input = body.max_input_tokens.or(previous.and_then(|p| p.0));
        let output = body.max_output_tokens.or(previous.and_then(|p| p.1));
        if input.is_none_or(|n| !(1..=16_777_216).contains(&n))
            || output.is_none_or(|n| !(1..=16_777_216).contains(&n))
        {
            return Err(ApiError::bad_request("首个OAuth Key需要明确已验证的max_input_tokens/max_output_tokens；不可猜测上下文额度"));
        }
        (input, output)
    } else {
        if body.max_input_tokens.is_some() || body.max_output_tokens.is_some() {
            return Err(ApiError::bad_request("此字段仅用于OAuth凭据"));
        }
        (None, None)
    };
    let existing_credentials = pool::credentials(&c, id)?;
    if existing_credentials
        .iter()
        .any(|k| (k.kind == "oauth") != (kind == "oauth"))
    {
        return Err(ApiError::bad_request(
            "OAuth与API Key入口语义不同，请单独创建渠道",
        ));
    }
    let prior_credentials = existing_credentials
        .into_iter()
        .map(|k| k.id)
        .collect::<std::collections::HashSet<_>>();
    let (source,credential)=db::in_tx(&c,|c|{
  c.execute("UPDATE provider_channels SET adapter_kind=?2 WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM provider_credentials WHERE provider_id=?1)",rusqlite::params![id,requested_adapter])?;
  let source=channels::create(c,&channels::NewChannel{name:label.clone(),base_url:url.clone(),enabled:true,unwrap_envelope:false,max_concurrent:0})?;
  if let Some(key)=&body.key{channels::set_secret(c,&s.secrets,source,key)?;}
  channels::set_models(c,source,&models)?;providers::set_membership(c,source.0,Some(id))?;
  // Inherit declared protocol semantics, not another account's credentials or headers.
  let first:Option<i64>=c.query_row("SELECT channel_id FROM provider_connections WHERE provider_id=?1 AND channel_id<>?2 AND (?3=0 OR channel_id IN (SELECT id FROM channels WHERE base_url=?4)) ORDER BY channel_id LIMIT 1",rusqlite::params![id,source.0,is_local,url],|r|r.get(0)).optional()?;
  if let Some(template)=first{crate::protocol_config::save(c,source.0,&crate::protocol_config::load(c,template)?)?;}
  if let Some(account)=account{
   crate::protocol_config::save(c,source.0,&crate::protocol_config::Capabilities{native:vec![crate::protocol::Protocol::Responses],allow_conversion:true,..Default::default()})?;
   c.execute("INSERT INTO oauth_channel_bindings(channel_id,account_id,max_input_tokens,max_output_tokens) VALUES(?1,?2,?3,?4)",rusqlite::params![source.0,account,bounds.0,bounds.1])?;
  }
  if let Some(tools)=&body.tools {
   let raw=serde_json::to_string(tools).map_err(|_|db::DbError::Other("工具配置无法序列化".into()))?;
   db::settings::set(c,&format!("channel_tools:{}",source.0),&raw)?;
   c.execute("UPDATE channels SET binding_generation=binding_generation+1 WHERE id=?1",[source.0])?;
  }
  let credential=pool::adopt_source(c,&s.secrets,source.0,id)?;
  if prior_credentials.contains(&credential){return Err(db::DbError::Other("此渠道已存在相同凭据，请使用已有Key".into()));}
  c.execute("UPDATE provider_credentials SET label=?2,max_concurrent=?3 WHERE id=?1",rusqlite::params![credential,label,body.max_concurrent])?;
  Ok((source.0,credential))
 }).map_err(|e|ApiError::bad_request(e.to_string()))?;
    let row = pool::credentials(&c, id)?
        .into_iter()
        .find(|k| k.id == credential)
        .ok_or_else(|| ApiError::internal("凭据创建后不可见"))?;
    Ok(Json(json!({"credential":row,"source_id":source})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialPatch {
    base_url: Option<String>,
    label: Option<String>,
    enabled: Option<bool>,
    priority: Option<i64>,
    max_concurrent: Option<u32>,
    key: Option<String>,
    models: Option<Vec<CredentialModel>>,
}
async fn credentials_update(
    _root: ManagementAuth,
    State(s): State<SharedState>,
    Path((provider, id)): Path<(i64, i64)>,
    Json(body): Json<CredentialPatch>,
) -> Result<Json<Value>, ApiError> {
    if let Some(label) = &body.label {
        providers::name(label).map_err(ApiError::bad_request)?;
    }
    if body
        .key
        .as_ref()
        .is_some_and(|k| k.trim().is_empty() || k.len() > 65536)
    {
        return Err(ApiError::bad_request("新密钥无效"));
    }
    let c = s.conn();
    require(&c, provider)?;
    let original = pool::credentials(&c, provider)?
        .into_iter()
        .find(|k| k.id == id)
        .ok_or_else(|| ApiError::not_found("凭据不存在"))?;
    if body.key.is_some() && original.kind != "api_key" {
        return Err(ApiError::bad_request(
            "此凭据通过专用授权流程轮换，不能写入API Key",
        ));
    }
    if let Some(models) = &body.models {
        let mut seen = std::collections::HashSet::new();
        if models.len() > 256 {
            return Err(ApiError::bad_request("最多配置256个模型"));
        }
        for m in models {
            let name = model(&m.model)?;
            if !seen.insert(tokengo_shared::normalize_model_name(&name)) {
                return Err(ApiError::bad_request("模型不能重复"));
            }
            if original
                .models
                .iter()
                .any(|old| tokengo_shared::same_model_name(&old.model, &name) && old.conflict)
            {
                return Err(ApiError::new(
                    axum::http::StatusCode::CONFLICT,
                    "credential_mapping_conflict",
                    "此Key的同名旧映射存在差异，不能静默覆盖；请先分别确认来源配置",
                ));
            }
        }
    }
    if let Some(url)=&body.base_url {
        if !pool::local(&c,provider)? { return Err(ApiError::bad_request("普通供应商请在渠道设置修改唯一API URL")); }
        pool::canonical_url(url).map_err(|e|ApiError::bad_request(e.to_string()))?;
    }
    let names=if body.base_url.is_some() {original.models.iter().map(|m|m.model.clone()).collect::<Vec<_>>()}else{vec![]};
    db::in_tx(&c, |c| {
        if let Some(url)=&body.base_url { pool::update_local_url(c,&s.secrets,provider,id,url)?; }
        if let Some(key) = &body.key {
            pool::rotate_secret(c, &s.secrets, id, key)?;
        }
        if let Some(models) = &body.models {
            for m in models {
                let mut targets = original
                    .source_ids
                    .iter()
                    .filter_map(|id| channels::get(c, ChannelId(*id)).transpose())
                    .collect::<db::Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|s| {
                        s.models
                            .iter()
                            .any(|p| tokengo_shared::same_model_name(&p.model, &m.model))
                    })
                    .collect::<Vec<_>>();
                if targets.is_empty() {
                    let id = *original
                        .source_ids
                        .first()
                        .ok_or_else(|| db::DbError::Other("凭据没有来源".into()))?;
                    targets.push(
                        channels::get(c, ChannelId(id))?
                            .ok_or_else(|| db::DbError::Other("来源不存在".into()))?,
                    );
                }
                for source in targets {
                    let mut mappings = source.models;
                    let idx = if let Some(i) = mappings
                        .iter()
                        .position(|p| tokengo_shared::same_model_name(&p.model, &m.model))
                    {
                        i
                    } else {
                        mappings.push(channels::ModelMapping::new(&m.model, None, false));
                        mappings.len() - 1
                    };
                    let target = &mut mappings[idx];
                    if let Some(upstream) = &m.upstream {
                        target.upstream =
                            (!upstream.trim().is_empty()).then(|| upstream.trim().to_owned());
                    }
                    if let Some(enabled) = m.enabled {
                        target.enabled = enabled;
                    }
                    if let Some(vision) = m.vision {
                        target.vision = vision;
                    }
                    if let Some(fields) = &m.drop_fields {
                        target.drop_fields = channels::ModelMapping::drop_fields_from_text(fields);
                    }
                    channels::set_models(c, source.id, &mappings)?;
                }
            }
        }
        c.execute("UPDATE provider_credentials SET label=COALESCE(?2,label),enabled=COALESCE(?3,enabled),priority=COALESCE(?4,priority),max_concurrent=COALESCE(?5,max_concurrent) WHERE id=?1",rusqlite::params![id,body.label,body.enabled,body.priority,body.max_concurrent])?;
        reset(c,&names)
    }).map_err(|e|ApiError::bad_request(e.to_string()))?;
    let row = pool::credentials(&c, provider)?
        .into_iter()
        .find(|k| k.id == id)
        .ok_or_else(|| ApiError::internal("凭据不可见"))?;
    drop(c);
    changed(&s,names);
    Ok(Json(json!({"credential":row})))
}
async fn credentials_disable(
    _root: ManagementAuth,
    State(s): State<SharedState>,
    Path((provider, id)): Path<(i64, i64)>,
) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    require(&c, provider)?;
    let row = pool::credentials(&c, provider)?
        .into_iter()
        .find(|k| k.id == id)
        .ok_or_else(|| ApiError::not_found("凭据不存在"))?;
    c.execute(
        "UPDATE provider_credentials SET enabled=0 WHERE id=?1",
        [row.id],
    )
    .map_err(db::DbError::from)?;
    let row = pool::credentials(&c, provider)?
        .into_iter()
        .find(|k| k.id == id)
        .unwrap();
    Ok(Json(json!({"credential":row,"preserved":true})))
}


/// Catalog discovery uses exactly one selected credential and endpoint; it never
/// saves draft credentials, model mappings, routing, billing or permissions.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverModels {
    credential_id: Option<i64>,
    key: Option<String>,
    kind: Option<String>,
    base_url: Option<String>,
}
async fn credential_catalog(
    _root: ManagementAuth,
    State(s): State<SharedState>,
    Path(provider): Path<i64>,
    body: Result<Json<DiscoverModels>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) = body.map_err(|_| ApiError::bad_request("模型目录请求格式无效"))?;
    // Everything needed for upstream auth is copied while holding the DB guard;
    // never hold that guard across an awaited HTTP request.
    let (channel, secret, oauth_account, snapshot, credential_revision) = {
        let c = s.conn();
        require(&c, provider)?;
        let is_local = pool::local(&c, provider)?;
        if let Some(id) = body.credential_id {
            if body.key.is_some() || body.kind.is_some() {
                return Err(ApiError::bad_request(
                    "已保存Key请只选择凭据，不要同时提交新密钥或类型",
                ));
            }
            let credential = pool::credentials(&c, provider)?
                .into_iter()
                .find(|k| k.id == id)
                .ok_or_else(|| ApiError::not_found("此渠道不存在该凭据"))?;
            let requested = body
                .base_url
                .as_deref()
                .map(pool::canonical_url)
                .transpose()
                .map_err(|_| ApiError::bad_request("API URL无效"))?;
            let mut sources = credential
                .source_ids
                .iter()
                .map(|id| connection(&c, *id))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(url) = requested {
                sources.retain(|row| pool::canonical_url(&row.base_url).is_ok_and(|u| u == url));
            } else if credential.base_urls.len() != 1 {
                return Err(ApiError::bad_request("请先选择此凭据的API URL"));
            }
            let source = sources
                .into_iter()
                .next()
                .ok_or_else(|| ApiError::bad_request("API URL不属于此凭据"))?;
            let fingerprint = crate::reasoning_capabilities::fingerprint(&c, source.id.0)?;
            let secret = channels::secret(&c, &s.secrets, source.id)?;
            let headers = channels::headers(&c, &s.secrets, source.id)?;
            let account = if credential.kind == "oauth" {
                let account = credential.oauth_account_id.ok_or_else(|| {
                    ApiError::bad_request("OAuth凭据尚未绑定账号，请先授权或手工输入模型")
                })?;
                let binding: Option<i64> = c
                    .query_row(
                        "SELECT account_id FROM oauth_channel_bindings WHERE channel_id=?1",
                        [source.id.0],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(db::DbError::from)?;
                if binding != Some(account) || source.base_url != crate::oauth::API_BASE {
                    return Err(ApiError::bad_request(
                        "OAuth凭据绑定或官方地址已改变，请重新确认账号配置",
                    ));
                }
                Some(account)
            } else {
                None
            };
            (
                tokengo_shared::Channel {
                    id: source.id,
                    name: source.name,
                    base_url: pool::canonical_url(&source.base_url)
                        .map_err(|_| ApiError::bad_request("已存API URL无效"))?,
                    enabled: true,
                    priority: 0,
                    model_alias: None,
                    vision: false,
                    drop_fields: vec![],
                    unwrap_envelope: false,
                    headers,
                    max_concurrent: 0,
                },
                secret,
                account,
                Some(fingerprint),
                Some((id, credential.revision)),
            )
        } else {
            let kind = body.kind.as_deref().unwrap_or("api_key");
            if kind == "oauth" {
                return Err(ApiError::bad_request(
                    "请先绑定已授权OAuth账号，再读取该账号的官方模型目录；也可以手工输入",
                ));
            }
            if !matches!(kind, "api_key" | "anonymous")
                || (kind == "api_key"
                    && body
                        .key
                        .as_ref()
                        .is_none_or(|v| v.trim().is_empty() || v.len() > 65536))
                || (kind == "anonymous" && body.key.is_some())
            {
                return Err(ApiError::bad_request("请填写有效的API Key，或选择免认证"));
            }
            let configured: Option<String> = c
                .query_row(
                    "SELECT base_url FROM provider_channels WHERE id=?1",
                    [provider],
                    |r| r.get(0),
                )
                .map_err(db::DbError::from)?;
            let url = if is_local {
                body.base_url
                    .as_deref()
                    .ok_or_else(|| ApiError::bad_request("请填写此本地模型入口的API URL"))?
            } else {
                let configured = configured
                    .as_deref()
                    .ok_or_else(|| ApiError::bad_request("请先设置渠道API URL"))?;
                if body.base_url.as_deref().is_some_and(|u| {
                    pool::canonical_url(u).ok() != pool::canonical_url(configured).ok()
                }) {
                    return Err(ApiError::bad_request("普通渠道只能探测渠道设置中的API URL"));
                }
                configured
            };
            let url = pool::canonical_url(url).map_err(|_| {
                ApiError::bad_request("API URL无效：仅允许无凭据及查询参数的HTTP(S)地址")
            })?;
            // Draft keys do not borrow custom auth headers from another key.
            (
                tokengo_shared::Channel {
                    id: ChannelId(0),
                    name: "模型目录".into(),
                    base_url: url,
                    enabled: true,
                    priority: 0,
                    model_alias: None,
                    vision: false,
                    drop_fields: vec![],
                    unwrap_envelope: false,
                    headers: vec![],
                    max_concurrent: 0,
                },
                body.key,
                None,
                None,
                None,
            )
        }
    };
    let (status, raw) = if let Some(account) = oauth_account {
        let value = crate::oauth::models(&s, account).await.map_err(|_| {
            ApiError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "model_catalog_unavailable",
                "此OAuth账号的官方模型目录暂不可用，请稍后重试或手工输入模型",
            )
        })?;
        (200, value.to_string())
    } else {
        let timeout = std::time::Duration::from_millis(s.routing().cfg.attempt_timeout_ms.max(1))
            .min(std::time::Duration::from_secs(15));
        s.upstream
            .fetch_models(
                &channel,
                secret.as_deref(),
                timeout,
                &crate::upstream::new_uuid(),
                db::now_ms(),
            )
            .await
            .map_err(|_| {
                ApiError::new(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "model_catalog_unavailable",
                    "无法读取此Key的模型目录，请检查地址和认证，或手工输入模型",
                )
            })?
    };
    if !(200..300).contains(&status) {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "model_catalog_upstream_error",
            format!("此Key的模型目录返回HTTP {status}，请检查权限或手工输入模型"),
        ));
    }
    if let Some((id, revision)) = credential_revision {
        let c = s.conn();
        let current = pool::credentials(&c, provider)?
            .into_iter()
            .find(|k| k.id == id);
        if current.is_none_or(|k| k.revision != revision || !k.source_ids.contains(&channel.id.0))
            || snapshot.as_ref().is_none_or(|before| {
                crate::reasoning_capabilities::fingerprint(&c, channel.id.0)
                    .ok()
                    .as_ref()
                    != Some(before)
            })
        {
            return Err(ApiError::new(
                axum::http::StatusCode::CONFLICT,
                "credential_changed",
                "读取期间凭据或地址已改变，请重新探测",
            ));
        }
    }
    let (catalog, total) = crate::upstream::parse_model_catalog_with_total(&raw);
    if catalog.is_empty() && !crate::upstream::looks_like_a_model_list(&raw) {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "model_catalog_invalid",
            "此入口未返回可识别的模型目录，请手工输入上游模型名",
        ));
    }
    // Return only names, never upstream bodies/metadata, and reject credential
    // echoes even if an upstream embeds one inside an apparently valid model ID.
    let forbidden = secret
        .iter()
        .map(String::as_str)
        .chain(channel.headers.iter().map(|(_, v)| v.as_str()))
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    let mut models = catalog.into_iter().map(|m| m.id).collect::<Vec<_>>();
    if models
        .iter()
        .any(|m| forbidden.iter().any(|s| m.contains(s)))
    {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "model_catalog_invalid",
            "上游返回了不安全的模型目录，请手工输入模型",
        ));
    }
    models.sort();
    models.dedup();
    let count = models.len();
    Ok(Json(
        json!({"models":models,"count":count,"http_status":status,"truncated":total>count}),
    ))
}


/// Explicit reveal is root-only and POST-protected by the panel's existing CSRF
/// contract. Neither list endpoints nor OAuth tokens are exposed by this path.
async fn credentials_reveal(
    _root: PanelAuth,
    State(s): State<SharedState>,
    Path((provider, id)): Path<(i64, i64)>,
) -> Result<Json<Value>, ApiError> {
    let c = s.conn();
    require(&c, provider)?;
    let credential = pool::credentials(&c, provider)?.into_iter().find(|k|k.id == id)
        .ok_or_else(||ApiError::not_found("此渠道不存在该上游Key"))?;
    if credential.kind != "api_key" {
        return Err(ApiError::bad_request(if credential.kind == "oauth" {
            "OAuth凭据仅显示账号授权状态，不提供访问Token或刷新Token明文"
        } else { "免认证入口没有可显示的API Key" }));
    }
    let mut secrets = Vec::new();
    for source in credential.source_ids {
        let row = channels::get(&c,ChannelId(source))?.ok_or_else(||ApiError::not_found("该Key的来源不存在"))?;
        if row.provider_id != Some(provider) {
            return Err(ApiError::new(axum::http::StatusCode::CONFLICT,"credential_source_conflict","此Key的来源归属不一致，请检查渠道配置"));
        }
        let oauth:bool = c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings WHERE channel_id=?1)",[source],|r|r.get(0)).map_err(db::DbError::from)?;
        if oauth {
            return Err(ApiError::new(axum::http::StatusCode::CONFLICT,"credential_kind_conflict","此凭据存在OAuth绑定，仅允许查看账号授权状态"));
        }
        let secret = channels::secret(&c,&s.secrets,row.id).map_err(|_|ApiError::internal("无法解密此上游Key"))?;
        secrets.push(secret.filter(|value| !value.trim().is_empty()));
    }
    if secrets.is_empty() || secrets.iter().all(Option::is_none) {
        return Err(ApiError::not_found("此上游Key没有已保存的密钥，请检查原始认证配置"));
    }
    if secrets.iter().any(|value|value != &secrets[0]) {
        return Err(ApiError::new(axum::http::StatusCode::CONFLICT,"credential_secret_conflict","此Key的多个来源保存了不同密钥，请分别确认原始配置"));
    }
    let key = secrets.remove(0).ok_or_else(||ApiError::not_found("此上游Key没有已保存的密钥"))?;
    Ok(Json(json!({"key":key})))
}
async fn secret_no_store(mut response: axum::response::Response) -> axum::response::Response {
    response.headers_mut().insert(axum::http::header::CACHE_CONTROL,axum::http::HeaderValue::from_static("no-store"));
    response.headers_mut().insert(axum::http::header::PRAGMA,axum::http::HeaderValue::from_static("no-cache"));
    response
}
