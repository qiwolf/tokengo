//! Source-bound public-client device OAuth. Native CLI identities are never impersonated.
//! Each deployment supplies its own provider-issued client ID. No client secret is embedded.
use crate::{db, oauth::{Account, Binding, OAuthCredential, OAuthError, Revoked}, protocol::Protocol, SharedState};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

type Result<T> = std::result::Result<T, OAuthError>;
fn store<T>(r: std::result::Result<T, impl std::fmt::Debug>) -> Result<T> { r.map_err(|_|OAuthError("oauth_storage_unavailable")) }
fn invalid<T>() -> Result<T> { Err(OAuthError("oauth_invalid_result")) }
fn random() -> String { let mut b=[0u8;32];rand::rngs::OsRng.fill_bytes(&mut b);URL_SAFE_NO_PAD.encode(b) }
fn client(state:&SharedState,p:ProviderSpec,account_id:Option<i64>,origin:Option<i64>)->Result<reqwest::Client> {
    crate::egress::oauth_client(state,p.id,account_id,origin).map_err(|_|OAuthError("oauth_egress_unavailable"))
}
#[derive(Clone,Copy)]
pub struct ProviderSpec {
    pub id: &'static str, pub label: &'static str, pub issuer: &'static str,
    pub api_base: &'static str, pub native_protocol: Protocol,
    pub families: &'static [&'static str], pub docs_url: &'static str,
    client_env: &'static str, device_path: &'static str, token_path: &'static str,
    scope: &'static str, audience: &'static str, pub inference_available: bool,
}
pub fn provider(id:&str)->Option<ProviderSpec> {
    Some(match id {
        "kimi"=>ProviderSpec{id:"kimi",label:"Kimi Code",issuer:"https://auth.kimi.com",api_base:"https://api.kimi.com/coding/v1",native_protocol:Protocol::Chat,families:&["kimi"],docs_url:"https://www.kimi.com/code/docs/en/kimi-code-cli/guides/getting-started.html",client_env:"TOKENGO_KIMI_OAUTH_CLIENT_ID",device_path:"/api/oauth/device_authorization",token_path:"/api/oauth/token",scope:"",audience:"",inference_available:true},
        "minimax-cn"=>ProviderSpec{id:"minimax-cn",label:"MiniMax Code · 中国",issuer:"https://account.minimax.cn",api_base:"https://agent.minimax.cn/mavis/api/v1/llm/v1",native_protocol:Protocol::Messages,families:&["minimax"],docs_url:"https://github.com/MiniMax-AI/minimax-code",client_env:"TOKENGO_MINIMAX_CN_OAUTH_CLIENT_ID",device_path:"/oauth2/device/code",token_path:"/oauth2/token",scope:"agent.default",audience:"agent-backend",inference_available:true},
        "minimax-global"=>ProviderSpec{id:"minimax-global",label:"MiniMax Code · Global",issuer:"https://account.minimax.io",api_base:"https://agent.minimax.io/mavis/api/v1/llm/v1",native_protocol:Protocol::Messages,families:&["minimax"],docs_url:"https://github.com/MiniMax-AI/minimax-code",client_env:"TOKENGO_MINIMAX_GLOBAL_OAUTH_CLIENT_ID",device_path:"/oauth2/device/code",token_path:"/oauth2/token",scope:"agent.default",audience:"agent-backend",inference_available:true},
        "copilot"=>ProviderSpec{id:"copilot",label:"GitHub Copilot",issuer:"https://github.com",api_base:"",native_protocol:Protocol::Chat,families:&["gpt","claude","gemini","kimi","other"],docs_url:"https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli",client_env:"TOKENGO_GITHUB_OAUTH_CLIENT_ID",device_path:"/login/device/code",token_path:"/login/oauth/access_token",scope:"read:user",audience:"",inference_available:false},
        _=>return None,
    })
}
fn registered_client(p:ProviderSpec)->Result<String> {
    let value=std::env::var(p.client_env).map_err(|_|OAuthError("oauth_client_registration_required"))?;
    if value.is_empty()||value.len()>256||value.chars().any(|c|c.is_control()||c.is_whitespace()) { return Err(OAuthError("oauth_client_registration_required")); }
    // The published CLI IDs identify those CLI applications, not TokenGo.
    if matches!(value.as_str(),"17e5f671-d194-4dfb-9706-5516cb48c098"|"mcode-public"|"Iv1.b507a08c87ecfe98"|"01ab8ac9400c4e429b23") { return Err(OAuthError("oauth_independent_client_required")); }
    Ok(value)
}
pub fn providers()->Value {
    let mut rows=vec![json!({"id":"openai","label":"ChatGPT / Codex","families":["gpt"],"available":true,"authentication_available":true,"inference_available":true,"reason":"existing_independent_registration","api_base":crate::oauth::API_BASE,"native_protocol":"responses","method":"browser_pkce","docs_url":"https://learn.chatgpt.com/docs/auth"})];
    for id in ["kimi","minimax-cn","minimax-global","copilot"] {
        let p=provider(id).unwrap();let configured=registered_client(p).is_ok();
        rows.push(json!({"id":p.id,"label":p.label,"families":p.families,"available":configured,"authentication_available":configured,"inference_available":configured&&p.inference_available,"reason":if !configured{"oauth_client_registration_required"}else if !p.inference_available{"oauth_transport_not_available"}else{"registered_device_flow"},"configuration_key":p.client_env,"api_base":p.api_base,"native_protocol":p.native_protocol,"method":"device_code","docs_url":p.docs_url}));
    }
    for (id,label,families,reason,url) in [
        ("antigravity","Google Antigravity",vec!["gemini"],"independent_registration_and_transport_required","https://www.antigravity.google/docs/install?tab=cli"),
        ("iflow","iFlow",vec!["qwen","deepseek","glm","kimi","other"],"public_oauth_contract_not_confirmed","https://docs.iflow.cn/cli/quickstart/"),
        ("claude-native","Claude Code",vec!["claude"],"official_client_only_not_gateway_binding","https://code.claude.com/docs/en/legal-and-compliance#authentication-and-credential-use"),
        ("qwen-retired","Qwen OAuth",vec!["qwen"],"upstream_oauth_retired","https://github.com/QwenLM/qwen-code/blob/main/docs/users/quickstart.md")
    ] {rows.push(json!({"id":id,"label":label,"families":families,"available":false,"authentication_available":false,"inference_available":false,"reason":reason,"method":"unavailable","docs_url":url}));}
    json!({"providers":rows})
}
pub fn account_provider(c:&rusqlite::Connection,id:i64)->Result<String> {
    let issuer:String=store(c.query_row("SELECT issuer FROM oauth_accounts WHERE id=?1",[id],|r|r.get(0)))?;
    if issuer=="https://auth.openai.com" {return Ok("openai".into());}
    ["kimi","minimax-cn","minimax-global","copilot"].into_iter().find(|id|provider(id).is_some_and(|p|p.issuer==issuer)).map(str::to_owned).ok_or(OAuthError("oauth_invalid_source"))
}
pub fn account_transport(c:&rusqlite::Connection,id:i64)->Result<(String,Protocol,bool)> {
    let id=account_provider(c,id)?;
    if id=="openai" {return Ok((crate::oauth::API_BASE.into(),Protocol::Responses,true));}
    let p=provider(&id).ok_or(OAuthError("oauth_invalid_source"))?;
    if !p.inference_available||p.api_base.is_empty(){return Err(OAuthError("oauth_transport_not_available"));}
    Ok((p.api_base.into(),p.native_protocol,false))
}
pub fn channel_managed_openai(c:&rusqlite::Connection,channel:i64)->Result<bool> {
    let id:Option<i64>=store(c.query_row("SELECT account_id FROM oauth_channel_bindings WHERE channel_id=?1",[channel],|r|r.get(0)).optional())?;
    id.map(|id|account_provider(c,id).map(|p|p=="openai")).transpose().map(|v|v.unwrap_or(false))
}
fn validate_origin(c:&rusqlite::Connection,p:ProviderSpec,origin:Option<i64>)->Result<()> {
    let Some(id)=origin else{return Ok(())};
    let row:Option<(Option<String>,bool,String)>=store(c.query_row("SELECT base_url,enabled,canonical_state FROM provider_channels WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional())?;
    let Some((base,enabled,canonical))=row else{return Err(OAuthError("oauth_invalid_origin_provider"))};
    if !enabled||canonical!="canonical"{return Err(OAuthError("oauth_invalid_origin_provider"));}
    if p.inference_available&&base.as_deref().is_none_or(|v|v.trim_end_matches('/')!=p.api_base){return Err(OAuthError("oauth_provider_source_mismatch"));}
    Ok(())
}
#[derive(Serialize,Deserialize)]
struct DeviceSecret { provider:String,label:String,client_id:String,device_code:String,verifier:String,poll_user_code:bool,#[serde(default)] origin_provider_id:Option<i64> }
#[derive(Serialize,Deserialize)]
struct TokenBundle { provider:String,access_token:String,refresh_token:Option<String>,scopes:Vec<String>,#[serde(default)] origin_provider_id:Option<i64> }
#[derive(Serialize)]
pub struct DeviceStarted {pub attempt_id:String,pub verification_uri:String,pub verification_uri_complete:Option<String>,pub user_code:String,pub expires_at:i64,pub interval:i64}
#[derive(Serialize)]
pub struct DevicePoll {pub status:String,#[serde(skip_serializing_if="Option::is_none")]pub account:Option<Account>,pub interval:i64}
fn status(status:&str,interval:i64)->DevicePoll {DevicePoll{status:status.into(),account:None,interval}}
fn bounded_string(value:&Value,key:&str,max:usize)->Result<String>{value.get(key).and_then(Value::as_str).filter(|v|!v.is_empty()&&v.len()<=max&&!v.chars().any(char::is_control)).map(str::to_owned).ok_or(OAuthError("oauth_invalid_result"))}
async fn json_body(mut response:reqwest::Response)->Result<(u16,Value)> {
    let status=response.status().as_u16();let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await.map_err(|_|OAuthError("oauth_unavailable"))? {if bytes.len()+chunk.len()>131072{return invalid();}bytes.extend_from_slice(&chunk);}
    Ok((status,serde_json::from_slice(&bytes).map_err(|_|OAuthError("oauth_invalid_result"))?))
}
async fn form(state:&SharedState,p:ProviderSpec,path:&str,values:&[(&str,&str)],account:Option<i64>,origin:Option<i64>)->Result<(u16,Value)> {
    let mut request=client(state,p,account,origin)?.post(format!("{}{}",p.issuer,path)).header("Accept","application/json").form(values);
    if p.id=="kimi" {request=request.header("X-Msh-Platform","tokengo").header("X-Msh-Version",env!("CARGO_PKG_VERSION"));}
    json_body(request.send().await.map_err(|_|OAuthError("oauth_unavailable"))?).await
}
fn verification_uri(p:ProviderSpec,value:&str)->Result<String> {
    let u=url::Url::parse(value).map_err(|_|OAuthError("oauth_invalid_result"))?;
    let host=u.host_str().unwrap_or("");let allowed=match p.id {"kimi"=>matches!(host,"auth.kimi.com"|"www.kimi.com"|"kimi.com"),"minimax-cn"=>matches!(host,"account.minimax.cn"),"minimax-global"=>matches!(host,"account.minimax.io"),"copilot"=>host=="github.com",_=>false};
    if u.scheme()!="https"||!allowed||!u.username().is_empty()||u.password().is_some()||u.port().is_some()||u.fragment().is_some()
        ||u.query_pairs().any(|(key,_)|matches!(key.as_ref(),"access_token"|"refresh_token"|"device_code"|"client_secret")) {return invalid();}Ok(value.into())
}
// MiniMax's fixed official normalizer uses relative seconds below 1e12,
// absolute Unix milliseconds above it. Only account polling interval is in ms.
fn account_expiry_seconds(expiry:i64,now:i64)->Option<i64> {
    if expiry<=0{return None;}
    if expiry<1_000_000_000_000{return Some(expiry);}
    let remaining=expiry.checked_sub(now)?;if remaining<=0{return None;}Some(remaining.saturating_add(999)/1000)
}
pub fn cleanup_pending(state:&SharedState)->Result<()> {
    let now=db::now_ms();let c=state.conn();
    store(c.execute("UPDATE oauth_device_pending SET status='expired',nonce=NULL,cipher=NULL,poll_owner=NULL,poll_lease_until=0 WHERE status='pending' AND expires_at<=?1",[now]))?;
    store(c.execute("DELETE FROM oauth_device_pending WHERE status<>'pending' AND expires_at<?1",[now-86_400_000]))?;Ok(())
}
pub async fn start(state:&SharedState,id:&str,label:String,origin_provider_id:Option<i64>)->Result<DeviceStarted> {
    let p=provider(id).ok_or(OAuthError("oauth_provider_not_available"))?;let client_id=registered_client(p)?;
    validate_origin(&state.conn(),p,origin_provider_id)?;
    let label=label.trim().to_owned();if label.is_empty()||label.len()>128||label.chars().any(char::is_control){return Err(OAuthError("oauth_invalid_label"));}
    cleanup_pending(state)?;
    let now=db::now_ms();{
        let c=state.conn();
        let count:i64=store(c.query_row("SELECT count(*) FROM oauth_device_pending WHERE status='pending'",[],|r|r.get(0)))?;if count>=32{return Err(OAuthError("oauth_too_many_attempts"));}
    }
    let verifier=random();let challenge=URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut values=vec![("client_id",client_id.as_str())];
    if !p.scope.is_empty(){values.push(("scope",p.scope));}
    if !p.audience.is_empty(){values.extend([("audience",p.audience),("code_challenge",challenge.as_str()),("code_challenge_method","S256")]);}
    let (code,v)=form(state,p,p.device_path,&values,None,origin_provider_id).await?;if !(200..300).contains(&code){return Err(OAuthError("oauth_device_start_failed"));}
    let user_code=bounded_string(&v,"user_code",256)?;
    let poll_user_code=v.get("device_code").is_none()&&p.id.starts_with("minimax-")&&v.get("expired_in").and_then(Value::as_i64).is_some();
    let device_code=if poll_user_code{user_code.clone()}else{bounded_string(&v,"device_code",4096)?};
    let uri=v.get("verification_uri").or_else(||v.get("verification_url")).and_then(Value::as_str).filter(|s|!s.is_empty())
        .or_else(||if p.id=="kimi"{v.get("verification_uri_complete").and_then(Value::as_str)}else{None}).ok_or(OAuthError("oauth_invalid_result"))?;
    let uri=verification_uri(p,uri)?;
    let complete=v.get("verification_uri_complete").and_then(Value::as_str).map(|s|verification_uri(p,s)).transpose()?;
    let expires=if let Some(seconds)=wire_integer(v.get("expires_in"),p.id=="kimi"){seconds}else if p.id.starts_with("minimax-") {account_expiry_seconds(v["expired_in"].as_i64().unwrap_or(0),now).ok_or(OAuthError("oauth_invalid_result"))?}else{return invalid()};
    if !(1..=3600).contains(&expires){return invalid();}
    let raw_interval=if v.get("interval").is_some(){wire_integer(v.get("interval"),p.id=="kimi").ok_or(OAuthError("oauth_invalid_result"))?}else if poll_user_code{5000}else{5};
    let interval=if poll_user_code{raw_interval.saturating_add(999)/1000}else{raw_interval};
    if !(1..=3600).contains(&interval){return invalid();}
    let expiry=now.checked_add(expires*1000).ok_or(OAuthError("oauth_invalid_result"))?;
    let attempt_id=format!("device_{}",random());let secret=DeviceSecret{provider:p.id.into(),label,client_id,device_code,verifier,poll_user_code,origin_provider_id};
    let (nonce,cipher)=store(state.secrets.seal(&store(serde_json::to_string(&secret))?))?;
    let inserted=store(state.conn().execute("INSERT INTO oauth_device_pending(id,provider,status,expires_at,next_poll_at,interval_ms,nonce,cipher) SELECT ?1,?2,'pending',?3,?4,?5,?6,?7 WHERE (SELECT count(*) FROM oauth_device_pending WHERE status='pending')<32",params![attempt_id,p.id,expiry,now+interval*1000,interval*1000,nonce,cipher]))?;
    if inserted!=1{return Err(OAuthError("oauth_too_many_attempts"));}
    Ok(DeviceStarted{attempt_id,verification_uri:uri,verification_uri_complete:complete,user_code,expires_at:expiry,interval})
}
fn terminal(c:&rusqlite::Connection,id:&str,owner:&str,kind:&str)->Result<()> {
    store(c.execute("UPDATE oauth_device_pending SET status=?3,nonce=NULL,cipher=NULL,poll_owner=NULL,poll_lease_until=0 WHERE id=?1 AND status='pending' AND poll_owner=?2",params![id,owner,kind]))?;Ok(())
}
pub async fn poll(state:&SharedState,id:String)->Result<DevicePoll> {
    if !id.starts_with("device_")||id.len()>128{return invalid();}
    let state=state.clone();tokio::spawn(async move{poll_inner(&state,&id).await}).await.map_err(|_|OAuthError("oauth_unavailable"))?
}
async fn poll_inner(state:&SharedState,id:&str)->Result<DevicePoll> {
    let now=db::now_ms();let owner=random();
    let (secret,interval)= {
        let c=state.conn();
        type Row=(String,i64,i64,i64,Option<Vec<u8>>,Option<Vec<u8>>,Option<i64>);
        let row:Row=store(c.query_row("SELECT status,expires_at,next_poll_at,interval_ms,nonce,cipher,account_id FROM oauth_device_pending WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))))?;
        if row.0=="complete"{return Ok(DevicePoll{status:"complete".into(),account:Some(crate::oauth::account(&c,row.6.ok_or(OAuthError("oauth_invalid_result"))?)?),interval:row.3/1000});}
        if row.0!="pending"{return Ok(status(&row.0,row.3/1000));}
        if row.1<=now{store(c.execute("UPDATE oauth_device_pending SET status='expired',nonce=NULL,cipher=NULL,poll_owner=NULL,poll_lease_until=0 WHERE id=?1 AND status='pending'",[id]))?;return Ok(status("expired",row.3/1000));}
        if row.2>now{return Ok(status("pending",row.3/1000));}
        let claimed=store(c.execute("UPDATE oauth_device_pending SET poll_owner=?2,poll_lease_until=?3,next_poll_at=?4 WHERE id=?1 AND status='pending' AND poll_lease_until<=?5 AND next_poll_at<=?5",params![id,owner,now+90000,now+row.3,now]))?;
        if claimed!=1{return Ok(status("pending",row.3/1000));}
        let n=row.4.ok_or(OAuthError("oauth_invalid_result"))?;let cipher=row.5.ok_or(OAuthError("oauth_invalid_result"))?;
        let secret:DeviceSecret=store(serde_json::from_str(&store(state.secrets.open(&n,&cipher))?))?;
        (secret,row.3)
    };
    let result=poll_claimed(state,id,&owner,secret,interval).await;
    store(state.conn().execute("UPDATE oauth_device_pending SET poll_owner=NULL,poll_lease_until=0 WHERE id=?1 AND poll_owner=?2",params![id,owner]))?;
    result
}
async fn poll_claimed(state:&SharedState,id:&str,owner:&str,secret:DeviceSecret,interval:i64)->Result<DevicePoll> {
    let p=provider(&secret.provider).ok_or(OAuthError("oauth_invalid_source"))?;
    let mut values=vec![("client_id",secret.client_id.as_str()),("grant_type","urn:ietf:params:oauth:grant-type:device_code"),(if secret.poll_user_code{"user_code"}else{"device_code"},secret.device_code.as_str())];
    if !p.audience.is_empty(){values.push(("code_verifier",secret.verifier.as_str()));}
    let (http,v)=form(state,p,p.token_path,&values,None,secret.origin_provider_id).await?;
    let code=v.get("error").or_else(||v.get("status")).and_then(Value::as_str).unwrap_or("");
    match code {
        "authorization_pending"|"pending"=>return Ok(status("pending",interval/1000)),
        "slow_down"=>{let next=interval.saturating_add(5000).min(3_600_000);store(state.conn().execute("UPDATE oauth_device_pending SET interval_ms=?3,next_poll_at=?4 WHERE id=?1 AND status='pending' AND poll_owner=?2",params![id,owner,next,db::now_ms()+next]))?;return Ok(status("slow_down",next/1000));}
        "expired_token"|"expired"=>{terminal(&state.conn(),id,owner,"expired")?;return Ok(status("expired",interval/1000));}
        "access_denied"|"denied"=>{terminal(&state.conn(),id,owner,"denied")?;return Ok(status("denied",interval/1000));}
        ""|"success"|"approved"=>{},
        _=>{terminal(&state.conn(),id,owner,"denied")?;return Err(OAuthError("oauth_device_authorization_failed"));}
    }
    if !(200..300).contains(&http){return Err(OAuthError("oauth_device_authorization_failed"));}
    let (mut bundle,expiry)=token_reply(p,&v,None)?;bundle.origin_provider_id=secret.origin_provider_id;
    let subject=if p.id=="copilot" {github_subject(state,&bundle.access_token,bundle.origin_provider_id).await?}else{format!("grant:{}",random())};
    let (nonce,cipher)=store(state.secrets.seal(&store(serde_json::to_string(&bundle))?))?;
    let account_id=store(db::in_tx(&state.conn(),|c|{
        let pending:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_device_pending WHERE id=?1 AND status='pending' AND poll_owner=?2 AND expires_at>?3)",params![id,owner,db::now_ms()],|r|r.get(0))?;
        if !pending{return Err(db::DbError::Other("OAuth attempt was canceled or expired".into()));}
        // Login creates a new grant identity unless the provider supplies a verified
        // user endpoint. No unverified JWT claims are treated as account identity.
        c.execute("INSERT INTO oauth_accounts(label,issuer,subject,client_id,status,scopes_json,expires_at,earliest_refresh_at,nonce,cipher) VALUES(?1,?2,?3,?4,'active',?5,?6,0,?7,?8) ON CONFLICT(issuer,client_id,subject) DO UPDATE SET label=excluded.label,status='active',scopes_json=excluded.scopes_json,expires_at=excluded.expires_at,nonce=excluded.nonce,cipher=excluded.cipher,credential_revision=oauth_accounts.credential_revision+1,refresh_owner=NULL,refresh_lease_until=0",params![secret.label,p.issuer,subject,secret.client_id,serde_json::to_string(&bundle.scopes).unwrap_or_else(|_|"[]".into()),expiry,nonce,cipher])?;
        let account_id:i64=c.query_row("SELECT id FROM oauth_accounts WHERE issuer=?1 AND client_id=?2 AND subject=?3",params![p.issuer,secret.client_id,subject],|r|r.get(0))?;
        let done=c.execute("UPDATE oauth_device_pending SET status='complete',nonce=NULL,cipher=NULL,poll_owner=NULL,poll_lease_until=0,account_id=?3 WHERE id=?1 AND status='pending' AND poll_owner=?2",params![id,owner,account_id])?;
        if done!=1{return Err(db::DbError::Other("OAuth attempt changed".into()));}
        db::identity::audit(c,None,"oauth.device.complete","oauth_account",account_id,json!({"provider":p.id}))?;Ok(account_id)
    }))?;
    Ok(DevicePoll{status:"complete".into(),account:Some(crate::oauth::account(&state.conn(),account_id)?),interval:interval/1000})
}
pub fn cancel(state:&SharedState,id:&str)->Result<()> {
    if !id.starts_with("device_")||id.len()>128{return invalid();}
    store(state.conn().execute("UPDATE oauth_device_pending SET status='canceled',nonce=NULL,cipher=NULL,poll_owner=NULL,poll_lease_until=0 WHERE id=?1 AND status='pending'",[id]))?;Ok(())
}
fn wire_integer(value:Option<&Value>,kimi_string:bool)->Option<i64> {
    value.and_then(|v|v.as_i64().or_else(||if kimi_string{v.as_str().filter(|s|!s.is_empty()&&s.len()<=10&&s.bytes().all(|b|b.is_ascii_digit())).and_then(|s|s.parse().ok())}else{None}))
}
fn token_reply(p:ProviderSpec,v:&Value,old:Option<&TokenBundle>)->Result<(TokenBundle,i64)> {
    let access_token=bounded_string(v,"access_token",16384)?;
    let bearer=match v.get("token_type") {None if p.id=="kimi"=>true,Some(Value::String(s))=>s.eq_ignore_ascii_case("bearer"),_=>false};
    if !bearer{return invalid();}
    let refresh_token=match v.get("refresh_token") {Some(Value::String(s)) if !s.is_empty()&&s.len()<=16384&&!s.chars().any(char::is_control)=>Some(s.clone()),None=>old.and_then(|b|b.refresh_token.clone()),_=>return invalid()};
    if p.id!="copilot"&&refresh_token.is_none(){return invalid();}
    let seconds=match wire_integer(v.get("expires_in"),p.id=="kimi"){Some(n) if (1..=31_536_000).contains(&n)=>n,None if p.id=="copilot"=>86400,_=>return invalid()};
    // A scope hint may be carried inside the access token returned directly by
    // this fixed TLS token endpoint. It is never used as verified user identity.
    let hint=v.get("scope").cloned().or_else(|| {
        if !p.id.starts_with("minimax-"){return None;}
        let payload=access_token.split('.').nth(1)?;let raw=URL_SAFE_NO_PAD.decode(payload).ok()?;
        let claims:Value=serde_json::from_slice(&raw).ok()?;claims.get("scope").or_else(||claims.get("scp")).cloned()
    });
    let scopes=match hint.as_ref() {Some(Value::String(s)) if s.len()<=4096=>s.split_whitespace().map(str::to_owned).collect::<Vec<_>>(),Some(Value::Array(rows)) if rows.len()<=64=>rows.iter().map(|r|r.as_str().filter(|s|s.len()<=128).map(str::to_owned).ok_or(OAuthError("oauth_invalid_result"))).collect::<Result<Vec<_>>>()?,None=>old.map(|v|v.scopes.clone()).unwrap_or_default(),_=>return invalid()};
    if !p.scope.is_empty()&&!scopes.iter().any(|s|s==p.scope){return Err(OAuthError("oauth_invalid_scope"));}
    let expiry=db::now_ms().checked_add(seconds*1000).ok_or(OAuthError("oauth_invalid_result"))?;
    Ok((TokenBundle{provider:p.id.into(),access_token,refresh_token,scopes,origin_provider_id:old.and_then(|v|v.origin_provider_id)},expiry))
}
async fn github_subject(state:&SharedState,token:&str,origin:Option<i64>)->Result<String> {
    let response=client(state,provider("copilot").unwrap(),None,origin)?.get("https://api.github.com/user").header("Accept","application/vnd.github+json").bearer_auth(token).send().await.map_err(|_|OAuthError("oauth_unavailable"))?;
    let (code,v)=json_body(response).await?;if code!=200{return Err(OAuthError("oauth_identity_unavailable"));}v.get("id").and_then(Value::as_u64).map(|id|format!("github:{id}")).ok_or(OAuthError("oauth_invalid_identity"))
}
fn bundle(state:&SharedState,id:i64)->Result<TokenBundle> {
    let pair:(Vec<u8>,Vec<u8>)=store(state.conn().query_row("SELECT nonce,cipher FROM oauth_accounts WHERE id=?1 AND status='active' AND nonce IS NOT NULL AND cipher IS NOT NULL",[id],|r|Ok((r.get(0)?,r.get(1)?))))?;
    let b:TokenBundle=store(serde_json::from_str(&store(state.secrets.open(&pair.0,&pair.1))?))?;
    if account_provider(&state.conn(),id)?!=b.provider{return Err(OAuthError("oauth_invalid_source"));}Ok(b)
}
async fn ensure_session(state:&SharedState,id:i64)->Result<()> {
    let state=state.clone();tokio::spawn(async move{refresh(&state,id).await}).await.map_err(|_|OAuthError("oauth_unavailable"))?
}
async fn refresh(state:&SharedState,id:i64)->Result<()> {
    let began=tokio::time::Instant::now();
    loop {
        match refresh_once(state,id).await {
            Err(e) if e.0=="oauth_refresh_in_progress" && began.elapsed()<Duration::from_secs(25)=>tokio::time::sleep(Duration::from_millis(100)).await,
            result=>return result,
        }
    }
}
async fn refresh_once(state:&SharedState,id:i64)->Result<()> {
    let now=db::now_ms();let a=crate::oauth::account(&state.conn(),id)?;
    if a.status!="active"{return Err(OAuthError("oauth_unavailable"));}
    if a.expires_at>now+60000{return Ok(());}
    let p=provider(&a.provider).ok_or(OAuthError("oauth_invalid_source"))?;let old=bundle(state,id)?;
    let Some(refresh_token)=old.refresh_token.as_deref() else {
        if a.expires_at>now{return Ok(());}
        store(state.conn().execute("UPDATE oauth_accounts SET status='reauth_required',nonce=NULL,cipher=NULL,credential_revision=credential_revision+1 WHERE id=?1 AND status='active' AND credential_revision=?2",params![id,a.credential_revision]))?;return Err(OAuthError("oauth_reauthorization_required"));
    };
    let owner=random();let claimed=store(state.conn().execute("UPDATE oauth_accounts SET refresh_owner=?2,refresh_lease_until=?3 WHERE id=?1 AND status='active' AND credential_revision=?4 AND refresh_lease_until<=?5",params![id,owner,now+60000,a.credential_revision,now]))?;
    if claimed!=1{return Err(OAuthError("oauth_refresh_in_progress"));}
    let result=async {
        let mut values=vec![("client_id",a.client_id.as_str()),("grant_type","refresh_token"),("refresh_token",refresh_token)];
        if !p.audience.is_empty(){values.push(("audience",p.audience));values.push(("scope",p.scope));}
        let (http,v)=form(state,p,p.token_path,&values,Some(id),old.origin_provider_id).await?;
        if !(200..300).contains(&http){
            if http==429||http>=500{return Err(OAuthError("oauth_unavailable"));}
            if matches!(http,401|403)||matches!(v.get("error").and_then(Value::as_str),Some("invalid_grant"|"invalid_token")){store(state.conn().execute("UPDATE oauth_accounts SET status='reauth_required',nonce=NULL,cipher=NULL,credential_revision=credential_revision+1 WHERE id=?1 AND status='active' AND credential_revision=?2 AND refresh_owner=?3",params![id,a.credential_revision,owner]))?;}
            return Err(OAuthError("oauth_reauthorization_required"));
        }
        let (next,expiry)=token_reply(p,&v,Some(&old))?;let (nonce,cipher)=store(state.secrets.seal(&store(serde_json::to_string(&next))?))?;
        let changed=store(state.conn().execute("UPDATE oauth_accounts SET nonce=?2,cipher=?3,expires_at=?4,scopes_json=?5,credential_revision=credential_revision+1,refresh_owner=NULL,refresh_lease_until=0 WHERE id=?1 AND status='active' AND credential_revision=?6 AND refresh_owner=?7",params![id,nonce,cipher,expiry,store(serde_json::to_string(&next.scopes))?,a.credential_revision,owner]))?;
        if changed!=1{return Err(OAuthError("oauth_session_changed"));}Ok(())
    }.await;
    store(state.conn().execute("UPDATE oauth_accounts SET refresh_owner=NULL,refresh_lease_until=0 WHERE id=?1 AND refresh_owner=?2",params![id,owner]))?;result
}
fn clean_source(c:&rusqlite::Connection,channel:i64,p:ProviderSpec)->db::Result<bool> {
    Ok(db::channels::get(c,tokengo_shared::ChannelId(channel))?.is_some_and(|r|r.base_url.trim_end_matches('/')==p.api_base&&!r.has_secret&&r.headers.is_empty()&&r.models.iter().all(|m|m.drop_fields.is_empty())))
}
pub fn bind(state:&SharedState,channel:i64,b:&Binding)->Result<()> {
    let p=provider(&account_provider(&state.conn(),b.account_id)?).ok_or(OAuthError("oauth_invalid_source"))?;
    if !p.inference_available{return Err(OAuthError("oauth_transport_not_available"));}
    if b.reasoning_effort.is_some()||b.service_tier.is_some(){return Err(OAuthError("oauth_invalid_defaults"));}
    if !matches!(b.max_input_tokens.zip(b.max_output_tokens),Some((i,o)) if (1..=16_777_216).contains(&i)&&(1..=16_777_216).contains(&o)){return Err(OAuthError("oauth_invalid_bounds"));}
    let c=state.conn();store(db::in_tx(&c,|c|{
        if !clean_source(c,channel,p)?{return Err(db::DbError::Other("OAuth requires clean fixed provider source".into()));}
        let a=crate::oauth::account(c,b.account_id).map_err(|_|db::DbError::Other("OAuth account unavailable".into()))?;
        if a.status!="active"||a.expires_at<=db::now_ms(){return Err(db::DbError::Other("OAuth account unavailable".into()));}
        let source=db::channels::get(c,tokengo_shared::ChannelId(channel))?.ok_or_else(||db::DbError::Other("Source unavailable".into()))?;
        let models=source.models.iter().filter(|m|m.enabled).map(|m|m.upstream.as_deref().unwrap_or(&m.model).to_owned()).collect::<Vec<_>>();
        validate_bound_models(c,b.account_id,&models).map_err(|_|db::DbError::Other("OAuth model protocol is not confirmed as native Chat for this account".into()))?;
        crate::protocol_config::save(c,channel,&crate::protocol_config::Capabilities{native:vec![p.native_protocol],allow_conversion:true,..Default::default()})?;
        c.execute("INSERT INTO oauth_channel_bindings(channel_id,account_id,max_input_tokens,max_output_tokens) VALUES(?1,?2,?3,?4) ON CONFLICT(channel_id) DO UPDATE SET account_id=excluded.account_id,reasoning_effort=NULL,service_tier=NULL,max_input_tokens=excluded.max_input_tokens,max_output_tokens=excluded.max_output_tokens",params![channel,b.account_id,b.max_input_tokens,b.max_output_tokens])?;
        db::identity::audit(c,None,"oauth.bind","channel",channel,json!({"account_id":b.account_id,"provider":p.id}))?;Ok(())
    }))
}
pub async fn credentials(state:&SharedState,channel:i64)->Result<Option<OAuthCredential>> {
    let initial:Option<i64>=store(state.conn().query_row("SELECT account_id FROM oauth_channel_bindings WHERE channel_id=?1",[channel],|r|r.get(0)).optional())?;
    let Some(id)=initial else{return Ok(None)};ensure_session(state,id).await?;
    let needs_catalog={let c=state.conn();let a=crate::oauth::account(&c,id)?;
        if a.provider!="kimi"{false}else{
            let raw=store(db::settings::get(&c,&format!("oauth_external_catalog:{id}")))?;
            raw.as_deref().and_then(|s|serde_json::from_str::<Value>(s).ok()).is_none_or(|v|v["revision"]!=a.credential_revision||v["client_id"]!=a.client_id||v["expires_at"].as_i64().is_none_or(|expiry|expiry<=db::now_ms()))
        }};
    if needs_catalog {models(state,id).await?;}
    let c=state.conn();let p=provider(&account_provider(&c,id)?).ok_or(OAuthError("oauth_invalid_source"))?;
    if !p.inference_available||!store(clean_source(&c,channel,p))?{return Err(OAuthError("oauth_invalid_source"));}
    let a=crate::oauth::account(&c,id)?;
    let row:(i64,i64,Option<i64>,Option<i64>)=store(c.query_row("SELECT b.account_id,c.binding_generation,b.max_input_tokens,b.max_output_tokens FROM oauth_channel_bindings b JOIN channels c ON c.id=b.channel_id WHERE b.channel_id=?1",[channel],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))))?;
    if row.0!=id||a.status!="active"||a.expires_at<=db::now_ms(){return Err(OAuthError("oauth_binding_changed"));}
    let caps=store(crate::protocol_config::load(&c,channel))?;
    if caps.native!=vec![p.native_protocol]||caps.native_input_audio{return Err(OAuthError("oauth_invalid_source"));}
    let source=store(db::channels::get(&c,tokengo_shared::ChannelId(channel)))?.ok_or(OAuthError("oauth_invalid_source"))?;
    validate_bound_models(&c,id,&source.models.iter().filter(|m|m.enabled).map(|m|m.upstream.as_deref().unwrap_or(&m.model).to_owned()).collect::<Vec<_>>())?;
    drop(c);let token=bundle(state,id)?;
    Ok(Some(OAuthCredential{access_token:token.access_token,account_id:id,credential_revision:a.credential_revision,binding_generation:row.1,expires_at:a.expires_at,api_base:p.api_base.into(),provider:p.native_protocol,reasoning_effort:None,service_tier:None,max_input_tokens:row.2,max_output_tokens:row.3,account_header:None,managed_openai:false,oauth_provider:p.id.into()}))
}
pub fn credentials_current(state:&SharedState,channel:i64,credential:&OAuthCredential)->Result<bool> {
    let c=state.conn();let p=provider(&credential.oauth_provider).ok_or(OAuthError("oauth_invalid_source"))?;
    let current:Option<(i64,i64,i64,String,i64)>=store(c.query_row("SELECT b.account_id,c.binding_generation,a.credential_revision,a.status,a.expires_at FROM oauth_channel_bindings b JOIN channels c ON c.id=b.channel_id JOIN oauth_accounts a ON a.id=b.account_id WHERE b.channel_id=?1",[channel],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional())?;
    Ok(current.is_some_and(|(id,generation,revision,status,expiry)|id==credential.account_id&&generation==credential.binding_generation&&revision==credential.credential_revision&&status=="active"&&expiry>db::now_ms())&&account_provider(&c,credential.account_id)?==p.id&&store(clean_source(&c,channel,p))?)
}
pub fn inference_headers(credential:&OAuthCredential)->Vec<(String,String)> {
    // An explicit Authorization header prevents native Messages transport from
    // interpreting an OAuth bearer as x-api-key. This value is transport-only.
    let mut headers=vec![("Authorization".into(),format!("Bearer {}",credential.access_token))];
    if credential.oauth_provider=="kimi" {headers.extend([("X-Msh-Platform".into(),"tokengo".into()),("X-Msh-Version".into(),env!("CARGO_PKG_VERSION").into())]);}
    if credential.oauth_provider.starts_with("minimax-"){headers.push(("anthropic-version".into(),"2023-06-01".into()));}
    headers
}
/// Only explicit account catalog protocol declarations allow Kimi Chat bindings.
/// MiniMax's verified Messages transport permits manually configured model IDs;
/// this is not reported as account-catalog verification.
pub fn validate_bound_models(c:&rusqlite::Connection,id:i64,models:&[String])->Result<()> {
    let name=account_provider(c,id)?;if name=="openai"{return Ok(());}
    let p=provider(&name).ok_or(OAuthError("oauth_invalid_source"))?;
    if !p.inference_available{return Err(OAuthError("oauth_transport_not_available"));}
    if models.iter().any(|m|m.trim().is_empty()||m.len()>256||m.chars().any(char::is_control)){return Err(OAuthError("oauth_invalid_model"));}
    if name.starts_with("minimax-")||models.is_empty(){return Ok(());}
    let raw=store(db::settings::get(c,&format!("oauth_external_catalog:{id}")))?.ok_or(OAuthError("oauth_model_protocol_confirmation_required"))?;
    let v:Value=store(serde_json::from_str(&raw))?;let a=crate::oauth::account(c,id)?;
    if v["provider"]!=name||v["client_id"]!=a.client_id||v["revision"]!=a.credential_revision||v["expires_at"].as_i64().is_none_or(|expiry|expiry<=db::now_ms()) {return Err(OAuthError("oauth_model_protocol_confirmation_required"));}
    let rows=v["models"].as_array().ok_or(OAuthError("oauth_model_protocol_confirmation_required"))?;
    if models.iter().any(|model|!rows.iter().any(|row|row["id"].as_str()==Some(model.as_str())&&matches!(row["protocol"].as_str(),Some("chat")))){return Err(OAuthError("oauth_model_protocol_not_supported"));}Ok(())
}
fn catalog_protocol(value:Option<&Value>)->&'static str {
    match value {None=>"chat",Some(Value::String(s)) if s=="kimi"=>"chat",Some(Value::String(s)) if s=="anthropic"=>"messages",Some(Value::String(s)) if s=="response"=>"responses",_=>"unsupported"}
}
pub async fn models(state:&SharedState,id:i64)->Result<Value> {
    let p=provider(&account_provider(&state.conn(),id)?).ok_or(OAuthError("oauth_invalid_source"))?;
    if !p.inference_available{return Err(OAuthError("oauth_transport_not_available"));}
    if p.id!="kimi"{return Err(OAuthError("oauth_models_manual_configuration_required"));}
    ensure_session(state,id).await?;let a=crate::oauth::account(&state.conn(),id)?;let token=bundle(state,id)?;
    let response=client(state,p,Some(id),token.origin_provider_id)?.get(format!("{}/models",p.api_base)).bearer_auth(&token.access_token).header("X-Msh-Platform","tokengo").send().await.map_err(|_|OAuthError("oauth_unavailable"))?;
    let (http,v)=json_body(response).await?;if !(200..300).contains(&http){return Err(OAuthError("oauth_models_unavailable"));}
    let serialized=v.to_string();if serialized.contains(&token.access_token)||token.refresh_token.as_ref().is_some_and(|s|serialized.contains(s)){return invalid();}
    let rows=v["data"].as_array().filter(|rows|rows.len()<=2048).ok_or(OAuthError("oauth_invalid_catalog"))?;
    let mut safe=Vec::new();let mut seen=std::collections::HashSet::new();
    for row in rows {
        let id=bounded_string(row,"id",256)?;if !seen.insert(id.clone()){return Err(OAuthError("oauth_invalid_catalog"));}
        let mut out=json!({"id":id,"object":"model"});
        for key in ["context_length","supports_reasoning","supports_image_in","supports_video_in","display_name","effort","capabilities"] {if let Some(value)=row.get(key){out[key]=value.clone();}}
        // Current official Kimi SDK uses its Chat adapter for legacy missing/kimi
        // protocol, and explicitly branches anthropic and response. Unknown future
        // declarations are not assigned that legacy default.
        out["protocol"]=json!(catalog_protocol(row.get("protocol")));
        if let Some(value)=row.get("protocol"){out["upstream_protocol"]=value.clone();}
        safe.push(out);
    }
    let c=state.conn();let current=crate::oauth::account(&c,id)?;if current.status!="active"||current.credential_revision!=a.credential_revision{return Err(OAuthError("oauth_session_changed"));}
    store(db::settings::set(&c,&format!("oauth_external_catalog:{id}"),&json!({"provider":p.id,"client_id":a.client_id,"revision":a.credential_revision,"expires_at":db::now_ms()+86400000,"models":safe}).to_string()))?;
    let parsed=crate::upstream::parse_model_catalog(&json!({"data":safe}).to_string());
    let mut stmt=store(c.prepare("SELECT channel_id FROM oauth_channel_bindings WHERE account_id=?1"))?;
    let channels=store(stmt.query_map([id],|r|r.get::<_,i64>(0)))?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|OAuthError("oauth_storage_unavailable"))?;
    for channel in channels {store(db::catalog::replace(&c,tokengo_shared::ChannelId(channel),&parsed,db::now_ms()))?;}
    Ok(json!({"object":"list","data":safe}))
}
pub async fn revoke(state:&SharedState,id:i64)->Result<Revoked> {
    let a=crate::oauth::account(&state.conn(),id)?;let p=provider(&a.provider).ok_or(OAuthError("oauth_invalid_source"))?;
    let token=bundle(state,id).ok();
    {let c=state.conn();store(db::in_tx(&c,|c|{
        c.execute("UPDATE oauth_accounts SET status='revoked',nonce=NULL,cipher=NULL,refresh_owner=NULL,refresh_lease_until=0,credential_revision=credential_revision+1 WHERE id=?1",[id])?;
        c.execute("DELETE FROM settings WHERE key=?1",[format!("oauth_external_catalog:{id}")])?;
        db::identity::audit(c,None,"oauth.revoke","oauth_account",id,json!({"provider":p.id}))?;Ok(())
    }))?;}
    let confirmed=if p.id.starts_with("minimax-") {if let Some(refresh)=token.as_ref().and_then(|t|t.refresh_token.as_deref()) {
        if let Ok(http)=client(state,p,Some(id),token.as_ref().and_then(|t|t.origin_provider_id)) {
            http.post(format!("{}/oauth2/revoke",p.issuer)).form(&[("client_id",a.client_id.as_str()),("token",refresh),("token_type_hint","refresh_token")]).send().await.is_ok_and(|r|r.status().is_success())
        }else{false}
    }else{false}}else{false};
    Ok(Revoked{account_id:id,revoked:true,remote_revocation_confirmed:confirmed})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_missing_or_declared_kimi_protocol_uses_legacy_chat() {
        assert_eq!(catalog_protocol(None),"chat");
        assert_eq!(catalog_protocol(Some(&json!("kimi"))),"chat");
        assert_eq!(catalog_protocol(Some(&json!("anthropic"))),"messages");
        assert_eq!(catalog_protocol(Some(&json!("response"))),"responses");
        for value in [Value::Null,json!(123),json!({}),json!("future")]{assert_eq!(catalog_protocol(Some(&value)),"unsupported");}
    }
    #[test]
    fn minimax_expiry_units_follow_the_fixed_official_contract() {
        let now=1_700_000_000_000;
        assert_eq!(account_expiry_seconds(600,now),Some(600));
        assert_eq!(account_expiry_seconds(now+600001,now),Some(601));
        assert_eq!(account_expiry_seconds(now-1,now),None);
        assert_eq!(account_expiry_seconds(0,now),None);
    }
    #[test]
    fn verification_links_are_fixed_to_the_selected_provider() {
        let p=provider("kimi").unwrap();assert!(verification_uri(p,"https://auth.kimi.com/device?user_code=ABC").is_ok());
        for value in ["http://auth.kimi.com/device","https://auth.kimi.com.evil.example/device","https://evil.example/device","https://user@auth.kimi.com/device","https://auth.kimi.com:444/device"] {assert!(verification_uri(p,value).is_err());}
    }
    #[test]
    fn kimi_omitted_token_kind_and_integer_string_expiry_follow_its_sdk() {
        let p=provider("kimi").unwrap();
        assert!(token_reply(p,&json!({"access_token":"a","refresh_token":"r","expires_in":"3600"}),None).is_ok());
        assert!(token_reply(p,&json!({"access_token":"a","refresh_token":"r","token_type":"MAC","expires_in":"3600"}),None).is_err());
        for expiry in ["bad","-1","0","999999999999999999999"] {
            assert!(token_reply(p,&json!({"access_token":"a","refresh_token":"r","expires_in":expiry}),None).is_err());
        }
        assert!(token_reply(provider("minimax-cn").unwrap(),&json!({"access_token":"a","refresh_token":"r","expires_in":"3600","scope":"agent.default"}),None).is_err());
    }
    #[test]
    fn token_validation_rejects_wrong_kind_missing_refresh_and_scope() {
        let p=provider("minimax-cn").unwrap();
        assert!(token_reply(p,&json!({"access_token":"a","refresh_token":"r","token_type":"Bearer","expires_in":3600,"scope":"agent.default"}),None).is_ok());
        for v in [json!({"access_token":"a","token_type":"Bearer","expires_in":3600,"scope":"agent.default"}),json!({"access_token":"a","refresh_token":"r","token_type":"MAC","expires_in":3600,"scope":"agent.default"}),json!({"access_token":"a","refresh_token":"r","token_type":"Bearer","expires_in":3600,"scope":"other"})] {assert!(token_reply(p,&v,None).is_err());}
    }
    #[test]
    fn issuer_is_not_guessed_from_model_or_brand_substrings() {
        let c=db::open_in_memory().unwrap();
        c.execute("INSERT INTO oauth_accounts(label,issuer,subject,client_id,status,scopes_json,expires_at,earliest_refresh_at) VALUES('x','https://github.com.evil.example','s','c','active','[]',1,0)",[]).unwrap();
        assert!(account_provider(&c,c.last_insert_rowid()).is_err());
    }
    #[test]
    fn cancellation_clears_pending_secrets_even_with_a_claimed_poll() {
        let c=db::open_in_memory().unwrap();
        c.execute("INSERT INTO oauth_device_pending(id,provider,status,expires_at,next_poll_at,interval_ms,nonce,cipher,poll_owner,poll_lease_until) VALUES('device_x','kimi','pending',9999999999999,0,5000,x'01',x'02','owner',9999999999999)",[]).unwrap();
        c.execute("UPDATE oauth_device_pending SET status='canceled',nonce=NULL,cipher=NULL,poll_owner=NULL,poll_lease_until=0 WHERE id='device_x' AND status='pending'",[]).unwrap();
        let row:(String,Option<Vec<u8>>,Option<Vec<u8>>)=c.query_row("SELECT status,nonce,cipher FROM oauth_device_pending WHERE id='device_x'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(row,("canceled".into(),None,None));
        assert_eq!(c.execute("UPDATE oauth_device_pending SET status='complete' WHERE id='device_x' AND status='pending' AND poll_owner='owner'",[]).unwrap(),0);
    }
}
