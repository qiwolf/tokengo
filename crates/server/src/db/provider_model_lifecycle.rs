//! Explicit credential-scoped model removal. Mutable mappings are not request/financial history.
//! Call inside the credential PATCH transaction, validate after all model/credential edits.
use super::{channels, now_ms, provider_pool, user_virtual_routes as vr, DbError, Result};
use rusqlite::{params, Connection};
use serde_json::json;
use tokengo_shared::{same_model_name, ChannelId};

fn bad(message: impl Into<String>) -> DbError { DbError::Other(message.into()) }
struct Dependency {
    owner: Option<i64>,
    label: String,
    target: vr::Target,
}
impl Dependency {
    fn usable(&self,c:&Connection)->Result<bool> {
        let auth=match vr::owner_auth(c,self.owner.unwrap_or(0)) {
            Ok(auth)=>auth,
            Err(e) if e.code=="virtual_route_forbidden"=>return Ok(false),
            Err(e)=>return Err(bad(e.message)),
        };
        let sources=if self.owner.is_some(){vr::allowed_sources(c,&auth,&self.target)}else{vr::allowed_sources_shared(c,&auth,&self.target)};
        Ok(!sources.map_err(|e|bad(e.message))?.is_empty())
    }
}
pub struct RemovalGuard {
    provider: i64,
    credential: i64,
    names: Vec<String>,
    previously_usable: Vec<Dependency>,
    sources: Vec<i64>,
}
impl RemovalGuard {
    pub fn apply(c:&Connection,provider:i64,credential:i64,source_ids:&[i64],names:&[String])->Result<Self> {
        let mut guard=Self{provider,credential,names:names.to_vec(),previously_usable:Vec::new(),sources:source_ids.to_vec()};
        if names.is_empty(){return Ok(guard);}
        let mut dependencies=Vec::new();
        for shared in super::shared_virtual_routes::list(c)? {
            for target in shared.config.targets {
                if target.provider_id==provider&&names.iter().any(|name|same_model_name(name,&target.logical_model)) {
                    dependencies.push(Dependency{owner:None,label:format!("共享虚拟模型“{}”",shared.config.name),target});
                }
            }
        }
        let private=c.prepare("SELECT DISTINCT v.user_id,v.id,v.name,t.logical_model FROM user_virtual_models v JOIN user_virtual_targets t ON t.virtual_model_id=v.id WHERE t.provider_id=?1 ORDER BY v.id")?.query_map([provider],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for (owner,id,name,model) in private {
            if names.iter().any(|name|same_model_name(name,&model))&&vr::deleted_at(c,owner,id)?.is_none(){
                dependencies.push(Dependency{owner:Some(owner),label:format!("私有虚拟模型“{name}”"),target:vr::Target{provider_id:provider,logical_model:model,switch_on_429:true,switch_on_400:false}});
            }
        }
        for dependency in dependencies {if dependency.usable(c)?{guard.previously_usable.push(dependency);}}
        for source in source_ids {
            let owner=provider_pool::source_credential(c,*source)?.ok_or_else(||bad("模型接入的Key归属已变化，请刷新后重试"))?;
            if owner.id!=credential||owner.provider_id!=provider{return Err(bad("模型接入不属于当前Key，拒绝修改其他Key配置"));}
            let source=channels::get(c,ChannelId(*source))?.ok_or_else(||bad("模型接入不存在，请刷新后重试"))?;
            let kept=source.models.iter().filter(|m|!names.iter().any(|name|same_model_name(name,&m.model))).cloned().collect::<Vec<_>>();
            if kept.len()!=source.models.len(){channels::set_models(c,source.id,&kept)?;}
        }
        Ok(guard)
    }
    /// Keep references intact; removal is never an implicit edit of an alias or its owner's grants.
    pub fn validate_and_audit(self,c:&Connection,operations_token:Option<i64>)->Result<()> {
        if self.names.is_empty(){return Ok(());}
        let mut broken=Vec::new();
        for dependency in &self.previously_usable {
            if !dependency.usable(c)?&&!broken.contains(&dependency.label){broken.push(dependency.label.clone());}
        }
        if !broken.is_empty(){return Err(bad(format!("credential_model_in_use: 移除后将失去最后已授权可用目标，仍被{}引用；请先调整这些虚拟模型的调用目标。",broken.join("、"))));}
        c.execute("INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json) VALUES(?1,'provider_credential.models.remove','credential',?2,?3)",params![now_ms(),self.credential,json!({"provider_id":self.provider,"source_ids":self.sources,"removed_models":self.names,"history_preserved":true,"actor_kind":if operations_token.is_some(){"operations_token"}else{"panel_admin"},"operations_token_id":operations_token}).to_string()])?;
        Ok(())
    }
}
