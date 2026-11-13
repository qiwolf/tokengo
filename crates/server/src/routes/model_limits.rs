//! Source-authenticated catalog declarations, kept separate from administrator overrides.
use super::{admin::PanelAuth,ApiError};
use crate::{db,SharedState};
use axum::{extract::{Path,State},http::StatusCode,routing::post,Json,Router};
use rusqlite::{Connection,OptionalExtension};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use tokengo_shared::{same_model_name,ChannelId};

pub(super) fn router()->Router<SharedState>{Router::new().route("/api/channels/{id}/limits/discover",post(discover))}
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub(crate) struct Limits {pub max_input_tokens:Option<i64>,pub max_output_tokens:Option<i64>}
#[derive(Clone,Debug,Serialize,Deserialize)]
struct Declaration {channel_id:i64,model:String,upstream_model:String,limits:Limits,source_url:String,discovered_at_ms:i64,status:String,model_matched:bool,note:String}
#[derive(Serialize,Deserialize)]
struct Stored {identity:String,binding_generation:i64,declaration:Declaration}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverInput {model:String}
fn cache_key(channel:i64,model:&str)->String{format!("model_limits_discovery_v1:{channel}:{}",hex::encode(Sha256::digest(tokengo_shared::normalize_model_name(model).as_bytes())))}
fn positive(v:&Value)->Option<i64>{v.as_i64().filter(|n|*n>0)}
fn first_positive(row:&Value,paths:&[&str])->Option<i64>{paths.iter().find_map(|path|row.pointer(path).and_then(positive))}
fn rows(body:&Value)->Vec<(&str,&Value)> {
 let root=body.get("data").or_else(||body.get("models")).unwrap_or(body);
 match root {
  Value::Array(items)=>items.iter().filter_map(|row|row.get("id").or_else(||row.get("name")).and_then(Value::as_str).map(|id|(id,row))).collect(),
  Value::Object(map)=>map.iter().map(|(key,row)|(row.get("id").and_then(Value::as_str).unwrap_or(key),row)).collect(),
  _=>vec![],
 }
}
fn declarations(body:&Value,model:&str)->(bool,Limits){
 let Some((_,row))=rows(body).into_iter().find(|(id,_)|same_model_name(id,model))else{return(false,Limits::default());};
 let input=first_positive(row,&["/max_input_tokens","/input_token_limit","/limit/input","/limits/input","/limit/context","/limits/context","/context_window","/context_length","/max_context_length","/max_model_len","/inputTokenLimit"]);
 let output=first_positive(row,&["/max_output_tokens","/output_token_limit","/max_completion_tokens","/limit/output","/limits/output","/outputTokenLimit","/top_provider/max_completion_tokens"]);
 (true,Limits{max_input_tokens:input,max_output_tokens:output})
}
fn public_url(raw:&str)->String {
 url::Url::parse(raw).ok().map(|mut url|{let _=url.set_username("");let _=url.set_password(None);url.set_query(None);url.set_fragment(None);url.to_string()}).unwrap_or_else(||"source model catalog".into())
}
/// Only current source identity and mapping can supply fallback pricing metadata.
/// These supplier declarations never overwrite configured protocol/OAuth limits.
fn saved_declaration(c:&Connection,channel:i64,model:&str,upstream:&str)->db::Result<Option<Declaration>>{
 let Some(raw)=db::settings::get(c,&cache_key(channel,model))?else{return Ok(None);};
 let stored:Stored=serde_json::from_str(&raw).map_err(|_|db::DbError::Other("Invalid stored model limit discovery".into()))?;
 if stored.identity!=crate::reasoning_capabilities::fingerprint(c,channel)?||!same_model_name(&stored.declaration.upstream_model,upstream){return Ok(None);}
 let generation:Option<i64>=c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[channel],|r|r.get(0)).optional()?;
 if generation!=Some(stored.binding_generation){return Ok(None);}
 Ok(Some(stored.declaration))
}
pub(crate) fn saved_limits(c:&Connection,channel:i64,model:&str,upstream:&str)->db::Result<Option<Limits>>{
 Ok(saved_declaration(c,channel,model,upstream)?.map(|declaration|declaration.limits))
}
/// Safe same-identity provenance for form prefills; never contains transport
/// headers, source secrets, or the internal credential identity fingerprint.
pub(super) fn saved_discovery(c:&Connection,channel:i64,model:&str,upstream:&str)->db::Result<Option<Value>>{
 saved_declaration(c,channel,model,upstream)?.map(serde_json::to_value).transpose().map_err(|_|db::DbError::Other("Discovery metadata serialization failed".into()))
}
/// Effective execution bounds follow explicit OAuth/protocol configuration first.
/// When missing, use the same-source supplier declaration only if *both* limits
/// are positive. These are supplier-declared ceilings, not a measured hard-limit
/// test, and no price snapshot is used to infer an execution bound.
pub(crate) fn effective_context(c:&Connection,channel:i64,model:&str,upstream:&str,configured:Option<(i64,i64)>)->db::Result<Option<(i64,i64)>>{
 if configured.is_some(){return Ok(configured);}
 Ok(saved_limits(c,channel,model,upstream)?.and_then(|limits|limits.max_input_tokens.zip(limits.max_output_tokens)).filter(|(input,output)|*input>0&&*output>0))
}
async fn discover(_auth:PanelAuth,State(state):State<SharedState>,Path(id):Path<i64>,payload:Result<Json<DiscoverInput>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError>{
 let Json(input)=payload.map_err(|_|ApiError::bad_request("Invalid limit discovery body; model is required"))?;
 if input.model.trim().is_empty()||input.model.len()>256{return Err(ApiError::bad_request("Select an enabled model on this source"));}
 {
  let c=state.conn();
  let oauth:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings WHERE channel_id=?1)",[id],|r|r.get(0)).map_err(db::DbError::from)?;
  if oauth{return Err(ApiError::new(StatusCode::BAD_REQUEST,"limits_discovery_unsupported","This OAuth source requires its authenticated OAuth model catalog; API-key catalog discovery is not supported"));}
 }
 let ready=super::admin::ready_channel(&state,ChannelId(id))?.ok_or_else(||ApiError::not_found("Source not found"))?;
 let mapping=ready.models.iter().find(|m|m.enabled&&same_model_name(&m.model,&input.model)).ok_or_else(||ApiError::bad_request("Selected model is not enabled on this source"))?;
 let model=tokengo_shared::normalize_model_name(&mapping.model);let upstream=crate::model_registry::upstream(mapping).to_owned();
 let generation:i64=state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get(0)).map_err(db::DbError::from)?;
 let source_url=public_url(&crate::upstream::model_catalog_url(&ready.channel.base_url));
 let timeout=std::time::Duration::from_millis(state.routing().cfg.attempt_timeout_ms.max(1)).min(std::time::Duration::from_secs(15));
 // ready_channel owns decrypted source headers/secret; no DB guard crosses await.
 let response=state.upstream.fetch_models(&ready.channel,ready.secret.as_deref(),timeout,&crate::upstream::new_uuid(),db::now_ms()).await;
 let (status,body)=response.map_err(|_|ApiError::new(StatusCode::BAD_GATEWAY,"limits_discovery_unavailable","Unable to retrieve this source's model catalog; no configuration was changed"))?;
 if !(200..300).contains(&status){return Err(ApiError::new(StatusCode::BAD_GATEWAY,"limits_discovery_upstream",format!("Source model catalog returned HTTP {status}; no configuration was changed")));}
 let raw:Value=serde_json::from_str(&body).map_err(|_|ApiError::new(StatusCode::BAD_GATEWAY,"limits_discovery_format","Source did not return a JSON model catalog; no configuration was changed"))?;
 let (matched,limits)=declarations(&raw,&upstream);let declared=limits.max_input_tokens.is_some()||limits.max_output_tokens.is_some();
 let declaration=Declaration{channel_id:id,model:model.clone(),upstream_model:upstream.clone(),limits,source_url,discovered_at_ms:db::now_ms(),status:if declared{"supplier_declared"}else{"not_declared"}.into(),model_matched:matched,note:if declared{"Supplier catalog declaration; administrator values remain editable and take precedence. Not a measured hard-limit test."}else{"The selected model's input/output limits were not declared in this source catalog; enter confirmed values manually."}.into()};
 {
  let c=state.conn();let current=db::channels::get(&c,ChannelId(id))?;let current_generation:Option<i64>=c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get(0)).optional().map_err(db::DbError::from)?;
  if current_generation!=Some(generation)||crate::reasoning_capabilities::fingerprint(&c,id)?!=ready.source_identity||!current.as_ref().is_some_and(|row|row.models.iter().any(|m|m.enabled&&same_model_name(&m.model,&model)&&crate::model_registry::upstream(m)==upstream)){
   return Err(ApiError::new(StatusCode::CONFLICT,"source_changed","Source credentials, mapping or protocol changed during discovery; refresh and retry"));
  }
  let stored=Stored{identity:ready.source_identity,binding_generation:generation,declaration:declaration.clone()};
  let encoded=serde_json::to_string(&stored).map_err(|_|ApiError::internal("Discovery serialization failed"))?;
  db::accounts::atomic(&c,||{
   db::settings::set(&c,&cache_key(id,&model),&encoded)?;
   db::identity::audit(&c,None,"source.limits_discovered","channel",id,json!({"model":model,"upstream_model":upstream,"status":declaration.status,"source_url":declaration.source_url,"discovered_at_ms":declaration.discovered_at_ms}))?;
   Ok(())
  })?;
 }
 let mut response=serde_json::to_value(declaration).map_err(|_|ApiError::internal("Discovery serialization failed"))?;response["persisted"]=true.into();Ok(Json(response))
}

#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn limit_declarations_match_models_and_do_not_guess_numbers(){
  let raw=json!({"models":{"target":{"limit":{"context":1000000,"output":16000}},"other":{"context_window":123}}});let(found,limits)=declarations(&raw,"target");assert!(found);assert_eq!(limits.max_input_tokens,Some(1000000));assert_eq!(limits.max_output_tokens,Some(16000));
  let(found,limits)=declarations(&json!({"data":[{"id":"target","context_window":"1000000","max_output_tokens":0,"description":"64k output"}]}),"target");assert!(found);assert_eq!(limits.max_input_tokens,None);assert_eq!(limits.max_output_tokens,None);assert!(!declarations(&raw,"missing").0);
 }
 fn declaration_fixture()->(Connection,i64){
  let c=db::open_in_memory().unwrap();let id=db::channels::create(&c,&db::channels::NewChannel{name:"effective-limits".into(),base_url:"https://fixture.invalid/v1".into(),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap().0;
  db::channels::set_models(&c,ChannelId(id),&[db::channels::ModelMapping::new("logical",Some("upstream".into()),false)]).unwrap();(c,id)
 }
 fn store_declaration(c:&Connection,id:i64,input:Option<i64>,output:Option<i64>){
  let generation=c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get(0)).unwrap();
  let stored=Stored{identity:crate::reasoning_capabilities::fingerprint(c,id).unwrap(),binding_generation:generation,declaration:Declaration{channel_id:id,model:"logical".into(),upstream_model:"upstream".into(),limits:Limits{max_input_tokens:input,max_output_tokens:output},source_url:"https://fixture.invalid/v1/models".into(),discovered_at_ms:db::now_ms(),status:"supplier_declared".into(),model_matched:true,note:"supplier declaration fixture".into()}};
  db::settings::set(c,&cache_key(id,"logical"),&serde_json::to_string(&stored).unwrap()).unwrap();
 }
 #[test]
 fn effective_execution_context_prefers_explicit_values_and_requires_a_current_pair(){
  let(c,id)=declaration_fixture();
  assert_eq!(effective_context(&c,id,"logical","upstream",None).unwrap(),None);
  store_declaration(&c,id,Some(1000000),Some(128000));
  assert_eq!(effective_context(&c,id,"logical","upstream",None).unwrap(),Some((1000000,128000)));
  assert_eq!(effective_context(&c,id,"logical","upstream",Some((500000,16000))).unwrap(),Some((500000,16000)),"explicit configuration has priority even when the declaration is larger");
  assert_eq!(effective_context(&c,id,"logical","another-upstream",None).unwrap(),None,"another upstream model cannot borrow the declaration");
  assert_eq!(effective_context(&c,id,"another-logical","upstream",None).unwrap(),None,"another logical mapping cannot borrow the declaration");
  for (input,output) in [(Some(1000000),None),(None,Some(128000)),(Some(0),Some(128000)),(Some(1000000),Some(-1))]{store_declaration(&c,id,input,output);assert_eq!(effective_context(&c,id,"logical","upstream",None).unwrap(),None,"incomplete or invalid declarations stay unknown");}
  store_declaration(&c,id,Some(1000000),Some(128000));
  c.execute("UPDATE channels SET binding_generation=binding_generation+1 WHERE id=?1",[id]).unwrap();
  assert_eq!(effective_context(&c,id,"logical","upstream",None).unwrap(),None,"changed source binding invalidates the declaration");
  assert_eq!(effective_context(&c,id,"logical","upstream",Some((500000,16000))).unwrap(),Some((500000,16000)));
 }

}
