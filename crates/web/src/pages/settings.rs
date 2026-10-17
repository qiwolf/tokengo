//! 设置：对外 key、面板口令、在线会话、退出。
//!
//! 对外 key 是**客户端的凭证**，改了要同步改客户端 —— 保存按钮旁边把这句话写明，
//! 不然会有人手一滑把在跑的客户端全打 401。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::pages::PageHead;
use crate::Ctx;

#[component]
pub fn Settings(ctx: Ctx) -> impl IntoView {
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

    let copy_key = move |_| {
        if let Some(k) = generated.get_untracked() {
            if let Some(window) = web_sys::window() {
                let _ = window.navigator().clipboard().write_text(&k);
                ctx.say("已复制到剪贴板");
            }
        } else {
            ctx.say("先点「生成新 key」");
        }
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
            />
            <div class="toolbar">
                <button class="ghost" on:click=move |_| reload()>"刷新"</button>
                <span class="muted small">
                    {move || format!("面板 v{} · 在线会话 {} 个", tokengo_shared::VERSION, info.get().active_sessions)}
                </span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <h2>"对外 key"</h2>
            <p class="muted small">
                "客户端连网关用的那个 key（" <code>"Authorization: Bearer …"</code>
                "）。库里只存哈希，面板只能显示提示片段，看不到明文。"
            </p>
            <div class="panel">
                <div class="form-grid">
                    <label>"当前 key 提示"</label>
                    <div>
                        <code>{move || info.get().gateway_key_hint.unwrap_or_else(|| "（没设置）".into())}</code>
                    </div>
                </div>
                {move || generated.get().map(|k| view! {
                    <div class="banner ok" style="margin-top:8px;word-break:break-all">
                        "新 key（只显示这一次）：" <code>{k}</code>
                    </div>
                })}
                <div class="modal-foot">
                    <button class="ghost" on:click=generate_key>"生成新 key"</button>
                    <button class="ghost" on:click=copy_key>"复制"</button>
                </div>
            </div>

            <h2>"接入信息"</h2>
            <p class="muted small">"把下面的地址和 key 填到客户端即可。"</p>
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

            <h2>"账目清理"</h2>
            <p class="muted small">
                "网关每接一次请求就记一行账。不清理的话账目只进不出，一年能长三四个 G，把容器磁盘占满。"
                "这里设一个保留天数，超期的老账每小时检查一次、自动清掉；填 " <b>"0"</b> " 就是暂不清理。"
            </p>
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

            <h2>"面板口令"</h2>
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

            <h2>"这个面板怎么更新的"</h2>
            <div class="panel">
                <p class="muted small">
                    "面板是 Rust 编出来的 WASM，和后端同一个仓库。改了界面要重新构建："
                </p>
                <pre class="code-block">"cd crates/web && trunk build --release"</pre>
                <p class="muted small">
                    "服务端从磁盘目录服务产物（" <code>"TOKENGO_WEB_DIR"</code>
                    " 可指定），所以换面板**不用重启网关**，浏览器强刷一下就是新的。"
                </p>
            </div>
        </div>
    }
}
