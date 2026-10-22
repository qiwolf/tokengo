use super::PageHead;
use crate::{Ctx, account_api as api, fmt};
use leptos::prelude::*;
#[component]
pub fn Portal(ctx: Ctx) -> impl IntoView {
    let data = RwSignal::new(None::<api::Portal>);
    let owner = RwSignal::new(None::<i64>);
    let models = RwSignal::new(Vec::<String>::new());
    let services=RwSignal::new(Vec::<serde_json::Value>::new());
    let channels = RwSignal::new(Vec::<(i64, String)>::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let load = move |before: Option<i64>| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::portal(before).await {
                Ok(v) => data.set(Some(v)),
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    load(None);
    view! {<div class="page portal-page"><PageHead title="我的空间" desc="管理自己的 API Key，查看用量和费用；全部 Key 共享你的总额度。"><button type="button" class="ghost" disabled=move||busy.get() on:click=move|_|load(None)>"刷新"</button></PageHead>
    <Show when=move||!error.get().is_empty()><div class="banner bad" role="alert">{move||error.get()}</div></Show>
    <Show when=move||busy.get()&&data.get().is_none()><div class="account-loading" role="status">"正在读取你的用量…"</div></Show>
    {move||data.get().map(|p|{let summary=p.usage.summary;let before=p.usage.next_before_id;
    view!{<div class="portal-summary"><section><span>"已入账请求"</span><strong>{fmt::num(summary.request_count)}</strong><small>{format!("{} 笔尝试账本",fmt::num(summary.ledger_count))}</small></section><section><span>"已知输入 / 输出 tokens"</span><strong>{format!("{} / {}",fmt::num(summary.input_tokens),fmt::num(summary.output_tokens))}</strong><small>{format!("另有 {} 笔输入、{} 笔输出未知",fmt::num(summary.unknown_input_entries),fmt::num(summary.unknown_output_entries))}</small></section><section><span>"已知记账金额"</span><strong>{api::debit(Some(summary.known_debit_micro))}</strong><small>{format!("未定价 {} 项；以上仅含已知金额",fmt::num(summary.unpriced_entries))}</small></section></div>
    <super::key_manager::KeyManager ctx=ctx owner=owner models=models channels=channels services=services personal=true/>
    <section class="card"><header class="card-head"><h2>"已授权调用模型"</h2></header><div class="card-body"><p class="account-help">"可使用以下名称发起请求。共享模型由管理员维护，此处只展示已授权入口，不展示或编辑内部调用目标；单个 Key 还可能有更小的授权范围。"</p><p role="status">{move||if models.get().is_empty(){"暂无可公开调用的已授权模型。"}else{""}}</p><div class="policy-options">{move||models.get().into_iter().map(|name|view!{<code>{name}</code>}).collect_view()}</div></div></section>
    <section class="card"><header class="card-head"><h2>"已授权透传服务"</h2></header><div class="card-body"><p class="account-help">"按服务前缀发起 HTTP 请求，认证方式仍为 API Key；单个 Key 还可能有更小的授权范围。"</p><p role="status">{move||if services.get().is_empty(){"暂无已授权且已启用的透传服务。"}else{""}}</p><div class="policy-options">{move||services.get().into_iter().map(|s|view!{<div><strong>{s["name"].as_str().unwrap_or("").to_owned()}</strong><div><code>{s["prefix"].as_str().unwrap_or("").to_owned()}</code></div></div>}).collect_view()}</div></div></section>
    <section class="card"><header class="card-head"><h2>"用量记录"</h2><span class="muted small">"最新在前 · 每页 25 条"</span></header><div class="card-body">
    {if p.usage.entries.is_empty(){view!{<p class="account-empty">"这页暂无调用记录。"</p>}.into_any()}else{view!{<div class="table-wrap"><table><thead><tr><th>"时间"</th><th>"模型"</th><th>"计费单位"</th><th>"输入 / 输出 tokens"</th><th>"结果"</th><th>"扣费"</th></tr></thead><tbody>{p.usage.entries.into_iter().map(|e|{let tokens=api::entry_tokens(&e);let debit=api::debit(e.debit_micro);let unit=if e.billing_unit=="request"{"按次"}else{"token"};let outcome=e.outcome.unwrap_or_else(||e.status.clone());view!{<tr><td data-label="时间">{fmt::ts_short(e.created_ms)}</td><td data-label="模型">{e.model}</td><td data-label="计费单位">{unit}</td><td data-label="输入 / 输出 tokens" class="num">{tokens}</td><td data-label="结果">{outcome}</td><td data-label="扣费" class="num">{debit}</td></tr>}}).collect_view()}</tbody></table></div>}.into_any()}}
    <div class="account-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move|_|load(None)>"回到最新"</button><button type="button" class="ghost" disabled=move||busy.get()||before.is_none() on:click=move|_|load(before)>"较早记录"</button></div></div></section>}
    })}</div>}
}
