use super::policy_editor::{self as policy, PolicyEditor};
use crate::{Ctx, admin_api as api, fmt};
use leptos::prelude::*;
use serde_json::{Value, json};
#[component]
pub fn KeyManager(
    ctx: Ctx,
    owner: RwSignal<Option<i64>>,
    models: RwSignal<Vec<String>>,
    channels: RwSignal<Vec<(i64, String)>>,
    #[prop(default = false)] personal: bool,
    #[prop(optional)] form_busy: Option<RwSignal<bool>>,
) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Value>::new());
    let selected = RwSignal::new(None::<Value>);
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
    let revision = RwSignal::new(0u64);
    let user_budget = RwSignal::new(Value::Null);
    Effect::new(move |_| {
        let id = owner.get();
        revision.get();
        let token = generation.get_untracked() + 1;
        generation.set(token);
        secret.set(None);
        rows.set(vec![]);
        selected.set(None);
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
                            base.set(v["policy"].clone());
                            draft.set(v["policy"].clone());
                            user_budget.set(v["budget"].clone());
                            limit.set(
                                v["budget"]["limit_micro"]
                                    .as_i64()
                                    .map(policy::money_input)
                                    .unwrap_or_default(),
                            );
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
    let reset = move || {
        selected.set(None);
        name.set(String::new());
        draft.set(base.get_untracked());
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
                    secret.set(new_secret);
                    rows.update(|r| {
                        r.retain(|k| k["id"] != row["id"]);
                        r.insert(0, row.clone());
                    });
                    selected.set(Some(row));
                    notice.set("密钥已保存；明文只在本次创建后显示。".into());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    let revoke = move |_| {
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
    view! {<section class="card key-manager"><header class="card-head"><h2>{if personal{"我的 API Key"}else{"用户的 API Key"}}</h2><button type="button" class="ghost" disabled=move||busy.get()||loading.get() on:click=move |_|revision.update(|r|*r+=1)>"刷新"</button></header><div class="card-body">
    <Show when=move||personal><p class="muted small">{move||policy::budget_label(&user_budget.get())}</p><p class="muted small">"各 Key 共享用户总额度，设置 Key 上限不会增加总额度。最多32把活动 Key；已有密钥保留。"</p></Show>
    <p role="status">{move||if loading.get(){"正在读取密钥…"}else if !personal&&owner.get().is_none(){"保存或选择用户后可管理密钥。"}else if rows.get().is_empty(){"暂无密钥，可在下方创建。"}else{""}}</p>
    <div class="table-wrap"><table><thead><tr><th>"名称 / 标识"</th><th>"状态 / 到期"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|k|{let state=if !k["revoked_at"].is_null(){"已撤销"}else if k["enabled"]!=true{"已停用"}else if k["expires_at"].as_i64().is_some_and(|n|n<=crate::now_ms()){"已到期"}else{"可用"};let label=api::text(&k,"name");let hint=api::text(&k,"hint");let time=k["expires_at"].as_i64().map(fmt::ts_short).unwrap_or_else(||"未设到期".into());view!{<tr><td data-label="密钥"><strong>{label}</strong><div><code>{hint}</code></div></td><td data-label="状态">{state}<div class="muted small">{time}</div></td><td data-label="操作"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{name.set(api::text(&k,"name"));draft.set(k["policy"].clone());expiry.set(policy::expiry_input(k["expires_at"].as_i64()));limit.set(k["budget"]["limit_micro"].as_i64().map(policy::money_input).unwrap_or_default());selected.set(Some(k.clone()));secret.set(None);error.set(String::new());notice.set(String::new());}>"管理"</button></td></tr>}}).collect_view()}</tbody></table></div>
    <hr/><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|reset()>"新建 API Key"</button><h3>{move||if selected.get().is_some(){"编辑 Key"}else{"创建 Key"}}</h3>
    <form class="account-form" on:submit=save><fieldset disabled=move||busy.get()||loading.get()||(!personal&&owner.get().is_none())||selected.get().is_some_and(|k|!k["revoked_at"].is_null())>
    <label class="account-field"><span>"名称"</span><input required type="text" maxlength="80" prop:value=name on:input=move|e|name.set(event_target_value(&e))/></label>
    <PolicyEditor policy=draft models=models channels=channels prefix=if personal{"my-key"}else{"admin-key"} admin=!personal/>
    <div class="policy-limits"><label class="account-field"><span>"到期日期（本机时区）"</span><input type="datetime-local" prop:value=expiry on:input=move|e|expiry.set(event_target_value(&e))/></label><label class="account-field"><span>"累计消费上限（USD）"</span><input type="number" min="0" step="0.000001" placeholder="留空：不额外限制" prop:value=limit on:input=move|e|limit.set(event_target_value(&e))/></label></div><p class="account-help">"这是按已配置价格与 Token 用量计算的金额限额，金额最多6位小数；不是原始 Token 套餐。0表示没有付费额度，未定价调用仍按现有计量规则。"</p>
    <div class="account-actions"><button class="primary" type="submit">{move||if busy.get(){"正在保存…"}else{"保存 Key"}}</button><button class="ghost danger" type="button" disabled=move||selected.get().is_none() on:click=revoke>"永久撤销"</button></div></fieldset></form>
    <p role="alert">{move||error.get()}</p><p role="status">{move||notice.get()}</p>
    {move||secret.get().map(|s|view!{<div class="one-time-key" role="status"><h3>"请立即保存 API Key"</h3><p>"关闭后无法再次查看。未写入浏览器持久存储。"</p><code class="one-time-secret">{s}</code><button class="ghost" type="button" on:click=move |_|secret.set(None)>"已保存，隐藏明文"</button></div>})}
    </div></section>}
}
