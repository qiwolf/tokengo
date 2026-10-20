use crate::{admin_api as api, Ctx};
use crate::model::{Channel, NameRoute};
use leptos::prelude::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Empty controls mean inheritance; explicit zero/false remain explicit overrides.
#[derive(Clone, Default, Debug, PartialEq)]
struct PolicyDraft {
    ordered: bool,
    targets: Vec<i64>,
    retry: String,
    reprobe: String,
    probe: String,
    auto: String,
    attempt: String,
    nonstream: String,
    budget: String,
}
impl PolicyDraft {
    fn read(v: &Value) -> Self {
        let number = |key: &str| v[key].as_u64().map(|n|n.to_string()).unwrap_or_default();
        let boolean = |key: &str| v[key].as_bool().map(|b|b.to_string()).unwrap_or_default();
        Self { ordered:v["channel_ids"].is_array(), targets:v["channel_ids"].as_array().map(|a|a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default(), retry:number("retry_times"),reprobe:number("reprobe_interval_ms"),probe:boolean("probe_enabled"),auto:boolean("include_in_auto"),attempt:number("attempt_timeout_ms"),nonstream:number("nonstream_timeout_ms"),budget:number("request_budget_ms") }
    }
    fn payload(&self) -> Result<Value,String> {
        let mut result=json!({});
        if self.ordered {
            if self.targets.is_empty() {return Err("请至少选择一个已接入此模型的上游 Key".into());}
            if self.targets.iter().enumerate().any(|(i,n)|*n<=0 || self.targets[..i].contains(n)) {return Err("上游 Key 选择重复或已失效，请重新读取策略".into());}
            result["channel_ids"]=json!(self.targets);
        }
        for (key,label,raw,min,max) in [("retry_times","每个目标重试次数",&self.retry,0,2),("reprobe_interval_ms","回切检查间隔",&self.reprobe,0,86_400_000),("attempt_timeout_ms","流式首包等待时间",&self.attempt,1,3_600_000),("nonstream_timeout_ms","非流式等待时间",&self.nonstream,1,3_600_000),("request_budget_ms","单次请求总时限",&self.budget,1,3_600_000)] {
            if raw.trim().is_empty(){continue;}
            let n=raw.trim().parse::<u64>().map_err(|_|format!("{label}请输入整数"))?;
            if !(min..=max).contains(&n){return Err(format!("{label}应在 {min} 到 {max} 之间"));}
            result[key]=json!(n);
        }
        for (key,raw) in [("probe_enabled",&self.probe),("include_in_auto",&self.auto)] {match raw.as_str(){""=>{},"true"=>result[key]=json!(true),"false"=>result[key]=json!(false),_=>return Err("请选择有效的启停选项".into())}}
        Ok(result)
    }
}

#[component]
fn NumberOverride(id: &'static str, label: &'static str, value: Signal<String>, set: Callback<String>, max:u64, #[prop(default=0)] min:u64) -> impl IntoView {
    view!{<label for=id>{label}<input id=id type="number" min=min max=max step="1" placeholder="留空继承全局" prop:value=value on:input=move|e|set.run(event_target_value(&e))/></label>}
}

#[component]
pub fn RoutePolicy(ctx: Ctx, open: RwSignal<bool>, #[prop(optional)] saving: Option<RwSignal<bool>>) -> impl IntoView {
    let model=RwSignal::new(String::new());
    let loaded=RwSignal::new(None::<String>);
    let draft=RwSignal::new(PolicyDraft::default());
    let saved=RwSignal::new(PolicyDraft::default());
    Effect::new(move |_|{if !open.get(){draft.set(saved.get_untracked());}});
    let routes=RwSignal::new(Vec::<NameRoute>::new());
    let channels=RwSignal::new(Vec::<Channel>::new());
    let key_labels=RwSignal::new(BTreeMap::<i64,String>::new());
    let busy=RwSignal::new(true);
    Effect::new(move |_|{if let Some(shared)=saving{shared.set(busy.get());}});
    let error=RwSignal::new(String::new());
    let message=RwSignal::new(String::new());
    leptos::task::spawn_local(async move {
        let r=crate::api::model_routes().await;
        let c=crate::api::channels().await;
        match (r,c){(Ok(r),Ok(c))=>{
            let providers:BTreeSet<_>=c.iter().filter_map(|row|row.provider_id).collect();
            let mut labels=BTreeMap::new();
            for provider in providers {
                match api::request("GET",&format!("/api/providers/{provider}/credentials"),None).await {
                    Ok(v)=>for (position,row) in api::rows(&v,"credentials").into_iter().enumerate(){
                        let label=crate::provider_api::credential_label(&row,position);
                        if let Some(ids)=row["source_ids"].as_array(){for id in ids.iter().filter_map(Value::as_i64){labels.insert(id,label.clone());}}
                    },
                    Err(e)=>{ctx.note_err(&e);error.set("部分上游 Key 备注读取失败，暂显示未命名；请重新打开后确认。".into());}
                }
            }
            key_labels.set(labels);routes.set(r.models);channels.set(c);
        },(Err(e),_)|(_,Err(e))=>{ctx.note_err(&e);error.set(e.message);}}
        busy.set(false);
    });
    let load=move |name:String| {
        if busy.get_untracked(){return;}
        model.set(name.clone());loaded.set(None);error.set(String::new());message.set(String::new());
        if name.is_empty(){return;}
        busy.set(true);
        leptos::task::spawn_local(async move {
            match api::request("GET",&format!("/api/model-routing?model={}",api::encode(&name)),None).await {
                Ok(v)=>{let d=PolicyDraft::read(&v["policy"]);draft.set(d.clone());saved.set(d);loaded.set(Some(name));message.set("已读取已保存的策略。留空或选择继承的项目沿用全局设置。".into());},
                Err(e)=>{ctx.note_err(&e);error.set(e.message);}
            }
            busy.set(false);
        });
    };
    let apply=move |delete:bool| {
        if busy.get_untracked(){return;}
        let Some(name)=loaded.get_untracked() else{return;};
        let payload=if delete{None}else{match draft.get_untracked().payload(){Ok(v)=>Some(v),Err(e)=>{error.set(e);return;}}};
        busy.set(true);error.set(String::new());message.set(String::new());
        leptos::task::spawn_local(async move {
            let path=format!("/api/model-routing?model={}",api::encode(&name));
            match api::request(if delete{"DELETE"}else{"PUT"},&path,payload).await {
                Ok(_)=>match api::request("GET",&path,None).await {
                    Ok(v)=>{let d=PolicyDraft::read(&v["policy"]);draft.set(d.clone());saved.set(d);message.set(if delete{"已恢复全局设置，并重新读取确认。"}else{"策略已保存并重新读取确认；本模型后续请求使用新策略。"}.into());},
                    Err(e)=>{ctx.note_err(&e);error.set(format!("提交已成功，但重新读取失败：{}。请重读确认。",e.message));}
                },
                Err(e)=>{ctx.note_err(&e);error.set(e.message);}
            }
            busy.set(false);
        });
    };
    view!{<section class="admin-section route-policy-form"><p class="muted small">"为指定的本地模型设置上游 Key 的调用顺序、重试与超时。只使用该模型已接入的 Key；未选中的 Key 不参与该模型的路由。"</p>
    <fieldset disabled=move||busy.get()><label for="routing-model">"选择本地模型"<select id="routing-model" prop:value=model on:change=move|e|load(event_target_value(&e))><option value="">"请选择模型"</option>{move||routes.get().into_iter().filter(|r|r.name!="auto").map(|r|view!{<option value=r.name.clone()>{r.name.clone()}</option>}).collect_view()}</select></label>
    <Show when=move||loaded.get().is_some()><h3>"上游 Key 与调用顺序"</h3><label class="pick"><input type="checkbox" prop:checked=move||draft.get().ordered on:change=move|e|{let checked=event_target_checked(&e);draft.update(|d|{d.ordered=checked;if checked && d.targets.is_empty(){let name=loaded.get_untracked().unwrap_or_default();d.targets=routes.get_untracked().iter().find(|r|r.name==name).map(|r|r.declared_by.clone()).unwrap_or_default();}});}/>"单独设置此模型的 Key 顺序"</label>
    <p class="muted small">"未勾选时继承全局顺序。勾选后，按下面列表从上到下尝试；启停状态仍以渠道和 Key 设置为准。"</p>
    <Show when=move||draft.get().ordered>{move||{
        let name=loaded.get().unwrap_or_default();let ids=routes.get().iter().find(|r|r.name==name).map(|r|r.declared_by.clone()).unwrap_or_default();let selected=draft.get().targets;let mut list:Vec<_>=channels.get().into_iter().filter(|c|ids.contains(&c.id)).collect();list.sort_by_key(|c|selected.iter().position(|id|*id==c.id).unwrap_or(usize::MAX));
        view!{<div class="table-wrap"><table><thead><tr><th>"参与调用"</th><th>"供应方 / 上游 Key"</th><th>"顺序"</th></tr></thead><tbody>{list.into_iter().map(|c|{let id=c.id;let provider_id=c.provider_id;let has_multiple_urls=channels.get_untracked().iter().filter(|other|other.provider_id==provider_id).map(|other|other.base_url.clone()).collect::<BTreeSet<_>>().len()>1;let endpoint=if has_multiple_urls{format!(" · {}",c.base_url)}else{String::new()};let label=format!("{} · {}{}{}",c.provider_name.unwrap_or_else(||"未分组".into()),key_labels.get().get(&id).cloned().unwrap_or_else(||"未命名上游 Key".into()),endpoint,if c.enabled&&c.provider_enabled{""}else{"（已停用）"});view!{<tr><td><input type="checkbox" aria-label=format!("参与调用：{label}") prop:checked=move||draft.get().targets.contains(&id) on:change=move|e|{let checked=event_target_checked(&e);draft.update(|d|if checked{if !d.targets.contains(&id){d.targets.push(id)}}else{d.targets.retain(|n|*n!=id)});}/></td><td>{label}</td><td><div class="toolbar"><button type="button" class="ghost" disabled=move||draft.get().targets.iter().position(|n|*n==id).map(|i|i==0).unwrap_or(true) on:click=move|_|draft.update(|d|{if let Some(i)=d.targets.iter().position(|n|*n==id){if i>0{d.targets.swap(i,i-1)}}})>"上移"</button><button type="button" class="ghost" disabled=move||draft.get().targets.iter().position(|n|*n==id).map(|i|i+1==draft.get().targets.len()).unwrap_or(true) on:click=move|_|draft.update(|d|{if let Some(i)=d.targets.iter().position(|n|*n==id){if i+1<d.targets.len(){d.targets.swap(i,i+1)}}})>"下移"</button></div></td></tr>}}).collect_view()}</tbody></table></div>}
    }}</Show>
    <h3>"重试与恢复"</h3><div class="form-grid">
    <NumberOverride id="routing-retry" label="每个目标重试次数（0–2）" value=Signal::derive(move||draft.get().retry) set=Callback::new(move|v|draft.update(|d|d.retry=v)) max=2/>
    <NumberOverride id="routing-reprobe" label="回切检查间隔（毫秒，0–86400000）" value=Signal::derive(move||draft.get().reprobe) set=Callback::new(move|v|draft.update(|d|d.reprobe=v)) max=86_400_000/>
    <label for="routing-probe">"失败后自动探测恢复"<select id="routing-probe" prop:value=move||draft.get().probe on:change=move|e|draft.update(|d|d.probe=event_target_value(&e))><option value="">"继承全局"</option><option value="true">"启用"</option><option value="false">"停用"</option></select></label>
    <label for="routing-auto">"参与 auto 自动模型选择"<select id="routing-auto" prop:value=move||draft.get().auto on:change=move|e|draft.update(|d|d.auto=event_target_value(&e))><option value="">"继承全局"</option><option value="true">"参与"</option><option value="false">"不参与"</option></select></label></div>
    <h3>"等待时间"</h3><p class="muted small">"单位为毫秒（1000 毫秒 = 1 秒），各项范围 1–3600000；留空继承全局设置。"</p><div class="form-grid">
    <NumberOverride id="routing-attempt" label="流式首包等待时间" value=Signal::derive(move||draft.get().attempt) set=Callback::new(move|v|draft.update(|d|d.attempt=v)) min=1 max=3_600_000/>
    <NumberOverride id="routing-nonstream" label="非流式等待时间" value=Signal::derive(move||draft.get().nonstream) set=Callback::new(move|v|draft.update(|d|d.nonstream=v)) min=1 max=3_600_000/>
    <NumberOverride id="routing-budget" label="单次请求总时限" value=Signal::derive(move||draft.get().budget) set=Callback::new(move|v|draft.update(|d|d.budget=v)) min=1 max=3_600_000/>
    </div><div class="toolbar"><button type="button" class="primary" on:click=move|_|apply(false)>"保存策略"</button><button type="button" class="ghost" on:click=move|_|{draft.set(saved.get_untracked());error.set(String::new());message.set("已取消未保存的修改，恢复为上次读取的策略。".into());}>"取消修改"</button><button type="button" class="ghost" on:click=move|_|apply(true)>"全部恢复继承全局"</button><button type="button" class="ghost" on:click=move|_|load(model.get_untracked())>"重新读取"</button></div></Show></fieldset>
    <p role="alert">{move||error.get()}</p><p role="status">{move||if busy.get(){"正在读取或保存策略…".into()}else{message.get()}}</p></section>}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn inheritance_is_omission_not_false_or_zero(){let d=PolicyDraft::default();assert_eq!(d.payload().unwrap(),json!({}));let mut d=d;d.retry="0".into();d.reprobe="0".into();d.probe="false".into();d.auto="false".into();assert_eq!(d.payload().unwrap(),json!({"retry_times":0,"reprobe_interval_ms":0,"probe_enabled":false,"include_in_auto":false}));}
    #[test] fn roundtrip_keeps_order_and_all_timeout_overrides(){let v=json!({"channel_ids":[8,2],"retry_times":2,"reprobe_interval_ms":12345,"probe_enabled":true,"include_in_auto":false,"attempt_timeout_ms":1,"nonstream_timeout_ms":75000,"request_budget_ms":3600000});assert_eq!(PolicyDraft::read(&v).payload().unwrap(),v);}
    #[test] fn invalid_input_never_creates_an_api_payload(){for v in [json!({"channel_ids":[]}),json!({"channel_ids":[2,2]}),json!({"retry_times":3}),json!({"reprobe_interval_ms":86400001}),json!({"attempt_timeout_ms":0}),json!({"request_budget_ms":3600001})]{assert!(PolicyDraft::read(&v).payload().is_err(),"{v}");}let mut d=PolicyDraft::default();d.retry="0.5".into();assert!(d.payload().is_err());}
}
