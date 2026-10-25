//! Administrator-owned outbound proxy policy. No user-supplied model alias selects egress.
use crate::{crypto::SecretBox, db, SharedState};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Mutex, time::Duration};

const PREFIX:&str="egress_policy:";
pub const FAMILIES:&[&str]=&["gpt","claude","gemini","deepseek","qwen","glm","kimi","mimo","minimax","doubao","grok","mistral","llama","cohere","other"];
#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    version:u64, mode:String, proxy_url:Option<String>,
    credential_nonce:Option<Vec<u8>>, credential_cipher:Option<Vec<u8>>,
    network_family:Option<String>,
}
impl Default for Stored {fn default()->Self{Self{version:0,mode:"inherit".into(),proxy_url:None,credential_nonce:None,credential_cipher:None,network_family:None}}}
#[derive(Serialize,Deserialize)]
struct Credentials {username:String,password:String}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub mode:String,
    #[serde(default)]pub proxy_url:Option<String>,
    pub credential_action:String,
    pub username:Option<String>, pub password:Option<String>,
    pub expected_version:u64,
    #[serde(default)]pub network_family:Option<String>,
}
#[derive(Clone,Copy,Hash,PartialEq,Eq)]
pub enum Purpose {Inference,Subscription,Catalog,OAuth,OpenAiOAuth}
impl Purpose {fn key(self)->&'static str{match self{Self::Inference=>"inference",Self::Subscription=>"subscription",Self::Catalog=>"catalog",Self::OAuth=>"oauth",Self::OpenAiOAuth=>"openai-oauth"}}}
#[derive(Default)]
pub struct Runtime {clients:Mutex<HashMap<String,reqwest::Client>>}
impl Runtime {pub fn invalidate(&self){self.clients.lock().unwrap_or_else(|e|e.into_inner()).clear();}}
fn key(scope:&str,id:&str)->String{format!("{PREFIX}{scope}:{id}")}
fn load(c:&Connection,scope:&str,id:&str)->Result<Stored,String>{
    db::settings::get(c,&key(scope,id)).map_err(|_|"egress_storage_unavailable".to_owned())?
        .map(|raw|serde_json::from_str(&raw).map_err(|_|"egress_invalid_stored_policy".into())).transpose().map(|v|v.unwrap_or_default())
}
fn target(c:&Connection,scope:&str,id:&str)->Result<(),String>{
    if scope=="family" {return if FAMILIES.contains(&id){Ok(())}else{Err("egress_invalid_family".into())};}
    let n=id.parse::<i64>().ok().filter(|n|*n>0).ok_or("egress_invalid_target")?;
    let table=match scope{"provider"=>"provider_channels","source"=>"channels",_=>return Err("egress_invalid_scope".into())};
    let exists:bool=c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),[n],|r|r.get(0)).map_err(|_|"egress_storage_unavailable")?;
    if exists{Ok(())}else{Err("egress_target_not_found".into())}
}
fn proxy_url(raw:&str)->Result<String,String>{
    let u=url::Url::parse(raw.trim()).map_err(|_|"egress_invalid_proxy_url")?;
    if !matches!(u.scheme(),"http"|"https")||u.host_str().is_none()||!u.username().is_empty()||u.password().is_some()||u.port()==Some(0)||!matches!(u.path(),""|"/")||u.query().is_some()||u.fragment().is_some(){return Err("egress_proxy_requires_http_https_without_credentials".into());}
    Ok(u.as_str().trim_end_matches('/').to_owned())
}
fn safe(scope:&str,id:&str,row:&Stored)->Value{json!({"scope":scope,"id":id,"version":row.version,"mode":row.mode,"proxy_url":row.proxy_url.as_deref().and_then(|v|proxy_url(v).ok()),"has_credentials":row.credential_cipher.is_some(),"network_family":row.network_family})}
pub fn list(c:&Connection)->Result<Value,String>{
    let mut q=c.prepare("SELECT key,value FROM settings WHERE key LIKE 'egress_policy:%' ORDER BY key").map_err(|_|"egress_storage_unavailable")?;
    let rows=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(|_|"egress_storage_unavailable")?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|"egress_storage_unavailable")?;
    let mut result=vec![];
    for (name,raw) in rows {let Some((scope,id))=name.strip_prefix(PREFIX).and_then(|v|v.split_once(':'))else{continue};let row:Stored=serde_json::from_str(&raw).map_err(|_|"egress_invalid_stored_policy")?;result.push(safe(scope,id,&row));}
    Ok(json!({"policies":result,"families":FAMILIES,"precedence":["source","provider","family","system_environment"],"supported_schemes":["http","https"]}))
}
pub fn save(c:&Connection,secrets:&SecretBox,scope:&str,id:&str,write:Write)->Result<Value,String>{
    target(c,scope,id)?;
    if !matches!(write.mode.as_str(),"inherit"|"direct"|"proxy") {return Err("egress_invalid_mode".into());}
    if write.network_family.as_deref().is_some_and(|f|!FAMILIES.contains(&f)) {return Err("egress_invalid_family".into());}
    if scope=="family"&&write.network_family.is_some(){return Err("egress_family_scope_cannot_override_family".into());}
    if !matches!(write.credential_action.as_str(),"keep"|"replace"|"clear") {return Err("egress_invalid_credential_action".into());}
    if write.credential_action!="replace"&&(write.username.is_some()||write.password.is_some()){return Err("egress_credentials_require_replace".into());}
    let url=if write.mode=="proxy"{Some(proxy_url(write.proxy_url.as_deref().ok_or("egress_proxy_url_required")?)?)}else{None};
    let sealed=if write.credential_action=="replace" {
        let username=write.username.filter(|s|!s.is_empty()&&s.len()<=1024&&!s.chars().any(char::is_control)).ok_or("egress_invalid_proxy_credentials")?;
        let password=write.password.filter(|s|s.len()<=4096&&!s.chars().any(char::is_control)).ok_or("egress_invalid_proxy_credentials")?;
        Some(secrets.seal(&serde_json::to_string(&Credentials{username,password}).map_err(|_|"egress_storage_unavailable")?).map_err(|_|"egress_storage_unavailable")?)
    }else{None};
    db::in_tx(c,|c|{
        target(c,scope,id).map_err(db::DbError::Other)?;
        let old=load(c,scope,id).map_err(db::DbError::Other)?;
        if old.version!=write.expected_version{return Err(db::DbError::Other("egress_version_conflict".into()));}
        if write.credential_action=="keep"&&old.credential_cipher.is_some()&&old.proxy_url!=url{return Err(db::DbError::Other("egress_proxy_changed_clear_or_replace_credentials".into()));}
        let (nonce,cipher)=match write.credential_action.as_str(){"replace"=>{let(n,c)=sealed.as_ref().unwrap();(Some(n.clone()),Some(c.clone()))},"clear"=>(None,None),_=>(old.credential_nonce,old.credential_cipher)};
        let row=Stored{version:old.version.checked_add(1).ok_or_else(||db::DbError::Other("egress_version_conflict".into()))?,mode:write.mode.clone(),proxy_url:url.clone(),credential_nonce:nonce,credential_cipher:cipher,network_family:write.network_family.clone()};
        db::settings::set(c,&key(scope,id),&serde_json::to_string(&row).map_err(|_|db::DbError::Other("egress_storage_unavailable".into()))?)?;
        db::identity::audit(c,None,"network.proxy_policy","egress",0,json!({"scope":scope,"target":id,"mode":row.mode,"version":row.version,"has_credentials":row.credential_cipher.is_some()}))?;
        Ok(safe(scope,id,&row))
    }).map_err(|e|match e{db::DbError::Other(reason) if reason.starts_with("egress_")=>reason,_=>"egress_storage_unavailable".into()})
}
fn exact_family(base:&str)->Option<&'static str>{
    let u=url::Url::parse(base).ok()?;if u.scheme()!="https"||!u.username().is_empty()||u.password().is_some()||u.port().is_some(){return None;}
    match (u.host_str()?,u.path().trim_end_matches('/')){
        ("api.openai.com","/v1")=>Some("gpt"),("api.anthropic.com",""|"/v1")=>Some("claude"),
        ("generativelanguage.googleapis.com","/v1"|"/v1beta"|"/v1/openai"|"/v1beta/openai")=>Some("gemini"),
        ("api.deepseek.com",""|"/v1"|"/beta"|"/anthropic/v1")=>Some("deepseek"),
        ("dashscope.aliyuncs.com"|"dashscope-intl.aliyuncs.com","/compatible-mode/v1")=>Some("qwen"),
        ("api.kimi.com"|"api.kimi.ai","/coding/v1")|("api.moonshot.ai"|"api.moonshot.cn","/v1")=>Some("kimi"),
        ("account.minimax.cn"|"account.minimax.io",_)|("agent.minimax.cn"|"agent.minimax.io","/mavis/api/v1/llm/v1")|("api.minimax.io"|"api.minimaxi.com","/v1"|"/anthropic"|"/anthropic/v1")=>Some("minimax"),
        ("api.xiaomimimo.com","/v1"|"/anthropic/v1")=>Some("mimo"),
        ("open.bigmodel.cn"|"api.z.ai","/api/paas/v4")=>Some("glm"),
        ("api.x.ai","/v1")=>Some("grok"),("api.mistral.ai","/v1")=>Some("mistral"),
        _=>None,
    }
}
fn oauth_family(provider:&str)->&str{match provider{"openai"=>"gpt","antigravity"=>"gemini","minimax-cn"|"minimax-global"=>"minimax","kimi"=>"kimi",_=>"other"}}
fn selected(c:&Connection,source:Option<i64>,provider:Option<i64>,family:Option<&str>,base:&str)->Result<Stored,String>{
    let source_policy=source.map(|id|load(c,"source",&id.to_string())).transpose()?;
    let parent=if let Some(id)=source{c.query_row("SELECT provider_id FROM provider_connections WHERE channel_id=?1",[id],|r|r.get::<_,i64>(0)).optional().map_err(|_|"egress_storage_unavailable")?.or(provider)}else{provider};
    let provider_policy=parent.map(|id|load(c,"provider",&id.to_string())).transpose()?;
    let family=source_policy.as_ref().and_then(|v|v.network_family.as_deref()).or_else(||provider_policy.as_ref().and_then(|v|v.network_family.as_deref())).or(family).or_else(||exact_family(base)).map(str::to_owned);
    for p in [source_policy,provider_policy].into_iter().flatten(){if p.mode!="inherit"{return Ok(p);}}
    if let Some(family)=family {let p=load(c,"family",&family)?;if p.mode!="inherit"{return Ok(p);}}
    Ok(Stored::default())
}
fn identity(row:&Stored)->Result<String,String>{let effective=if row.mode=="proxy"{json!({"mode":row.mode,"url":row.proxy_url,"nonce":row.credential_nonce,"cipher":row.credential_cipher})}else{json!({"mode":row.mode})};serde_json::to_vec(&effective).map(|v|hex::encode(Sha256::digest(v))).map_err(|_|"egress_invalid_stored_policy".into())}
/// Compare effective routes, not randomized credential ciphertexts. This digest
/// is internal only and never returned in DTOs or logs.
fn effective_route_identity(secrets:&SecretBox,row:&Stored)->Result<String,String>{
    if row.mode!="proxy"{return identity(row);}
    let credentials=match (&row.credential_nonce,&row.credential_cipher){
        (Some(n),Some(c))=>{let plain=secrets.open(n,c).map_err(|_|"egress_proxy_credentials_unavailable")?;let value:Credentials=serde_json::from_str(&plain).map_err(|_|"egress_proxy_credentials_unavailable")?;serde_json::to_value(value).map_err(|_|"egress_proxy_credentials_unavailable")?},
        (None,None)=>Value::Null,_=>return Err("egress_proxy_credentials_unavailable".into()),
    };
    let url=proxy_url(row.proxy_url.as_deref().ok_or("egress_invalid_stored_policy")?)?;
    let bytes=serde_json::to_vec(&json!({"mode":"proxy","url":url,"credentials":credentials})).map_err(|_|"egress_invalid_stored_policy")?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
fn consistent_routes(secrets:&SecretBox,rows:Vec<Stored>)->Result<Stored,String>{
    let mut fingerprint=None;
    for row in &rows{let current=effective_route_identity(secrets,row)?;if fingerprint.as_ref().is_some_and(|old|old!=&current){return Err("egress_account_proxy_ambiguous".into());}fingerprint=Some(current);}
    Ok(rows.into_iter().next().unwrap_or_default())
}
fn legacy_openai_environment(purpose:Purpose,mode:&str)->bool{mode=="inherit"&&matches!(purpose,Purpose::Subscription|Purpose::OpenAiOAuth)}
fn client(state:&SharedState,row:Stored,purpose:Purpose)->Result<reqwest::Client,String>{
    let key=format!("{}:{}",purpose.key(),identity(&row)?);
    if let Some(client)=state.egress.clients.lock().unwrap_or_else(|e|e.into_inner()).get(&key).cloned(){return Ok(client);}
    let mut builder=reqwest::Client::builder().user_agent(concat!("tokengo/",env!("CARGO_PKG_VERSION"))).tcp_nodelay(true).connect_timeout(Duration::from_secs(10)).pool_idle_timeout(Duration::from_secs(90));
    if purpose!=Purpose::Inference {builder=builder.redirect(reqwest::redirect::Policy::none());}
    if matches!(purpose,Purpose::OAuth|Purpose::OpenAiOAuth){builder=builder.connect_timeout(Duration::from_secs(5)).timeout(Duration::from_secs(20));}
    if legacy_openai_environment(purpose,&row.mode){builder=crate::openai_transport::configure(builder).map_err(str::to_owned)?;}
    builder=match row.mode.as_str(){
        "inherit"=>builder,
        "direct"=>builder.no_proxy(),
        "proxy"=>{
            let url=proxy_url(row.proxy_url.as_deref().ok_or("egress_invalid_stored_policy")?)?;
            let mut proxy=reqwest::Proxy::all(&url).map_err(|_|"egress_proxy_configuration_invalid")?;
            match (row.credential_nonce,row.credential_cipher){
                (Some(n),Some(c))=>{let raw=state.secrets.open(&n,&c).map_err(|_|"egress_proxy_credentials_unavailable")?;let credentials:Credentials=serde_json::from_str(&raw).map_err(|_|"egress_proxy_credentials_unavailable")?;proxy=proxy.basic_auth(&credentials.username,&credentials.password);},
                (None,None)=>{},_=>return Err("egress_proxy_credentials_unavailable".into()),
            }
            builder.no_proxy().proxy(proxy)
        },_=>return Err("egress_invalid_stored_policy".into()),
    };
    let result=builder.build().map_err(|_|"egress_proxy_client_unavailable".to_owned())?;
    let mut cache=state.egress.clients.lock().unwrap_or_else(|e|e.into_inner());if cache.len()>=256{cache.clear();}cache.insert(key,result.clone());Ok(result)
}
pub fn source_client(state:&SharedState,channel:&tokengo_shared::Channel,purpose:Purpose)->Result<reqwest::Client,String>{
    let row={let c=state.conn();selected(&c,Some(channel.id.0),None,None,&channel.base_url)?};client(state,row,purpose)
}
pub fn oauth_client(state:&SharedState,provider:&str,account_id:Option<i64>,origin_provider_id:Option<i64>)->Result<reqwest::Client,String>{
    let row={let c=state.conn();
        if let Some(id)=origin_provider_id{target(&c,"provider",&id.to_string())?;}
        let sources=if let Some(id)=account_id{let mut q=c.prepare("SELECT channel_id FROM oauth_channel_bindings WHERE account_id=?1 ORDER BY channel_id").map_err(|_|"egress_storage_unavailable")?;let rows=q.query_map([id],|r|r.get::<_,i64>(0)).map_err(|_|"egress_storage_unavailable")?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|"egress_storage_unavailable")?;rows}else{vec![]};
        if sources.is_empty(){selected(&c,None,origin_provider_id,Some(oauth_family(provider)),"")?}
        else {let rows=sources.into_iter().map(|source|selected(&c,Some(source),None,Some(oauth_family(provider)),"")).collect::<Result<Vec<_>,_>>()?;consistent_routes(&state.secrets,rows)?}
    };client(state,row,if provider=="openai"{Purpose::OpenAiOAuth}else{Purpose::OAuth})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_url_rejects_embedded_credentials_and_unsupported_schemes_without_echo(){
        for raw in ["http://u:SECRET@example.com:8080","socks5://example.com:1080","https://example.com/path","http://example.com?token=SECRET"]{let error=proxy_url(raw).unwrap_err();assert!(!error.contains("SECRET"));}
        assert_eq!(proxy_url("http://127.0.0.1:7890/").unwrap(),"http://127.0.0.1:7890");
    }
    #[test]
    fn independently_sealed_equal_proxy_credentials_are_the_same_account_route(){
        let (secrets,_)=SecretBox::generate();
        let row=|password:&str|{let(n,c)=secrets.seal(&serde_json::to_string(&Credentials{username:"proxy-user".into(),password:password.into()}).unwrap()).unwrap();Stored{mode:"proxy".into(),proxy_url:Some("http://127.0.0.1:7890".into()),credential_nonce:Some(n),credential_cipher:Some(c),..Default::default()}};
        let a=row("same-secret");let b=row("same-secret");
        assert_ne!(a.credential_nonce,b.credential_nonce);
        assert!(consistent_routes(&secrets,vec![a.clone(),b]).is_ok());
        assert!(matches!(consistent_routes(&secrets,vec![a,row("different-secret")]),Err(reason) if reason.contains("ambiguous")));
    }
    #[test]
    fn credentials_are_encrypted_redacted_and_proxy_changes_require_explicit_action(){
        let c=db::open_in_memory().unwrap();let (secrets,_)=SecretBox::generate();
        let write=|version,action:&str,url:&str|Write{mode:"proxy".into(),proxy_url:Some(url.into()),credential_action:action.into(),username:if action=="replace"{Some("proxy-user".into())}else{None},password:if action=="replace"{Some("PROXY_SECRET".into())}else{None},expected_version:version,network_family:None};
        let public=save(&c,&secrets,"family","gpt",write(0,"replace","http://127.0.0.1:7890")).unwrap();
        assert_eq!(public["has_credentials"],true);assert!(!public.to_string().contains("PROXY_SECRET"));assert!(public.get("username").is_none());
        let row=load(&c,"family","gpt").unwrap();let raw=secrets.open(row.credential_nonce.as_ref().unwrap(),row.credential_cipher.as_ref().unwrap()).unwrap();let decrypted:Credentials=serde_json::from_str(&raw).unwrap();assert_eq!(decrypted.password,"PROXY_SECRET");
        assert!(save(&c,&secrets,"family","gpt",write(0,"keep","http://127.0.0.1:7890")).unwrap_err().contains("version_conflict"));
        assert!(save(&c,&secrets,"family","gpt",write(1,"keep","http://127.0.0.1:7891")).is_err());
        let public=save(&c,&secrets,"family","gpt",write(1,"clear","http://127.0.0.1:7891")).unwrap();assert_eq!(public["has_credentials"],false);
    }
    #[test]
    fn explicit_policy_overrides_do_not_parse_legacy_openai_proxy_environment(){
        assert!(legacy_openai_environment(Purpose::Subscription,"inherit"));
        assert!(!legacy_openai_environment(Purpose::Subscription,"direct"));
        assert!(!legacy_openai_environment(Purpose::OpenAiOAuth,"proxy"));
        assert!(!legacy_openai_environment(Purpose::OAuth,"inherit"));
    }
    #[test]
    fn source_provider_family_precedence_and_family_override_are_owned(){
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT);CREATE TABLE provider_connections(channel_id INTEGER,provider_id INTEGER);INSERT INTO provider_connections VALUES(7,3);").unwrap();
        let put=|scope:&str,id:&str,mode:&str,family:Option<&str>|{let row=Stored{mode:mode.into(),network_family:family.map(str::to_owned),..Default::default()};db::settings::set(&c,&key(scope,id),&serde_json::to_string(&row).unwrap()).unwrap();};
        put("family","gpt","direct",None);put("provider","3","inherit",Some("gpt"));
        assert_eq!(selected(&c,Some(7),None,None,"http://custom.invalid").unwrap().mode,"direct");
        put("provider","3","proxy",Some("gpt"));
        assert_eq!(selected(&c,Some(7),None,None,"http://custom.invalid").unwrap().mode,"proxy");
        put("source","7","direct",None);
        assert_eq!(selected(&c,Some(7),None,None,"http://custom.invalid").unwrap().mode,"direct");
        let a=Stored{mode:"direct".into(),version:1,network_family:Some("gpt".into()),..Default::default()};
        let b=Stored{version:9,network_family:Some("kimi".into()),..a.clone()};
        assert_eq!(identity(&a).unwrap(),identity(&b).unwrap(),"equivalent direct policies are not an OAuth proxy conflict");
    }
    #[test]
    fn family_is_selected_from_official_endpoint_not_model_name(){
        assert_eq!(exact_family("https://api.openai.com/v1"),Some("gpt"));
        assert_eq!(exact_family("https://api.openai.com.evil/v1"),None);
        assert_eq!(exact_family("http://127.0.0.1:8000/v1"),None);
    }
}
