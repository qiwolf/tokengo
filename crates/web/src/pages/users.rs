use super::{
    PageHead,
    key_manager::KeyManager,
    policy_editor::{self as policy, PolicyEditor},
};
use crate::{Ctx, Page, account_api, admin_api as api};
use leptos::prelude::*;
use serde_json::{Value, json};
#[component]
pub fn Users(ctx: Ctx) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Value>::new());
    let roots = RwSignal::new(Vec::<Value>::new());
    let selected = RwSignal::new(None::<Value>);
    let owner = RwSignal::new(None::<i64>);
    let username = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let enabled = RwSignal::new(true);
    let draft = RwSignal::new(policy::default_policy());
    let limit = RwSignal::new(String::new());
    let models = RwSignal::new(Vec::<String>::new());
    let channels = RwSignal::new(Vec::<(i64, String)>::new());
    let busy = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let list_error = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let revision = RwSignal::new(0u64);
    Effect::new(move |_| {
        revision.get();
        loading.set(true);
        list_error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("GET", "/api/managed-users", None).await {
                Ok(v) => {
                    rows.set(api::rows(&v, "users"));
                    roots.set(api::rows(&v, "root_accounts"));
                }
                Err(e) => {
                    ctx.note_err(&e);
                    list_error.set(e.message);
                }
            }
            match crate::api::channels().await {
                Ok(v) => {
                    let mut m = v
                        .iter()
                        .flat_map(|c| c.models.iter().map(|m| m.model.clone()))
                        .collect::<Vec<_>>();
                    m.sort();
                    m.dedup();
                    models.set(m);
                    channels.set(
                        v.into_iter()
                            .map(|c| {
                                (
                                    c.id,
                                    format!(
                                        "{} · {} · #{}",
                                        c.provider_name.unwrap_or_else(|| "未分组".into()),
                                        c.name,
                                        c.id
                                    ),
                                )
                            })
                            .collect(),
                    );
                }
                Err(e) => {
                    ctx.note_err(&e);
                    list_error.set(format!(
                        "模型选项读取失败：{}。已有授权保留；请刷新后编辑。",
                        e.message
                    ));
                }
            }
            loading.set(false);
        });
    });
    let reset = move || {
        selected.set(None);
        owner.set(None);
        username.set(String::new());
        name.set(String::new());
        password.set(String::new());
        enabled.set(true);
        draft.set(policy::default_policy());
        limit.set(String::new());
        error.set(String::new());
        notice.set(String::new());
    };
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let existing = selected.get_untracked();
        let has_login = existing
            .as_ref()
            .is_some_and(|v| v["accounts"].as_array().is_some_and(|a| !a.is_empty()));
        let creating = existing.is_none();
        let pw = password.get_untracked();
        let result = (|| -> Result<Value, String> {
            policy::validate(&draft.get_untracked())?;
            if name.get_untracked().trim().is_empty() {
                return Err("请填写显示名称。".into());
            }
            if creating || (!has_login && !pw.is_empty()) {
                if !account_api::valid_username(username.get_untracked().trim()) {
                    return Err("登录名需为3–64位字母、数字、点、下划线或连字符。".into());
                }
                if !account_api::valid_password(&pw) {
                    return Err("初始密码需为12–256字节。".into());
                }
            } else if !pw.is_empty() && !account_api::valid_password(&pw) {
                return Err("新密码需为12–256字节；留空不修改。".into());
            }
            let mut b = json!({"display_name":name.get_untracked().trim(),"policy":draft.get_untracked(),"limit_micro":policy::optional_limit(&limit.get_untracked())?});
            if creating {
                b["username"] = json!(username.get_untracked().trim());
                b["password"] = json!(pw);
            } else {
                b["enabled"] = json!(enabled.get_untracked());
                if has_login && !pw.is_empty() {
                    b["password"] = json!(pw);
                }
            }
            Ok(b)
        })();
        let body = match result {
            Ok(v) => v,
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let id = existing.and_then(|v| v["id"].as_i64());
        if id.is_some()
            && (!pw.is_empty() || !enabled.get_untracked())
            && !web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(
                        "停用用户会禁止其登录及所有 API 调用；重置密码会退出其登录会话。确认保存？",
                    )
                    .ok()
                })
                .unwrap_or(false)
        {
            return;
        }
        busy.set(true);
        error.set(String::new());
        notice.set(String::new());
        leptos::task::spawn_local(async move {
            let path = id
                .map(|n| format!("/api/managed-users/{n}"))
                .unwrap_or_else(|| "/api/managed-users".into());
            match api::request(
                if id.is_some() { "PATCH" } else { "POST" },
                &path,
                Some(body),
            )
            .await
            {
                Ok(mut v) => {
                    let uid = v["user"]["id"].as_i64().or(id);
                    if id.is_some() && !has_login && !pw.is_empty() {
                        if let Some(uid) = uid {
                            match api::request("POST",&format!("/api/managed-users/{uid}/login-credential"),Some(json!({"username":username.get_untracked().trim(),"password":pw,"display_name":name.get_untracked().trim()}))).await{Ok(r)=>{if r["user"].is_object(){v=r}},Err(e)=>error.set(format!("用户资料已保存，但登录尚未开通：{}。原密钥和账本保留。",e.message))}
                        }
                    }
                    password.set(String::new());
                    selected.set(Some(v["user"].clone()));
                    owner.set(uid);
                    if let Some(a) = v["user"]["accounts"].as_array().and_then(|a| a.first()) {
                        username.set(api::text(a, "username"));
                    }
                    notice.set(
                        if creating {
                            "用户已创建，可直接使用登录名和初始密码登录，并自行创建 API Key。"
                        } else {
                            "用户已保存；模型权限和总额度实时约束其全部 Key。"
                        }
                        .into(),
                    );
                    revision.update(|r| *r += 1);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<div class="page admin-page"><PageHead title="用户" desc="一次设置登录账号、模型授权和费用上限；用户登录后自行管理 API Key。"><button class="primary" disabled=move||busy.get() on:click=move |_|reset()>"新建用户"</button></PageHead>
    <section class="card"><header class="card-head"><h2>"用户列表"</h2><button class="ghost" disabled=move||loading.get()||busy.get() on:click=move |_|revision.update(|r|*r+=1)>"刷新"</button></header><div class="card-body"><p role="alert">{move||list_error.get()}</p><p role="status">{move||if loading.get(){"正在读取用户…"}else if rows.get().is_empty(){"还没有用户，点击新建即可设置登录和调用权限。"}else{""}}</p><div class="table-wrap"><table><thead><tr><th>"用户"</th><th>"登录账号"</th><th>"状态 / 额度"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|u|{let id=u["id"].as_i64().unwrap_or(0);let n=api::text(&u,"name");let accounts=api::rows(&u,"accounts");let names=if accounts.is_empty(){"尚未开通登录".into()}else{accounts.iter().map(|a|api::text(a,"username")).collect::<Vec<_>>().join("、")};let state=if u["enabled"]==true{"启用"}else{"停用"};let quota=policy::budget_label(&u["budget"]);view!{<tr><td data-label="用户"><strong>{n}</strong><div class="muted small">{format!("#{id}")}</div></td><td data-label="登录账号">{names}</td><td data-label="状态 / 额度">{state}<div class="muted small">{quota}</div></td><td data-label="操作"><button class="ghost" disabled=move||busy.get() on:click=move |_|{owner.set(Some(id));username.set(api::rows(&u,"accounts").first().map(|a|api::text(a,"username")).unwrap_or_default());name.set(api::text(&u,"name"));password.set(String::new());enabled.set(u["enabled"]==true);draft.set(u["policy"].clone());limit.set(u["budget"]["limit_micro"].as_i64().map(|n|n.to_string()).unwrap_or_default());selected.set(Some(u.clone()));error.set(String::new());notice.set(String::new());}>"管理"</button></td></tr>}}).collect_view()}</tbody></table></div>
    <p class="account-help">{move||roots.get().first().map(|a|format!("管理员 {} 的登录安全在“账号安全”管理，不占用普通用户。",api::text(a,"username"))).unwrap_or_default()}</p><button class="text-link" on:click=move |_|ctx.go(Page::Account)>"账号安全"</button></div></section>
    <section class="card"><header class="card-head"><h2>{move||selected.get().map(|v|format!("管理用户：{}",api::text(&v,"name"))).unwrap_or_else(||"创建用户".into())}</h2></header><div class="card-body"><form class="account-form" on:submit=save><fieldset disabled=move||busy.get()||loading.get()><div class="policy-limits">
    <label class="account-field"><span>"登录账号"</span><input type="text" minlength="3" maxlength="64" autocomplete="off" autocapitalize="none" spellcheck="false" readonly=move||selected.get().is_some_and(|u|u["accounts"].as_array().is_some_and(|a|!a.is_empty())) prop:value=username on:input=move|e|username.set(event_target_value(&e))/></label><label class="account-field"><span>"显示名称"</span><input type="text" required maxlength="80" prop:value=name on:input=move|e|name.set(event_target_value(&e))/></label></div>
    <crate::account_ui::PasswordField id="user-password" label="初始密码 / 重置密码" value=password autocomplete="new-password" error=error describedby="user-help user-error" required=false/>
    <p id="user-help" class="account-help">{move||if selected.get().is_none(){"新用户须填写账号和初始密码，保存后即可登录。"}else if selected.get().is_some_and(|u|u["accounts"].as_array().is_none_or(Vec::is_empty)){"已有用户的 Key 与历史保留；填写登录名和初始密码即可为此用户开通登录，无需新建或关联另一种用户。"}else{"编辑时密码留空不修改；如需改自己的管理员登录名或密码，请到账号安全。"}}</p>
    <label class="account-check"><input type="checkbox" disabled=move||selected.get().is_none() prop:checked=enabled on:change=move|e|enabled.set(event_target_checked(&e))/>"启用用户（登录与 API 调用同步生效）"</label>
    <PolicyEditor policy=draft models=models channels=channels prefix="user" admin=true/>
    <label class="account-field"><span>"累计消费上限（USD）"</span><input type="number" min="0" step="0.000001" placeholder="留空：不限制累计金额" prop:value=limit on:input=move|e|limit.set(event_target_value(&e))/></label><p class="account-help">"按配置价格与实际 Token/按次用量累计费用；金额最多6位小数，留空不限，0表示没有付费额度。月度套餐和原始 Token 配额将在后续独立实现，本页不将金额冒充 Token 余额。"</p>
    <p id="user-error" role="alert">{move||error.get()}</p><p role="status">{move||notice.get()}</p><button type="submit" class="primary">{move||if busy.get(){"正在保存…"}else{"保存用户"}}</button></fieldset></form></div></section>
    <KeyManager ctx=ctx owner=owner models=models channels=channels form_busy=busy/>
    </div>}
}
