//! 设置：对外 key、面板口令、在线会话、退出。
//!
//! 对外 key 是**客户端的凭证**，改了要同步改客户端 —— 保存按钮旁边把这句话写明，
//! 不然会有人手一滑把在跑的客户端全打 401。

use leptos::prelude::*;

use crate::api;
use crate::pages::PageHead;
use crate::Ctx;

#[component]
pub fn Settings(ctx: Ctx) -> impl IntoView {
    let info = RwSignal::new(crate::model::SettingsResp::default());
    let err = RwSignal::new(None::<String>);
    let new_key = RwSignal::new(String::new());
    let generated = RwSignal::new(None::<String>);
    let old_pw = RwSignal::new(String::new());
    let new_pw = RwSignal::new(String::new());

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

    let save_key = move |_| {
        let k = new_key.get_untracked();
        if k.trim().is_empty() {
            ctx.say("对外 key 不能为空");
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match api::set_gateway_key(k.trim()).await {
                Ok(()) => {
                    new_key.set(String::new());
                    ctx.say("对外 key 已更换 —— 客户端要同步改成新的，否则会一直 401");
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("换 key 失败：{}", e.message));
                }
            }
        });
    };

    let generate_key = move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::generate_gateway_key().await {
                Ok(k) => {
                    new_key.set(k.clone());
                    generated.set(Some(k.clone()));
                    ctx.say(format!("已生成新 key：{}（已填入下方输入框）", k));
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("生成失败：{}", e.message));
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
                    <label>"换成新 key"</label>
                    <div>
                        <input type="text" prop:value=new_key placeholder="新 key（明文，只提交不保存）"
                            on:input=move |ev| new_key.set(event_target_value(&ev)) />
                        <div class="muted small">
                            "点了保存就立刻生效：正在用旧 key 的客户端会马上 401，记得一起改。"
                        </div>
                    </div>
                </div>
                <div class="modal-foot">
                    <button class="ghost" on:click=generate_key>"生成新 key"</button>
                    <button class="primary" on:click=save_key>"更换对外 key"</button>
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
