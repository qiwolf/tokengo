//! Browser-owner request history. Administrative APIs retain their existing root gate.
use super::{accounts::{self, Browser},ApiError};
use crate::{db::{self,account_overview},SharedState};
use axum::{extract::{FromRequestParts,Path,Query,State},http::{request::Parts,StatusCode},routing::get,Json,Router};
use serde::Deserialize;
use serde_json::json;
struct UserGrant {browser:Browser,owner:i64}
impl UserGrant {
    fn owner(&self,c:&rusqlite::Connection)->Result<i64,ApiError> {
        if !self.browser.valid(c)? || self.browser.owner(c)?!=Some(self.owner)
            || self.browser.credential.account.user_id!=Some(self.owner) {
            return Err(ApiError::unauthorized("登录已失效"));
        }
        if !db::identity::user(c,self.owner)?.is_some_and(|u|u.enabled) {return Err(ApiError::unauthorized("用户不可用"))}
        Ok(self.owner)
    }
}
impl FromRequestParts<SharedState> for UserGrant {
    type Rejection=ApiError;
    async fn from_request_parts(parts:&mut Parts,state:&SharedState)->Result<Self,ApiError> {
        let browser=Browser::from_request_parts(parts,state).await?;
        let owner=browser.credential.account.user_id.ok_or_else(||ApiError::new(StatusCode::FORBIDDEN,"user_required","此接口仅供普通用户使用"))?;
        let grant=Self{browser,owner};
        grant.owner(&state.conn())?;
        Ok(grant)
    }
}
pub fn router()->Router<SharedState> {
    Router::new().route("/api/account/overview/stats",get(stats))
        .route("/api/account/overview/logs",get(logs))
        .route("/api/account/overview/requests/{id}",get(detail))
        .layer(axum::middleware::map_response(accounts::json_errors))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatsQuery {hours:Option<i64>}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogsQuery {limit:Option<u32>,before_id:Option<i64>}
async fn stats(grant:UserGrant,State(state):State<SharedState>,Query(q):Query<StatsQuery>)->Result<Json<serde_json::Value>,ApiError> {
    let c=state.conn();let owner=grant.owner(&c)?;
    let hours=q.hours.unwrap_or(24).clamp(1,2160);
    let stats=account_overview::stats_since(&c,owner,db::now_ms()-hours*3600000).map_err(|_|ApiError::internal("调用统计暂不可用"))?;
    Ok(Json(json!({"window_hours":hours,"stats":stats})))
}
async fn logs(grant:UserGrant,State(state):State<SharedState>,Query(q):Query<LogsQuery>)->Result<Json<account_overview::RequestPage>,ApiError> {
    let c=state.conn();let owner=grant.owner(&c)?;
    if q.before_id.is_some_and(|id|id<=0) {return Err(ApiError::bad_request("分页位置无效"))}
    Ok(Json(account_overview::recent(&c,owner,q.limit.unwrap_or(100).clamp(1,1000),q.before_id).map_err(|_|ApiError::internal("调用记录暂不可用"))?))
}
async fn detail(grant:UserGrant,State(state):State<SharedState>,Path(id):Path<i64>)->Result<Json<account_overview::RequestDetail>,ApiError> {
    let c=state.conn();let owner=grant.owner(&c)?;
    let detail=account_overview::detail(&c,owner,id).map_err(|_|ApiError::internal("调用详情暂不可用"))?.ok_or_else(||ApiError::not_found("请求不存在或已按保留期限清理"))?;
    Ok(Json(detail))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn user_grant_freezes_owner_and_rechecks_credential_role_and_version_before_read() {
        let dir=std::env::temp_dir().join(format!("tokengo-user-grant-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=crate::AppState::init(crate::Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into_owned(),..crate::Config::default()}).unwrap();
        let (owner,other,account,session)={
            let c=state.conn();let owner=db::identity::create_user(&c,"Grant owner","user",&Default::default()).unwrap().id;
            let other=db::identity::create_user(&c,"Other owner","user",&Default::default()).unwrap().id;
            let account=db::accounts::create(&c,"grant-test","Grant owner",owner,"test-only-stored-hash").unwrap().id;
            let session=db::accounts::create_session(&c,&db::accounts::by_id(&c,account).unwrap().unwrap()).unwrap();(owner,other,account,session)
        };
        let (mut parts,_)=axum::http::Request::builder().uri("/api/account/overview/logs").header("cookie",format!("tokengo_account_session={session}")).body(()).unwrap().into_parts();
        let grant=UserGrant::from_request_parts(&mut parts,&state).await.unwrap();let c=state.conn();assert_eq!(grant.owner(&c).unwrap(),owner);
        c.execute("UPDATE panel_accounts SET user_id=?2 WHERE id=?1",rusqlite::params![account,other]).unwrap();assert!(grant.owner(&c).is_err());
        c.execute("UPDATE panel_accounts SET user_id=?2 WHERE id=?1",rusqlite::params![account,owner]).unwrap();assert_eq!(grant.owner(&c).unwrap(),owner);
        c.execute("UPDATE panel_accounts SET credential_version=credential_version+1 WHERE id=?1",[account]).unwrap();assert!(grant.owner(&c).is_err());
        c.execute("UPDATE panel_accounts SET credential_version=credential_version-1,password_hash='changed-hash' WHERE id=?1",[account]).unwrap();assert!(grant.owner(&c).is_err());
        c.execute("UPDATE panel_accounts SET password_hash='test-only-stored-hash',role='root',user_id=NULL WHERE id=?1",[account]).unwrap();assert!(grant.owner(&c).is_err());
    }
}
