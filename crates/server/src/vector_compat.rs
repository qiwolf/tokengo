//! Server-issued identity and numeric evidence for embedding/rerank fallback.
//! Workload profiles record measured samples; upstream model capacity governs live requests.
use crate::{
    db::{self, user_virtual_routes as vr},
    state::SharedState,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, time::Duration};
use tokengo_shared::{Channel, ModelKind};

pub(crate) const TTL_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_CHARS: usize = 512;
const MAX_BATCH: usize = 16;
const MAX_DOCS: usize = 128;
const MAX_TOTAL_CHARS: usize = 8192;
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const VERSION: u32 = 3;
const WORKLOAD_CHARS: usize = 4096;
const WORKLOAD_BATCH: usize = 300;
const WORKLOAD_DOCS: usize = 300;
const WORKLOAD_QUERY_CHARS: usize = 512;
const WORKLOAD_TOTAL_CHARS: usize = WORKLOAD_DOCS * WORKLOAD_CHARS + WORKLOAD_QUERY_CHARS;
const WORKLOAD_PROFILE: &str = "unicode4096-batch300-rerank300-query512-numeric-envelope-v3";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub dimensions: Option<usize>,
    pub unit_normalized: bool,
    pub max_input_chars: usize,
    #[serde(default)]
    pub max_query_chars: usize,
    #[serde(default)]
    pub workload_profile: String,
    #[serde(default)]
    pub workload_verified_at_ms: Option<i64>,
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
        // Identity namespace deliberately remains v1: old evidence is valid for its old bounds.
        &json!({"version":1,"kind":format!("{kind:?}"),"user":user,"targets":facts}),
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
    if !valid_shape(&contract)||contract.fingerprint!=snapshot.fingerprint||(kind==ModelKind::Embeddings&&contract.dimensions.is_none_or(|d|d==0)){return Err(unverified("兼容合同版本、来源身份或边界已改变，不能继承旧证据"));}
    Ok(contract)
}

fn valid_shape(c: &Contract) -> bool {
    match c.version {
        1 => c.max_input_chars == MAX_CHARS && c.max_batch == MAX_BATCH
            && c.max_documents == MAX_DOCS && c.max_total_chars == MAX_TOTAL_CHARS
            && c.max_query_chars == 0 && c.workload_profile.is_empty()
            && c.workload_verified_at_ms.is_none(),
        2 | VERSION => c.max_input_chars == WORKLOAD_CHARS && c.max_batch == WORKLOAD_BATCH
            && c.max_documents == WORKLOAD_DOCS && c.max_total_chars == WORKLOAD_TOTAL_CHARS
            && c.max_query_chars == WORKLOAD_QUERY_CHARS && c.workload_profile == (if c.version == 2 {"unicode4096-batch300-rerank300-query512-v2"} else {WORKLOAD_PROFILE})
            && c.workload_verified_at_ms.is_some_and(|at| at > 0),
        _ => false,
    }
}
/// Old evidence remains usable; a complete current-profile check adds workload evidence without shrinking live capacity.
pub(crate) fn current_profile(contract: &Contract) -> bool { contract.version == VERSION && valid_shape(contract) }
fn candidate_contract(fingerprint: String) -> Contract {
    Contract { dimensions: None, unit_normalized: false, max_input_chars: WORKLOAD_CHARS,
        max_query_chars: WORKLOAD_QUERY_CHARS, max_batch: WORKLOAD_BATCH,
        max_documents: WORKLOAD_DOCS, max_total_chars: WORKLOAD_TOTAL_CHARS,
        workload_profile: WORKLOAD_PROFILE.into(), workload_verified_at_ms: None,
        evidence: String::new(), fingerprint, verified_at_ms: 0, version: VERSION }
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
    let request_bytes = serde_json::to_vec(&body).map_err(|_| unverified("兼容检查输入无法序列化"))?;
    if request_bytes.len() > MAX_REQUEST_BYTES { return Err(unverified("兼容检查请求超过4MiB正文上限")); }
    let timeout = if request_bytes.len() > MAX_TOTAL_CHARS * 3 { Duration::from_secs(120) } else { Duration::from_secs(15) };
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
            timeout,
            MAX_RESPONSE_BYTES,
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
        if a.len()!=b.len() {return Err(incompatible("重排来源的查询参考数量不兼容，已隔离此来源"));}
        for (a,b) in a.iter().zip(b) {compare_ranks(a,b).map_err(incompatible)?;}
    }
    Ok(())
}
/// Provider floating-point kernels may permute near ties. Compare scores by original
/// candidate index, and reject inversions only when either ranking has a real margin.
fn compare_ranks(a: &[(usize,f64)], b: &[(usize,f64)]) -> Result<(), &'static str> {
    const SCORE_TOLERANCE: f64 = 0.02 + 1e-12;
    let by_index=b.iter().enumerate().map(|(position,(index,score))|(*index,(position,*score))).collect::<HashMap<_,_>>();
    if a.len()!=b.len() || by_index.len()!=b.len() || a.iter().map(|(index,_)|index).collect::<HashSet<_>>().len()!=a.len()
        || a.iter().any(|(index,score)|by_index.get(index).is_none_or(|(_,other)|!score.is_finite()||!other.is_finite()||(score-other).abs()>SCORE_TOLERANCE)) {
        return Err("重排来源的候选集合或逐项评分不兼容，已隔离此来源");
    }
    for (position,(index,score)) in a.iter().enumerate() {
        let (other_position,other_score)=by_index[index];
        for (next_index,next_score) in &a[position+1..] {
            let (next_position,next_other_score)=by_index[next_index];
            if other_position>next_position && ((score-next_score).abs()>SCORE_TOLERANCE || (other_score-next_other_score).abs()>SCORE_TOLERANCE) {
                return Err("重排来源出现显著评分差的排序逆序，已隔离此来源");
            }
        }
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
        if contract.unit_normalized && !unit_normalized {
            return Err(incompatible("兼容探针的默认归一化与既有来源证据不一致，不能重新签发改变后的模型空间"));
        }
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
/// Deterministic long-form corpus: every candidate is preserved and individually identifiable.
fn workload_texts(count: usize) -> Vec<String> {
    (0..count).map(|i| {
        let prefix = format!("Document {i:03}: 医院临床资料，证据编号 {i:03}。 ");
        let topic = match i % 4 {
            0 => "Hospital evidence and safe medication. 医院证据与安全用药，医学图书馆保存临床研究。 ",
            1 => "Quiet river and green mountain. 山川河流与自然风光，记录生态环境。 ",
            2 => "Reliable clinical research and patient care. 临床研究和患者照护，需要可靠的证据。 ",
            _ => "Library books and historical records. 图书馆书籍与历史资料，可以帮助检索。 ",
        };
        prefix.chars().chain(topic.chars().cycle()).take(WORKLOAD_CHARS).collect()
    }).collect()
}
async fn observe_workload(state: &SharedState, context: &ProbeContext<'_>, source: &Source, contract: &Contract) -> Result<(Reference, String, String), vr::VirtualError> {
    let body = if context.kind == ModelKind::Embeddings {
        json!({"input":workload_texts(WORKLOAD_BATCH),"encoding_format":"float"})
    } else {
        let query: String = "hospital evidence 医院临床研究与安全用药资料。".chars().cycle().take(WORKLOAD_QUERY_CHARS).collect();
        json!({"query":query,"documents":workload_texts(WORKLOAD_DOCS),"top_n":WORKLOAD_DOCS,"return_documents":true})
    };
    validate_request(contract, context.kind, &body)?;
    let input_digest = hex::encode(Sha256::digest(serde_json::to_vec(&body).map_err(|_| unverified("无法记录负载摘要"))?));
    let output = request(state,context,source,if context.kind == ModelKind::Embeddings {"embeddings"} else {"rerank"},body.clone()).await?;
    validate_output(contract,context.kind,&body,&output).map_err(incompatible)?;
    let observed = if context.kind == ModelKind::Embeddings {
        let vectors = embedding_vectors(&output,WORKLOAD_BATCH,contract.dimensions).map_err(incompatible)?;
        // Check mapping to the first and last original input, rather than count alone.
        for index in [0, WORKLOAD_BATCH - 1] {
            let single_body=json!({"input":body["input"][index],"encoding_format":"float"});
            let single=request(state,context,source,"embeddings",single_body.clone()).await?;
            validate_output(contract,context.kind,&single_body,&single).map_err(incompatible)?;
            let single=embedding_vectors(&single,1,contract.dimensions).map_err(incompatible)?;
            compare_vectors(&[vectors[index].clone()],&single).map_err(incompatible)?;
        }
        Reference {vectors:Some(vectors),single:Some(Vec::new()),unit_normalized:contract.unit_normalized,..Reference::default()}
    } else {
        Reference {rank:Some(vec![ranks(&output,WORKLOAD_DOCS,WORKLOAD_DOCS).map_err(incompatible)?]),unit_normalized:true,..Reference::default()}
    };
    // Numeric evidence excludes usage counters, model display names and other volatile metadata.
    let output_digest=hex::encode(Sha256::digest(serde_json::to_vec(&observed).map_err(|_| unverified("无法记录输出摘要"))?));
    Ok((observed,input_digest,output_digest))
}
fn read_reference(c:&Connection,user:i64,fp:&str)->Result<Option<Reference>,vr::VirtualError>{
    db::settings::get(c,&reference_key(user,fp))?.map(|raw|serde_json::from_str(&raw).map_err(|_|unverified("合成参考记录损坏，不能静默更换模型空间"))).transpose()
}
pub(crate) async fn verify_impl(state:&SharedState,user:i64,config:&vr::Config,actor:Option<i64>,force:bool,guard:Option<&crate::vector_renewal::RouteGuard>)->Result<(),vr::VirtualError>{
    if config.targets.len()<2{return Ok(());}
    let(kind,snapshot,mut reference,previous_contract)={let c=state.conn();let kind=db::model_routes::routes_for(&c,&config.targets[0].logical_model)?.ok_or_else(||unverified("目标模型不存在"))?.kind;if !supported(kind){return Ok(());}if !force&&require(&c,user,&config.targets,kind).is_ok(){return Ok(());}let snap=snapshot(&c,user,&config.targets,kind)?;let reference=read_reference(&c,user,&snap.fingerprint)?;let previous=historical(&c,user,&config.targets,kind).ok();(kind,snap,reference,previous)};
    let context=ProbeContext{user,actor,targets:&config.targets,kind,fingerprint:&snapshot.fingerprint,guard};
    // Full workload certification is explicit; daily renewal never reruns 300 long documents.
    let budget = Duration::from_millis((snapshot.sources.len() as u64).saturating_mul(full_source_budget_ms(kind)));
    let checked=tokio::time::timeout(budget,async{
        let mut contract=candidate_contract(snapshot.fingerprint.clone());
        // A missing numeric reference does not erase existing identity evidence.
        // Inherit the known dimensions/default normalization before the very first
        // response is checked, including legacy-reference bootstrap and v1 upgrade.
        if let Some(previous)=&previous_contract {
            contract.dimensions=previous.dimensions;
            contract.unit_normalized=previous.unit_normalized;
        }
        let mut workload_references: Vec<(i64, Reference, Option<Reference>)> = Vec::new();
        let mut evidence = Vec::new();
        for source in &snapshot.sources {
            let observed=observe(state,&context,source,&contract).await?;
            if let Some(baseline)=&reference{compare_reference(baseline,&observed,kind)?;}else{reference=Some(observed.clone());}
            if kind==ModelKind::Embeddings {contract.unit_normalized=if contract.dimensions.is_none(){observed.unit_normalized}else{contract.unit_normalized&&observed.unit_normalized};contract.dimensions=Some(observed.vectors.as_ref().unwrap()[0].len());}
            let (workload, input_digest, output_digest)=observe_workload(state,&context,source,&contract).await?;
            // Exactly two predeclared full embedding observations measure source noise;
            // never repeat until a favorable comparison happens. Rerank is unchanged.
            let repeated = if kind == ModelKind::Embeddings {
                let (repeat, repeat_input, repeat_output) = observe_workload(state,&context,source,&contract).await?;
                if repeat_input != input_digest {return Err(incompatible("重复负载输入摘要不一致"));}
                Some((repeat, repeat_output))
            } else {None};
            let numeric_self_check = if let Some((repeat,_))=&repeated {
                Some(compare_workload_vectors(workload.vectors.as_ref().unwrap(),repeat.vectors.as_ref().unwrap(),workload.vectors.as_ref().unwrap(),repeat.vectors.as_ref().unwrap()).map_err(incompatible)?)
            } else {None};
            let mut numeric_checks=Vec::new();
            // Compatibility is not transitive: every source pair must satisfy the cap.
            for (baseline_id,baseline,baseline_repeat) in &workload_references {
                if kind == ModelKind::Embeddings {
                    let check=compare_workload_vectors(
                        baseline.vectors.as_ref().unwrap(),baseline_repeat.as_ref().unwrap().vectors.as_ref().unwrap(),
                        workload.vectors.as_ref().unwrap(),repeated.as_ref().unwrap().0.vectors.as_ref().unwrap(),
                    ).map_err(incompatible)?;
                    numeric_checks.push(json!({"against_source_id":baseline_id,"check":check}));
                } else {compare_reference(baseline,&workload,kind)?;}
            }
            evidence.push(json!({"source_id":source.channel.id.0,"logical_model":source.model,"input_sha256":input_digest,"output_sha256":output_digest,"repeat_output_sha256":repeated.as_ref().map(|r|&r.1),"numeric_self_check":numeric_self_check,"numeric_checks":numeric_checks}));
            workload_references.push((source.channel.id.0,workload,repeated.map(|r|r.0)));
        }
        contract.verified_at_ms=db::now_ms();
        contract.workload_verified_at_ms=Some(contract.verified_at_ms);
        contract.evidence=json!({"profile":WORKLOAD_PROFILE,"method":"full input/output count, index integrity, original document equality and cross-source numeric/ranking checks; synthetic evidence is not model-weight identity","numeric_policy":{"short_and_renewal_cosine_min":0.999,"full_embedding_repetitions":2,"full_embedding_normalized_l2_base":0.002_f64.sqrt(),"full_embedding_normalized_l2_cap":0.1,"norm_ratio_tolerance":0.02,"rule":"maximum cross-repeat distance <= min(cap, sum of both within-source repeat distances + base); each within-source repeat distance <= cap"},"sources":evidence}).to_string();
        Ok::<_,vr::VirtualError>(contract)
    }).await.map_err(|_|error("vector_source_unavailable","完整负载兼容检查超过时间预算；现有小域证据保持不变"))??;
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
/// The certification corpus describes measured evidence, never a synthetic capacity limit.
/// Provider extensions remain untouched and are accepted/rejected by the actual upstream.
pub fn validate_request(
    _contract: &Contract,
    kind: ModelKind,
    body: &Value,
) -> Result<(), vr::VirtualError> {
    if !body.is_object() { return Err(unverified("请求必须是 JSON 对象")); }
    if kind == ModelKind::Embeddings {
        embedding_input_count(&body["input"]).map_err(unverified)?;
        if body.get("encoding_format").is_some_and(|v| v != "float" && v != "base64") {
            return Err(unverified("向量encoding_format必须为float或base64"));
        }
        if body.get("dimensions").is_some_and(|v| v.as_u64().is_none_or(|n| n == 0 || usize::try_from(n).is_err())) {
            return Err(unverified("向量dimensions必须是正整数，实际支持范围由上游模型决定"));
        }
    } else if kind == ModelKind::Rerank {
        let docs=body["documents"].as_array().filter(|docs| !docs.is_empty()).ok_or_else(||unverified("重排documents必须是非空文档数组"))?;
        if !body["query"].is_string() || docs.iter().any(|doc| document_text(doc).is_none()) {
            return Err(unverified("重排query必须是字符串，documents必须是字符串或包含text字符串的对象"));
        }
        if body.get("top_n").is_some_and(|v| v.as_u64().is_none_or(|n| n == 0))
            || body.get("return_documents").is_some_and(|v| !v.is_boolean()) {
            return Err(unverified("重排top_n须为正整数，return_documents须为布尔值"));
        }
    } else { return Err(unverified("兼容合同不适用于此接口")); }
    Ok(())
}
fn document_text(value: &Value) -> Option<&str> {
    value.as_str().or_else(||value.get("text").and_then(Value::as_str))
}
fn embedding_input_count(input: &Value) -> Result<usize, &'static str> {
    if input.is_string() { return Ok(1); }
    let items=input.as_array().filter(|items| !items.is_empty()).ok_or("向量input必须是文本、文本数组、token数组或token数组批次")?;
    let token=|v: &Value| v.as_u64().is_some_and(|n| u32::try_from(n).is_ok());
    if items.iter().all(Value::is_string) { return Ok(items.len()); }
    if items.iter().all(token) { return Ok(1); }
    if items.iter().all(|v|v.as_array().is_some_and(|tokens|!tokens.is_empty()&&tokens.iter().all(token))) { return Ok(items.len()); }
    Err("向量input包含混合类型或无效token")
}
/// True only inside the original empirical evidence. False does not mean unsupported.
/// Callers may report the evidence boundary, but must not turn it into a chain-wide 400.
pub(crate) fn within_verified_domain(contract: &Contract, kind: ModelKind, body: &Value) -> bool {
    if validate_request(contract,kind,body).is_err() { return false; }
    let Some(object)=body.as_object() else {return false;};
    let allowed: &[&str]=if kind==ModelKind::Embeddings {&["model","input","encoding_format"]} else {&["model","query","documents","top_n","return_documents"]};
    if object.keys().any(|k|!allowed.contains(&k.as_str())) {return false;}
    if serde_json::to_vec(body).map_or(true,|bytes|bytes.len()>MAX_REQUEST_BYTES) {return false;}
    if kind==ModelKind::Embeddings {
        if body.get("encoding_format").is_some_and(|v|v!="float") {return false;}
        let texts=if let Some(text)=body["input"].as_str(){vec![text]}else if let Some(items)=body["input"].as_array(){
            let Some(texts)=items.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else{return false;};texts
        }else{return false;};
        texts.len()<=contract.max_batch && texts.iter().all(|s|!s.is_empty()&&s.chars().count()<=contract.max_input_chars)
            && texts.iter().map(|s|s.chars().count()).sum::<usize>()<=contract.max_total_chars
    }else{
        let query=&body["query"];let docs=body["documents"].as_array().unwrap();
        if body.get("top_n").and_then(Value::as_u64).is_some_and(|n|n>docs.len() as u64) {return false;}
        checked_text(query,if contract.max_query_chars==0{contract.max_input_chars}else{contract.max_query_chars})
            && docs.len()<=contract.max_documents && docs.iter().all(|v|checked_text(v,contract.max_input_chars))
            && query.as_str().unwrap().chars().count()+docs.iter().map(|v|v.as_str().map_or(0,|s|s.chars().count())).sum::<usize>()<=contract.max_total_chars
    }
}
fn full_source_budget_ms(kind:ModelKind)->u64 {if kind==ModelKind::Embeddings {360_000} else {270_000}}
pub(crate) fn full_verify_budget_ms(c: &Connection, user: i64, targets: &[vr::Target], kind: ModelKind) -> Result<u64, vr::VirtualError> {
    Ok((snapshot(c,user,targets,kind)?.sources.len() as u64).saturating_mul(full_source_budget_ms(kind)))
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
            .filter(|a| !a.is_empty())
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
fn decode_base64_embeddings(output: &Value) -> Result<Value, String> {
    use base64::Engine;
    let mut decoded=output.clone();
    let rows=decoded["data"].as_array_mut().ok_or("base64向量响应缺少data数组")?;
    for row in rows {
        let raw=row["embedding"].as_str().ok_or("base64向量响应必须是base64字符串")?;
        let bytes=base64::engine::general_purpose::STANDARD.decode(raw).map_err(|_|"向量响应base64编码无效")?;
        if bytes.is_empty() || bytes.len()%4!=0 {return Err("base64向量字节数不是非空float32数组".into());}
        let values=bytes.chunks_exact(4).map(|chunk| {
            let n=f32::from_le_bytes(chunk.try_into().unwrap());
            if !n.is_finite(){return Err("base64向量包含非有限数值");}
            Ok(json!(n))
        }).collect::<Result<Vec<_>,_>>()?;
        row["embedding"]=Value::Array(values);
    }
    Ok(decoded)
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
/// Calibrate only long-workload comparison against each source's measured repeat
/// noise. Stable inputs retain the original .999 directional budget. A normalized
/// L2 cap of .1 (cosine .995) prevents an unstable source widening it without bound.
fn compare_workload_vectors(a: &[Vec<f64>], ar: &[Vec<f64>], b: &[Vec<f64>], br: &[Vec<f64>]) -> Result<Value,String> {
    if a.is_empty() || [ar.len(),b.len(),br.len()].iter().any(|n|*n!=a.len()) {return Err("重复向量批次不兼容".into());}
    let distance=|x:&[f64],y:&[f64]| -> Result<f64,String> {
        if x.len()!=y.len() || x.is_empty() {return Err("重复向量维度不兼容".into());}
        let xn=x.iter().map(|v|v*v).sum::<f64>().sqrt();let yn=y.iter().map(|v|v*v).sum::<f64>().sqrt();
        if !xn.is_finite() || !yn.is_finite() || xn<=0.0 || yn<=0.0 || (xn/yn-1.0).abs()>0.02 {return Err("重复向量数值或归一化不兼容".into());}
        let cosine=x.iter().zip(y).map(|(v,w)|v*w).sum::<f64>()/(xn*yn);
        if !cosine.is_finite() {return Err("重复向量方向无效".into());}
        Ok((2.0*(1.0-cosine.clamp(-1.0,1.0))).sqrt())
    };
    let base=0.002_f64.sqrt();let cap=0.1;let mut max_noise:f64=0.0;let mut max_cross:f64=0.0;let mut max_utilization:f64=0.0;
    for i in 0..a.len() {
        let an=distance(&a[i],&ar[i])?;let bn=distance(&b[i],&br[i])?;
        if an>cap+1e-12 || bn>cap+1e-12 {return Err("来源自身重复向量波动超过硬上限，不能安全回退".into());}
        let budget=(base+an+bn).min(cap);let mut cross:f64=0.0;
        for (x,y) in [(&a[i],&b[i]),(&a[i],&br[i]),(&ar[i],&b[i]),(&ar[i],&br[i])] {cross=cross.max(distance(x,y)?);}
        if cross>budget+1e-12 {return Err("向量来源方向差异超过逐样本重复误差预算，不能安全回退".into());}
        max_noise=max_noise.max(an.max(bn));max_cross=max_cross.max(cross);max_utilization=max_utilization.max(cross/budget);
    }
    Ok(json!({"items":a.len(),"max_within_source_l2":max_noise,"max_cross_source_l2":max_cross,"max_budget_utilization":max_utilization}))
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
        let count = embedding_input_count(&body["input"]).map_err(str::to_owned)?;
        let dimension = body.get("dimensions").and_then(Value::as_u64).and_then(|n|usize::try_from(n).ok()).or(contract.dimensions);
        let decoded;
        let output = if body["encoding_format"] == "base64" {
            decoded = decode_base64_embeddings(output)?;
            &decoded
        } else { output };
        // The live result must preserve shape and finite, nonzero vectors. Normalization
        // is recorded by certification probes, not imposed on valid provider extensions.
        embedding_vectors(output, count, dimension)?;
    } else if kind == ModelKind::Rerank {
        let docs = body["documents"].as_array().ok_or("重排文档无效")?;
        let count = body
            .get("top_n")
            .and_then(Value::as_u64)
            .map_or(docs.len(), |n|usize::try_from(n).unwrap_or(usize::MAX).min(docs.len()));
        ranks(output, docs.len(), count)?;
        if body["return_documents"] == true {
            for row in output["results"].as_array().ok_or("重排结果无效")? {
                let i = row["index"].as_u64().ok_or("重排索引无效")? as usize;
                let text = row
                    .get("document")
                    .and_then(|v| v.as_str().or_else(|| v.get("text").and_then(Value::as_str)));
                if text != docs.get(i).and_then(document_text) {
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
            max_query_chars: 0,
            workload_profile: String::new(),
            workload_verified_at_ms: None,
            max_batch: MAX_BATCH,
            max_documents: MAX_DOCS,
            max_total_chars: MAX_TOTAL_CHARS,
            evidence: "合成样本的数值和排序一致性检查；不是模型权重相同的证明".into(),
            fingerprint: "test".into(),
            verified_at_ms: 0,
            version: 1,
        }
    }
    #[test]
    fn request_validation_preserves_real_provider_capabilities_outside_evidence() {
        let c = contract();
        for body in [
            json!({"input":[1,"mixed"]}),
            json!({"input":"x","dimensions":0}),
            json!({"input":"x","encoding_format":"invalid"}),
            json!({"input":[[1],[-1]]}),
        ] { assert!(validate_request(&c, ModelKind::Embeddings, &body).is_err()); }
        for body in [
            json!({"input":[1,2]}),
            json!({"input":"x","dimensions":2}),
            json!({"input":"x","encoding_format":"base64"}),
            json!({"input":"x","user":"u"}),
            json!({"input":"中".repeat(513)}),
        ] {
            assert!(validate_request(&c, ModelKind::Embeddings, &body).is_ok());
            assert!(!within_verified_domain(&c, ModelKind::Embeddings, &body));
        }
        let body=json!({"input":["中".repeat(512),"text"],"encoding_format":"float"});
        assert!(validate_request(&c, ModelKind::Embeddings, &body).is_ok());
        assert!(within_verified_domain(&c, ModelKind::Embeddings, &body));
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
    fn certification_size_is_evidence_not_a_user_request_limit() {
        let c = contract();
        let docs = vec!["中".repeat(512); 17];
        let body=json!({"query":"q","documents":docs});
        assert!(validate_request(&c,ModelKind::Rerank,&body).is_ok());
        assert!(!within_verified_domain(&c,ModelKind::Rerank,&body));
        let batch = vec!["中".repeat(512); 16];
        assert!(validate_request(&c, ModelKind::Embeddings, &json!({"input":batch})).is_ok());
    }
    fn workload_contract() -> Contract {
        let mut c = candidate_contract("test".into());
        c.dimensions = Some(2);
        c.workload_verified_at_ms = Some(1);
        c
    }
    #[test]
    fn legacy_evidence_remains_readable_without_capping_live_capacity() {
        let mut raw = serde_json::to_value(contract()).unwrap();
        for name in ["max_query_chars", "workload_profile", "workload_verified_at_ms"] {
            raw.as_object_mut().unwrap().remove(name);
        }
        let c: Contract = serde_json::from_value(raw).unwrap();
        assert!(valid_shape(&c));
        assert!(!current_profile(&c));
        assert!(validate_request(&c, ModelKind::Rerank, &json!({"query":"q","documents":["中".repeat(513)]})).is_ok());
        assert!(!within_verified_domain(&c, ModelKind::Rerank, &json!({"query":"q","documents":["中".repeat(513)]})));
        let mut altered = c;
        altered.max_input_chars = WORKLOAD_CHARS;
        assert!(!valid_shape(&altered));
    }
    #[test]
    fn workload_profile_accepts_300_full_length_original_documents_and_embeddings() {
        let c = workload_contract();
        assert!(current_profile(&c));
        let docs = workload_texts(WORKLOAD_DOCS);
        assert_eq!(docs.len(), 300);
        assert!(docs.iter().all(|text| text.chars().count() == WORKLOAD_CHARS));
        assert_eq!(docs.iter().collect::<HashSet<_>>().len(), 300);
        let body = json!({"query":"中".repeat(512),"documents":docs,"top_n":300,"return_documents":true});
        let original = body.clone();
        assert!(validate_request(&c, ModelKind::Rerank, &body).is_ok());
        assert_eq!(body, original);
        let embedding = json!({"input":workload_texts(WORKLOAD_BATCH),"encoding_format":"float"});
        assert!(validate_request(&c, ModelKind::Embeddings, &embedding).is_ok());
        for invalid in [
            json!({"query":"中".repeat(513),"documents":["x"]}),
            json!({"query":"q","documents":["中".repeat(4097)]}),
            json!({"query":"q","documents":vec!["x";301]}),
        ] {
            assert!(validate_request(&c, ModelKind::Rerank, &invalid).is_ok());
            assert!(!within_verified_domain(&c, ModelKind::Rerank, &invalid));
        }
        let extra=json!({"input":vec!["x";301]});
        assert!(validate_request(&c, ModelKind::Embeddings, &extra).is_ok());
        assert!(!within_verified_domain(&c, ModelKind::Embeddings, &extra));
        // Characters and wire bytes are separate limits: escaped JSON can exceed 4 MiB.
        let escaped=json!({"input":vec!["\u{0001}".repeat(WORKLOAD_CHARS);WORKLOAD_BATCH]});
        assert!(validate_request(&c, ModelKind::Embeddings, &escaped).is_ok());
        assert!(!within_verified_domain(&c, ModelKind::Embeddings, &escaped));
    }
    #[test]
    fn provider_rerank_extensions_and_large_query_are_forwardable_without_certification_claim() {
        let c=workload_contract();
        let body=json!({"query":"中".repeat(1000),"documents":vec!["x".repeat(9000);400],"max_chunks_per_doc":20,"overlap_tokens":80,"top_n":300});
        assert!(validate_request(&c,ModelKind::Rerank,&body).is_ok());
        assert!(!within_verified_domain(&c,ModelKind::Rerank,&body));
    }
    #[test]
    fn token_batches_base64_and_requested_dimensions_keep_output_integrity() {
        use base64::Engine;
        let c=workload_contract();
        let encode=|xs: &[f32]| base64::engine::general_purpose::STANDARD.encode(xs.iter().flat_map(|n|n.to_le_bytes()).collect::<Vec<_>>());
        let body=json!({"input":[10,20,30],"encoding_format":"base64","dimensions":3});
        let out=json!({"data":[{"index":0,"embedding":encode(&[1.0,0.0,0.0])}]});
        assert!(validate_request(&c,ModelKind::Embeddings,&body).is_ok());
        assert!(validate_output(&c,ModelKind::Embeddings,&body,&out).is_ok());
        let batch=json!({"input":[[10,20],[30]],"encoding_format":"base64","dimensions":3});
        assert!(validate_output(&c,ModelKind::Embeddings,&batch,&out).is_err());
        let wrong=json!({"data":[{"index":0,"embedding":encode(&[1.0,0.0])}]});
        assert!(validate_output(&c,ModelKind::Embeddings,&body,&wrong).is_err());
        let invalid=json!({"data":[{"index":0,"embedding":encode(&[f32::NAN,0.0,0.0])}]});
        assert!(validate_output(&c,ModelKind::Embeddings,&body,&invalid).is_err());
    }
    #[test]
    fn cross_source_rerank_accepts_near_tie_swaps_but_rejects_material_disagreement() {
        assert!(compare_ranks(&[(0,0.8),(1,0.79),(2,0.1)],&[(1,0.8),(0,0.79),(2,0.1)]).is_ok());
        assert!(compare_ranks(&[(0,0.8),(1,0.775)],&[(1,0.785),(0,0.78)]).is_err());
        assert!(compare_ranks(&[(0,0.8),(1,0.7)],&[(1,0.8),(0,0.7)]).is_err());
        assert!(compare_ranks(&[(0,0.8),(1,0.79)],&[(0,0.8),(2,0.79)]).is_err());
        assert!(compare_ranks(&[(0,0.8),(1,0.79)],&[(0,0.8),(0,0.79)]).is_err());
    }
    #[test]
    fn full_rerank_output_requires_all_candidates_and_original_text() {
        let c=workload_contract();
        let docs=workload_texts(WORKLOAD_DOCS);
        let body=json!({"query":"q","documents":docs,"top_n":300,"return_documents":true});
        let rows=(0..300).map(|i|json!({"index":i,"relevance_score":1.0-i as f64/300.0,"document":{"text":body["documents"][i]}})).collect::<Vec<_>>();
        let mut output=json!({"results":rows});
        assert!(validate_output(&c,ModelKind::Rerank,&body,&output).is_ok());
        output["results"][299]["document"]["text"]=json!("truncated");
        assert!(validate_output(&c,ModelKind::Rerank,&body,&output).is_err());
        output["results"].as_array_mut().unwrap().pop();
        assert!(validate_output(&c,ModelKind::Rerank,&body,&output).is_err());
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
    fn live_output_does_not_impose_probe_normalization_on_provider_requests() {
        let c = contract();
        let body = json!({"input":"text"});
        let out = json!({"data":[{"index":0,"embedding":[2.0,0.0]}]});
        assert!(validate_output(&c, ModelKind::Embeddings, &body, &out).is_ok());
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
    fn long_workload_noise_budget_is_bounded_and_does_not_weaken_stable_sources() {
        let vector=|angle:f64|vec![angle.cos(),angle.sin()];
        let a=vec![vector(0.0)];let stable=vec![vector(0.05)];
        assert!(compare_vectors(&a,&stable).is_err());
        assert!(compare_workload_vectors(&a,&a,&stable,&stable).is_err());
        let noise=vec![vector(0.04)];let b=vec![vector(0.07)];
        assert!(compare_workload_vectors(&a,&noise,&b,&b).is_ok());
        let too_far=vec![vector(0.11)];
        assert!(compare_workload_vectors(&a,&noise,&too_far,&too_far).is_err());
        assert!(compare_workload_vectors(&a,&too_far,&a,&a).is_err());
        assert!(compare_workload_vectors(&a,&noise,&[vec![2.0,0.0]],&[vec![2.0,0.0]]).is_err());
        assert!(compare_workload_vectors(&a,&noise,&[vec![0.0,1.0]],&[vec![0.0,1.0]]).is_err());
        assert!(compare_workload_vectors(&a,&noise,&[],&[]).is_err());
        assert!(compare_workload_vectors(&a,&noise,&[vec![f64::NAN,0.0]],&a).is_err());
    }
    #[test]
    fn full_observation_budget_covers_every_declared_embedding_request() {
        // short batch + short single + 2*(full batch + first/last single).
        assert!(full_source_budget_ms(ModelKind::Embeddings)>=2*120_000+6*15_000);
        assert_eq!(full_source_budget_ms(ModelKind::Rerank),270_000);
    }
    #[test]
    fn noisy_source_compatibility_is_not_transitive() {
        let v=|angle:f64|vec![vec![angle.cos(),angle.sin()]];
        assert!(compare_workload_vectors(&v(0.0),&v(0.0),&v(0.04),&v(0.08)).is_ok());
        assert!(compare_workload_vectors(&v(0.0),&v(0.0),&v(-0.04),&v(-0.08)).is_ok());
        assert!(compare_workload_vectors(&v(0.04),&v(0.08),&v(-0.04),&v(-0.08)).is_err());
    }
    #[test]
    fn previous_v2_workload_evidence_remains_readable() {
        let mut c=workload_contract();c.version=2;c.workload_profile="unicode4096-batch300-rerank300-query512-v2".into();
        assert!(valid_shape(&c));assert!(!current_profile(&c));
        c.workload_profile=WORKLOAD_PROFILE.into();assert!(!valid_shape(&c));
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
        .is_ok());
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
