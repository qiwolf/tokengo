//! Durable, fingerprint-scoped renewal. No client request may bypass compatibility or ACLs.
use crate::{
    SharedState,
    db::{self, user_virtual_routes as vr},
    vector_compat,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use tokengo_shared::ModelKind;
const RENEW_AFTER_MS: i64 = 18 * 60 * 60 * 1000;
const RETRY_MS: i64 = 5 * 60 * 1000;
const LEASE_MS: i64 = 90_000;
#[derive(Clone, Default, Serialize, Deserialize)]
struct Renewal {
    #[serde(default)]
    sources: Vec<vector_compat::SourceCheck>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    error_code: String,
    #[serde(default)]
    attempted_at_ms: i64,
    #[serde(default)]
    next_retry_at_ms: i64,
    #[serde(default)]
    lease_until_ms: i64,
    #[serde(default)]
    token: String,
    /// A failed voluntary workload upgrade must not revoke still-valid old evidence.
    #[serde(default)]
    profile_upgrade_error: String,
    #[serde(default)]
    profile_upgrade_message: String,
    #[serde(default)]
    profile_upgrade_at_ms: i64,
}
fn key(user: i64, fingerprint: &str) -> String {
    format!("internal_vector_renewal_v1:{user}:{fingerprint}")
}
fn read(c: &Connection, k: &str) -> Result<Renewal, vr::VirtualError> {
    Ok(match db::settings::get(c, k)? {
        None => Renewal::default(),
        Some(raw) => serde_json::from_str(&raw).unwrap_or_else(|_| Renewal {
            state: "failed".into(),
            message: "续验状态记录损坏，系统将自动重新校验".into(),
            error_code: "renewal_state_invalid".into(),
            ..Renewal::default()
        }),
    })
}
fn write(c: &Connection, k: &str, r: &Renewal) -> Result<(), vr::VirtualError> {
    db::settings::set(
        c,
        k,
        &serde_json::to_string(r).map_err(|_| fail("无法保存自动续验状态"))?,
    )?;
    Ok(())
}
fn fail(message: impl Into<String>) -> vr::VirtualError {
    vr::VirtualError {
        code: "vector_compatibility_renewal_failed",
        message: message.into(),
    }
}
/// Source failures revoke only that source; expired per-source evidence is never routed.
pub(crate) fn check_failure(c:&Connection,user:i64,fingerprint:&str)->Result<(),vr::VirtualError>{
    let r=read(c,&key(user,fingerprint))?;
    if r.state=="failed"{return Err(fail(format!("兼容续验失败：{}；暂无可安全核实的来源，系统将重试",r.message)));}
    if !r.sources.is_empty(){if !r.sources.iter().any(|s|s.usable(db::now_ms())){return Err(fail("当前没有已核实的健康来源；系统每5分钟自动重试，请检查来源状态"));}return Ok(());}
    if !r.error_code.is_empty()&&!matches!(r.error_code.as_str(),"vector_source_unavailable"|"vector_fallback_rate_limited"){return Err(fail(format!("兼容续验尚未恢复可用来源：{}；系统将自动重试",r.message)));}Ok(())
}
pub(crate) fn source_allowed(c:&Connection,user:i64,contract:&vector_compat::Contract,source:i64,model:&str)->Result<bool,vr::VirtualError>{
    let r=read(c,&key(user,vector_compat::contract_fingerprint(contract)))?;
    if r.state=="failed"{return Ok(false);}
    if r.sources.is_empty(){return Ok(r.error_code.is_empty()||matches!(r.error_code.as_str(),"vector_source_unavailable"|"vector_fallback_rate_limited"));}
    Ok(r.sources.iter().any(|s|s.source_id==source&&tokengo_shared::same_model_name(&s.logical_model,model)&&s.usable(db::now_ms())))
}
#[derive(Clone)]
pub(crate) struct RouteGuard {
    pub user: i64,
    pub id: i64,
    pub version: i64,
    pub allow_disabled: bool,
}
impl RouteGuard {
    pub(crate) fn check(&self, c: &Connection) -> Result<(), vr::VirtualError> {
        let valid = if self.user == 0 {
            db::shared_virtual_routes::get(c,self.id)?
                .is_some_and(|r| r.version == self.version && (self.allow_disabled || r.config.enabled))
        } else {
            vr::get(c,self.user,self.id)?
                .is_some_and(|r| r.version == self.version && (self.allow_disabled || r.config.enabled))
        };
        if !valid {
            return Err(fail("续验期间策略已停用或修改，旧检查结果不再发布"));
        }
        Ok(())
    }
}
/// A separate task ticket keeps repeated clicks from creating parallel checks of one version.
struct VerificationJob {
    state: SharedState,
    identity: (i64, i64, i64),
    token: String,
}
impl Drop for VerificationJob {
    fn drop(&mut self) {
        self.state.vector_verification_jobs.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.identity);
        let c = self.state.conn();
        let (user, id, _) = self.identity;
        if let Ok(Some(raw)) = db::settings::get(&c, &manual_key(user, id)) {
            if let Ok(mut row) = serde_json::from_str::<Value>(&raw) {
                if row["token"] == self.token && row["state"] == "checking" {
                    row["state"] = "failed".into();
                    row["error_code"] = "vector_verification_interrupted".into();
                    row["message"] = "独立验证任务已中断，请重新启动验证；配置不受影响".into();
                    row["finished_at_ms"] = db::now_ms().into();
                    let _ = db::settings::set(&c, &manual_key(user, id), &row.to_string());
                }
            }
        }
    }
}
fn manual_key(user: i64, id: i64) -> String { format!("internal_vector_manual_job_v1:{user}:{id}") }
/// Editor requests only schedule validation. Neither saving nor closing the page owns this task.
pub(crate) fn start_saved(
    state: &SharedState,
    user: i64,
    id: i64,
    version: i64,
    config: vr::Config,
    actor: Option<i64>,
) -> Result<Value, vr::VirtualError> {
    let guard = RouteGuard { user, id, version, allow_disabled: true };
    {
        let c = state.conn();
        guard.check(&c)?;
        if let Some(actor) = actor {
            db::shared_virtual_routes::root_actor(&c, actor)?;
            vr::preflight_shared(&c, &config)?;
        } else {
            vr::preflight(&c, user, &config)?;
        }
        let kind = config.targets.first().and_then(|t| db::model_routes::routes_for(&c, &t.logical_model).ok().flatten()).map(|r| r.kind).unwrap_or_default();
        if config.targets.len() < 2 || !matches!(kind, ModelKind::Embeddings | ModelKind::Rerank) {
            return Ok(serde_json::json!({"state":"not_required","message":"此策略无需向量或重排跨来源兼容验证"}));
        }
        // Surface unsupported source configurations immediately, without scheduling
        // a job whose first local snapshot would otherwise fail out of view.
        let fp = vector_compat::fingerprint(&c, user, &config.targets, kind)?;
        let renewal = read(&c, &key(user, &fp))?;
        if renewal.lease_until_ms > db::now_ms() {
            let same_job = state.vector_verification_jobs.lock().unwrap_or_else(|e| e.into_inner()).contains(&(user,id,version));
            if same_job {
                return Ok(saved_status(state, &c, user, id, version, &config));
            }
            return Err(vr::VirtualError {
                code: "vector_compatibility_renewing",
                message: "相同来源的另一项检查仍在执行，本次未创建验证任务；请稍后重新启动验证".into(),
            });
        }
    }
    let identity = (user, id, version);
    let inserted = state.vector_verification_jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(identity);
    if inserted {
        let state = state.clone();
        let token = uuid::Uuid::new_v4().to_string();
        let job = VerificationJob { state: state.clone(), identity, token: token.clone() };
        {
            let c = state.conn();
            db::settings::set(&c, &manual_key(user, id), &json!({"version":version,"token":token,
                "state":"checking","message":"独立兼容验证已排入后台","started_at_ms":db::now_ms()}).to_string())?;
        }
        tokio::spawn(async move {
            let _job = job;
            let result = verify(&state, user, &config, actor, Some(guard), false).await;
            let (phase, message, code) = match result {
                Ok(()) => ("valid", "独立兼容验证已完成".to_owned(), ""),
                Err(e) => {
                    tracing::warn!(scope=user,model=%config.name,error_code=e.code,"独立兼容验证未完成；结果可在调用策略查看");
                    ("failed", e.message, e.code)
                },
            };
            let c = state.conn();
            if let Ok(Some(raw)) = db::settings::get(&c, &manual_key(user, id)) {
                if let Ok(mut row) = serde_json::from_str::<Value>(&raw) {
                    if row["token"] == token {
                        row["state"] = phase.into();
                        row["message"] = message.into();
                        row["error_code"] = code.into();
                        row["finished_at_ms"] = db::now_ms().into();
                        if let Err(e) = db::settings::set(&c, &manual_key(user, id), &row.to_string()) {
                            tracing::warn!(scope=user,route=id,error=%e,"无法持久化独立验证结果");
                        }
                    }
                }
            }
        });
    }
    Ok(serde_json::json!({"state":"checking","message":"兼容验证已在后台进行，可以继续编辑或离开页面"}))
}
/// Expose a queued task immediately, before it has acquired its durable source lease.
pub(crate) fn saved_status(state: &SharedState, c: &Connection, user: i64, id: i64, version: i64, config: &vr::Config) -> Value {
    let mut value = status(c, user, config);
    let active = state.vector_verification_jobs.lock().unwrap_or_else(|e| e.into_inner()).contains(&(user,id,version));
    if let Ok(Some(raw)) = db::settings::get(c, &manual_key(user, id)) {
        if let Ok(mut row) = serde_json::from_str::<Value>(&raw) {
            if row["version"] == version {
                if let Some(fields) = row.as_object_mut() { fields.remove("token"); }
                if row["state"] == "failed" {
                    let previous = value["message"].as_str().unwrap_or("");
                    value["message"] = format!("独立验证未完成：{}；{}", row["message"].as_str().unwrap_or("请重新验证"), previous).into();
                    if value["state"] != "valid" && value["state"] != "degraded" && value["state"] != "disabled" {
                        value["state"] = "failed".into();
                        value["error_code"] = row["error_code"].clone();
                    }
                } else if row["state"] == "checking" && !active && value["state"] != "checking" {
                    row["state"] = "interrupted".into();
                    row["message"] = "上次独立验证未完成，请重新启动验证".into();
                    value["message"] = row["message"].clone();
                }
                value["manual_verification"] = row;
            }
        }
    }
    if active {
        value["state"] = "checking".into();
        value["message"] = "兼容验证在后台进行，可以继续编辑或离开页面".into();
    }
    value
}
/// Restore only our unfinished lease when a task is cancelled or superseded by an edit.
struct RenewalLease {
    state: SharedState,
    key: String,
    token: String,
    previous: Renewal,
}
impl Drop for RenewalLease {
    fn drop(&mut self) {
        let c = self.state.conn();
        if let Ok(current) = read(&c, &self.key) {
            if current.token == self.token && current.state == "checking" && current.lease_until_ms > 0 {
                let mut previous = self.previous.clone();
                previous.lease_until_ms = 0;
                let _ = write(&c, &self.key, &previous);
            }
        }
    }
}
/// Shared by manual saves and background scans, with a durable lease across restarts.
pub(crate) async fn verify(
    state: &SharedState,
    user: i64,
    config: &vr::Config,
    actor: Option<i64>,
    guard: Option<RouteGuard>,
    automatic: bool,
) -> Result<(), vr::VirtualError> {
    if config.targets.len() < 2 {
        return Ok(());
    }
    let (fingerprint, kind, full_verification, previous_valid) = {
        let c = state.conn();
        let kind = db::model_routes::routes_for(&c, &config.targets[0].logical_model)?
            .map(|r| r.kind)
            .unwrap_or_default();
        if !matches!(kind, ModelKind::Embeddings | ModelKind::Rerank) {
            return Ok(());
        }
        if let Some(actor) = actor {
            db::shared_virtual_routes::root_actor(&c, actor)?;
            vr::preflight_shared(&c, config)?;
        } else {
            vr::preflight(&c, user, config)?;
        }
        if let Some(g) = &guard {
            g.check(&c)?;
        }
        // First-time heavy certification is an explicit action after saving. The
        // scheduler renews existing evidence but never starts a load test for a new edit.
        if automatic && !vector_compat::has_history(&c, user, &config.targets, kind) {
            return Ok(());
        }
        let fingerprint = vector_compat::fingerprint(&c, user, &config.targets, kind)?;
        if automatic && matches!(vector_compat::has_reference(&c, user, &fingerprint), Ok(false)) {
            return Ok(());
        }
        let current = vector_compat::require(&c, user, &config.targets, kind);
        let previous_valid = current.is_ok();
        // An explicit compatibility check may upgrade an old workload profile. Background
        // renewal keeps using the compact immutable reference and never does a load test.
        let full_verification = !automatic
            && current.as_ref().map_or(true, |contract| !vector_compat::current_profile(contract));
        if let Ok(contract) = current {
            let renewal = read(&c, &key(user, &fingerprint))?;
            if !full_verification
                && (renewal.state.is_empty() || renewal.state == "valid")
                && renewal.sources.iter().all(|source| source.usable(db::now_ms()))
                && vector_compat::has_reference(&c, user, &fingerprint).unwrap_or(false)
                && (!automatic || db::now_ms().saturating_sub(contract.verified_at_ms) < RENEW_AFTER_MS)
            {
                return Ok(());
            }
        }
        (fingerprint, kind, full_verification, previous_valid)
    };
    let lease_ms = if full_verification {
        vector_compat::full_verify_budget_ms(&state.conn(), user, &config.targets, kind)?
            .saturating_add(30_000).min(i64::MAX as u64) as i64
    } else {
        LEASE_MS
    };
    let k = key(user, &fingerprint);
    let token = uuid::Uuid::new_v4().to_string();
    let previous={
        let c = state.conn();
        let tx = c.unchecked_transaction()?;
        let mut r = read(&tx, &k)?;
        let now = db::now_ms();
        if r.lease_until_ms > now || (automatic && r.next_retry_at_ms > now) {
            if automatic {
                return Ok(());
            }
            return Err(if r.state == "failed" {
                fail(format!(
                    "兼容自动续验失败：{}；系统将在重试时间到达后自动重试",
                    r.message
                ))
            } else {
                vr::VirtualError {
                    code: "vector_compatibility_renewing",
                    message: "兼容自动续验正在进行，请稍后重试，无需重新保存策略".into(),
                }
            });
        }
        let previous = r.clone();
        r.state = "checking".into();
        r.token = token.clone();
        r.attempted_at_ms = now;
        r.lease_until_ms = now.saturating_add(lease_ms);
        r.next_retry_at_ms = 0;
        write(&tx, &k, &r)?;
        tx.commit()?;
        previous
    };
    let _lease = RenewalLease { state: state.clone(), key: k.clone(), token: token.clone(), previous: previous.clone() };
    let has_history={let c=state.conn();vector_compat::has_history(&c,user,&config.targets,kind)};
    let has_reference={let c=state.conn();vector_compat::has_reference(&c,user,&fingerprint)};
    let partial=if !full_verification&&has_history&&matches!(has_reference,Ok(true)){Some(vector_compat::renew_sources(state,user,config,actor,guard.as_ref(),&previous.sources).await)}else{None};
    let result=if let Err(e)=has_reference{Err(e)}else if let Some(result)=&partial{result.as_ref().map(|_|()).map_err(|e|vr::VirtualError{code:e.code,message:e.message.clone()})}else if automatic {Err(vr::VirtualError{code:"vector_fallback_unverified",message:"缺少数值参考，请独立启动兼容验证；自动续验不会执行完整负载检查".into()})}else{vector_compat::verify_impl(state,user,config,actor,true,guard.as_ref()).await};
    {
        let c=state.conn();let tx=c.unchecked_transaction()?;let mut r=read(&tx,&k)?;
        if r.token!=token{return Err(fail("自动续验租约已变化，旧结果不再生效"));}
        if let Some(g)=&guard{g.check(&tx)?;}if let Some(actor)=actor{db::shared_virtual_routes::root_actor(&tx,actor)?;}
        if vector_compat::fingerprint(&tx,user,&config.targets,kind)?!=fingerprint{return Err(fail("续验期间来源身份或权限已变化"));}
        r.lease_until_ms=0;
        match (&result,&partial) {
            (Ok(()),Some(Ok(outcome)))=>{
                let usable=outcome.sources.iter().filter(|s|s.usable(db::now_ms())).count();
                r.sources=outcome.sources.clone();
                if usable>0 {outcome.publish(&tx,user)?;r.state=if usable==r.sources.len(){"valid"}else{"degraded"}.into();r.message=format!("{usable}/{} 个来源已通过续验；不可用或不兼容来源已独立隔离",r.sources.len());r.error_code.clear();r.next_retry_at_ms=if usable==r.sources.len(){0}else{db::now_ms()+RETRY_MS};}
                else {r.state="failed".into();r.message="所有来源均未通过续验，暂无健康来源；系统将自动重试".into();r.error_code="vector_sources_unavailable".into();r.next_retry_at_ms=db::now_ms()+RETRY_MS;}
            },
            (Ok(()),_)=>{r.sources.clear();r.state="valid".into();r.message="兼容校验通过，系统自动续验".into();r.error_code.clear();r.next_retry_at_ms=0;r.profile_upgrade_error.clear();r.profile_upgrade_message.clear();r.profile_upgrade_at_ms=0;},
            (Err(e),_) if full_verification && previous_valid
                && vector_compat::require(&tx,user,&config.targets,kind).is_ok() => {
                // Identity was checked immediately above; require also rechecks TTL and
                // individual source health. A failed new profile is not old-profile revocation.
                r = previous.clone();
                r.profile_upgrade_error = e.code.into();
                r.profile_upgrade_message = e.message.clone();
                r.profile_upgrade_at_ms = db::now_ms();
            },
            (Err(e),_)=>{let transient=has_history&&matches!(e.code,"vector_source_unavailable"|"vector_fallback_rate_limited");if !transient{r.sources.clear();}r.state=if transient{"degraded"}else{"failed"}.into();r.message=e.message.clone();r.error_code=e.code.into();r.next_retry_at_ms=db::now_ms()+RETRY_MS;},
        }
        write(&tx,&k,&r)?;tx.commit()?;
    }
    if let Err(e) = &result {
        tracing::warn!(scope=user,model=%config.name,kind=?kind,error_code=e.code,"兼容检查未完成；逐来源服务状态和负载升级失败原因已记录");
    }
    if !automatic {if let Some(Ok(outcome))=&partial {if !outcome.sources.iter().all(|s|s.usable(db::now_ms())){return Err(vr::VirtualError{code:"vector_source_unavailable",message:"部分来源尚未通过兼容检查；现有健康来源继续服务，请修复来源后再单独验证".into()});}}}
    result
}
/// Visible to the owning user/root. Does not expose URLs, credentials or provider response bodies.
pub fn status(c: &Connection, user: i64, config: &vr::Config) -> Value {
    if !config.enabled {
        return json!({"state":"disabled","message":"策略已停用，不执行自动续验"});
    }
    let kind = config
        .targets
        .first()
        .and_then(|t| {
            db::model_routes::routes_for(c, &t.logical_model)
                .ok()
                .flatten()
        })
        .map(|r| r.kind)
        .unwrap_or_default();
    if config.targets.len() < 2 || !matches!(kind, ModelKind::Embeddings | ModelKind::Rerank) {
        return json!({"state":"not_required","message":"此策略无需向量或重排兼容续验"});
    }
    let result = (|| -> Result<Value, vr::VirtualError> {
        if user == 0 {
            if let Some(route) = db::shared_virtual_routes::list(c)?
                .into_iter()
                .find(|r| tokengo_shared::same_model_name(&r.config.name, &config.name))
            {
                db::shared_virtual_routes::root_actor(c, route.actor_account_id)?;
            }
        }
        let fp = vector_compat::fingerprint(c, user, &config.targets, kind)?;
        let r = read(c, &key(user, &fp))?;
        let raw = db::settings::get(c, &format!("internal_vector_compat_v1:{user}:{fp}"))?
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let verified = raw.as_ref().and_then(|v| v["verified_at_ms"].as_i64());
        let current = vector_compat::require(c, user, &config.targets, kind);
        let renewable = vector_compat::has_history(c, user, &config.targets, kind)
            && vector_compat::has_reference(c, user, &fp).unwrap_or(false);
        let now = db::now_ms();
        let available=r.sources.iter().filter(|s|s.usable(now)).count();
        let (phase, mut message) = if !r.sources.is_empty()&&available>0&&available<r.sources.len(){("degraded",format!("部分来源已隔离：{}；仍有 {available}/{} 个健康来源可用，系统5分钟后重试",r.sources.iter().filter(|s|!s.usable(now)).map(|s|format!("来源{}：{}",s.source_id,s.message)).collect::<Vec<_>>().join("；"),r.sources.len()))}else if r.state=="degraded" {("degraded",format!("续验未完成：{}；保留仍有效且身份未变的兼容证据继续服务，系统5分钟后重试",r.message))}else if r.state == "failed" {
            (
                "failed",
                if renewable {
                    format!("自动续验失败：{}；暂无已核实健康来源，系统每5分钟自动重试", r.message)
                } else {
                    format!("兼容验证失败：{}；配置已保存，请修复来源后重新启动兼容验证", r.message)
                },
            )
        } else if r.lease_until_ms > now {
            (
                "checking",
                if r.error_code.is_empty() {
                    "兼容自动续验中，无需手动保存策略".into()
                } else {
                    format!("兼容自动重试中；上次失败：{}；多目标调用仍暂停", r.message)
                },
            )
        } else if vector_compat::has_history(c, user, &config.targets, kind) && !vector_compat::has_reference(c, user, &fp).unwrap_or(false) {
            (if current.is_ok() {"valid"} else {"pending"}, "缺少当前来源的数值参考，自动续验已跳过；请独立启动兼容验证，保存不会执行完整负载检查".into())
        } else if current.is_ok() {
            ("valid", "兼容校验通过，系统将在到期前自动续验".into())
        } else {
            (
                "pending",
                "配置已保存。尚无当前来源的兼容证据，请单独验证目标并启动兼容验证；验证通过前多目标调用暂不可用".into(),
            )
        };
        if !renewable && r.lease_until_ms <= now && matches!(phase, "degraded") {
            message = "配置已保存，但缺少当前来源的完整数值参考，自动续验已跳过；请单独启动兼容验证".into();
        }
        if !r.profile_upgrade_error.is_empty() {
            message.push_str(&format!("；新负载范围校验未通过：{}；仍按原有有效证据和健康来源服务，未扩大已验证范围", r.profile_upgrade_message));
        }
        Ok(
            json!({"state":phase,"message":message,"verified_at_ms":verified,"expires_at_ms":verified.map(|v|v+vector_compat::TTL_MS),"attempted_at_ms":r.attempted_at_ms,"next_retry_at_ms":r.next_retry_at_ms,"lease_until_ms":r.lease_until_ms,"error_code":r.error_code,"sources":r.sources,"available_sources":available,"profile_upgrade_error":r.profile_upgrade_error,"profile_upgrade_message":r.profile_upgrade_message,"profile_upgrade_at_ms":r.profile_upgrade_at_ms}),
        )
    })();
    result.unwrap_or_else(|e|json!({"state":"failed","message":format!("无法自动续验：{}；请检查策略权限与来源配置，系统会持续重新检查",e.message),"error_code":e.code}))
}
/// Run once at startup, then each minute. Only enabled saved routes authorize recurring probes.
pub async fn tick(state: &SharedState) -> Result<(), vr::VirtualError> {
    let jobs = {
        let c = state.conn();
        let mut jobs = Vec::new();
        for r in db::shared_virtual_routes::list(&c)? {
            if r.config.enabled {
                jobs.push((
                    0,
                    r.config,
                    Some(r.actor_account_id),
                    RouteGuard {
                        user: 0,
                        id: r.id,
                        version: r.version,
                        allow_disabled: false,
                    },
                ));
            }
        }
        let users = {
            let mut q =
                c.prepare("SELECT DISTINCT user_id FROM user_virtual_models WHERE enabled=1")?;
            let rows = q
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for user in users {
            for r in vr::list(&c, user)? {
                if r.config.enabled {
                    jobs.push((
                        user,
                        r.config,
                        None,
                        RouteGuard {
                            user,
                            id: r.id,
                            version: r.version,
                            allow_disabled: false,
                        },
                    ));
                }
            }
        }
        jobs
    };
    for (user, config, actor, guard) in jobs {
        if let Err(e) = verify(state, user, &config, actor, Some(guard), true).await {
            tracing::warn!(scope=user,model=%config.name,error_code=e.code,"兼容续验未完成；状态已可在调用策略查看");
        }
    }
    Ok(())
}
pub fn spawn(state: SharedState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if let Err(e) = tick(&state).await {
                tracing::warn!(error_code = e.code, "兼容自动续验扫描失败，将自动重试");
            }
        }
    });
}
