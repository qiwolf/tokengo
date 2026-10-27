//! Explicit management-only compatibility checks. No save hook invokes these routes.
use axum::{extract::{Path,Query,State},routing::{get,post},Json,Router};
use serde::Deserialize;
use serde_json::{json,Value};
use crate::{db,protocol::Protocol,provider_diagnostics as diagnostics,SharedState};
use super::{ApiError,operations::ManagementAuth};
#[derive(Deserialize)]
pub struct Selection {model:String}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verify {model:String,native:Option<Protocol>,mode:Option<String>,expected_identity:Option<diagnostics::Identity>}
pub fn router()->Router<SharedState> {
 Router::new().route("/channels/{id}/models/compatibility",get(describe))
 .route("/channels/{id}/models/compatibility/verify",post(verify))
 .route("/channels/{id}/models/compatibility/jobs/{job}",get(job).delete(cancel))
}
async fn describe(_auth:ManagementAuth,State(state):State<SharedState>,Path(id):Path<i64>,Query(q):Query<Selection>)->Result<Json<Value>,ApiError> {Ok(Json(diagnostics::snapshot(&state,id,&q.model)?))}
async fn job(_auth:ManagementAuth,State(state):State<SharedState>,Path((id,job)):Path<(i64,String)>)->Result<Json<diagnostics::Job>,ApiError> {Ok(Json(load(&state,id,&job)?))}
fn load(state:&SharedState,id:i64,job:&str)->Result<diagnostics::Job,ApiError> {
 let value={let jobs=state.compatibility_jobs.lock().unwrap_or_else(|e|e.into_inner());jobs.entries.get(job).filter(|e|e.job.identity.channel_id==id).map(|e|e.job.clone()).ok_or_else(||ApiError::not_found("验证任务不存在或已过期"))?};
 Ok(diagnostics::refresh(state,value))
}
async fn cancel(_auth:ManagementAuth,State(state):State<SharedState>,Path((id,job)):Path<(i64,String)>)->Result<Json<diagnostics::Job>,ApiError> {
 load(&state,id,&job)?;
 {let mut jobs=state.compatibility_jobs.lock().unwrap_or_else(|e|e.into_inner());if let Some(e)=jobs.entries.get_mut(&job) {if e.job.state=="running" {if let Some(abort)=e.abort.take(){abort.abort();}e.job.state="canceled".into();e.job.finished_at_ms=Some(db::now_ms());e.job.note="验证已取消；已产生的上游用量无法撤销".into();}}}
 Ok(Json(load(&state,id,&job)?))
}
async fn verify(_auth:ManagementAuth,State(state):State<SharedState>,Path(id):Path<i64>,Json(body):Json<Verify>)->Result<Json<diagnostics::Job>,ApiError> {
 let identity=diagnostics::identity(&state,id,&body.model)?;
 if body.expected_identity.as_ref().is_some_and(|old|old!=&identity) {return Err(ApiError::new(axum::http::StatusCode::CONFLICT,"source_changed","来源已变化，请刷新诊断后重试"));}
 let native=body.native.unwrap_or(identity.native[0]);
 if !identity.native.contains(&native) {return Err(ApiError::bad_request("所选协议未在此来源声明为原生协议"));}
 let mode=body.mode.unwrap_or_else(||"inference".into());
 if !matches!(mode.as_str(),"inference"|"runtime_metadata") {return Err(ApiError::bad_request("未知验证模式"));}
 let ready=super::admin::ready_channel(&state,tokengo_shared::ChannelId(id))?.ok_or_else(||ApiError::not_found("接入不存在"))?;
 let (profile,caps)={let c=state.conn();let caps=crate::protocol_config::load(&c,id)?;
   let row=db::channels::get(&c,tokengo_shared::ChannelId(id))?.ok_or_else(||ApiError::not_found("接入不存在"))?;
   let credential=db::provider_pool::source_credential(&c,id)?;
   if !row.enabled||!row.provider_enabled||credential.as_ref().is_some_and(|v|!v.enabled)||!row.models.iter().any(|m|m.model==body.model&&m.enabled)||!db::model_routes::enabled(&c,&body.model)? {return Err(ApiError::bad_request("渠道、凭据或模型已停用，未发送验证"));}
   if mode=="inference"&&db::kinds::get(&c,&body.model)?.is_some_and(|p|!matches!(p.kind,tokengo_shared::ModelKind::Chat|tokengo_shared::ModelKind::Unknown)) {return Err(ApiError::bad_request("此入口只验证对话原生协议；其他用途使用对应诊断入口"));}
   (caps.parameter_profile.clone(),caps)};
 if diagnostics::identity(&state,id,&body.model)?!=identity {return Err(ApiError::new(axum::http::StatusCode::CONFLICT,"source_changed","来源已变化，请刷新后重试"));}
 if mode=="runtime_metadata"&&!matches!(profile.as_deref(),Some("ollama"|"vllm")) {return Err(ApiError::bad_request("标准运行时元数据采集仅适用于显式 Ollama/vLLM 来源"));}
 let mut request=match native {
  Protocol::Chat=>json!({"model":identity.upstream_model,"messages":[{"role":"user","content":"ping"}],"stream":false}),
  Protocol::Responses=>json!({"model":identity.upstream_model,"input":"ping","stream":false,"store":false}),
  Protocol::Messages=>json!({"model":identity.upstream_model,"messages":[{"role":"user","content":"ping"}],"stream":false}),
 };
 let mut reservation_estimate=None;
 if mode=="inference" {
  let ctx=crate::parameter_contracts::Context{base_url:&ready.channel.base_url,model:&identity.upstream_model,protocol:native,api_version:identity.api_version.as_deref().or(if native==Protocol::Messages{Some("2023-06-01")}else{None}),is_oauth:false,profile_override:profile.as_deref()};
  let cap_key=crate::parameter_budgets::default_output_key(&ctx);
  // This is an API request-shape minimum, not model-capability evidence.
  // Unknown deployed models must receive the same legal Responses shape.
  let minimum=if crate::parameter_contracts::provider(&ctx)=="dashscope"&&native==Protocol::Responses{16}else{1};
  request[cap_key]=json!(minimum);
  crate::parameter_contracts_china::adapt(&ctx,&mut request).map_err(|_|ApiError::bad_request("来源合同不允许此最小请求，请使用对应原生诊断"))?;
  crate::parameter_contracts::validate(&ctx,&request).and_then(|_|crate::parameter_contracts_china::validate(&ctx,&request)).map_err(|_|ApiError::bad_request("来源合同不允许此最小请求，请使用对应原生诊断"))?;
  reservation_estimate=Some(crate::parameter_budgets::reservation_estimate(&ctx,&request,caps.max_output_tokens,None,1)
      .map_err(|_|ApiError::bad_request("最小请求的输出参数类型、协议或预算组合不合法"))?);
 }
 let job=diagnostics::insert(&state,identity.clone(),native,mode.clone(),reservation_estimate)?;let jid=job.id.clone();let worker_state=state.clone();
 let deadline_at_ms=job.deadline_at_ms;
 let worker=tokio::spawn(async move {
  let timeout=std::time::Duration::from_millis(deadline_at_ms.saturating_sub(db::now_ms()).max(0) as u64);
  let work=async {
   if !diagnostics::identity(&worker_state,identity.channel_id,&identity.model).is_ok_and(|next|next==identity) {diagnostics::finish(&worker_state,&jid,"stale",None,None,"来源变化，验证未发送");return;}
   if mode=="runtime_metadata" {
    let result=collect_runtime(&worker_state,&ready,profile.as_deref().unwrap_or(""),&identity.upstream_model,timeout).await;
    match result {Ok(candidate)=>diagnostics::finish(&worker_state,&jid,"passed",None,Some(candidate),"已从当前配置来源取得标准 API 候选摘要；软件身份与能力仍未验证，未保存配置"),Err(_)=>diagnostics::finish(&worker_state,&jid,"failed",None,None,"标准 API 元数据采集失败或格式无效；未保存配置")}
   } else {
    let result=worker_state.upstream.probe_report(&ready.channel,ready.secret.as_deref(),native.path(),bytes::Bytes::from(request.to_string()),timeout,&crate::upstream::new_uuid(),db::now_ms()).await;
    match result {Ok((code,_))=>diagnostics::finish(&worker_state,&jid,if (200..300).contains(&code){"passed"}else{"failed"},Some(code),None,"仅代表本次模型、凭据与所选原生协议最小请求；未验证工具、推理档位或全部协议；已产生用量无法撤销"),Err(_)=>diagnostics::finish(&worker_state,&jid,"failed",None,None,"本次最小请求失败；未推断整个账号不可用，详情请核对来源和权限")}
   }
  };
  if tokio::time::timeout(timeout,work).await.is_err() {diagnostics::finish(&worker_state,&jid,"failed",None,None,"验证达到15秒总时限；已产生用量无法撤销");}
 });
 {let mut jobs=state.compatibility_jobs.lock().unwrap_or_else(|e|e.into_inner());if let Some(e)=jobs.entries.get_mut(&job.id) {if e.job.state=="running" {e.abort=Some(worker.abort_handle());}else{worker.abort();}}}
 Ok(Json(job))
}
async fn collect_runtime(state:&SharedState,ready:&super::admin::ReadyChannel,profile:&str,model:&str,timeout:std::time::Duration)->Result<Value,()> {
 let call=|path:&'static str,body:Option<Value>|state.upstream.runtime_metadata_request(&ready.channel,ready.secret.as_deref(),path,body,timeout);
 let identity=if profile=="ollama" {
  let version=call("/api/version",None).await.map_err(|_|())?;
  let show=call("/api/show",Some(json!({"model":model}))).await.map_err(|_|())?;
  crate::runtime_identity::ollama_standard_api_claim(model,&version,&show).map_err(|_|())?
 }else {
  let version=call("/version",None).await.map_err(|_|())?;let models=call("/v1/models",None).await.map_err(|_|())?;
  crate::runtime_identity::vllm_standard_api_claim(&version,&models).map_err(|_|())?
 };
 Ok(identity.describe(model))
}
