use crate::{admin_api as api, Ctx};
use leptos::prelude::*;
use serde_json::json;

/// Explicit JSON overrides preserve the server's inherit-vs-override distinction.
#[component]
pub fn RoutePolicy(ctx: Ctx) -> impl IntoView {
    let model = RwSignal::new(String::new());
    let loaded = RwSignal::new(None::<String>);
    let body = RwSignal::new("{}".to_string());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let load = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let m = model.get_untracked().trim().to_string();
        if m.is_empty() {
            error.set("请填写已配置的逻辑模型名（不含 auto）".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        message.set(String::new());
        loaded.set(None);
        leptos::task::spawn_local(async move {
            match api::request(
                "GET",
                &format!("/api/model-routing?model={}", api::encode(&m)),
                None,
            )
            .await
            {
                Ok(v) => {
                    body.set(api::pretty(&v["policy"]));
                    loaded.set(Some(m));
                    message.set("策略已读取；空对象表示继承全局配置。".into());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    let apply = move |delete: bool| {
        if busy.get_untracked() {
            return;
        }
        let Some(m) = loaded.get_untracked() else {
            return;
        };
        let v = if delete {
            json!({})
        } else {
            match api::object(&body.get_untracked()) {
                Ok(v) => v,
                Err(e) => {
                    error.set(e);
                    return;
                }
            }
        };
        busy.set(true);
        error.set(String::new());
        message.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request(
                if delete { "DELETE" } else { "PUT" },
                &format!("/api/model-routing?model={}", api::encode(&m)),
                if delete { None } else { Some(v) },
            )
            .await
            {
                Ok(v) => {
                    body.set(if delete {
                        "{}".into()
                    } else {
                        api::pretty(&v["policy"])
                    });
                    message.set(
                        if delete {
                            "已恢复继承全局配置"
                        } else {
                            "模型策略已保存，运行路由已重置"
                        }
                        .into(),
                    );
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {<section class="card admin-section"><header class="card-head"><h2>"单模型路由策略"</h2></header><div class="card-body">
    <p class="muted small">"只作用于该逻辑模型的已声明渠道。channel_ids 是有序白名单，并非额外添加渠道；未列出的渠道不参与该模型。省略 / null 字段继承全局配置。保存会重置该模型当前运行路由。"</p>
    <form on:submit=load><fieldset disabled=move||busy.get()><div class="form-grid"><label for="routing-model">"逻辑模型名"</label><input id="routing-model" required type="text" aria-describedby="routing-error" prop:value=model on:input=move|e|{model.set(event_target_value(&e));loaded.set(None);message.set(String::new());}/></div><button class="ghost" type="submit">"读取策略"</button></fieldset></form>
    <label for="routing-json">{move||loaded.get().map(|m|format!("{m} 的策略 JSON")).unwrap_or_else(||"先读取模型，再编辑策略".into())}</label>
    <textarea id="routing-json" rows="9" disabled=move||busy.get()||loaded.get().is_none() aria-describedby="routing-help routing-error" prop:value=body on:input=move|e|body.set(event_target_value(&e))/>
    <p id="routing-help" class="muted small">"支持 channel_ids（如 [2,1]，每条需已声明本模型）、retry_times（0..2）、reprobe_interval_ms（0..86400000）、probe_enabled、include_in_auto（布尔）。空对象 {} 继承全局。未知字段由服务端拒绝。"</p>
    <p id="routing-error" role="alert">{move||error.get()}</p><p role="status">{move||if busy.get(){"正在请求模型策略…".into()}else{message.get()}}</p>
    <button class="primary" disabled=move||busy.get()||loaded.get().is_none() on:click=move |_|apply(false)>"保存策略"</button>
    <button class="ghost" disabled=move||busy.get()||loaded.get().is_none() on:click=move |_|apply(true)>"恢复继承全局"</button>
    </div></section>}
}
