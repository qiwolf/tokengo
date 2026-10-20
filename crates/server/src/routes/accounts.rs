//! Browser-only account login. API roles are never panel-root credentials.
use super::ApiError;
use crate::{
    db::{
        self,
        accounts::{self, Credential},
    },
    SharedState,
};
use axum::{
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Mutex};
pub const COOKIE: &str = "tokengo_account_session";
fn denied() -> ApiError {
    ApiError::unauthorized("账号或密码错误，或账号不可用")
}
fn forbidden() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "forbidden", "没有权限执行此操作")
}
fn bad(e: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(e.to_string())
}
#[derive(Default)]
struct Window {
    start: i64,
    count: u32,
}
#[derive(Default)]
pub struct LoginGate {
    inner: Mutex<(Window, HashMap<String, Window>)>,
}
impl LoginGate {
    pub fn check(&self, name: &str) -> Result<(), ApiError> {
        let now = db::now_ms();
        let mut lock = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if now - lock.0.start >= 300000 {
            lock.0 = Window {
                start: now,
                count: 0,
            };
        }
        lock.1.retain(|_, w| now - w.start < 300000);
        if lock.0.count >= 120 || lock.1.len() >= 4096 {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "login_throttled",
                "登录尝试过多，请稍后重试",
            ));
        }
        let key = accounts::digest(name);
        let w = lock.1.entry(key).or_insert(Window {
            start: now,
            count: 0,
        });
        if w.count >= 10 {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "login_throttled",
                "登录尝试过多，请稍后重试",
            ));
        }
        w.count += 1;
        lock.0.count += 1;
        Ok(())
    }
    pub(crate) fn success(&self, name: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .remove(&accounts::digest(name));
    }
}
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/account/login", post(login))
        .route("/api/account/setup", post(setup))
        .route("/api/account/me", get(me))
        .route("/api/account/logout", post(logout))
        .route("/api/account/password", put(password))
        .route("/api/account/profile", put(profile))
        .route("/api/account/portal", get(portal))
        .route("/api/accounts", get(list).post(create))
        .route("/api/accounts/{id}", put(update))
        .layer(DefaultBodyLimit::max(8192))
        .layer(axum::middleware::map_response(json_errors))
}
pub(crate) async fn json_errors(r: Response) -> Response {
    let mut r = if [StatusCode::UNPROCESSABLE_ENTITY, StatusCode::BAD_REQUEST].contains(&r.status())
        && r.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/plain"))
    {
        ApiError::bad_request("请求字段或格式无效").into_response()
    } else {
        r
    };
    r.headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    r
}

fn cookie(h: &HeaderMap, name: &str) -> Result<Option<String>, ApiError> {
    let mut found = None;
    for raw in h.get_all(header::COOKIE) {
        let raw = raw.to_str().map_err(|_| denied())?;
        for part in raw.split(';') {
            let part = part.trim();
            let Some((key, value)) = part.split_once('=') else {
                if part == name {
                    return Err(denied());
                }
                continue;
            };
            if key.trim() == name && key != name {
                return Err(denied());
            }
            if key == name {
                if value.is_empty() || value.len() > 512 || found.is_some() {
                    return Err(denied());
                }
                found = Some(value.to_owned())
            }
        }
    }
    Ok(found)
}
pub fn legacy_request_headers(h: &HeaderMap) -> Result<(), ApiError> {
    ambiguous(h)?;
    if h.contains_key(header::COOKIE) {
        csrf_headers(h)?
    }
    Ok(())
}
fn ambiguous(h: &HeaderMap) -> Result<(), ApiError> {
    if h.contains_key(header::AUTHORIZATION) && h.contains_key(header::COOKIE) {
        Err(denied())
    } else {
        Ok(())
    }
}
fn safe(method: &axum::http::Method) -> bool {
    matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS")
}
pub fn csrf(parts: &Parts) -> Result<(), ApiError> {
    csrf_headers(&parts.headers)
}
fn csrf_headers(h: &HeaderMap) -> Result<(), ApiError> {
    if h.get("x-tokengo-panel").and_then(|v| v.to_str().ok()) != Some("1")
        || h.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("cross-site")
    {
        return Err(forbidden());
    }
    if h.get_all(header::ORIGIN).iter().count() > 1 {
        return Err(forbidden());
    }
    if let Some(origin) = h.get(header::ORIGIN) {
        let origin = origin
            .to_str()
            .map_err(|_| forbidden())?
            .parse::<axum::http::Uri>()
            .map_err(|_| forbidden())?;
        let scheme = origin
            .scheme_str()
            .filter(|s| matches!(*s, "http" | "https"))
            .ok_or_else(forbidden)?;
        let authority = origin.authority().ok_or_else(forbidden)?;
        if authority.as_str().contains('@') {
            return Err(forbidden());
        }
        let host = h
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(forbidden)?;
        let requested = format!("{scheme}://{host}")
            .parse::<axum::http::Uri>()
            .map_err(|_| forbidden())?;
        let default = if scheme == "https" { 443 } else { 80 };
        if origin.host().map(str::to_ascii_lowercase)
            != requested.host().map(str::to_ascii_lowercase)
            || origin.port_u16().unwrap_or(default) != requested.port_u16().unwrap_or(default)
        {
            return Err(forbidden());
        }
    }
    Ok(())
}
pub struct RootGrant {
    raw: String,
    browser: bool,
    account_id: i64,
}
impl RootGrant {
    pub(crate) fn valid(&self, c: &rusqlite::Connection) -> db::Result<bool> {
        if self.browser {
            Ok(accounts::authenticate(c, &self.raw)?.is_some_and(|a| a.account.role == "root"))
        } else {
            db::sessions::check_session(c, &self.raw)
        }
    }
}
impl FromRequestParts<SharedState> for RootGrant {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, ApiError> {
        root_grant(parts, state).await
    }
}
pub async fn root_authorize(parts: &Parts, state: &SharedState) -> Result<(), ApiError> {
    root_grant(parts, state).await.map(|_| ())
}
async fn root_grant(parts: &Parts, state: &SharedState) -> Result<RootGrant, ApiError> {
    ambiguous(&parts.headers)?;
    if let Some(raw) = cookie(&parts.headers, COOKIE)? {
        let a = accounts::authenticate(&state.conn(), &raw)?.ok_or_else(denied)?;
        if a.account.role != "root" {
            return Err(forbidden());
        }
        if !safe(&parts.method) {
            csrf(parts)?
        }
        return Ok(RootGrant {
            raw,
            browser: true,
            account_id: a.account.id,
        });
    }
    let bearer = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let legacy = if let Some(t) = bearer {
        Some(t.to_owned())
    } else {
        let c = cookie(&parts.headers, "tokengo_session")?;
        if c.is_some() && !safe(&parts.method) {
            csrf(parts)?
        }
        c
    };
    if let Some(raw) = legacy {
        let c = state.conn();
        if db::sessions::check_session(&c, &raw)? {
            let account_id = accounts::root(&c)?.map(|a| a.account.id).unwrap_or(0);
            return Ok(RootGrant {
                raw,
                browser: false,
                account_id,
            });
        }
    }
    Err(denied())
}
pub struct Browser {
    pub credential: Credential,
    raw: String,
}
impl FromRequestParts<SharedState> for Browser {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        ambiguous(&parts.headers)?;
        if parts.headers.contains_key(header::AUTHORIZATION) {
            return Err(denied());
        }
        let raw = cookie(&parts.headers, COOKIE)?.ok_or_else(denied)?;
        let credential = accounts::authenticate(&state.conn(), &raw)?.ok_or_else(denied)?;
        if !safe(&parts.method) {
            csrf(parts)?
        }
        Ok(Self { credential, raw })
    }
}
fn secure(h: &HeaderMap) -> bool {
    h.get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("https://"))
        || h.get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').next().is_some_and(|v| v.trim() == "https"))
}
fn response(v: Value, h: &HeaderMap, session: Option<&str>) -> Response {
    let mut r = Json(v).into_response();
    let value = match session {
        Some(raw) => format!(
            "{COOKIE}={raw}; Path=/; HttpOnly; SameSite=Lax; Max-Age=43200{}",
            if secure(h) { "; Secure" } else { "" }
        ),
        None => format!(
            "{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
            if secure(h) { "; Secure" } else { "" }
        ),
    };
    r.headers_mut()
        .insert(header::SET_COOKIE, value.parse().unwrap());
    r.headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    r
}
fn same(c: &rusqlite::Connection, a: &Credential) -> db::Result<bool> {
    Ok(accounts::by_id(c, a.account.id)?.is_some_and(|current| {
        current.version == a.version
            && current.hash == a.hash
            && current.account.username == a.account.username
    }) && accounts::active(c, a)?)
}
pub(crate) async fn hash(state: &SharedState, password: String) -> Result<String, ApiError> {
    let slot = state.auth_slots.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "auth_busy",
            "登录服务繁忙，请稍后重试",
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        db::sessions::hash_password(&password)
    })
    .await
    .map_err(|_| ApiError::internal("密码处理失败"))?
    .map_err(Into::into)
}
pub(crate) async fn verify(
    state: &SharedState,
    password: String,
    hash: String,
) -> Result<bool, ApiError> {
    if password.len() > 256 {
        return Ok(false);
    }
    let slot = state.auth_slots.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "auth_busy",
            "登录服务繁忙，请稍后重试",
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        db::sessions::verify_password(&password, &hash)
    })
    .await
    .map_err(|_| ApiError::internal("密码校验失败"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    username: String,
    password: String,
}
async fn login(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<Login>,
) -> Result<Response, ApiError> {
    ambiguous(&headers)?;
    csrf_headers(&headers)?;
    let name = body.username.to_ascii_lowercase();
    state.panel_login_gate.check(&name)?;
    let account = accounts::by_username(&state.conn(), &name)?;
    let expected = account
        .as_ref()
        .map(|a| a.hash.clone())
        .or(accounts::root(&state.conn())?.map(|a| a.hash))
        .unwrap_or_default();
    let ok = verify(&state, body.password, expected).await?;
    let c = state.conn();
    let a = account.ok_or_else(denied)?;
    if !ok || !same(&c, &a)? {
        return Err(denied());
    }
    let raw = accounts::atomic(&c, || accounts::create_session(&c, &a))?;
    state.panel_login_gate.success(&name);
    Ok(response(json!({"account":a.account}), &headers, Some(&raw)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Setup {
    password: String,
    display_name: Option<String>,
    gateway_key: Option<String>,
}
async fn setup(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<Setup>,
) -> Result<Response, ApiError> {
    ambiguous(&headers)?;
    csrf_headers(&headers)?;
    accounts::password(&body.password).map_err(bad)?;
    {
        let c = state.conn();
        if db::sessions::has_panel_password(&c)? || accounts::root(&c)?.is_some() {
            return Err(bad("已经初始化，请登录"));
        }
    }
    state.panel_login_gate.check("__initial_setup")?;
    let display = body.display_name.unwrap_or_else(|| "管理员".into());
    accounts::display_name(&display).map_err(bad)?;
    if body
        .gateway_key
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 4096)
    {
        return Err(bad("无效的Gateway Key"));
    }
    let hash = hash(&state, body.password).await?;
    let generated = body.gateway_key.is_none();
    let key = body.gateway_key.unwrap_or_else(db::gateway_keys::generate);
    let c = state.conn();
    let (a, raw) = accounts::atomic(&c, || {
        if db::sessions::has_panel_password(&c)? || accounts::root(&c)?.is_some() {
            return Err(db::DbError::Other("已经初始化，请登录".into()));
        }
        db::sessions::set_panel_hash(&c, &hash)?;
        if let Some(id) = db::gateway_keys::default_id(&c)? {
            db::gateway_keys::delete(&c, id)?;
        }
        db::gateway_keys::create(&c, &state.secrets, "Default", &key, true)?;
        db::settings::set(&c, "panel_password_set_at_ms", &db::now_ms().to_string())?;
        let a = accounts::root(&c)?.ok_or_else(|| db::DbError::Other("管理员账号不存在".into()))?;
        c.execute(
            "UPDATE panel_accounts SET display_name=?1 WHERE id=?2",
            rusqlite::params![display, a.account.id],
        )?;
        let a = accounts::by_id(&c, a.account.id)?.unwrap();
        let raw = accounts::create_session(&c, &a)?;
        accounts::audit(&c, a.account.id, "panel.setup", a.account.id)?;
        Ok((a, raw))
    })
    .map_err(bad)?;
    drop(c);
    state.forget_gateway_key();
    Ok(response(
        json!({"account":a.account,"gateway_key":key,"gateway_key_generated":generated}),
        &headers,
        Some(&raw),
    ))
}
async fn me(browser: Browser) -> Json<Value> {
    Json(json!({"account":browser.credential.account}))
}
async fn logout(
    browser: Browser,
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    accounts::revoke(&state.conn(), &browser.raw)?;
    Ok(response(json!({"ok":true}), &headers, None))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Password {
    old_password: String,
    new_password: String,
}
async fn password(
    browser: Browser,
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<Password>,
) -> Result<Response, ApiError> {
    accounts::password(&body.new_password).map_err(bad)?;
    state
        .panel_login_gate
        .check(&browser.credential.account.username)?;
    if !verify(&state, body.old_password, browser.credential.hash.clone()).await? {
        return Err(denied());
    }
    let hash = hash(&state, body.new_password).await?;
    let c = state.conn();
    if !same(&c, &browser.credential)? || accounts::authenticate(&c, &browser.raw)?.is_none() {
        return Err(denied());
    }
    accounts::atomic(&c, || {
        if browser.credential.account.role == "root" {
            db::sessions::set_panel_hash(&c, &hash)?;
        } else {
            c.execute(
                "UPDATE panel_accounts SET password_hash=?1 WHERE id=?2",
                rusqlite::params![hash, browser.credential.account.id],
            )?;
        }
        accounts::audit(
            &c,
            browser.credential.account.id,
            "panel.password",
            browser.credential.account.id,
        )
    })?;
    state
        .panel_login_gate
        .success(&browser.credential.account.username);
    Ok(response(
        json!({"ok":true,"reauthenticate":true}),
        &headers,
        None,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    username: String,
    old_password: String,
}
async fn profile(
    browser: Browser,
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<Profile>,
) -> Result<Response, ApiError> {
    if browser.credential.account.role != "root" {
        return Err(forbidden());
    }
    let name = accounts::username(&body.username).map_err(bad)?;
    if name == browser.credential.account.username {
        return Err(bad("新用户名不能与当前用户名相同"));
    }
    state
        .panel_login_gate
        .check(&browser.credential.account.username)?;
    if !verify(&state, body.old_password, browser.credential.hash.clone()).await? {
        return Err(denied());
    }
    let c = state.conn();
    if !same(&c, &browser.credential)? || accounts::authenticate(&c, &browser.raw)?.is_none() {
        return Err(denied());
    }
    if accounts::by_username(&c, &name)?
        .is_some_and(|a| a.account.id != browser.credential.account.id)
    {
        return Err(bad("用户名已被使用"));
    }
    accounts::atomic(&c, || {
        c.execute(
            "UPDATE panel_accounts SET username=?1 WHERE id=?2",
            rusqlite::params![name, browser.credential.account.id],
        )?;
        c.execute(
            "DELETE FROM account_sessions WHERE account_id=?1",
            [browser.credential.account.id],
        )?;
        db::sessions::revoke_all(&c)?;
        accounts::audit(
            &c,
            browser.credential.account.id,
            "panel.username",
            browser.credential.account.id,
        )
    })?;
    state
        .panel_login_gate
        .success(&browser.credential.account.username);
    Ok(response(
        json!({"ok":true,"reauthenticate":true}),
        &headers,
        None,
    ))
}
async fn list(root: RootGrant, State(state): State<SharedState>) -> Result<Json<Value>, ApiError> {
    let c = state.conn();
    if !root.valid(&c)? {
        return Err(denied());
    }
    Ok(Json(json!({"accounts":accounts::accounts(&c)?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewAccount {
    username: String,
    password: String,
    display_name: String,
    user_id: i64,
    role: Option<String>,
}
async fn create(
    root: RootGrant,
    State(state): State<SharedState>,
    Json(body): Json<NewAccount>,
) -> Result<Json<Value>, ApiError> {
    accounts::password(&body.password).map_err(bad)?;
    let name = accounts::username(&body.username).map_err(bad)?;
    if matches!(name.as_str(), "admin" | "root") || body.role.is_some_and(|r| r != "user") {
        return Err(bad("这里只能创建普通账号"));
    }
    let hash = hash(&state, body.password).await?;
    let c = state.conn();
    if !root.valid(&c)? {
        return Err(denied());
    }
    let a = accounts::atomic(&c, || {
        let a = accounts::create(&c, &name, &body.display_name, body.user_id, &hash)?;
        accounts::audit(&c, root.account_id, "panel.create", a.id)?;
        Ok(a)
    })
    .map_err(bad)?;
    Ok(Json(json!({"account":a})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountPatch {
    display_name: Option<String>,
    enabled: Option<bool>,
    password: Option<String>,
}
async fn update(
    root: RootGrant,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<AccountPatch>,
) -> Result<Json<Value>, ApiError> {
    if let Some(n) = &body.display_name {
        accounts::display_name(n).map_err(bad)?;
    }
    let reset_password = body.password.is_some();
    let hash = if let Some(p) = body.password {
        accounts::password(&p).map_err(bad)?;
        Some(hash(&state, p).await?)
    } else {
        None
    };
    let c = state.conn();
    if !root.valid(&c)? {
        return Err(denied());
    }
    let a = accounts::by_id(&c, id)?.ok_or_else(|| ApiError::not_found("账号不存在"))?;
    if a.account.role == "root" && reset_password {
        return Err(bad("管理员密码须通过账号安全接口确认当前密码后修改"));
    }
    if a.account.role == "root" && body.enabled == Some(false) {
        return Err(bad("不能禁用唯一管理员"));
    }
    let account = accounts::atomic(&c, || {
        if let Some(display) = body.display_name {
            c.execute(
                "UPDATE panel_accounts SET display_name=?1 WHERE id=?2",
                rusqlite::params![display.trim(), id],
            )?;
        }
        if let Some(enabled) = body.enabled {
            c.execute(
                "UPDATE panel_accounts SET enabled=?1 WHERE id=?2",
                rusqlite::params![enabled, id],
            )?;
        }
        if let Some(hash) = hash {
            if a.account.role == "root" {
                db::sessions::set_panel_hash(&c, &hash)?;
            } else {
                c.execute(
                    "UPDATE panel_accounts SET password_hash=?1 WHERE id=?2",
                    rusqlite::params![hash, id],
                )?;
            }
        }
        accounts::audit(&c, root.account_id, "panel.update", id)?;
        Ok(accounts::by_id(&c, id)?.unwrap().account)
    })?;
    Ok(Json(json!({"account":account})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PortalQuery {
    before_id: Option<i64>,
    limit: Option<u32>,
}
async fn portal(
    browser: Browser,
    State(state): State<SharedState>,
    Query(q): Query<PortalQuery>,
) -> Result<Json<Value>, ApiError> {
    if browser.credential.account.role != "user" {
        return Err(forbidden());
    }
    let user = browser.credential.account.user_id.ok_or_else(forbidden)?;
    if q.before_id.is_some_and(|i| i <= 0) || q.limit.is_some_and(|i| !(1..=100).contains(&i)) {
        return Err(bad("分页参数无效"));
    }
    let c = state.conn();
    if accounts::authenticate(&c, &browser.raw)?.is_none() {
        return Err(denied());
    }
    let keys:Vec<_>=db::identity::keys(&c,user)?.into_iter().map(|k|json!({"id":k.id,"name":k.name,"hint":k.hint,"enabled":k.enabled,"revoked_at":k.revoked_at,"expires_at":k.expires_at,"parent_key_id":k.parent_key_id,"created_at_ms":k.created_at_ms})).collect();
    let rows = db::billing::usage(
        &c,
        &db::billing::UsageQuery {
            user_id: Some(user),
            before_id: q.before_id,
            limit: Some(q.limit.unwrap_or(50)),
            ..Default::default()
        },
    )?;
    let next = rows.last().map(|r| r.id);
    let entries:Vec<_>=rows.into_iter().map(|r|json!({"id":r.id,"request_id":r.request_id,"key_id":r.key_id,"model":r.model,"billing_unit":r.billing_unit,"status":r.status,"outcome":r.outcome,"input_tokens":r.input_tokens,"output_tokens":r.output_tokens,"usage_certainty":r.usage_certainty,"debit_micro":r.debit_micro,"created_ms":r.created_ms,"settled_ms":r.settled_ms})).collect();
    let summary=c.query_row("SELECT count(DISTINCT request_id),count(*),coalesce(sum(input_tokens),0),coalesce(sum(output_tokens),0),coalesce(sum(input_tokens IS NULL),0),coalesce(sum(output_tokens IS NULL),0),coalesce(sum(price_id IS NULL AND request_price_id IS NULL),0),coalesce(sum(debit_micro),0) FROM billing_ledger WHERE user_id=?1",[user],|r|Ok(json!({"request_count":r.get::<_,i64>(0)?,"ledger_count":r.get::<_,i64>(1)?,"input_tokens":r.get::<_,i64>(2)?,"output_tokens":r.get::<_,i64>(3)?,"unknown_input_entries":r.get::<_,i64>(4)?,"unknown_output_entries":r.get::<_,i64>(5)?,"unpriced_entries":r.get::<_,i64>(6)?,"known_debit_micro":r.get::<_,i64>(7)?}))).map_err(db::DbError::from)?;
    Ok(Json(
        json!({"account":browser.credential.account,"keys":keys,"usage":{"summary":summary,"entries":entries,"next_before_id":next}}),
    ))
}
