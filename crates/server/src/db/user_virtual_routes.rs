//! User-owned metadata and an injected-clock planner. Never issues upstream HTTP.
use super::{identity, model_routes, DbError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fmt;
use tokengo_shared::{normalize_model_name, same_model_name, ModelKind};
pub const SQL: &str = include_str!("user_virtual_routes.sql");
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub provider_id: i64,
    pub logical_model: String,
    pub switch_on_429: bool,
    pub switch_on_400: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Window {
    /// Monday is bit 0; Sunday is bit 6. Half-open local minute interval.
    pub days_mask: u8,
    pub start_minute: u16,
    pub end_minute: u16,
    pub target_positions: Vec<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub name: String,
    pub enabled: bool,
    pub mode: String,
    pub timezone: String,
    pub targets: Vec<Target>,
    pub windows: Vec<Window>,
    pub default_positions: Vec<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VirtualModel {
    pub deleted_at_ms: Option<i64>,
    pub id: i64,
    pub user_id: i64,
    pub version: i64,
    #[serde(flatten)]
    pub config: Config,
}
#[derive(Clone, Debug)]
pub struct ResolvedTarget {
    pub position: usize,
    pub provider_id: i64,
    pub logical_model: String,
    pub switch_on_429: bool,
    pub switch_on_400: bool,
}
#[derive(Clone, Debug)]
pub struct ResolvedVirtualPlan {
    pub shared: bool,
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub version: i64,
    pub targets: Vec<ResolvedTarget>,
    pub compatibility: Option<crate::vector_compat::Contract>,
}
#[derive(Debug)]
pub struct VirtualError {
    pub code: &'static str,
    pub message: String,
}
impl fmt::Display for VirtualError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for VirtualError {}
impl From<DbError> for VirtualError {
    fn from(_: DbError) -> Self {
        error("virtual_route_storage_error", "无法读取或保存虚拟模型配置")
    }
}
impl From<rusqlite::Error> for VirtualError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::from(e).into()
    }
}
fn error(code: &'static str, message: impl Into<String>) -> VirtualError {
    VirtualError {
        code,
        message: message.into(),
    }
}
fn invalid(message: impl Into<String>) -> VirtualError {
    error("invalid_virtual_route", message)
}
fn positions(v: &[usize], n: usize) -> bool {
    v.len() <= 16
        && v.iter()
            .enumerate()
            .all(|(i, p)| *p > 0 && *p <= n && !v[..i].contains(p))
}
pub fn validate_shape(v: &Config) -> Result<(), VirtualError> {
    validate_stored_shape(v, false)
}
// Deleting a provider may leave a disabled route with an empty target or time slot.
// Readers must still expose it for repair; writes and re-enabling always use strict validation.
fn validate_stored_shape(v: &Config, allow_incomplete: bool) -> Result<(), VirtualError> {
    let incomplete = allow_incomplete && !v.enabled;
    let name = v.name.trim();
    if name.is_empty()
        || name.chars().count() > 128
        || name.chars().any(char::is_control)
        || model_routes::is_auto(name)
        || normalize_model_name(name).is_empty()
    {
        return Err(invalid("虚拟模型名须为1至128字符，不能使用auto或控制字符"));
    }
    if !["ordered", "scheduled"].contains(&v.mode.as_str())
        || !["Asia/Shanghai", "UTC"].contains(&v.timezone.as_str())
    {
        return Err(invalid("仅支持顺序/时段模式及Asia/Shanghai或UTC时区"));
    }
    if (v.targets.is_empty() && !incomplete)
        || v.targets.len() > 16
        || v.windows.len() > 64
        || !positions(&v.default_positions, v.targets.len())
    {
        return Err(invalid("目标须为1至16项，默认链位置须唯一有效"));
    }
    for (i, t) in v.targets.iter().enumerate() {
        if t.provider_id <= 0
            || t.logical_model.trim().is_empty()
            || model_routes::is_auto(&t.logical_model)
            || t.logical_model.chars().any(char::is_control)
            || t.logical_model.len() > 1024
            || v.targets[..i].iter().any(|a| {
                a.provider_id == t.provider_id
                    && same_model_name(&a.logical_model, &t.logical_model)
            })
        {
            return Err(invalid(
                "目标必须是唯一的真实渠道和逻辑模型，不能递归或使用auto",
            ));
        }
    }
    if v.mode == "ordered" && (!v.windows.is_empty() || !v.default_positions.is_empty()) {
        return Err(invalid("顺序模式直接使用全部目标，不能附加时段或默认链"));
    }
    for (i, w) in v.windows.iter().enumerate() {
        if w.days_mask == 0
            || w.days_mask > 127
            || w.start_minute >= w.end_minute
            || w.end_minute > 1440
            || (w.target_positions.is_empty() && !incomplete)
            || !positions(&w.target_positions, v.targets.len())
        {
            return Err(invalid("时段须为有效的半开区间；跨午夜请拆成两段"));
        }
        if v.windows[..i].iter().any(|a| {
            a.days_mask & w.days_mask != 0
                && a.start_minute < w.end_minute
                && w.start_minute < a.end_minute
        }) {
            return Err(invalid("同一天的时段不能重叠"));
        }
    }
    Ok(())
}
pub fn time_positions(v: &Config, now_ms: i64) -> Result<Vec<usize>, VirtualError> {
    validate_shape(v)?;
    if v.mode == "ordered" {
        return Ok((1..=v.targets.len()).collect());
    }
    let offset = if v.timezone == "Asia/Shanghai" {
        28_800_000
    } else {
        0
    };
    let local = now_ms
        .checked_add(offset)
        .ok_or_else(|| invalid("时钟超出范围"))?;
    let day = local.div_euclid(86_400_000);
    let weekday = (day + 3).rem_euclid(7) as u8;
    let minute = (local.rem_euclid(86_400_000) / 60_000) as u16;
    for w in &v.windows {
        if w.days_mask & (1 << weekday) != 0 && minute >= w.start_minute && minute < w.end_minute {
            return Ok(w.target_positions.clone());
        }
    }
    if v.default_positions.is_empty() {
        Err(error(
            "route_time_unavailable",
            "当前时段未配置可用目标或默认链",
        ))
    } else {
        Ok(v.default_positions.clone())
    }
}
pub fn owner_auth(c: &Connection, user_id: i64) -> Result<identity::AuthContext, VirtualError> {
    // Scope zero is server-owned compatibility evidence, never an inference identity.
    if user_id == 0 {
        return Ok(identity::AuthContext { user_id: 0, key_id: 0, role: "system_compatibility".into(), user_policy: identity::Policy::default(), key_policy: identity::Policy::default(), ancestor_key_ids: vec![], expires_at: None, policy_version: String::new() });
    }
    let u = identity::user(c, user_id)?
        .filter(|u| u.enabled)
        .ok_or_else(|| error("virtual_route_forbidden", "用户已停用"))?;
    Ok(identity::AuthContext {
        user_id,
        key_id: 0,
        role: "user".into(),
        user_policy: u.policy,
        key_policy: identity::Policy::default(),
        ancestor_key_ids: vec![],
        expires_at: None,
        policy_version: String::new(),
    })
}
pub fn allowed_sources(
    c: &Connection,
    auth: &identity::AuthContext,
    t: &Target,
) -> Result<Vec<i64>, VirtualError> {
    if !auth.permits_model(&t.logical_model) || super::model_access::internal_only(c, &t.logical_model)? {
        return Ok(vec![]);
    }
    allowed_sources_from(c, auth, t, model_routes::channels_for(c, &t.logical_model)?)
}
/// Internal delegation skips only the target-name grant, preserving source and cost policy.
pub fn allowed_sources_shared(c: &Connection, auth: &identity::AuthContext, t: &Target) -> Result<Vec<i64>, VirtualError> {
    allowed_sources_from(c,auth,t,model_routes::provider_channels_for(c,&t.logical_model,t.provider_id)?)
}
fn allowed_sources_from(c:&Connection,auth:&identity::AuthContext,t:&Target,channels:Vec<tokengo_shared::Channel>)->Result<Vec<i64>,VirtualError>{
    let mut ids = Vec::new();
    for ch in channels {
        if !ch.routable()
            || super::provider_channels::membership(c, ch.id.0)?
                .is_none_or(|(p, _, enabled)| p != t.provider_id || !enabled)
        {
            continue;
        }
        if super::provider_pool::source_credential(c, ch.id.0)?
            .is_none_or(|credential| !credential.enabled || credential.provider_id != t.provider_id)
        {
            continue;
        }
        let tools = crate::search_tools::load(c, ch.id.0)?;
        let paid = if tools.search || tools.fetch {
            super::billing::request_price(c, &t.logical_model, ch.id.0)?
                .is_none_or(|p| p.micro_per_call > 0)
        } else {
            super::billing::price(c, &t.logical_model, ch.id.0)?
                .is_none_or(|p| p.input_micro_per_million > 0 || p.output_micro_per_million > 0)
        };
        if auth.permits_channel(ch.id.0, paid) {
            ids.push(ch.id.0);
        }
    }
    Ok(ids)
}
fn validate(c: &Connection, user_id: i64, v: &Config) -> Result<(), VirtualError> {
    preflight(c, user_id, v)?;
    compatibility(c, user_id, &v.targets)?;
    Ok(())
}

/// Reject invalid or unauthorized edits before billable compatibility probes.
pub fn preflight(c: &Connection, user_id: i64, v: &Config) -> Result<(), VirtualError> {
    validate_shape(v)?;
    let auth = owner_auth(c, user_id)?;
    if shared_name_exists(c, &v.name)? {
        return Err(invalid("虚拟模型名已被管理员共享入口使用"));
    }
    if model_routes::routes_for(c, &v.name)?.is_some()
    {
        return Err(invalid("虚拟模型名不能与标准模型重名"));
    }
    let mut kind = None;
    let mut first: Option<&str> = None;
    for t in &v.targets {
        if allowed_sources(c, &auth, t)?.is_empty() {
            return Err(error(
                "virtual_target_forbidden",
                "目标模型或渠道未授权、已停用或不存在；不能指向其他虚拟模型",
            ));
        }
        let route = model_routes::routes_for(c, &t.logical_model)?
            .ok_or_else(|| invalid("目标必须是真实逻辑模型"))?;
        if let Some(k) = kind {
            if k != route.kind {
                return Err(invalid("同一虚拟链的模型用途必须一致"));
            }
        }
        if route.kind == ModelKind::Unknown
            && first.is_some_and(|n: &str| !same_model_name(n, &t.logical_model))
        {
            return Err(invalid(
                "不同模型的未知用途不能证明兼容，请先由管理员确认用途",
            ));
        }
        kind = Some(route.kind);
        first = Some(t.logical_model.as_str());
    }
    Ok(())
}


/// Admin-owned chains may delegate real targets but never recurse into aliases.
pub fn preflight_shared(c: &Connection, v: &Config) -> Result<ModelKind, VirtualError> {
    validate_shape(v)?;
    if model_routes::routes_for(c, &v.name)?.is_some() {
        return Err(invalid("共享虚拟模型名不能与标准模型重名"));
    }
    let collision: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM user_virtual_models WHERE normalized_name=?1)", [normalize_model_name(&v.name)], |r| r.get(0))?;
    if collision { return Err(invalid("共享虚拟模型名不能与用户私有模型重名")); }
    let auth = owner_auth(c, 0)?;
    let mut kind = None;
    let mut first: Option<&str> = None;
    for t in &v.targets {
        if allowed_sources_shared(c, &auth, t)?.is_empty() { return Err(error("virtual_target_forbidden", "共享目标已停用、不存在或没有可用来源")); }
        let route = model_routes::routes_for(c, &t.logical_model)?.ok_or_else(|| invalid("共享目标必须是真实逻辑模型"))?;
        if kind.is_some_and(|k| k != route.kind) { return Err(invalid("共享调用链的模型用途必须一致")); }
        if route.kind == ModelKind::Unknown && first.is_some_and(|n| !same_model_name(n, &t.logical_model)) { return Err(invalid("未知用途的不同模型不能证明兼容，请先确认用途")); }
        kind = Some(route.kind); first = Some(&t.logical_model);
    }
    Ok(kind.unwrap_or_default())
}

fn compatibility(
    c: &Connection,
    user_id: i64,
    targets: &[Target],
) -> Result<Option<crate::vector_compat::Contract>, VirtualError> {
    if targets.len() < 2 {
        return Ok(None);
    }
    let kind = model_routes::routes_for(c, &targets[0].logical_model)?
        .map(|r| r.kind).unwrap_or_default();
    if matches!(kind, ModelKind::Embeddings | ModelKind::Rerank) {
        return crate::vector_compat::require(c, user_id, targets, kind).map(Some);
    }
    Ok(None)
}
pub fn deleted_at(c: &Connection, user_id: i64, id: i64) -> super::Result<Option<i64>> {
    super::settings::get(c, &trash_key(user_id,id))?.map(|raw|raw.parse::<i64>().map_err(|_|DbError::Other("Invalid virtual model trash marker".into()))).transpose()
}
fn trash_key(user_id:i64,id:i64)->String {format!("internal_virtual_model_trash_v1:{user_id}:{id}")}
pub fn list(c: &Connection, user_id: i64) -> super::Result<Vec<VirtualModel>> {
    Ok(list_all(c,user_id)?.into_iter().filter(|r|r.deleted_at_ms.is_none()).collect())
}
type StoredVirtualRow = (i64, String, i64, String, String, i64, String);
const MODEL_COLUMNS: &str = "id,name,enabled,mode,timezone,version,default_positions_json";
fn stored_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredVirtualRow> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?))
}
fn hydrate(c: &Connection, user_id: i64, row: StoredVirtualRow) -> super::Result<VirtualModel> {
    let (id, name, enabled, mode, timezone, version, defaults) = row;
    let mut st=c.prepare("SELECT provider_id,logical_model,switch_on_429,switch_on_400 FROM user_virtual_targets WHERE virtual_model_id=?1 ORDER BY position")?;
    let targets = st
        .query_map([id], |r| {
            Ok(Target {
                provider_id: r.get(0)?,
                logical_model: r.get(1)?,
                switch_on_429: r.get::<_, i64>(2)? != 0,
                switch_on_400: r.get::<_, i64>(3)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut st=c.prepare("SELECT days_mask,start_minute,end_minute,target_positions_json FROM user_virtual_windows WHERE virtual_model_id=?1 ORDER BY position")?;
    let raw = st
        .query_map([id], |r| {
            Ok((
                r.get::<_, u8>(0)?,
                r.get::<_, u16>(1)?,
                r.get::<_, u16>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut windows = Vec::new();
    for (days_mask, start_minute, end_minute, s) in raw {
        windows.push(Window {
            days_mask,
            start_minute,
            end_minute,
            target_positions: serde_json::from_str(&s)
                .map_err(|_| DbError::Other("Invalid virtual window".into()))?,
        });
    }
    let config = Config {
        name,
        enabled: enabled != 0,
        mode,
        timezone,
        targets,
        windows,
        default_positions: serde_json::from_str(&defaults)
            .map_err(|_| DbError::Other("Invalid virtual defaults".into()))?,
    };
    validate_stored_shape(&config, true)
        .map_err(|_| DbError::Other("Invalid persisted virtual route".into()))?;
    Ok(VirtualModel {
        deleted_at_ms: deleted_at(c,user_id,id)?, id, user_id, version, config,
    })
}
pub fn list_all(c: &Connection, user_id: i64) -> super::Result<Vec<VirtualModel>> {
    let mut statement = c.prepare(&format!("SELECT {MODEL_COLUMNS} FROM user_virtual_models WHERE user_id=?1 ORDER BY id"))?;
    let rows = statement.query_map([user_id], stored_row)?.collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(|row| hydrate(c,user_id,row)).collect()
}
fn get_including_trash(c: &Connection, user_id: i64, id: i64) -> super::Result<Option<VirtualModel>> {
    let row = c.query_row(&format!("SELECT {MODEL_COLUMNS} FROM user_virtual_models WHERE id=?1 AND user_id=?2"), params![id,user_id], stored_row).optional()?;
    row.map(|row| hydrate(c,user_id,row)).transpose()
}
/// Resolve one owned active record without reading unrelated routes or their children.
pub fn get(c: &Connection, user_id: i64, id: i64) -> super::Result<Option<VirtualModel>> {
    if deleted_at(c,user_id,id)?.is_some() { return Ok(None); }
    get_including_trash(c,user_id,id)
}
// Older installations can contain normalized_name values written by a previous
// normalizer. A miss may inspect only this user's id/name metadata; it never
// hydrates unrelated targets, windows or JSON configuration.
fn legacy_name_id(c: &Connection, user_id: i64, name: &str, except_id: i64, candidate_id: Option<i64>) -> super::Result<Option<i64>> {
    let mut statement=c.prepare("SELECT id,name FROM user_virtual_models WHERE user_id=?1 AND id<>?2 AND id<=?3 ORDER BY id")?;
    let mut rows=statement.query(params![user_id,except_id,candidate_id.unwrap_or(i64::MAX)])?;
    while let Some(row)=rows.next()? {
        let id:i64=row.get(0)?;
        let stored_name:String=row.get(1)?;
        if same_model_name(&stored_name,name) && deleted_at(c,user_id,id)?.is_none() {
            return Ok(Some(id));
        }
    }
    Ok(None)
}
fn by_name(c: &Connection, user_id: i64, name: &str) -> super::Result<Option<VirtualModel>> {
    let indexed=c.query_row("SELECT id,name FROM user_virtual_models WHERE user_id=?1 AND normalized_name=?2", params![user_id,normalize_model_name(name)], |row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?))).optional()?;
    let candidate_id=match indexed {
        Some((id,stored_name)) if same_model_name(&stored_name,name) && deleted_at(c,user_id,id)?.is_none()=>Some(id),
        _=>None,
    };
    // The old directory selected the first non-trashed matching name in ID order.
    // Legacy and canonical indexes may coexist, so inspect only metadata up to
    // the indexed candidate before hydrating exactly the selected record.
    match legacy_name_id(c,user_id,name,-1,candidate_id)? {
        Some(id)=>get(c,user_id,id),
        None=>Ok(None),
    }
}

fn name_exists(c: &Connection, user_id: i64, name: &str, except_id: i64) -> super::Result<bool> {
    let indexed=c.query_row("SELECT id,name FROM user_virtual_models WHERE user_id=?1 AND normalized_name=?2 AND id<>?3", params![user_id,normalize_model_name(name),except_id], |row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?))).optional()?;
    if let Some((id,stored_name))=indexed {
        if same_model_name(&stored_name,name) && deleted_at(c,user_id,id)?.is_none() {return Ok(true);}
    }
    Ok(legacy_name_id(c,user_id,name,except_id,None)?.is_some())
}
fn shared_name_exists(c: &Connection, name: &str) -> Result<bool,VirtualError> {
    super::shared_virtual_routes::name_exists(c,name).map_err(Into::into)
}

fn save_children(c: &Connection, id: i64, v: &Config) -> Result<(), VirtualError> {
    c.execute(
        "DELETE FROM user_virtual_windows WHERE virtual_model_id=?1",
        [id],
    )?;
    c.execute(
        "DELETE FROM user_virtual_targets WHERE virtual_model_id=?1",
        [id],
    )?;
    for (i, t) in v.targets.iter().enumerate() {
        c.execute(
            "INSERT INTO user_virtual_targets VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id,
                (i + 1) as i64,
                t.provider_id,
                t.logical_model.trim(),
                t.switch_on_429,
                t.switch_on_400
            ],
        )?;
    }
    for (i, w) in v.windows.iter().enumerate() {
        c.execute(
            "INSERT INTO user_virtual_windows VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id,
                i as i64,
                w.days_mask,
                w.start_minute,
                w.end_minute,
                serde_json::to_string(&w.target_positions).unwrap()
            ],
        )?;
    }
    Ok(())
}
fn audit(
    c: &Connection,
    user_id: i64,
    id: i64,
    action: &str,
    version: i64,
) -> Result<(), VirtualError> {
    c.execute("INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json) VALUES(?1,?2,'virtual_model',?3,?4)",params![super::now_ms(),action,id,serde_json::json!({"user_id":user_id,"version":version}).to_string()])?;
    Ok(())
}
pub fn create(c: &Connection, user_id: i64, v: &Config) -> Result<VirtualModel, VirtualError> {
    let tx = c.unchecked_transaction()?;
    validate(&tx, user_id, v)?;
    if name_exists(&tx, user_id, &v.name, -1)?
    {
        return Err(error(
            "virtual_route_conflict",
            "已存在同名私有模型，请编辑原记录",
        ));
    }
    tx.execute("INSERT INTO user_virtual_models(user_id,name,normalized_name,enabled,mode,timezone,default_positions_json,version,created_ms,updated_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?8)",params![user_id,v.name.trim(),normalize_model_name(&v.name),v.enabled,v.mode,v.timezone,serde_json::to_string(&v.default_positions).unwrap(),super::now_ms()])?;
    let id = tx.last_insert_rowid();
    save_children(&tx, id, v)?;
    audit(&tx, user_id, id, "virtual_model.create", 1)?;
    tx.commit()?;
    Ok(get(c, user_id, id)?.unwrap())
}
pub fn replace(
    c: &Connection,
    user_id: i64,
    id: i64,
    version: i64,
    v: &Config,
) -> Result<VirtualModel, VirtualError> {
    let tx = c.unchecked_transaction()?;
    owner_auth(&tx, user_id)?;
    let old = get(&tx, user_id, id)?
        .ok_or_else(|| error("virtual_route_not_found", "虚拟模型不存在"))?;
    if old.version != version {
        return Err(error("virtual_route_changed", "路由已修改，请刷新后重试"));
    }
    validate(&tx, user_id, v)?;
    if name_exists(&tx, user_id, &v.name, id)?
    {
        return Err(error("virtual_route_conflict", "已存在同名私有模型"));
    }
    let n=tx.execute("UPDATE user_virtual_models SET name=?3,normalized_name=?4,enabled=?5,mode=?6,timezone=?7,default_positions_json=?8,version=version+1,updated_ms=?9 WHERE id=?1 AND user_id=?2 AND version=?10",params![id,user_id,v.name.trim(),normalize_model_name(&v.name),v.enabled,v.mode,v.timezone,serde_json::to_string(&v.default_positions).unwrap(),super::now_ms(),version])?;
    if n != 1 {
        return Err(error("virtual_route_changed", "路由已修改，请刷新"));
    }
    save_children(&tx, id, v)?;
    audit(&tx, user_id, id, "virtual_model.update", version + 1)?;
    tx.commit()?;
    Ok(get(c, user_id, id)?.unwrap())
}
pub fn disable(
    c: &Connection,
    user_id: i64,
    id: i64,
    version: i64,
) -> Result<VirtualModel, VirtualError> {
    let tx = c.unchecked_transaction()?;
    owner_auth(&tx, user_id)?;
    if get(&tx,user_id,id)?.is_none() {return Err(error("virtual_route_not_found","虚拟模型不存在"));}
    let n=tx.execute("UPDATE user_virtual_models SET enabled=0,version=version+1,updated_ms=?4 WHERE id=?1 AND user_id=?2 AND version=?3",params![id,user_id,version,super::now_ms()])?;
    if n != 1 {
        return Err(if get(&tx,user_id,id)?.is_some() {
            error("virtual_route_changed", "路由已修改，请刷新")
        } else {
            error("virtual_route_not_found", "虚拟模型不存在")
        });
    }
    audit(&tx, user_id, id, "virtual_model.disable", version + 1)?;
    let saved = get(&tx,user_id,id)?.unwrap();
    tx.commit()?;
    Ok(saved)
}
/// Soft deletion keeps route children and financial/audit history, freeing only its alias.
pub fn trash(c:&Connection,user_id:i64,id:i64,version:i64)->Result<VirtualModel,VirtualError>{
    let tx=c.unchecked_transaction()?;owner_auth(&tx,user_id)?;
    let old=get(&tx,user_id,id)?.ok_or_else(||error("virtual_route_not_found","虚拟模型不存在"))?;
    if old.version!=version{return Err(error("virtual_route_changed","路由已修改，请刷新后重试"));}
    let now=super::now_ms();let tombstone=format!("\u{1}tokengo-trash:{user_id}:{id}");
    let n=tx.execute("UPDATE user_virtual_models SET enabled=0,normalized_name=?4,version=version+1,updated_ms=?5 WHERE id=?1 AND user_id=?2 AND version=?3",params![id,user_id,version,tombstone,now])?;
    if n!=1{return Err(error("virtual_route_changed","路由已修改，请刷新后重试"));}
    super::settings::set(&tx,&trash_key(user_id,id),&now.to_string())?;
    audit(&tx,user_id,id,"virtual_model.trash",version+1)?;
    let saved=get_including_trash(&tx,user_id,id)?.unwrap();tx.commit()?;Ok(saved)
}
pub fn restore(c:&Connection,user_id:i64,id:i64,version:i64)->Result<VirtualModel,VirtualError>{
    let tx=c.unchecked_transaction()?;owner_auth(&tx,user_id)?;
    let old=get_including_trash(&tx,user_id,id)?.filter(|r|r.deleted_at_ms.is_some()).ok_or_else(||error("virtual_route_not_found","回收站中不存在此虚拟模型"))?;
    if old.version!=version{return Err(error("virtual_route_changed","路由已修改，请刷新后重试"));}
    let name=&old.config.name;
    if model_routes::routes_for(&tx,name)?.is_some() || shared_name_exists(&tx,name)? || name_exists(&tx,user_id,name,id)? {return Err(error("virtual_route_conflict","原模型名称已被占用，请先处理同名模型后恢复"));}
    let n=tx.execute("UPDATE user_virtual_models SET enabled=0,normalized_name=?4,version=version+1,updated_ms=?5 WHERE id=?1 AND user_id=?2 AND version=?3",params![id,user_id,version,normalize_model_name(name),super::now_ms()])?;
    if n!=1{return Err(error("virtual_route_changed","路由已修改，请刷新后重试"));}
    super::settings::delete(&tx,&trash_key(user_id,id))?;audit(&tx,user_id,id,"virtual_model.restore",version+1)?;
    let saved=get(&tx,user_id,id)?.unwrap();tx.commit()?;Ok(saved)
}

pub fn resolve(
    c: &Connection,
    user_id: i64,
    name: &str,
    now_ms: i64,
) -> Result<Option<ResolvedVirtualPlan>, VirtualError> {
    owner_auth(c, user_id)?;
    let Some(r) = by_name(c, user_id, name)?
    else {
        return Ok(None);
    };
    if !r.config.enabled {
        return Err(error("virtual_route_forbidden", "虚拟模型已停用"));
    }
    if model_routes::routes_for(c, &r.config.name)?.is_some()
    {
        return Err(error(
            "virtual_route_changed",
            "该名称已成为标准模型名，请更改私有模型名称",
        ));
    }
    let indices = time_positions(&r.config, now_ms)?;
    let compatibility = compatibility(c, user_id, &r.config.targets)?;
    let targets = indices
        .into_iter()
        .map(|position| {
            let t = &r.config.targets[position - 1];
            ResolvedTarget {
                position,
                provider_id: t.provider_id,
                logical_model: t.logical_model.clone(),
                switch_on_429: t.switch_on_429,
                switch_on_400: t.switch_on_400,
            }
        })
        .collect();
    Ok(Some(ResolvedVirtualPlan {
        shared: false,
        id: r.id,
        user_id,
        name: r.config.name,
        version: r.version,
        targets,
        compatibility,
    }))
}
pub fn validate_current(
    c: &Connection,
    user_id: i64,
    id: i64,
    version: i64,
    position: usize,
    now_ms: i64,
) -> Result<Option<crate::vector_compat::Contract>, VirtualError> {
    let auth = owner_auth(c, user_id)?;
    let r = get(c, user_id, id)?
        .ok_or_else(|| error("virtual_route_forbidden", "路由不属于当前用户"))?;
    if r.version != version || !r.config.enabled {
        return Err(error("virtual_route_changed", "路由已修改或停用，请重试"));
    }
    if model_routes::routes_for(c, &r.config.name)?.is_some()
    {
        return Err(error(
            "virtual_route_changed",
            "该名称已成为标准模型名，请更改私有模型名称",
        ));
    }
    if !time_positions(&r.config, now_ms)?.contains(&position) {
        return Err(error("route_time_unavailable", "该目标当前时段不再可用"));
    }
    let compatibility = compatibility(c, user_id, &r.config.targets)?;
    let t = r
        .config
        .targets
        .get(
            position
                .checked_sub(1)
                .ok_or_else(|| invalid("目标位置无效"))?,
        )
        .ok_or_else(|| invalid("目标位置无效"))?;
    if allowed_sources(c, &auth, t)?.is_empty() {
        return Err(error("virtual_target_forbidden", "目标已撤权或停用"));
    }
    Ok(compatibility)
}
pub fn alias_names(
    c: &Connection,
    auth: &identity::AuthContext,
    now_ms: i64,
) -> Result<Vec<String>, VirtualError> {
    let fresh = if auth.key_id > 0 {
        identity::context(c, auth.key_id)?
            .filter(|a| a.user_id == auth.user_id)
            .ok_or_else(|| error("virtual_route_forbidden", "当前密钥已撤销或失效"))?
    } else {
        owner_auth(c, auth.user_id)?
    };
    let mut names = Vec::new();
    for r in list(c, fresh.user_id)? {
        if !r.config.enabled
            || model_routes::routes_for(c, &r.config.name)?.is_some()
        {
            continue;
        }
        let positions = match time_positions(&r.config, now_ms) {
            Ok(v) => v,
            Err(e) if e.code == "route_time_unavailable" => continue,
            Err(e) => return Err(e),
        };
        for pos in positions {
            if !allowed_sources(c, &fresh, &r.config.targets[pos - 1])?.is_empty() {
                names.push(r.config.name.clone());
                break;
            }
        }
    }
    Ok(names)
}
