use super::PageHead;
use crate::{admin_api as api, Ctx};
use leptos::prelude::*;
use serde_json::{json, Value};

#[component]
pub fn Users(ctx: Ctx) -> impl IntoView {
    let users = RwSignal::new(Vec::<Value>::new());
    let list_error = RwSignal::new(String::new());
    let loading = RwSignal::new(false);
    let selected = RwSignal::new(None::<i64>);
    let name = RwSignal::new(String::new());
    let role = RwSignal::new("user".to_string());
    let enabled = RwSignal::new(true);
    let policy = RwSignal::new(api::POLICY.to_string());
    let message = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let keys = RwSignal::new(Vec::<Value>::new());
    let key_loading = RwSignal::new(false);
    let key_id = RwSignal::new(None::<i64>);
    let key_name = RwSignal::new(String::new());
    let key_enabled = RwSignal::new(true);
    let key_revoked = RwSignal::new(false);
    let key_policy = RwSignal::new(api::POLICY.to_string());
    let expiry = RwSignal::new(String::new());
    let key_busy = RwSignal::new(false);
    let key_error = RwSignal::new(String::new());
    let key_message = RwSignal::new(String::new());
    let secret = RwSignal::new(None::<String>);
    let revision = RwSignal::new(0u64);
    Effect::new(move |_| {
        revision.get();
        loading.set(true);
        list_error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("GET", "/api/users", None).await {
                Ok(v) => users.set(api::rows(&v, "users")),
                Err(e) => {
                    ctx.note_err(&e);
                    list_error.set(e.message);
                }
            }
            loading.set(false);
        });
    });
    Effect::new(move |_| {
        let owner = selected.get();
        revision.get();
        keys.set(vec![]);
        secret.set(None);
        key_id.set(None);
        key_name.set(String::new());
        key_policy.set(api::POLICY.into());
        expiry.set(String::new());
        key_revoked.set(false);
        key_enabled.set(true);
        key_error.set(String::new());
        key_message.set(String::new());
        if let Some(id) = owner {
            key_loading.set(true);
            leptos::task::spawn_local(async move {
                let result = api::request("GET", &format!("/api/users/{id}/keys"), None).await;
                if selected.get_untracked() == Some(id) {
                    match result {
                        Ok(v) => keys.set(api::rows(&v, "keys")),
                        Err(e) => {
                            ctx.note_err(&e);
                            key_error.set(e.message);
                        }
                    }
                    key_loading.set(false);
                }
            });
        } else {
            key_loading.set(false);
        }
    });
    let reset = move |_| {
        selected.set(None);
        name.set(String::new());
        role.set("user".into());
        enabled.set(true);
        policy.set(api::POLICY.into());
        error.set(String::new());
        message.set(String::new());
        secret.set(None);
    };
    let save_user = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let p = match api::object(&policy.get_untracked()) {
            Ok(v) => v,
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let n = name.get_untracked().trim().to_string();
        if n.is_empty() {
            error.set("请填写用户名称".into());
            return;
        }
        let id = selected.get_untracked();
        let mut body = json!({"name":n,"role":role.get_untracked(),"policy":p});
        if id.is_some() {
            body["enabled"] = json!(enabled.get_untracked());
        }
        error.set(String::new());
        message.set(String::new());
        busy.set(true);
        leptos::task::spawn_local(async move {
            let path = id
                .map(|i| format!("/api/users/{i}"))
                .unwrap_or_else(|| "/api/users".into());
            match api::request(
                if id.is_some() { "PATCH" } else { "POST" },
                &path,
                Some(body),
            )
            .await
            {
                Ok(v) => {
                    selected.set(v["user"]["id"].as_i64());
                    message.set("用户已保存".into());
                    revision.update(|v| *v += 1);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    let save_key = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if key_busy.get_untracked() {
            return;
        }
        let Some(owner) = selected.get_untracked() else {
            return;
        };
        let p = match api::object(&key_policy.get_untracked()) {
            Ok(v) => v,
            Err(e) => {
                key_error.set(e);
                return;
            }
        };
        let n = key_name.get_untracked().trim().to_string();
        if n.is_empty() {
            key_error.set("请填写密钥名称".into());
            return;
        }
        let expires = if expiry.get_untracked().trim().is_empty() {
            None
        } else {
            match api::integer(&expiry.get_untracked(), "到期时间", false) {
                Ok(v) => Some(v),
                Err(e) => {
                    key_error.set(e);
                    return;
                }
            }
        };
        let id = key_id.get_untracked();
        let mut body = json!({"name":n,"policy":p,"expires_at":expires});
        if id.is_some() {
            body["enabled"] = json!(key_enabled.get_untracked());
        }
        secret.set(None);
        key_busy.set(true);
        key_error.set(String::new());
        key_message.set(String::new());
        leptos::task::spawn_local(async move {
            let path = id
                .map(|i| format!("/api/vkeys/{i}"))
                .unwrap_or_else(|| format!("/api/users/{owner}/keys"));
            match api::request(
                if id.is_some() { "PATCH" } else { "POST" },
                &path,
                Some(body),
            )
            .await
            {
                Ok(v) => {
                    if selected.get_untracked() == Some(owner) {
                        keys.update(|all| {
                            let row = v["key"].clone();
                            all.retain(|k| k["id"] != row["id"]);
                            all.insert(0, row);
                        });
                        key_id.set(v["key"]["id"].as_i64());
                        secret.set(v["secret"].as_str().map(str::to_owned));
                        key_message.set("密钥已保存".into());
                    }
                }
                Err(e) => {
                    ctx.note_err(&e);
                    key_error.set(e.message);
                }
            }
            key_busy.set(false);
        });
    };
    view! {
        <div class="page admin-page">
        <PageHead title="用户" desc="用户身份、VKey 权限与限流。用户与密钥策略取交集，子密钥还受父链约束。"/>
        <section class="card"><header class="card-head"><h2>"用户列表"</h2><button class="ghost" disabled=move||loading.get()||busy.get()||key_busy.get() on:click=move |_|revision.update(|v|*v+=1)>"刷新"</button></header>
        <div class="card-body">
        <p role="alert">{move||list_error.get()}</p>
        <p role="status">{move||if loading.get(){"正在加载用户…"}else if users.get().is_empty(){"暂无用户；可在下方创建。"}else{""}}</p>
        <div class="table-wrap"><table><thead><tr><th>"ID / 名称"</th><th>"角色 / 状态"</th><th>"操作"</th></tr></thead><tbody>
        {move||users.get().into_iter().map(|u|{let id=u["id"].as_i64().unwrap_or_default();let label=format!("{} · {}",id,api::text(&u,"name"));let state=format!("{} · {}",api::text(&u,"role"),if u["enabled"]==true{"启用"}else{"停用"});view!{<tr><td data-label="用户">{label}</td><td data-label="状态">{state}</td><td data-label="操作"><button class="ghost" disabled=move||busy.get()||key_busy.get() on:click=move |_|{selected.set(Some(id));name.set(api::text(&u,"name"));role.set(api::text(&u,"role"));enabled.set(u["enabled"]==true);policy.set(api::pretty(&u["policy"]));error.set(String::new());message.set(String::new());}>"编辑与管理密钥"</button></td></tr>}}).collect_view()}
        </tbody></table></div></div></section>
        <section class="card"><header class="card-head"><h2>{move||selected.get().map(|i|format!("编辑用户 #{i}")).unwrap_or_else(||"创建用户".into())}</h2><button class="ghost" disabled=move||busy.get()||key_busy.get() on:click=reset>"切换到创建用户"</button></header><div class="card-body">
        <form on:submit=save_user><fieldset disabled=move||busy.get()||key_busy.get()><div class="form-grid">
        <label for="user-name">"名称"</label><input id="user-name" aria-describedby="user-error" required type="text" prop:value=name on:input=move|e|name.set(event_target_value(&e))/>
        <label for="user-role">"角色"</label><select id="user-role" aria-describedby="user-error" prop:value=role on:change=move|e|role.set(event_target_value(&e))><option value="user">"user"</option><option value="admin">"admin"</option></select>
        <label for="user-enabled">"启用用户"</label><div><input id="user-enabled" type="checkbox" disabled=move||selected.get().is_none() prop:checked=enabled on:change=move|e|enabled.set(event_target_checked(&e))/><p class="muted small">"新用户默认启用。停用后，其所有密钥均无法请求。admin 角色还需用户和密钥都显式授予 admin_access 才能管理。"</p></div>
        <label for="user-policy">"用户策略 JSON"</label><textarea id="user-policy" rows="12" aria-describedby="policy-help user-error" aria-invalid=move||api::object(&policy.get()).is_err().to_string() prop:value=policy on:input=move|e|policy.set(event_target_value(&e))/>
        </div><PolicyHelp/>
        <p id="user-error" role="alert">{move||error.get()}</p><p role="status">{move||message.get()}</p>
        <button class="primary" type="submit">{move||if busy.get(){"正在保存…"}else{"保存用户"}}</button></fieldset></form>
        </div></section>
        <section class="card"><header class="card-head"><h2>"所选用户的 VKey"</h2></header><div class="card-body">
        <p class="muted">{move||if selected.get().is_none(){"请先选择或创建用户。"}else if key_loading.get(){"正在加载密钥…"}else if keys.get().is_empty(){"此用户暂无密钥。"}else{"选择一条密钥可编辑策略、到期时间、启停或撤销。"}}</p>
        <div class="table-wrap"><table><thead><tr><th>"ID / 名称 / 提示"</th><th>"状态 / 到期"</th><th>"父密钥"</th><th>"操作"</th></tr></thead><tbody>
        {move||keys.get().into_iter().map(|k|{let label=format!("{} · {} · {}",api::text(&k,"id"),api::text(&k,"name"),api::text(&k,"hint"));let revoked=!k["revoked_at"].is_null();let state=format!("{} · {}",if revoked{"已撤销"}else if k["expires_at"].as_i64().is_some_and(|t|t as f64<=js_sys::Date::now()){"已到期"}else if k["enabled"]==true{"启用"}else{"停用"},if k["expires_at"].is_null(){"永不过期".into()}else{format!("到期毫秒 {}",api::text(&k,"expires_at"))});let parent=if k["parent_key_id"].is_null(){"无".into()}else{api::text(&k,"parent_key_id")};view!{<tr><td data-label="密钥">{label}</td><td data-label="状态">{state}</td><td data-label="父密钥">{parent}</td><td data-label="操作"><button class="ghost" disabled=move||key_busy.get()||busy.get() on:click=move |_|{key_id.set(k["id"].as_i64());key_name.set(api::text(&k,"name"));key_policy.set(api::pretty(&k["policy"]));key_enabled.set(k["enabled"]==true);key_revoked.set(revoked);expiry.set(k["expires_at"].as_i64().map(|n|n.to_string()).unwrap_or_default());secret.set(None);key_error.set(String::new());key_message.set(String::new());}>"编辑"</button></td></tr>}}).collect_view()}
        </tbody></table></div>
        <hr/><button class="ghost" disabled=move||selected.get().is_none()||key_busy.get()||busy.get() on:click=move |_|{key_id.set(None);key_name.set(String::new());key_policy.set(api::POLICY.into());expiry.set(String::new());key_enabled.set(true);key_revoked.set(false);secret.set(None);key_error.set(String::new());key_message.set(String::new());}>"新建 VKey"</button>
        <h3>{move||key_id.get().map(|i|format!("编辑密钥 #{i}")).unwrap_or_else(||"创建 VKey（明文仅显示一次）".into())}</h3>
        <form on:submit=save_key><fieldset disabled=move||selected.get().is_none()||key_busy.get()||busy.get()||key_loading.get()><div class="form-grid">
        <label for="vkey-name">"密钥名称"</label><input id="vkey-name" aria-describedby="key-error" required type="text" prop:value=key_name on:input=move|e|key_name.set(event_target_value(&e))/>
        <label for="vkey-policy">"密钥策略 JSON"</label><textarea id="vkey-policy" rows="12" aria-describedby="policy-help key-error" aria-invalid=move||api::object(&key_policy.get()).is_err().to_string() prop:value=key_policy on:input=move|e|key_policy.set(event_target_value(&e))/>
        <label for="vkey-expiry">"到期时间（Unix 毫秒）"</label><div><input id="vkey-expiry" type="text" inputmode="numeric" aria-describedby="expiry-help key-error" prop:value=expiry on:input=move|e|expiry.set(event_target_value(&e))/><p id="expiry-help" class="muted small">"留空表示永不过期（编辑时会清除旧到期时间）。创建时必须晚于当前时间。"</p></div>
        <label for="vkey-enabled">"启用密钥"</label><div><input id="vkey-enabled" type="checkbox" disabled=move||key_id.get().is_none()||key_revoked.get() prop:checked=key_enabled on:change=move|e|key_enabled.set(event_target_checked(&e))/><p class="muted small">"新密钥默认启用；已撤销的密钥不能再次启用。"</p></div>
        </div><p id="key-error" role="alert">{move||key_error.get()}</p><p role="status">{move||key_message.get()}</p>
        <button type="submit" class="primary">{move||if key_busy.get(){"正在保存…"}else{"保存密钥"}}</button>
        <button type="button" class="ghost danger" disabled=move||key_id.get().is_none()||key_revoked.get() on:click=move |_|{
            if !web_sys::window().and_then(|w|w.confirm_with_message("永久撤销此密钥？撤销后不能恢复。").ok()).unwrap_or(false){return;}
            let Some(id)=key_id.get_untracked()else{return;};key_busy.set(true);key_error.set(String::new());secret.set(None);
            leptos::task::spawn_local(async move{match api::request("PATCH",&format!("/api/vkeys/{id}"),Some(json!({"revoke":true}))).await{Ok(v)=>{keys.update(|all|{all.retain(|k|k["id"]!=v["key"]["id"]);all.insert(0,v["key"].clone());});key_revoked.set(true);key_enabled.set(false);key_message.set("密钥已永久撤销".into());},Err(e)=>{ctx.note_err(&e);key_error.set(e.message);}}key_busy.set(false);});
        }>"永久撤销"</button></fieldset></form>
        {move||secret.get().map(|value|view!{<div class="panel"><h3>"请立即保存密钥"</h3><p>"只在本次创建后显示；刷新、离开页面或切换用户后无法再次查看。未写入浏览器存储。"</p><pre class="admin-json">{value}</pre><button class="ghost" on:click=move |_|secret.set(None)>"已保存，隐藏明文"</button></div>})}
        </div></section></div>
    }
}
#[component]
fn PolicyHelp() -> impl IntoView {
    view! {<p id="policy-help" class="muted small">"allowed_models / allowed_channels：null 不额外限制，[] 全部禁止，数组填写模型名 / 渠道 ID。allow_paid 控制付费上游；admin_access 默认 false。rpm / tpm / max_concurrent：null 不额外限制，正整数为每分钟请求 / token / 并发上限（接口不接受 0）。用户、当前密钥及父链取交集；TPM 按输入与输出上限预占。"</p>}
}
