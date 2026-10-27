//! Versioned source compatibility diagnostics; explicit verification is separate from saves.
use crate::{Ctx, compatibility_api::{self as api, Job, Snapshot}};
use leptos::prelude::*;
use serde_json::Value;
use std::sync::{Arc, Mutex};
#[derive(Clone, Default)]
pub(crate) struct RequestScope { generation: u64, busy: bool }
impl RequestScope {
    fn begin(&mut self) -> Option<u64> { if self.busy {return None} self.generation+=1; self.busy=true; Some(self.generation) }
    fn invalidate(&mut self) { self.generation+=1; self.busy=false; }
    fn accepts(&self, ticket: u64) -> bool { self.generation == ticket }
    fn finish(&mut self,ticket:u64)->bool {if !self.accepts(ticket){return false} self.busy=false;true}
}
fn evidence_label(status: &str) -> &'static str {
    match status {"verified"=>"已验证（仅所述范围）","operator_declared"=>"人工声明，未实测","standard_api_claim"|"claimed"=>"标准接口声明，未确认身份",_=>"尚未验证"}
}
fn value_text(v:&Value)->String {match v {Value::Null=>"尚未确认".into(),Value::String(s)=>s.clone(),Value::Bool(b)=>if *b{"是".into()}else{"否".into()},_=>v.to_string()}}
fn runtime_metadata_applicable(contracts:&[Value])->bool {
    contracts.iter().any(|contract|contract["provider"]["runtime_profile"]==true)
}
fn configured_purpose_text(purpose:&Value)->String {
    let configured=purpose["configured"].as_str().or_else(||purpose["catalog_purpose"].as_str()).unwrap_or("");
    if configured.is_empty() {return "未配置；请在模型用途设置中维护，不按执行种类或推理能力推断。".into()}
    purpose["label"].as_str().filter(|label|!label.is_empty()).unwrap_or(configured).to_string()
}
fn budget_label(value:&Value)->String {
    let semantics=match value["semantics"].as_str().unwrap_or("") {"Total"|"total"=>"总输出（含思考）", "AnswerOnly"|"answer_only"=>"仅答案（思考预算另计）",_=>"上限口径尚未确认"};
    format!("输入上限：{}；输出上限：{}；{}。已配置：{}；已知合同：{}",value_text(&value["max_input_tokens"]),value_text(&value["max_output_tokens"]),semantics,value_text(&value["configured"]),value_text(&value["known"]))
}
fn status_label(value:&str)->&'static str {match value{
    "supported_native"=>"原生支持","exact_transform"=>"等价转换","documented_semantic_fold"=>"按文档折叠（语义有差异）","unsupported"=>"不支持","executor_gated"=>"执行器受限","passed"=>"本次范围验证通过","failed"=>"验证失败","running"=>"验证进行中","never"=>"尚未真实验证","canceled"=>"已取消","stale"=>"来源已变化，结果失效",_=>"尚未确认"}}
async fn wait_ms(ms:i32) {
    use wasm_bindgen::JsCast;
    let promise=js_sys::Promise::new(&mut |resolve,_| {
        if let Some(w)=web_sys::window(){let _=w.set_timeout_with_callback_and_timeout_and_arguments_0(resolve.unchecked_ref(),ms);}
    });
    let _=wasm_bindgen_futures::JsFuture::from(promise).await;
}
const VERIFY_TOTAL_MS:i64=15_000;
#[derive(Clone, Copy)]
// Server epoch timestamps are display-only; subtract only two browser-local timestamps.
struct ClientDeadline { started_at_ms:i64 }
impl ClientDeadline {
    fn starting_at(started_at_ms:i64)->Self {Self{started_at_ms}}
    fn remaining_ms(self,local_now_ms:i64)->i32 {
        VERIFY_TOTAL_MS.saturating_sub(local_now_ms.saturating_sub(self.started_at_ms)).clamp(0,VERIFY_TOTAL_MS) as i32
    }
}
fn verification_help(direct_native:bool)->&'static str {
    if direct_native {"本页最小模型调用验证可能产生费用；本页独立任务总截止时间15秒，可取消，已产生用量不能撤销。读取运行时元数据使用独立标准接口，不要求SSH，不修改第三方后端，不自动采纳声明。现有探针链路使用其自身的请求时限。"}
    else {"此来源通过下方现有探针链路验证，使用其自身配置的请求时限（等待超时1–60秒）；可能产生上游费用。预填和保存不会自动执行，已产生用量不能撤销。"}
}
fn inference_availability(snapshot:&Value,job:Option<&Job>)->String {
    if let Some(job)=job.filter(|job|job.mode=="inference") {
        format!("最近模型调用：{} · 原生协议 {}。{}",status_label(&job.state),job.native.as_deref().unwrap_or("尚未确认"),job.note)
    } else {
        format!("{}。{}",status_label(snapshot["state"].as_str().unwrap_or("")),snapshot["note"].as_str().unwrap_or("未进行最小请求验证；保存配置不会自动探测。"))
    }
}
async fn bounded_job(channel:i64,id:&str,deadline:ClientDeadline)->Result<Job,String>{
    use futures_util::{future::{select,Either},pin_mut};
    let read=api::job(channel,id);let timeout=wait_ms(deadline.remaining_ms(crate::now_ms()));
    pin_mut!(read,timeout);
    match select(read,timeout).await {Either::Left((result,_))=>result.map_err(|e|e.message),Either::Right(_)=>Err("已到总截止时间，正在请求取消；已产生用量不能撤销。".into())}
}
fn cancel_job(channel:i64,active:&Arc<Mutex<Option<String>>>){
    if let Some(id)=active.lock().unwrap().take(){leptos::task::spawn_local(async move{let _=api::cancel(channel,&id).await;});}
}
#[component]
pub fn CompatibilityDiagnostics(ctx:Ctx,channel:i64,model:String)->impl IntoView {
    let open=RwSignal::new(false);
    let snapshot=RwSignal::new(None::<Snapshot>);
    let result=RwSignal::new(None::<Job>);
    let latest_inference=RwSignal::new(None::<Job>);
    let loading=RwSignal::new(false);
    let busy=RwSignal::new(false);
    let error=RwSignal::new(String::new());
    let note=RwSignal::new(String::new());
    let native=RwSignal::new(String::new());
    let scope=StoredValue::new(Arc::new(Mutex::new(RequestScope::default())));
    let active=StoredValue::new(Arc::new(Mutex::new(None::<String>)));
    let model=StoredValue::new(model);
    let load=Callback::new(move|()|{
        if busy.get_untracked(){return}
        let state=scope.get_value();state.lock().unwrap().invalidate();let ticket=state.lock().unwrap().begin().unwrap();
        loading.set(true);snapshot.set(None);result.set(None);error.set(String::new());note.set(String::new());
        let requested_model=model.get_value();
        leptos::task::spawn_local(async move {
            let response=api::read(channel,&requested_model).await;
            if !state.lock().unwrap().finish(ticket){return}
            loading.set(false);
            match response {Ok(data) if data.schema_version==1&&data.identity.channel_id==channel&&data.identity.model==requested_model=>{native.set(data.identity.native.first().cloned().unwrap_or_default());let previous=data.verification.clone().filter(|job|job.identity==data.identity);latest_inference.set(previous.clone());result.set(previous);snapshot.set(Some(data));},Ok(_)=>error.set("诊断版本不兼容，请刷新页面或联系管理员。".into()),Err(e)=>{ctx.note_err(&e);error.set(e.message);}}
        });
    });
    Effect::new(move|_|{
        if !open.get(){scope.get_value().lock().unwrap().invalidate();cancel_job(channel,&active.get_value());busy.set(false);loading.set(false);}
    });
    let cleanup_scope=scope.get_value();let cleanup_active=active.get_value();
    on_cleanup(move||{cleanup_scope.lock().unwrap().invalidate();cancel_job(channel,&cleanup_active);});
    let verify=Callback::new(move|mode:&'static str|{
        if busy.get_untracked()||loading.get_untracked(){return}
        let Some(data)=snapshot.get_untracked()else{return};
        let selected=native.get_untracked();
        if !data.identity.native.contains(&selected){error.set("请先选择已声明的原生协议。".into());return}
        let state=scope.get_value();let Some(ticket)=state.lock().unwrap().begin()else{return};
        busy.set(true);error.set(String::new());result.set(None);note.set("正在创建独立验证任务…总截止时间15秒。".into());
        let active=active.get_value();
        let local_deadline=ClientDeadline::starting_at(crate::now_ms());
        let deadline_scope=state.clone();let deadline_active=active.clone();
        leptos::task::spawn_local(async move {
            wait_ms(15_000).await;
            let expired={let current=deadline_scope.lock().unwrap();current.accepts(ticket)&&current.busy};
            if expired {deadline_scope.lock().unwrap().invalidate();cancel_job(channel,&deadline_active);busy.set(false);result.set(None);note.set(String::new());error.set("已到本次操作15秒总截止时间，已请求取消；上游已产生用量不能撤销。".into());}
        });
        leptos::task::spawn_local(async move {
            match api::verify(channel,&data,mode,&selected).await {
                Ok(mut job)=>{
                    let current=state.lock().unwrap().accepts(ticket);
                    if !current{let _=api::cancel(channel,&job.id).await;return}
                    if job.schema_version!=1||job.identity!=data.identity {let id=job.id.clone();leptos::task::spawn_local(async move{let _=api::cancel(channel,&id).await;});if !state.lock().unwrap().finish(ticket){return}busy.set(false);error.set("来源或模型版本已变化，请重新读取诊断。".into());return}
                    *active.lock().unwrap()=Some(job.id.clone());
                    note.set("验证进行中，可取消。取消不会撤销上游已产生的用量。".into());
                    while job.state=="running" {
                        if !state.lock().unwrap().accepts(ticket){return}
                        result.set(Some(job.clone()));
                        wait_ms(500).await;
                        if !state.lock().unwrap().accepts(ticket){return}
                        let response=bounded_job(channel,&job.id,local_deadline).await;
                        if !state.lock().unwrap().accepts(ticket){return}
                        match response {
                            Ok(next) if next.schema_version==1&&next.identity==data.identity=>job=next,
                            Ok(_)=>{cancel_job(channel,&active);state.lock().unwrap().finish(ticket);busy.set(false);error.set("诊断身份已变化，旧结果不再展示。请重新读取。".into());return},
                            Err(message)=>{cancel_job(channel,&active);state.lock().unwrap().finish(ticket);busy.set(false);error.set(message);return}
                        }
                    }
                    if !state.lock().unwrap().finish(ticket){return}
                    active.lock().unwrap().take();busy.set(false);note.set(String::new());if job.mode=="inference"{latest_inference.set(Some(job.clone()));}result.set(Some(job));
                },
                Err(e)=>{if !state.lock().unwrap().finish(ticket){return}busy.set(false);note.set(String::new());ctx.note_err(&e);error.set(e.message);}
            }
        });
    });
    view! {
        <button type="button" class="ghost" aria-haspopup="dialog" on:click=move |_|{open.set(true);load.run(());}>"兼容诊断与独立验证"</button>
        <super::ModuleDialog open=open title="来源与模型兼容诊断".to_string()>
        <section class="compatibility-diagnostics">
            <p class="account-help">"读取此页和保存设置均不调用模型。下方真实验证需单独启动，结果仅对应当前来源、模型、协议和配置版本。"</p>
            <Show when=move||loading.get()><p role="status">"正在读取已有配置与合同…"</p></Show>
            {move||snapshot.get().map(|data|{
                let runtime=data.runtime_identity.clone();
                let identity=data.identity.clone();
                let protocols=identity.native.clone();
                let availability_identity=identity.clone();let previous_availability=data.availability.clone();
                let lane=data.verification_lane.clone();let direct_native=lane.direct_native_inference;
                let runtime_metadata=runtime_metadata_applicable(&data.contracts);
                let lane_kind=StoredValue::new(lane.recommended_kind.clone());
                let lane_model=StoredValue::new(lane.model.clone());let lane_source=lane.source_id;
                let known_page=lane.management_page=="probes"&&lane.management_hash=="#probes"&&lane.access=="root_session";
                view!{
                    <p><strong>"本次范围："</strong>{format!("来源 #{} · {} → {}",identity.channel_id,identity.model,identity.upstream_model)}</p>
                    <p class="muted small">{format!("诊断版本 {} · 合同 {} · 配置代次 {} · 认证线路 {} · 来源指纹 {}",data.schema_version,data.contract_revision,identity.binding_generation,identity.authentication_line,identity.source_fingerprint)}</p>
                    <dl class="compatibility-facts">
                        <dt>"配置用途"</dt><dd>{configured_purpose_text(&data.purpose)}<p class="account-help">"用户维护的目录用途，不代表推理能力或实际调用已验证。"</p></dd>
                        <dt>"执行种类"</dt><dd>{format!("{} · 来源 {}",value_text(&data.purpose["kind"]),value_text(&data.purpose["source"]))}</dd>
                        <dt>"推理档位"</dt><dd>{super::model_reasoning::capability_label(&data.reasoning)}</dd>
                        <dt>"思考模式"</dt><dd>{super::model_reasoning::thinking_modes_label(&data.reasoning)}</dd>
                        <dt>"预算合同"</dt><dd>{budget_label(&data.budget)}<p class="account-help">"这是配置和合同摘要；实际输入、思考预算及输出上限需按本次请求共同计算，不代表本次调用已验收。"</p></dd>
                        <dt>"账号与本地配置"</dt><dd>{format!("本地启用：{}；账号状态：{}；远端访问不由配置推断",value_text(&data.account_availability["local_enabled"]),value_text(&data.account_availability["account_state"]))}</dd>
                        <dt>"真实验证可用性"</dt><dd aria-live="polite">{move||{let current=result.get().filter(|job|job.identity==availability_identity&&job.mode=="inference").or_else(||latest_inference.get().filter(|job|job.identity==availability_identity));inference_availability(&previous_availability,current.as_ref())}}</dd>
                        <dt>"运行时身份"</dt><dd>{format!("{} · {} · 能力：{}；认证：{}",value_text(&runtime["runtime"]),evidence_label(runtime["evidence"].as_str().unwrap_or("")),value_text(&runtime["capabilities"]),value_text(&runtime["authentication"]))}<p class="account-help">"人工声明及标准接口返回均不证明运行时身份或账号权限。未知能力保持未知。"</p></dd>
                    </dl>
                    <h3>"字段合同与限制"</h3>
                    <p class="account-help">"以下是有条件的合同说明，未验证任意请求体；透传未知参数不等于已验证支持。"</p>
                    <div class="compatibility-contracts">{data.fields.into_iter().map(|field|view!{
                        <details><summary>{format!("{} · {} · {}",value_text(&field["field"]),value_text(&field["protocol"]),status_label(field["status"].as_str().unwrap_or("")))}</summary>
                        <p>{format!("范围：{}",value_text(&field["scope"]))}</p><p>{format!("条件：{}",value_text(&field["conditions"]))}</p><p class="muted small">{format!("规则：{}",value_text(&field["rule_ids"]))}</p></details>
                    }).collect_view()}</div>
                    <details><summary>"来源合同证据"</summary>
                        <p>{format!("文档：{}；样本：{}；真实调用：{}",value_text(&data.evidence["documentation"]),value_text(&data.evidence["sample"]),value_text(&data.evidence["live"]))}</p>
                        {data.contracts.into_iter().map(|contract|{
                            let provider=&contract["provider"];let parameters=&contract["parameters"];
                            let references=provider["references"].as_array().cloned().unwrap_or_default();
                            view!{<p>{format!("{} · {} · {}",value_text(&provider["provider"]),value_text(&contract["protocol"]),value_text(&parameters["source"]))}</p><p>{value_text(&provider["notes"])}</p><p class="muted small">{format!("模型规则：{}；选用依据：{}；认证判断：{}",value_text(&parameters["model_rules"]),value_text(&parameters["selection"]),value_text(&parameters["authentication_availability"]))}</p>
                            <div>{references.into_iter().filter_map(|v|v.as_str().filter(|url|url.starts_with("https://")).map(str::to_owned)).map(|url|view!{<p><a href=url.clone() target="_blank" rel="noopener noreferrer">{url.clone()}</a></p>}).collect_view()}</div>}
                        }).collect_view()}
                        <p class="account-help">"文档合同、样本测试、真实调用分别记载；一次最小调用成功不证明全部字段或其他账号可用。"</p>
                    </details>
                    <label class="account-field">"本次验证原生协议"<select prop:value=native disabled=move||busy.get() on:change=move|e|native.set(event_target_value(&e))>{protocols.into_iter().map(|p|view!{<option value=p.clone()>{p.clone()}</option>}).collect_view()}</select></label>
                    <p class="account-help">{verification_help(direct_native)}</p>
                    <p class="account-help">{lane.note}</p>
                    <button type="button" class="primary" on:click=move |_|{
                        if !known_page||lane_source!=channel||lane_model.get_value()!=model.get_value(){error.set("验证入口与当前来源不一致，请重新读取诊断。".into());return}
                        super::custom_probes::queue_diagnostic_target(lane_source,lane_model.get_value(),lane_kind.get_value());open.set(false);ctx.go(super::Page::Probes);
                    }>"打开现有探针并预填本来源"</button>
                    <p class="account-help">"探针使用该来源现有认证与计量链路；仅预填未保存草稿，不会自动保存或执行。请审阅目标、请求类型和可能费用，保存后再明确执行一次。"</p>
                    <Show when=move||direct_native><div class="toolbar compatibility-actions">
                        <button type="button" class="primary" disabled=move||busy.get()||loading.get()||native.get().is_empty() on:click=move |_|verify.run("inference")>"启动真实调用验证（可能计费）"</button>
                    </div></Show>
                    <Show when=move||runtime_metadata><div class="toolbar compatibility-actions"><button type="button" class="ghost" disabled=move||busy.get()||loading.get()||native.get().is_empty() on:click=move |_|verify.run("runtime_metadata")>"独立读取运行时元数据"</button></div><p class="account-help">"显式Ollama/vLLM来源可按标准接口读取运行时元数据，适用于向量和重排等用途；这是本页独立任务，期限15秒，不自动采纳能力或修改来源配置。"</p></Show>
                }
            })}
            <Show when=move||busy.get()><button type="button" class="ghost" on:click=move |_|{scope.get_value().lock().unwrap().invalidate();cancel_job(channel,&active.get_value());busy.set(false);result.set(None);note.set("已请求取消；已产生的用量不能撤销。".into());}>"取消本次验证"</button></Show>
            <p class="account-success" role="status">{move||note.get()}</p>
            <p class="account-error" role="alert">{move||error.get()}</p>
            {move||result.get().map(|job|{let candidate=job.candidate.clone();let has_candidate=!candidate.is_null();view!{
                <section aria-live="polite"><h3>"本次验证结果"</h3><p>{format!("{} · {} · {}",if job.mode=="runtime_metadata"{"标准运行时元数据"}else{"最小模型调用"},job.native.as_deref().unwrap_or("尚未确认协议"),status_label(&job.state))}</p><p>{job.note}</p><p class="muted small">{format!("任务 {} · HTTP {} · 总截止 {} ms（自任务开始）",job.id,job.http_status.map(|s|s.to_string()).unwrap_or_else(||"未取得响应".into()),job.deadline_at_ms-job.started_at_ms)}</p><Show when=move||has_candidate><p>{format!("运行时：{} · {}；版本已返回：{}；模板已返回：{}；推理解析器已返回：{}；工具解析器已返回：{}",value_text(&candidate["runtime"]),evidence_label(candidate["evidence"].as_str().unwrap_or("")),value_text(&candidate["version_known"]),value_text(&candidate["template_known"]),value_text(&candidate["reasoning_parser_known"]),value_text(&candidate["tool_parser_known"]))}</p><p>"元数据仅作为声明候选，不自动升级为已验证身份或模型能力。"</p></Show></section>
            }})}
            <button type="button" class="ghost" disabled=move||busy.get()||loading.get() on:click=move |_|load.run(())>"重新读取诊断"</button>
        </section>
        </super::ModuleDialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn runtime_metadata_is_applicable_by_explicit_runtime_profile_independent_of_model_kind() {
        for provider in ["ollama","vllm"] {
            assert!(runtime_metadata_applicable(&[serde_json::json!({"provider":{"provider":provider,"runtime_profile":true}})]));
        }
        assert!(!runtime_metadata_applicable(&[serde_json::json!({"provider":{"provider":"unknown","runtime_profile":false},"runtime_identity":{"evidence":"standard_api_claim"}})]));
        assert!(!runtime_metadata_applicable(&[]));
    }
    #[test] fn configured_purpose_is_independent_from_execution_kind_and_reasoning_capability() {
        let configured=serde_json::json!({"configured":"chat","label":"对话与推理","kind":"unknown","source":"unknown","configured_source":"operator_configured"});
        assert_eq!(configured_purpose_text(&configured),"对话与推理");
        let missing=serde_json::json!({"configured":"","label":"","kind":"embeddings","source":"observed","configured_source":"not_configured"});
        assert!(configured_purpose_text(&missing).contains("未配置"));assert!(!configured_purpose_text(&missing).contains("向量"));
        let missing_legacy=serde_json::json!({"kind":"chat","source":"manual"});assert!(configured_purpose_text(&missing_legacy).contains("未配置"));
    }
    #[test] fn probe_lane_uses_its_own_timeout_without_promising_the_direct_job_deadline() {
        assert!(verification_help(true).contains("本页独立任务总截止时间15秒"));
        assert!(verification_help(false).contains("其自身配置的请求时限"));
        assert!(!verification_help(false).contains("15秒"));
    }
    #[test] fn local_deadline_is_independent_of_server_and_browser_epoch_offsets() {
        for server_epoch in [0i64,1_800_000_000_000] {
            for browser_offset in [-60_000i64,0,60_000,3_600_000] {
                let clicked_at=server_epoch+browser_offset;
                let deadline=ClientDeadline::starting_at(clicked_at);
                assert_eq!(deadline.remaining_ms(clicked_at+500),14_500);
                assert_eq!(deadline.remaining_ms(clicked_at+3_000),12_000);
                assert_eq!(deadline.remaining_ms(clicked_at+15_000),0);
                assert_eq!(deadline.remaining_ms(clicked_at+30_000),0);
            }
        }
    }
    #[test] fn current_inference_availability_overrides_static_never_and_names_native_scope() {
        let job:Job=serde_json::from_value(serde_json::json!({"schema_version":1,"id":"synthetic","identity":{"channel_id":1,"model":"alias","upstream_model":"upstream","source_fingerprint":"safe","binding_generation":1,"native":["chat"],"authentication_line":"externally_configured_credentials","api_version":null,"contract_revision":"v1","credential_revision":null,"provider_version":null},"native":"chat","mode":"inference","state":"passed","started_at_ms":0,"deadline_at_ms":15000,"note":"最小请求范围"})).unwrap();
        let snapshot=serde_json::json!({"state":"never"});
        let text=inference_availability(&snapshot,Some(&job));assert!(text.contains("验证通过"));assert!(text.contains("原生协议 chat"));assert!(!text.contains("尚未真实验证"));
        let mut metadata=job;metadata.mode="runtime_metadata".into();assert!(inference_availability(&snapshot,Some(&metadata)).contains("尚未真实验证"));
    }
    #[test] fn duplicate_verification_is_rejected_while_busy() {
        let mut s = RequestScope::default();
        assert!(s.begin().is_some()); assert!(s.begin().is_none());
    }
    #[test] fn closed_or_replaced_scope_never_accepts_previous_response() {
        let mut s = RequestScope::default(); let old=s.begin().unwrap(); s.invalidate();
        assert!(!s.accepts(old)); let current=s.begin().unwrap();
        assert!(s.accepts(current)); assert!(!s.accepts(old));
    }
    #[test] fn late_finish_cannot_unlock_another_verification() {
        let mut s=RequestScope::default();let old=s.begin().unwrap();s.invalidate();let current=s.begin().unwrap();
        assert!(!s.finish(old));assert!(s.begin().is_none());assert!(s.finish(current));assert!(s.begin().is_some());
    }
    #[test] fn claims_and_unknown_cannot_be_labeled_verified() {
        for value in ["unknown", "claimed", "standard_api_claim", "operator_declared", "supported"] {
            assert!(!evidence_label(value).contains("已验证"));
        }
        assert!(evidence_label("verified").contains("已验证"));
    }
}
