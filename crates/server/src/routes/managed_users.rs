//! One product user: a stable entitlement owner plus private login credentials.
//! Existing identity, key ancestry, pricing and financial ledger IDs stay intact.
use super::{
    ApiError,
    accounts::{self, Browser},
    operations::ManagementGrant,
};
use crate::{
    SharedState,
    db::{
        self, accounts as credentials, billing,
        identity::{self, Policy},
        model_routes,
    },
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    routing::{get, patch, post},
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

const MAX_ACTIVE_KEYS: usize = 32;
fn invalid(e: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(e.to_string())
}
fn forbidden() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::FORBIDDEN,
        "policy_broadening",
        "只能在本人授权和现有密钥限制内缩小权限、额度或有效期",
    )
}
fn dbbad(e: impl std::fmt::Display) -> db::DbError {
    db::DbError::Other(e.to_string())
}
fn nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<i64>>, D::Error> {
    Option::<i64>::deserialize(d).map(Some)
}
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/managed-users", get(list).post(create))
        .route("/api/managed-users/{id}", patch(update).delete(delete))
        .route("/api/managed-users/{id}/login-credential", post(add_login))
        .route("/api/account/keys", get(own_keys).post(own_create))
        .route(
            "/api/account/keys/{id}",
            patch(own_update).delete(own_revoke),
        )
        .layer(DefaultBodyLimit::max(65536))
        .layer(axum::middleware::map_response(accounts::json_errors))
}
fn require_root(root: &ManagementGrant, c: &Connection) -> Result<(), ApiError> {
    if root.valid(c)? {
        Ok(())
    } else {
        Err(ApiError::unauthorized("登录已失效"))
    }
}
fn owner(browser: &Browser, c: &Connection) -> Result<identity::User, ApiError> {
    let id = browser.owner(c)?.ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "user_required",
            "此操作只供普通用户使用",
        )
    })?;
    identity::user(c, id)?
        .filter(|u| u.enabled)
        .ok_or_else(|| ApiError::unauthorized("用户不可用"))
}
fn budget(c: &Connection, scope: &str, id: i64) -> db::Result<Option<billing::Budget>> {
    Ok(billing::budgets(c, Some(scope), Some(id))?
        .into_iter()
        .next())
}
fn user_view(c: &Connection, u: &identity::User) -> db::Result<Value> {
    let mut value = serde_json::to_value(u).map_err(dbbad)?;
    value["accounts"] = json!(
        credentials::accounts(c)?
            .into_iter()
            .filter(|a| a.user_id == Some(u.id))
            .collect::<Vec<_>>()
    );
    value["budget"] = json!(budget(c, "user", u.id)?);
    Ok(value)
}
fn key_view(c: &Connection, k: &identity::VKey) -> db::Result<Value> {
    let mut value = serde_json::to_value(k).map_err(dbbad)?;
    value["budget"] = json!(budget(c, "key", k.id)?);
    let paused=identity::self_paused(c,k)?;
    let user_valid=identity::user(c,k.user_id)?.is_some_and(|u|u.enabled);
    let parent_valid=if let Some(parent)=k.parent_key_id { identity::context(c,parent)?.is_some_and(|a|a.user_id==k.user_id) } else { true };
    let capacity=k.enabled || identity::keys(c,k.user_id)?.iter().filter(|key|key.enabled && key.revoked_at.is_none() && key.expires_at.is_none_or(|t|t>db::now_ms())).count()<MAX_ACTIVE_KEYS;
    value["self_paused"]=json!(paused);
    value["can_enable"]=json!(user_valid && parent_valid && capacity && k.revoked_at.is_none() && k.expires_at.is_none_or(|t|t>db::now_ms()) && (k.enabled || paused));
    Ok(value)
}
// Called only inside the caller's atomic user/key write. Preserve all counters.
// Do not call billing::set_budget here: it would open a nested transaction.
fn set_budget(
    c: &Connection,
    scope: &str,
    id: i64,
    limit: Option<i64>,
    actor: &str,
) -> db::Result<()> {
    if limit.is_some_and(|n| n < 0) {
        return Err(dbbad("费用上限不能为负数"));
    }
    c.execute("INSERT INTO billing_budgets(scope,scope_id,limit_micro) VALUES(?1,?2,?3) ON CONFLICT(scope,scope_id) DO UPDATE SET limit_micro=excluded.limit_micro", params![scope,id,limit])?;
    c.execute("INSERT INTO billing_audit(event,actor,object,detail,created_ms) VALUES('budget_updated',?1,?2,?3,?4)",params![actor,format!("{scope}:{id}"),json!({"limit_micro":limit}).to_string(),db::now_ms()])?;
    Ok(())
}
fn audit(c: &Connection, actor: i64, event: &str, kind: &str, id: i64) -> db::Result<()> {
    c.execute("INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json) VALUES(?1,?2,?3,?4,?5)", params![db::now_ms(),event,kind,id,json!({"actor_account_id":actor}).to_string()])?;
    Ok(())
}
fn full(c: &Connection) -> db::Result<()> {
    c.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}
async fn list(root: ManagementGrant, State(state): State<SharedState>) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    require_root(&root, &c)?;
    let users = identity::users(&c)?
        .iter()
        .map(|u| user_view(&c, u))
        .collect::<db::Result<Vec<_>>>()?;
    Ok(Json(
        json!({"users":users,"root_accounts":credentials::accounts(&c)?.into_iter().filter(|a|a.role=="root").collect::<Vec<_>>()}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewUser {
    username: String,
    password: String,
    display_name: String,
    #[serde(default)]
    policy: Policy,
    #[serde(default)]
    limit_micro: Option<i64>,
}
async fn create(
    root: ManagementGrant,
    State(state): State<SharedState>,
    Json(body): Json<NewUser>,
) -> Result<Json<Value>, ApiError> {
    let name = credentials::username(&body.username).map_err(invalid)?;
    credentials::password(&body.password).map_err(invalid)?;
    credentials::display_name(&body.display_name).map_err(invalid)?;
    body.policy.validate().map_err(invalid)?;
    if matches!(name.as_str(), "admin" | "root")
        || body.policy.admin_access
        || body.limit_micro.is_some_and(|n| n < 0)
    {
        return Err(invalid("普通用户资料或费用上限无效"));
    }
    let hash = accounts::hash(&state, body.password).await?;
    let c = state.conn();
    require_root(&root, &c)?;
    full(&c)?;
    let value = credentials::atomic(&c, || {
        let u = identity::create_user(&c, &body.display_name, "user", &body.policy)?;
        credentials::create(&c, &name, &body.display_name, u.id, &hash)?;
        if body.limit_micro.is_some() {
            set_budget(&c, "user", u.id, body.limit_micro, "root")?;
        }
        audit(
            &c,
            root.actor_account_id(),
            "managed_user.create",
            "user",
            u.id,
        )?;
        user_view(&c, &u)
    })
    .map_err(invalid)?;
    Ok(Json(json!({"user":value})))
}
async fn delete(root: ManagementGrant, State(state): State<SharedState>, Path(id): Path<i64>) -> Result<Json<Value>,ApiError> {
    let c=state.conn();
    require_root(&root,&c)?;
    identity::user(&c,id)?.ok_or_else(||ApiError::not_found("用户不存在"))?;
    if id==1 { return Err(ApiError::new(axum::http::StatusCode::FORBIDDEN,"protected_user","管理员身份不能删除，请在账号安全中管理")); }
    full(&c)?;
    credentials::atomic(&c,||db::lifecycle::archive_user(&c,id,root.actor_account_id()))
        .map_err(|e|ApiError::new(axum::http::StatusCode::CONFLICT,"user_delete_blocked",e.to_string()))?;
    Ok(Json(json!({"deleted":true,"archived":true,"history_preserved":true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserPatch {
    display_name: Option<String>,
    enabled: Option<bool>,
    policy: Option<Policy>,
    password: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    limit_micro: Option<Option<i64>>,
}
async fn update(
    root: ManagementGrant,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<UserPatch>,
) -> Result<Json<Value>, ApiError> {
    if let Some(v) = &body.display_name {
        credentials::display_name(v).map_err(invalid)?;
    }
    if let Some(v) = &body.policy {
        v.validate().map_err(invalid)?;
    }
    if body.limit_micro.flatten().is_some_and(|n| n < 0) {
        return Err(invalid("费用上限不能为负数"));
    }
    let hash = if let Some(p) = body.password {
        credentials::password(&p).map_err(invalid)?;
        Some(accounts::hash(&state, p).await?)
    } else {
        None
    };
    let c = state.conn();
    require_root(&root, &c)?;
    full(&c)?;
    let mut u = identity::user(&c, id)?.ok_or_else(|| ApiError::not_found("用户不存在"))?;
    let linked = credentials::accounts(&c)?
        .into_iter()
        .filter(|a| a.user_id == Some(id))
        .collect::<Vec<_>>();
    if hash.is_some() && linked.is_empty() {
        return Err(invalid("此用户尚未启用登录，请先设置登录账号"));
    }
    if hash.is_some() && linked.len() > 1 {
        return Err(invalid(
            "此用户有多个旧版登录凭证，请分别管理密码，不能批量重置",
        ));
    }
    if let Some(v) = &body.display_name {
        u.name = v.trim().to_owned();
    }
    if let Some(v) = body.enabled {
        u.enabled = v;
    }
    if let Some(v) = body.policy {
        u.policy = v;
    }
    let value=credentials::atomic(&c,||{
        identity::update_user(&c,&u)?;
        for a in &linked{
            if let Some(v)=&body.display_name{c.execute("UPDATE panel_accounts SET display_name=?1 WHERE id=?2",params![v.trim(),a.id])?;}
            if let Some(v)=body.enabled{c.execute("UPDATE panel_accounts SET enabled=?1 WHERE id=?2",params![v,a.id])?;}
            if let Some(v)=&hash{c.execute("UPDATE panel_accounts SET password_hash=?1 WHERE id=?2",params![v,a.id])?;}
        }
        if let Some(limit)=body.limit_micro{set_budget(&c,"user",id,limit,"root")?;}
        c.execute("INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json) VALUES(?1,'managed_user.update','user',?2,?3)",params![db::now_ms(),id,json!({"actor_account_id":root.actor_account_id()}).to_string()])?;
        user_view(&c,&u)
    }).map_err(invalid)?;
    Ok(Json(json!({"user":value})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginCredential {
    username: String,
    password: String,
    display_name: Option<String>,
}
async fn add_login(
    root: ManagementGrant,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<LoginCredential>,
) -> Result<Json<Value>, ApiError> {
    let name = credentials::username(&body.username).map_err(invalid)?;
    credentials::password(&body.password).map_err(invalid)?;
    if matches!(name.as_str(), "admin" | "root") {
        return Err(invalid("此登录名保留给管理员"));
    }
    if let Some(v) = &body.display_name {
        credentials::display_name(v).map_err(invalid)?;
    }
    let hash = accounts::hash(&state, body.password).await?;
    let c = state.conn();
    require_root(&root, &c)?;
    let u = identity::user(&c, id)?.ok_or_else(|| ApiError::not_found("用户不存在"))?;
    if credentials::accounts(&c)?
        .iter()
        .any(|a| a.user_id == Some(id))
    {
        return Err(invalid("该用户已有登录凭证，不能重复关联"));
    }
    let value = credentials::atomic(&c, || {
        let a = credentials::create(
            &c,
            &name,
            body.display_name.as_deref().unwrap_or(&u.name),
            id,
            &hash,
        )?;
        audit(
            &c,
            root.actor_account_id(),
            "managed_user.login_enabled",
            "panel_account",
            a.id,
        )?;
        user_view(&c, &u)
    })
    .map_err(invalid)?;
    Ok(Json(json!({"user":value})))
}
fn safe_policy(u: &identity::User) -> Policy {
    let mut p = u.policy.clone();
    p.admin_access = false;
    p
}
async fn own_keys(
    browser: Browser,
    State(state): State<SharedState>,
) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    let u = owner(&browser, &c)?;
    let policy = safe_policy(&u);
    let auth = identity::AuthContext {
        user_id: u.id,
        key_id: 0,
        role: "user".into(),
        user_policy: policy.clone(),
        key_policy: Policy::default(),
        ancestor_key_ids: vec![],
        expires_at: None,
        policy_version: String::new(),
    };
    let mut models = Vec::new();
    for m in model_routes::servable(&c)? {
        if model_routes::visible(&c, &m.name)?
            && crate::gateway::permitted_model(&c, &auth, &m.name)?
        {
            models.push(m.name);
        }
    }
    if auth.permits_model(model_routes::AUTO_NAME) && model_routes::auto_available(&c)? {
        let mut eligible = false;
        for kind in [
            tokengo_shared::ModelKind::Chat,
            tokengo_shared::ModelKind::Embeddings,
            tokengo_shared::ModelKind::Rerank,
        ] {
            if model_routes::auto_candidates(&c, kind, false, db::now_ms())?
                .iter()
                .any(|name| {
                    models
                        .iter()
                        .any(|m| tokengo_shared::same_model_name(m, name))
                })
            {
                eligible = true;
            }
        }
        if eligible {
            models.insert(0, model_routes::AUTO_NAME.to_owned());
        }
    }
    let keys = identity::keys(&c, u.id)?
        .iter()
        .map(|k| key_view(&c, k))
        .collect::<db::Result<Vec<_>>>()?;
    Ok(Json(
        json!({"keys":keys,"policy":policy,"budget":budget(&c,"user",u.id)?,"models":models,"max_active_keys":MAX_ACTIVE_KEYS}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewKey {
    name: String,
    policy: Option<Policy>,
    expires_at: Option<i64>,
    #[serde(default, deserialize_with = "nullable")]
    limit_micro: Option<Option<i64>>,
}
fn check_limit(
    c: &Connection,
    u: &identity::User,
    old: Option<&identity::VKey>,
    limit: Option<i64>,
) -> Result<(), ApiError> {
    if limit.is_some_and(|n| n < 0) {
        return Err(invalid("费用上限不能为负数"));
    }
    let mut caps = vec![budget(c, "user", u.id)?.and_then(|b| b.limit_micro)];
    if let Some(k) = old {
        caps.push(budget(c, "key", k.id)?.and_then(|b| b.limit_micro));
        let mut parent = k.parent_key_id;
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = parent {
            if !seen.insert(id) || seen.len() > 32 {
                return Err(forbidden());
            }
            let p = identity::key(c, id)?
                .filter(|k| k.user_id == u.id)
                .ok_or_else(forbidden)?;
            caps.push(budget(c, "key", id)?.and_then(|b| b.limit_micro));
            parent = p.parent_key_id;
        }
    }
    if caps
        .into_iter()
        .flatten()
        .any(|cap| limit.is_none_or(|n| n > cap))
    {
        return Err(forbidden());
    }
    Ok(())
}
async fn own_create(
    browser: Browser,
    State(state): State<SharedState>,
    Json(body): Json<NewKey>,
) -> Result<Json<Value>, ApiError> {
    identity::validate_name(&body.name).map_err(invalid)?;
    let c = state.conn();
    let u = owner(&browser, &c)?;
    full(&c)?;
    let p = body.policy.unwrap_or_else(|| safe_policy(&u));
    p.validate().map_err(invalid)?;
    if p.admin_access || !p.no_broader_than(&safe_policy(&u)) {
        return Err(forbidden());
    }
    let limit = body
        .limit_micro
        .unwrap_or(budget(&c, "user", u.id)?.and_then(|b| b.limit_micro));
    check_limit(&c, &u, None, limit)?;
    let value = credentials::atomic(&c, || {
        let now = db::now_ms();
        let active = identity::keys(&c, u.id)?
            .iter()
            .filter(|k| k.enabled && k.revoked_at.is_none() && k.expires_at.is_none_or(|t| t > now))
            .count();
        if active >= MAX_ACTIVE_KEYS {
            return Err(dbbad("活动密钥已达32把，请先停用或撤销旧密钥"));
        }
        let (key, secret) = identity::create_key(&c, u.id, &body.name, &p, body.expires_at, None)?;
        if limit.is_some() {
            set_budget(
                &c,
                "key",
                key.id,
                limit,
                &format!("account:{}", browser.credential.account.id),
            )?;
        }
        audit(
            &c,
            browser.credential.account.id,
            "account.key_create",
            "gateway_key",
            key.id,
        )?;
        Ok(json!({"key":key_view(&c,&key)?,"secret":secret}))
    })
    .map_err(invalid)?;
    Ok(Json(value))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyPatch {
    name: Option<String>,
    policy: Option<Policy>,
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "nullable")]
    expires_at: Option<Option<i64>>,
    #[serde(default, deserialize_with = "nullable")]
    limit_micro: Option<Option<i64>>,
    #[serde(default)]
    revoke: bool,
}
async fn own_update(
    browser: Browser,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<KeyPatch>,
) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    let u = owner(&browser, &c)?;
    full(&c)?;
    let mut row = identity::key(&c, id)?
        .filter(|k| k.user_id == u.id)
        .ok_or_else(|| ApiError::not_found("密钥不存在"))?;
    let was_enabled=row.enabled;
    let was_self_paused=identity::self_paused(&c,&row)?;
    if row.revoked_at.is_some() {
        return Err(forbidden());
    }
    if body.enabled==Some(true) {
        if (!row.enabled && !was_self_paused) || row.expires_at.is_some_and(|t|t<=db::now_ms()) { return Err(forbidden()); }
        if let Some(parent)=row.parent_key_id {
            if identity::context(&c,parent)?.is_none_or(|a|a.user_id!=u.id) { return Err(forbidden()); }
        }
        let active=identity::keys(&c,u.id)?.iter().filter(|k|k.enabled && k.revoked_at.is_none() && k.expires_at.is_none_or(|t|t>db::now_ms())).count();
        if !row.enabled && active>=MAX_ACTIVE_KEYS { return Err(invalid("活动密钥已达32把，请先停用或撤销其他密钥")); }
        row.enabled=true;
    }
    if let Some(p) = body.policy {
        p.validate().map_err(invalid)?;
        if p.admin_access || !p.no_broader_than(&row.policy) || !p.no_broader_than(&safe_policy(&u))
        {
            return Err(forbidden());
        }
        if let Some(parent) = row.parent_key_id {
            let auth = identity::context(&c, parent)?.ok_or_else(forbidden)?;
            if auth.user_id != u.id || !p.no_broader_than(&auth.effective_policy()) {
                return Err(forbidden());
            }
        }
        row.policy = p;
    }
    if let Some(v) = body.name {
        identity::validate_name(&v).map_err(invalid)?;
        row.name = v;
    }
    if let Some(expiry) = body.expires_at {
        if expiry.is_some_and(|v| v <= db::now_ms())
            || row
                .expires_at
                .is_some_and(|cap| expiry.is_none_or(|v| v > cap))
        {
            return Err(forbidden());
        }
        if let Some(parent) = row.parent_key_id {
            let a = identity::context(&c, parent)?.ok_or_else(forbidden)?;
            if a.expires_at
                .is_some_and(|cap| expiry.is_none_or(|v| v > cap))
            {
                return Err(forbidden());
            }
        }
        row.expires_at = expiry;
    }
    if let Some(limit) = body.limit_micro {
        check_limit(&c, &u, Some(&row), limit)?;
    }
    if body.enabled == Some(false) {
        row.enabled = false;
    }
    if body.revoke {
        row.enabled = false;
        row.revoked_at = Some(db::now_ms());
    }
    let value = credentials::atomic(&c, || {
        identity::update_key(&c, &row)?;
        if body.enabled==Some(false) && was_enabled && !body.revoke {
            db::settings::set(&c,&format!("self_paused_key:{id}"),&u.id.to_string())?;
        }
        if body.revoke { identity::clear_self_pause(&c,id)?; }
        if let Some(limit) = body.limit_micro {
            set_budget(
                &c,
                "key",
                id,
                limit,
                &format!("account:{}", browser.credential.account.id),
            )?;
        }
        audit(
            &c,
            browser.credential.account.id,
            "account.key_update",
            "gateway_key",
            id,
        )?;
        key_view(&c, &row)
    })
    .map_err(invalid)?;
    Ok(Json(json!({"key":value})))
}
async fn own_revoke(
    browser: Browser,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    let u = owner(&browser, &c)?;
    let row = identity::key(&c, id)?
        .filter(|k| k.user_id == u.id)
        .ok_or_else(|| ApiError::not_found("密钥不存在"))?;
    let value = credentials::atomic(&c, || {
        identity::revoke(&c, id)?;
        audit(
            &c,
            browser.credential.account.id,
            "account.key_revoke",
            "gateway_key",
            id,
        )?;
        key_view(&c, &identity::key(&c, row.id)?.unwrap())
    })
    .map_err(invalid)?;
    Ok(Json(json!({"key":value})))
}
