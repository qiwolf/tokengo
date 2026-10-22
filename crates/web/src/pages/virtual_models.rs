//! Separate administrator-shared and user-private ordered/scheduled fallback editors.
use super::PageHead;
use crate::{Ctx, admin_api as api};
use leptos::prelude::*;
use serde_json::{Value, json};
fn compatibility_text(row:&Value)->String {
    let c=&row["compatibility"];
    let mut message=c["message"].as_str().unwrap_or("").to_string();
    for (key,label) in [("verified_at_ms","最近通过"),("expires_at_ms","有效至"),("next_retry_at_ms","下次重试")] {
        if let Some(ms)=c[key].as_i64().filter(|v|*v>0) {
            let date=js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
            message.push_str(&format!(" · {label} {}",date.to_locale_string("zh-CN",&wasm_bindgen::JsValue::UNDEFINED).as_string().unwrap_or_default()));
        }
    }
    message
}
fn fresh() -> Value {
    json!({"name":"","enabled":true,"mode":"ordered","timezone":"Asia/Shanghai","targets":[],"windows":[],"default_positions":[]})
}
fn new_target(option: &Value) -> Value {
    json!({"provider_id":option["provider_id"],"logical_model":option["logical_model"],"switch_on_429":true,"switch_on_400":false})
}
fn clock(minute: i64) -> String {
    format!("{:02}:{:02}", (minute % 1440) / 60, minute % 60)
}
fn minute(s: &str, end: bool) -> Option<i64> {
    let (h, m) = s.split_once(':')?;
    let h = h.parse::<i64>().ok()?;
    let m = m.parse::<i64>().ok()?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    let n = h * 60 + m;
    Some(if end && n == 0 { 1440 } else { n })
}
fn selected_positions(d: &Value) -> Vec<i64> {
    d.as_array()
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}
fn toggle_position(draft: RwSignal<Value>, window: Option<usize>, position: i64, checked: bool) {
    draft.update(|d| {
        let v = if let Some(i) = window {
            &mut d["windows"][i]["target_positions"]
        } else {
            &mut d["default_positions"]
        };
        let mut list = selected_positions(v);
        list.retain(|p| *p != position);
        if checked {
            list.push(position)
        }
        list.sort_unstable();
        *v = json!(list);
    });
}
fn rearrange(d: &mut Value, from: usize, to: Option<usize>) {
    let Some(a) = d["targets"].as_array_mut() else {
        return;
    };
    if from >= a.len() {
        return;
    }
    let n = a.len();
    let mut old = (0..n).collect::<Vec<_>>();
    let item = old.remove(from);
    if let Some(to) = to {
        old.insert(to.min(old.len()), item);
    }
    let old_targets = a.clone();
    *a = old.iter().map(|i| old_targets[*i].clone()).collect();
    let remap = |v: &Value| {
        let mut out = selected_positions(v)
            .into_iter()
            .filter_map(|p| {
                old.iter()
                    .position(|i| *i + 1 == p as usize)
                    .map(|n| n as i64 + 1)
            })
            .collect::<Vec<_>>();
        out.sort_unstable();
        json!(out)
    };
    d["default_positions"] = remap(&d["default_positions"]);
    if let Some(w) = d["windows"].as_array_mut() {
        for w in w {
            w["target_positions"] = remap(&w["target_positions"]);
        }
    }
}
fn submission(draft: &Value) -> Result<Value, String> {
    let mut d = json!({"name":draft["name"],"enabled":draft["enabled"],"mode":draft["mode"],"timezone":draft["timezone"],"targets":draft["targets"],"windows":draft["windows"],"default_positions":draft["default_positions"]});
    if let Some(v) = draft.get("version") {
        d["version"] = v.clone();
    }
    if d["name"].as_str().unwrap_or("").trim().is_empty() {
        return Err("请填写调用时使用的虚拟模型名。".into());
    }
    let a = d["targets"].as_array().ok_or("请添加调用目标。")?;
    if a.is_empty()
        || a.len() > 16
        || a.iter().any(|t| {
            t["provider_id"].as_i64().is_none()
                || t["logical_model"].as_str().is_none_or(str::is_empty)
        })
    {
        return Err("请选择1至16个已获授权的供应方与模型。".into());
    }
    if d["mode"] == "ordered" {
        d["windows"] = json!([]);
        d["default_positions"] = json!([]);
        return Ok(d);
    }
    let mut windows = vec![];
    for w in api::rows(&d, "windows") {
        let start = w["start_minute"].as_i64().ok_or("请选择开始时间。")?;
        let end = w["end_minute"].as_i64().ok_or("请选择结束时间。")?;
        if start == end {
            return Err("时段开始和结束不能相同。".into());
        }
        if start > end {
            let mut first = w.clone();
            first["end_minute"] = json!(1440);
            let mut next = w;
            next["start_minute"] = json!(0);
            let mask = next["days_mask"].as_i64().unwrap_or(0);
            next["days_mask"] = json!(((mask << 1) & 127) | (mask >> 6));
            windows.push(first);
            windows.push(next);
        } else {
            windows.push(w);
        }
    }
    d["windows"] = json!(windows);
    if d["mode"] == "ordered" {
        d["windows"] = json!([]);
        d["default_positions"] = json!([]);
    }
    Ok(d)
}
/// The PATCH endpoint replaces the full config; re-enabling must retain its targets and revision.
fn reenable(row: &Value) -> Result<Value, String> {
    let mut config = row.clone();
    config["enabled"] = json!(true);
    submission(&config)
}
#[component]
pub fn VirtualModels(ctx: Ctx, #[prop(default=false)] shared:bool) -> impl IntoView {
    if shared && !ctx.is_root() {return view!{<div class="page"><PageHead title="共享虚拟模型" desc="共享调用策略由管理员维护；请在我的空间查看已授权的调用名称。"/></div>}.into_any();}
    if !shared && ctx.is_root() {
        return view!{<div class="page"><PageHead title="调用策略" desc="调用策略属于具体用户；请使用普通用户账号登录个人空间配置。"/></div>}.into_any();
    }
    let api_base=if shared{"/api/shared-virtual-models"}else{"/api/account/virtual-models"};
    let title=if shared{"共享虚拟模型"}else{"调用策略"};
    let description=if shared{"维护统一调用入口并向用户分配访问权限。底层模型可设为仅内部调用，用户只需使用共享名称。"}else{"给模型组合起一个私有名字，按顺序或时段选择调用目标。每次尝试都受你的权限、Key 和总额度限制。"};
    let show_trash=RwSignal::new(false);
    let delete_open=RwSignal::new(false);
    let delete_target=RwSignal::new(Value::Null);
    let rows = RwSignal::new(Vec::<Value>::new());
    let options = RwSignal::new(Vec::<Value>::new());
    let selected = RwSignal::new(None::<i64>);
    let editor_open = RwSignal::new(false);
    let draft = RwSignal::new(fresh());
    let busy = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let revision = RwSignal::new(0u64);
    let timer=crate::every(30_000,move||{if !busy.get_untracked()&&!loading.get_untracked(){revision.update(|r|*r+=1);}});
    on_cleanup(move||crate::stop_timer(timer));
    Effect::new(move |_| {
        revision.get();
        let in_trash=show_trash.get();
        loading.set(true);
        leptos::task::spawn_local(async move {
            match api::request("GET", &if in_trash{format!("{api_base}?trash=true")}else{api_base.into()}, None).await {
                Ok(v) => {
                    if v["virtual_models"].is_array() && v["options"].is_array() {
                        rows.set(api::rows(&v, "virtual_models"));
                        options.set(api::rows(&v, "options"));
                    } else {
                        error.set("策略接口数据不完整，请刷新后重试。".into());
                    }
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            loading.set(false);
        });
    });
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let body = match submission(&draft.get_untracked()) {
            Ok(mut d) => {
                for k in [
                    "id",
                    "user_id",
                    "created_ms",
                    "updated_ms",
                    "normalized_name",
                ] {
                    d.as_object_mut().unwrap().remove(k);
                }
                if selected.get_untracked().is_none() {
                    d.as_object_mut().unwrap().remove("version");
                }
                d
            }
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let id = selected.get_untracked();
        busy.set(true);
        error.set(String::new());
        notice.set(String::new());
        leptos::task::spawn_local(async move {
            let path = id
                .map(|i| format!("{api_base}/{i}"))
                .unwrap_or_else(|| api_base.into());
            match api::request(
                if id.is_some() { "PATCH" } else { "POST" },
                &path,
                Some(body),
            )
            .await
            {
                Ok(v) => {
                    let row = v.get("virtual_model").cloned().unwrap_or(v);
                    if row["id"].as_i64().is_some() {
                        selected.set(row["id"].as_i64());
                        draft.set(row);
                        notice.set(if shared{"共享模型已保存，请在用户页面分配该名称的访问权限。"}else{"调用策略已保存；使用本页的虚拟模型名发起调用。"}.into());
                        editor_open.set(false);
                        show_trash.set(false);
                        revision.update(|r| *r += 1);
                    } else {
                        error.set("保存结果缺少策略身份，请刷新确认，勿重复创建。".into());
                    }
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view!{<div class="page admin-page virtual-model-page"><PageHead title=title desc=description><button type="button" class="primary" disabled=move||busy.get()||loading.get() on:click=move |_|{selected.set(None);draft.set(fresh());error.set(String::new());notice.set(String::new());editor_open.set(true);}>"新建虚拟模型"</button></PageHead>
 <section class="card"><header class="card-head"><h2>{move||if show_trash.get(){"虚拟模型回收站"}else if shared{"管理员共享模型"}else{"我的虚拟模型"}}</h2><div class="account-actions"><button type="button" class="ghost" disabled=move||busy.get()||loading.get() on:click=move |_|{show_trash.update(|v|*v=!*v);error.set(String::new());notice.set(String::new());}>{move||if show_trash.get(){"返回模型列表"}else{"回收站"}}</button><button type="button" class="ghost" disabled=move||busy.get()||loading.get() on:click=move |_|revision.update(|r|*r+=1)>"刷新"</button></div></header><div class="card-body"><p role="status">{move||if loading.get(){"正在读取调用策略…"}else if rows.get().is_empty(){if show_trash.get(){"回收站为空。删除的虚拟模型会显示在这里。"}else{"还没有虚拟模型。点击新建虚拟模型，添加供应方与模型即可开始。"}}else{""}}</p><Show when=move||show_trash.get()><p class="account-help">"删除后停止调用，历史请求和审计保留。恢复后保持停用；同名入口已被重新创建时，不能覆盖恢复。"</p></Show><div class="table-wrap"><table><thead><tr><th>"调用模型名"</th><th>"策略 / 状态"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|row|{
 let edit_row=row.clone();let disable_row=row.clone();let trash_row=row.clone();let restore_row=row.clone();let row_enabled=row["enabled"]==true;let row_deleted=row["deleted_at_ms"].as_i64().is_some();let name=api::text(&row,"name");let compat=compatibility_text(&row);let compat_failed=row["compatibility"]["state"]=="failed";let label=format!("{} · {} 个目标 · {}",if row["mode"]=="scheduled"{"按时段"}else{"按顺序"},api::rows(&row,"targets").len(),if row_deleted{"已删除"}else if row_enabled{"启用"}else{"停用"});
 view!{<tr><td data-label="模型名"><code>{name}</code></td><td data-label="策略">{label}<p role=if compat_failed{"alert"}else{"status"}>{compat}</p></td><td data-label="操作">{if row_deleted{
 view!{<button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{if busy.get_untracked(){return}let Some(id)=restore_row["id"].as_i64()else{return};let version=restore_row["version"].clone();busy.set(true);error.set(String::new());notice.set(String::new());leptos::task::spawn_local(async move{match api::request("POST",&format!("{api_base}/{id}/restore"),Some(json!({"version":version}))).await{Ok(_)=>{notice.set("虚拟模型已恢复为停用状态，请返回模型列表确认后启用。".into());revision.update(|r|*r+=1);},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});}>"恢复"</button>}.into_any()
 }else{
 view!{<div class="account-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{selected.set(edit_row["id"].as_i64());draft.set(edit_row.clone());error.set(String::new());notice.set(String::new());editor_open.set(true);}>"编辑"</button><button type="button" class=if row_enabled{"ghost danger"}else{"ghost"} disabled=move||busy.get() on:click=move |_|{if busy.get_untracked(){return}let Some(id)=disable_row["id"].as_i64()else{return};let version=disable_row["version"].clone();let active=disable_row["enabled"]==true;let enable_body=if active{None}else{match reenable(&disable_row){Ok(v)=>Some(v),Err(e)=>{error.set(e);return}}};busy.set(true);error.set(String::new());notice.set(String::new());leptos::task::spawn_local(async move{match api::request(if active{"DELETE"}else{"PATCH"},&format!("{api_base}/{id}"),Some(if active{json!({"version":version})}else{enable_body.unwrap()})).await{Ok(v)=>{if selected.get_untracked()==Some(id){draft.set(v["virtual_model"].clone());}notice.set(if active{"虚拟模型已停用，历史与配置保留。"}else{"虚拟模型已启用。"}.into());revision.update(|r|*r+=1);},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});}>{if row_enabled{"停用"}else{"启用"}}</button><button type="button" class="ghost danger" disabled=move||busy.get() on:click=move |_|{delete_target.set(trash_row.clone());error.set(String::new());delete_open.set(true);}>"删除"</button></div>}.into_any()
 }}</td></tr>}
 }).collect_view()}</tbody></table></div></div></section>
 <super::ModuleDialog open=delete_open busy=busy title="删除虚拟模型".to_string()><p>{move||format!("将 {} 移到回收站？删除后立即停止调用，历史请求和审计保留，可以恢复。",api::text(&delete_target.get(),"name"))}</p><p role="alert">{move||error.get()}</p><div class="account-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|delete_open.set(false)>"取消"</button><button type="button" class="danger" disabled=move||busy.get() on:click=move |_|{if busy.get_untracked(){return}let row=delete_target.get_untracked();let Some(id)=row["id"].as_i64()else{return};busy.set(true);error.set(String::new());leptos::task::spawn_local(async move{match api::request("POST",&format!("{api_base}/{id}/trash"),Some(json!({"version":row["version"]}))).await{Ok(_)=>{delete_open.set(false);notice.set("虚拟模型已移到回收站，调用已停止，可在回收站恢复。".into());revision.update(|r|*r+=1);},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});}>{move||if busy.get(){"正在移到回收站…"}else{"移到回收站"}}</button></div></super::ModuleDialog>
 <p role="status">{move||if !editor_open.get(){notice.get()}else{String::new()}}</p><p role="alert">{move||if !editor_open.get(){error.get()}else{String::new()}}</p><super::ModuleDialog open=editor_open busy=busy title="调用策略设置".to_string()><h3>{move||if selected.get().is_some(){"编辑调用策略"}else{"新建调用策略"}}</h3><form class="account-form" on:submit=save><fieldset disabled=move||busy.get()||loading.get()><div class="policy-limits"><label class="account-field"><span>"虚拟模型名"</span><input type="text" required maxlength="128" placeholder="如 my-coding" prop:value=move||draft.get()["name"].as_str().unwrap_or("").to_string() on:input=move|e|draft.update(|d|d["name"]=json!(event_target_value(&e)))/></label><label class="account-field"><span>"调用方式"</span><select prop:value=move||draft.get()["mode"].as_str().unwrap_or("ordered").to_string() on:change=move|e|draft.update(|d|d["mode"]=json!(event_target_value(&e)))><option value="ordered">"按1、2、3、4顺序切换"</option><option value="scheduled">"按每周时段选择顺序"</option></select></label></div>
 <label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["enabled"]==true on:change=move|e|draft.update(|d|d["enabled"]=json!(event_target_checked(&e)))/>"启用此虚拟模型"</label><h3>"调用目标"</h3><p class="account-help">"每个目标选择供应方与实际模型。429/400开关控制对应错误是否切到下一目标。向量与重排多目标保存时会自动检查兼容性，通过后支持网络、超时、5xx及无效响应回退；请求参数和输入长度须在已验证范围内。其他模型维持原有规则，流式输出开始后不会重放。"</p>
 {move||api::rows(&draft.get(),"targets").into_iter().enumerate().map(|(index,target)|{let position=index+1;view!{<section class="virtual-target panel"><header class="card-head"><h3>{format!("第{position}步")}</h3><div class="account-actions"><button type="button" class="ghost" aria-label=format!("上移第{position}步") disabled=move||index==0 on:click=move |_|draft.update(|d|rearrange(d,index,Some(index-1)))>"上移"</button><button type="button" class="ghost" aria-label=format!("下移第{position}步") disabled=move||{index+1>=api::rows(&draft.get(),"targets").len()} on:click=move |_|draft.update(|d|rearrange(d,index,Some(index+1)))>"下移"</button><button type="button" class="ghost danger" aria-label=format!("移除第{position}步") on:click=move |_|draft.update(|d|rearrange(d,index,None))>"移除"</button></div></header><label class="account-field"><span>"供应方与模型"</span><select prop:value=move||options.get().iter().position(|o|o["provider_id"]==draft.get()["targets"][index]["provider_id"]&&o["logical_model"]==draft.get()["targets"][index]["logical_model"]).map(|n|n.to_string()).unwrap_or_default() on:change=move|e|{if let Ok(n)=event_target_value(&e).parse::<usize>(){if let Some(o)=options.get_untracked().get(n){draft.update(|d|{d["targets"][index]["provider_id"]=o["provider_id"].clone();d["targets"][index]["logical_model"]=o["logical_model"].clone();})}}}><option value="">"选择已授权目标"</option>{move||options.get().into_iter().enumerate().map(|(i,o)|view!{<option value=i.to_string()>{format!("{} · {}",api::text(&o,"provider_name"),api::text(&o,"logical_model"))}</option>}).collect_view()}</select></label><div class="account-actions"><label class="account-check"><input type="checkbox" prop:checked=target["switch_on_429"]==true on:change=move|e|draft.update(|d|d["targets"][index]["switch_on_429"]=json!(event_target_checked(&e)))/>"429时切下一目标"</label><label class="account-check"><input type="checkbox" prop:checked=target["switch_on_400"]==true on:change=move|e|draft.update(|d|d["targets"][index]["switch_on_400"]=json!(event_target_checked(&e)))/>"400时切下一目标"</label></div></section>}}).collect_view()}
 <button type="button" class="ghost" disabled=move||options.get().is_empty()||(api::rows(&draft.get(),"targets").len()>=16) on:click=move |_|{if let Some(o)=options.get_untracked().first(){draft.update(|d|{let mut targets=api::rows(d,"targets");targets.push(new_target(o));let n=targets.len();d["targets"]=json!(targets);let mut defaults=selected_positions(&d["default_positions"]);defaults.push(n as i64);d["default_positions"]=json!(defaults);})}}>"添加调用目标"</button>
 <Show when=move||draft.get()["mode"]=="scheduled"><h3>"每周时段"</h3><label class="account-field"><span>"时区"</span><select prop:value=move||draft.get()["timezone"].as_str().unwrap_or("Asia/Shanghai").to_string() on:change=move|e|draft.update(|d|d["timezone"]=json!(event_target_value(&e)))><option value="Asia/Shanghai">"北京时间 UTC+8"</option><option value="UTC">"UTC"</option></select></label><p class="account-help">"开始时间包含、结束时间不包含；结束00:00表示当日24:00。跨午夜自动拆分为两段；重叠时段不能保存。开始新尝试时复核时段，已开始的推理不会到点中断。"</p>
 {move||api::rows(&draft.get(),"windows").into_iter().enumerate().map(|(i,w)|view!{<section class="panel"><header class="card-head"><h3>{format!("时段{}",i+1)}</h3><button type="button" class="ghost danger" on:click=move |_|draft.update(|d|{let mut a=api::rows(d,"windows");a.remove(i);d["windows"]=json!(a);})>"移除此时段"</button></header><div class="policy-options">{["周一","周二","周三","周四","周五","周六","周日"].into_iter().enumerate().map(|(day,label)|view!{<label class="account-check"><input type="checkbox" prop:checked=w["days_mask"].as_i64().unwrap_or(0)&(1<<day)!=0 on:change=move|e|draft.update(|d|{let mask=d["windows"][i]["days_mask"].as_i64().unwrap_or(0);d["windows"][i]["days_mask"]=json!(if event_target_checked(&e){mask|(1<<day)}else{mask&!(1<<day)});})/>{label}</label>}).collect_view()}</div><div class="policy-limits"><label class="account-field"><span>"开始"</span><input type="time" required prop:value=clock(w["start_minute"].as_i64().unwrap_or(0)) on:input=move|e|{if let Some(n)=minute(&event_target_value(&e),false){draft.update(|d|d["windows"][i]["start_minute"]=json!(n))}}/></label><label class="account-field"><span>"结束"</span><input type="time" required prop:value=clock(w["end_minute"].as_i64().unwrap_or(1440)) on:input=move|e|{if let Some(n)=minute(&event_target_value(&e),true){draft.update(|d|d["windows"][i]["end_minute"]=json!(n))}}/></label></div><p>"此时段按以下步骤顺序调用："</p><div class="policy-options">{move||api::rows(&draft.get(),"targets").iter().enumerate().map(|(n,t)|{let checked=selected_positions(&draft.get()["windows"][i]["target_positions"]).contains(&(n as i64+1));view!{<label class="account-check"><input type="checkbox" prop:checked=checked on:change=move|e|toggle_position(draft,Some(i),n as i64+1,event_target_checked(&e))/>{format!("第{}步 · {}",n+1,api::text(t,"logical_model"))}</label>}}).collect_view()}</div></section>}).collect_view()}
 <button type="button" class="ghost" on:click=move |_|draft.update(|d|{let mut w=api::rows(d,"windows");w.push(json!({"days_mask":127,"start_minute":0,"end_minute":1440,"target_positions":[1]}));d["windows"]=json!(w);})>"添加时段"</button><h3>"无时段命中时"</h3><p class="account-help">"勾选默认调用顺序；全部不选则拒绝调用，不会擅自换到其他模型。"</p><div class="policy-options">{move||api::rows(&draft.get(),"targets").iter().enumerate().map(|(n,t)|{let checked=selected_positions(&draft.get()["default_positions"]).contains(&(n as i64+1));view!{<label class="account-check"><input type="checkbox" prop:checked=checked on:change=move|e|toggle_position(draft,None,n as i64+1,event_target_checked(&e))/>{format!("第{}步 · {}",n+1,api::text(t,"logical_model"))}</label>}}).collect_view()}</div></Show>
 <p role="alert">{move||error.get()}</p><p role="status">{move||notice.get()}</p><div class="account-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|editor_open.set(false)>"取消"</button><button class="primary" type="submit">{move||if busy.get(){"正在保存并检查目标…"}else{"保存调用策略"}}</button></div></fieldset></form></super::ModuleDialog></div>}.into_any()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reenabling_preserves_full_scheduled_config_and_revision() {
        let row = json!({"id":9,"user_id":2,"version":7,"name":"mine","enabled":false,"mode":"scheduled","timezone":"UTC","targets":[{"provider_id":1,"logical_model":"a","switch_on_429":true,"switch_on_400":false}],"windows":[{"days_mask":127,"start_minute":0,"end_minute":1440,"target_positions":[1]}],"default_positions":[1]});
        let body = reenable(&row).unwrap();
        assert_eq!(body["enabled"], true);
        for key in ["version", "name", "mode", "timezone", "targets", "windows", "default_positions"] {
            assert_eq!(body[key], row[key], "{key}");
        }
        assert!(body.get("id").is_none());
        assert!(body.get("user_id").is_none());
    }

    #[test]
    fn shared_submission_strips_administrator_metadata_and_keeps_revision() {
        let row=json!({"id":3,"version":8,"actor_account_id":1,"kind":"embeddings","name":"shared-fallback","enabled":true,"mode":"ordered","timezone":"UTC","targets":[{"provider_id":2,"logical_model":"cloud-vector","switch_on_429":true,"switch_on_400":false}],"windows":[],"default_positions":[]});
        let body=submission(&row).unwrap();assert_eq!(body["version"],8);assert_eq!(body["name"],"shared-fallback");
        assert!(body.as_object().unwrap().keys().all(|key|["name","enabled","mode","timezone","targets","windows","default_positions","version"].contains(&key.as_str())));
    }
    #[test]
    fn switching_to_ordered_ignores_unfinished_hidden_windows() {
        let d = json!({"name":"mine","enabled":true,"mode":"ordered","timezone":"UTC","targets":[{"provider_id":1,"logical_model":"a"}],"windows":[{"start_minute":1,"end_minute":1}],"default_positions":[1]});
        let b = submission(&d).unwrap();
        assert_eq!(b["windows"], json!([]));
        assert_eq!(b["default_positions"], json!([]));
    }

    #[test]
    fn moving_targets_keeps_window_identity_but_uses_new_order() {
        let mut d = json!({"targets":[{"logical_model":"a"},{"logical_model":"b"}],"windows":[{"target_positions":[1]}],"default_positions":[1,2]});
        rearrange(&mut d, 0, Some(1));
        assert_eq!(d["targets"][0]["logical_model"], "b");
        assert_eq!(d["windows"][0]["target_positions"], json!([2]));
        assert_eq!(d["default_positions"], json!([1, 2]));
    }
    #[test]
    fn midnight_windows_split_and_shift_weekday() {
        let d = json!({"name":"mine","mode":"scheduled","targets":[{"provider_id":1,"logical_model":"a"}],"windows":[{"days_mask":64,"start_minute":1320,"end_minute":360,"target_positions":[1]}]});
        let b = submission(&d).unwrap();
        assert_eq!(b["windows"][0]["end_minute"], 1440);
        assert_eq!(b["windows"][1]["days_mask"], 1);
        assert_eq!(b["windows"][1]["start_minute"], 0);
    }
}
