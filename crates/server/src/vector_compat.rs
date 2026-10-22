//! Server-issued, expiring compatibility evidence for bounded embedding/rerank fallback.
//! Synthetic checks establish a small request domain, not universal model equivalence.
use crate::{
    db::{self, user_virtual_routes as vr},
    state::SharedState,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, time::Duration};
use tokengo_shared::{Channel, ModelKind};

pub(crate) const TTL_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_CHARS: usize = 512;
const MAX_BATCH: usize = 16;
const MAX_DOCS: usize = 128;
const MAX_TOTAL_CHARS: usize = 8192;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const VERSION: u32 = 1;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub dimensions: Option<usize>,
    pub unit_normalized: bool,
    pub max_input_chars: usize,
    pub max_batch: usize,
    pub max_documents: usize,
    pub max_total_chars: usize,
    pub evidence: String,
    fingerprint: String,
    pub verified_at_ms: i64,
    version: u32,
}
struct Source {
    channel: Channel,
    model: String,
}
struct Snapshot {
    fingerprint: String,
    sources: Vec<Source>,
}
fn error(code: &'static str, message: impl Into<String>) -> vr::VirtualError {
    vr::VirtualError {
        code,
        message: message.into(),
    }
}
fn unverified(message: impl Into<String>) -> vr::VirtualError {
    error("vector_fallback_unverified", message)
}
fn incompatible(message: impl Into<String>) -> vr::VirtualError {
    error("vector_fallback_incompatible", message)
}
fn supported(kind: ModelKind) -> bool {
    matches!(kind, ModelKind::Embeddings | ModelKind::Rerank)
}
fn snapshot(
    c: &Connection,
    user: i64,
    targets: &[vr::Target],
    kind: ModelKind,
) -> Result<Snapshot, vr::VirtualError> {
    if targets.len() < 2 || !supported(kind) {
        return Err(unverified("兼容合同仅用于向量或重排多目标调用链"));
    }
    let auth = vr::owner_auth(c, user)?;
    let mut facts = Vec::new();
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    for target in targets {
        let route = db::model_routes::routes_for(c, &target.logical_model)?
            .ok_or_else(|| unverified("目标模型已不存在，请重新保存调用策略"))?;
        if route.kind != kind {
            return Err(incompatible("调用链模型用途不一致或尚未确认，不能自动回退"));
        }
        let ids = if user == 0 {
            vr::allowed_sources_shared(c, &auth, target)?
        } else {
            vr::allowed_sources(c, &auth, target)?
        };
        if ids.is_empty() {
            return Err(unverified("目标无已授权可用来源，请检查渠道权限"));
        }
        let mut target_facts = Vec::new();
        for channel in db::model_routes::channels_for(c, &target.logical_model)? {
            if !ids.contains(&channel.id.0) {
                continue;
            }
            if !crate::protocol_config::background_probe_allowed(c, channel.id.0)? {
                return Err(incompatible("此来源不支持受控向量或重排兼容检查"));
            }
            if channel.drop_fields.iter().any(|field| {
                [
                    "model",
                    "input",
                    "encoding_format",
                    "dimensions",
                    "query",
                    "documents",
                    "top_n",
                    "return_documents",
                ]
                .contains(&field.as_str())
            }) {
                return Err(incompatible(
                    "来源配置会删除兼容检查所需参数，请先修正渠道字段过滤配置",
                ));
            }
            let credential = db::provider_pool::source_credential(c, channel.id.0)?;
            let upstream = channel.upstream_model(&target.logical_model).to_owned();
            target_facts.push(json!({"id":channel.id.0,"identity":crate::reasoning_capabilities::fingerprint(c,channel.id.0)?,"upstream":upstream,"drop_fields":channel.drop_fields,"unwrap":channel.unwrap_envelope,"credential":credential}));
            if seen.insert((channel.id.0, target.logical_model.clone())) {
                sources.push(Source {
                    channel,
                    model: target.logical_model.clone(),
                });
            }
        }
        facts.push(json!({"target":target,"sources":target_facts}));
    }
    if sources.is_empty() || sources.len() > 32 {
        return Err(unverified("兼容检查要求1至32个已授权来源"));
    }
    let bytes = serde_json::to_vec(
        &json!({"version":VERSION,"kind":format!("{kind:?}"),"user":user,"targets":facts}),
    )
    .map_err(|_| unverified("无法生成兼容检查指纹"))?;
    Ok(Snapshot {
        fingerprint: hex::encode(Sha256::digest(bytes)),
        sources,
    })
}
pub(crate) fn fingerprint(c: &Connection, user:i64, targets:&[vr::Target], kind:ModelKind)->Result<String,vr::VirtualError>{Ok(snapshot(c,user,targets,kind)?.fingerprint)}
fn key(user: i64, fingerprint: &str) -> String {
    format!("internal_vector_compat_v1:{user}:{fingerprint}")
}
/// Recomputes the complete source set. Changed, added or removed sources invalidate evidence.
pub fn require(
    c: &Connection,
    user: i64,
    targets: &[vr::Target],
    kind: ModelKind,
) -> Result<Contract, vr::VirtualError> {
    crate::vector_renewal::check_failure(c,user,&fingerprint(c,user,targets,kind)?)?;
    let contract=historical(c,user,targets,kind)?;
    let age=db::now_ms().saturating_sub(contract.verified_at_ms);
    if age<0||age>TTL_MS {return Err(unverified("兼容证据已过期，系统将逐来源自动续验，请稍后重试"));}
    Ok(contract)
}
/// Exact current fingerprint and contract shape are mandatory even for renewal of old evidence.
fn historical(c:&Connection,user:i64,targets:&[vr::Target],kind:ModelKind)->Result<Contract,vr::VirtualError>{
    let snapshot=snapshot(c,user,targets,kind)?;
    let raw=db::settings::get(c,&key(user,&snapshot.fingerprint))?.ok_or_else(||unverified("尚无此来源身份的兼容证据，不能套用旧模型证据"))?;
    let contract:Contract=serde_json::from_str(&raw).map_err(|_|unverified("兼容记录无效，系统将重新校验"))?;
    if contract.version!=VERSION||contract.fingerprint!=snapshot.fingerprint||contract.max_input_chars!=MAX_CHARS||contract.max_batch!=MAX_BATCH||contract.max_documents!=MAX_DOCS||contract.max_total_chars!=MAX_TOTAL_CHARS||(kind==ModelKind::Embeddings&&contract.dimensions.is_none_or(|d|d==0||d>65536)){return Err(unverified("兼容合同版本、来源身份或边界已改变，不能继承旧证据"));}
    Ok(contract)
}

fn samples() -> Vec<String> {
    vec![
        "医院知识检索与安全用药".into(),
        "A quiet river flows beside the hospital.".into(),
        "向量模型兼容检查，医疗资料检索。"
            .chars()
            .cycle()
            .take(MAX_CHARS)
            .collect(),
        "The medical library stores reliable evidence. "
            .chars()
            .cycle()
            .take(MAX_CHARS)
            .collect(),
    ]
}
fn rerank_docs() -> Vec<String> {
    vec![
        "The capital of France is Paris. 法国首都是巴黎。".into(),
        "Cats are small animals. 猫是小型动物。".into(),
        "医院知识检索与用药安全资料。"
            .chars()
            .cycle()
            .take(MAX_CHARS)
            .collect(),
        "A library contains books and reliable evidence. "
            .chars()
            .cycle()
            .take(MAX_CHARS)
            .collect(),
    ]
}
struct ProbeContext<'a> {
    user: i64,
    actor: Option<i64>,
    targets: &'a [vr::Target],
    kind: ModelKind,
    fingerprint: &'a str,
    guard: Option<&'a crate::vector_renewal::RouteGuard>,
}
struct ProbeBilling {
    state: SharedState,
    ledger: Option<i64>,
    sent: bool,
    system: bool,
}
impl ProbeBilling {
    fn settle(
        &mut self,
        input: Option<i64>,
        output: Option<i64>,
        outcome: &str,
    ) -> Result<(), vr::VirtualError> {
        if let Some(id) = self.ledger.take() {
            let settled = if self.system {
                crate::billing_writer::settle_diagnostic(
                    &self.state,
                    id,
                    input,
                    output,
                    None,
                    false,
                    outcome,
                )
            } else {
                crate::billing_writer::settle(&self.state, id, input, output, outcome)
            };
            settled.map_err(|_| unverified("兼容检查费用结算待恢复，请检查账本后重试"))?;
        }
        Ok(())
    }
}
impl Drop for ProbeBilling {
    fn drop(&mut self) {
        if self.ledger.is_some() {
            if self.system {
                let outcome = if self.sent {
                    "compatibility_cancelled_unknown"
                } else {
                    "not_sent"
                };
                let _ = self.settle(None, None, outcome);
            } else if self.sent {
                let _ = self.settle(None, None, "compatibility_cancelled_unknown");
            } else if let Some(id) = self.ledger.take() {
                let _ = db::billing::release(&self.state.conn(), id, "compatibility_not_sent");
            }
        }
    }
}

fn input_bound(path: &str, body: &Value) -> Result<i64, vr::VirtualError> {
    let bytes = if path == "rerank" {
        let query = body["query"].as_str().map_or(0, str::len);
        let docs = body["documents"]
            .as_array()
            .ok_or_else(|| unverified("重排诊断输入无效"))?;
        query
            .checked_mul(docs.len().saturating_sub(1))
            .and_then(|extra| body.to_string().len().checked_add(extra))
    } else {
        Some(body.to_string().len())
    };
    bytes
        .and_then(|n| n.checked_add(256))
        .and_then(|n| i64::try_from(n).ok())
        .ok_or_else(|| unverified("兼容检查请求费用边界溢出"))
}
type SourceCapacity = (
    Option<crate::state::ChannelPermit>,
    Option<crate::state::ChannelPermit>,
);
fn acquire_source_capacity(
    state: &SharedState,
    source: i64,
    limit: u32,
    credential: Option<&db::provider_pool::CredentialRef>,
) -> Result<SourceCapacity, vr::VirtualError> {
    let busy = || {
        error(
            "vector_fallback_rate_limited",
            "兼容检查来源或上游凭据已达到并发上限，请稍后重试",
        )
    };
    // Match normal gateway lock order: physical source first, shared credential second.
    let source_permit = state
        .channel_gate(source, limit)
        .map(|gate| gate.try_acquire_owned().map_err(|_| busy()))
        .transpose()?;
    let credential_permit = credential
        .map(|credential| {
            state
                .credential_gate(credential.id, credential.max_concurrent)
                .map(|gate| gate.try_acquire_owned().map_err(|_| busy()))
                .transpose()
        })
        .transpose()?
        .flatten();
    Ok((source_permit, credential_permit))
}
async fn request(
    state: &SharedState,
    context: &ProbeContext<'_>,
    source: &Source,
    path: &str,
    mut body: Value,
) -> Result<Value, vr::VirtualError> {
    body["model"] = json!(source.channel.upstream_model(&source.model));
    let bound = input_bound(path, &body)?;
    let _permit = if context.user == 0 {
        None
    } else {
        Some(
            crate::admission::acquire_user(state.clone(), context.user, bound as u64, 0).map_err(
                |e| {
                    if e.status.as_u16() == 429 {
                        error(
                            "vector_fallback_rate_limited",
                            "兼容检查受到当前用户请求频率、Token额度或并发限制，请稍后重试",
                        )
                    } else {
                        error(
                            "virtual_target_forbidden",
                            "兼容检查用户已停用或配额状态不可读取",
                        )
                    }
                },
            )?,
        )
    };
    let (mut channel, secret, ledger, _capacity) = {
        let c = state.conn();
        if let Some(guard)=context.guard{guard.check(&c)?;}
        if let Some(actor) = context.actor {
            db::shared_virtual_routes::root_actor(&c, actor)?;
        }
        if snapshot(&c, context.user, context.targets, context.kind)?.fingerprint
            != context.fingerprint
        {
            return Err(unverified("兼容检查期间来源配置或权限已变化，请重试"));
        }
        let mut channel = source.channel.clone();
        channel.max_concurrent = db::channels::get(&c, channel.id)?
            .ok_or_else(|| unverified("兼容检查来源已删除，请刷新后重试"))?
            .max_concurrent;
        let credential = db::provider_pool::source_credential(&c, channel.id.0)?;
        let capacity = acquire_source_capacity(
            state,
            channel.id.0,
            channel.max_concurrent,
            credential.as_ref(),
        )?;
        channel.headers = db::channels::headers(&c, &state.secrets, channel.id)?;
        let secret = db::provider_pool::load_source_secret(&c, &state.secrets, channel.id.0)?;
        let source_snapshot = json!({"kind":if context.user==0{"shared_compatibility"}else{"user_compatibility"},"source_id":channel.id.0,"logical_model":source.model,"upstream_model":channel.upstream_model(&source.model),"source_identity":crate::reasoning_capabilities::fingerprint(&c,channel.id.0)?,"credential":credential,"compatibility_fingerprint":context.fingerprint});
        let ledger=if context.user==0 {
          let auth=json!({"kind":"admin_diagnostic","subscription":false,"actor_account_id":context.actor,"purpose":"shared_model_compatibility"});
          db::billing::reserve_diagnostic(&c,&format!("shared-compatibility:{}",uuid::Uuid::new_v4()),channel.id.0,&source.model,false,false,&auth,&source_snapshot)
        }else{db::billing::reserve_user_diagnostic(&c,context.user,&format!("compatibility:{}",uuid::Uuid::new_v4()),channel.id.0,&source.model,bound,&source_snapshot)}
        .map_err(|_| unverified("兼容检查无法预留费用，请检查用户余额、冻结状态及来源计价上限"))?;
        (channel, secret, ledger, capacity)
    };
    let mut billing = ProbeBilling {
        state: state.clone(),
        ledger: Some(ledger),
        sent: false,
        system: context.user == 0,
    };
    if let Some(object) = body.as_object_mut() {
        for field in &channel.drop_fields {
            object.remove(field);
        }
    }
    billing.sent = true;
    // Configured source authentication; no redirects, unbounded response, or user-supplied URL.
    let (status, bytes) = state
        .upstream
        .custom_probe_request(
            &channel,
            secret.as_deref(),
            path,
            bytes::Bytes::from(body.to_string()),
            Duration::from_secs(15),
            MAX_BYTES,
        )
        .await
        .map_err(|_| {
            error("vector_source_unavailable",format!(
                "来源 {} 的兼容检查失败、超时或响应过大",
                channel.id.0
            ))
        })?;
    let mut value: Value = match serde_json::from_slice(&bytes) {
        Ok(value)=>value,
        Err(_) if !(200..300).contains(&status)=>{billing.settle(None,None,"compatibility_upstream_error")?;return Err(error("vector_source_unavailable",format!("来源 {} 的兼容检查返回 HTTP {status}",channel.id.0)));},
        Err(_)=>return Err(incompatible("兼容检查响应不是有效 JSON")),
    };
    if channel.unwrap_envelope {
        tokengo_shared::unwrap_envelope(&mut value);
    }
    let usage = crate::gateway::Usage::read(&value);
    let input = usage.input.or_else(|| {
        if path == "rerank" {
            value
                .get("usage")
                .and_then(|v| v.get("total_tokens"))
                .and_then(Value::as_i64)
                .filter(|n| *n >= 0)
        } else {
            None
        }
    });
    billing.settle(
        input,
        usage.output.or_else(|| input.map(|_| 0)),
        if (200..300).contains(&status) {
            "compatibility_response"
        } else {
            "compatibility_upstream_error"
        },
    )?;
    if !(200..300).contains(&status) {
        return Err(error("vector_source_unavailable",format!(
            "来源 {} 的兼容检查返回 HTTP {status}",
            channel.id.0
        )));
    }
    channel.headers.clear();
    Ok(value)
}
/// Synthetic probes consume user quotas and budget with a separate diagnostic ledger identity.
pub async fn verify(
    state: &SharedState,
    user: i64,
    config: &vr::Config,
) -> Result<(), vr::VirtualError> {
    if user <= 0 {
        return Err(error("virtual_route_forbidden", "私有兼容检查要求真实用户"));
    }
    {
        let c = state.conn();
        vr::preflight(&c, user, config)?;
    }
    crate::vector_renewal::verify(state, user, config, None, None, false).await
}
/// Only authenticated root-managed shared configuration enters the system diagnostic scope.
pub async fn verify_shared(
    state: &SharedState,
    config: &vr::Config,
    actor: i64,
) -> Result<(), vr::VirtualError> {
    {
        let c = state.conn();
        db::shared_virtual_routes::root_actor(&c, actor)?;
        vr::preflight_shared(&c, config)?;
    }
    crate::vector_renewal::verify(state, 0, config, Some(actor), None, false).await
}
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {vectors:Option<Vec<Vec<f64>>>,single:Option<Vec<Vec<f64>>>,rank:Option<Vec<Vec<(usize,f64)>>>,unit_normalized:bool}
fn reference_key(user:i64,fp:&str)->String{format!("internal_vector_reference_v1:{user}:{fp}")}
fn compare_reference(reference:&Reference,current:&Reference,kind:ModelKind)->Result<(),vr::VirtualError>{
    if kind==ModelKind::Embeddings {
        compare_vectors(reference.vectors.as_ref().ok_or_else(||unverified("向量参考缺失"))?,current.vectors.as_ref().ok_or_else(||incompatible("向量输出缺失"))?).map_err(incompatible)?;
        compare_vectors(reference.single.as_ref().ok_or_else(||unverified("单条向量参考缺失"))?,current.single.as_ref().ok_or_else(||incompatible("单条向量缺失"))?).map_err(incompatible)?;
    }else{
        let a=reference.rank.as_ref().ok_or_else(||unverified("重排参考缺失"))?;let b=current.rank.as_ref().ok_or_else(||incompatible("重排输出缺失"))?;
        if a.len()!=b.len()||a.iter().zip(b).any(|(a,b)|a.len()!=b.len()||a.iter().zip(b).any(|((i,x),(j,y))|i!=j||(x-y).abs()>0.02)){return Err(incompatible("重排来源的排序或评分不兼容，已隔离此来源"));}
    }
    Ok(())
}
async fn observe(state:&SharedState,context:&ProbeContext<'_>,source:&Source,contract:&Contract)->Result<Reference,vr::VirtualError>{
    if context.kind==ModelKind::Embeddings {
        let short=samples();let batch=(0..MAX_BATCH).map(|i|short[i%short.len()].chars().cycle().take(MAX_CHARS).collect::<String>()).collect::<Vec<_>>();let body=json!({"input":batch,"encoding_format":"float"});
        let output=request(state,context,source,"embeddings",body.clone()).await?;validate_output(contract,context.kind,&body,&output).map_err(incompatible)?;
        let vectors=embedding_vectors(&output,MAX_BATCH,contract.dimensions).map_err(incompatible)?;
        let body=json!({"input":short[0]});let output=request(state,context,source,"embeddings",body.clone()).await?;validate_output(contract,context.kind,&body,&output).map_err(incompatible)?;
        let single=embedding_vectors(&output,1,Some(vectors[0].len())).map_err(incompatible)?;
        let unit_normalized=vectors.iter().chain(single.iter()).all(|v|unit_norm(v));
        Ok(Reference{vectors:Some(vectors),single:Some(single),rank:None,unit_normalized})
    }else{
        let docs=rerank_docs();let mut source_ranks=Vec::new();
        for (query,top_n,return_docs) in [("What is the capital of France? 法国首都是哪里？".to_owned(),4,false),("医院资料与用药安全".chars().cycle().take(MAX_CHARS).collect(),2,true)] {
            let body=json!({"query":query,"documents":docs,"top_n":top_n,"return_documents":return_docs});let out=request(state,context,source,"rerank",body.clone()).await?;validate_output(contract,context.kind,&body,&out).map_err(incompatible)?;source_ranks.push(ranks(&out,docs.len(),top_n).map_err(incompatible)?);
        }
        let large_docs=(0..MAX_DOCS).map(|i|match i{0=>"Hospital evidence: 医院证据与医学图书馆研究资料。".to_owned(),1=>"Clinical research papers are stored in a hospital library.".to_owned(),2=>"Medical doctors consult reliable books.".to_owned(),_=>format!("Record {i}: green mountain and quiet river. 山川河流。")}).collect::<Vec<_>>();
        let body=json!({"query":"hospital evidence 医疗证据","documents":large_docs,"top_n":3,"return_documents":false});let output=request(state,context,source,"rerank",body.clone()).await?;validate_output(contract,context.kind,&body,&output).map_err(incompatible)?;source_ranks.push(ranks(&output,MAX_DOCS,3).map_err(incompatible)?);
        Ok(Reference{rank:Some(source_ranks),unit_normalized:true,..Reference::default()})
    }
}
fn read_reference(c:&Connection,user:i64,fp:&str)->Result<Option<Reference>,vr::VirtualError>{
    db::settings::get(c,&reference_key(user,fp))?.map(|raw|serde_json::from_str(&raw).map_err(|_|unverified("合成参考记录损坏，不能静默更换模型空间"))).transpose()
}
pub(crate) async fn verify_impl(state:&SharedState,user:i64,config:&vr::Config,actor:Option<i64>,force:bool,guard:Option<&crate::vector_renewal::RouteGuard>)->Result<(),vr::VirtualError>{
    if config.targets.len()<2{return Ok(());}
    let(kind,snapshot,mut reference,previous_contract)={let c=state.conn();let kind=db::model_routes::routes_for(&c,&config.targets[0].logical_model)?.ok_or_else(||unverified("目标模型不存在"))?.kind;if !supported(kind){return Ok(());}if !force&&require(&c,user,&config.targets,kind).is_ok(){return Ok(());}let snap=snapshot(&c,user,&config.targets,kind)?;let reference=read_reference(&c,user,&snap.fingerprint)?;let previous=historical(&c,user,&config.targets,kind).ok();(kind,snap,reference,previous)};
    let context=ProbeContext{user,actor,targets:&config.targets,kind,fingerprint:&snapshot.fingerprint,guard};
    let checked=tokio::time::timeout(Duration::from_secs(60),async{
        let mut contract=previous_contract.clone().unwrap_or_else(||Contract{dimensions:None,unit_normalized:false,max_input_chars:MAX_CHARS,max_batch:MAX_BATCH,max_documents:MAX_DOCS,max_total_chars:MAX_TOTAL_CHARS,evidence:"合成样本的数值和排序一致性检查；不是模型权重相同的证明".into(),fingerprint:snapshot.fingerprint.clone(),verified_at_ms:0,version:VERSION});
        for source in &snapshot.sources {
            let observed=observe(state,&context,source,&contract).await?;
            if let Some(baseline)=&reference{compare_reference(baseline,&observed,kind)?;}else{reference=Some(observed.clone());}
            if kind==ModelKind::Embeddings {contract.unit_normalized=if contract.dimensions.is_none(){observed.unit_normalized}else{contract.unit_normalized&&observed.unit_normalized};contract.dimensions=Some(observed.vectors.as_ref().unwrap()[0].len());}
        }
        contract.verified_at_ms=db::now_ms();Ok::<_,vr::VirtualError>(contract)
    }).await.map_err(|_|error("vector_source_unavailable","兼容检查总时间超过60秒"))??;
    let c=state.conn();if let Some(g)=guard{g.check(&c)?;}if let Some(a)=actor{db::shared_virtual_routes::root_actor(&c,a)?;}if snapshot_fn(&c,user,&config.targets,kind)?.fingerprint!=snapshot.fingerprint{return Err(unverified("检查期间来源配置或权限已变化"));}
    let tx=c.unchecked_transaction()?;db::settings::set(&tx,&key(user,&checked.fingerprint),&serde_json::to_string(&checked).map_err(|_|unverified("无法保存兼容记录"))?)?;
    if let Some(reference)=reference{db::settings::set(&tx,&reference_key(user,&snapshot.fingerprint),&serde_json::to_string(&reference).map_err(|_|unverified("无法保存合成参考"))?)?;}tx.commit()?;Ok(())
}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub(crate) struct SourceCheck {pub source_id:i64,pub logical_model:String,pub state:String,pub checked_at_ms:i64,pub message:String,pub error_code:String}
impl SourceCheck{pub(crate) fn usable(&self,now:i64)->bool{self.state=="valid"&&now>=self.checked_at_ms&&now-self.checked_at_ms<=TTL_MS}}
pub(crate) struct PartialVerification{pub contract:Contract,reference:Reference,pub sources:Vec<SourceCheck>}
impl PartialVerification{
    pub(crate) fn publish(&self,c:&Connection,user:i64)->Result<(),vr::VirtualError>{
        db::settings::set(c,&key(user,&self.contract.fingerprint),&serde_json::to_string(&self.contract).map_err(|_|unverified("无法保存兼容记录"))?)?;
        db::settings::set(c,&reference_key(user,&self.contract.fingerprint),&serde_json::to_string(&self.reference).map_err(|_|unverified("无法保存合成参考"))?)?;Ok(())
    }
}
/// Renew each source against an immutable numeric/ranking reference; one outage is not chain revocation.
pub(crate) async fn renew_sources(state:&SharedState,user:i64,config:&vr::Config,actor:Option<i64>,guard:Option<&crate::vector_renewal::RouteGuard>,previous:&[SourceCheck])->Result<PartialVerification,vr::VirtualError>{
    let(kind,snapshot,mut contract,reference)={let c=state.conn();let kind=db::model_routes::routes_for(&c,&config.targets[0].logical_model)?.ok_or_else(||unverified("目标模型不存在"))?.kind;let snap=snapshot(&c,user,&config.targets,kind)?;let contract=historical(&c,user,&config.targets,kind)?;let reference=read_reference(&c,user,&snap.fingerprint)?.ok_or_else(||unverified("尚未建立完整数值参考，不能由单一来源续签整链"))?;(kind,snap,contract,reference)};
    let context=ProbeContext{user,actor,targets:&config.targets,kind,fingerprint:&snapshot.fingerprint,guard};
    let mut order=(0..snapshot.sources.len()).collect::<Vec<_>>();
    order.sort_by_key(|i|{let s=&snapshot.sources[*i];previous.iter().find(|r|r.source_id==s.channel.id.0&&r.logical_model==s.model).map(|r|(if r.usable(db::now_ms()){0}else if r.checked_at_ms==0{1}else{2},r.checked_at_ms)).unwrap_or((1,0))});
    let deadline=tokio::time::Instant::now()+Duration::from_secs(60);let mut checks=Vec::new();
    for i in order {
        let source=&snapshot.sources[i];
        let remaining=deadline.saturating_duration_since(tokio::time::Instant::now());
        let result=if remaining.is_zero(){Err(error("vector_source_unavailable","本轮续验时间预算耗尽，将继续重试"))}else{
            tokio::time::timeout(remaining.min(Duration::from_secs(20)),observe(state,&context,source,&contract)).await.map_err(|_|error("vector_source_unavailable","此来源续验超时，其他来源独立检查")).and_then(|r|r).and_then(|observed|{
                compare_reference(&reference,&observed,kind)?;
                Ok(())
            })
        };
        if let Err(e)=&result {if !matches!(e.code,"vector_source_unavailable"|"vector_fallback_incompatible"|"vector_fallback_rate_limited"){return Err(error(e.code,e.message.clone()));}}
        let (phase,message,code)=match result{Ok(())=>("valid","此来源兼容续验通过".to_owned(),String::new()),Err(e)=>("isolated",e.message,e.code.into())};
        checks.push(SourceCheck{source_id:source.channel.id.0,logical_model:source.model.clone(),state:phase.into(),checked_at_ms:if remaining.is_zero(){0}else{db::now_ms()},message,error_code:code});
    }
    let c=state.conn();if let Some(g)=guard{g.check(&c)?;}if let Some(a)=actor{db::shared_virtual_routes::root_actor(&c,a)?;}if snapshot_fn(&c,user,&config.targets,kind)?.fingerprint!=snapshot.fingerprint{return Err(unverified("续验期间来源身份或权限已变化"));}
    if checks.iter().any(|r|r.usable(db::now_ms())){contract.verified_at_ms=db::now_ms();}
    // The reference was established by a complete successful check and remains immutable.
    Ok(PartialVerification{contract,reference,sources:checks})
}
// Separate name avoids shadowing the initial snapshot in verify.
fn snapshot_fn(
    c: &Connection,
    user: i64,
    targets: &[vr::Target],
    kind: ModelKind,
) -> Result<Snapshot, vr::VirtualError> {
    snapshot(c, user, targets, kind)
}
fn checked_text(value: &Value, max: usize) -> bool {
    value
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.chars().count() <= max)
}
/// Reject parameters and input forms outside the empirically verified request domain.
pub fn validate_request(
    contract: &Contract,
    kind: ModelKind,
    body: &Value,
) -> Result<(), vr::VirtualError> {
    let object = body
        .as_object()
        .ok_or_else(|| unverified("请求必须是 JSON 对象"))?;
    let allowed: &[&str] = if kind == ModelKind::Embeddings {
        &["model", "input", "encoding_format", "dimensions"]
    } else if kind == ModelKind::Rerank {
        &["model", "query", "documents", "top_n", "return_documents"]
    } else {
        return Err(unverified("兼容合同不适用于此接口"));
    };
    if object.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(unverified("多目标回退请求含未经过兼容检查的参数"));
    }
    let total_chars = if kind == ModelKind::Embeddings {
        body["input"].as_str().map_or_else(
            || {
                body["input"].as_array().map_or(0, |items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.chars().count())
                        .sum::<usize>()
                })
            },
            |s| s.chars().count(),
        )
    } else {
        body["query"].as_str().map_or(0, |s| s.chars().count())
            + body["documents"].as_array().map_or(0, |docs| {
                docs.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.chars().count())
                    .sum::<usize>()
            })
    };
    if total_chars > contract.max_total_chars {
        return Err(unverified(format!(
            "多目标回退请求文本总量超过已验证的{}字符范围",
            contract.max_total_chars
        )));
    }
    if kind == ModelKind::Embeddings {
        let valid = checked_text(&body["input"], contract.max_input_chars)
            || body["input"].as_array().is_some_and(|a| {
                !a.is_empty()
                    && a.len() <= contract.max_batch
                    && a.iter().all(|v| checked_text(v, contract.max_input_chars))
            });
        if !valid
            || body.get("encoding_format").is_some_and(|v| v != "float")
            || body.get("dimensions").is_some()
        {
            return Err(unverified(format!(
                "向量回退仅已验证每条1至{}字符、最多{}条文本、float编码，暂不支持dimensions参数",
                contract.max_input_chars, contract.max_batch
            )));
        }
    } else {
        let Some(docs) = body["documents"].as_array() else {
            return Err(unverified("重排回退仅支持字符串文档数组"));
        };
        if !checked_text(&body["query"], contract.max_input_chars)
            || docs.is_empty()
            || docs.len() > contract.max_documents
            || docs
                .iter()
                .any(|v| !checked_text(v, contract.max_input_chars))
            || body
                .get("top_n")
                .is_some_and(|v| v.as_u64().is_none_or(|n| n == 0 || n > docs.len() as u64))
            || body
                .get("return_documents")
                .is_some_and(|v| !v.is_boolean())
        {
            return Err(unverified(format!("重排回退仅已验证查询和文档各不超过{}字符、最多{}篇字符串文档及有效top_n/return_documents",contract.max_input_chars,contract.max_documents)));
        }
    }
    Ok(())
}
fn embedding_vectors(
    output: &Value,
    count: usize,
    dimension: Option<usize>,
) -> Result<Vec<Vec<f64>>, String> {
    let rows = output["data"]
        .as_array()
        .filter(|a| a.len() == count)
        .ok_or("向量响应条数与输入不一致")?;
    let mut indexed = vec![None; count];
    let mut dim = dimension;
    for row in rows {
        let index = row["index"]
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| *i < count)
            .ok_or("向量响应索引无效")?;
        if indexed[index].is_some() {
            return Err("向量响应索引重复".into());
        }
        let array = row["embedding"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 65536)
            .ok_or("向量响应必须是非空float数组")?;
        if dim.is_some_and(|d| d != array.len()) {
            return Err("向量响应维度与兼容合同不一致".into());
        }
        dim = Some(array.len());
        let vector = array
            .iter()
            .map(|v| {
                v.as_f64()
                    .filter(|n| n.is_finite())
                    .ok_or("向量响应含无效数值")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let norm = vector.iter().map(|x| x * x).sum::<f64>();
        if !norm.is_finite() || norm <= 0.0 {
            return Err("向量响应为零向量或数值溢出".into());
        }
        indexed[index] = Some(vector);
    }
    indexed
        .into_iter()
        .map(|v| v.ok_or_else(|| "向量响应索引不完整".into()))
        .collect()
}
fn unit_norm(vector: &[f64]) -> bool {
    (vector.iter().map(|x| x * x).sum::<f64>().sqrt() - 1.0).abs() <= 0.02
}
fn compare_vectors(a: &[Vec<f64>], b: &[Vec<f64>]) -> Result<(), String> {
    if a.len() != b.len() {
        return Err("向量响应批次不兼容".into());
    }
    for (a, b) in a.iter().zip(b) {
        if a.len() != b.len() {
            return Err("向量来源维度不兼容".into());
        }
        let an = a.iter().map(|x| x * x).sum::<f64>().sqrt();
        let bn = b.iter().map(|x| x * x).sum::<f64>().sqrt();
        let cosine = a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>() / (an * bn);
        if !cosine.is_finite() || cosine < 0.999 || ((an / bn) - 1.0).abs() > 0.02 {
            return Err("向量来源的方向或归一化不兼容，不能安全回退".into());
        }
    }
    Ok(())
}
fn ranks(output: &Value, documents: usize, count: usize) -> Result<Vec<(usize, f64)>, String> {
    let rows = output["results"]
        .as_array()
        .filter(|a| a.len() == count)
        .ok_or("重排响应条数与top_n不一致")?;
    let mut seen = HashSet::new();
    let mut result: Vec<(usize, f64)> = Vec::new();
    for row in rows {
        let index = row["index"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n < documents)
            .ok_or("重排响应索引越界")?;
        if !seen.insert(index) {
            return Err("重排响应索引重复".into());
        }
        let score = row["relevance_score"]
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or("重排评分无效")?;
        if result.last().is_some_and(|(_, previous)| *previous < score) {
            return Err("重排响应未按评分降序排列".into());
        }
        result.push((index, score));
    }
    Ok(result)
}
pub fn validate_output(
    contract: &Contract,
    kind: ModelKind,
    body: &Value,
    output: &Value,
) -> Result<(), String> {
    if kind == ModelKind::Embeddings {
        let count = body["input"].as_array().map_or(1, Vec::len);
        let vectors = embedding_vectors(output, count, contract.dimensions)?;
        if contract.unit_normalized && vectors.iter().any(|v| !unit_norm(v)) {
            return Err("向量响应归一化方式与兼容合同不一致".into());
        }
    } else if kind == ModelKind::Rerank {
        let docs = body["documents"].as_array().ok_or("重排文档无效")?;
        let count = body
            .get("top_n")
            .and_then(Value::as_u64)
            .map_or(docs.len(), |n| n as usize);
        ranks(output, docs.len(), count)?;
        if body["return_documents"] == true {
            for row in output["results"].as_array().ok_or("重排结果无效")? {
                let i = row["index"].as_u64().ok_or("重排索引无效")? as usize;
                let text = row
                    .get("document")
                    .and_then(|v| v.as_str().or_else(|| v.get("text").and_then(Value::as_str)));
                if text != docs.get(i).and_then(Value::as_str) {
                    return Err("重排返回文档与输入不一致".into());
                }
            }
        }
    } else {
        return Err("兼容合同不适用于此接口".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn contract() -> Contract {
        Contract {
            dimensions: Some(2),
            unit_normalized: true,
            max_input_chars: MAX_CHARS,
            max_batch: MAX_BATCH,
            max_documents: MAX_DOCS,
            max_total_chars: MAX_TOTAL_CHARS,
            evidence: "合成样本的数值和排序一致性检查；不是模型权重相同的证明".into(),
            fingerprint: "test".into(),
            verified_at_ms: 0,
            version: VERSION,
        }
    }
    #[test]
    fn embedding_domain_rejects_unverified_parameters_and_token_inputs() {
        let c = contract();
        for body in [
            json!({"input":[1,2]}),
            json!({"input":"x","dimensions":2}),
            json!({"input":"x","encoding_format":"base64"}),
            json!({"input":"x","user":"u"}),
            json!({"input":"中".repeat(513)}),
        ] {
            assert!(validate_request(&c, ModelKind::Embeddings, &body).is_err());
        }
        assert!(validate_request(
            &c,
            ModelKind::Embeddings,
            &json!({"input":["中".repeat(512),"text"],"encoding_format":"float"})
        )
        .is_ok());
    }
    #[test]
    fn embedding_output_checks_dimensions_indexes_and_nonzero() {
        let c = contract();
        let body = json!({"input":["a","b"]});
        for out in [
            json!({"data":[{"index":0,"embedding":[1,0]},{"index":0,"embedding":[1,0]}]}),
            json!({"data":[{"index":0,"embedding":[0,0]},{"index":1,"embedding":[1,0]}]}),
            json!({"data":[{"index":0,"embedding":[1]},{"index":1,"embedding":[1]}]}),
        ] {
            assert!(validate_output(&c, ModelKind::Embeddings, &body, &out).is_err());
        }
    }
    #[test]
    fn request_total_size_is_bounded() {
        let c = contract();
        let docs = vec!["中".repeat(512); 17];
        assert!(validate_request(
            &c,
            ModelKind::Rerank,
            &json!({"query":"q","documents":docs})
        )
        .is_err());
        let batch = vec!["中".repeat(512); 16];
        assert!(validate_request(&c, ModelKind::Embeddings, &json!({"input":batch})).is_ok());
    }
    #[test]
    fn compatibility_source_capacity_busy_rejects_before_billing_and_releases_on_drop() {
        let dir = std::env::temp_dir().join(format!("compat-capacity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::AppState::init(crate::Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
        let gate = state.channel_gate(77, 1).unwrap();
        let held = gate.clone().try_acquire_owned().unwrap();
        assert!(
            matches!(acquire_source_capacity(&state,77,1,None),Err(error) if error.code=="vector_fallback_rate_limited")
        );
        drop(held);
        let credential = db::provider_pool::CredentialRef {
            id: 88,
            provider_id: 1,
            enabled: true,
            priority: 0,
            max_concurrent: 1,
            key_strategy: "ordered".into(),
            switch_on_429: true,
            switch_on_400: false,
            revision: 1,
            provider_version: 1,
        };
        let held_credential = state
            .credential_gate(88, 1)
            .unwrap()
            .try_acquire_owned()
            .unwrap();
        assert!(acquire_source_capacity(&state, 77, 1, Some(&credential)).is_err());
        drop(gate.clone().try_acquire_owned().unwrap());
        drop(held_credential);
        let capacity = acquire_source_capacity(&state, 77, 1, Some(&credential)).unwrap();
        assert!(gate.clone().try_acquire_owned().is_err());
        assert_eq!(state.credential_load(88), 1);
        drop(capacity);
        drop(gate.clone().try_acquire_owned().unwrap());
        assert_eq!(state.credential_load(88), 0);
        {
            let c = state.conn();
            let count: i64 = c
                .query_row("SELECT COUNT(*) FROM billing_ledger", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn output_normalization_drift_is_rejected() {
        let c = contract();
        let body = json!({"input":"text"});
        let out = json!({"data":[{"index":0,"embedding":[2.0,0.0]}]});
        assert!(validate_output(&c, ModelKind::Embeddings, &body, &out).is_err());
        let mut nonunit = c;
        nonunit.unit_normalized = false;
        assert!(validate_output(&nonunit, ModelKind::Embeddings, &body, &out).is_ok());
    }
    #[test]
    fn probe_cancellation_charges_unknown_usage_without_regular_request_rows() {
        let dir = std::env::temp_dir().join(format!("compat-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::AppState::init(crate::Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
        let (user, ledger) = {
            let c = state.conn();
            let user = db::identity::create_user(&c, "cancel-user", "user", &Default::default())
                .unwrap()
                .id;
            db::billing::create_price(
                &c,
                &db::billing::Price {
                    id: 0,
                    channel_id: 1,
                    model: "synthetic-vector".into(),
                    input_micro_per_million: 1_000_000,
                    output_micro_per_million: 0,
                    max_input_tokens: 1000,
                    max_output_tokens: 0,
                },
                "test",
            )
            .unwrap();
            db::billing::set_budget(&c, "user", user, Some(100), "test").unwrap();
            let id = db::billing::reserve_user_diagnostic(
                &c,
                user,
                "compatibility:cancel",
                1,
                "synthetic-vector",
                30,
                &json!({"source_id":1}),
            )
            .unwrap();
            (user, id)
        };
        drop(ProbeBilling {
            state: state.clone(),
            ledger: Some(ledger),
            sent: true,
            system: false,
        });
        {
            let c = state.conn();
            let budget = db::billing::budgets(&c, Some("user"), Some(user))
                .unwrap()
                .remove(0);
            assert_eq!(budget.spent_micro, 30);
            assert_eq!(budget.reserved_micro, 0);
            let requests: i64 = c
                .query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))
                .unwrap();
            assert_eq!(requests, 0);
        }
        drop(state);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn same_dimension_does_not_prove_equivalence() {
        assert!(compare_vectors(&[vec![1., 0.]], &[vec![0., 1.]]).is_err());
        assert!(compare_vectors(&[vec![1., 0.]], &[vec![2., 0.]]).is_err());
        assert!(compare_vectors(&[vec![1., 0.]], &[vec![1., 0.001]]).is_ok());
    }
    #[test]
    fn rerank_checks_document_domain_and_result_contract() {
        let c = contract();
        assert!(validate_request(
            &c,
            ModelKind::Rerank,
            &json!({"query":"q","documents":[{"text":"a"}]})
        )
        .is_err());
        let body = json!({"query":"q","documents":["a","b"],"top_n":1,"return_documents":true});
        assert!(validate_output(
            &c,
            ModelKind::Rerank,
            &body,
            &json!({"results":[{"index":1,"relevance_score":0.8,"document":{"text":"b"}}]})
        )
        .is_ok());
        assert!(validate_output(
            &c,
            ModelKind::Rerank,
            &body,
            &json!({"results":[{"index":2,"relevance_score":0.8}]})
        )
        .is_err());
        assert!(ranks(&json!({"results":[{"index":0,"relevance_score":0.2},{"index":0,"relevance_score":0.1}]}),2,2).is_err());
    }
}

/// A successful renewal does not alter execution semantics solely by refreshing its timestamp.
pub fn execution_equivalent(a:Option<&Contract>,b:Option<&Contract>)->bool{match (a,b){(None,None)=>true,(Some(a),Some(b))=>{let mut a=a.clone();let mut b=b.clone();a.verified_at_ms=0;b.verified_at_ms=0;a==b},_=>false}}

pub(crate) fn contract_fingerprint(contract:&Contract)->&str{&contract.fingerprint}

pub(crate) fn has_history(c:&Connection,user:i64,targets:&[vr::Target],kind:ModelKind)->bool{historical(c,user,targets,kind).is_ok()}

pub(crate) fn has_reference(c:&Connection,user:i64,fp:&str)->Result<bool,vr::VirtualError>{Ok(read_reference(c,user,fp)?.is_some())}
