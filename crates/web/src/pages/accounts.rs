use super::PageHead;
use crate::{Ctx, Page, account_api as api, admin_api};
use leptos::prelude::*;
use serde_json::Value;
#[component]
pub fn Accounts(ctx: Ctx) -> impl IntoView {
    let rows = RwSignal::new(Vec::<api::Account>::new());
    let users = RwSignal::new(Vec::<Value>::new());
    let selected = RwSignal::new(None::<api::Account>);
    let username = RwSignal::new(String::new());
    let display = RwSignal::new(String::new());
    let user_id = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let enabled = RwSignal::new(true);
    let error = RwSignal::new(String::new());
    let list_error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let revision = RwSignal::new(0u64);
    Effect::new(move |_| {
        revision.get();
        loading.set(true);
        leptos::task::spawn_local(async move {
            match api::accounts().await {
                Ok(v) => {
                    rows.set(v);
                    list_error.set(String::new());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    list_error.set(e.message);
                }
            }
            match admin_api::request("GET", "/api/users", None).await {
                Ok(v) => users.set(admin_api::rows(&v, "users")),
                Err(e) => {
                    ctx.note_err(&e);
                    list_error.set(e.message);
                }
            }
            loading.set(false);
        });
    });
    let reset = move || {
        selected.set(None);
        username.set(String::new());
        display.set(String::new());
        user_id.set(String::new());
        password.set(String::new());
        enabled.set(true);
        error.set(String::new());
        notice.set(String::new());
    };
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        error.set(String::new());
        notice.set(String::new());
        let editing = selected.get_untracked();
        let pw = password.get_untracked();
        if editing.is_none()
            && (!api::valid_username(username.get_untracked().trim())
                || !api::valid_password(&pw)
                || user_id
                    .get_untracked()
                    .parse::<i64>()
                    .ok()
                    .is_none_or(|n| n <= 0))
        {
            error.set("请填写合法账号、12–256 字节密码，并选择已启用的 API 用户。".into());
            return;
        }
        if editing.is_some() && !pw.is_empty() && !api::valid_password(&pw) {
            error.set("重置密码需为 12–256 字节；留空则不修改。".into());
            return;
        }
        let sensitive = editing
            .as_ref()
            .is_some_and(|a| !pw.is_empty() || (a.enabled && !enabled.get_untracked()));
        if sensitive
            && !web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message("停用或重置密码将撤销该账号所有设备的控制台登录。不会停用 API 用户或密钥。确认保存？")
                        .ok()
                })
                .unwrap_or(false)
        {
            return;
        }
        busy.set(true);
        leptos::task::spawn_local(async move {
            let result = if let Some(a) = editing {
                api::update(
                    a.id,
                    &display.get_untracked(),
                    enabled.get_untracked(),
                    (!pw.is_empty()).then_some(pw.as_str()),
                )
                .await
            } else {
                api::create(
                    username.get_untracked().trim(),
                    &pw,
                    &display.get_untracked(),
                    user_id.get_untracked().parse().unwrap_or(0),
                )
                .await
            };
            match result {
                Ok(a) => {
                    password.set(String::new());
                    username.set(a.username.clone());
                    display.set(a.display_name.clone());
                    enabled.set(a.enabled);
                    user_id.set(a.user_id.map(|v| v.to_string()).unwrap_or_default());
                    selected.set(Some(a));
                    notice.set("账号已保存。登录权限与 API 密钥权限相互独立。".into());
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
    view! {<div class="page"><PageHead title="面板账号" desc="控制台的登录身份，与 API 用户和 VKey 分开管理。"><button class="ghost" on:click=move|_|reset()>"新建账号"</button></PageHead><div class="account-admin-grid">
    <section class="card"><header class="card-head"><h2>"登录账号"</h2><span class="muted small">{move||format!("{} 个",rows.get().len())}</span></header><div class="card-body">
    <Show when=move||!list_error.get().is_empty()><div class="banner bad" role="alert">{move||list_error.get()}</div></Show><Show when=move||loading.get()><p class="muted" role="status">"正在读取账号…"</p></Show>
    <div class="table-wrap"><table><thead><tr><th>"账号 / 名称"</th><th>"角色与绑定"</th><th>"状态"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().map(|a|{let id=a.id;let name=a.username.clone();let label=a.label();let role=if a.is_root(){"管理员".into()}else{format!("普通账号 · API 用户 #{}",a.user_id.unwrap_or(0))};let state=if a.enabled{"已启用"}else{"已停用"};view!{<tr class:account-selected=move||selected.get().is_some_and(|a|a.id==id)><td data-label="账号 / 名称"><strong>{name}</strong><div class="muted small">{label}</div></td><td data-label="角色与绑定">{role}</td><td data-label="状态">{state}</td><td data-label="操作"><button class="ghost" on:click=move|_|{username.set(a.username.clone());display.set(a.display_name.clone());enabled.set(a.enabled);user_id.set(a.user_id.map(|n|n.to_string()).unwrap_or_default());password.set(String::new());selected.set(Some(a.clone()));error.set(String::new());notice.set(String::new());}>"编辑"</button></td></tr>}}).collect_view()}</tbody></table></div></div></section>
    <section class="card"><header class="card-head"><h2>{move||if selected.get().is_some(){"编辑账号"}else{"新建普通账号"}}</h2></header><div class="card-body"><form class="account-form" on:submit=submit aria-busy=move||busy.get().to_string()>
    <div class="account-field"><label for="manage-username">"登录账号"</label><input id="manage-username" name="account_username" required=true minlength="3" maxlength="64" autocomplete="off" autocapitalize="none" spellcheck="false" readonly=move||selected.get().is_some() prop:value=username aria-describedby="accounts-help accounts-error" aria-invalid=move||(!error.get().is_empty()).to_string() on:input=move|ev|{username.set(event_target_value(&ev));error.set(String::new());}/></div>
    <div class="account-field"><label for="manage-display">"显示名称"</label><input id="manage-display" name="display_name" maxlength="80" autocomplete="off" prop:value=display on:input=move|ev|display.set(event_target_value(&ev))/></div>
    {move||if selected.get().is_none(){view!{<div class="account-field"><label for="manage-api-user">"绑定 API 用户"</label><select id="manage-api-user" name="user_id" required=true prop:value=user_id aria-describedby="accounts-help accounts-error" on:change=move|ev|user_id.set(event_target_value(&ev))><option value="">"选择已启用的用户"</option>{move||users.get().into_iter().filter(|u|u["enabled"].as_bool()==Some(true)).map(|u|{let id=u["id"].as_i64().unwrap_or(0);view!{<option value=id.to_string()>{format!("#{} · {}",id,u["name"].as_str().unwrap_or(""))}</option>}}).collect_view()}</select><button type="button" class="text-link" on:click=move|_|ctx.go(Page::Users)>"管理 API 用户"</button></div><crate::account_ui::PasswordField id="manage-password" label="初始密码" value=password autocomplete="new-password" error=error describedby="accounts-help accounts-error"/>}.into_any()}else if selected.get().is_some_and(|a|a.is_root()){view!{<p class="account-help">"管理员账号不能停用。本人可在账号安全页修改登录账号或密码。"</p><button type="button" class="ghost" on:click=move|_|ctx.go(Page::Account)>"前往账号安全"</button>}.into_any()}else{view!{<p class="account-help">{move||format!("绑定 API 用户 #{}；账号名与绑定在此处不可修改。",user_id.get())}</p><label class="account-check"><input type="checkbox" prop:checked=enabled on:change=move|ev|enabled.set(event_target_checked(&ev))/>"允许登录控制台"</label><crate::account_ui::PasswordField id="manage-reset-password" label="重置密码（留空不修改）" value=password autocomplete="new-password" error=error describedby="accounts-help accounts-error" required=false/>}.into_any()}}
    <p id="accounts-help" class="account-help">"账号名为 3–64 位字母、数字、点、下划线或连字符，保存为小写。创建登录账号不会新增 API 用户或密钥。停用只撤控制台登录；重置密码会退出该账号所有设备，均不改变 API 权限。"</p><div id="accounts-error" class="account-error" role="alert">{move||error.get()}</div><div class="account-success" role="status">{move||notice.get()}</div><button class="primary" type="submit" disabled=move||busy.get()>{move||if busy.get(){"正在保存…"}else if selected.get().is_some(){"保存修改"}else{"创建账号"}}</button>
    </form></div></section></div></div>}
}
