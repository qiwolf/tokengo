//! 设置：对外密钥、面板口令与账目维护。

use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

pub(crate) async fn copy_text(text: &str) -> bool {
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
    // Keep the selection inside an open modal's focus trap when copying from account forms.
    let selector = "dialog[open], [role='dialog'][aria-modal='true']";
    // Copy buttons may live in a second modal inside the user's settings dialog.
    // The outer dialog is inert then: selecting there can report success without copying.
    let host = document.active_element()
        .and_then(|element|element.closest(selector).ok().flatten())
        .or_else(||document.query_selector("dialog[open] dialog[open]").ok().flatten())
        .or_else(||document.query_selector(selector).ok().flatten());
    let appended = if let Some(host)=host {host.append_child(&area)}else{body.append_child(&area)};
    if appended.is_err() { return false; }
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
    let creating = RwSignal::new(false);
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
                    creating.set(false);
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
                <div class="toolbar"><button type="button" class="primary" disabled=move||busy.get()||!loaded.get() on:click=move |_|creating.set(true)>"新建密钥"</button><button type="button" class="ghost" on:click=move |_| refresh() disabled=move || busy.get()>"刷新密钥"</button></div>
                <super::ModuleDialog open=creating busy=busy title="新建网关密钥".to_string()><div class="account-form">
                    <label for="gateway-key-name">"名称"</label>
                    <input id="gateway-key-name" placeholder="例如：手机、电脑" prop:value=name
                        disabled=move || busy.get()
                        on:input=move |ev| name.set(event_target_value(&ev)) />
                    <button type="button" class="primary" on:click=create
                        disabled=move || busy.get() || !loaded.get() || name.get().trim().is_empty()>"生成密钥"</button>
                    <Show when=move||error.get().is_some()>{move||error.get().map(|e|view!{<p class="banner bad" role="alert">{e}</p>})}</Show>
                </div></super::ModuleDialog>
                <Show when=move || busy.get()><p class="muted small" role="status">"正在处理…"</p></Show>
                {move || error.get().map(|message| view! { <div class="banner bad" role="alert">{message}</div> })}
                <Show when=move || loaded.get() && keys.get().is_empty()>
                    <p class="muted">"尚未创建网关密钥。点击“新建密钥”并填写名称即可生成。"</p>
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
                        <article class="panel gateway-key-row" style="margin-top:12px;overflow-wrap:anywhere" data-key-id=id>
                            <div class="gateway-key-record">
                                <div class="gateway-key-identity">
                                    <strong class="gateway-key-name" title=row.name.clone()>{row.name.clone()}</strong>
                                    <code>{row.hint.unwrap_or_else(|| "（无提示）".into())}</code>
                                    <span class="muted small gateway-key-created">{format!("创建于 {}", fmt::ts(row.created_at_ms))}</span>
                                </div>
                                <div class="gateway-key-actions">
                                    <button type="button" class="ghost" on:click=copy disabled=move || busy.get()>"复制"</button>
                                    <button type="button" class="ghost danger" on:click=delete disabled=move || busy.get()>"删除"</button>
                                </div>
                            </div>
                            {move || manual.get().filter(|(key_id, _)| *key_id == id).map(|(_, key)| view! {
                                <div class="banner" style="margin-top:8px">
                                    <label>"自动复制失败，请手动复制（关闭后隐藏）"
                                        <input readonly prop:value=key style="width:100%;user-select:all" />
                                    </label>
                                    <button type="button" class="ghost" on:click=move |_| manual.set(None)>"关闭"</button>
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
fn RequestTimeouts(ctx: Ctx) -> impl IntoView {
    let editing = RwSignal::new(false);
    let stream = RwSignal::new(30u64);
    let complete = RwSignal::new(120u64);
    let budget = RwSignal::new(180u64);
    let saved = RwSignal::new((30u64,120u64,180u64));
    let error = RwSignal::new(None::<String>);
    let loaded = RwSignal::new(false);
    let busy = RwSignal::new(false);
    wasm_bindgen_futures::spawn_local(async move {
        match api::routing().await {
            Ok(r) => {
                stream.set(r.config.attempt_timeout_ms / 1000);
                complete.set(r.config.nonstream_timeout_ms / 1000);
                budget.set(r.config.request_budget_ms / 1000);
                saved.set((r.config.attempt_timeout_ms / 1000,r.config.nonstream_timeout_ms / 1000,r.config.request_budget_ms / 1000));
                loaded.set(true);
            }
            Err(e) => {ctx.note_err(&e);error.set(Some(format!("读取等待时间失败：{}",e.message)));},
        }
    });
    let save = move |_| {
        if busy.get_untracked() || !loaded.get_untracked() { return; }
        let values = [stream.get_untracked(), complete.get_untracked(), budget.get_untracked()];
        if values.iter().any(|v| *v == 0 || *v > 3600) {
            error.set(Some("等待时间须为 1～3600 秒".into())); return;
        }
        busy.set(true);
        error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            // Re-read to preserve unrelated retry and probe settings.
            let result = async {
                let mut cfg = api::routing().await?.config;
                cfg.attempt_timeout_ms = values[0] * 1000;
                cfg.nonstream_timeout_ms = values[1] * 1000;
                cfg.request_budget_ms = values[2] * 1000;
                api::save_routing_config(&cfg).await
            }.await;
            match result {
                Ok(()) => {saved.set((values[0],values[1],values[2]));editing.set(false);ctx.say("等待时间已保存");},
                Err(e) => { ctx.note_err(&e); error.set(Some(format!("保存失败：{}", e.message))); }
            }
            busy.set(false);
        });
    };
    view! {
        <section class="card request-timeouts">
            <header class="card-head"><h2>"请求等待时间"</h2><button type="button" class="ghost" disabled=move||!loaded.get() on:click=move |_|{let (s,c,b)=saved.get_untracked();stream.set(s);complete.set(c);budget.set(b);error.set(None);editing.set(true);}>"编辑等待时间"</button></header><div class="card-body"><p class="muted small">{move||format!("流式响应 {} 秒 · 非流式完成 {} 秒 · 总预算 {} 秒",saved.get().0,saved.get().1,saved.get().2)}</p><Show when=move||!editing.get()>{move||error.get().map(|message|view!{<p class="banner bad" role="alert">{message}</p>})}</Show></div>
            <super::ModuleDialog open=editing busy=busy title="请求等待时间".to_string()><div class="card-body"><div class="form-grid">
                <label>"连接时限"</label><span>"10 秒"</span>
                <label for="stream-timeout">"流式响应 / 空闲（秒）"</label>
                <input id="stream-timeout" type="number" min="1" max="3600" disabled=move || !loaded.get() || busy.get()
                    prop:value=move || stream.get().to_string() on:input=move |ev| stream.set(event_target_value(&ev).parse().unwrap_or(0)) />
                <label for="nonstream-timeout">"非流式完成（秒）"</label>
                <input id="nonstream-timeout" type="number" min="1" max="3600" disabled=move || !loaded.get() || busy.get()
                    prop:value=move || complete.get().to_string() on:input=move |ev| complete.set(event_target_value(&ev).parse().unwrap_or(0)) />
                <label for="request-budget">"调用与切换总预算（秒）"</label>
                <input id="request-budget" type="number" min="1" max="3600" disabled=move || !loaded.get() || busy.get()
                    prop:value=move || budget.get().to_string() on:input=move |ev| budget.set(event_target_value(&ev).parse().unwrap_or(0)) />
            </div>{move||error.get().map(|message|view!{<p class="banner bad" role="alert">{message}</p>})}<button type="button" disabled=move || !loaded.get() || busy.get() class="primary" on:click=save>"保存等待时间"</button></div></super::ModuleDialog>
        </section>
    }
}

#[component]
pub fn Settings(ctx: Ctx) -> impl IntoView {
    let section = RwSignal::new(0usize);
    let logs_open=RwSignal::new(false);
    let maintenance_open=RwSignal::new(false);
    let info = RwSignal::new(crate::model::SettingsResp::default());
    let err = RwSignal::new(None::<String>);

    // 账目维护：只让用户填一个数（保留天数），其余三行是只读实话
    let maint = RwSignal::new(crate::model::MaintenanceResp::default());
    let days = RwSignal::new(30i64);
    let maint_msg = RwSignal::new(None::<String>);
    let maint_busy = RwSignal::new(false);
    let maint_loaded = RwSignal::new(false);
    let maint_error = RwSignal::new(false);

    let reload_maint = move || {
        wasm_bindgen_futures::spawn_local(async move {
            match api::maintenance().await {
                Ok(m) => {
                    days.set(m.retention_days);
                    maint.set(m);
                    maint_loaded.set(true);
                }
                Err(e) => {ctx.note_err(&e);maint_error.set(true);maint_msg.set(Some(format!("读取失败：{}",e.message)));},
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
        if maint_busy.get_untracked()||!maint_loaded.get_untracked(){return;}
        maint_busy.set(true);maint_error.set(false);maint_msg.set(None);
        let d = days.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            match api::set_retention_days(d).await {
                Ok(()) => {
                    maint_msg.set(Some(if d == 0 {
                        "已关闭自动清理：旧账不会再被删掉".to_string()
                    } else {
                        format!("以后只保留最近 {d} 天的请求记录")
                    }));
                    maintenance_open.set(false);
                    reload_maint();
                }
                Err(e) => {ctx.note_err(&e);maint_error.set(true);maint_msg.set(Some(format!("保存失败：{}", e.message)));},
            }
            maint_busy.set(false);
        });
    };

    let prune_now = move |_| {
        if maint_busy.get_untracked()||!maint_loaded.get_untracked(){return;}
        let saved_days=maint.get_untracked().retention_days;
        if !web_sys::window().and_then(|w|w.confirm_with_message(&format!("按当前已保存策略清理账目：保留 {saved_days} 天，0 表示不清理。此操作不可撤销；未保存的天数不生效。继续？")).ok()).unwrap_or(false){return;}
        maint_busy.set(true);maint_error.set(false);maint_msg.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            match api::prune_now().await {
                Ok(r) => {
                    maint_msg.set(Some(r.notice));
                    reload_maint();
                }
                Err(e) => {ctx.note_err(&e);maint_error.set(true);maint_msg.set(Some(format!("清理失败：{}", e.message)));},
            }
            maint_busy.set(false);
        });
    };

    view! {
        <div class="page">
            <PageHead
                title="系统设置"
                desc="网关密钥、运行参数与维护。"
            ><button type="button" class="ghost" on:click=move |_| reload()>"刷新"</button></PageHead>
            <div class="toolbar">
                <span class="muted small">
                    {move || format!("面板 v{}", tokengo_shared::VERSION)}
                </span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <div class="settings-workspace">
                <nav class="settings-sections" aria-label="设置分组">
                    <button type="button" class="settings-section-link" aria-pressed=move||if section.get()==0 {"true"} else {"false"} aria-controls="settings-group-0" on:click=move |_|section.set(0)><strong>"接入与密钥"</strong><span>"客户端地址与网关密钥"</span></button>
                    <button type="button" class="settings-section-link" aria-pressed=move||if section.get()==1 {"true"} else {"false"} aria-controls="settings-group-1" on:click=move |_|section.set(1)><strong>"请求参数"</strong><span>"响应等待与请求预算"</span></button>
                    <button type="button" class="settings-section-link" aria-pressed=move||if section.get()==2 {"true"} else {"false"} aria-controls="settings-group-2" on:click=move |_|section.set(2)><strong>"运维凭据"</strong><span>"自动化工具的独立凭据"</span></button>
                    <button type="button" class="settings-section-link" aria-pressed=move||if section.get()==3 {"true"} else {"false"} aria-controls="settings-group-3" on:click=move |_|section.set(3)><strong>"账号安全"</strong><span>"登录账号与密码"</span></button>
                    <button type="button" class="settings-section-link" aria-pressed=move||if section.get()==4 {"true"} else {"false"} aria-controls="settings-group-4" on:click=move |_|section.set(4)><strong>"维护与日志"</strong><span>"账目保留与运行诊断"</span></button>
                </nav>
                <div class="settings-panels">
                    <div id="settings-group-0" class="settings-group" role="region" aria-label="接入与密钥" hidden=move||section.get()!=0>
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


                    </div>
                    <div id="settings-group-1" class="settings-group" role="region" aria-label="请求参数" hidden=move||section.get()!=1>
<RequestTimeouts ctx=ctx />
                    </div>
                    <div id="settings-group-2" class="settings-group" role="region" aria-label="运维凭据" hidden=move||section.get()!=2>
<super::operations_tokens::OperationsTokens ctx=ctx />
                    </div>
                    <div id="settings-group-3" class="settings-group" role="region" aria-label="账号安全" hidden=move||section.get()!=3>
            <section class="card"><header class="card-head"><h2>"账号安全"</h2></header><div class="card-body"><p class="muted">"登录账号与 API 密钥独立。管理员可修改自己的登录账号和密码。"</p><button type="button" class="ghost" on:click=move |_|ctx.go(crate::Page::Account)>"管理登录账号与密码"</button></div></section>


                    </div>
                    <div id="settings-group-4" class="settings-group" role="region" aria-label="维护与日志" hidden=move||section.get()!=4>
            <section class="card">
                <header class="card-head">
            <h2>"账目清理"</h2><button type="button" class="ghost" disabled=move||!maint_loaded.get()||maint_busy.get() on:click=move |_|{days.set(maint.get_untracked().retention_days);maintenance_open.set(true);}>"管理账目保留"</button>
            <p class="muted small">{move||if !maint_loaded.get(){"正在读取保留策略…".to_string()}else if maint.get().retention_days==0 {"已关闭自动清理。".to_string()}else{format!("自动保留最近 {} 天的请求账目。",maint.get().retention_days)}}</p>
                </header>
                <Show when=move||!maintenance_open.get()>{move||maint_msg.get().map(|m|view!{<p class=move||if maint_error.get(){"banner bad"}else{"banner ok"} role="status">{m}</p>})}</Show><super::ModuleDialog open=maintenance_open busy=maint_busy title="账目保留与清理".to_string()><p class="muted small">"超过保留期限的账目会自动清理；设为 0 则停止自动清理。调整保留天数前，请确认历史记录仍满足你的查询需求。"</p>
                <div class="card-body">
            <div class="panel">
                <div class="form-grid">
                    <label>"保留天数"</label>
                    <div>
                        <input type="number" min="0" disabled=move||maint_busy.get() prop:value=move || days.get().to_string()
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
                    <div class=move||if maint_error.get(){"banner bad"}else{"banner ok"} role="status" style="margin-top:8px">{m}</div>
                })}
                <div class="modal-foot">
                    <button type="button" class="primary" disabled=move||maint_busy.get()||!maint_loaded.get() on:click=save_days>"保存天数"</button>
                    <button type="button" class="ghost danger" disabled=move||maint_busy.get()||!maint_loaded.get() on:click=prune_now>"立即清理过期账目"</button>
                </div>
            </div>
                </div>
            </super::ModuleDialog></section>

        <section class="card"><header class="card-head"><h2>"运行日志"</h2><button type="button" class="ghost" on:click=move |_|logs_open.set(true)>"查看运行日志"</button></header><p class="card-body muted small">"按级别筛选运行记录，查看诊断信息。"</p></section>

                    </div>
                </div>
            </div>
        <super::ModuleDialog open=logs_open title="运行日志".to_string()><Show when=move ||logs_open.get()><crate::pages::logs::Logs ctx=ctx /></Show></super::ModuleDialog>
        </div>
    }
}
