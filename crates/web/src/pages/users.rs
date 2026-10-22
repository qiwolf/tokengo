use super::{
    PageHead,
    key_manager::KeyManager,
    policy_editor::{self as policy, PolicyEditor},
};
use crate::{Ctx, Page, account_api, admin_api as api};
use leptos::prelude::*;
use serde_json::{Value, json};
/// Rejection sampling avoids modulo bias; every password contains all four character classes.
fn password_from_random(mut fill:impl FnMut(&mut [u8])->Result<(),String>)->Result<String,String>{
    const CLASSES:[&[u8];4]=[b"ABCDEFGHIJKLMNOPQRSTUVWXYZ",b"abcdefghijklmnopqrstuvwxyz",b"0123456789",b"!@#$%*-_+=?"];
    let alphabet=CLASSES.concat();
    let mut pick=|bound:usize|->Result<usize,String>{let ceiling=256-(256%bound);for _ in 0..1024 {let mut byte=[0u8];fill(&mut byte)?;if (byte[0] as usize)<ceiling{return Ok(byte[0] as usize%bound);}}Err("无法生成安全随机密码，请重试或手动设置。".into())};
    let mut bytes=Vec::with_capacity(24);for class in CLASSES {bytes.push(class[pick(class.len())?]);}while bytes.len()<24 {bytes.push(alphabet[pick(alphabet.len())?]);}for i in (1..bytes.len()).rev(){let j=pick(i+1)?;bytes.swap(i,j);}
    String::from_utf8(bytes).map_err(|_|"无法生成安全随机密码。".into())
}
fn generate_password()->Result<String,String>{
    let crypto=web_sys::window().and_then(|w|w.crypto().ok()).ok_or("浏览器不支持安全随机数，请手动设置初始密码。")?;
    password_from_random(|bytes|crypto.get_random_values_with_u8_array(bytes).map(|_|()).map_err(|_|"安全随机数生成失败，请重试或手动设置初始密码。".into()))
}
#[derive(Clone)]
struct CreatedCredentials {username:String,display_name:String,password:String}
impl CreatedCredentials {
    fn from_saved(creating:bool,user:&Value,password:&str)->Option<Self>{
        if !creating||user["id"].as_i64().is_none_or(|id|id<=0)||password.is_empty(){return None;}
        Some(Self{username:login_username(user)?,display_name:api::text(user,"name"),password:password.into()})
    }
    fn text(&self)->String{format!("登录账号：{}\n显示名称：{}\n初始密码：{}",self.username,self.display_name,self.password)}
}
fn login_username(user: &Value) -> Option<String> {
    api::rows(user, "accounts").iter().find_map(|account| {
        let username = api::text(account, "username");
        (!username.is_empty()).then_some(username)
    })
}

fn login_required(creating: bool, has_login: bool, open_login: bool) -> bool {
    creating || (!has_login && open_login)
}

/// One draft is shared by every section; switching sections never rebuilds it.
#[derive(Clone, Debug, PartialEq)]
struct UserDraft {
    username: String,
    name: String,
    password: String,
    enabled: bool,
    policy: Value,
    limit: String,
}
impl UserDraft {
    fn from_user(user: &Value) -> Self {
        Self {
            username: login_username(user).unwrap_or_default(),
            name: api::text(user, "name"), password: String::new(),
            enabled: user["enabled"] == true, policy: user["policy"].clone(),
            limit: user["budget"]["limit_micro"].as_i64().map(policy::money_input).unwrap_or_default(),
        }
    }
    fn payload(&self, creating: bool, has_login: bool, open_login: bool) -> Result<Value, String> {
        policy::validate(&self.policy)?;
        if self.name.trim().is_empty() { return Err("请填写显示名称。".into()); }
        if login_required(creating, has_login, open_login) {
            if !account_api::valid_username(self.username.trim()) { return Err("登录名需为3–64位字母、数字、点、下划线或连字符。".into()); }
            if !account_api::valid_password(&self.password) { return Err("初始密码需为12–256字节。".into()); }
        } else if !self.password.is_empty() && !account_api::valid_password(&self.password) {
            return Err("新密码需为12–256字节；留空不修改。".into());
        }
        let mut body = json!({"display_name":self.name.trim(),"policy":self.policy,"limit_micro":policy::optional_limit(&self.limit)?});
        if creating {
            body["username"] = json!(self.username.trim()); body["password"] = json!(self.password);
        } else {
            body["enabled"] = json!(self.enabled);
            if has_login && !self.password.is_empty() { body["password"] = json!(self.password); }
        }
        Ok(body)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum UserSection { Profile, Access, Limits, Keys }

#[component]
pub fn Users(ctx: Ctx) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Value>::new());
    let roots = RwSignal::new(Vec::<Value>::new());
    let selected = RwSignal::new(None::<Value>);
    let delete_target=RwSignal::new(None::<Value>);
    let delete_open=RwSignal::new(false);
    let owner = RwSignal::new(None::<i64>);
    let editing = RwSignal::new(false);
    let section = RwSignal::new(UserSection::Profile);
    let key_editor_active = RwSignal::new(false);
    let open_login = RwSignal::new(false);
    let username = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let created_credentials=RwSignal::new(None::<CreatedCredentials>);
    let credentials_copy_note=RwSignal::new(String::new());
    let credentials_copy_failed=RwSignal::new(false);
    let enabled = RwSignal::new(true);
    let draft = RwSignal::new(policy::default_policy());
    let limit = RwSignal::new(String::new());
    let models = RwSignal::new(Vec::<String>::new());
    let services=RwSignal::new(Vec::<Value>::new());
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
                    channels.set(
                        v.into_iter()
                            .map(|c| {
                                (
                                    c.id,
                                    format!(
                                        "{} · {}",
                                        c.provider_name.unwrap_or_else(|| "未分组".into()),
                                        c.name
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
            match crate::api::model_routes().await {
                Ok(catalog) => models.set(policy::visible_model_names(&catalog)),
                Err(e) => {
                    // Do not fall back to channel declarations: they include hidden models.
                    models.set(Vec::new()); ctx.note_err(&e);
                    list_error.set(format!("可见模型目录读取失败：{}。已有授权保留，当前不提供新增模型选项。",e.message));
                }
            }
            match api::request("GET","/api/passthrough-routes",None).await {
                Ok(v)=>services.set(v.as_array().cloned().unwrap_or_default()),
                Err(e)=>{services.set(Vec::new());ctx.note_err(&e);list_error.set(format!("透传服务目录读取失败：{}。已有授权保留。",e.message));}
            }
            loading.set(false);
        });
    });
    let reset = move || {
        selected.set(None);
        owner.set(None);
        editing.set(true);
        section.set(UserSection::Profile);
        open_login.set(false);
        username.set(String::new());
        name.set(String::new());
        created_credentials.set(None);credentials_copy_note.set(String::new());credentials_copy_failed.set(false);
        let generated=generate_password();
        password.set(generated.as_ref().cloned().unwrap_or_default());
        enabled.set(true);
        draft.set(policy::new_user_policy());
        limit.set(String::new());
        error.set(generated.err().unwrap_or_default());
        notice.set(String::new());
    };
    Effect::new(move |_| {
        if !editing.get() {
            created_credentials.set(None);credentials_copy_note.set(String::new());credentials_copy_failed.set(false);
            password.set(String::new()); open_login.set(false); section.set(UserSection::Profile);
            if let Some(row) = selected.get_untracked() {
                let saved = UserDraft::from_user(&row);
                username.set(saved.username); name.set(saved.name); enabled.set(saved.enabled);
                draft.set(saved.policy); limit.set(saved.limit);
            }
        }
    });
    let toggle_user = move |row: Value| {
        if busy.get_untracked() { return; }
        let Some(id) = row["id"].as_i64() else { return; };
        let next = row["enabled"] != true;
        if !next && !web_sys::window().and_then(|w|w.confirm_with_message("停用此用户后，将禁止其登录和全部 API 调用。历史数据与 Key 保留。确认停用？").ok()).unwrap_or(false) { return; }
        busy.set(true); error.set(String::new()); list_error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("PATCH", &format!("/api/managed-users/{id}"), Some(json!({"enabled":next}))).await {
                Ok(v) => {
                    if selected.get_untracked().is_some_and(|u|u["id"].as_i64()==Some(id)) {
                        selected.set(Some(v["user"].clone())); enabled.set(next);
                    }
                    notice.set(if next {"用户已启用，可登录和调用授权模型。"} else {"用户已停用，登录及 API 调用已禁止。"}.into());
                    revision.update(|r|*r+=1);
                }
                Err(e) => { ctx.note_err(&e); error.set(e.message.clone()); list_error.set(e.message); }
            }
            busy.set(false);
        });
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
        let result = UserDraft {
            username: username.get_untracked(), name: name.get_untracked(), password: pw.clone(),
            enabled: enabled.get_untracked(), policy: draft.get_untracked(), limit: limit.get_untracked(),
        }.payload(creating, has_login, open_login.get_untracked());
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
        if !creating && !pw.is_empty() {created_credentials.set(None);credentials_copy_note.set(String::new());credentials_copy_failed.set(false);}
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
                            match api::request("POST",&format!("/api/managed-users/{uid}/login-credential"),Some(json!({"username":username.get_untracked().trim(),"password":pw.clone(),"display_name":name.get_untracked().trim()}))).await{Ok(r)=>{if r["user"].is_object(){v=r}},Err(e)=>{login_failed=true;error.set(format!("用户资料已保存，但登录尚未开通：{}。请修正后重新设置初始密码。",e.message))}}
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
                    if creating {
                        if let Some(delivery)=CreatedCredentials::from_saved(true,&v["user"],&pw) {
                            let text=delivery.text();created_credentials.set(Some(delivery));
                            let copied=super::settings::copy_text(&text).await;
                            credentials_copy_failed.set(!copied);
                            credentials_copy_note.set(if copied{"用户已保存，登录信息已复制到剪贴板。"}else{"用户已保存，但自动复制失败。请点击“复制登录信息”重试，或选中下方文字手动复制。"}.into());
                        } else {
                            error.set("用户保存接口返回的信息不完整，未复制登录信息；请刷新核实用户状态。".into());
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
    let delete_user=move |_| {
        if busy.get_untracked(){return;}
        let Some(row)=delete_target.get_untracked()else{return;};
        let Some(id)=row["id"].as_i64()else{return;};
        busy.set(true);error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request("DELETE",&format!("/api/managed-users/{id}"),None).await {
                Ok(_)=>{delete_open.set(false);delete_target.set(None);editing.set(false);owner.set(None);password.set(String::new());notice.set("用户已删除；登录和 API Key 已撤销，账务历史保留。".into());revision.update(|n|*n+=1);}
                Err(e)=>{ctx.note_err(&e);error.set(e.message);}
            }
            busy.set(false);
        });
    };
    view! {<div class="page admin-page users-page">
    <style>{include_str!("../../users.css")}</style>
    <PageHead title="用户" desc="管理账号、授权与消费上限；用户可自行管理 API Key。"/>
    <section class="card users-directory">
        <header class="card-head"><h2>"用户列表"</h2><div class="user-actions"><button type="button" class="ghost" disabled=move||loading.get()||busy.get() on:click=move |_|revision.update(|r|*r+=1)>"刷新"</button><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|reset()>"新建用户"</button></div></header>
        <div class="card-body">
        <div class="users-toolbar"><label class="account-field"><span>"搜索显示名称或登录账号"</span><input type="search" placeholder="搜索用户" prop:value=search on:input=move|e|search.set(event_target_value(&e))/></label><label class="account-check"><input type="checkbox" prop:checked=show_disabled on:change=move|e|show_disabled.set(event_target_checked(&e))/>"包含已停用用户"</label></div>
        <Show when=move||!notice.get().is_empty()><p class="banner good" role="status">{move||notice.get()}</p></Show>
        <Show when=move||!list_error.get().is_empty()><p class="banner bad" role="alert">{move||list_error.get()}</p></Show>
        <Show when=move||loading.get()><p role="status">"正在读取用户…"</p></Show>
        <div class="table-wrap"><table class="users-table"><thead><tr><th>"显示名称"</th><th>"登录账号"</th><th>"状态"</th><th>"消费额度"</th><th>"操作"</th></tr></thead><tbody>{move||rows.get().into_iter().filter(|u|{let query=search.get().trim().to_lowercase();(show_disabled.get()||u["enabled"]==true)&&(query.is_empty()||api::text(u,"name").to_lowercase().contains(&query)||login_username(u).unwrap_or_default().to_lowercase().contains(&query))}).map(|u|{
            let id=u["id"].as_i64().unwrap_or(0);
            let n=api::text(&u,"name");
            let account=login_username(&u).unwrap_or_else(||"未开通登录".into());
            let active=u["enabled"]==true;
            let quota=policy::budget_label(&u["budget"]);
            let delete_row=u.clone(); let status_row=u.clone(); let can_delete=id!=1;
            view!{<tr><td data-label="显示名称"><strong>{n}</strong></td><td data-label="登录账号">{account}</td><td data-label="状态"><span class=if active {"badge good"}else{"badge"}>{if active {"已启用"}else{"已停用"}}</span></td><td data-label="消费额度"><span class="users-quota">{quota}</span></td><td data-label="操作"><div class="users-row-actions"><button type="button" class="ghost" disabled=move||busy.get()||loading.get() on:click=move |_|{
                created_credentials.set(None);credentials_copy_note.set(String::new());credentials_copy_failed.set(false);
                let saved=UserDraft::from_user(&u); owner.set(Some(id)); username.set(saved.username);name.set(saved.name);password.set(String::new());enabled.set(saved.enabled);draft.set(saved.policy);limit.set(saved.limit);selected.set(Some(u.clone()));error.set(String::new());notice.set(String::new());open_login.set(false);section.set(UserSection::Profile);editing.set(true);
            }>"管理"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|toggle_user(status_row.clone())>{if active {"停用"}else{"启用"}}</button><button type="button" class="ghost danger" disabled=move||busy.get()||!can_delete title=if can_delete{"删除用户，撤销登录和 API Key，保留历史"}else{"保留的管理员身份不可删除"} on:click=move |_|{delete_target.set(Some(delete_row.clone()));error.set(String::new());delete_open.set(true);}>"删除"</button></div></td></tr>}
        }).collect_view()}</tbody></table></div>
        <Show when=move||!loading.get()&&rows.get().iter().all(|u|{let q=search.get().trim().to_lowercase();(!show_disabled.get()&&u["enabled"]!=true)||(!q.is_empty()&&!api::text(u,"name").to_lowercase().contains(&q)&&!login_username(u).unwrap_or_default().to_lowercase().contains(&q))})><p class="users-empty">"没有符合条件的用户。可调整筛选或新建用户。"</p></Show>
        <footer class="users-directory-footer"><span>{move||roots.get().first().map(|a|format!("管理员 {} 的登录信息在账号安全中维护。",api::text(a,"username"))).unwrap_or_default()}</span><button type="button" class="ghost" on:click=move |_|ctx.go(Page::Account)>"账号安全"</button></footer>
        </div>
    </section>
    <super::ModuleDialog open=editing busy=busy title="用户设置".to_string()>
    <div class="user-workspace">
        <Show when=move||created_credentials.get().is_some()><section class="user-credential-delivery" aria-labelledby="user-credential-title"><h3 id="user-credential-title">"新用户登录信息"</h3><p class="account-help">"用户已保存。下列信息仅在当前页面内存中保留，关闭此处或用户设置后会清除；请及时交付并安全保存。"</p><label class="account-field" for="user-credential-text"><span>"可选择并手动复制"</span><textarea id="user-credential-text" readonly=true autocomplete="off" spellcheck="false" rows="3" prop:value=move||created_credentials.get().map(|v|v.text()).unwrap_or_default()/></label><p role=move||if credentials_copy_failed.get(){"alert"}else{"status"}>{move||credentials_copy_note.get()}</p><div class="user-actions"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{let Some(delivery)=created_credentials.get_untracked()else{return};busy.set(true);leptos::task::spawn_local(async move{let copied=super::settings::copy_text(&delivery.text()).await;credentials_copy_failed.set(!copied);credentials_copy_note.set(if copied{"登录信息已复制到剪贴板。"}else{"复制失败，请选中上方文字手动复制。"}.into());busy.set(false);});}>"复制登录信息"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{created_credentials.set(None);credentials_copy_note.set(String::new());credentials_copy_failed.set(false);}>"关闭并清除"</button></div></section></Show>
        <Show when=move||!key_editor_active.get()><header class="user-identity"><div><h3>{move||selected.get().map(|v|api::text(&v,"name")).unwrap_or_else(||"新建用户".into())}</h3><p>{move||selected.get().map(|v|login_username(&v).map(|s|format!("登录账号：{s}")).unwrap_or_else(||"尚未开通登录 · 已有 Key 与历史保留".into())).unwrap_or_else(||"设置账号并分配权限，保存后用户即可登录。".into())}</p></div><Show when=move||selected.get().is_some()><span class=move||if enabled.get(){"badge good"}else{"badge"}>{move||if enabled.get(){"已启用"}else{"已停用"}}</span><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{if let Some(u)=selected.get_untracked(){toggle_user(u)}}>{move||if enabled.get(){"停用用户"}else{"启用用户"}}</button></Show></header>
        <nav class="user-workspace-nav" aria-label="用户设置分区">{[(UserSection::Profile,"基本资料"),(UserSection::Access,"模型与服务"),(UserSection::Limits,"限流与额度"),(UserSection::Keys,"API Key")].into_iter().map(|(tab,label)|view!{
            <button type="button" class="ghost" aria-pressed=move||(section.get()==tab).to_string() disabled=move||busy.get()||(tab==UserSection::Keys&&selected.get().is_none()) title=if tab==UserSection::Keys{"创建用户后可管理其 API Key"}else{""} on:click=move |_|section.set(tab)>{label}</button>
        }).collect_view()}</nav></Show>
        <Show when=move||section.get()!=UserSection::Keys>
        <form class="account-form user-workspace-form" on:submit=save novalidate>
        <fieldset disabled=move||busy.get()||loading.get()>
        <div class="user-workspace-panel">
        <Show when=move||section.get()==UserSection::Profile>
            <h3>"基本资料"</h3>
            <Show when=move||selected.get().is_some_and(|u|login_username(&u).is_none())><div class="user-login-state"><p>"此用户暂未开通登录，已有 API Key 继续使用原权限。"</p><label class="account-check"><input type="checkbox" prop:checked=open_login on:change=move|e|{open_login.set(event_target_checked(&e));password.set(String::new());username.set(String::new());}/>"为此用户开通登录"</label></div></Show>
            <div class="user-profile-grid"><Show when=move||selected.get().is_none()||selected.get().is_some_and(|u|login_username(&u).is_some())||open_login.get()><label class="account-field"><span>"登录账号"</span><input type="text" minlength="3" maxlength="64" autocomplete="off" autocapitalize="none" spellcheck="false" readonly=move||selected.get().is_some_and(|u|login_username(&u).is_some()) prop:value=username on:input=move|e|username.set(event_target_value(&e))/></label></Show><label class="account-field"><span>"显示名称"</span><input type="text" maxlength="80" prop:value=name on:input=move|e|name.set(event_target_value(&e))/></label></div>
            <Show when=move||selected.get().is_none()||selected.get().is_some_and(|u|login_username(&u).is_some())||open_login.get()><crate::account_ui::PasswordField id="user-password" label="设置密码" value=password autocomplete="new-password" error=error describedby="user-help user-error" required=false/><Show when=move||selected.get().is_none()><div class="user-password-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|{match generate_password(){Ok(generated)=>{password.set(generated);error.set(String::new());},Err(message)=>error.set(message)}}>"重新生成密码"</button></div></Show><p id="user-help" class="account-help">{move||if selected.get().is_none(){"默认生成24位安全随机密码，可修改或重新生成；若浏览器无法生成，请手动设置。保存成功后自动复制登录信息；复制失败可再次点击复制。"}else if open_login.get(){"请输入至少12字节的初始密码。"}else{"密码留空不修改；设置新密码将退出此用户现有登录会话。登录账号创建后不可在此修改。"}}</p></Show>
        </Show>
        <Show when=move||section.get()==UserSection::Access><h3>"模型与透传服务授权"</h3><PolicyEditor policy=draft models=models channels=channels services=services prefix="user-access" admin=true show_limits=false/></Show>
        <Show when=move||section.get()==UserSection::Limits><h3>"限流与额度"</h3><PolicyEditor policy=draft models=models channels=channels services=services prefix="user-limits" admin=true show_access=false/><label class="account-field user-budget-field"><span>"累计消费上限（USD）"</span><input type="number" min="0" step="0.000001" placeholder="留空不限制" prop:value=limit on:input=move|e|limit.set(event_target_value(&e))/></label><p class="account-help">"按实际用量与模型价格累计费用。留空不限，0 表示无付费额度；此处是累计金额，不是月度套餐或 Token 余额。"</p></Show>
        </div>
        <Show when=move||!error.get().is_empty()><p id="user-error" class="banner bad" role="alert">{move||error.get()}</p></Show><Show when=move||!notice.get().is_empty()><p class="banner good" role="status">{move||notice.get()}</p></Show>
        <footer class="user-workspace-footer"><span>"保存会同时应用基本资料、授权和额度。"</span><div class="user-actions"><button type="button" class="ghost" on:click=move |_|editing.set(false)>"取消"</button><button type="submit" class="primary">{move||if busy.get(){"正在保存…"}else if selected.get().is_none(){"创建用户"}else{"保存修改"}}</button></div></footer>
        </fieldset></form>
        </Show>
        <Show when=move||editing.get()&&section.get()==UserSection::Keys&&owner.get().is_some()><Show when=move||!key_editor_active.get()><p class="account-help">"Key 操作独立保存；其他分区的草稿保留，返回后可继续编辑。"</p></Show><KeyManager ctx=ctx owner=owner models=models channels=channels services=services form_busy=busy embedded=true editor_active=key_editor_active/></Show>
    </div>
    </super::ModuleDialog>
    <super::ModuleDialog open=delete_open busy=busy title="删除用户".to_string()>
        <p>{move||delete_target.get().map(|u|format!("确定删除用户“{}”？",api::text(&u,"name"))).unwrap_or_default()}</p><p>"将撤销该用户的登录和全部 API Key，移除调用策略与授权。历史请求与账单保留；删除后不能从列表恢复。"</p>
        <p class="banner bad" role="alert">{move||error.get()}</p><div class="module-dialog-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|delete_open.set(false)>"取消"</button><button type="button" class="danger" disabled=move||busy.get() on:click=delete_user>"确认删除用户"</button></div>
    </super::ModuleDialog></div>}
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
    fn saved_user() -> Value {
        json!({"id":7,"name":"Andy","enabled":true,
            "accounts":[{"username":"andy-login"}],
            "policy":{"allowed_models":["legacy-hidden"],"allowed_channels":[],"allow_paid":false,"admin_access":true,"rpm":3,"tpm":null,"max_concurrent":2},
            "budget":{"limit_micro":12345678}})
    }
    #[test]
    fn cross_section_save_keeps_identity_acl_limits_and_zero_budget() {
        let saved = saved_user();
        let mut form = UserDraft::from_user(&saved);
        // Profile, access and quota edits share one draft; no tab supplies defaults.
        form.name = "Changed".into();
        form.policy["rpm"] = json!(9);
        form.limit = "0".into();
        let body = form.payload(false, true, false).unwrap();
        assert_eq!(body["display_name"], "Changed");
        assert_eq!(body["policy"]["allowed_models"], json!(["legacy-hidden"]));
        assert_eq!(body["policy"]["allowed_channels"], json!([]));
        assert_eq!(body["policy"]["admin_access"], true);
        assert_eq!(body["policy"]["allow_paid"], false);
        assert_eq!(body["policy"]["rpm"], 9);
        assert_eq!(body["limit_micro"], 0);
        assert!(body.get("password").is_none());
        assert!(body.get("username").is_none());
    }
    #[test]
    fn cancelling_reloads_saved_snapshot_and_forgets_password() {
        let saved = saved_user();
        let original = UserDraft::from_user(&saved);
        let mut dirty = original.clone();
        dirty.name = "Unsaved".into(); dirty.password = "temporary-password".into();
        dirty.policy["allowed_models"] = Value::Null; dirty.limit.clear();
        assert_ne!(dirty, original);
        // This is the same restoration used by Cancel, dialog Close and Escape.
        dirty = UserDraft::from_user(&saved);
        assert_eq!(dirty, original);
        assert!(dirty.password.is_empty());
        assert_eq!(dirty.username, "andy-login");
        assert_eq!(dirty.limit, "12.345678");
    }
    #[test]
    fn historical_user_edit_does_not_silently_create_login_or_grants() {
        let mut saved = saved_user(); saved["accounts"] = json!([]);
        saved["policy"]["allowed_models"] = json!([]);
        let form = UserDraft::from_user(&saved);
        let body = form.payload(false, false, false).unwrap();
        assert!(body.get("username").is_none());
        assert!(body.get("password").is_none());
        assert_eq!(body["policy"]["allowed_models"], json!([]));
        assert!(form.payload(false, false, true).is_err());
    }

    #[test]
    fn generated_password_is_24_ascii_bytes_with_all_required_classes() {
        let mut next=0u8;
        let password=password_from_random(|bytes|{for byte in bytes {*byte=next;next=next.wrapping_add(37);}Ok(())}).unwrap();
        assert_eq!(password.len(),24);assert!(password.is_ascii());
        assert!(password.bytes().any(|b|b.is_ascii_uppercase()));assert!(password.bytes().any(|b|b.is_ascii_lowercase()));assert!(password.bytes().any(|b|b.is_ascii_digit()));assert!(password.bytes().any(|b|!b.is_ascii_alphanumeric()));
        assert!(account_api::valid_password(&password));
    }
    #[test]
    fn secure_random_failure_never_falls_back_to_predictable_password() {
        assert!(password_from_random(|_|Err("fixture random unavailable".into())).is_err());
        assert!(password_from_random(|bytes|{bytes.fill(255);Ok(())}).is_err());
    }
    #[test]
    fn credentials_delivery_requires_successful_saved_identity_and_never_existing_edit() {
        let saved=saved_user();
        assert!(CreatedCredentials::from_saved(false,&saved,"fixture-initial-password").is_none());
        assert!(CreatedCredentials::from_saved(true,&Value::Null,"fixture-initial-password").is_none());
        assert!(CreatedCredentials::from_saved(true,&json!({"id":7,"name":"without login"}),"fixture-initial-password").is_none());
        assert!(CreatedCredentials::from_saved(true,&saved,"").is_none());
        let delivery=CreatedCredentials::from_saved(true,&saved,"fixture-initial-password").unwrap();
        assert_eq!(delivery.text(),"登录账号：andy-login\n显示名称：Andy\n初始密码：fixture-initial-password");
        assert!(UserDraft::from_user(&saved).password.is_empty());
    }

}
