//! Root-only management of independent machine credentials. Secrets live only in this view.
use leptos::prelude::*;
use serde::Deserialize;
use serde_json::json;
use crate::{api, fmt, Ctx};

#[derive(Clone, Deserialize)]
struct Token {
    id: i64,
    public_id: String,
    name: String,
    created_at_ms: i64,
    expires_at_ms: i64,
    last_used_at_ms: Option<i64>,
    status: String,
}
#[derive(Deserialize)]
struct TokenList { tokens: Vec<Token>, next_before_id: Option<i64> }
#[derive(Deserialize)]
struct CreatedToken { token: Token, secret: String }

#[component]
pub fn OperationsTokens(ctx: Ctx) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Token>::new());
    let next = RwSignal::new(None::<i64>);
    let busy = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let validation = RwSignal::new(false);
    let notice = RwSignal::new(None::<String>);
    let create_open = RwSignal::new(false);
    let revoke_open = RwSignal::new(false);
    let all_open = RwSignal::new(false);
    let selected = RwSignal::new(None::<Token>);
    let name = RwSignal::new(String::new());
    let days = RwSignal::new("90".to_string());
    let password = RwSignal::new(String::new());
    let secret = RwSignal::new(String::new());
    let copied = RwSignal::new(None::<bool>);

    // ModuleDialog routes Escape and its close button through these same signals.
    Effect::new(move |_| {
        if !create_open.get() {
            password.set(String::new());
            secret.set(String::new());
            name.set(String::new());
            validation.set(false);
            days.set("90".into());
            copied.set(None);
        }
        if !all_open.get() && !create_open.get() { password.set(String::new()); }
        if !revoke_open.get() { selected.set(None); }
    });

    let load = move |before: Option<i64>| {
        if busy.get_untracked() { return; }
        busy.set(true);
        error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            let path = before.map(|id| format!("/api/ops-tokens?limit=20&before_id={id}"))
                .unwrap_or_else(|| "/api/ops-tokens?limit=20".into());
            match api::typed::<TokenList>("GET", &path, None).await {
                Ok(result) => {
                    if before.is_some() { rows.update(|items| items.extend(result.tokens)); }
                    else { rows.set(result.tokens); }
                    next.set(result.next_before_id);
                    loaded.set(true);
                }
                Err(e) => { ctx.note_err(&e); error.set(Some(e.message)); }
            }
            busy.set(false);
        });
    };
    load(None);

    let create = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() { return; }
        let token_name = name.get_untracked().trim().to_string();
        let lifetime = days.get_untracked().parse::<u16>().ok().filter(|n| (1..=365).contains(n));
        if token_name.is_empty() || token_name.chars().count() > 64 || lifetime.is_none() || password.get_untracked().is_empty() {
            validation.set(true); error.set(Some("请输入 1–64 字名称、1–365 的整数天数及当前密码。".into())); return;
        }
        validation.set(false);
        let body = json!({"name": token_name, "expires_in_days": lifetime.unwrap(), "password": password.get_untracked()});
        password.set(String::new());
        error.set(None); busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api::typed::<CreatedToken>("POST", "/api/ops-tokens", Some(body)).await {
                Ok(result) => {
                    secret.set(result.secret);
                    notice.set(Some(format!("已创建运维令牌「{}」。", result.token.name)));
                    busy.set(false); load(None);
                }
                Err(e) => { ctx.note_err(&e); error.set(Some(e.message)); busy.set(false); }
            }
        });
    };
    let revoke = move |_| {
        if busy.get_untracked() { return; }
        let Some(token) = selected.get_untracked() else { return; };
        busy.set(true); error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            match api::call("DELETE", &format!("/api/ops-tokens/{}", token.id), None).await {
                Ok(_) => { revoke_open.set(false); notice.set(Some("令牌已撤销，后续请求将被拒绝。".into())); busy.set(false); load(None); }
                Err(e) => { ctx.note_err(&e); error.set(Some(e.message)); busy.set(false); }
            }
        });
    };
    let revoke_all = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() || password.get_untracked().is_empty() { return; }
        let body = json!({"password": password.get_untracked()});
        password.set(String::new()); busy.set(true); error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            match api::call("POST", "/api/ops-tokens/revoke-all", Some(body)).await {
                Ok(_) => { all_open.set(false); notice.set(Some("全部现有运维令牌已撤销。".into())); busy.set(false); load(None); }
                Err(e) => { ctx.note_err(&e); error.set(Some(e.message)); busy.set(false); }
            }
        });
    };

    view! {
        <section class="card">
            <header class="card-head"><h2>"运维令牌"</h2><p class="muted small">"给可信智能体的高权限站点运维凭据。可修改渠道、路由、用户和费用策略；请保存在秘密存储中。"</p></header>
            <div class="card-body">
                <p class="muted small">"所有令牌只有一种权限：站点运维。普通修改 root 用户名或密码不会撤销令牌；安全重置会撤销全部令牌。令牌不能管理 root 安全或签发运维令牌。"</p>
                <div class="toolbar">
                    <button type="button" class="primary" disabled=move || busy.get() on:click=move |_| { error.set(None); create_open.set(true); }>"创建运维令牌"</button>
                    <button type="button" class="ghost" disabled=move || busy.get() on:click=move |_| load(None)>"刷新令牌"</button>
                    <button type="button" class="ghost danger" disabled=move || busy.get() || !loaded.get() on:click=move |_| { error.set(None); password.set(String::new()); all_open.set(true); }>"撤销全部"</button>
                </div>
                <Show when=move || busy.get()><p role="status" class="muted small">"正在处理运维令牌…"</p></Show>
                <Show when=move || !create_open.get() && !revoke_open.get() && !all_open.get()>{move || error.get().map(|e| view!{<p class="banner bad" role="alert">{e}</p>})}</Show>
                {move || notice.get().map(|m| view!{<p role="status" class="muted small">{m}</p>})}
                <Show when=move || loaded.get() && rows.get().is_empty()><p class="muted">"尚无运维令牌，点击「创建运维令牌」接入可信智能体。"</p></Show>
                <For each=move || rows.get() key=|token| (token.id, token.status.clone(), token.last_used_at_ms) children=move |token| {
                    let selected_token = token.clone();
                    let status = match token.status.as_str() { "active" => "有效", "expired" => "已到期", "revoked" => "已撤销", _ => "未知状态" };
                    let active = token.status == "active";
                    view! {
                        <div class="panel">
                            <h3>{token.name.clone()}</h3>
                            <p class="muted small"><code>{token.public_id}</code>" · "{status}" · 站点运维"</p>
                            <p class="muted small">{format!("创建：{} · 到期：{}", fmt::ts(token.created_at_ms), fmt::ts(token.expires_at_ms))}</p>
                            <p class="muted small">{format!("最近使用（约）：{}", token.last_used_at_ms.map(fmt::ts).unwrap_or_else(|| "尚未使用".into()))}</p>
                            <button type="button" class="ghost danger" disabled=move || busy.get() || !active aria-label=format!("撤销运维令牌 {}", token.name) on:click=move |_| { selected.set(Some(selected_token.clone())); error.set(None); revoke_open.set(true); }>"撤销"</button>
                        </div>
                    }
                }/>
                <Show when=move || next.get().is_some()><button type="button" class="ghost" disabled=move || busy.get() on:click=move |_| load(next.get_untracked())>"加载更多令牌"</button></Show>
            </div>
            <super::ModuleDialog open=create_open busy=busy title="创建运维令牌".to_string()>
                <Show when=move || secret.get().is_empty() fallback=move || view! {
                    <p class="banner bad">"完整令牌仅展示这一次。关闭后无法恢复，请先保存到智能体的秘密存储中。"</p>
                    <label for="ops-token-secret">"完整 Bearer 令牌（可手动选择复制）"</label>
                    <textarea id="ops-token-secret" readonly rows="4" spellcheck="false" prop:value=secret />
                    <p class="muted small">{format!("实例地址：{}", web_sys::window().and_then(|w|w.location().origin().ok()).unwrap_or_default())}</p>
                    <p><code>"Authorization: Bearer <完整令牌>"</code></p>
                    <p class="muted small">"先调用 GET /api/ops/me 确认身份，再读取 GET /api/ops/capabilities。不要把令牌放进 URL、聊天记录或代码仓库。"</p>
                    <button type="button" class="primary" disabled=move || busy.get() on:click=move |_| {
                        if busy.get_untracked() { return; }
                        busy.set(true);
                        wasm_bindgen_futures::spawn_local(async move { copied.set(Some(super::settings::copy_text(&secret.get_untracked()).await)); busy.set(false); });
                    }>"复制完整令牌"</button>
                    {move || copied.get().map(|ok| view!{<p role="status">{if ok {"已复制。请妥善保存后关闭。"} else {"自动复制失败，请在上方文本框中手动选择并复制。"}}</p>})}
                }>
                    <form class="account-form" on:submit=create>
                        <label for="ops-token-name">"名称"</label><input id="ops-token-name" aria-describedby="ops-create-error" aria-invalid=move || validation.get() && (name.get().trim().is_empty() || name.get().trim().chars().count() > 64) maxlength="64" required disabled=move || busy.get() prop:value=name on:input=move |ev| name.set(event_target_value(&ev)) />
                        <label for="ops-token-days">"有效期（天）"</label><input id="ops-token-days" aria-describedby="ops-create-error ops-token-lifetime" aria-invalid={move || validation.get() && !days.get().parse::<u16>().is_ok_and(|n| (1..=365).contains(&n))} type="number" min="1" max="365" step="1" required disabled=move || busy.get() prop:value=days on:input=move |ev| days.set(event_target_value(&ev)) />
                        <label for="ops-token-password">"当前 root 密码"</label><input id="ops-token-password" aria-describedby="ops-create-error" aria-invalid=move || validation.get() && password.get().is_empty() type="password" autocomplete="current-password" required disabled=move || busy.get() prop:value=password on:input=move |ev| password.set(event_target_value(&ev)) />
                        <p id="ops-token-lifetime" class="muted small">"默认 90 天，最多 365 天；到期后需创建新令牌。每站点最多 20 把有效令牌。"</p>
                        <button type="submit" class="primary" disabled=move || busy.get()>"创建并展示令牌"</button>
                    </form>
                </Show>
                {move || error.get().map(|e| view!{<p id="ops-create-error" class="banner bad" role="alert">{e}</p>})}
            </super::ModuleDialog>
            <super::ModuleDialog open=revoke_open busy=busy title="撤销运维令牌".to_string()>
                <p>{move || selected.get().map(|t| format!("撤销「{}」后，使用它的智能体无法再发起运维请求。此操作不可恢复；已提交的操作不会回滚。", t.name)).unwrap_or_default()}</p>
                {move || error.get().map(|e| view!{<p class="banner bad" role="alert">{e}</p>})}
                <button type="button" class="ghost" disabled=move || busy.get() on:click=move |_| revoke_open.set(false)>"取消"</button>
                <button type="button" class="ghost danger" disabled=move || busy.get() on:click=revoke>"确认撤销"</button>
            </super::ModuleDialog>
            <super::ModuleDialog open=all_open busy=busy title="撤销全部运维令牌".to_string()>
                <p>"所有使用现有令牌的智能体将停止接入，此操作不可恢复。请输入当前 root 密码确认。"</p>
                <form class="account-form" on:submit=revoke_all>
                    <label for="ops-revoke-password">"当前 root 密码"</label><input id="ops-revoke-password" aria-describedby="ops-revoke-error" type="password" autocomplete="current-password" required disabled=move || busy.get() prop:value=password on:input=move |ev| password.set(event_target_value(&ev)) />
                    {move || error.get().map(|e| view!{<p id="ops-revoke-error" class="banner bad" role="alert">{e}</p>})}
                    <button type="button" class="ghost" disabled=move || busy.get() on:click=move |_| all_open.set(false)>"取消"</button>
                    <button type="submit" class="ghost danger" disabled=move || busy.get() || password.get().is_empty()>"确认撤销全部"</button>
                </form>
            </super::ModuleDialog>
        </section>
    }
}
