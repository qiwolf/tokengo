use super::policy_editor::{self as policy, PolicyEditor};
use crate::{Ctx, admin_api as api, fmt};
use leptos::prelude::*;
use serde_json::{Value, json};
fn key_copy_notice(copied: bool) -> &'static str {
    if copied { "完整 API Key 已复制到系统剪贴板，请安全保存。" }
    else { "自动复制未成功。请点击“复制 Key”重试，或选中上方完整 Key 手动复制；关闭后无法再次查看。" }
}
/// A status change only changes enabled; credentials, policy and limits remain untouched.
fn status_patch(key: &Value, personal: bool) -> Result<(i64, Value, bool), String> {
    if !key["revoked_at"].is_null() {return Err("此 Key 已永久撤销，不能恢复；请新建 Key。".into());}
    if personal && key["enabled"]!=true && key["can_enable"]!=true {return Err("此 Key 不能自行恢复：可能由管理员停用、已到期或上级 Key 不可用，请联系管理员。".into());}
    let id=key["id"].as_i64().filter(|id|*id>0).ok_or("Key 身份无效，请刷新后重试。")?;
    let enabled=key["enabled"]!=true;
    Ok((id,json!({"enabled":enabled}),enabled))
}
fn key_path(personal: bool,id:i64)->String {
    if personal {format!("/api/account/keys/{id}")}else{format!("/api/vkeys/{id}")}
}
fn new_key_policy(base: Value, personal: bool) -> Value {
    if !personal { return base; }
    // Parent grants are inherited, not historical explicit grants of a new Key.
    // The existing save path materializes that inheritance within the parent's bounds.
    let mut inherited = base; inherited["allowed_models"] = Value::Null; inherited["allowed_services"]=Value::Null; inherited
}
/// A Key task replaces its host panel when embedded, rather than stacking dialogs.
#[component]
fn KeyEditorSurface(open: RwSignal<bool>, busy: RwSignal<bool>, embedded: bool, children: ChildrenFn) -> impl IntoView {
    let children = StoredValue::new(children);
    if embedded {
        view!{<Show when=move||open.get()><div class="user-key-editor"><header class="user-key-editor-head"><h3>"API Key 设置"</h3><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|open.set(false)>"返回 API Key 列表"</button></header>{move||children.with_value(|render| render())}</div></Show>}.into_any()
    } else {
        view!{<super::ModuleDialog open=open busy=busy title="API Key 设置".to_string()>{children.with_value(|render| render())}</super::ModuleDialog>}.into_any()
    }
}
#[component]
pub fn KeyManager(
    ctx: Ctx,
    owner: RwSignal<Option<i64>>,
    models: RwSignal<Vec<String>>,
    channels: RwSignal<Vec<(i64, String)>>,
    services: RwSignal<Vec<Value>>,
    #[prop(default = false)] personal: bool,
    #[prop(default = false)] embedded: bool,
    #[prop(optional)] editor_active: Option<RwSignal<bool>>,
    #[prop(optional)] form_busy: Option<RwSignal<bool>>,
) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Value>::new());
    let selected = RwSignal::new(None::<Value>);
    let editor_open = RwSignal::new(false);
    Effect::new(move |_| { if let Some(active)=editor_active { active.set(editor_open.get()); } });
    on_cleanup(move || { if let Some(active)=editor_active { active.set(false); } });
    let name = RwSignal::new(String::new());
    let draft = RwSignal::new(policy::default_policy());
    let base = RwSignal::new(policy::default_policy());
    let expiry = RwSignal::new(String::new());
    let limit = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let busy = form_busy.unwrap_or_else(|| RwSignal::new(false));
    let generation = RwSignal::new(0u64);
    let loading = RwSignal::new(false);
    let secret = RwSignal::new(None::<String>);
    let copy_note = RwSignal::new(String::new());
    let copy_failed = RwSignal::new(false);
    let revision = RwSignal::new(0u64);
    let delete_open = RwSignal::new(false);
    let delete_target = RwSignal::new(None::<Value>);
    let delete_error = RwSignal::new(String::new());
    let user_budget = RwSignal::new(Value::Null);
    Effect::new(move |_| {
        let id = owner.get();
        revision.get();
        let token = generation.get_untracked() + 1;
        generation.set(token);
        secret.set(None);
        delete_open.set(false);
        delete_target.set(None);
        if personal {services.set(Vec::new());}
        rows.set(vec![]);
        selected.set(None);
        editor_open.set(false);
        name.set(String::new());
        expiry.set(String::new());
        limit.set(String::new());
        error.set(String::new());
        notice.set(String::new());
        if !personal && id.is_none() {
            loading.set(false);
            return;
        }
        loading.set(true);
        leptos::task::spawn_local(async move {
            let path = if personal {
                "/api/account/keys".into()
            } else {
                format!("/api/users/{}/keys", id.unwrap())
            };
            match api::request("GET", &path, None).await {
                Ok(v) => {
                    if owner.get_untracked() == id && generation.get_untracked() == token {
                        let mut loaded = api::rows(&v, "keys");
                        if !personal {
                            for row in &mut loaded {
                                let key_id = row["id"].as_i64().unwrap_or(0);
                                match api::request(
                                    "GET",
                                    &format!("/api/budgets?scope=key&scope_id={key_id}"),
                                    None,
                                )
                                .await
                                {
                                    Ok(b) => {
                                        if owner.get_untracked() != id
                                            || generation.get_untracked() != token
                                        {
                                            return;
                                        }
                                        row["budget"] = api::rows(&b, "budgets")
                                            .into_iter()
                                            .find(|b| b["scope_id"] == key_id)
                                            .unwrap_or(Value::Null);
                                    }
                                    Err(e) => {
                                        if owner.get_untracked() == id
                                            && generation.get_untracked() == token
                                        {
                                            error.set(format!(
                                                "密钥额度读取失败：{}。请刷新，未覆盖旧额度。",
                                                e.message
                                            ));
                                            loading.set(false);
                                        }
                                        return;
                                    }
                                }
                            }
                        }
                        if owner.get_untracked() != id || generation.get_untracked() != token {
                            return;
                        }
                        rows.set(loaded);
                        if personal {
                            services.set(v["services"].as_array().cloned().unwrap_or_default());
                            base.set(v["policy"].clone());
                            draft.set(v["policy"].clone());
                            user_budget.set(v["budget"].clone());
                            limit.set(
                                v["budget"]["limit_micro"]
                                    .as_i64()
                                    .map(policy::money_input)
                                    .unwrap_or_default(),
                            );
                            // This account endpoint already applies model_routes.visible and user permissions.
                            models.set(
                                v["models"]
                                    .as_array()
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(|s| s.as_str().map(str::to_owned))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                            );
                        } else {
                            base.set(policy::default_policy());
                            draft.set(policy::default_policy());
                        }
                    }
                }
                Err(e) => {
                    if owner.get_untracked() == id && generation.get_untracked() == token {
                        ctx.note_err(&e);
                        error.set(e.message);
                    }
                }
            }
            if owner.get_untracked() == id && generation.get_untracked() == token {
                loading.set(false);
            }
        });
    });
    Effect::new(move |_| {
        if !editor_open.get() { secret.set(None); }
    });
    let reset = move || {
        selected.set(None);
        name.set(String::new());
        draft.set(new_key_policy(base.get_untracked(),personal));
        expiry.set(String::new());
        limit.set(if personal {
            user_budget.get_untracked()["limit_micro"]
                .as_i64()
                .map(policy::money_input)
                .unwrap_or_default()
        } else {
            String::new()
        });
        secret.set(None);
        error.set(String::new());
        notice.set(String::new());
    };
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let result = (|| -> Result<Value, String> {
            policy::validate(&draft.get_untracked())?;
            if name.get_untracked().trim().is_empty() {
                return Err("请填写密钥名称。".into());
            }
            let exp = policy::expiry_value(&expiry.get_untracked())?;
            let cap = policy::optional_limit(&limit.get_untracked())?;
            let mut v = json!({"name":name.get_untracked().trim(),"policy":if personal{policy::inherit_policy(draft.get_untracked(),&base.get_untracked())}else{draft.get_untracked()},"expires_at":exp});
            if personal {
                v["limit_micro"] = json!(cap);
            }
            Ok(v)
        })();
        let body = match result {
            Ok(v) => v,
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let id = selected.get_untracked().and_then(|k| k["id"].as_i64());
        let owner_id = owner.get_untracked();
        let token = generation.get_untracked();
        let captured_cap = policy::optional_limit(&limit.get_untracked()).unwrap_or(None);
        let previous_budget = selected
            .get_untracked()
            .map(|k| k["budget"].clone())
            .unwrap_or(Value::Null);
        if !personal && owner_id.is_none() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        notice.set(String::new());
        secret.set(None);
        leptos::task::spawn_local(async move {
            let path = if personal {
                id.map(|i| format!("/api/account/keys/{i}"))
                    .unwrap_or_else(|| "/api/account/keys".into())
            } else {
                id.map(|i| format!("/api/vkeys/{i}"))
                    .unwrap_or_else(|| format!("/api/users/{}/keys", owner_id.unwrap()))
            };
            match api::request(
                if id.is_some() { "PATCH" } else { "POST" },
                &path,
                Some(body),
            )
            .await
            {
                Ok(v) => {
                    let mut row = v["key"].clone();
                    let new_secret = v["secret"].as_str().map(str::to_owned);
                    if !personal {
                        if let Some(k) = row["id"].as_i64() {
                            let cap = captured_cap;
                            match api::request(
                                "PUT",
                                &format!("/api/budgets?scope=key&scope_id={k}"),
                                Some(json!({"limit_micro":cap})),
                            )
                            .await
                            {
                                Ok(b) => {
                                    row["budget"] = api::rows(&b, "budgets")
                                        .into_iter()
                                        .find(|b| b["scope_id"] == k)
                                        .unwrap_or(Value::Null);
                                }
                                Err(e) => {
                                    row["budget"] = previous_budget;
                                    if owner.get_untracked() == owner_id
                                        && generation.get_untracked() == token
                                    {
                                        error.set(format!("密钥已保存，但费用上限未保存：{}。保留旧限额，请保管刚生成的密钥后刷新重试额度。",e.message));
                                    }
                                }
                            }
                        }
                    }
                    if owner.get_untracked() != owner_id || generation.get_untracked() != token {
                        busy.set(false);
                        return;
                    }
                    let has_new_secret = new_secret.is_some();
                    secret.set(new_secret.clone());
                    copy_note.set(String::new());
                    copy_failed.set(false);
                    rows.update(|r| {
                        r.retain(|k| k["id"] != row["id"]);
                        r.insert(0, row.clone());
                    });
                    selected.set(Some(row));
                    notice.set(if has_new_secret {"密钥已创建，请立即保存明文。"} else {"密钥设置已保存。"}.into());
                    if !has_new_secret && error.get_untracked().is_empty() { editor_open.set(false); }
                    if let Some(value) = new_secret {
                        let copied = super::settings::copy_text(&value).await;
                        if owner.get_untracked() == owner_id && generation.get_untracked() == token && secret.get_untracked().as_ref() == Some(&value) {
                            copy_failed.set(!copied);
                            copy_note.set(key_copy_notice(copied).into());
                        }
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
    let delete_key = move |_| {
        if busy.get_untracked() { return; }
        let Some(key) = delete_target.get_untracked() else { return; };
        let Some(id) = key["id"].as_i64() else { return; };
        if key["revoked_at"].is_null() { delete_error.set("请先永久撤销此 Key，再删除。".into()); return; }
        let owner_id = owner.get_untracked();
        let token = generation.get_untracked();
        busy.set(true); delete_error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("POST", &format!("{}/delete",key_path(personal,id)), Some(json!({}))).await {
                Ok(v) => {
                    if owner.get_untracked()==owner_id && generation.get_untracked()==token {
                        if v["deleted"]==true && v["id"].as_i64()==Some(id) {
                            rows.update(|r|r.retain(|k|k["id"].as_i64()!=Some(id)));
                            delete_open.set(false); delete_target.set(None);
                            notice.set("已删除撤销的 Key；调用历史与审计记录保留。".into());
                        } else { delete_error.set("删除结果不完整，请刷新列表确认。".into()); }
                    }
                }
                Err(e) => {
                    if owner.get_untracked()==owner_id && generation.get_untracked()==token {ctx.note_err(&e);delete_error.set(e.message);}
                }
            }
            busy.set(false);
        });
    };
    let toggle_enabled=move |key:Value| {
        if busy.get_untracked(){return;}
        let (id,patch,enabled)=match status_patch(&key,personal){Ok(v)=>v,Err(e)=>{error.set(e);return;}};
        let owner_id=owner.get_untracked();let token=generation.get_untracked();
        busy.set(true);error.set(String::new());notice.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("PATCH",&key_path(personal,id),Some(patch)).await {
                Ok(v)=>{
                    if owner.get_untracked()==owner_id&&generation.get_untracked()==token {
                        let mut row=v["key"].clone();
                        if row["id"].as_i64()!=Some(id){error.set("状态更新结果不完整，请刷新确认。".into());}
                        else {
                            if row["budget"].is_null(){row["budget"]=key["budget"].clone();}
                            rows.update(|r|{if let Some(existing)=r.iter_mut().find(|k|k["id"].as_i64()==Some(id)){*existing=row.clone();}});
                            notice.set(if enabled{"Key 已启用；仍受用户权限、到期时间和额度限制。"}else{"Key 已停用，可随时重新启用。"}.into());
                        }
                    }
                },
                Err(e)=>{if owner.get_untracked()==owner_id&&generation.get_untracked()==token{ctx.note_err(&e);error.set(e.message);}},
            }
            busy.set(false);
        });
    };
    let revoke = move |_| {
        if busy.get_untracked() { return; }
        let Some(k) = selected.get_untracked() else {
            return;
        };
        let Some(id) = k["id"].as_i64() else { return };
        if !web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(
                    "永久撤销此 API Key？使用它及其子密钥的客户端将无法继续调用，且不能恢复。",
                )
                .ok()
            })
            .unwrap_or(false)
        {
            return;
        }
        let owner_id=owner.get_untracked();
        let token=generation.get_untracked();
        busy.set(true);
        secret.set(None);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            let path = if personal {
                format!("/api/account/keys/{id}")
            } else {
                format!("/api/vkeys/{id}")
            };
            match api::request("PATCH", &path, Some(json!({"revoke":true}))).await {
                Ok(v) => {
                    if owner.get_untracked()!=owner_id || generation.get_untracked()!=token {busy.set(false);return;}
                    let row = v["key"].clone();
                    rows.update(|r| {
                        r.retain(|k| k["id"] != row["id"]);
                        r.insert(0, row.clone());
                    });
                    selected.set(Some(row));
                    notice.set("密钥已永久撤销。".into());
                }
                Err(e) => {
                    if owner.get_untracked()==owner_id && generation.get_untracked()==token {ctx.note_err(&e);error.set(e.message);}
                }
            }
            busy.set(false);
        });
    };
    view! {<Show when=move||!embedded||!editor_open.get()><section class="card key-manager"><header class="card-head"><h2>{if personal{"我的 API Key"}else{"用户的 API Key"}}</h2><div class="account-actions"><button type="button" class="primary" disabled=move||busy.get()||loading.get()||(!personal&&owner.get().is_none()) on:click=move |_|{reset();editor_open.set(true);}>"新建 API Key"</button><button type="button" class="ghost" disabled=move||busy.get()||loading.get() on:click=move |_|revision.update(|r|*r+=1)>"刷新"</button></div></header><div class="card-body">
    <Show when=move||personal><p class="muted small">{move||policy::budget_label(&user_budget.get())}</p><p class="muted small">"各 Key 共享用户总额度，设置 Key 上限不会增加总额度。最多32把活动 Key；已有密钥保留。"</p></Show>
    <p role="status">{move||if loading.get(){"正在读取密钥…"}else if !personal&&owner.get().is_none(){"保存或选择用户后可管理密钥。"}else if rows.get().is_empty(){"暂无密钥，点击新建 API Key 开始。"}else{""}}</p>
    <div class="table-wrap"><table><thead><tr><th>"名称 / 标识"</th><th>"状态 / 到期"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|k|{let toggle_row=k.clone();let delete_row=k.clone();let revoked=!k["revoked_at"].is_null();let enabled=k["enabled"]==true;let resume_blocked=personal&&!enabled&&k["can_enable"]!=true;let state=if !k["revoked_at"].is_null(){"已撤销"}else if k["enabled"]!=true{"已停用"}else if k["expires_at"].as_i64().is_some_and(|n|n<=crate::now_ms()){"已到期"}else{"可用"};let label=api::text(&k,"name");let hint=api::text(&k,"hint");let key_id=api::text(&k,"id");let time=k["expires_at"].as_i64().map(fmt::ts_short).unwrap_or_else(||"未设到期".into());view!{<tr><td data-label="密钥"><strong>{label}</strong><div><code>{hint}</code></div><div class="muted small">{format!("VKey ID：{key_id}")}</div></td><td data-label="状态">{state}<div class="muted small">{time}</div></td><td data-label="操作"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{name.set(api::text(&k,"name"));draft.set(k["policy"].clone());expiry.set(policy::expiry_input(k["expires_at"].as_i64()));limit.set(k["budget"]["limit_micro"].as_i64().map(policy::money_input).unwrap_or_default());selected.set(Some(k.clone()));secret.set(None);error.set(String::new());notice.set(String::new());editor_open.set(true);}>"编辑 Key"</button><button type="button" class=if enabled{"ghost danger"}else{"ghost"} title=if resume_blocked{"管理员停用、已到期或上级 Key 不可用时不能自行恢复"}else{"停用保留配置与历史，重新启用仍受权限和额度限制"} disabled=move||busy.get()||loading.get()||revoked||resume_blocked on:click=move |_|toggle_enabled(toggle_row.clone())>{if revoked{"已永久撤销"}else if enabled{"停用 Key"}else if resume_blocked{"不可自行启用"}else{"启用 Key"}}</button>{if revoked{view!{<button type="button" class="ghost danger" disabled=move||busy.get()||loading.get() on:click=move |_|{delete_target.set(Some(delete_row.clone()));delete_error.set(String::new());delete_open.set(true);}>"删除 Key"</button>}.into_any()}else{().into_any()}}</td></tr>}}).collect_view()}</tbody></table></div>
    <Show when=move||!editor_open.get()><p role="status">{move||notice.get()}</p><p role="alert">{move||error.get()}</p></Show></div></section>
    </Show><KeyEditorSurface open=editor_open busy=busy embedded=embedded>
    <Show when=move||secret.get().is_some()><section class="one-time-key" aria-labelledby=if personal{"my-created-key-title"}else{"admin-created-key-title"}>
    <h3 id=if personal{"my-created-key-title"}else{"admin-created-key-title"}>"API Key 已创建，请立即保存"</h3>
    <p>"完整 Key 仅在此处显示一次，关闭后无法再次查看。请保存到密码管理器；未写入浏览器持久存储。"</p>
    <label class="account-field"><span>"完整 API Key（可手动选择复制）"</span><textarea readonly=true autocomplete="off" spellcheck="false" rows="3" prop:value=move||secret.get().unwrap_or_default()/></label>
    <p role=move||if copy_failed.get(){"alert"}else{"status"}>{move||copy_note.get()}</p>
    <div class="account-actions"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{let Some(value)=secret.get_untracked()else{return};busy.set(true);leptos::task::spawn_local(async move{let copied=super::settings::copy_text(&value).await;copy_failed.set(!copied);copy_note.set(key_copy_notice(copied).into());busy.set(false);});}>"复制 Key"</button>
    <button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{secret.set(None);copy_note.set(String::new());editor_open.set(false);}>"已保存，关闭并清除"</button></div></section></Show>
    <Show when=move||secret.get().is_none()>
    <h3>{move||if selected.get().is_some(){"编辑 Key"}else{"创建 Key"}}</h3>
    <form class="account-form" on:submit=save><fieldset disabled=move||busy.get()||loading.get()||(!personal&&owner.get().is_none())||selected.get().is_some_and(|k|!k["revoked_at"].is_null())>
    <label class="account-field"><span>"名称"</span><input required type="text" maxlength="80" prop:value=name on:input=move|e|name.set(event_target_value(&e))/></label>
    <PolicyEditor policy=draft models=models channels=channels services=services prefix=if personal{"my-key"}else{"admin-key"} admin=!personal/>
    <div class="policy-limits"><label class="account-field"><span>"到期日期（本机时区）"</span><input type="datetime-local" prop:value=expiry on:input=move|e|expiry.set(event_target_value(&e))/></label><label class="account-field"><span>"累计消费上限（USD）"</span><input type="number" min="0" step="0.000001" placeholder="留空：不额外限制" prop:value=limit on:input=move|e|limit.set(event_target_value(&e))/></label></div><p class="account-help">"这是按已配置价格与 Token 用量计算的金额限额，金额最多6位小数；不是原始 Token 套餐。0表示没有付费额度，未定价调用仍按现有计量规则。"</p>
    <div class="account-actions"><button class="primary" type="submit">{move||if busy.get(){"正在保存…"}else{"保存 Key"}}</button><button class="ghost danger" type="button" disabled=move||selected.get().is_none() on:click=revoke>"永久撤销"</button></div></fieldset><div class="account-actions"><button class="ghost" type="button" disabled=move||busy.get() on:click=move |_|{if !busy.get_untracked(){editor_open.set(false);}}>"取消"</button></div></form>
    </Show>
    <p role="alert">{move||error.get()}</p><p role="status">{move||notice.get()}</p>
    </KeyEditorSurface>
    <super::ModuleDialog open=delete_open busy=busy title="删除已撤销的 API Key".to_string()>
    <p>{move||format!("确定删除“{}”？",delete_target.get().map(|k|api::text(&k,"name")).unwrap_or_default())}</p>
    <p class="account-help">"此 Key 已永久撤销。删除后从日常列表移除，调用历史、账务、审计及父子 Key 的安全约束保留。"</p>
    <p role="alert">{move||delete_error.get()}</p>
    <div class="account-actions"><button type="button" class="danger" disabled=move||busy.get() on:click=delete_key>{move||if busy.get(){"正在删除…"}else{"确认删除 Key"}}</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|delete_open.set(false)>"取消"</button></div>
    </super::ModuleDialog>}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_copy_keeps_actionable_manual_delivery_message() {
        assert!(key_copy_notice(false).contains("手动复制"));
        assert!(key_copy_notice(false).contains("关闭后无法再次查看"));
        assert!(!key_copy_notice(false).contains("已复制"));
        assert!(key_copy_notice(true).contains("已复制到系统剪贴板"));
    }
    #[test]
    fn enabling_and_disabling_only_patch_enabled() {
        for enabled in [false,true] {
            let key=json!({"id":42,"enabled":enabled,"revoked_at":null,"name":"keep","policy":{"allowed_models":[]},"budget":{"limit_micro":123}});
            let (id,patch,next)=status_patch(&key,false).unwrap();
            assert_eq!(id,42);assert_eq!(next,!enabled);assert_eq!(patch,json!({"enabled":!enabled}));
        }
    }
    #[test]
    fn revoked_or_missing_keys_cannot_be_toggled() {
        assert!(status_patch(&json!({"id":42,"enabled":false,"revoked_at":123}),false).is_err());
        assert!(status_patch(&json!({"enabled":false,"revoked_at":null}),false).is_err());
    }
    #[test]
    fn ordinary_users_only_resume_keys_the_server_allows() {
        let mut key=json!({"id":42,"enabled":false,"revoked_at":null,"can_enable":false});
        assert!(status_patch(&key,true).is_err());
        assert!(status_patch(&key,false).is_ok());
        key["can_enable"]=json!(true);
        assert!(status_patch(&key,true).is_ok());
        key["enabled"]=json!(true);key["can_enable"]=json!(false);
        assert!(status_patch(&key,true).is_ok());
    }
    #[test]
    fn status_changes_use_the_role_scoped_endpoint() {
        assert_eq!(key_path(true,42),"/api/account/keys/42");
        assert_eq!(key_path(false,42),"/api/vkeys/42");
    }
    #[test]
    fn new_personal_key_inherits_parent_without_offering_hidden_names_as_own_history() {
        let mut parent=policy::default_policy();parent["allowed_models"]=json!(["visible","hidden"]);parent["allowed_channels"]=json!([]);
        let draft=new_key_policy(parent.clone(),true);
        assert!(draft["allowed_models"].is_null());
        assert!(draft["allowed_services"].is_null());
        let saved=policy::inherit_policy(draft,&parent);
        assert_eq!(saved["allowed_models"],parent["allowed_models"]);
        assert_eq!(saved["allowed_channels"],json!([]));
        assert_eq!(new_key_policy(parent.clone(),false),parent);
        parent["allowed_models"]=json!([]);
        assert_eq!(policy::inherit_policy(new_key_policy(parent.clone(),true),&parent)["allowed_models"],json!([]));
    }

}
