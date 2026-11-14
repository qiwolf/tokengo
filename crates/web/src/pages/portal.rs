use super::{PageHead, usage_period::{UsagePeriod, default_period_query, period_query, applied_period as period_label}};
use super::usage_dashboard::UsageDashboard;
use crate::{Ctx, account_api as api, fmt};
use leptos::prelude::*;
#[component]
pub fn Portal(ctx: Ctx) -> impl IntoView {
    let data = RwSignal::new(None::<api::Portal>);
    let summary=RwSignal::new(serde_json::Value::Null);let analytics=RwSignal::new(serde_json::Value::Null);
    let page_limit=RwSignal::new(25u32);

    let owner = RwSignal::new(None::<i64>);
    let models = RwSignal::new(Vec::<String>::new());
    let services=RwSignal::new(Vec::<serde_json::Value>::new());
    let channels = RwSignal::new(Vec::<(i64, String)>::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    let period = RwSignal::new("today".to_string());let start_time=RwSignal::new(String::new());let end_time=RwSignal::new(String::new());
    let start = RwSignal::new(String::new());
    let end = RwSignal::new(String::new());
    let active_query = RwSignal::new(format!("{}&currency=all",default_period_query()));
    let applied_period = RwSignal::new(period_label(&active_query.get_untracked()));
    let load = move |before: Option<i64>, query: String, limit: u32| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::portal(before, &query, limit).await {
                Ok(v) => {
                    summary.set(serde_json::to_value(&v.usage.summary).unwrap_or_default());
                    analytics.set(v.usage.analytics.clone());
                    page_limit.set(limit);
                    data.set(Some(v));
                    applied_period.set(period_label(&query));
                    active_query.set(query);
                },
                Err(e) => {
                    page_limit.set(page_limit.get_untracked());
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    load(None, active_query.get_untracked(),page_limit.get_untracked());
    let search = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() { return; }
        let selected = period.get_untracked();
        match period_query(&selected, &start.get_untracked(), &end.get_untracked(), &start_time.get_untracked(), &end_time.get_untracked()) {
            Ok(query) => load(None, format!("{query}&currency=all"),page_limit.get_untracked()),
            Err(e) => error.set(e),
        }
    };
    view! {<div class="page portal-page"><PageHead title="我的空间" desc="管理自己的 API Key，查看用量和费用。本地预算与历史冻结标记不再限制调用。"><button type="button" class="ghost" disabled=move||busy.get() on:click=move|_|load(None, active_query.get_untracked(),page_limit.get_untracked())>"刷新"</button></PageHead>
    <section class="card portal-usage-panel"><header class="card-head"><h2>"用量看板"</h2></header><div class="card-body">
    <form class="usage-filter-bar" on:submit=search><fieldset disabled=move||busy.get()><div class="usage-filter-row"><UsagePeriod period=period start=start end=end start_time=start_time end_time=end_time compact=true/><div class="usage-inline-actions"><button class="primary" type="submit">{move||if busy.get(){"查询中…"}else{"查询"}}</button><button type="button" class="ghost" on:click=move |_|{period.set("today".into());start.set(String::new());end.set(String::new());start_time.set(String::new());end_time.set(String::new());load(None,format!("{}&currency=all",default_period_query()),25);}>"重置"</button></div></div></fieldset></form>

    <p class="billing-query-status" role="status" aria-live="polite">{move||format!("当前结果：{}{}",applied_period.get(),if busy.get(){" · 查询中…"}else{""})}</p>
    <UsageDashboard summary=summary analytics=analytics/>
    </div></section>
    <Show when=move||!error.get().is_empty()><div class="banner bad" role="alert">{move||error.get()}</div></Show>
    <Show when=move||busy.get()&&data.get().is_none()><div class="account-loading" role="status">"正在读取你的用量…"</div></Show>
    {move||data.get().map(|p|{let before=p.usage.next_before_id;
    view!{
    <section class="card"><header class="card-head usage-ledger-heading"><div><h2>"调用明细"</h2><span class="muted small">"最新在前"</span></div><div class="usage-page-size"><label for="portal-usage-limit">"每页"</label><select id="portal-usage-limit" disabled=move||busy.get() prop:value=move||page_limit.get().to_string() on:change=move|e|{let limit=event_target_value(&e).parse::<u32>().unwrap_or(25);load(None,active_query.get_untracked(),limit);} ><option value="25">"25 条"</option><option value="50">"50 条"</option><option value="100">"100 条"</option></select></div></header><div class="card-body">
    {if p.usage.entries.is_empty(){view!{<p class="account-empty">"所选时间范围内暂无更多调用记录，可调整时间后查询。"</p>}.into_any()}else{view!{<div class="table-wrap"><table><thead><tr><th>"时间"</th><th>"模型"</th><th>"计费单位"</th><th>"用量"</th><th>"结果"</th><th>"扣费"</th></tr></thead><tbody>{p.usage.entries.into_iter().map(|e|{let tokens=api::entry_tokens(&e);let debit=api::reporting_debit(&e);let unit=fmt::unit_label(&e.billing_unit);let outcome=e.outcome.unwrap_or_else(||e.status.clone());view!{<tr><td data-label="时间">{fmt::ts_short(e.created_ms)}</td><td data-label="模型">{e.model}</td><td data-label="计费单位">{unit}</td><td data-label="用量" class="num">{tokens}</td><td data-label="结果">{outcome}</td><td data-label="扣费" class="num">{debit}</td></tr>}}).collect_view()}</tbody></table></div>}.into_any()}}
    <div class="account-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move|_|load(None, active_query.get_untracked(),page_limit.get_untracked())>"回到最新"</button><button type="button" class="ghost" disabled=move||busy.get()||before.is_none() on:click=move|_|load(before, active_query.get_untracked(),page_limit.get_untracked())>"较早记录"</button></div></div></section>
    <super::key_manager::KeyManager ctx=ctx owner=owner models=models channels=channels services=services personal=true/>
    <section class="card"><header class="card-head"><h2>"已授权调用模型"</h2></header><div class="card-body"><p class="account-help">"可使用以下名称发起请求。共享模型由管理员维护，此处只展示已授权入口，不展示或编辑内部调用目标；单个 Key 还可能有更小的授权范围。"</p><p role="status">{move||if models.get().is_empty(){"暂无可公开调用的已授权模型。"}else{""}}</p><div class="policy-options">{move||models.get().into_iter().map(|name|view!{<code>{name}</code>}).collect_view()}</div></div></section>
    <section class="card"><header class="card-head"><h2>"已授权透传服务"</h2></header><div class="card-body"><p class="account-help">"按服务前缀发起 HTTP 请求，认证方式仍为 API Key；单个 Key 还可能有更小的授权范围。"</p><p role="status">{move||if services.get().is_empty(){"暂无已授权且已启用的透传服务。"}else{""}}</p><div class="policy-options">{move||services.get().into_iter().map(|s|view!{<div><strong>{s["name"].as_str().unwrap_or("").to_owned()}</strong><div><code>{s["prefix"].as_str().unwrap_or("").to_owned()}</code></div></div>}).collect_view()}</div></div></section>
}
    })}</div>}
}
