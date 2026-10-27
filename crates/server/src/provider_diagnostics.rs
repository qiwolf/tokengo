//! Versioned, source-bound diagnostics. Descriptions perform no network I/O;
//! explicit jobs retain only bounded outcomes and never confer capabilities.
use crate::{db, parameter_contracts::Context, protocol::Protocol, routes::ApiError, SharedState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::task::AbortHandle;
use sha2::{Digest,Sha256};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEADLINE_MS: i64 = 15_000;
const MAX_JOBS: usize = 128;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub channel_id: i64,
    pub model: String,
    pub upstream_model: String,
    pub source_fingerprint: String,
    pub binding_generation: i64,
    pub native: Vec<Protocol>,
    pub authentication_line: String,
    pub api_version: Option<String>,
    pub contract_revision: String,
    pub credential_revision: Option<i64>,
    pub provider_version: Option<i64>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub schema_version: u32,
    pub id: String,
    pub identity: Identity,
    pub native: Protocol,
    pub mode: String,
    pub state: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub deadline_at_ms: i64,
    pub http_status: Option<u16>,
    pub note: String,
    pub candidate: Option<Value>,
    pub reservation_estimate: Option<crate::parameter_budgets::ReservationEstimate>,
}
pub struct Entry { pub job: Job, pub abort: Option<AbortHandle> }
#[derive(Default)]
pub struct Registry { pub entries: HashMap<String,Entry> }
impl Drop for Registry { fn drop(&mut self) { for entry in self.entries.values() { if let Some(abort)=&entry.abort {abort.abort();} } } }

pub fn safe_api_version(value:&str)->Option<String> {
    (value.len()==10 && value.bytes().enumerate().all(|(i,b)|if i==4||i==7{b==b'-'}else{b.is_ascii_digit()})).then(||value.into())
}
pub fn identity(state:&SharedState,id:i64,model:&str)->Result<Identity,ApiError> {
    if model.is_empty()||model.len()>1024||model.chars().any(char::is_control) { return Err(ApiError::bad_request("请选择有效模型")); }
    let c=state.conn();
    let row=db::channels::get(&c,tokengo_shared::ChannelId(id))?.ok_or_else(||ApiError::not_found("上游接入不存在"))?;
    let mapping=row.models.iter().find(|m|m.model==model).ok_or_else(||ApiError::bad_request("模型未配置在所选接入"))?;
    let caps=crate::protocol_config::load(&c,id)?;
    let credential=db::provider_pool::source_credential(&c,id)?;
    let oauth:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings WHERE channel_id=?1)",[id],|r|r.get(0)).map_err(db::DbError::from)?;
    let api_version=db::channels::headers(&c,&state.secrets,row.id)?.iter().find(|(name,_)|name.eq_ignore_ascii_case("anthropic-version")).and_then(|(_,value)|safe_api_version(value));
    let authentication_line=if oauth {
        if crate::oauth_external::channel_managed_openai(&c,id).map_err(|_|ApiError::internal("无法读取认证来源类型"))? {"managed_openai_oauth"}else{"external_oauth"}
    }else{"externally_configured_credentials"};
    // The public digest also binds effective mapping and availability controls;
    // a disabled/rewritten model must not retain a prior availability result.
    let raw=crate::reasoning_capabilities::fingerprint(&c,id)?;
    let binding=serde_json::to_vec(&(&raw,row.enabled,row.provider_enabled,row.unwrap_envelope,(&mapping.model,&mapping.upstream,mapping.enabled,mapping.vision,&mapping.drop_fields),db::model_routes::enabled(&c,model)?,db::kinds::get(&c,model)?.map(|k|k.kind.as_str().to_string()),credential.as_ref().map(|v|(v.id,v.provider_id,v.enabled,v.revision,v.provider_version))))
        .map_err(|_|ApiError::internal("无法确认诊断来源版本"))?;
    let source_fingerprint=hex::encode(Sha256::digest(binding));
    Ok(Identity { channel_id:id,model:model.into(),upstream_model:mapping.upstream.as_deref().filter(|s|!s.is_empty()).unwrap_or(model).into(),
        source_fingerprint,
        binding_generation:c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get(0)).map_err(db::DbError::from)?,
        native:caps.native,authentication_line:authentication_line.into(),api_version,
        contract_revision:crate::contract_evidence::REVISION.into(),credential_revision:credential.as_ref().map(|c|c.revision),provider_version:credential.map(|c|c.provider_version) })
}
pub fn current(state:&SharedState,job:&Job)->bool { identity(state,job.identity.channel_id,&job.identity.model).is_ok_and(|i|i==job.identity) }
pub fn refresh(state:&SharedState,mut job:Job)->Job {
    if !current(state,&job) {
        job.state="stale".into();job.note="来源配置或凭据已变化；旧结果不可用于当前来源".into();job.candidate=None;job.reservation_estimate=None;
        let mut jobs=state.compatibility_jobs.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(entry)=jobs.entries.get_mut(&job.id) {if let Some(abort)=entry.abort.take(){abort.abort();}entry.job=job.clone();}
    }
    job
}
/// Use fixed field-to-rule references, intersected with the exact applicable
/// registry rules. Human-readable registry field descriptions are not a parser
/// contract. A reference supplies provenance without promoting support status.
fn field_evidence(mut row:Value,applicable:&Value)->Value {
    let candidates:&[&str]=match row["field"].as_str().unwrap_or("") {
        "max_output_tokens"=>&["G01"],
        "max_tokens"=>&["G14"],
        "reasoning_effort"=>&["G11"],
        "thinking"=>&["G07-chat","G14","G15-chat","G15-messages"],
        "tools"=>&["G04","G07-chat"],
        "response_format"=>&["G13"],
        "explicit_reasoning_off"=>&["G07-chat","G08","G11","G15-chat","G15-messages"],
        _=>&[],
    };
    let ids:Vec<_>=candidates.iter().copied().filter(|candidate|applicable.as_array()
        .is_some_and(|items|items.iter().any(|id|id.as_str()==Some(*candidate)))).collect();
    row["rule_match"]=json!(if ids.is_empty(){"none"}else{"matched"});
    row["rule_ids"]=json!(ids);row["rule_source"]=json!("contract_evidence");row
}

fn verification_lane(identity:&Identity,tools:&crate::search_tools::ToolCapabilities,model_kind:&str)->Value {
    let oauth=identity.authentication_line!="externally_configured_credentials";
    let (kind,recommended,note)=if tools.search||tools.fetch {
        ("standalone_tool",if tools.search{"search".to_string()}else{"fetch".to_string()},"此来源使用独立工具请求。打开探针页核对查询词或网页地址，再运行已有工具诊断；用量按系统诊断记录。")
    }else if oauth {
        let protocol=if identity.authentication_line=="managed_openai_oauth" {Protocol::Responses}else{identity.native.first().copied().unwrap_or(Protocol::Chat)};
        ("authenticated_gateway",serde_json::to_value(protocol).ok().and_then(|v|v.as_str().map(str::to_owned)).unwrap_or_else(||"chat".into()),"此来源通过已绑定账号鉴权。打开探针页使用现有授权刷新、原生协议与系统诊断计量链路；不会把 OAuth 当 API Key 发送。")
    }else if model_kind=="embeddings" {
        ("specialized_gateway","embeddings".to_string(),"此模型用途为向量。打开探针页使用现有 Embeddings 输入与维度检查，不会发送对话 ping；用量按系统诊断记录。")
    }else if model_kind=="rerank" {
        ("specialized_gateway","rerank".to_string(),"此模型用途为重排。打开探针页填写查询与文档，执行原生 top-1 协议与结构检查；不证明排序质量，不会发送对话 ping。")
    }else {
        ("direct_native",serde_json::to_value(identity.native.first().copied().unwrap_or(Protocol::Chat)).ok().and_then(|v|v.as_str().map(str::to_owned)).unwrap_or_else(||"chat".into()),"可在此显式验证所选原生协议的最小请求；未知证据不会关闭功能，具体请求仍须满足权限、协议和预算要求。")
    };
    json!({"kind":kind,"management_page":"probes","management_hash":"#probes","source_id":identity.channel_id,"model":identity.model,
        "recommended_kind":recommended,"direct_native_inference":kind=="direct_native","note":note,"access":"root_session"})
}

pub fn snapshot(state:&SharedState,id:i64,model:&str)->Result<Value,ApiError> {
    let identity=identity(state,id,model)?;
    let (purpose,reasoning,caps,contracts,fields,account,budget_semantics,lane)= {
        let c=state.conn();let caps=crate::protocol_config::load(&c,id)?;
        let row=db::channels::get(&c,tokengo_shared::ChannelId(id))?.ok_or_else(||ApiError::not_found("接入不存在"))?;
        let mut purpose=db::kinds::get(&c,model)?.map(|p|json!({"kind":p.kind.as_str(),"source":p.source.as_str()})).unwrap_or_else(||json!({"kind":"unknown","source":"unknown"}));
        // Catalog purpose is operator presentation metadata, independent of
        // executable kind, dispatch capabilities and source/job identity.
        let configured=db::model_access::catalog_purpose(&c,model)?;
        let label=match configured.as_str() {""=>"未配置","chat"=>"对话与推理",other=>db::model_access::catalog_purpose_label(other)};
        purpose["configured"]=json!(configured);purpose["catalog_purpose"]=json!(configured);
        purpose["label"]=json!(label);purpose["configured_source"]=json!(if configured.is_empty(){"not_configured"}else{"operator_configured"});
        let reasoning=crate::reasoning_capabilities::for_model(&c,id,&identity.upstream_model)?;
        let mut contracts=Vec::new();let mut fields=Vec::new();let mut budget_semantics=Vec::new();
        for native in &caps.native {
            let ctx=Context{base_url:&row.base_url,model:&identity.upstream_model,protocol:*native,api_version:identity.api_version.as_deref().or(if *native==Protocol::Messages{Some("2023-06-01")}else{None}),is_oauth:identity.authentication_line!="externally_configured_credentials",profile_override:caps.parameter_profile.as_deref()};
            let parameters=crate::parameter_contracts::describe(&ctx);
            let provider=crate::parameter_contracts_china::describe(&ctx);
            let applicable=parameters["contract_evidence"]["rules"].clone();
            let documented=parameters["model_rules"]=="exact_documented_model"||provider["known_model"]==true;
            let semantics=if !documented {"Unknown"}else{match crate::parameter_contracts_china::output_semantics(&ctx,&json!({})) {
                crate::parameter_contracts_china::OutputSemantics::Total=>"Total",
                crate::parameter_contracts_china::OutputSemantics::AnswerOnly{..}=>"AnswerOnly",
                crate::parameter_contracts_china::OutputSemantics::Unknown=>"Unknown",
            }};
            budget_semantics.push(json!({"protocol":native,"semantics":semantics,"scope":"documented_cap_without_explicit_request_controls","request_bound_known":false}));
            fields.push(field_evidence(json!({"field":crate::parameter_budgets::default_output_key(&ctx),"protocol":native,"status":if documented&&semantics=="Total"{"supported_native"}else{"unknown"},"scope":"documented_native_output_cap_only","conditions":["Exact source/model/protocol; explicit thinking and other controls may change budget semantics; no live capability grant"],"rule_ids":[]}),&applicable));
            contracts.push(json!({"protocol":native,"parameters":parameters,"provider":provider}));
            // Unknown native passthrough is not positive proof. Preserve separate
            // execution gates; a documentation description does not unlock them.
            for field in ["reasoning_effort","thinking","tools","response_format"] {
                fields.push(field_evidence(json!({"field":field,"protocol":native,"status":"unknown","scope":"exact_model_and_native_request","conditions":["Inspect source contract and exact request; native preservation alone does not prove support"],"rule_ids":[]}),&applicable));
            }
            let mut off=match native {Protocol::Chat=>json!({"reasoning_effort":"none"}),Protocol::Responses=>json!({"reasoning":{"effort":"none"}}),Protocol::Messages=>json!({"output_config":{"effort":"none"}})};
            if crate::parameter_contracts_china::adapt(&ctx,&mut off).is_ok_and(|changed|changed) {
                fields.push(field_evidence(json!({"field":"explicit_reasoning_off","protocol":native,"status":"exact_transform","scope":"documented_explicit_off_adapter_only","conditions":["Only the literal none control; no claim for other reasoning levels or live availability"],"rule_ids":[]}),&applicable));
            }
            // MiMo's explicit Responses effort values intentionally collapse to
            // the same thinking-on behavior; this does not promise distinctions.
            if crate::parameter_contracts::provider(&ctx)=="mimo" && *native==Protocol::Responses && matches!(ctx.model,"mimo-v2.6-flash"|"mimo-v2.6-pro"|"mimo-v2.6-pro-ultraspeed") {
                fields.push(field_evidence(json!({"field":"reasoning.effort","protocol":native,"status":"documented_semantic_fold","scope":"source_contract_only","conditions":["Valid non-none efforts all enable the same thinking behavior; none disables"],"rule_ids":[]}),&applicable));
            }
            fields.push(json!({"field":"background","protocol":native,"status":"executor_gated","scope":"gateway_executor","conditions":["No background executor capability is granted by a source profile"],"rule_ids":["stateful_executor"],"rule_match":"matched","rule_source":"gateway_executor"}));
        }
        let cred=db::provider_pool::source_credential(&c,id)?;
        let local=row.enabled&&row.provider_enabled&&cred.as_ref().is_none_or(|v|v.enabled)&&row.models.iter().any(|m|m.model==model&&m.enabled)&&db::model_routes::enabled(&c,model)?;
        let account=if identity.authentication_line!="externally_configured_credentials" {c.query_row("SELECT a.status FROM oauth_channel_bindings b JOIN oauth_accounts a ON a.id=b.account_id WHERE b.channel_id=?1",[id],|r|r.get::<_,String>(0)).ok().map(|s|if s=="active"{"active"}else{"unavailable"})}else{None};
        {
            let lane=verification_lane(&identity,&crate::search_tools::load(&c,id)?,purpose["kind"].as_str().unwrap_or("unknown"));
            (purpose,reasoning,caps,contracts,fields,json!({"local_enabled":local,"account_state":account,"remote_access":"not_inferred"}),budget_semantics,lane)
        }
    };
    let latest={let jobs=state.compatibility_jobs.lock().unwrap_or_else(|e|e.into_inner());jobs.entries.values().filter(|e|e.job.identity.channel_id==id&&e.job.identity.model==model&&e.job.mode=="inference").max_by_key(|e|e.job.started_at_ms).map(|e|e.job.clone())};
    let verification=latest.map(|j|refresh(state,j));
    let availability=verification.as_ref().map(|j|json!({"state":j.state,"job_id":j.id,"http_status":j.http_status,"note":j.note})).unwrap_or_else(||json!({"state":"never"}));
    Ok(json!({"schema_version":SCHEMA_VERSION,"contract_revision":crate::contract_evidence::REVISION,"identity":identity,"purpose":purpose,"reasoning":reasoning,
        "budget":{"max_input_tokens":caps.max_input_tokens,"max_output_tokens":caps.max_output_tokens,"configured":caps.context_bounds().is_some(),"known":false,"semantics":"Unknown","contracts":budget_semantics,"scope":"configured_source_bounds_not_a_request_or_live_measurement","reservation_policy":"contract_or_local_estimate","supplier_hard_cap_verified":false},
        "contracts":contracts,"runtime_identity":caps.runtime_description(&identity.upstream_model),"fields":fields,
        "evidence":{"documentation":"contract_metadata","sample":"not_asserted","live":"single_native_minimal_request_only"},"account_availability":account,"verification_lane":lane,"availability":availability,"verification":verification}))
}
pub fn insert(state:&SharedState,identity:Identity,native:Protocol,mode:String,reservation_estimate:Option<crate::parameter_budgets::ReservationEstimate>)->Result<Job,ApiError> {
    let mut jobs=state.compatibility_jobs.lock().unwrap_or_else(|e|e.into_inner());
    if jobs.entries.values().any(|e|e.job.identity==identity&&e.job.native==native&&e.job.mode==mode&&e.job.state=="running") {return Err(ApiError::new(axum::http::StatusCode::CONFLICT,"verification_running","此来源已有相同验证正在运行"));}
    while jobs.entries.len()>=MAX_JOBS {
        let oldest=jobs.entries.iter().filter(|(_,e)|e.job.state!="running").min_by_key(|(_,e)|e.job.started_at_ms).map(|(id,_)|id.clone());
        if let Some(id)=oldest {jobs.entries.remove(&id);}else{return Err(ApiError::new(axum::http::StatusCode::TOO_MANY_REQUESTS,"verification_capacity","验证任务已满，请稍后再试"));}
    }
    let now=db::now_ms();let job=Job{schema_version:SCHEMA_VERSION,id:uuid::Uuid::new_v4().to_string(),identity,native,mode,state:"running".into(),started_at_ms:now,finished_at_ms:None,deadline_at_ms:now+DEADLINE_MS,http_status:None,note:"仅验证所选原生协议的最小请求；取消无法撤销已经产生的用量".into(),candidate:None,reservation_estimate};
    jobs.entries.insert(job.id.clone(),Entry{job:job.clone(),abort:None});Ok(job)
}
pub fn finish(state:&SharedState,id:&str,status:&str,code:Option<u16>,candidate:Option<Value>,note:&str) {
    let old={let jobs=state.compatibility_jobs.lock().unwrap_or_else(|e|e.into_inner());jobs.entries.get(id).map(|e|e.job.clone())};
    let Some(old)=old else{return};let same=current(state,&old);
    let mut jobs=state.compatibility_jobs.lock().unwrap_or_else(|e|e.into_inner());
    if let Some(entry)=jobs.entries.get_mut(id) {if entry.job.state!="running" {return;}entry.job.state=if same{status}else{"stale"}.into();entry.job.finished_at_ms=Some(db::now_ms());entry.job.http_status=code;entry.job.candidate=if same{candidate}else{None};if !same {entry.job.reservation_estimate=None;}entry.job.note=if same{note}else{"来源变化，结果失效"}.into();
        if same {if let Some(estimate)=&entry.job.reservation_estimate {entry.job.note.push_str(&format!("；本次输出预算为本地估算 {} tokens，未确认为供应方实测硬上界",estimate.output_tokens));}}
        entry.abort=None;}
}
