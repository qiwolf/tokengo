//! 设置：对外 key、面板口令、在线会话、退出。
//!
//! 对外 key 是**客户端的凭证**，改了要同步改客户端 —— 保存按钮旁边把这句话写明，
//! 不然会有人手一滑把在跑的客户端全打 401。

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
pub fn Settings(ctx: Ctx) -> impl IntoView {
    let logs_open=RwSignal::new(false);
    let info = RwSignal::new(crate::model::SettingsResp::default());
    let err = RwSignal::new(None::<String>);
    let generated = RwSignal::new(None::<String>);
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

    let generate_key = move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::generate_gateway_key().await {
                Ok(k) => {
                    generated.set(Some(k.clone()));
                    ctx.say(format!("已生成新 key：{}（已填入下方，可复制）", k));
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("生成失败：{}", e.message));
                }
            }
        });
    };

    // 复制：**任何时候都能用** —— 有刚生成的就直接复制，没有就向服务端取明文
    // （明文用主密钥加密存在库里，面板只给掩码）。用户的要求就是「生成一次，以后随时复制」。
    let copy_key = move |_| {
        let already = generated.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            let key = match already {
                Some(k) => Some(k),
                None => match api::reveal_gateway_key().await {
                    Ok(k) => Some(k),
                    Err(e) => {
                        ctx.note_err(&e);
                        ctx.say(e.message.clone());
                        None
                    }
                },
            };
            if let Some(k) = key {
                if copy_text(&k).await {
                    ctx.say("对外 key 已复制到剪贴板");
                } else {
                    generated.set(Some(k));
                    ctx.say("浏览器阻止了自动复制，请长按下方 key 手动复制");
                }
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
                    {move || format!("面板 v{} · 在线会话 {} 个", tokengo_shared::VERSION, info.get().active_sessions)}
                </span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <section class="card">
                <header class="card-head">
            <h2>"对外 key"</h2>
            <p class="muted small">
                "客户端连网关用的那个 key（" <code>"Authorization: Bearer …"</code>
                "）。**生成一次就行**：明文用主密钥加密存在库里，下面点「复制」随时能取（不用反复生成）。"
                "换 key 会让旧 key 立刻失效，记得同步改客户端。"
            </p>
                </header>
                <div class="card-body">
            <div class="panel">
                <div class="form-grid">
                    <label>"当前 key 提示"</label>
                    <div>
                        <code>{move || info.get().gateway_key_hint.unwrap_or_else(|| "（没设置）".into())}</code>
                    </div>
                </div>
                {move || generated.get().map(|k| view! {
                    <div class="banner ok" style="margin-top:8px;word-break:break-all">
                        "可复制的 key：" <code style="user-select:all">{k}</code>
                        "　（已存下来，以后随时点「复制」都能取，不用再生成）"
                    </div>
                })}
                <div class="modal-foot">
                    <span class="muted small">
                        "当前 key："
                        <code>{move || info.get().gateway_key_hint.clone().unwrap_or_else(|| "（还没设）".into())}</code>
                    </span>
                    <span class="spacer"></span>
                    <button class="ghost" on:click=generate_key>"生成新 key"</button>
                    <button class="primary" on:click=copy_key>"复制"</button>
                </div>
                {move || if info.get().gateway_key_hint.is_some() && !info.get().gateway_key_copyable {
                    view! {
                        <div class="muted small" style="margin-top:6px">
                            "这把 key 是旧版生成的（库里只有哈希、取不回明文）：点一次「生成新 key」换一把，"
                            "之后就随时能复制了（换完记得把客户端里的 key 也更新一下）。"
                        </div>
                    }
                        .into_any()
                } else {
                    ().into_any()
                }}
            </div>
                </div>
            </section>

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
