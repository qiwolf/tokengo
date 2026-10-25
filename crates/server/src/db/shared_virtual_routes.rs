//! Root-managed aliases. Source credentials and user financial ownership stay independent.
use super::{identity, model_routes, user_virtual_routes as vr, DbError};
use crate::vector_compat::Contract;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokengo_shared::{normalize_model_name, same_model_name, ModelKind};
pub const SQL: &str = include_str!("shared_virtual_routes.sql");
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SharedModel {
    pub deleted_at_ms: Option<i64>,
    pub id: i64,
    pub version: i64,
    pub actor_account_id: i64,
    pub kind: ModelKind,
    #[serde(flatten)]
    pub config: vr::Config,
}
fn err(code: &'static str, message: &str) -> vr::VirtualError {
    vr::VirtualError {
        code,
        message: message.into(),
    }
}
pub fn root_actor(c: &Connection, actor: i64) -> Result<(), vr::VirtualError> {
    if actor <= 0
        || !c.query_row(
            "SELECT EXISTS(SELECT 1 FROM panel_accounts WHERE id=?1 AND role='root' AND enabled=1)",
            [actor],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(err("virtual_route_forbidden", "仅有效管理员可管理共享模型"));
    }
    Ok(())
}
pub fn list(c: &Connection) -> super::Result<Vec<SharedModel>> {
    Ok(list_all(c)?.into_iter().filter(|r|r.deleted_at_ms.is_none()).collect())
}
pub fn list_all(c: &Connection) -> super::Result<Vec<SharedModel>> {
    let mut q=c.prepare("SELECT id,version,actor_account_id,config_json,enabled FROM shared_virtual_models ORDER BY id")?;
    let raw = q
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(|row| hydrate(c,row)).collect()
}
type Persisted = (i64,i64,i64,String,bool);
fn hydrate(c:&Connection,(id, version, actor_account_id, raw, enabled):Persisted)->super::Result<SharedModel>{
            let mut config: vr::Config = serde_json::from_str(&raw)
                .map_err(|_| DbError::Other("Invalid persisted shared model".into()))?;
            config.enabled &= enabled;
            let kind = config
                .targets
                .first()
                .map(|t| model_routes::routes_for(c, &t.logical_model))
                .transpose()?
                .flatten()
                .map(|r| r.kind)
                .unwrap_or_default();
            Ok(SharedModel {
                deleted_at_ms: vr::deleted_at(c,0,id)?,
                id,
                version,
                actor_account_id,
                kind,
                config,
            })
}
fn read_one(c:&Connection,sql:&str,value:rusqlite::types::Value)->super::Result<Option<SharedModel>>{
    let row=c.query_row(sql,[value],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    row.map(|row|hydrate(c,row)).transpose()
}
fn by_id(c:&Connection,id:i64)->super::Result<Option<SharedModel>>{
    read_one(c,"SELECT id,version,actor_account_id,config_json,enabled FROM shared_virtual_models WHERE id=?1",id.into())
}
pub fn get(c:&Connection,id:i64)->super::Result<Option<SharedModel>>{
    Ok(by_id(c,id)?.filter(|r|r.deleted_at_ms.is_none()))
}
fn by_name(c:&Connection,name:&str)->super::Result<Option<SharedModel>>{
    matching_name_id(c,name,None)?.map(|id|by_id(c,id)).transpose().map(Option::flatten)
}
// Older imports did not all use the current Rust normalizer. Inspect only names
// for that compatibility path, never hydrate every alias/catalog. Preserve the
// previous first-active-id precedence if a legacy spelling duplicates an index.
fn matching_name_id(c:&Connection,name:&str,except:Option<i64>)->super::Result<Option<i64>>{
    let indexed:Option<(i64,String)>=c.query_row("SELECT id,name FROM shared_virtual_models WHERE normalized_name=?1 AND (?2 IS NULL OR id<>?2)",params![normalize_model_name(name),except],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let indexed=match indexed {Some((id,label))if same_model_name(&label,name)&&vr::deleted_at(c,0,id)?.is_none()=>Some(id),_=>None};
    let mut q=c.prepare("SELECT id,name FROM shared_virtual_models WHERE (?1 IS NULL OR id<=?1) AND (?2 IS NULL OR id<>?2) ORDER BY id")?;
    let mut rows=q.query(params![indexed,except])?;
    while let Some(row)=rows.next()? {
        let id:i64=row.get(0)?;let label:String=row.get(1)?;
        if same_model_name(&label,name)&&vr::deleted_at(c,0,id)?.is_none(){return Ok(Some(id));}
    }
    Ok(indexed)
}
pub fn name_exists(c:&Connection,name:&str)->super::Result<bool>{
    Ok(matching_name_id(c,name,None)?.is_some())
}
fn name_available(c: &Connection, name: &str, except: Option<i64>) -> Result<(), vr::VirtualError> {
    if model_routes::routes_for(c,name)?.is_some() {
        return Err(err(
            "virtual_route_conflict",
            "共享模型名不能与标准模型重名",
        ));
    }
    let private: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_virtual_models WHERE normalized_name=?1)",
        [normalize_model_name(name)],
        |r| r.get(0),
    )?;
    let active_collision=matching_name_id(c,name,except)?.is_some();
    if private || active_collision {
        return Err(err(
            "virtual_route_conflict",
            "共享模型名不能与现有私有或共享模型重名",
        ));
    }
    Ok(())
}
pub fn preflight(
    c: &Connection,
    v: &vr::Config,
    except: Option<i64>,
) -> Result<(), vr::VirtualError> {
    vr::preflight_shared(c, v)?;
    name_available(c, &v.name, except)
}
fn audit(
    c: &Connection,
    id: i64,
    version: i64,
    actor: i64,
    action: &str,
) -> Result<(), vr::VirtualError> {
    c.execute("INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json) VALUES(?1,?2,'shared_virtual_model',?3,?4)",params![super::now_ms(),action,id,serde_json::json!({"actor_account_id":actor,"version":version}).to_string()])?;
    Ok(())
}
pub fn create(c: &Connection, v: &vr::Config, actor: i64) -> Result<SharedModel, vr::VirtualError> {
    let tx = c.unchecked_transaction()?;
    root_actor(&tx, actor)?;
    preflight(&tx, v, None)?;
    let mut config = v.clone();
    config.name = config.name.trim().to_owned();
    tx.execute("INSERT INTO shared_virtual_models(name,normalized_name,enabled,config_json,version,actor_account_id,created_ms,updated_ms) VALUES(?1,?2,?3,?4,1,?5,?6,?6)",params![config.name,normalize_model_name(&config.name),config.enabled,serde_json::to_string(&config).map_err(|_|err("invalid_virtual_route","无法编码共享配置"))?,actor,super::now_ms()])?;
    let id = tx.last_insert_rowid();
    audit(&tx, id, 1, actor, "shared_virtual_model.create")?;
    tx.commit()?;
    Ok(get(c,id)?.unwrap())
}
pub fn replace(
    c: &Connection,
    id: i64,
    version: i64,
    v: &vr::Config,
    actor: i64,
) -> Result<SharedModel, vr::VirtualError> {
    let tx = c.unchecked_transaction()?;
    root_actor(&tx, actor)?;
    let old = get(&tx,id)?
        .ok_or_else(|| err("virtual_route_not_found", "共享模型不存在"))?;
    if old.version != version {
        return Err(err("virtual_route_changed", "共享模型已修改，请刷新"));
    }
    preflight(&tx, v, Some(id))?;
    let mut config = v.clone();
    config.name = config.name.trim().to_owned();
    let changed=tx.execute("UPDATE shared_virtual_models SET name=?3,normalized_name=?4,enabled=?5,config_json=?6,version=version+1,actor_account_id=?7,updated_ms=?8 WHERE id=?1 AND version=?2",params![id,version,config.name,normalize_model_name(&config.name),config.enabled,serde_json::to_string(&config).map_err(|_|err("invalid_virtual_route","无法编码共享配置"))?,actor,super::now_ms()])?;
    if changed != 1 {
        return Err(err("virtual_route_changed", "共享模型已修改，请刷新"));
    }
    audit(&tx, id, version + 1, actor, "shared_virtual_model.update")?;
    tx.commit()?;
    Ok(get(c,id)?.unwrap())
}
pub fn disable(
    c: &Connection,
    id: i64,
    version: i64,
    actor: i64,
) -> Result<SharedModel, vr::VirtualError> {
    let tx = c.unchecked_transaction()?;
    root_actor(&tx, actor)?;
    let mut old = get(&tx,id)?
        .ok_or_else(|| err("virtual_route_not_found", "共享模型不存在"))?;
    if old.version != version {
        return Err(err("virtual_route_changed", "共享模型已修改，请刷新"));
    }
    old.config.enabled = false;
    let n=tx.execute("UPDATE shared_virtual_models SET enabled=0,config_json=?3,version=version+1,actor_account_id=?4,updated_ms=?5 WHERE id=?1 AND version=?2",params![id,version,serde_json::to_string(&old.config).map_err(|_|err("invalid_virtual_route","无法编码共享配置"))?,actor,super::now_ms()])?;
    if n != 1 {
        return Err(err("virtual_route_changed", "共享模型已修改，请刷新"));
    }
    audit(&tx, id, version + 1, actor, "shared_virtual_model.disable")?;
    tx.commit()?;
    Ok(get(c,id)?.unwrap())
}
pub fn trash(c:&Connection,id:i64,version:i64,actor:i64)->Result<SharedModel,vr::VirtualError>{
    let tx=c.unchecked_transaction()?;root_actor(&tx,actor)?;
    let mut old=get(&tx,id)?.ok_or_else(||err("virtual_route_not_found","共享模型不存在"))?;
    if old.version!=version{return Err(err("virtual_route_changed","共享模型已修改，请刷新"));}
    old.config.enabled=false;let now=super::now_ms();
    let n=tx.execute("UPDATE shared_virtual_models SET enabled=0,normalized_name=?3,config_json=?4,version=version+1,actor_account_id=?5,updated_ms=?6 WHERE id=?1 AND version=?2",params![id,version,format!("\u{1}tokengo-trash:0:{id}"),serde_json::to_string(&old.config).map_err(|_|err("invalid_virtual_route","无法编码共享配置"))?,actor,now])?;
    if n!=1{return Err(err("virtual_route_changed","共享模型已修改，请刷新"));}
    super::settings::set(&tx,&format!("internal_virtual_model_trash_v1:0:{id}"),&now.to_string())?;audit(&tx,id,version+1,actor,"shared_virtual_model.trash")?;
    let saved=by_id(&tx,id)?.unwrap();tx.commit()?;Ok(saved)
}
pub fn restore(c:&Connection,id:i64,version:i64,actor:i64)->Result<SharedModel,vr::VirtualError>{
    let tx=c.unchecked_transaction()?;root_actor(&tx,actor)?;
    let mut old=by_id(&tx,id)?.filter(|r|r.deleted_at_ms.is_some()).ok_or_else(||err("virtual_route_not_found","回收站中不存在此共享模型"))?;
    if old.version!=version{return Err(err("virtual_route_changed","共享模型已修改，请刷新"));}
    name_available(&tx,&old.config.name,Some(id))?;old.config.enabled=false;
    let n=tx.execute("UPDATE shared_virtual_models SET enabled=0,normalized_name=?3,config_json=?4,version=version+1,actor_account_id=?5,updated_ms=?6 WHERE id=?1 AND version=?2",params![id,version,normalize_model_name(&old.config.name),serde_json::to_string(&old.config).map_err(|_|err("invalid_virtual_route","无法编码共享配置"))?,actor,super::now_ms()])?;
    if n!=1{return Err(err("virtual_route_changed","共享模型已修改，请刷新"));}
    super::settings::delete(&tx,&format!("internal_virtual_model_trash_v1:0:{id}"))?;audit(&tx,id,version+1,actor,"shared_virtual_model.restore")?;
    let saved=get(&tx,id)?.unwrap();tx.commit()?;Ok(saved)
}

fn fresh(
    c: &Connection,
    auth: &identity::AuthContext,
) -> Result<identity::AuthContext, vr::VirtualError> {
    if auth.key_id > 0 {
        identity::context(c, auth.key_id)?
            .filter(|a| a.user_id == auth.user_id)
            .ok_or_else(|| err("virtual_route_forbidden", "当前密钥已撤销或失效"))
    } else {
        vr::owner_auth(c, auth.user_id)
    }
}
fn live(
    c: &Connection,
    auth: &identity::AuthContext,
    r: &SharedModel,
) -> Result<(), vr::VirtualError> {
    if !r.config.enabled || !auth.permits_model(&r.config.name) {
        return Err(err("virtual_route_forbidden", "共享模型已停用或未授权"));
    }
    name_available(c, &r.config.name, Some(r.id))?;
    vr::validate_shape(&r.config)?;
    vr::validate_target_kinds(c, &r.config.targets)?;
    Ok(())
}
pub fn resolve(
    c: &Connection,
    auth: &identity::AuthContext,
    name: &str,
    now: i64,
) -> Result<Option<vr::ResolvedVirtualPlan>, vr::VirtualError> {
    let Some(r) = by_name(c,name)? else {
        return Ok(None);
    };
    let auth = fresh(c, auth)?;
    live(c, &auth, &r)?;
    let mut targets = Vec::new();
    for position in vr::time_positions(&r.config, now)? {
        let t = &r.config.targets[position - 1];
        if vr::allowed_sources_shared(c, &auth, t)?.is_empty() {
            continue;
        }
        targets.push(vr::ResolvedTarget {
            position,
            provider_id: t.provider_id,
            logical_model: t.logical_model.clone(),
            switch_on_429: t.switch_on_429,
            switch_on_400: t.switch_on_400,
        });
    }
    if targets.is_empty() {
        return Err(err(
            "virtual_target_forbidden",
            "共享模型没有已授权的可用来源",
        ));
    }
    Ok(Some(vr::ResolvedVirtualPlan {
        id: r.id,
        user_id: auth.user_id,
        name: r.config.name,
        version: r.version,
        targets,
        compatibility: None,
        shared: true,
    }))
}
pub fn validate_current(
    c: &Connection,
    auth: &identity::AuthContext,
    id: i64,
    version: i64,
    pos: usize,
    now: i64,
) -> Result<Option<Contract>, vr::VirtualError> {
    let r = get(c,id)?
        .ok_or_else(|| err("virtual_route_not_found", "共享模型不存在"))?;
    let auth = fresh(c, auth)?;
    live(c, &auth, &r)?;
    if r.version != version {
        return Err(err("virtual_route_changed", "共享模型已修改，请重试"));
    }
    if !vr::time_positions(&r.config, now)?.contains(&pos) {
        return Err(err("route_time_unavailable", "共享模型目标当前时段不可用"));
    }
    let t = r
        .config
        .targets
        .get(
            pos.checked_sub(1)
                .ok_or_else(|| err("invalid_virtual_route", "目标位置无效"))?,
        )
        .ok_or_else(|| err("invalid_virtual_route", "目标位置无效"))?;
    if vr::allowed_sources_shared(c, &auth, t)?.is_empty() {
        return Err(err("virtual_target_forbidden", "共享模型目标已撤权或停用"));
    }
    Ok(None)
}
pub fn alias_names(
    c: &Connection,
    auth: &identity::AuthContext,
    now: i64,
) -> Result<Vec<String>, vr::VirtualError> {
    let mut names = Vec::new();
    for r in list(c)? {
        match resolve(c, auth, &r.config.name, now) {
            Ok(Some(_)) => names.push(r.config.name),
            Ok(None) => {}
            Err(e) if e.code != "virtual_route_storage_error" => {}
            Err(e) => return Err(e),
        }
    }
    Ok(names)
}
