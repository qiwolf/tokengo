use super::{
    PageHead,
    key_manager::KeyManager,
    policy_editor::{self as policy, PolicyEditor},
};
use crate::{Ctx, Page, account_api, admin_api as api};
use leptos::prelude::*;
use serde_json::{Value, json};
fn login_username(user: &Value) -> Option<String> {
    api::rows(user, "accounts").iter().find_map(|account| {
        let username = api::text(account, "username");
        (!username.is_empty()).then_some(username)
    })
}

fn login_required(creating: bool, has_login: bool, open_login: bool) -> bool {
    creating || (!has_login && open_login)
}

#[component]
pub fn Users(ctx: Ctx) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Value>::new());
    let roots = RwSignal::new(Vec::<Value>::new());
    let selected = RwSignal::new(None::<Value>);
    let owner = RwSignal::new(None::<i64>);
    let editing = RwSignal::new(false);
    let keys_open = RwSignal::new(false);
    let open_login = RwSignal::new(false);
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
    let show_disabled = RwSignal::new(false);
    let search = RwSignal::new(String::new());
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
        editing.set(true);
        keys_open.set(false);
        open_login.set(false);
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
        let has_login = existing.as_ref().and_then(login_username).is_some();
        let creating = existing.is_none();
        let needs_login = login_required(creating, has_login, open_login.get_untracked());
        let pw = password.get_untracked();
        let result = (|| -> Result<Value, String> {
            policy::validate(&draft.get_untracked())?;
            if name.get_untracked().trim().is_empty() {
                return Err("请填写显示名称。".into());
            }
            if needs_login {
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
        let id = existing.as_ref().and_then(|v| v["id"].as_i64());
        if id.is_some()
            && ((has_login && !pw.is_empty())
                || (existing.as_ref().is_some_and(|u| u["enabled"] == true)
                    && !enabled.get_untracked()))
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
                    let mut login_failed = false;
                    if id.is_some() && needs_login {
                        if let Some(uid) = uid {
                            match api::request("POST",&format!("/api/managed-users/{uid}/login-credential"),Some(json!({"username":username.get_untracked().trim(),"password":pw,"display_name":name.get_untracked().trim()}))).await{Ok(r)=>{if r["user"].is_object(){v=r}},Err(e)=>{login_failed=true;error.set(format!("用户资料已保存，但登录尚未开通：{}。请修正后重新设置初始密码。",e.message))}}
                        }
                    }
                    password.set(String::new());
                    selected.set(Some(v["user"].clone()));
                    owner.set(uid);
                    username.set(login_username(&v["user"]).unwrap_or_default());
                    open_login.set(login_failed);
                    notice.set(
                        if login_failed {
                            "资料已保存；请完成登录设置。"
                        } else if creating {
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
    view! {<div class="page admin-page users-page"><PageHead title="用户" desc="管理登录账号、模型权限和消费上限；用户登录后可自行创建 API Key。"/>
    <Show when=move||!editing.get()>
    <section class="card"><header class="card-head"><h2>"用户列表"</h2><div class="user-actions"><button type="button" class="ghost" disabled=move||loading.get()||busy.get() on:click=move |_|revision.update(|r|*r+=1)>"刷新"</button><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|reset()>"新建用户"</button></div></header>
    <div class="card-body"><div class="policy-limits"><label class="account-field"><span>"搜索用户 / 登录名"</span><input type="search" prop:value=search on:input=move|e|search.set(event_target_value(&e))/></label><label class="account-check"><input type="checkbox" prop:checked=show_disabled on:change=move|e|show_disabled.set(event_target_checked(&e))/>"显示已停用用户"</label></div>
    <p role="alert">{move||list_error.get()}</p><p role="status">{move||if loading.get(){"正在读取用户…"}else if rows.get().is_empty(){"还没有用户，点击新建用户即可开始。"}else{""}}</p>
    <div class="table-wrap"><table><thead><tr><th>"用户"</th><th>"登录账号"</th><th>"状态 / 额度"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().filter(|u|{let query=search.get().trim().to_lowercase();(show_disabled.get()||u["enabled"]==true)&&(query.is_empty()||api::text(u,"name").to_lowercase().contains(&query)||api::rows(u,"accounts").iter().any(|a|api::text(a,"username").to_lowercase().contains(&query)))}).map(|u|{
        let id=u["id"].as_i64().unwrap_or(0);
        let n=api::text(&u,"name");
        let names=api::rows(&u,"accounts").iter().filter_map(|a|a["username"].as_str()).collect::<Vec<_>>().join("、");
        let names=if names.is_empty(){"未设置登录".into()}else{names};
        let state=if u["enabled"]==true{"已启用"}else{"已停用"};
        let quota=policy::budget_label(&u["budget"]);
        view!{<tr><td data-label="用户"><strong>{n}</strong></td><td data-label="登录账号">{names}</td><td data-label="状态 / 额度">{state}<div class="muted small">{quota}</div></td><td data-label="操作"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{
            owner.set(Some(id));username.set(login_username(&u).unwrap_or_default());name.set(api::text(&u,"name"));password.set(String::new());enabled.set(u["enabled"]==true);draft.set(u["policy"].clone());limit.set(u["budget"]["limit_micro"].as_i64().map(policy::money_input).unwrap_or_default());selected.set(Some(u.clone()));error.set(String::new());notice.set(String::new());open_login.set(false);keys_open.set(false);editing.set(true);
        }>"管理"</button></td></tr>}
    }).collect_view()}</tbody></table></div>
    <p class="account-help">{move||roots.get().first().map(|a|format!("管理员 {} 的登录名和密码在账号安全中管理。",api::text(a,"username"))).unwrap_or_default()}</p><button type="button" class="text-link" on:click=move |_|ctx.go(Page::Account)>"账号安全"</button></div></section></Show>
    <Show when=move||editing.get()>
    <div class="user-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{editing.set(false);keys_open.set(false);owner.set(None);password.set(String::new());}>"返回用户列表"</button></div>
    <section class="card"><header class="card-head"><h2>{move||selected.get().map(|v|format!("管理用户：{}",api::text(&v,"name"))).unwrap_or_else(||"新建用户".into())}</h2></header>
    <Show when=move||selected.get().is_some()><nav class="user-section-nav" aria-label="用户管理功能"><button type="button" class="ghost" aria-pressed=move||(!keys_open.get()).to_string() disabled=move||busy.get() on:click=move |_|keys_open.set(false)>"账号与授权"</button><button type="button" class="ghost" aria-pressed=move||keys_open.get().to_string() disabled=move||busy.get() on:click=move |_|keys_open.set(true)>"API Key"</button></nav></Show>
    <Show when=move||!keys_open.get()><div class="card-body"><form class="account-form" on:submit=save><fieldset disabled=move||busy.get()||loading.get()>
    <section class="user-form-section" aria-labelledby="user-account-heading"><h3 id="user-account-heading">"登录与基本资料"</h3>
    <Show when=move||selected.get().is_some_and(|u|login_username(&u).is_none())><div class="user-login-state"><p>"此用户未设置登录账号，现有 API Key 仍按原有权限使用。"</p><label class="account-check"><input type="checkbox" prop:checked=open_login on:change=move|e|{open_login.set(event_target_checked(&e));password.set(String::new());username.set(String::new());}/>"为此用户开通登录"</label></div></Show>
    <div class="policy-limits"><Show when=move||selected.get().is_none()||selected.get().is_some_and(|u|login_username(&u).is_some())||open_login.get()><label class="account-field"><span>"登录账号"</span><input type="text" minlength="3" maxlength="64" autocomplete="off" autocapitalize="none" spellcheck="false" required=true readonly=move||selected.get().is_some_and(|u|login_username(&u).is_some()) prop:value=username on:input=move|e|username.set(event_target_value(&e))/></label></Show><label class="account-field"><span>"显示名称"</span><input type="text" required maxlength="80" prop:value=name on:input=move|e|name.set(event_target_value(&e))/></label></div>
    <Show when=move||selected.get().is_none()||selected.get().is_some_and(|u|login_username(&u).is_some())||open_login.get()><crate::account_ui::PasswordField id="user-password" label="设置密码" value=password autocomplete="new-password" error=error describedby="user-help user-error" required=false/>
    <p id="user-help" class="account-help">{move||if selected.get().is_none()||open_login.get(){"请输入登录账号及至少12字节的初始密码，保存后即可登录。"}else{"登录账号已设置；密码留空表示不修改。填写新密码会退出此用户现有的登录会话。"}}</p></Show>
    <label class="account-check"><input type="checkbox" disabled=move||selected.get().is_none() prop:checked=enabled on:change=move|e|enabled.set(event_target_checked(&e))/>"启用用户"</label><p class="account-help">"停用后，此用户将无法登录或调用模型，历史记录和 Key 会保留。"</p></section>
    <section class="user-form-section" aria-labelledby="user-policy-heading"><h3 id="user-policy-heading">"模型授权与调用限制"</h3><PolicyEditor policy=draft models=models channels=channels prefix="user" admin=true/></section>
    <section class="user-form-section" aria-labelledby="user-budget-heading"><h3 id="user-budget-heading">"消费额度"</h3><label class="account-field"><span>"累计消费上限（USD）"</span><input type="number" min="0" step="0.000001" placeholder="留空表示不限制" prop:value=limit on:input=move|e|limit.set(event_target_value(&e))/></label><p class="account-help">"按实际用量和模型价格累计费用。留空不限，0 表示无付费额度；此处设置的是金额，不是 Token 数量或月度套餐。"</p></section>
    <p id="user-error" role="alert">{move||error.get()}</p><p role="status">{move||notice.get()}</p><div class="user-actions"><button type="submit" class="primary">{move||if busy.get(){"正在保存…"}else if selected.get().is_none(){"创建用户"}else{"保存修改"}}</button><button type="button" class="ghost" on:click=move |_|{editing.set(false);owner.set(None);password.set(String::new());}>"取消"</button></div></fieldset></form></div></Show>
    </section>
    <Show when=move||keys_open.get()&&owner.get().is_some()><KeyManager ctx=ctx owner=owner models=models channels=channels form_busy=busy/></Show>
    </Show></div>}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_user_has_no_invented_login() {
        assert_eq!(
            login_username(&json!({"id":7,"name":"Andy","accounts":[]})),
            None
        );
        assert!(!login_required(false, false, false));
        assert!(login_required(false, false, true));
    }

    #[test]
    fn saved_login_uses_account_username_instead_of_display_name() {
        assert_eq!(
            login_username(&json!({"name":"Andy","accounts":[{"username":"andy-login"}]})),
            Some("andy-login".into())
        );
        assert!(!login_required(false, true, true));
        assert!(login_required(true, false, false));
    }
}
