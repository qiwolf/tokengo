use super::{Page, PageHead, usage_period::{UsagePeriod, default_period_query, period_query, applied_period as period_label}};
use super::usage_dashboard::UsageDashboard;
use super::scroll_feed::ScrollFeed;
use crate::{Ctx, account_api as api, fmt};
use leptos::prelude::*;
#[component]
pub fn Portal(ctx: Ctx) -> impl IntoView {
    let tasks=crate::page_tasks::PageTasks::new();
    let data = RwSignal::new(None::<api::Portal>);
    let summary=RwSignal::new(serde_json::Value::Null);let analytics=RwSignal::new(serde_json::Value::Null);
    let page_limit=RwSignal::new(25u32);
    let list_generation=RwSignal::new(0u64);
    let loaded_older=RwSignal::new(false);

    let models = RwSignal::new(Vec::<String>::new());
    let services=RwSignal::new(Vec::<serde_json::Value>::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let grants_error=RwSignal::new(String::new());
    // Grants are independent of ledger/analytics success; the personal endpoint owns visibility filtering.
    tasks.spawn(async move {
        match crate::admin_api::request("GET","/api/account/keys",None).await {
            Ok(value)=>{
                models.set(value["models"].as_array().map(|a|a.iter().filter_map(|v|v.as_str().map(str::to_owned)).collect()).unwrap_or_default());
                services.set(value["services"].as_array().cloned().unwrap_or_default());
            },
            Err(e)=>{ctx.note_err(&e);grants_error.set(format!("授权信息读取失败：{}",e.message));}
        }
    });

    let period = RwSignal::new("today".to_string());let start_time=RwSignal::new(String::new());let end_time=RwSignal::new(String::new());
    let start = RwSignal::new(String::new());
    let end = RwSignal::new(String::new());
    let active_query = RwSignal::new(format!("{}&currency=all",default_period_query()));
    let applied_period = RwSignal::new(period_label(&active_query.get_untracked()));
    let statistics=RwSignal::new(api::UsageStatisticsRequest::default());
    let statistics_busy=RwSignal::new(false);let statistics_error=RwSignal::new(String::new());
    let load = move |before: Option<i64>, query: String, limit: u32, refresh_statistics: bool| {
        if busy.get_untracked() { return; }
        let append=before.is_some();
        list_generation.update(|g|*g+=1);let request_generation=list_generation.get_untracked();
        if !append {loaded_older.set(false);}
        page_limit.set(limit);active_query.set(query.clone());
        let mut report=None;
        if !append {statistics.update(|s|report=s.begin(&query,refresh_statistics));}
        if report.is_some(){summary.set(serde_json::Value::Null);analytics.set(serde_json::Value::Null);statistics_busy.set(true);statistics_error.set(String::new());}
        busy.set(true);
        error.set(String::new());
        tasks.spawn(async move {
            let result=api::portal(before, &query, limit).await;
            if list_generation.get_untracked()!=request_generation {return;}
            match result {
                Ok(mut v) => {
                    if append {
                        let empty=v.usage.entries.is_empty();
                        data.with_untracked(|old|{if let Some(old)=old {
                            let mut entries=old.usage.entries.clone();
                            append_entries(&mut entries,std::mem::take(&mut v.usage.entries));
                            v.usage.entries=entries;
                        }});
                        if empty||v.usage.next_before_id.is_some_and(|next|before.is_some_and(|current|next>=current)){v.usage.next_before_id=None;}
                        loaded_older.set(true);
                    }
                    page_limit.set(limit);
                    data.set(Some(v));
                    applied_period.set(period_label(&query));
                    active_query.set(query);
                    // Render the account and lightweight ledger page before asking for interval totals.
                    if let Some((generation,filter))=report {
                        tasks.spawn(async move {
                            let result=api::usage_statistics(&filter).await;
                            if !statistics.with_untracked(|s|s.accepts(generation)){return;}
                            match result {Ok(v)=>{summary.set(v.summary);analytics.set(v.analytics);},Err(e)=>{ctx.note_err(&e);statistics_error.set(e.message);statistics.update(|s|s.failed(generation));}}
                            statistics_busy.set(false);
                        });
                    }
                },
                Err(e) => {
                    ctx.note_err(&e);error.set(e.message);
                    if let Some((generation,_))=report {statistics.update(|s|s.not_started(generation));statistics_busy.set(false);statistics_error.set("列表查询失败，统计尚未读取。".into());}
                }
            }
            busy.set(false);
        });
    };
    load(None, active_query.get_untracked(),page_limit.get_untracked(),false);
    let search = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() { return; }
        let selected = period.get_untracked();
        match period_query(&selected, &start.get_untracked(), &end.get_untracked(), &start_time.get_untracked(), &end_time.get_untracked()) {
            Ok(query) => {if let Some(window)=web_sys::window(){window.scroll_to_with_x_and_y(0.0,0.0);}load(None, format!("{query}&currency=all"),page_limit.get_untracked(),false)},
            Err(e) => error.set(e),
        }
    };
    view! {<div class="page portal-page"><PageHead title="我的空间" desc="查看本人用量、费用及已授权模型与服务。本地预算与历史冻结标记不再限制调用。"><button type="button" class="primary" on:click=move |_|ctx.go(Page::Keys)>"API Key"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move|_|load(None, active_query.get_untracked(),page_limit.get_untracked(),true)>"刷新"</button></PageHead>
    <section class="card portal-usage-panel"><header class="card-head"><h2>"用量看板"</h2></header><div class="card-body">
    <form class="usage-filter-bar" on:submit=search><fieldset disabled=move||busy.get()><div class="usage-filter-row"><UsagePeriod period=period start=start end=end start_time=start_time end_time=end_time compact=true/><div class="usage-inline-actions"><button class="primary" type="submit">{move||if busy.get(){"查询中…"}else{"查询"}}</button><button type="button" class="ghost" on:click=move |_|{period.set("today".into());start.set(String::new());end.set(String::new());start_time.set(String::new());end_time.set(String::new());load(None,format!("{}&currency=all",default_period_query()),25,false);}>"重置"</button></div></div></fieldset></form>

    <p class="billing-query-status" role="status" aria-live="polite">{move||format!("当前结果：{}{}",applied_period.get(),if busy.get(){" · 查询中…"}else{""})}</p>
    <Show when=move||statistics_busy.get()><p role="status" aria-live="polite">"正在统计所选范围，调用明细可先查看…"</p></Show>
    <Show when=move||!statistics_error.get().is_empty()><p role="alert">{move||statistics_error.get()}</p></Show>
    <Show when=move||!statistics_busy.get()&&statistics_error.get().is_empty()&&summary.get().is_object()><UsageDashboard summary=summary analytics=analytics/></Show>
    </div></section>
    <Show when=move||!error.get().is_empty()><div class="banner bad" role="alert">{move||error.get()}</div></Show>
    <Show when=move||busy.get()&&data.get().is_none()><div class="account-loading" role="status">"正在读取你的用量…"</div></Show>
    {move||data.get().map(|p|{
    view!{
    <section class="card"><header id="portal-ledger-start" class="card-head usage-ledger-heading scroll-feed-start"><div><h2>"调用明细"</h2><span class="muted small">"最新在前"</span></div><div class="usage-page-size"><label for="portal-usage-limit">"每批"</label><select id="portal-usage-limit" disabled=move||busy.get() prop:value=move||page_limit.get().to_string() on:change=move|e|{let limit=event_target_value(&e).parse::<u32>().unwrap_or(25);load(None,active_query.get_untracked(),limit,false);} ><option value="25">"25 条"</option><option value="50">"50 条"</option><option value="100">"100 条"</option></select></div></header><div class="card-body">
    {if p.usage.entries.is_empty(){view!{<p class="account-empty">"所选时间范围内暂无更多调用记录，可调整时间后查询。"</p>}.into_any()}else{view!{<div class="table-wrap"><table><thead><tr><th>"时间"</th><th>"模型"</th><th>"计费单位"</th><th>"用量"</th><th>"结果"</th><th>"扣费"</th></tr></thead><tbody>{p.usage.entries.into_iter().map(|e|{let tokens=api::entry_tokens(&e);let debit=api::reporting_debit(&e);let unit=fmt::unit_label(&e.billing_unit);let outcome=e.outcome.unwrap_or_else(||e.status.clone());view!{<tr><td data-label="时间">{fmt::ts_short(e.created_ms)}</td><td data-label="模型">{e.model}</td><td data-label="计费单位">{unit}</td><td data-label="用量" class="num">{tokens}</td><td data-label="结果">{outcome}</td><td data-label="扣费" class="num">{debit}</td></tr>}}).collect_view()}</tbody></table></div>}.into_any()}}
    </div></section>}})}
    <ScrollFeed anchor="portal-ledger-start" busy=Signal::derive(move||busy.get()) has_more=Signal::derive(move||data.with(|p|p.as_ref().is_some_and(|p|p.usage.next_before_id.is_some()))) failed=Signal::derive(move||!error.get().is_empty()) show_latest=Signal::derive(move||loaded_older.get()) label="调用记录"
        on_more=Callback::new(move|()|{let before=data.with_untracked(|p|p.as_ref().and_then(|p|p.usage.next_before_id));load(before,active_query.get_untracked(),page_limit.get_untracked(),false);})
        on_latest=Callback::new(move|()|load(None,active_query.get_untracked(),page_limit.get_untracked(),false))/>

    <Show when=move||!grants_error.get().is_empty()><p role="alert">{move||grants_error.get()}</p></Show>
    <section class="card"><header class="card-head"><h2>"已授权调用模型"</h2></header><div class="card-body"><p class="account-help">"可使用以下名称发起请求。共享模型由管理员维护，此处只展示已授权入口，不展示或编辑内部调用目标；单个 Key 还可能有更小的授权范围。"</p><p role="status">{move||if models.get().is_empty(){"暂无可公开调用的已授权模型。"}else{""}}</p><div class="policy-options">{move||models.get().into_iter().map(|name|view!{<code>{name}</code>}).collect_view()}</div></div></section>
    <section class="card"><header class="card-head"><h2>"已授权透传服务"</h2></header><div class="card-body"><p class="account-help">"按服务前缀发起 HTTP 请求，认证方式仍为 API Key；单个 Key 还可能有更小的授权范围。"</p><p role="status">{move||if services.get().is_empty(){"暂无已授权且已启用的透传服务。"}else{""}}</p><div class="policy-options">{move||services.get().into_iter().map(|s|view!{<div><strong>{s["name"].as_str().unwrap_or("").to_owned()}</strong><div><code>{s["prefix"].as_str().unwrap_or("").to_owned()}</code></div></div>}).collect_view()}</div></div></section>
    </div>}
}

/// Direct personal Key entry; no portal, ledger or analytics request is required.
#[component]
pub fn PersonalKeys(ctx:Ctx)->impl IntoView {
    let owner=RwSignal::new(None::<i64>);
    let models=RwSignal::new(Vec::<String>::new());
    let services=RwSignal::new(Vec::<serde_json::Value>::new());
    let channels=RwSignal::new(Vec::<(i64,String)>::new());
    view!{<div class="page personal-keys-page"><PageHead title="API Key" desc="创建和管理自己的 API Key，设置权限、限流与有效期。"/>
        <super::key_manager::KeyManager ctx=ctx owner=owner models=models channels=channels services=services personal=true/>
    </div>}
}

fn append_entries(current:&mut Vec<api::Entry>,incoming:Vec<api::Entry>) {
    let mut seen:std::collections::HashSet<i64>=current.iter().map(|e|e.id).collect();
    current.extend(incoming.into_iter().filter(|e|seen.insert(e.id)));
}

#[cfg(test)]
mod scroll_tests {
    fn entry(id:i64)->crate::account_api::Entry {
        serde_json::from_value(serde_json::json!({"id":id,"request_id":id,"key_id":1,"model":"synthetic","billing_unit":"token","status":"settled","outcome":null,"input_tokens":1,"output_tokens":1,"usage_certainty":"reported","debit_micro":0,"created_ms":id,"settled_ms":null})).unwrap()
    }
    #[test]
    fn overlapping_pages_keep_loaded_rows_and_append_each_older_id_once() {
        let mut rows=vec![entry(9),entry(8)];
        super::append_entries(&mut rows,vec![entry(8),entry(7),entry(7),entry(6)]);
        assert_eq!(rows.iter().map(|e|e.id).collect::<Vec<_>>(),vec![9,8,7,6]);
    }
}
