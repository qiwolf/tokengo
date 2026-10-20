//! User-owned metadata and an injected-clock planner. Never issues upstream HTTP.
use super::{identity, model_routes, DbError};
use rusqlite::{params, Connection};
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
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub version: i64,
    pub targets: Vec<ResolvedTarget>,
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
    if v.targets.is_empty()
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
            || w.target_positions.is_empty()
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
    if !auth.permits_model(&t.logical_model) {
        return Ok(vec![]);
    }
    let mut ids = Vec::new();
    for ch in model_routes::channels_for(c, &t.logical_model)? {
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
    validate_shape(v)?;
    let auth = owner_auth(c, user_id)?;
    if model_routes::list(c)?
        .iter()
        .any(|m| same_model_name(&m.name, &v.name))
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
        if matches!(route.kind, ModelKind::Embeddings) && v.targets.len() > 1 {
            return Err(invalid(
                "向量虚拟回退需要已核实的维度兼容合同，首版暂不允许配置",
            ));
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
pub fn list(c: &Connection, user_id: i64) -> super::Result<Vec<VirtualModel>> {
    let mut st=c.prepare("SELECT id,name,enabled,mode,timezone,version,default_positions_json FROM user_virtual_models WHERE user_id=?1 ORDER BY id")?;
    let rows = st
        .query_map([user_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for (id, name, enabled, mode, timezone, version, defaults) in rows {
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
        validate_shape(&config)
            .map_err(|_| DbError::Other("Invalid persisted virtual route".into()))?;
        out.push(VirtualModel {
            id,
            user_id,
            version,
            config,
        });
    }
    Ok(out)
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
    if list(&tx, user_id)?
        .iter()
        .any(|r| same_model_name(&r.config.name, &v.name))
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
    Ok(list(c, user_id)?.into_iter().find(|r| r.id == id).unwrap())
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
    let old = list(&tx, user_id)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| error("virtual_route_not_found", "虚拟模型不存在"))?;
    if old.version != version {
        return Err(error("virtual_route_changed", "路由已修改，请刷新后重试"));
    }
    validate(&tx, user_id, v)?;
    if list(&tx, user_id)?
        .iter()
        .any(|r| r.id != id && same_model_name(&r.config.name, &v.name))
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
    Ok(list(c, user_id)?.into_iter().find(|r| r.id == id).unwrap())
}
pub fn disable(
    c: &Connection,
    user_id: i64,
    id: i64,
    version: i64,
) -> Result<VirtualModel, VirtualError> {
    let tx = c.unchecked_transaction()?;
    owner_auth(&tx, user_id)?;
    let n=tx.execute("UPDATE user_virtual_models SET enabled=0,version=version+1,updated_ms=?4 WHERE id=?1 AND user_id=?2 AND version=?3",params![id,user_id,version,super::now_ms()])?;
    if n != 1 {
        return Err(if list(&tx, user_id)?.iter().any(|r| r.id == id) {
            error("virtual_route_changed", "路由已修改，请刷新")
        } else {
            error("virtual_route_not_found", "虚拟模型不存在")
        });
    }
    audit(&tx, user_id, id, "virtual_model.disable", version + 1)?;
    let saved = list(&tx, user_id)?
        .into_iter()
        .find(|r| r.id == id)
        .unwrap();
    tx.commit()?;
    Ok(saved)
}
pub fn resolve(
    c: &Connection,
    user_id: i64,
    name: &str,
    now_ms: i64,
) -> Result<Option<ResolvedVirtualPlan>, VirtualError> {
    owner_auth(c, user_id)?;
    let Some(r) = list(c, user_id)?
        .into_iter()
        .find(|r| same_model_name(&r.config.name, name))
    else {
        return Ok(None);
    };
    if !r.config.enabled {
        return Err(error("virtual_route_forbidden", "虚拟模型已停用"));
    }
    if model_routes::list(c)?
        .iter()
        .any(|m| same_model_name(&m.name, &r.config.name))
    {
        return Err(error(
            "virtual_route_changed",
            "该名称已成为标准模型名，请更改私有模型名称",
        ));
    }
    let indices = time_positions(&r.config, now_ms)?;
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
        id: r.id,
        user_id,
        name: r.config.name,
        version: r.version,
        targets,
    }))
}
pub fn validate_current(
    c: &Connection,
    user_id: i64,
    id: i64,
    version: i64,
    position: usize,
    now_ms: i64,
) -> Result<(), VirtualError> {
    let auth = owner_auth(c, user_id)?;
    let r = list(c, user_id)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| error("virtual_route_forbidden", "路由不属于当前用户"))?;
    if r.version != version || !r.config.enabled {
        return Err(error("virtual_route_changed", "路由已修改或停用，请重试"));
    }
    if model_routes::list(c)?
        .iter()
        .any(|m| same_model_name(&m.name, &r.config.name))
    {
        return Err(error(
            "virtual_route_changed",
            "该名称已成为标准模型名，请更改私有模型名称",
        ));
    }
    if !time_positions(&r.config, now_ms)?.contains(&position) {
        return Err(error("route_time_unavailable", "该目标当前时段不再可用"));
    }
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
    Ok(())
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
    let standard = model_routes::list(c)?;
    let mut names = Vec::new();
    for r in list(c, fresh.user_id)? {
        if !r.config.enabled
            || standard
                .iter()
                .any(|m| same_model_name(&m.name, &r.config.name))
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
