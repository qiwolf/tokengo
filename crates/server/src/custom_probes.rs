//! Administrator-owned bounded diagnostics. Configuration and short leases live in settings;
//! probes record system diagnostic costs without charging users or change routing/fallback health.
use crate::{db, gateway::EndpointKind, routes::{ApiError, operations::ManagementAuth}, SharedState};
use axum::{extract::{Path, State}, routing::{get, post, put}, Json, Router};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokengo_shared::{Channel, ChannelId, same_model_name};

const PREFIX: &str = "custom_probe:";
const LEASE_PREFIX: &str = "custom_probe_lease:";
const MAX_PROBES: usize = 64;
const RESPONSE_LIMIT: usize = 256 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub name: String,
    pub source_id: i64,
    pub model: String,
    pub kind: String,
    pub input: String,
    pub max_tokens: u32,
    pub expected_status: u16,
    pub contains: Option<String>,
    pub expected_dimensions: Option<u32>,
    pub timeout_ms: u64,
    pub max_latency_ms: Option<u64>,
    pub interval_secs: u64,
    pub enabled: bool,
    #[serde(default="default_search_count")]
    pub search_count:u32,
    #[serde(default)]
    pub need_content:bool,
    #[serde(default)]
    pub localization:Option<String>,
    #[serde(default)]
    pub ori_question:Option<String>,
}
fn default_search_count()->u32 {1}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProbeResult {
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    pub note: String,
    pub at_ms: i64,
    #[serde(default)]
    pub billing:Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Probe {
    pub id: String,
    #[serde(flatten)]
    pub config: Config,
    pub version: u64,
    pub last_result: Option<ProbeResult>,
    pub next_run_at_ms: i64,
}
#[derive(Deserialize)]
struct Write {
    #[serde(flatten)]
    config: Config,
    version: Option<u64>,
}
#[derive(Deserialize)]
struct Delete { version: Option<u64> }
#[derive(Serialize, Deserialize)]
struct Lease { token: String, version: u64, until_ms: i64 }
fn conflict(message: &str) -> ApiError { ApiError::new(axum::http::StatusCode::CONFLICT,"custom_probe_conflict",message) }
fn db_bad(message: &str) -> db::DbError { db::DbError::Other(message.into()) }
fn key(id:&str)->String { format!("{PREFIX}{id}") }
fn lease_key(id:&str)->String { format!("{LEASE_PREFIX}{id}") }
fn valid_id(id:&str)->Result<(),ApiError> { uuid::Uuid::parse_str(id).map(|_|()).map_err(|_|ApiError::not_found("探针不存在")) }
fn get_one(c:&Connection,id:&str)->db::Result<Option<Probe>> {
    db::settings::get(c,&key(id))?.map(|raw|serde_json::from_str(&raw).map_err(|_|db_bad("探针配置损坏，请重新创建"))).transpose()
}
fn store(c:&Connection,p:&Probe)->db::Result<()> { db::settings::set(c,&key(&p.id),&serde_json::to_string(p).map_err(|_|db_bad("探针配置无法保存"))?) }
pub fn list(c:&Connection)->db::Result<Vec<Probe>> {
    let mut q=c.prepare("SELECT value FROM settings WHERE key LIKE 'custom_probe:%' ORDER BY key")?;
    let rows=q.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(|raw|serde_json::from_str(&raw).map_err(|_|db_bad("探针配置损坏，请联系管理员"))).collect()
}
fn endpoint(kind:&str)->Option<EndpointKind> { Some(match kind {"chat"=>EndpointKind::Chat,"responses"=>EndpointKind::Responses,"messages"=>EndpointKind::Messages,"embeddings"=>EndpointKind::Embeddings,"search"=>EndpointKind::Search,"fetch"=>EndpointKind::Fetch,_=>return None}) }
impl Config {
    fn validate(&mut self)->Result<(),String> {
        self.name=self.name.trim().into();self.model=self.model.trim().into();
        if self.name.is_empty() || self.name.chars().count()>80 || self.name.chars().any(char::is_control) {return Err("探针名称须为1至80字且不含控制字符".into());}
        if self.source_id<=0 || self.model.is_empty() || self.model.len()>1024 || db::model_routes::is_auto(&self.model) || self.model.chars().any(char::is_control) {return Err("请选择真实上游接入及其本地映射模型，不能使用 auto".into());}
        if endpoint(&self.kind).is_none() {return Err("请选择支持的对话、向量、搜索或网页提取探针类型".into());}
        if endpoint(&self.kind).and_then(EndpointKind::tool_kind).is_some() {tool_body(self)?;} else if self.input.is_empty() || self.input.len()>4096 {return Err("探测输入须为1至4096字节".into());}
        if !(1..=256).contains(&self.max_tokens) || !(200..=599).contains(&self.expected_status) || !(1000..=60000).contains(&self.timeout_ms) || !(60..=86400).contains(&self.interval_secs) {return Err("输出上限1至256，预期状态200至599，超时1至60秒，间隔60至86400秒".into());}
        if self.contains.as_ref().is_some_and(|s|s.len()>512) {return Err("匹配文本最多512字节".into());}
        self.contains=self.contains.take().filter(|s|!s.is_empty());
        if self.kind=="embeddings"&&self.contains.is_some(){return Err("向量探针请使用维度检查，不使用对话文本匹配".into());}
        if self.expected_dimensions.is_some_and(|n|n==0||n>65536) || (self.expected_dimensions.is_some()&&self.kind!="embeddings") {return Err("向量维度仅用于向量探针，范围1至65536".into());}
        if self.max_latency_ms.is_some_and(|n|n==0||n>60000) {return Err("延迟上限须为1至60000毫秒".into());}
        Ok(())
    }
}
struct Ready { channel: Channel, secret: Option<String>, upstream_model: String, endpoint: EndpointKind, signature: String, credential:db::provider_pool::CredentialRef, oauth:bool, tools:crate::search_tools::ToolCapabilities }
fn ready(c:&Connection,state:&SharedState,cfg:&Config,require_enabled:bool)->Result<Ready,String> {
    let row=db::channels::get(c,ChannelId(cfg.source_id)).map_err(|_|"无法读取上游接入")?.ok_or("绑定的上游接入已删除，请编辑探针重新选择")?;
    let credential=db::provider_pool::source_credential(c,cfg.source_id).map_err(|_|"无法读取上游 Key")?.ok_or("该上游接入没有有效的渠道 Key 归属")?;
    if require_enabled && (!row.enabled||!row.provider_enabled||!credential.enabled) {return Err("渠道、上游 Key 或 API 接入已停用，未发送探测请求".into());}
    let identity=db::provider_pool::credentials(c,credential.provider_id).map_err(|_|"无法读取上游凭据类型")?.into_iter().find(|k|k.id==credential.id).ok_or("上游 Key 已删除")?;
    let oauth=identity.kind=="oauth";
    let tools=crate::search_tools::load(c,cfg.source_id).map_err(|_|"无法读取工具能力")?;
    let endpoint=endpoint(&cfg.kind).ok_or("探针类型无效")?;
    if oauth {
        if cfg.kind!="responses" {return Err("OAuth 订阅仅支持 Responses 原生探针".into());}
        let active:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings b JOIN oauth_accounts a ON a.id=b.account_id WHERE b.channel_id=?1 AND a.status='active')",[cfg.source_id],|r|r.get(0)).map_err(|_|"无法读取 OAuth 账号状态")?;
        if !active {return Err("OAuth 账号未绑定、已撤销或需要重新授权".into());}
    }
    if let Some(kind)=endpoint.tool_kind() {if oauth||!tools.permits(kind){return Err("该上游接入未启用所选独立工具".into());}}
    else if tools.search||tools.fetch {return Err("独立工具接入请选择搜索或网页提取探针".into());}
    let mapping=row.models.iter().find(|m|same_model_name(&m.model,&cfg.model)).ok_or("此接入未配置所选模型，请编辑探针重新选择")?;
    if require_enabled && (!mapping.enabled||!db::model_routes::enabled(c,&mapping.model).map_err(|_|"无法读取模型状态")?) {return Err("模型已停用，未发送探测请求".into());}
    let known=db::kinds::all(c).map_err(|_|"无法读取模型类型")?.into_iter().find(|k|same_model_name(&k.name,&mapping.model)).map(|k|k.kind);
    if cfg.kind=="embeddings"&&known!=Some(tokengo_shared::ModelKind::Embeddings) {return Err("请先在模型管理中将此模型标记为向量类型，再配置向量探针".into());}
    if endpoint.is_generation()&&known.is_some_and(|kind|!matches!(kind,tokengo_shared::ModelKind::Chat|tokengo_shared::ModelKind::Unknown)) {return Err("所选模型不是对话类型，不能使用对话协议探针".into());}

    if let Some(protocol)=endpoint.protocol() {
        if !crate::protocol_config::load(c,cfg.source_id).map_err(|_|"上游协议配置无效")?.native.contains(&protocol) {return Err("此接入未声明支持所选原生协议，请选择正确的探针类型".into());}
    }
    let upstream_model=mapping.upstream.clone().unwrap_or_else(||mapping.model.clone());
    let secret=db::channels::secret(c,&state.secrets,row.id).map_err(|_|"无法读取上游凭据，请检查 Key 配置")?;
    let headers=db::channels::headers(c,&state.secrets,row.id).map_err(|_|"无法读取上游请求头")?;
    let signature=serde_json::to_string(&(crate::reasoning_capabilities::fingerprint(c,cfg.source_id).map_err(|_|"无法确认上游配置版本")?,credential.id,credential.provider_id,credential.revision,credential.provider_version,c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[cfg.source_id],|r|r.get::<_,i64>(0)).map_err(|_|"无法读取绑定版本")?,&upstream_model,row.unwrap_envelope,crate::protocol_config::load(c,cfg.source_id).map_err(|_|"上游协议配置无效")?)).map_err(|_|"无法确认上游配置版本")?;
    Ok(Ready{credential,oauth,tools,channel:Channel{id:row.id,name:row.name,base_url:row.base_url,enabled:row.enabled,priority:0,model_alias:None,vision:false,drop_fields:vec![],unwrap_envelope:row.unwrap_envelope,headers,max_concurrent:row.max_concurrent},secret,upstream_model,endpoint,signature})
}
fn ready_binding_signature(c:&Connection,state:&SharedState,cfg:&Config,signature:&str)->bool {
    ready(c,state,cfg,true).is_ok_and(|next|next.signature==signature)
}
pub fn router()->Router<SharedState> {
    Router::new().route("/api/custom-probes",get(index).post(create))
        .route("/api/custom-probes/{id}",put(update).delete(remove))
        .route("/api/custom-probes/{id}/run",post(run))
}
async fn index(_root:ManagementAuth,State(state):State<SharedState>)->Result<Json<Value>,ApiError> {
    let c=state.conn();let mut targets=Vec::new();let mut eligible=Vec::new();
    for row in db::channels::list(&c)? {
        let Some(cred)=db::provider_pool::source_credential(&c,row.id.0)? else {continue};
        let Some(identity)=db::provider_pool::credentials(&c,cred.provider_id)?.into_iter().find(|k|k.id==cred.id)else{continue};
        let tools=crate::search_tools::load(&c,row.id.0)?;let protocol=crate::protocol_config::load(&c,row.id.0)?;
        let native=protocol.native.iter().map(|p|serde_json::to_value(p).unwrap_or(Value::Null)).collect::<Vec<_>>();
        let oauth=identity.kind=="oauth";
        let account:Option<(String,String)>=if oauth {c.query_row("SELECT a.label,a.status FROM oauth_channel_bindings b JOIN oauth_accounts a ON a.id=b.account_id WHERE b.channel_id=?1",[row.id.0],|r|Ok((r.get(0)?,r.get(1)?))).ok()}else{None};
        let available=row.enabled&&row.provider_enabled&&cred.enabled&&(!oauth||account.as_ref().is_some_and(|(_,s)|s=="active"));
        if available{eligible.push(row.id.0);}
        let kinds=db::kinds::all(&c)?;
        let models=row.models.iter().filter(|m|m.enabled&&db::model_routes::enabled(&c,&m.model).unwrap_or(false)).map(|m|json!({"name":m.model,"kind":kinds.iter().find(|k|same_model_name(&k.name,&m.model)).map(|k|k.kind)})).collect::<Vec<_>>();
        targets.push(json!({"source_id":row.id.0,"provider_name":row.provider_name,"credential_kind":identity.kind,"credential_label":identity.label,"account_label":account.as_ref().map(|(l,_)|l),"native_kinds":native,"models":models,"search":tools.search,"fetch":tools.fetch,"available":available,"availability_note":if available{"可用于系统诊断"}else{"渠道、Key 或账号不可用，请修复后重试"},"cost_note":if oauth{"订阅用量记录为系统诊断，USD 成本未知；输出受超时与响应大小保护"}else{"实际用量与来源价格记录为系统诊断，不扣用户额度；缺少价格时成本未知"}}));
    }
    Ok(Json(json!({"probes":list(&c)?,"eligible_source_ids":eligible,"targets":targets})))
}
async fn create(_root:ManagementAuth,State(state):State<SharedState>,Json(mut body):Json<Write>)->Result<Json<Value>,ApiError> {
    body.config.validate().map_err(ApiError::bad_request)?;
    let c=state.conn();
    if list(&c)?.len()>=MAX_PROBES {return Err(ApiError::bad_request("最多配置64个自定义探针"));}
    ready(&c,&state,&body.config,body.config.enabled).map_err(ApiError::bad_request)?;
    let probe=Probe{id:uuid::Uuid::new_v4().to_string(),config:body.config,version:1,last_result:None,next_run_at_ms:db::now_ms()+60_000};
    // Save never makes a paid request. First background execution waits a full interval.
    let probe=Probe{next_run_at_ms:db::now_ms()+probe.config.interval_secs as i64*1000,..probe};
    store(&c,&probe)?;Ok(Json(json!({"probe":probe})))
}
async fn update(_root:ManagementAuth,State(state):State<SharedState>,Path(id):Path<String>,Json(mut body):Json<Write>)->Result<Json<Value>,ApiError> {
    valid_id(&id)?;body.config.validate().map_err(ApiError::bad_request)?;
    let c=state.conn();let old=get_one(&c,&id)?.ok_or_else(||ApiError::not_found("探针不存在"))?;
    if body.version!=Some(old.version) {return Err(conflict("探针已被修改，请刷新后再保存"));}
    // A stale/missing source can always be disabled and repaired without a 500.
    if body.config.enabled || body.config.source_id!=old.config.source_id || body.config.model!=old.config.model || body.config.kind!=old.config.kind {ready(&c,&state,&body.config,body.config.enabled).map_err(ApiError::bad_request)?;}
    let version=old.version.checked_add(1).ok_or_else(||conflict("探针版本已达上限，请重新创建"))?;
    let probe=Probe{id,version,next_run_at_ms:db::now_ms()+body.config.interval_secs as i64*1000,config:body.config,last_result:None};
    store(&c,&probe)?;Ok(Json(json!({"probe":probe})))
}
async fn remove(_root:ManagementAuth,State(state):State<SharedState>,Path(id):Path<String>,body:Option<Json<Delete>>)->Result<Json<Value>,ApiError> {
    valid_id(&id)?;let c=state.conn();let old=get_one(&c,&id)?.ok_or_else(||ApiError::not_found("探针不存在"))?;
    if body.and_then(|v|v.version).is_some_and(|v|v!=old.version) {return Err(conflict("探针已被修改，请刷新后再删除"));}
    db::in_tx(&c,|c|{db::settings::delete(c,&key(&id))?;db::settings::delete(c,&lease_key(&id))})?;
    Ok(Json(json!({"deleted":true})))
}
async fn run(root:crate::routes::operations::ManagementGrant,State(state):State<SharedState>,Path(id):Path<String>)->Result<Json<Value>,ApiError> { valid_id(&id)?;Ok(Json(json!({"result":execute_actor(&state,&id,false,db::now_ms(),Some(root.actor_account_id())).await?}))) }
fn tool_body(cfg:&Config)->Result<Value,String> {
    let kind=endpoint(&cfg.kind).and_then(EndpointKind::tool_kind).ok_or("不是工具探针")?;
    let mut value=if cfg.kind=="search" {json!({"query":cfg.input,"count":cfg.search_count,"need_content":cfg.need_content,"localization":cfg.localization.as_deref().unwrap_or("")})}else{json!({"url":cfg.input})};
    if let Some(question)=&cfg.ori_question {value["ori_question"]=json!(question);}
    crate::search_tools::normalize_request(kind,&value)
}
fn body(cfg:&Config,model:&str)->bytes::Bytes {
    let value=match cfg.kind.as_str(){
        "search"|"fetch"=>tool_body(cfg).expect("validated tool input"),
        "responses"=>json!({"model":model,"input":cfg.input,"max_output_tokens":cfg.max_tokens,"stream":false,"store":false}),
        "messages"=>json!({"model":model,"messages":[{"role":"user","content":cfg.input}],"max_tokens":cfg.max_tokens,"stream":false}),
        "embeddings"=>json!({"model":model,"input":cfg.input,"encoding_format":"float"}),
        _=>json!({"model":model,"messages":[{"role":"user","content":cfg.input}],"max_tokens":cfg.max_tokens,"stream":false}),
    };bytes::Bytes::from(value.to_string())
}
/// Match generated assistant output only, never model IDs, request echoes or JSON field names.
fn assistant_text(value:&Value,kind:&str)->String {
    let mut text=Vec::<String>::new();
    let mut add=|v:&Value|{if let Some(s)=v.as_str(){text.push(s.to_string());}};
    match kind {
        "chat"=>if let Some(choices)=value["choices"].as_array(){for choice in choices{let content=&choice["message"]["content"];if content.is_string(){add(content);}else if let Some(parts)=content.as_array(){for part in parts{if part["type"]=="text"{add(&part["text"]);}}}}},
        "responses"=>if let Some(items)=value["output"].as_array(){for item in items {if item["type"]=="message"&&item["role"]=="assistant" {if let Some(parts)=item["content"].as_array(){for part in parts{if part["type"]=="output_text"{add(&part["text"]);}}}}}},
        "messages"=>if let Some(parts)=value["content"].as_array(){for part in parts{if part["type"]=="text"{add(&part["text"]);}}},
        "search"=>if let Some(results)=value["results"].as_array(){for result in results {for field in ["title","snippet","summary","content"]{add(&result[field]);}}},
        "fetch"=>add(&value["content"]),
        _=>{},
    }
    text.join("\n")
}
fn evaluate(cfg:&Config,code:u16,bytes:&[u8],latency:u64,unwrap:bool)->(bool,String) {
    if code!=cfg.expected_status {return (false,format!("预期 HTTP {}，实际 HTTP {code}",cfg.expected_status));}
    if cfg.max_latency_ms.is_some_and(|limit|latency>limit) {return (false,"响应延迟超过设定上限".into());}
    if (200..300).contains(&code) {
        let Ok(mut value)=serde_json::from_slice::<Value>(bytes)else{return(false,"成功状态的响应不是有效 JSON".into())};
        if unwrap {tokengo_shared::unwrap_envelope(&mut value);}
        if if let Some(kind)=endpoint(&cfg.kind).and_then(EndpointKind::tool_kind) {crate::search_tools::response_units(kind,code,&value).is_err()}else{crate::upstream::validate_response(&value,endpoint(&cfg.kind).unwrap().path()).is_err()} {return(false,"响应结构与所选模型接口不匹配或内容为空".into());}
        if cfg.kind=="embeddings" {
            let valid=value["data"].as_array().is_some_and(|rows|!rows.is_empty()&&rows.iter().all(|row|row["embedding"].as_array().is_some_and(|array|!array.is_empty()&&array.len()<=65536&&array.iter().all(|x|x.as_f64().is_some_and(f64::is_finite))&&cfg.expected_dimensions.is_none_or(|n|array.len()==n as usize))));
            if !valid {return(false,"向量结果无效或维度与设定不符".into());}
        }
    }
    if let Some(needle)=&cfg.contains {
        let mut value:Value=serde_json::from_slice(bytes).unwrap_or(Value::Null);
        if unwrap {tokengo_shared::unwrap_envelope(&mut value);}
        let output=if (200..300).contains(&code){assistant_text(&value,&cfg.kind)}else{value.pointer("/error/message").and_then(Value::as_str).unwrap_or("").to_string()};
        if !output.contains(needle){return(false,"助手输出或结构化错误说明未包含指定匹配文本".into());}
    }
    (true,"预期状态、内容及延迟条件均通过".into())
}
fn claim(state:&SharedState,id:&str,background:bool,now:i64)->Result<(Probe,String),ApiError> {
    let c=state.conn();let mut probe=get_one(&c,id)?.ok_or_else(||ApiError::not_found("探针不存在"))?;
    if background&&(!probe.config.enabled||probe.next_run_at_ms>now) {return Err(conflict("探针未启用或尚未到执行时间"));}
    let actual_now=db::now_ms();
    if let Some(raw)=db::settings::get(&c,&lease_key(id))? {
        if serde_json::from_str::<Lease>(&raw).is_ok_and(|lease|lease.until_ms>actual_now) {return Err(conflict("此探针正在执行，请勿重复运行"));}
    }
    let token=uuid::Uuid::new_v4().to_string();
    let lease=Lease{token:token.clone(),version:probe.version,until_ms:actual_now+probe.config.timeout_ms as i64+5000};
    probe.next_run_at_ms=actual_now+probe.config.interval_secs as i64*1000;
    db::in_tx(&c,|c|{store(c,&probe)?;db::settings::set(c,&lease_key(id),&serde_json::to_string(&lease).unwrap())})?;
    Ok((probe,token))
}
fn current(state:&SharedState,probe:&Probe,token:&str,background:bool)->db::Result<bool> {
    let c=state.conn();let Some(next)=get_one(&c,&probe.id)?else{return Ok(false)};
    let lease=db::settings::get(&c,&lease_key(&probe.id))?.and_then(|raw|serde_json::from_str::<Lease>(&raw).ok());
    Ok(next.version==probe.version&&(!background||next.config.enabled)&&lease.is_some_and(|lease|lease.token==token&&lease.version==probe.version))
}
fn finish(state:&SharedState,probe:&Probe,token:&str,result:&ProbeResult)->db::Result<()> {
    let c=state.conn();let lease=db::settings::get(&c,&lease_key(&probe.id))?.and_then(|raw|serde_json::from_str::<Lease>(&raw).ok());
    if !lease.is_some_and(|lease|lease.token==token) {return Ok(())}
    db::in_tx(&c,|c|{
        if let Some(mut current)=get_one(c,&probe.id)? {if current.version==probe.version {current.last_result=Some(result.clone());store(c,&current)?;}}
        db::settings::delete(c,&lease_key(&probe.id))
    })
}
struct RunGuard {state:SharedState,probe:Probe,token:String,ledger:Option<i64>,sent:bool,request_based:bool,done:bool}
impl Drop for RunGuard {fn drop(&mut self){if self.done{return}if let Some(id)=self.ledger {let _=crate::billing_writer::settle_diagnostic(&self.state,id,None,None,None,self.request_based,if self.sent{"cancelled_unknown"}else{"not_sent"});}let result=ProbeResult{ok:false,status:None,latency_ms:0,note:"执行已取消；系统诊断用量按发送状态保留，未扣用户额度".into(),at_ms:db::now_ms(),billing:None};let _=finish(&self.state,&self.probe,&self.token,&result);}}
fn billing_summary(state:&SharedState,id:i64)->db::Result<Value>{
    let c=state.conn();let row=db::billing::usage(&c,&db::billing::UsageQuery{user_id:Some(0),before_id:Some(id+1),limit:Some(1),..Default::default()})?.into_iter().find(|r|r.id==id).ok_or_else(||db_bad("系统诊断记录缺失"))?;
    Ok(json!({"ledger_id":id,"billing_unit":row.billing_unit,"request_units":row.request_units,"input_tokens":row.input_tokens,"output_tokens":row.output_tokens,"observed_cost_micro":row.observed_cost_micro,"debit_micro":0,"usage_certainty":row.usage_certainty,"cost_status":if row.observed_cost_micro.is_some(){"known"}else if row.auth_snapshot["subscription"]==true{"subscription"}else{"unknown"},"note":if row.observed_cost_micro.is_some(){"系统诊断成本，不扣用户额度"}else{"系统诊断成本未知，不表示免费；未扣用户额度"}}))
}
pub async fn execute(state:&SharedState,id:&str,background:bool,now:i64)->Result<ProbeResult,ApiError>{execute_actor(state,id,background,now,None).await}
async fn execute_actor(state:&SharedState,id:&str,background:bool,now:i64,actor:Option<i64>)->Result<ProbeResult,ApiError> {
    let (probe,token)=claim(state,id,background,now)?;
    let mut guard=RunGuard{state:state.clone(),probe:probe.clone(),token:token.clone(),ledger:None,sent:false,request_based:endpoint(&probe.config.kind).and_then(EndpointKind::tool_kind).is_some(),done:false};
    let started=Instant::now();
    let mut result=ProbeResult{ok:false,status:None,latency_ms:0,note:String::new(),at_ms:db::now_ms(),billing:None};
    let work=async {
        tokio::task::yield_now().await;
        if !current(state,&probe,&token,background).map_err(|_|"无法确认探针状态")?{return Err("探针已删除、停用或修改，未发出新请求");}
        let initial={let c=state.conn();ready(&c,state,&probe.config,true).map_err(|_|"绑定的上游接入已删除、停用或凭据不可用，未发送请求")?};
        // Persist before OAuth refresh or waiting for a paid dispatch. No customer identity is used.
        let auth=json!({"kind":"admin_diagnostic","probe_id":id,"probe_name":probe.config.name,"run_id":token,"actor_account_id":if crate::routes::operations::actor_id().is_some(){None}else{actor},"actor_type":if crate::routes::operations::actor_id().is_some(){"operations_token"}else{"root_account"},"actor_id":crate::routes::operations::actor_id().or(actor),"trigger":if background{"scheduled"}else{"manual"},"subscription":initial.oauth});
        let snapshot=json!({"source_id":probe.config.source_id,"provider_id":initial.credential.provider_id,"credential_id":initial.credential.id,"credential_revision":initial.credential.revision,"provider_version":initial.credential.provider_version,"source_name":initial.channel.name,"origin":reqwest::Url::parse(&initial.channel.base_url).ok().map(|u|u.origin().ascii_serialization()),"model":probe.config.model,"kind":probe.config.kind});
        let ledger={let c=state.conn();db::billing::reserve_diagnostic(&c,&format!("diagnostic:{token}"),probe.config.source_id,&probe.config.model,guard.request_based,initial.oauth,&auth,&snapshot).map_err(|_|"系统诊断记账不可用，未发送请求")?};guard.ledger=Some(ledger);
        let oauth=if initial.oauth{Some(crate::oauth::credentials(state,probe.config.source_id).await.map_err(|_|"OAuth 授权刷新失败或绑定已变化，请检查账号状态")?.ok_or("OAuth 绑定已删除")?)}else{None};
        let _source_permit=match state.channel_gate(initial.channel.id.0,initial.channel.max_concurrent){Some(gate)=>Some(gate.acquire_owned().await.map_err(|_|"接入并发控制不可用")?),None=>None};
        let _key_permit=match state.credential_gate(initial.credential.id,initial.credential.max_concurrent){Some(gate)=>Some(gate.acquire_owned().await.map_err(|_|"Key 并发控制不可用")?),None=>None};
        if !current(state,&probe,&token,background).map_err(|_|"无法确认探针状态")?{return Err("探针已删除、停用或修改，未发出新请求");}
        let live={let c=state.conn();let next=ready(&c,state,&probe.config,true).map_err(|_|"接入已停用或配置无效，未发送请求")?;if next.signature!=initial.signature{return Err("接入配置在等待期间变化，未发送请求")}next};
        if let Some(credential)=&oauth {if !crate::oauth::credentials_current(state,probe.config.source_id,credential).map_err(|_|"OAuth 绑定不可用")?{return Err("OAuth 绑定或账号已变化，未发送请求")}}
        let mut payload=body(&probe.config,&live.upstream_model);
        if let Some(credential)=&oauth {
            let mut value=json!({"model":live.upstream_model,"input":probe.config.input,"stream":true,"store":false});
            crate::protocol::collect::prepare_request(&mut value,credential.reasoning_effort.as_deref(),credential.service_tier.as_deref()).map_err(|_|"OAuth 请求参数不受支持，未发送请求")?;
            payload=bytes::Bytes::from(value.to_string());
        }
        if let Some(kind)=live.endpoint.tool_kind(){if !state.tool_rates.admit(&live.tools.account_scope,kind,live.tools.qps){return Err("供应方工具账户达到每秒请求限制，未发送请求");}}
        // Refresh can rotate the account credential; capture the version actually used for dispatch.
        {let c=state.conn();let mut snapshot=snapshot.clone();snapshot["provider_id"]=json!(live.credential.provider_id);snapshot["credential_id"]=json!(live.credential.id);snapshot["credential_revision"]=json!(live.credential.revision);snapshot["provider_version"]=json!(live.credential.provider_version);if let Some(o)=&oauth{snapshot["oauth_account_id"]=json!(o.account_id);snapshot["oauth_credential_revision"]=json!(o.credential_revision);snapshot["binding_generation"]=json!(o.binding_generation);}
            db::billing::prepare_diagnostic_dispatch(&c,ledger,&snapshot).map_err(|_|"无法保存实际来源快照，未发送请求")?;}
        guard.sent=true;
        let timeout=Duration::from_millis(probe.config.timeout_ms);
        let response=if let Some(o)=&oauth{state.upstream.custom_probe_oauth_request(&live.channel,o,payload,timeout,RESPONSE_LIMIT).await}else{state.upstream.custom_probe_request(&live.channel,live.secret.as_deref(),live.endpoint.path(),payload,timeout,RESPONSE_LIMIT).await};
        let (code,bytes)=match response{Ok(value)=>value,Err(error)=>{if live.oauth&&error.message.starts_with("oauth_response_"){result.note=format!("OAuth 响应检查失败：{}；未记录上游正文",error.message);return Ok(());}return Err("上游连接、读取或响应大小检查失败；未记录上游正文")}};
        result.status=Some(code);let latency=started.elapsed().as_millis()as u64;
        let (ok,note)=evaluate(&probe.config,code,&bytes,latency,live.channel.unwrap_envelope);result.ok=ok;result.note=note;
        let mut value:Value=serde_json::from_slice(&bytes).unwrap_or(Value::Null);if live.channel.unwrap_envelope{tokengo_shared::unwrap_envelope(&mut value);}
        let (input,output,units)=if let Some(kind)=live.endpoint.tool_kind(){(None,None,crate::search_tools::response_units(kind,code,&value).ok().map(i64::from))}else{let usage=crate::gateway::Usage::read(&value);(usage.input,usage.output,None)};
        crate::billing_writer::settle_diagnostic(state,ledger,input,output,units,guard.request_based,if (200..300).contains(&code){"success"}else{"upstream_error"}).map_err(|_|"系统诊断结算待恢复，请检查账本")?;
        let unchanged={let c=state.conn();ready_binding_signature(&c,state,&probe.config,&live.signature)};
        if !unchanged||oauth.as_ref().is_some_and(|o|!crate::oauth::credentials_current(state,probe.config.source_id,o).unwrap_or(false)){result.ok=false;result.note="上游接入配置已变化，本次旧配置结果不作为有效验证".into();}
        Ok::<(),&str>(())
    };
    match tokio::time::timeout(Duration::from_millis(probe.config.timeout_ms),work).await {
        Ok(Ok(()))=>{},Ok(Err(note))=>{result.ok=false;result.note=note.into();},Err(_)=>{result.ok=false;result.note="探测超过设定超时，已停止等待或读取响应".into();}
    }
    if let Some(ledger)=guard.ledger{
        crate::billing_writer::settle_diagnostic(state,ledger,None,None,None,guard.request_based,if guard.sent{"incomplete_unknown"}else{"not_sent"})?;
        result.billing=Some(billing_summary(state,ledger)?);
    }
    result.latency_ms=started.elapsed().as_millis()as u64;result.at_ms=db::now_ms();
    finish(state,&probe,&token,&result)?;guard.done=true;Ok(result)
}
pub async fn tick(state:&SharedState,now:i64)->usize {
    let due={let c=state.conn();list(&c).unwrap_or_default().into_iter().filter(|p|p.config.enabled&&p.next_run_at_ms<=now).map(|p|p.id).collect::<Vec<_>>()};
    let mut pending=due.into_iter();let mut jobs=tokio::task::JoinSet::new();
    for _ in 0..4 {if let Some(id)=pending.next(){let state=state.clone();jobs.spawn(async move{execute(&state,&id,true,now).await.is_ok()});}}
    let mut ran=0;while let Some(done)=jobs.join_next().await {if done.unwrap_or(false){ran+=1;}if let Some(id)=pending.next(){let state=state.clone();jobs.spawn(async move{execute(&state,&id,true,now).await.is_ok()});}}ran
}
pub fn spawn(state:SharedState)->tokio::task::JoinHandle<()> {
    tokio::spawn(async move {let mut timer=tokio::time::interval(Duration::from_secs(1));timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);loop{timer.tick().await;tick(&state,db::now_ms()).await;}})
}
