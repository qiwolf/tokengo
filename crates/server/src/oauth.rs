//! Independent public-client OpenAI registration. No legacy token-pair sharing.
use crate::{db, protocol::Protocol, SharedState};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
const ISSUER: &str = "https://auth.openai.com";
pub const API_BASE: &str = "https://api.openai.com/v1";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OAuthError(pub &'static str);
type Result<T> = std::result::Result<T, OAuthError>;
fn storage<T>(r: std::result::Result<T, impl std::fmt::Debug>) -> Result<T> {
    r.map_err(|_| OAuthError("oauth_storage_unavailable"))
}
fn invalid<T>() -> Result<T> {
    Err(OAuthError("oauth_invalid_result"))
}
fn random() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
#[derive(Clone)]
struct Endpoints {
    authorize: String,
    token: String,
    jwks: String,
    discovery: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            authorize: format!("{ISSUER}/api/accounts/authorize"),
            token: format!("{ISSUER}/api/accounts/oauth/token"),
            jwks: format!("{ISSUER}/.well-known/jwks.json"),
            discovery: format!("{ISSUER}/.well-known/openid-configuration"),
        }
    }
}
pub struct OAuthRuntime {
    client: reqwest::Client,
    endpoints: Mutex<Endpoints>,
    locks: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    #[cfg(test)]
    api_override: Mutex<Option<String>>,
}
impl Default for OAuthRuntime {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .build()
                .expect("OAuth HTTP client"),
            endpoints: Mutex::new(Endpoints::default()),
            locks: Mutex::new(HashMap::new()),
            #[cfg(test)]
            api_override: Mutex::new(None),
        }
    }
}
impl OAuthRuntime {
    fn endpoints(&self) -> Endpoints {
        self.endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn lock(&self, id: i64) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id)
            .or_default()
            .clone()
    }
    #[cfg(test)]
    pub(crate) fn test_api_base(&self, base: &str) {
        *self.api_override.lock().unwrap() = Some(base.into());
    }
    fn api_base(&self) -> String {
        #[cfg(test)]
        if let Some(base) = self.api_override.lock().unwrap().clone() {
            return base;
        }
        API_BASE.into()
    }
    #[cfg(test)]
    fn test_endpoints(&self, base: &str) {
        *self.endpoints.lock().unwrap() = Endpoints {
            authorize: format!("{base}/authorize"),
            token: format!("{base}/token"),
            jwks: format!("{base}/jwks"),
            discovery: format!("{base}/discovery"),
        };
    }
}
/// Deliberately no Debug or Serialize: a source bearer is never a management response.
pub struct OAuthCredential {
    pub access_token: String,
    pub account_id: i64,
    pub credential_revision: i64,
    pub binding_generation: i64,
    pub expires_at: i64,
    pub api_base: String,
    pub provider: Protocol,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Account {
    pub id: i64,
    pub label: String,
    pub issuer: String,
    pub subject: String,
    pub client_id: String,
    pub email: Option<String>,
    pub status: String,
    pub scopes: Vec<String>,
    pub expires_at: i64,
    pub earliest_refresh_at: i64,
    pub credential_revision: i64,
}
#[derive(Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    id_token: String,
    scopes: Vec<String>,
}
#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    id_token: Option<String>,
    token_type: String,
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    earliest_refresh_at: Option<i64>,
}
#[derive(Serialize, Deserialize)]
struct Pending {
    label: String,
    redirect_uri: String,
    verifier: String,
    nonce: String,
    client_id: String,
    account_id: Option<i64>,
    subject: Option<String>,
    revision: Option<i64>,
}
#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    email: Option<String>,
}
#[derive(Serialize)]
pub struct Started {
    pub attempt_id: String,
    pub authorization_url: String,
    pub expires_at: i64,
    pub host_id: String,
}
fn host_id(c: &rusqlite::Connection) -> Result<String> {
    storage(c.execute(
        "INSERT OR IGNORE INTO oauth_hosts(id,host_id) VALUES(1,?1)",
        [format!("urn:uuid:{}", uuid::Uuid::new_v4())],
    ))?;
    storage(
        c.query_row("SELECT host_id FROM oauth_hosts WHERE id=1", [], |r| {
            r.get(0)
        }),
    )
}
fn account(c: &rusqlite::Connection, id: i64) -> Result<Account> {
    let row=storage(c.query_row("SELECT id,label,issuer,subject,client_id,email,status,scopes_json,expires_at,earliest_refresh_at,credential_revision FROM oauth_accounts WHERE id=?1",[id],|r|Ok((Account{id:r.get(0)?,label:r.get(1)?,issuer:r.get(2)?,subject:r.get(3)?,client_id:r.get(4)?,email:r.get(5)?,status:r.get(6)?,scopes:vec![],expires_at:r.get(8)?,earliest_refresh_at:r.get(9)?,credential_revision:r.get(10)?},r.get::<_,String>(7)?))).optional())?.ok_or(OAuthError("oauth_account_not_found"))?;
    let (mut a, scopes) = row;
    a.scopes = storage(serde_json::from_str(&scopes))?;
    Ok(a)
}
pub fn accounts(state: &SharedState) -> Result<Vec<Account>> {
    let c = state.conn();
    let ids = storage(c.prepare("SELECT id FROM oauth_accounts ORDER BY id"))?
        .query_map([], |r| r.get::<_, i64>(0))
        .map_err(|_| OAuthError("oauth_storage_unavailable"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| OAuthError("oauth_storage_unavailable"))?;
    ids.into_iter().map(|id| account(&c, id)).collect()
}
fn seal<T: Serialize>(state: &SharedState, value: &T) -> Result<(Vec<u8>, Vec<u8>)> {
    storage(state.secrets.seal(&storage(serde_json::to_string(value))?))
}
fn tokens(state: &SharedState, id: i64) -> Result<Tokens> {
    let pair:Option<(Vec<u8>,Vec<u8>)>=storage(state.conn().query_row("SELECT nonce,cipher FROM oauth_accounts WHERE id=?1 AND status='active' AND nonce IS NOT NULL AND cipher IS NOT NULL",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional())?;
    let (n, c) = pair.ok_or(OAuthError("oauth_unavailable"))?;
    storage(serde_json::from_str(&storage(state.secrets.open(&n, &c))?))
}
fn callback_uri(value: &str) -> Result<()> {
    let u = reqwest::Url::parse(value).map_err(|_| OAuthError("oauth_invalid_callback"))?;
    if u.scheme() != "http"
        || u.host_str() != Some("127.0.0.1")
        || u.port().is_none_or(|port| port == 0)
        || u.path() != "/auth/callback"
        || u.query().is_some()
        || u.fragment().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(OAuthError("oauth_invalid_callback"));
    }
    Ok(())
}
pub fn start(
    state: &SharedState,
    label: String,
    redirect_uri: String,
    account_id: Option<i64>,
) -> Result<Started> {
    db::identity::validate_name(&label).map_err(|_| OAuthError("oauth_invalid_label"))?;
    callback_uri(&redirect_uri)?;
    let c = state.conn();
    let host = host_id(&c)?;
    let selected = account_id.map(|id| account(&c, id)).transpose()?;
    let mut pending = Pending {
        label: label.trim().into(),
        redirect_uri: redirect_uri.clone(),
        verifier: random(),
        nonce: random(),
        client_id: selected
            .as_ref()
            .map(|a| a.client_id.clone())
            .unwrap_or_else(|| "dynamic_agent_client".into()),
        account_id,
        subject: selected.as_ref().map(|a| a.subject.clone()),
        revision: selected.as_ref().map(|a| a.credential_revision),
    };
    let mut url = reqwest::Url::parse(&state.oauth.endpoints().authorize)
        .map_err(|_| OAuthError("oauth_unavailable"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", &pending.client_id)
            .append_pair("ext_agent_host_id", &host)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", SCOPE)
            .append_pair("resource", API_BASE)
            .append_pair("nonce", &pending.nonce)
            .append_pair("code_challenge_method", "S256")
            .append_pair(
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(pending.verifier.as_bytes())),
            );
        if selected.is_none() {
            q.append_pair("agent_name_hint", "Tokengo");
        }
    }
    // Fetch retained hint using this guard, never re-enter the state DB lock.
    if let Some(a) = &selected {
        let pair:Option<(Vec<u8>,Vec<u8>)>=storage(c.query_row("SELECT nonce,cipher FROM oauth_accounts WHERE id=?1 AND status='active' AND cipher IS NOT NULL",[a.id],|r|Ok((r.get(0)?,r.get(1)?))).optional())?;
        if let Some((n, b)) = pair {
            let saved: Tokens =
                storage(serde_json::from_str(&storage(state.secrets.open(&n, &b))?))?;
            url.query_pairs_mut()
                .append_pair("id_token_hint", &saved.id_token);
        }
    }
    let state_value = random();
    url.query_pairs_mut().append_pair("state", &state_value);
    let id = uuid::Uuid::new_v4().to_string();
    let expiry = db::now_ms() + 600_000;
    let (n, b) = seal(state, &pending)?;
    storage(db::in_tx(&c, |c| {
        c.execute(
            "DELETE FROM oauth_pending WHERE expires_at<?1",
            [db::now_ms()],
        )?;
        let count: i64 = c.query_row("SELECT count(*) FROM oauth_pending", [], |r| r.get(0))?;
        if count >= 64 {
            return Err(db::DbError::Other("pending limit".into()));
        }
        c.execute("INSERT INTO oauth_pending(id,state_digest,expires_at,nonce,cipher) VALUES(?1,?2,?3,?4,?5)",params![id,digest(&state_value),expiry,n,b])?;
        Ok(())
    }))?;
    pending.verifier.clear();
    Ok(Started {
        attempt_id: id,
        authorization_url: url.to_string(),
        expires_at: expiry,
        host_id: host,
    })
}
async fn json_body(response: reqwest::Response) -> Result<serde_json::Value> {
    bounded_json(response, 131072).await
}
async fn bounded_json(mut response: reqwest::Response, limit: usize) -> Result<serde_json::Value> {
    let mut out = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| OAuthError("oauth_unavailable"))?
    {
        if out.len() + chunk.len() > limit {
            return Err(OAuthError("oauth_invalid_result"));
        }
        out.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&out).map_err(|_| OAuthError("oauth_invalid_result"))
}
async fn exchange(state: &SharedState, form: &[(&str, &str)]) -> Result<TokenReply> {
    let response = state
        .oauth
        .client
        .post(state.oauth.endpoints().token)
        .form(form)
        .send()
        .await
        .map_err(|_| OAuthError("oauth_unavailable"))?;
    let status = response.status();
    let body = json_body(response).await?;
    if !status.is_success() {
        return Err(OAuthError(if body["error"] == "invalid_grant" {
            "oauth_reauthorization_required"
        } else {
            "oauth_unavailable"
        }));
    }
    serde_json::from_value(body).map_err(|_| OAuthError("oauth_invalid_result"))
}
async fn verify(
    state: &SharedState,
    id_token: &str,
    client_id: &str,
    nonce: Option<&str>,
) -> Result<Claims> {
    if id_token.len() > 32768 {
        return invalid();
    }
    let header = decode_header(id_token).map_err(|_| OAuthError("oauth_invalid_identity"))?;
    if header.alg != Algorithm::RS256 {
        return Err(OAuthError("oauth_invalid_identity"));
    }
    let kid = header
        .kid
        .filter(|k| !k.is_empty() && k.len() <= 256)
        .ok_or(OAuthError("oauth_invalid_identity"))?;
    let r = state
        .oauth
        .client
        .get(state.oauth.endpoints().jwks)
        .send()
        .await
        .map_err(|_| OAuthError("oauth_unavailable"))?;
    if !r.status().is_success() {
        return Err(OAuthError("oauth_unavailable"));
    }
    let jwks: JwkSet = serde_json::from_value(json_body(r).await?)
        .map_err(|_| OAuthError("oauth_invalid_identity"))?;
    if jwks.keys.len() > 32 {
        return invalid();
    }
    let jwk = jwks
        .find(&kid)
        .ok_or(OAuthError("oauth_invalid_identity"))?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| OAuthError("oauth_invalid_identity"))?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["iss", "aud", "sub", "exp", "iat"]);
    validation.leeway = 5;
    validation.validate_nbf = true;
    let claims = decode::<Claims>(id_token, &key, &validation)
        .map_err(|_| OAuthError("oauth_invalid_identity"))?
        .claims;
    if claims.iss != ISSUER
        || claims.sub.is_empty()
        || claims.sub.len() > 512
        || nonce.is_some_and(|n| claims.nonce.as_deref() != Some(n))
    {
        return Err(OAuthError("oauth_invalid_identity"));
    }
    Ok(claims)
}
fn checked_reply(reply: TokenReply, old: Option<&Tokens>) -> Result<(Tokens, i64, i64)> {
    if reply.token_type != "Bearer"
        || reply.access_token.is_empty()
        || reply.refresh_token.is_empty()
        || reply.access_token.len() > 32768
        || !reply
            .access_token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
        || reply.refresh_token.chars().any(char::is_control)
        || reply.refresh_token.len() > 32768
        || !(1..=86400).contains(&reply.expires_in)
    {
        return invalid();
    }
    let scopes = match reply.scope {
        Some(s) => s.split_whitespace().map(str::to_string).collect(),
        None => old
            .map(|v| v.scopes.clone())
            .ok_or(OAuthError("oauth_grant_missing"))?,
    };
    if [
        "offline_access",
        "resource.invoke",
        "chatgpt.tokens.use.direct",
    ]
    .iter()
    .any(|s| !scopes.iter().any(|v| v == s))
    {
        return Err(OAuthError("oauth_grant_missing"));
    }
    let id_token = reply
        .id_token
        .or_else(|| old.map(|v| v.id_token.clone()))
        .ok_or(OAuthError("oauth_invalid_identity"))?;
    let now = db::now_ms();
    let expiry = now
        .checked_add(reply.expires_in * 1000)
        .ok_or(OAuthError("oauth_invalid_result"))?;
    let earliest = reply
        .earliest_refresh_at
        .map(|v| {
            v.checked_mul(1000)
                .filter(|v| *v >= 0)
                .ok_or(OAuthError("oauth_invalid_result"))
        })
        .transpose()?
        .unwrap_or(now);
    Ok((
        Tokens {
            access_token: reply.access_token,
            refresh_token: reply.refresh_token,
            id_token,
            scopes,
        },
        expiry,
        earliest,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub attempt_id: String,
    pub state: String,
    pub code: Option<String>,
    pub client_id: Option<String>,
    pub error: Option<String>,
}
pub async fn complete(state: &SharedState, result: Completion) -> Result<Account> {
    if result.attempt_id.len() > 64 || result.state.len() > 128 {
        return invalid();
    }
    let pending = {
        let c = state.conn();
        storage(db::in_tx(&c,|c|{let row:Option<(Vec<u8>,Vec<u8>)>=c.query_row("SELECT nonce,cipher FROM oauth_pending WHERE id=?1 AND state_digest=?2 AND expires_at>?3 AND consumed=0",params![result.attempt_id,digest(&result.state),db::now_ms()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;if row.is_some(){c.execute("UPDATE oauth_pending SET consumed=1 WHERE id=?1",[&result.attempt_id])?;}Ok(row)}))?.ok_or(OAuthError("oauth_invalid_state"))?
    };
    let (n, b) = pending;
    let pending: Pending = storage(serde_json::from_str(&storage(state.secrets.open(&n, &b))?))?;
    if result.error.is_some() {
        return Err(OAuthError("oauth_authorization_denied"));
    }
    let client = if pending.client_id == "dynamic_agent_client" {
        result
            .client_id
            .filter(|v| {
                !v.is_empty()
                    && v != "dynamic_agent_client"
                    && v.len() <= 256
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            })
            .ok_or(OAuthError("oauth_registration_incomplete"))?
    } else {
        if result
            .client_id
            .as_ref()
            .is_some_and(|id| id != &pending.client_id)
        {
            return invalid();
        }
        pending.client_id.clone()
    };
    let code = result
        .code
        .filter(|v| !v.is_empty() && v.len() <= 8192 && !v.chars().any(char::is_control))
        .ok_or(OAuthError("oauth_invalid_result"))?;
    let reply = exchange(
        state,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &client),
            ("code", &code),
            ("code_verifier", &pending.verifier),
            ("redirect_uri", &pending.redirect_uri),
            ("resource", API_BASE),
        ],
    )
    .await?;
    let id = reply
        .id_token
        .as_deref()
        .ok_or(OAuthError("oauth_invalid_identity"))?;
    let claims = verify(state, id, &client, Some(&pending.nonce)).await?;
    if pending.subject.as_ref().is_some_and(|v| v != &claims.sub) {
        return Err(OAuthError("oauth_account_mismatch"));
    }
    let (bundle, expiry, earliest) = checked_reply(reply, None)?;
    let (n, b) = seal(state, &bundle)?;
    let scopes = storage(serde_json::to_string(&bundle.scopes))?;
    let c = state.conn();
    let id = storage(db::in_tx(&c, |c| {
        let id = if let Some(id) = pending.account_id {
            let changed=c.execute("UPDATE oauth_accounts SET email=?2,status='active',scopes_json=?3,expires_at=?4,earliest_refresh_at=?5,nonce=?6,cipher=?7,credential_revision=credential_revision+1,refresh_owner=NULL,refresh_lease_until=0 WHERE id=?1 AND subject=?8 AND client_id=?9 AND credential_revision=?10",params![id,claims.email,scopes,expiry,earliest,n,b,claims.sub,client,pending.revision])?;
            if changed != 1 {
                return Err(db::DbError::Other("stale authorization".into()));
            }
            id
        } else {
            c.execute("INSERT INTO oauth_accounts(label,issuer,subject,client_id,email,status,scopes_json,expires_at,earliest_refresh_at,nonce,cipher) VALUES(?1,?2,?3,?4,?5,'active',?6,?7,?8,?9,?10)",params![pending.label,ISSUER,claims.sub,client,claims.email,scopes,expiry,earliest,n,b])?;
            c.last_insert_rowid()
        };
        db::identity::audit(
            c,
            None,
            "oauth.authorize",
            "oauth_account",
            id,
            serde_json::json!({}),
        )?;
        Ok(id)
    }))?;
    account(&c, id)
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub account_id: i64,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub max_input_tokens: Option<i64>,
    #[serde(default)]
    pub max_output_tokens: Option<i64>,
}
fn clean_source(c: &rusqlite::Connection, channel_id: i64) -> db::Result<bool> {
    Ok(
        db::channels::get(c, tokengo_shared::ChannelId(channel_id))?.is_some_and(|row| {
            row.base_url.trim_end_matches('/') == API_BASE
                && !row.has_secret
                && row.headers.is_empty()
                && row.models.iter().all(|model| model.drop_fields.is_empty())
        }),
    )
}
pub fn bind(state: &SharedState, channel_id: i64, binding: &Binding) -> Result<()> {
    if [binding.max_input_tokens, binding.max_output_tokens]
        .into_iter()
        .flatten()
        .any(|n| !(1..=16_777_216).contains(&n))
    {
        return Err(OAuthError("oauth_invalid_bounds"));
    }
    if binding
        .reasoning_effort
        .as_deref()
        .is_some_and(|v| !matches!(v, "low" | "medium" | "high"))
        || binding
            .service_tier
            .as_deref()
            .is_some_and(|v| !matches!(v, "priority" | "auto" | "default"))
    {
        return Err(OAuthError("oauth_invalid_defaults"));
    }
    let c = state.conn();
    storage(db::in_tx(&c, |c| {
        if !clean_source(c, channel_id)? {
            return Err(db::DbError::Other(
                "OAuth binding requires clean official source without drop fields".into(),
            ));
        }
        let active:i64=c.query_row("SELECT count(*) FROM oauth_accounts WHERE id=?1 AND status='active' AND cipher IS NOT NULL",[binding.account_id],|r|r.get(0))?;
        if active != 1 {
            return Err(db::DbError::Other("account unavailable".into()));
        }
        crate::protocol_config::save(
            c,
            channel_id,
            &crate::protocol_config::Capabilities {
                native: vec![Protocol::Responses],
                allow_conversion: true,
                native_input_audio: false,
            },
        )?;
        c.execute("INSERT INTO oauth_channel_bindings(channel_id,account_id,reasoning_effort,service_tier,max_input_tokens,max_output_tokens) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(channel_id) DO UPDATE SET account_id=excluded.account_id,reasoning_effort=excluded.reasoning_effort,service_tier=excluded.service_tier,max_input_tokens=excluded.max_input_tokens,max_output_tokens=excluded.max_output_tokens",params![channel_id,binding.account_id,binding.reasoning_effort,binding.service_tier,binding.max_input_tokens,binding.max_output_tokens])?;
        db::identity::audit(
            c,
            None,
            "oauth.bind",
            "channel",
            channel_id,
            serde_json::json!({"account_id":binding.account_id}),
        )?;
        Ok(())
    }))
}
pub fn unbind(state: &SharedState, channel_id: i64) -> Result<()> {
    let c = state.conn();
    storage(db::in_tx(&c, |c| {
        c.execute(
            "DELETE FROM oauth_channel_bindings WHERE channel_id=?1",
            [channel_id],
        )?;
        db::identity::audit(
            c,
            None,
            "oauth.unbind",
            "channel",
            channel_id,
            serde_json::json!({}),
        )?;
        Ok(())
    }))
}
fn binding(state: &SharedState, channel: i64) -> Result<Option<(Binding, i64)>> {
    storage(state.conn().query_row("SELECT b.account_id,b.reasoning_effort,b.service_tier,b.max_input_tokens,b.max_output_tokens,c.binding_generation FROM oauth_channel_bindings b JOIN channels c ON c.id=b.channel_id WHERE b.channel_id=?1",[channel],|r|Ok((Binding{account_id:r.get(0)?,reasoning_effort:r.get(1)?,service_tier:r.get(2)?,max_input_tokens:r.get(3)?,max_output_tokens:r.get(4)?},r.get(5)?))).optional())
}
/// Independent, explicitly verified source context bounds. Missing pair stays unknown; never invent a zero price.
pub fn context_bounds(c: &rusqlite::Connection, channel_id: i64) -> Result<Option<(i64, i64)>> {
    let row:Option<(Option<i64>,Option<i64>)>=storage(c.query_row("SELECT max_input_tokens,max_output_tokens FROM oauth_channel_bindings WHERE channel_id=?1",[channel_id],|r|Ok((r.get(0)?,r.get(1)?))).optional())?;
    Ok(row.and_then(|(input, output)| input.zip(output)))
}
pub fn binding_metadata(state: &SharedState, channel_id: i64) -> Result<Option<(Binding, i64)>> {
    binding(state, channel_id)
}
/// No channel secret is consulted. Re-check binding after any awaited refresh.
pub async fn credentials(state: &SharedState, channel_id: i64) -> Result<Option<OAuthCredential>> {
    let Some((initial, _)) = binding(state, channel_id)? else {
        return Ok(None);
    };
    // OAuth refresh is a credential transaction, not a model request. Dropping the
    // waiting request must not cancel a rotating token after the provider consumed it.
    ensure_session(state, initial.account_id).await?;
    let Some((current, generation)) = binding(state, channel_id)? else {
        return Err(OAuthError("oauth_binding_changed"));
    };
    if current.account_id != initial.account_id {
        return Err(OAuthError("oauth_binding_changed"));
    }
    let a = account(&state.conn(), current.account_id)?;
    if a.status != "active" || a.expires_at <= db::now_ms() {
        return Err(OAuthError("oauth_unavailable"));
    }
    let caps = storage(crate::protocol_config::load(&state.conn(), channel_id))?;
    if caps.native != vec![Protocol::Responses] || caps.native_input_audio {
        return Err(OAuthError("oauth_invalid_source"));
    }
    if !storage(clean_source(&state.conn(), channel_id))? {
        return Err(OAuthError("oauth_invalid_source"));
    }
    let bundle = tokens(state, a.id)?;
    Ok(Some(OAuthCredential {
        access_token: bundle.access_token,
        account_id: a.id,
        credential_revision: a.credential_revision,
        binding_generation: generation,
        expires_at: a.expires_at,
        api_base: state.oauth.api_base(),
        provider: Protocol::Responses,
        reasoning_effort: current.reasoning_effort,
        service_tier: current.service_tier,
        max_input_tokens: current.max_input_tokens,
        max_output_tokens: current.max_output_tokens,
    }))
}
async fn ensure_session(state: &SharedState, account_id: i64) -> Result<()> {
    let worker_state = state.clone();
    tokio::spawn(async move {
        let lock = worker_state.oauth.lock(account_id);
        let _guard = lock.lock().await;
        refresh_if_due(&worker_state, account_id).await
    })
    .await
    .map_err(|_| OAuthError("oauth_unavailable"))?
}
/// Account-specific official model catalog. No token or arbitrary endpoint is accepted from callers.
pub async fn models(state: &SharedState, account_id: i64) -> Result<serde_json::Value> {
    ensure_session(state, account_id).await?;
    let bearer = tokens(state, account_id)?.access_token;
    let response = state
        .oauth
        .client
        .get(format!(
            "{}/models",
            state.oauth.api_base().trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .send()
        .await
        .map_err(|_| OAuthError("oauth_unavailable"))?;
    if !response.status().is_success() {
        return Err(OAuthError("oauth_models_unavailable"));
    }
    let catalog = bounded_json(response, 1 << 20).await?;
    if catalog["object"] != "list"
        || !catalog["data"].as_array().is_some_and(|models| {
            models.len() <= 4096
                && models.iter().all(|model| {
                    model["id"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty() && id.len() <= 512)
                })
        })
    {
        return Err(OAuthError("oauth_invalid_catalog"));
    }
    fn secret(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Object(m) => m.iter().any(|(k, v)| {
                matches!(
                    k.to_ascii_lowercase().as_str(),
                    "access_token" | "refresh_token" | "id_token" | "api_key" | "authorization"
                ) || secret(v)
            }),
            serde_json::Value::Array(items) => items.iter().any(secret),
            _ => false,
        }
    }
    if secret(&catalog) {
        return Err(OAuthError("oauth_invalid_catalog"));
    }
    Ok(catalog)
}
async fn refresh_if_due(state: &SharedState, id: i64) -> Result<()> {
    let began = tokio::time::Instant::now();
    loop {
        let a = account(&state.conn(), id)?;
        let now = db::now_ms();
        if a.status != "active" {
            return Err(OAuthError("oauth_unavailable"));
        }
        if a.expires_at > now + 60_000 || (a.expires_at > now && now < a.earliest_refresh_at) {
            return Ok(());
        }
        if now < a.earliest_refresh_at {
            return Err(OAuthError("oauth_refresh_too_early"));
        }
        let owner = uuid::Uuid::new_v4().to_string();
        let claimed=storage(state.conn().execute("UPDATE oauth_accounts SET refresh_owner=?2,refresh_lease_until=?3 WHERE id=?1 AND status='active' AND credential_revision=?4 AND refresh_lease_until<=?5",params![id,owner,now+90_000,a.credential_revision,now]))?;
        if claimed == 0 {
            if began.elapsed() > Duration::from_secs(25) {
                return Err(OAuthError("oauth_unavailable"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        let result = refresh_claimed(state, &a, &owner).await;
        storage(state.conn().execute("UPDATE oauth_accounts SET refresh_owner=NULL,refresh_lease_until=0 WHERE id=?1 AND refresh_owner=?2",params![id,owner]))?;
        return result;
    }
}
async fn refresh_claimed(state: &SharedState, a: &Account, owner: &str) -> Result<()> {
    let old = tokens(state, a.id)?;
    let reply = match exchange(
        state,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &a.client_id),
            ("refresh_token", &old.refresh_token),
            ("resource", API_BASE),
        ],
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            if e.0 == "oauth_reauthorization_required" {
                storage(state.conn().execute("UPDATE oauth_accounts SET status='reauth_required',nonce=NULL,cipher=NULL,credential_revision=credential_revision+1 WHERE id=?1 AND credential_revision=?2 AND refresh_owner=?3 AND status='active'",params![a.id,a.credential_revision,owner]))?;
            }
            return Err(e);
        }
    };
    if let Some(id) = reply.id_token.as_deref() {
        let claims = verify(state, id, &a.client_id, None).await?;
        if claims.sub != a.subject {
            return Err(OAuthError("oauth_account_mismatch"));
        }
    }
    let (bundle, expiry, earliest) = checked_reply(reply, Some(&old))?;
    let (n, b) = seal(state, &bundle)?;
    let changed=storage(state.conn().execute("UPDATE oauth_accounts SET nonce=?2,cipher=?3,expires_at=?4,earliest_refresh_at=?5,scopes_json=?6,credential_revision=credential_revision+1,refresh_owner=NULL,refresh_lease_until=0 WHERE id=?1 AND status='active' AND credential_revision=?7 AND refresh_owner=?8",params![a.id,n,b,expiry,earliest,storage(serde_json::to_string(&bundle.scopes))?,a.credential_revision,owner]))?;
    if changed != 1 {
        return Err(OAuthError("oauth_session_changed"));
    }
    Ok(())
}
#[derive(Serialize)]
pub struct Revoked {
    pub account_id: i64,
    pub revoked: bool,
    pub remote_revocation_confirmed: bool,
}
pub async fn revoke(state: &SharedState, id: i64) -> Result<Revoked> {
    let (client, token) = {
        let c = state.conn();
        let a = account(&c, id)?;
        let pair: Option<(Vec<u8>, Vec<u8>)> = storage(
            c.query_row(
                "SELECT nonce,cipher FROM oauth_accounts WHERE id=?1 AND cipher IS NOT NULL",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional(),
        )?;
        let token = if let Some((n, b)) = pair {
            let t: Tokens = storage(serde_json::from_str(&storage(state.secrets.open(&n, &b))?))?;
            Some(t.refresh_token)
        } else {
            None
        };
        storage(db::in_tx(&c, |c| {
            c.execute("UPDATE oauth_accounts SET status='revoked',nonce=NULL,cipher=NULL,refresh_owner=NULL,refresh_lease_until=0,credential_revision=credential_revision+1 WHERE id=?1",[id])?;
            db::identity::audit(
                c,
                None,
                "oauth.revoke",
                "oauth_account",
                id,
                serde_json::json!({}),
            )?;
            Ok(())
        }))?;
        (a.client_id, token)
    };
    let confirmed = if let Some(token) = token {
        remote_revoke(state, &client, &token).await.unwrap_or(false)
    } else {
        false
    };
    Ok(Revoked {
        account_id: id,
        revoked: true,
        remote_revocation_confirmed: confirmed,
    })
}
async fn remote_revoke(state: &SharedState, client: &str, token: &str) -> Result<bool> {
    let response = state
        .oauth
        .client
        .get(state.oauth.endpoints().discovery)
        .send()
        .await
        .map_err(|_| OAuthError("oauth_unavailable"))?;
    if !response.status().is_success() {
        return Ok(false);
    }
    let document = json_body(response).await?;
    if document["issuer"] != ISSUER {
        return Ok(false);
    }
    let endpoint = document["revocation_endpoint"]
        .as_str()
        .ok_or(OAuthError("oauth_invalid_result"))?;
    let parsed = reqwest::Url::parse(endpoint).map_err(|_| OAuthError("oauth_invalid_result"))?;
    let allowed = parsed.scheme() == "https"
        && parsed.host_str() == Some("auth.openai.com")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.port_or_known_default() == Some(443);
    #[cfg(test)]
    let allowed = allowed
        || endpoint.starts_with(
            &state
                .oauth
                .endpoints()
                .discovery
                .trim_end_matches("/discovery")
                .to_string(),
        );
    if !allowed {
        return Ok(false);
    }
    for attempt in 0..2 {
        match state
            .oauth
            .client
            .post(endpoint)
            .form(&[
                ("token", token),
                ("token_type_hint", "refresh_token"),
                ("client_id", client),
            ])
            .send()
            .await
        {
            Ok(r) if r.status() == reqwest::StatusCode::OK => return Ok(true),
            Ok(r) if !r.status().is_server_error() => return Ok(false),
            _ => {
                if attempt == 0 {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State,
        routing::{get, post},
        Form, Json, Router,
    };
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    const RSA: &[u8] = include_bytes!("../tests/fixtures/oauth-test-only-rsa.pem");
    const JWKS: &str = include_str!("../tests/fixtures/oauth-test-only-jwks.json");
    struct Mock {
        nonce: Mutex<String>,
        subject: Mutex<String>,
        issued: AtomicUsize,
        refreshes: AtomicUsize,
        refresh_inputs: Mutex<Vec<String>>,
        variant: Mutex<String>,
        base: Mutex<String>,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        hold: std::sync::atomic::AtomicBool,
    }
    impl Default for Mock {
        fn default() -> Self {
            Self {
                nonce: Mutex::new(String::new()),
                subject: Mutex::new("subject-A".into()),
                issued: AtomicUsize::new(0),
                refreshes: AtomicUsize::new(0),
                refresh_inputs: Mutex::new(vec![]),
                variant: Mutex::new(String::new()),
                base: Mutex::new(String::new()),
                entered: Default::default(),
                release: Default::default(),
                hold: Default::default(),
            }
        }
    }
    fn signed(mock: &Mock, nonce: bool) -> String {
        let variant = mock.variant.lock().unwrap().clone();
        let now = db::now_ms() / 1000;
        let mut claims = json!({"iss":ISSUER,"aud":"oaiapp_test","sub":mock.subject.lock().unwrap().clone(),"iat":now,"exp":now+3600,"email":"synthetic@example.invalid"});
        if nonce {
            claims["nonce"] = json!(mock.nonce.lock().unwrap().clone());
        }
        match variant.as_str() {
            "issuer" => claims["iss"] = json!("https://wrong.invalid"),
            "audience" => claims["aud"] = json!("other-client"),
            "expiry" => claims["exp"] = json!(now - 60),
            "nonce" => claims["nonce"] = json!("wrong"),
            "subject" => claims["sub"] = json!(""),
            _ => {}
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("tokengo-test-only".into());
        if variant == "signature" {
            return jsonwebtoken::encode(
                &Header::new(Algorithm::HS256),
                &claims,
                &EncodingKey::from_secret(b"synthetic"),
            )
            .unwrap();
        }
        encode(&header, &claims, &EncodingKey::from_rsa_pem(RSA).unwrap()).unwrap()
    }
    async fn token(
        State(mock): State<Arc<Mock>>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Json<Value> {
        assert_eq!(form["client_id"], "oaiapp_test");
        assert_eq!(form["resource"], API_BASE);
        let refresh = form["grant_type"] == "refresh_token";
        if refresh {
            assert!(!form.contains_key("scope"));
            mock.refreshes.fetch_add(1, Ordering::SeqCst);
            mock.refresh_inputs
                .lock()
                .unwrap()
                .push(form["refresh_token"].clone());
            if mock.hold.load(Ordering::SeqCst) {
                mock.entered.notify_one();
                mock.release.notified().await;
            }
        } else {
            assert_eq!(form["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
            assert!(form["code_verifier"].len() >= 43);
        }
        let n = mock.issued.fetch_add(1, Ordering::SeqCst);
        Json(
            json!({"access_token":format!("SYNTHETIC-ACCESS-{n}"),"refresh_token":format!("SYNTHETIC-ROTATING-REFRESH-{n}"),"id_token":signed(&mock,!refresh),"token_type":"Bearer","expires_in":3600,"earliest_refresh_at":db::now_ms()/1000,"scope":SCOPE}),
        )
    }
    async fn catalog(headers: axum::http::HeaderMap) -> Json<Value> {
        assert!(headers
            .get("Authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("Bearer SYNTHETIC-ACCESS-"));
        Json(
            json!({"object":"list","data":[{"id":"gpt-synthetic","context_window":32768,"max_output_tokens":4096}]}),
        )
    }
    async fn jwks() -> Json<Value> {
        Json(serde_json::from_str(JWKS).unwrap())
    }
    async fn discovery(State(mock): State<Arc<Mock>>) -> Json<Value> {
        Json(
            json!({"issuer":ISSUER,"revocation_endpoint":format!("{}/revoke",mock.base.lock().unwrap())}),
        )
    }
    async fn revocation(Form(form): Form<HashMap<String, String>>) -> axum::http::StatusCode {
        assert_eq!(form["client_id"], "oaiapp_test");
        assert_eq!(form["token_type_hint"], "refresh_token");
        assert!(form["token"].starts_with("SYNTHETIC-ROTATING"));
        axum::http::StatusCode::OK
    }
    struct Fixture {
        state: SharedState,
        mock: Arc<Mock>,
        task: tokio::task::JoinHandle<()>,
        dir: std::path::PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }
    impl Fixture {
        async fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("tokengo-oauth-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let state = crate::AppState::init(crate::Config {
                listen: "127.0.0.1:0".into(),
                db_path: dir.join("test.db"),
                master_key_path: dir.join("master").to_string_lossy().into(),
                web_dir: None,
                log_dir: None,
            })
            .unwrap();
            let mock = Arc::new(Mock::default());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            *mock.base.lock().unwrap() = base.clone();
            state.oauth.test_endpoints(&base);
            let router = Router::new()
                .route("/token", post(token))
                .route("/jwks", get(jwks))
                .route("/v1/models", get(catalog))
                .route("/discovery", get(discovery))
                .route("/revoke", post(revocation))
                .with_state(mock.clone());
            let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            Self {
                state,
                mock,
                task,
                dir,
            }
        }
        fn begin(&self, selected: Option<i64>) -> (Started, Completion) {
            let started = start(
                &self.state,
                "Synthetic account".into(),
                "http://127.0.0.1:1455/auth/callback".into(),
                selected,
            )
            .unwrap();
            let url = reqwest::Url::parse(&started.authorization_url).unwrap();
            let q: HashMap<_, _> = url
                .query_pairs()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            *self.mock.nonce.lock().unwrap() = q["nonce"].clone();
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["resource"], API_BASE);
            if selected.is_none() {
                assert_eq!(q["client_id"], "dynamic_agent_client");
                assert_eq!(q["agent_name_hint"], "Tokengo");
            } else {
                assert_eq!(q["client_id"], "oaiapp_test");
                assert!(!q.contains_key("agent_name_hint"));
            }
            let result = Completion {
                attempt_id: started.attempt_id.clone(),
                state: q["state"].clone(),
                code: Some("SYNTHETIC-CODE".into()),
                client_id: Some("oaiapp_test".into()),
                error: None,
            };
            (started, result)
        }
        async fn authorize(&self) -> Account {
            let (_, result) = self.begin(None);
            complete(&self.state, result).await.unwrap()
        }
        fn channel(&self, a: i64) -> i64 {
            let c = self.state.conn();
            let id = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "oauth-source".into(),
                    base_url: API_BASE.into(),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap()
            .0;
            drop(c);
            bind(
                &self.state,
                id,
                &Binding {
                    account_id: a,
                    reasoning_effort: Some("low".into()),
                    service_tier: Some("priority".into()),
                    max_input_tokens: Some(32768),
                    max_output_tokens: Some(4096),
                },
            )
            .unwrap();
            id
        }
        fn due(&self, a: i64) {
            self.state
                .conn()
                .execute(
                    "UPDATE oauth_accounts SET expires_at=?2,earliest_refresh_at=0 WHERE id=?1",
                    params![a, db::now_ms() + 100],
                )
                .unwrap();
        }
    }
    #[test]
    fn token_reply_rejects_header_unsafe_access_and_control_refresh_tokens() {
        for (access, refresh) in [
            ("has space", "refresh"),
            ("bad\r\nheader", "refresh"),
            ("safe-access", "bad\nrefresh"),
        ] {
            let reply = TokenReply {
                access_token: access.into(),
                refresh_token: refresh.into(),
                id_token: Some("synthetic".into()),
                token_type: "Bearer".into(),
                expires_in: 3600,
                scope: Some(SCOPE.into()),
                earliest_refresh_at: None,
            };
            assert!(checked_reply(reply, None).is_err());
        }
    }
    #[tokio::test]
    async fn registration_persists_encrypted_tokens_stable_host_and_once_only_state() {
        let f = Fixture::new().await;
        let (started, result) = f.begin(None);
        let saved_host = started.host_id.clone();
        let duplicate = Completion {
            attempt_id: result.attempt_id.clone(),
            state: result.state.clone(),
            code: result.code.clone(),
            client_id: result.client_id.clone(),
            error: None,
        };
        let a = complete(&f.state, result).await.unwrap();
        assert_eq!(a.subject, "subject-A");
        assert!(complete(&f.state, duplicate).await.is_err());
        assert_eq!(f.mock.issued.load(Ordering::SeqCst), 1);
        let again = f.begin(Some(a.id)).0;
        assert_eq!(saved_host, again.host_id);
        assert!(again.authorization_url.contains("id_token_hint="));
        let report = serde_json::to_string(&accounts(&f.state).unwrap()).unwrap();
        assert!(!report.contains("SYNTHETIC-ACCESS") && !report.contains("SYNTHETIC-ROTATING"));
        let c = f.state.conn();
        let value: (Vec<u8>, Vec<u8>) = c
            .query_row("SELECT nonce,cipher FROM oauth_accounts", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert!(!String::from_utf8_lossy(&value.1).contains("SYNTHETIC"));
        let clear = f.state.secrets.open(&value.0, &value.1).unwrap();
        assert!(clear.contains("SYNTHETIC-ACCESS-0"));
    }
    #[tokio::test]
    async fn authorization_audit_failure_rolls_back_credentials_atomically() {
        let f = Fixture::new().await;
        f.state.conn().execute_batch("CREATE TRIGGER deny_oauth_audit BEFORE INSERT ON audit_events WHEN NEW.action='oauth.authorize' BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END;").unwrap();
        let (_, result) = f.begin(None);
        assert!(complete(&f.state, result).await.is_err());
        assert!(accounts(&f.state).unwrap().is_empty());
        let count: i64 = f
            .state
            .conn()
            .query_row(
                "SELECT count(*) FROM oauth_pending WHERE consumed=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
    #[tokio::test]
    async fn rejects_jwt_signature_issuer_audience_expiry_nonce_subject_and_wrong_account() {
        for variant in [
            "signature",
            "issuer",
            "audience",
            "expiry",
            "nonce",
            "subject",
        ] {
            let f = Fixture::new().await;
            *f.mock.variant.lock().unwrap() = variant.into();
            let (_, result) = f.begin(None);
            assert!(complete(&f.state, result).await.is_err(), "{variant}");
            assert!(accounts(&f.state).unwrap().is_empty());
        }
        let f = Fixture::new().await;
        let a = f.authorize().await;
        *f.mock.subject.lock().unwrap() = "subject-B".into();
        let (_, result) = f.begin(Some(a.id));
        assert_eq!(
            complete(&f.state, result).await.unwrap_err().0,
            "oauth_account_mismatch"
        );
        assert_eq!(
            tokens(&f.state, a.id).unwrap().access_token,
            "SYNTHETIC-ACCESS-0"
        );
    }
    #[tokio::test]
    async fn restart_and_independent_runtimes_share_refresh_cas_and_model_catalog() {
        let f = Fixture::new().await;
        let a = f.authorize().await;
        let ch = f.channel(a.id);
        let fresh = crate::AppState::init(f.state.cfg.clone()).unwrap();
        fresh.oauth.test_endpoints(&f.mock.base.lock().unwrap());
        fresh
            .oauth
            .test_api_base(&format!("{}/v1", f.mock.base.lock().unwrap()));
        assert_eq!(
            host_id(&fresh.conn()).unwrap(),
            host_id(&f.state.conn()).unwrap()
        );
        assert_eq!(accounts(&fresh).unwrap()[0].subject, "subject-A");
        f.due(a.id);
        let mut jobs = vec![];
        for i in 0..8 {
            let state = if i % 2 == 0 {
                fresh.clone()
            } else {
                f.state.clone()
            };
            jobs.push(tokio::spawn(async move {
                credentials(&state, ch).await.unwrap().unwrap().access_token
            }));
        }
        for job in jobs {
            assert_eq!(job.await.unwrap(), "SYNTHETIC-ACCESS-1");
        }
        assert_eq!(f.mock.refreshes.load(Ordering::SeqCst), 1);
        let value = models(&fresh, a.id).await.unwrap();
        assert_eq!(value["data"][0]["context_window"], 32768);
        assert!(!value.to_string().contains("SYNTHETIC-ACCESS"));
    }
    #[tokio::test]
    async fn callback_validation_state_and_denial_never_exchange_or_overwrite() {
        let f = Fixture::new().await;
        for bad in [
            "http://localhost:1455/auth/callback",
            "http://127.0.0.1:1455/callback",
            "https://127.0.0.1:1455/auth/callback",
            "http://0.0.0.0:1455/auth/callback",
        ] {
            assert!(start(&f.state, "test".into(), bad.into(), None).is_err());
        }
        let (_, mut r) = f.begin(None);
        r.state = "wrong".into();
        assert_eq!(
            complete(&f.state, r).await.unwrap_err().0,
            "oauth_invalid_state"
        );
        let (_, mut r) = f.begin(None);
        r.error = Some("access_denied".into());
        assert_eq!(
            complete(&f.state, r).await.unwrap_err().0,
            "oauth_authorization_denied"
        );
        assert_eq!(f.mock.issued.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn rotating_refresh_is_serialized_persisted_and_does_not_change_binding_owner() {
        let f = Fixture::new().await;
        let a = f.authorize().await;
        let ch = f.channel(a.id);
        let generation = binding(&f.state, ch).unwrap().unwrap().1;
        f.due(a.id);
        let mut jobs = vec![];
        for _ in 0..8 {
            let state = f.state.clone();
            jobs.push(tokio::spawn(async move {
                credentials(&state, ch).await.unwrap().unwrap().access_token
            }));
        }
        for job in jobs {
            assert_eq!(job.await.unwrap(), "SYNTHETIC-ACCESS-1");
        }
        assert_eq!(f.mock.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(binding(&f.state, ch).unwrap().unwrap().1, generation);
        f.due(a.id);
        assert_eq!(
            credentials(&f.state, ch)
                .await
                .unwrap()
                .unwrap()
                .access_token,
            "SYNTHETIC-ACCESS-2"
        );
        assert_eq!(
            *f.mock.refresh_inputs.lock().unwrap(),
            vec![
                "SYNTHETIC-ROTATING-REFRESH-0",
                "SYNTHETIC-ROTATING-REFRESH-1"
            ]
        );
        let c = f.state.conn();
        assert_eq!(context_bounds(&c, ch).unwrap(), Some((32768, 4096)));
        assert_eq!(account(&c, a.id).unwrap().credential_revision, 3);
    }
    #[tokio::test]
    async fn expired_too_early_refresh_and_cross_subject_refresh_fail_closed() {
        let f = Fixture::new().await;
        let a = f.authorize().await;
        let ch = f.channel(a.id);
        {
            let c = f.state.conn();
            c.execute(
                "UPDATE oauth_accounts SET expires_at=?2,earliest_refresh_at=?3 WHERE id=?1",
                params![a.id, db::now_ms() - 1000, db::now_ms() + 300000],
            )
            .unwrap();
        }
        assert_eq!(
            credentials(&f.state, ch).await.err().unwrap().0,
            "oauth_refresh_too_early"
        );
        assert_eq!(f.mock.refreshes.load(Ordering::SeqCst), 0);
        f.due(a.id);
        *f.mock.subject.lock().unwrap() = "other-owner".into();
        assert_eq!(
            credentials(&f.state, ch).await.err().unwrap().0,
            "oauth_account_mismatch"
        );
        assert_eq!(
            tokens(&f.state, a.id).unwrap().access_token,
            "SYNTHETIC-ACCESS-0"
        );
    }
    #[tokio::test]
    async fn revocation_during_refresh_prevents_stale_cas_and_bumps_bound_generation() {
        let f = Fixture::new().await;
        let a = f.authorize().await;
        let ch = f.channel(a.id);
        let before = binding(&f.state, ch).unwrap().unwrap().1;
        f.due(a.id);
        f.mock.hold.store(true, Ordering::SeqCst);
        let state = f.state.clone();
        let job = tokio::spawn(async move { credentials(&state, ch).await });
        f.mock.entered.notified().await;
        let revoked = revoke(&f.state, a.id).await.unwrap();
        assert!(revoked.remote_revocation_confirmed);
        f.mock.release.notify_one();
        assert!(job.await.unwrap().is_err());
        assert!(credentials(&f.state, ch).await.is_err());
        assert!(binding(&f.state, ch).unwrap().unwrap().1 > before);
        assert!(tokens(&f.state, a.id).is_err());
    }
    #[tokio::test]
    async fn cancelling_refresh_waiter_still_persists_rotated_pair() {
        let f = Fixture::new().await;
        let a = f.authorize().await;
        let ch = f.channel(a.id);
        f.due(a.id);
        f.mock.hold.store(true, Ordering::SeqCst);
        let state = f.state.clone();
        let waiter = tokio::spawn(async move { credentials(&state, ch).await });
        f.mock.entered.notified().await;
        waiter.abort();
        assert!(matches!(waiter.await,Err(error) if error.is_cancelled()));
        f.mock.release.notify_one();
        for _ in 0..100 {
            if account(&f.state.conn(), a.id).unwrap().credential_revision == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            account(&f.state.conn(), a.id).unwrap().credential_revision,
            2
        );
        assert_eq!(
            tokens(&f.state, a.id).unwrap().refresh_token,
            "SYNTHETIC-ROTATING-REFRESH-1"
        );
        assert_eq!(
            credentials(&f.state, ch)
                .await
                .unwrap()
                .unwrap()
                .access_token,
            "SYNTHETIC-ACCESS-1"
        );
        assert_eq!(f.mock.refreshes.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn drop_fields_are_rejected_at_bind_and_live_credential_revalidation() {
        let f = Fixture::new().await;
        let a = f.authorize().await;
        let ch = f.channel(a.id);
        {
            let c = f.state.conn();
            let mut mapping = db::channels::ModelMapping::new("gpt-test", None, false);
            mapping.drop_fields = vec!["max_output_tokens".into()];
            db::channels::set_models(&c, tokengo_shared::ChannelId(ch), &[mapping]).unwrap();
        }
        assert_eq!(
            credentials(&f.state, ch).await.err().unwrap().0,
            "oauth_invalid_source"
        );
        unbind(&f.state, ch).unwrap();
        let b = Binding {
            account_id: a.id,
            reasoning_effort: None,
            service_tier: None,
            max_input_tokens: Some(32768),
            max_output_tokens: Some(4096),
        };
        assert!(bind(&f.state, ch, &b).is_err());
        assert!(binding(&f.state, ch).unwrap().is_none());
        {
            let c = f.state.conn();
            db::channels::set_models(
                &c,
                tokengo_shared::ChannelId(ch),
                &[db::channels::ModelMapping::new("gpt-test", None, false)],
            )
            .unwrap();
        }
        bind(&f.state, ch, &b).unwrap();
        assert!(credentials(&f.state, ch).await.unwrap().is_some());
    }
    #[tokio::test]
    async fn bindings_reject_old_secrets_wrong_hosts_and_invalid_defaults_and_track_changes() {
        let f = Fixture::new().await;
        let a = f.authorize().await;
        let ch = f.channel(a.id);
        let before = binding(&f.state, ch).unwrap().unwrap().1;
        let b = Binding {
            account_id: a.id,
            reasoning_effort: Some("high".into()),
            service_tier: None,
            max_input_tokens: Some(65536),
            max_output_tokens: Some(4096),
        };
        bind(&f.state, ch, &b).unwrap();
        assert!(binding(&f.state, ch).unwrap().unwrap().1 > before);
        let generation = binding(&f.state, ch).unwrap().unwrap().1;
        bind(&f.state, ch, &b).unwrap();
        assert_eq!(binding(&f.state, ch).unwrap().unwrap().1, generation);
        let mut bad = b.clone();
        bad.max_input_tokens = Some(0);
        assert!(bind(&f.state, ch, &bad).is_err());
        {
            let c = f.state.conn();
            c.execute(
                "UPDATE channels SET base_url='https://untrusted.invalid/v1' WHERE id=?1",
                [ch],
            )
            .unwrap();
        }
        assert!(bind(&f.state, ch, &b).is_err());
        {
            let c = f.state.conn();
            c.execute(
                "UPDATE channels SET base_url=?2 WHERE id=?1",
                params![ch, API_BASE],
            )
            .unwrap();
            c.execute("INSERT INTO channel_headers(channel_id,name,value,ordinal) VALUES(?1,'Authorization','SYNTHETIC-OLD-KEY',0)",[ch]).unwrap();
        }
        assert!(bind(&f.state, ch, &b).is_err());
        unbind(&f.state, ch).unwrap();
        assert!(credentials(&f.state, ch).await.unwrap().is_none());
    }
}
