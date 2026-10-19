//! 设置：对外密钥、面板口令与账目维护。

use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

async fn copy_text(text: &str) -> bool {
    let Some(window) = web_sys::window() else { return false };
    // HTTP origins have no Clipboard API; denied permissions must also fall back.
    if let Ok(value) = js_sys::Reflect::get(window.navigator().as_ref(), &JsValue::from_str("clipboard")) {
        if !value.is_null() && !value.is_undefined() {
            if let Ok(clipboard) = value.dyn_into::<web_sys::Clipboard>() {
                if wasm_bindgen_futures::JsFuture::from(clipboard.write_text(text)).await.is_ok() {
                    return true;
                }
            }
        }
    }
    let Some(document) = window.document() else { return false };
    let Some(body) = document.body() else { return false };
    let Ok(element) = document.create_element("textarea") else { return false };
    let Ok(area) = element.dyn_into::<web_sys::HtmlTextAreaElement>() else { return false };
    area.set_value(text);
    let _ = area.set_attribute("style", "position:fixed;left:0;top:0;opacity:0");
    let _ = area.set_attribute("readonly", "");
    if body.append_child(&area).is_err() { return false; }
    let previous = document.active_element();
    let _ = area.focus();
    area.select();
    let _ = area.set_selection_range(0, text.encode_utf16().count() as u32);
    let copied = js_sys::Reflect::get(document.as_ref(), &JsValue::from_str("execCommand"))
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .and_then(|f| f.call1(document.as_ref(), &JsValue::from_str("copy")).ok())
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    area.remove();
    if let Some(element) = previous.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = element.focus();
    }
    copied
}

use crate::api;
use crate::fmt;
use crate::pages::PageHead;
use crate::Ctx;

#[component]
fn GatewayKeys(ctx: Ctx) -> impl IntoView {
    let keys = RwSignal::new(Vec::<crate::model::GatewayKey>::new());
    let name = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let manual = RwSignal::new(None::<(i64, String)>);
    let refresh = move || {
        if busy.get_untracked() { return; }
        busy.set(true);
        manual.set(None);
        error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            match api::gateway_keys().await {
                Ok(rows) => { keys.set(rows); loaded.set(true); }
                Err(e) => { ctx.note_err(&e); error.set(Some(e.message)); }
            }
            busy.set(false);
        });
    };
    refresh();

    let create = move |_| {
        if busy.get_untracked() || !loaded.get_untracked() { return; }
        let value = name.get_untracked().trim().to_string();
        if value.is_empty() { return; }
        busy.set(true);
        error.set(None);
        manual.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            match api::create_gateway_key(&value).await {
                Ok(row) => {
                    keys.update(|rows| rows.push(row));
                    name.set(String::new());
                    ctx.say("密钥已生成，原有密钥不受影响；点该行「复制」即可使用");
                }
                Err(e) => { ctx.note_err(&e); error.set(Some(format!("生成失败：{}", e.message))); }
            }
            busy.set(false);
        });
    };

    view! {
        <section class="card gateway-keys">
            <header class="card-head">
                <h2>"对外 API 密钥"</h2>
                <p class="muted small">"客户端使用 Authorization: Bearer 密钥 接入网关。可分别为不同客户端生成命名密钥；新增不会更换或停用原有密钥。"</p>
            </header>
            <div class="card-body">
                <div class="toolbar">
                    <label for="gateway-key-name">"名称"</label>
                    <input id="gateway-key-name" placeholder="例如：手机、电脑" prop:value=name
                        disabled=move || busy.get()
                        on:input=move |ev| name.set(event_target_value(&ev)) />
                    <button class="primary" on:click=create
                        disabled=move || busy.get() || !loaded.get() || name.get().trim().is_empty()>"生成密钥"</button>
                    <button class="ghost" on:click=move |_| refresh() disabled=move || busy.get()>"刷新密钥"</button>
                </div>
                <Show when=move || busy.get()><p class="muted small" role="status">"正在处理…"</p></Show>
                {move || error.get().map(|message| view! { <div class="banner bad" role="alert">{message}</div> })}
                <Show when=move || loaded.get() && keys.get().is_empty()>
                    <p class="muted">"还没有对外密钥。填入名称后生成一把即可。"</p>
                </Show>
                <For each=move || keys.get() key=|row| row.id children=move |row| {
                    let id = row.id;
                    let delete_name = row.name.clone();
                    let copy = move |_| {
                        if busy.get_untracked() { return; }
                        busy.set(true);
                        error.set(None);
                        manual.set(None);
                        wasm_bindgen_futures::spawn_local(async move {
                            match api::reveal_gateway_key(id).await {
                                Ok(key) => {
                                    if copy_text(&key).await {
                                        ctx.say("密钥已复制到剪贴板");
                                    } else {
                                        manual.set(Some((id, key)));
                                        ctx.say("浏览器阻止了自动复制，请手动复制该行下方的密钥");
                                    }
                                }
                                Err(e) => { ctx.note_err(&e); error.set(Some(format!("复制失败：{}", e.message))); }
                            }
                            busy.set(false);
                        });
                    };
                    let delete = move |_| {
                        if busy.get_untracked() { return; }
                        let confirmed = web_sys::window().and_then(|w| w.confirm_with_message(
                            &format!("删除密钥「{delete_name}」？删除后立即失效，使用它的客户端将无法访问网关。其他密钥不受影响。此操作无法撤销。")
                        ).ok()).unwrap_or(false);
                        if !confirmed { return; }
                        busy.set(true);
                        error.set(None);
                        manual.set(None);
                        wasm_bindgen_futures::spawn_local(async move {
                            match api::delete_gateway_key(id).await {
                                Ok(()) => {
                                    keys.update(|rows| rows.retain(|row| row.id != id));
                                    ctx.say("密钥已删除，立即失效；其他密钥不受影响");
                                }
                                Err(e) => { ctx.note_err(&e); error.set(Some(format!("删除失败：{}", e.message))); }
                            }
                            busy.set(false);
                        });
                    };
                    view! {
                        <article class="panel" style="margin-top:12px;overflow-wrap:anywhere" data-key-id=id>
                            <div class="toolbar">
                                <strong>{row.name}</strong>
                                <code>{row.hint.unwrap_or_else(|| "（无提示）".into())}</code>
                                <span class="muted small">{format!("创建于 {}", fmt::ts(row.created_at_ms))}</span>
                                <span class="spacer"></span>
                                <button class="ghost" on:click=copy disabled=move || busy.get()>"复制"</button>
                                <button class="ghost danger" on:click=delete disabled=move || busy.get()>"删除"</button>
                            </div>
                            {move || manual.get().filter(|(key_id, _)| *key_id == id).map(|(_, key)| view! {
                                <div class="banner" style="margin-top:8px">
                                    <label>"自动复制失败，请手动复制（关闭后隐藏）"
                                        <input readonly prop:value=key style="width:100%;user-select:all" />
                                    </label>
                                    <button class="ghost" on:click=move |_| manual.set(None)>"关闭"</button>
                                </div>
                            })}
                        </article>
                    }
                } />
            </div>
        </section>
    }
}

#[component]
pub fn Settings(ctx: Ctx) -> impl IntoView {
    let logs_open=RwSignal::new(false);
    let info = RwSignal::new(crate::model::SettingsResp::default());
    let err = RwSignal::new(None::<String>);
    let old_pw = RwSignal::new(String::new());
    let new_pw = RwSignal::new(String::new());

    // 账目维护：只让用户填一个数（保留天数），其余三行是只读实话
    let maint = RwSignal::new(crate::model::MaintenanceResp::default());
    let days = RwSignal::new(30i64);
    let maint_msg = RwSignal::new(None::<String>);

    let reload_maint = move || {
        wasm_bindgen_futures::spawn_local(async move {
            match api::maintenance().await {
                Ok(m) => {
                    days.set(m.retention_days);
                    maint.set(m);
                }
                Err(e) => maint_msg.set(Some(e.message)),
            }
        });
    };

    let reload = move || {
        wasm_bindgen_futures::spawn_local(async move {
            match api::settings().await {
                Ok(s) => {
                    info.set(s);
                    err.set(None);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(Some(e.message));
                }
            }
        });
    };
    reload();
    reload_maint();

    let save_days = move |_| {
        let d = days.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            match api::set_retention_days(d).await {
                Ok(()) => {
                    maint_msg.set(Some(if d == 0 {
                        "已关闭自动清理：旧账不会再被删掉".to_string()
                    } else {
                        format!("以后只保留最近 {d} 天的请求记录")
                    }));
                    reload_maint();
                }
                Err(e) => maint_msg.set(Some(format!("保存失败：{}", e.message))),
            }
        });
    };

    let prune_now = move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::prune_now().await {
                Ok(r) => {
                    maint_msg.set(Some(r.notice));
                    reload_maint();
                }
                Err(e) => maint_msg.set(Some(format!("清理失败：{}", e.message))),
            }
        });
    };

    let change_pw = move |_| {
        let o = old_pw.get_untracked();
        let n = new_pw.get_untracked();
        if n.trim().is_empty() {
            ctx.say("新口令不能为空");
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match api::change_password(&o, &n).await {
                Ok(r) => {
                    // 改口令会踢掉所有会话，服务端已经补发了一个新的
                    api::set_token(&r.token);
                    old_pw.set(String::new());
                    new_pw.set(String::new());
                    ctx.say("口令已改，其他设备上的会话已失效");
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("改口令失败：{}", e.message));
                }
            }
        });
    };

    view! {
        <div class="page">
            <PageHead
                title="设置"
                desc="对外 key、面板口令、登录着的设备。"
            ><button class="ghost" on:click=move |_| reload()>"刷新"</button></PageHead>
            <div class="toolbar">
                <span class="muted small">
                    {move || format!("面板 v{}", tokengo_shared::VERSION)}
                </span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <GatewayKeys ctx=ctx />

            <section class="card">
                <header class="card-head">
            <h2>"接入信息"</h2>
            <p class="muted small">"把下面的地址和 key 填到客户端即可。"</p>
                </header>
                <div class="card-body">
            <div class="panel">
                <div class="form-grid">
                    <label>"基础 URL"</label>
                    <div><code>{move || {
                        let origin = web_sys::window().and_then(|w| w.location().origin().ok()).unwrap_or_default();
                        format!("{}/v1", origin)
                    }}</code></div>
                    <label>"对话"</label>
                    <div><code>"/v1/chat/completions"</code></div>
                    <label>"嵌入"</label>
                    <div><code>"/v1/embeddings"</code></div>
                    <label>"重排"</label>
                    <div><code>"/v1/rerank"</code></div>
                    <label>"模型列表"</label>
                    <div><code>"/v1/models"</code></div>
                </div>
            </div>
                </div>
            </section>

            <section class="card">
                <header class="card-head">
            <h2>"账目清理"</h2>
            <p class="muted small">
                "网关每接一次请求就记一行账。不清理的话账目只进不出，一年能长三四个 G，把容器磁盘占满。"
                "这里设一个保留天数，超期的老账每小时检查一次、自动清掉；填 " <b>"0"</b> " 就是暂不清理。"
            </p>
                </header>
                <div class="card-body">
            <div class="panel">
                <div class="form-grid">
                    <label>"保留天数"</label>
                    <div>
                        <input type="number" min="0" prop:value=move || days.get().to_string()
                            on:input=move |ev| {
                                if let Ok(v) = event_target_value(&ev).parse::<i64>() { days.set(v.max(0)); }
                            } />
                        <div class="muted small">"默认 30 天。填 0 = 停止自动清理。"</div>
                    </div>
                    <label>"现在有多少条"</label>
                    <div>{move || fmt::num(maint.get().total_requests)}</div>
                    <label>"最老一条"</label>
                    <div>{move || maint.get().oldest_ts_ms.map(|t| fmt::ts(t)).unwrap_or_else(|| "（还没记录）".into())}</div>
                    <label>"库有多大"</label>
                    <div>{move || format!("{:.1} MB", maint.get().db_bytes as f64 / 1_048_576.0)}</div>
                    <label>"上次清理"</label>
                    <div>{move || {
                        let m = maint.get();
                        match m.last_prune_at_ms {
                            Some(t) => format!("{}（清掉 {} 条）", fmt::ts(t), m.last_prune_count),
                            None => "还没跑过".into(),
                        }
                    }}</div>
                </div>
                {move || maint_msg.get().map(|m| view! {
                    <div class="banner ok" style="margin-top:8px">{m}</div>
                })}
                <div class="modal-foot">
                    <button class="primary" on:click=save_days>"保存天数"</button>
                    <button class="ghost" on:click=prune_now>"立即清理一次"</button>
                </div>
            </div>
                </div>
            </section>

            <section class="card">
                <header class="card-head">
            <h2>"面板口令"</h2>
                </header>
                <div class="card-body">
            <div class="panel">
                <div class="form-grid">
                    <label>"原口令"</label>
                    <input type="password" prop:value=old_pw
                        on:input=move |ev| old_pw.set(event_target_value(&ev)) />
                    <label>"新口令"</label>
                    <div>
                        <input type="password" prop:value=new_pw
                            on:input=move |ev| new_pw.set(event_target_value(&ev)) />
                        <div class="muted small">"改完会踢掉所有设备的登录，包括现在这台（会自动重新登录）。"</div>
                    </div>
                </div>
                <div class="modal-foot">
                    <button class="primary" on:click=change_pw>"改口令"</button>
                </div>
            </div>
                </div>
            </section>

        <details class="fold settings-logs" on:toggle=move |ev| {if let Some(target)=ev.target(){logs_open.set(js_sys::Reflect::get(&target,&"open".into()).ok().and_then(|v|v.as_bool()).unwrap_or(false));}}>
            <summary>"运行日志"</summary>
            <Show when=move ||logs_open.get()><crate::pages::logs::Logs ctx=ctx /></Show>
        </details>
        </div>
    }
}
