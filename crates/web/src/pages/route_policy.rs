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
    local_strategy: String,
    attempt: String,
    nonstream: String,
    budget: String,
}
impl PolicyDraft {
    fn read(v: &Value) -> Self {
        let number = |key: &str| v[key].as_u64().map(|n|n.to_string()).unwrap_or_default();
        let boolean = |key: &str| v[key].as_bool().map(|b|b.to_string()).unwrap_or_default();
        Self { ordered:v["channel_ids"].is_array(), targets:v["channel_ids"].as_array().map(|a|a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default(), retry:number("retry_times"),reprobe:number("reprobe_interval_ms"),probe:boolean("probe_enabled"),auto:boolean("include_in_auto"),local_strategy:match v.get("local_strategy"){None|Some(Value::Null)=>String::new(),Some(Value::String(value))=>value.clone(),_=>"__invalid__".into()},attempt:number("attempt_timeout_ms"),nonstream:number("nonstream_timeout_ms"),budget:number("request_budget_ms") }
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
        match self.local_strategy.as_str() {
            "" => {},
            "ordered" | "round_robin" => result["local_strategy"] = json!(self.local_strategy),
            _ => return Err("请选择有效的本地节点分配方式".into()),
        }
        Ok(result)
    }
}

/// Managed provider keys share one scheduling policy. Ungrouped sources remain independent.
fn target_group(id:i64, channels:&[Channel])->(bool,i64){
    match channels.iter().find(|c|c.id==id).and_then(|c|c.provider_id){Some(provider)=>(true,provider),None=>(false,id)}
}
fn selected_groups(targets:&[i64],channels:&[Channel])->Vec<Vec<i64>>{
    let mut groups:Vec<Vec<i64>>=Vec::new();
    for id in targets{let key=target_group(*id,channels);if let Some(group)=groups.iter_mut().find(|g|target_group(g[0],channels)==key){group.push(*id);}else{groups.push(vec![*id]);}}
    groups
}
fn move_provider_group(targets:&mut Vec<i64>,channels:&[Channel],id:i64,up:bool){
    let mut groups=selected_groups(targets,channels);let key=target_group(id,channels);
    if let Some(index)=groups.iter().position(|g|target_group(g[0],channels)==key){
        let next=if up{index.checked_sub(1)}else{(index+1<groups.len()).then_some(index+1)};
        if let Some(next)=next{groups.swap(index,next);*targets=groups.into_iter().flatten().collect();}
    }
}
fn select_target(targets:&mut Vec<i64>,channels:&[Channel],id:i64,selected:bool){
    if !selected{targets.retain(|existing|*existing!=id);return;}
    if targets.contains(&id){return;}
    let key=target_group(id,channels);
    let at=targets.iter().rposition(|other|target_group(*other,channels)==key).map(|i|i+1).unwrap_or(targets.len());
    targets.insert(at,id);
}

fn stale_targets(targets: &[i64], declared: &[i64]) -> Vec<i64> {
    targets.iter().copied().filter(|id| !declared.contains(id)).collect()
}

/// Removing an obsolete target is a draft edit, never an implicit change to inheritance.
fn remove_stale_target(draft: &mut PolicyDraft, id: i64, declared: &[i64]) -> bool {
    if declared.contains(&id) { return false; }
    let before = draft.targets.len();
    draft.targets.retain(|target| *target != id);
    draft.targets.len() != before
}

fn stale_target_label(id: i64, channels: &[Channel], labels: &BTreeMap<i64, String>) -> String {
    match channels.iter().find(|channel| channel.id == id) {
        Some(channel) => {
            let label = labels.get(&id).filter(|label| !label.trim().is_empty())
                .cloned().unwrap_or_else(|| channel.name.clone());
            let provider = channel.provider_name.as_deref().filter(|name| !name.trim().is_empty())
                .unwrap_or("未分组上游");
            format!("{provider} · {label}（连接 {id}，已不再接入此模型）")
        }
        None => format!("已删除的连接（编号 {id}）"),
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
        if !delete && draft.get_untracked().ordered {
            let declared=routes.get_untracked().iter().find(|r|r.name==name).map(|r|r.declared_by.clone()).unwrap_or_default();
            let selected=draft.get_untracked().targets;
            if !stale_targets(&selected,&declared).is_empty() {error.set("请先移除“失效的已选目标”中的项目，再保存策略。".into());return;}
            if selected.is_empty() || !selected.iter().any(|id|declared.contains(id)) {error.set("请至少勾选一个已接入此模型的上游 Key。".into());return;}
        }
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
    view!{<section class="admin-section route-policy-form"><p class="muted small">"为指定的本地模型选择服务商顺序、可用 Key 范围、重试与超时。只使用该模型已接入且勾选的 Key。"</p>
    <fieldset disabled=move||busy.get()><label for="routing-model">"选择本地模型"<select id="routing-model" prop:value=model on:change=move|e|load(event_target_value(&e))><option value="">"请选择模型"</option>{move||routes.get().into_iter().filter(|r|r.name!="auto").map(|r|view!{<option value=r.name.clone()>{r.name.clone()}</option>}).collect_view()}</select></label>
    <Show when=move||loaded.get().is_some()><h3>"服务商顺序与 Key 范围"</h3><label class="pick"><input type="checkbox" prop:checked=move||draft.get().ordered on:change=move|e|{let checked=event_target_checked(&e);draft.update(|d|{d.ordered=checked;if checked && d.targets.is_empty(){let name=loaded.get_untracked().unwrap_or_default();d.targets=routes.get_untracked().iter().find(|r|r.name==name).map(|r|r.declared_by.clone()).unwrap_or_default();}});}/>"单独设置此模型的服务商顺序与 Key 范围"</label>
    <p class="muted small">"未勾选时继承全局设置。勾选后可调整服务商整组的先后顺序，并选择组内可用 Key。未启用下方本地等权轮询时，同一服务商内部按渠道设置的 Key 策略、优先级及用户粘性选择，不按这里的 Key 展示顺序调用。已停用的渠道或 Key 不会参与请求。"</p>
    <Show when=move||draft.get().ordered>{move||{
        let name=loaded.get().unwrap_or_default();let ids=routes.get().iter().find(|r|r.name==name).map(|r|r.declared_by.clone()).unwrap_or_default();
        let available:Vec<_>=channels.get().into_iter().filter(|c|ids.contains(&c.id)).collect();
        let selected=draft.get().targets;
        let stale=stale_targets(&selected,&ids);
        let active_selected:Vec<_>=selected.iter().copied().filter(|id|ids.contains(id)).collect();
        let stale_notice=if stale.is_empty(){().into_any()}else{
            view!{<section class="admin-section" aria-labelledby="routing-stale-targets-title">
                <h4 id="routing-stale-targets-title">"失效的已选目标"</h4>
                <p id="routing-stale-targets-help" class="muted small">"这些目标已删除或不再接入当前模型，仍保留在已保存的范围中。移除只修改本模型的待保存选择；请选择当前来源并保存策略后生效。"</p>
                <ul>{stale.into_iter().map(|id|{
                    let label=stale_target_label(id,&channels.get(),&key_labels.get());
                    let action_label=format!("从本模型策略移除：{label}");
                    view!{<li><span>{label}</span><button type="button" class="ghost" aria-label=action_label aria-describedby="routing-stale-targets-help" on:click=move|_|{
                        let name=loaded.get_untracked().unwrap_or_default();
                        let declared=routes.get_untracked().iter().find(|r|r.name==name).map(|r|r.declared_by.clone()).unwrap_or_default();
                        draft.update(|d|{remove_stale_target(d,id,&declared);});
                        error.set(String::new());message.set("已移除待保存的失效目标；保存策略后生效。".into());
                    }>"移除失效目标"</button></li>}
                }).collect_view()}</ul>
            </section>}.into_any()
        };
        let mut group_keys:Vec<_>=selected_groups(&active_selected,&available).iter().map(|g|target_group(g[0],&available)).collect();
        for c in &available{let key=target_group(c.id,&available);if !group_keys.contains(&key){group_keys.push(key);}}
        view!{<div>{stale_notice}{group_keys.into_iter().map(|key|{
            let rows:Vec<_>=available.iter().filter(|c|target_group(c.id,&available)==key).cloned().collect();
            let first=rows.first().map(|c|c.id).unwrap_or(key.1);
            let provider=rows.first().and_then(|c|c.provider_name.clone()).unwrap_or_else(||"未分组上游".into());
            view!{<fieldset class="admin-section"><legend>{provider}</legend><div class="toolbar"><span class="muted small">"服务商顺序"</span><button type="button" class="ghost" disabled=move||{let selected=draft.get().targets;let c=channels.get();selected_groups(&selected,&c).iter().position(|g|target_group(g[0],&c)==key).map(|i|i==0).unwrap_or(true)} on:click=move|_|draft.update(|d|move_provider_group(&mut d.targets,&channels.get_untracked(),first,true))>"服务商上移"</button><button type="button" class="ghost" disabled=move||{let selected=draft.get().targets;let c=channels.get();let groups=selected_groups(&selected,&c);groups.iter().position(|g|target_group(g[0],&c)==key).map(|i|i+1==groups.len()).unwrap_or(true)} on:click=move|_|draft.update(|d|move_provider_group(&mut d.targets,&channels.get_untracked(),first,false))>"服务商下移"</button></div><div class="table-wrap"><table><thead><tr><th>"参与调用"</th><th>"上游 Key"</th></tr></thead><tbody>{rows.into_iter().map(|c|{
                let id=c.id;let provider_id=c.provider_id;let has_multiple_urls=channels.get_untracked().iter().filter(|other|other.provider_id==provider_id).map(|other|other.base_url.clone()).collect::<BTreeSet<_>>().len()>1;
                let endpoint=if has_multiple_urls{format!(" · {}",c.base_url)}else{String::new()};
                let label=format!("{}{}{}",key_labels.get().get(&id).cloned().unwrap_or_else(||"未命名上游 Key".into()),endpoint,if c.enabled&&c.provider_enabled{""}else{"（已停用）"});
                view!{<tr><td><input type="checkbox" aria-label=format!("参与调用：{label}") prop:checked=move||draft.get().targets.contains(&id) on:change=move|e|{let checked=event_target_checked(&e);draft.update(|d|select_target(&mut d.targets,&channels.get_untracked(),id,checked));}/></td><td>{label}</td></tr>}
            }).collect_view()}</tbody></table></div></fieldset>}
        }).collect_view()}</div>}
    }}</Show>
    <h3>"本地节点分配"</h3>
    <label for="routing-local-strategy">"分配方式"<select id="routing-local-strategy" aria-describedby="routing-local-strategy-help" prop:value=move||draft.get().local_strategy on:change=move|e|draft.update(|d|d.local_strategy=event_target_value(&e))><option value="">"继承现有策略"</option><option value="ordered">"按现有 Key 策略"</option><option value="round_robin">"等权轮询"</option></select></label>
    <p id="routing-local-strategy-help" class="muted small">"等权轮询只对本地服务中的同名向量与重排节点生效，在可用节点间依次分配请求；节点失败时自动尝试其他可用节点。在线服务、OAuth 与会话粘性保持原策略。"</p>
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
    #[test]
    fn local_strategy_roundtrips_without_losing_explicit_ordered() {
        for strategy in ["ordered", "round_robin"] {
            let value = json!({"local_strategy": strategy});
            assert_eq!(PolicyDraft::read(&value).payload().unwrap(), value);
        }
        assert_eq!(PolicyDraft::read(&json!({"local_strategy": null})).payload().unwrap(), json!({}));
    }
    #[test]
    fn invalid_local_strategy_never_creates_an_api_payload() {
        for strategy in [json!("random"), json!("ROUND_ROBIN"), json!(1), json!(false)] {
            assert!(PolicyDraft::read(&json!({"local_strategy": strategy})).payload().is_err());
        }
    }
    fn sources()->Vec<Channel>{vec![Channel{id:1,provider_id:Some(10),..Default::default()},Channel{id:2,provider_id:Some(10),..Default::default()},Channel{id:3,provider_id:Some(20),..Default::default()},Channel{id:4,provider_id:None,..Default::default()},Channel{id:5,provider_id:None,..Default::default()}]}
    #[test] fn provider_view_does_not_change_legacy_interleaved_order(){let ids=vec![1,3,2,4,5];assert_eq!(selected_groups(&ids,&sources()),vec![vec![1,2],vec![3],vec![4],vec![5]]);assert_eq!(ids,vec![1,3,2,4,5]);}
    #[test] fn moving_provider_moves_all_selected_keys_preserving_other_groups(){let mut ids=vec![1,3,2,4,5];move_provider_group(&mut ids,&sources(),3,true);assert_eq!(ids,vec![3,1,2,4,5]);move_provider_group(&mut ids,&sources(),4,false);assert_eq!(ids,vec![3,1,2,5,4]);let before=ids.clone();move_provider_group(&mut ids,&sources(),3,true);assert_eq!(ids,before);}
    #[test] fn adding_key_keeps_provider_keys_together_without_rewriting_others(){let mut ids=vec![1,3,4,5];select_target(&mut ids,&sources(),2,true);assert_eq!(ids,vec![1,2,3,4,5]);select_target(&mut ids,&sources(),2,true);assert_eq!(ids,vec![1,2,3,4,5]);select_target(&mut ids,&sources(),1,false);assert_eq!(ids,vec![2,3,4,5]);}

    #[test]
    fn stale_targets_remain_visible_until_explicitly_removed() {
        let selected = vec![2, 70, 56, 99];
        let declared = vec![56, 70];
        assert_eq!(stale_targets(&selected, &declared), vec![2, 99]);
        assert_eq!(selected, vec![2, 70, 56, 99]);
        assert!(stale_targets(&selected, &selected).is_empty());
    }

    #[test]
    fn removing_stale_targets_preserves_other_targets_and_policy_fields() {
        let original = json!({"channel_ids":[2,70,56,3,99],"retry_times":0,"reprobe_interval_ms":12345,"probe_enabled":true,"include_in_auto":false,"local_strategy":"ordered","attempt_timeout_ms":8000,"nonstream_timeout_ms":8000,"request_budget_ms":16000});
        let mut draft = PolicyDraft::read(&original);
        let declared = vec![56, 70, 3];
        assert!(remove_stale_target(&mut draft, 2, &declared));
        assert!(remove_stale_target(&mut draft, 99, &declared));
        assert!(!remove_stale_target(&mut draft, 56, &declared));
        assert!(!remove_stale_target(&mut draft, 101, &declared));
        let mut expected = original;
        expected["channel_ids"] = json!([70, 56, 3]);
        assert_eq!(draft.payload().unwrap(), expected);
    }

    #[test]
    fn removing_last_stale_target_does_not_enable_inherited_routing() {
        let mut draft = PolicyDraft::read(&json!({"channel_ids":[2],"attempt_timeout_ms":8000}));
        assert!(remove_stale_target(&mut draft, 2, &[56]));
        assert!(draft.ordered);
        assert!(draft.payload().is_err());
        select_target(&mut draft.targets, &[], 56, true);
        assert_eq!(draft.payload().unwrap(), json!({"channel_ids":[56],"attempt_timeout_ms":8000}));
    }

}
