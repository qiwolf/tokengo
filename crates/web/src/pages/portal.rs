use super::PageHead;
use crate::{Ctx, account_api as api, fmt};
use leptos::prelude::*;
#[component]
pub fn Portal(ctx: Ctx) -> impl IntoView {
    let data = RwSignal::new(None::<api::Portal>);
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
    view! {<div class="page portal-page"><PageHead title="我的用量" desc="仅显示你关联的 API 用户的密钥标识和用量。"><button class="ghost" disabled=move||busy.get() on:click=move|_|load(None)>"刷新"</button></PageHead>
    <Show when=move||!error.get().is_empty()><div class="banner bad" role="alert">{move||error.get()}</div></Show>
    <Show when=move||busy.get()&&data.get().is_none()><div class="account-loading" role="status">"正在读取你的用量…"</div></Show>
    {move||data.get().map(|p|{let summary=p.usage.summary;let before=p.usage.next_before_id;
    view!{<div class="portal-summary"><section><span>"已入账请求"</span><strong>{fmt::num(summary.request_count)}</strong><small>{format!("{} 笔尝试账本",fmt::num(summary.ledger_count))}</small></section><section><span>"已知输入 / 输出 tokens"</span><strong>{format!("{} / {}",fmt::num(summary.input_tokens),fmt::num(summary.output_tokens))}</strong><small>{format!("另有 {} 笔输入、{} 笔输出未知",fmt::num(summary.unknown_input_entries),fmt::num(summary.unknown_output_entries))}</small></section><section><span>"已知记账金额"</span><strong>{api::debit(Some(summary.known_debit_micro))}</strong><small>{format!("未定价 {} 项；以上仅含已知金额",fmt::num(summary.unpriced_entries))}</small></section></div>
    <section class="card"><header class="card-head"><h2>"我的 API 密钥"</h2></header><div class="card-body"><p class="muted small">"这里只显示标识与状态，不显示密钥。新增密钥或调整权限请联系管理员。"</p>
    {if p.keys.is_empty(){view!{<p class="account-empty">"尚未分配 API 密钥，请联系管理员。"</p>}.into_any()}else{view!{<div class="table-wrap"><table><thead><tr><th>"名称"</th><th>"密钥标识"</th><th>"状态"</th><th>"到期时间"</th></tr></thead><tbody>{p.keys.into_iter().map(|k|{let state=if k.revoked_at.is_some(){"已撤销"}else if !k.enabled{"已停用"}else if k.expires_at.is_some_and(|t|t<=crate::now_ms()){"已到期"}else{"可用"};view!{<tr><td data-label="名称">{k.name}</td><td data-label="密钥标识"><code>{k.hint}</code></td><td data-label="状态">{state}</td><td data-label="到期时间">{k.expires_at.map(fmt::ts_short).unwrap_or_else(||"未设到期".into())}</td></tr>}}).collect_view()}</tbody></table></div>}.into_any()}}
    </div></section><section class="card"><header class="card-head"><h2>"用量记录"</h2><span class="muted small">"最新在前 · 每页 25 条"</span></header><div class="card-body">
    {if p.usage.entries.is_empty(){view!{<p class="account-empty">"这页暂无调用记录。"</p>}.into_any()}else{view!{<div class="table-wrap"><table><thead><tr><th>"时间"</th><th>"模型"</th><th>"计费单位"</th><th>"输入 / 输出 tokens"</th><th>"结果"</th><th>"扣费"</th></tr></thead><tbody>{p.usage.entries.into_iter().map(|e|{let tokens=api::entry_tokens(&e);let debit=api::debit(e.debit_micro);let unit=if e.billing_unit=="request"{"按次"}else{"token"};let outcome=e.outcome.unwrap_or_else(||e.status.clone());view!{<tr><td data-label="时间">{fmt::ts_short(e.created_ms)}</td><td data-label="模型">{e.model}</td><td data-label="计费单位">{unit}</td><td data-label="输入 / 输出 tokens" class="num">{tokens}</td><td data-label="结果">{outcome}</td><td data-label="扣费" class="num">{debit}</td></tr>}}).collect_view()}</tbody></table></div>}.into_any()}}
    <div class="account-actions"><button class="ghost" disabled=move||busy.get() on:click=move|_|load(None)>"回到最新"</button><button class="ghost" disabled=move||busy.get()||before.is_none() on:click=move|_|load(before)>"较早记录"</button></div></div></section>}
    })}</div>}
}
