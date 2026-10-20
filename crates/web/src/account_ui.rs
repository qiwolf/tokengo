//! Account screens share cookie authentication, without exposing session credentials.
use crate::{Ctx, Phase, account_api as account};
use leptos::prelude::*;

#[component]
pub fn PasswordField(
    id: &'static str,
    label: &'static str,
    value: RwSignal<String>,
    autocomplete: &'static str,
    error: RwSignal<String>,
    describedby: &'static str,
    #[prop(default = true)] required: bool,
) -> impl IntoView {
    let shown = RwSignal::new(false);
    view! {<div class="account-field"><label for=id>{label}</label><div class="password-control">
    <input id=id name=id type=move||if shown.get(){"text"}else{"password"} autocomplete=autocomplete required=required maxlength="256" prop:value=value aria-invalid=move||(!error.get().is_empty()).to_string() aria-describedby=describedby on:input=move|ev|{value.set(event_target_value(&ev));error.set(String::new());}/>
    <button type="button" class="password-toggle" aria-controls=id aria-pressed=move||shown.get().to_string() aria-label=move||format!("{}{}",if shown.get(){"隐藏"}else{"显示"},label) on:click=move|_|shown.update(|v|*v=!*v)>{move||if shown.get(){"隐藏"}else{"显示"}}</button>
    </div></div>}
}
#[component]
pub fn AuthScreen(ctx: Ctx, #[prop(default = false)] setup: bool) -> impl IntoView {
    let saved_setup = RwSignal::new(None::<account::SetupResponse>);
    let copy_note = RwSignal::new(String::new());
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let display = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        if setup
            && (!account::valid_password(&password.get_untracked())
                || password.get_untracked() != confirm.get_untracked())
        {
            error.set("新密码需为 12–256 字节，两次输入必须一致。".into());
            return;
        }
        if !setup
            && (username.get_untracked().trim().is_empty() || password.get_untracked().is_empty())
        {
            error.set("请输入账号和密码。".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            let result = if setup {
                account::setup(&password.get_untracked(), &display.get_untracked()).await.map(|response|{saved_setup.set(Some(response));None})
            } else {
                account::login(username.get_untracked().trim(), &password.get_untracked()).await.map(Some)
            };
            match result {
                Ok(a) => {
                    password.set(String::new());
                    confirm.set(String::new());
                    if let Some(a)=a {ctx.accept_account(a);}
                }
                Err(e) => error.set(e.message),
            }
            busy.set(false);
        });
    };
    view! {<div class="account-auth"><header class="auth-brand"><strong>"Token-Go"</strong><span>"控制台"</span><button type="button" class="ghost" on:click=move|_|ctx.toggle_theme()>{move||if ctx.theme.get().is_light(){"深色"}else{"浅色"}}</button></header>
    <main class="account-auth-main"><section class="account-auth-card" aria-labelledby="auth-title"><div class="auth-heading"><p class="eyebrow">{if setup{"首次设置"}else{"欢迎回来"}}</p><h1 id="auth-title">{if setup{"创建管理员账号"}else{"登录控制台"}}</h1><p class="muted">{if setup{"初始账号为 admin，登录后可在账号安全中修改。"}else{"使用管理员为你分配的账号和密码。"}}</p></div>
    <Show when=move||ctx.toast.get().is_some()><p class="account-success" role="status">{move||ctx.toast.get().unwrap_or_default()}</p></Show><Show when=move||saved_setup.get().is_none()><form on:submit=submit aria-busy=move||busy.get().to_string()>
    <div class="account-field"><label for="account-username">"账号"</label><input id="account-username" name="username" autocomplete="username" required=true readonly=setup maxlength="64" autocapitalize="none" spellcheck="false" prop:value=move||if setup{"admin".to_string()}else{username.get()} aria-describedby="auth-help auth-error" aria-invalid=move||(!error.get().is_empty()).to_string() on:input=move|ev|{username.set(event_target_value(&ev));error.set(String::new());}/></div>
    {setup.then(||view!{<div class="account-field"><label for="setup-display">"显示名称（可选）"</label><input id="setup-display" name="display_name" autocomplete="name" maxlength="80" prop:value=display on:input=move|ev|display.set(event_target_value(&ev))/></div>})}
    <PasswordField id="account-password" label="密码" value=password autocomplete=if setup{"new-password"}else{"current-password"} error=error describedby="auth-help auth-error"/>
    {setup.then(||view!{<PasswordField id="account-confirm" label="确认密码" value=confirm autocomplete="new-password" error=error describedby="auth-help auth-error"/>})}
    <p id="auth-help" class="account-help">{if setup{"新密码需为 12–256 字节。请使用独立、难以猜测的密码。"}else{"使用登录账号与密码；不要填写模型 API Key。账号由管理员开通。"}}</p>
    <div id="auth-error" class="account-error" role="alert">{move||error.get()}</div>
    <button class="primary auth-submit" type="submit" disabled=move||busy.get()>{move||if busy.get(){"请稍候…"}else if setup{"创建并保存密钥"}else{"登录"}}</button>
    </form></Show>
    <Show when=move||saved_setup.get().is_some()><section aria-labelledby="setup-key-title" class="account-form"><h2 id="setup-key-title">"保存 Gateway Key"</h2><p class="account-help">"这是调用模型 API 的密钥，不是控制台登录凭据。请保存到密码管理器；离开此步骤会从本页清除。请勿分享或截图。"</p><div class="account-field"><label for="setup-key">{move||if saved_setup.get().is_some_and(|r|r.gateway_key_generated){"本次生成的 Gateway Key"}else{"Gateway Key"}}</label><textarea id="setup-key" readonly=true autocomplete="off" spellcheck="false" rows="3" prop:value=move||saved_setup.get().map(|r|r.gateway_key).unwrap_or_default()/></div><button type="button" class="ghost" on:click=move|_|leptos::task::spawn_local(async move{if let Some(r)=saved_setup.get_untracked(){let copied=crate::pages::settings::copy_text(&r.gateway_key).await;copy_note.set(if copied{"已复制，请安全保存。"}else{"复制失败，请手动选中上方密钥并保存。"}.into());}})>"复制密钥"</button><p role="status" class="account-help">{move||copy_note.get()}</p><button type="button" class="primary auth-submit" on:click=move|_|{if let Some(mut r)=saved_setup.get_untracked(){saved_setup.set(None);copy_note.set(String::new());r.gateway_key.clear();ctx.accept_account(r.account);}}>"已安全保存，进入控制台"</button></section></Show>
    </section><p class="auth-footer">"仅授权账号可访问 · 会话由服务器安全管理"</p></main></div>}
}
#[component]
pub fn PasswordForm(ctx: Ctx) -> impl IntoView {
    let old = RwSignal::new(String::new());
    let new = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        if old.get_untracked().is_empty()
            || !account::valid_password(&new.get_untracked())
            || new.get_untracked() != confirm.get_untracked()
        {
            error.set("请输入原密码；新密码需为 12–256 字节，且两次输入一致。".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match account::password(&old.get_untracked(), &new.get_untracked()).await {
                Ok(()) => {
                    old.set(String::new());
                    new.set(String::new());
                    confirm.set(String::new());
                    ctx.account.set(None);
                    ctx.phase.set(Phase::Login);
                    ctx.say("密码已更新，请使用新密码重新登录。");
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<section class="card account-password-card"><header class="card-head"><h2>"更改密码"</h2></header><div class="card-body"><form class="account-form" on:submit=submit aria-busy=move||busy.get().to_string()>
    <div class="account-field"><label for="password-username">"账号"</label><input id="password-username" name="username" autocomplete="username" readonly=true prop:value=move||ctx.account.get().map(|a|a.username).unwrap_or_default()/></div>
    <PasswordField id="password-old" label="原密码" value=old autocomplete="current-password" error=error describedby="password-help password-error"/>
    <PasswordField id="password-new" label="新密码" value=new autocomplete="new-password" error=error describedby="password-help password-error"/>
    <PasswordField id="password-confirm" label="确认新密码" value=confirm autocomplete="new-password" error=error describedby="password-help password-error"/>
    <p id="password-help" class="account-help">"更新后会撤销该账号的所有登录会话，包括当前会话。API 密钥不受影响。"</p><div id="password-error" class="account-error" role="alert">{move||error.get()}</div><button class="primary" type="submit" disabled=move||busy.get()>{move||if busy.get(){"正在更新…"}else{"更新密码并退出"}}</button>
    </form></div></section>}
}
#[component]
pub fn Profile(ctx: Ctx) -> impl IntoView {
    view! {<div class="page"><crate::pages::PageHead title="账号安全" desc="管理当前控制台账号的密码与登录会话。"/><Show when=move||ctx.is_root()><RenameForm ctx=ctx/></Show><PasswordForm ctx=ctx/></div>}
}
#[component]
pub fn RenameForm(ctx: Ctx) -> impl IntoView {
    let name = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        if !account::valid_username(name.get_untracked().trim())
            || password.get_untracked().is_empty()
        {
            error.set("新账号需为 3–64 位字母、数字、点、下划线或连字符，并确认当前密码。".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match account::rename(name.get_untracked().trim(), &password.get_untracked()).await {
                Ok(()) => {
                    password.set(String::new());
                    ctx.account.set(None);
                    ctx.phase.set(Phase::Login);
                    ctx.say("登录账号已更新，旧账号名已失效。请以新账号重新登录。");
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<section class="card account-password-card"><header class="card-head"><h2>"修改登录账号"</h2></header><div class="card-body"><form class="account-form" on:submit=submit aria-busy=move||busy.get().to_string()>
    <p class="muted">"当前账号："<strong>{move||ctx.account.get().map(|a|a.username).unwrap_or_default()}</strong></p>
    <div class="account-field"><label for="profile-username">"新登录账号"</label><input id="profile-username" name="new_username" autocomplete="off" autocapitalize="none" spellcheck="false" required=true minlength="3" maxlength="64" aria-describedby="profile-help profile-error" aria-invalid=move||(!error.get().is_empty()).to_string() prop:value=name on:input=move|ev|{name.set(event_target_value(&ev));error.set(String::new());}/></div>
    <PasswordField id="profile-password" label="当前密码" value=password autocomplete="current-password" error=error describedby="profile-help profile-error"/>
    <p id="profile-help" class="account-help">"账号会保存为小写。修改后旧账号名立即失效，所有管理会话退出；现有模型授权、额度与 API Key 不变。"</p><div id="profile-error" class="account-error" role="alert">{move||error.get()}</div><button type="submit" class="primary" disabled=move||busy.get()>{move||if busy.get(){"正在更新…"}else{"更新账号并退出"}}</button>
    </form></div></section>}
}
