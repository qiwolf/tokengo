//! Root-panel proxy configuration; operations tokens cannot change network credentials.
use crate::{egress, routes::{admin::PanelAuth,ApiError},SharedState};
use axum::{extract::{Path,State},routing::{get,put},Json,Router};
use serde_json::Value;
pub fn router()->Router<SharedState>{Router::new().route("/api/network/proxy-policies",get(list)).route("/api/network/proxy-policies/{scope}/{id}",put(save))}
fn error(reason:String)->ApiError{let status=if reason=="egress_version_conflict"{axum::http::StatusCode::CONFLICT}else if reason=="egress_storage_unavailable"{axum::http::StatusCode::SERVICE_UNAVAILABLE}else{axum::http::StatusCode::BAD_REQUEST};ApiError::new(status,"proxy_policy_error",reason)}
async fn list(_root:PanelAuth,State(state):State<SharedState>)->Result<Json<Value>,ApiError>{egress::list(&state.conn()).map(Json).map_err(error)}
async fn save(_root:PanelAuth,State(state):State<SharedState>,Path((scope,id)):Path<(String,String)>,body:Result<Json<egress::Write>,axum::extract::rejection::JsonRejection>)->Result<Json<Value>,ApiError>{
    let Json(body)=body.map_err(|_|ApiError::bad_request("代理配置字段或格式无效；未记录输入值"))?;
    let value={let c=state.conn();egress::save(&c,&state.secrets,&scope,&id,body).map_err(error)?};
    state.egress.invalidate();Ok(Json(value))
}
