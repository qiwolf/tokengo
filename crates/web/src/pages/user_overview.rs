//! Personal overview: owner-scoped APIs and a dedicated safe row type.
use leptos::prelude::*;
use crate::{account_overview as api, fmt, Ctx};
use crate::model::status_label;
use super::PageHead;

fn known_tokens(value:i64,known:bool)->String {
    if known {fmt::num(value)}else{"未报告".into()}
}
fn actual_model(r:&api::RequestRow)->String {
    if r.endpoint=="passthrough" {"HTTP透传".into()}
    else if !r.served_model.trim().is_empty(){r.served_model.clone()}
    else if r.status=="running"{"等待调用上游".into()}
    else {"未记录".into()}
}
fn attempt_label(outcome:&str)->&'static str {
    match outcome {"success"=>"成功","channel_fault"=>"上游失败","client_error"=>"客户端错误","canceled"=>"取消","not_sent"=>"未发送",_=>"未记录"}
}
fn request_fields(r:&api::RequestRow)->Vec<(&'static str,String)> {
    let actual=actual_model(r);
    vec![
        ("实际模型",actual),("请求入口",r.model.clone()),
        ("请求路径",r.request_path.clone().filter(|p|!p.trim().is_empty()).unwrap_or_else(||if r.endpoint=="passthrough"{"未记录".into()}else{r.endpoint.clone()})),
        ("Key",r.key_display_name.clone().unwrap_or_else(||"未记录 Key".into())),
        ("结果",status_label(&r.status).0.into()),("HTTP 状态",r.http_status.map(|s|s.to_string()).unwrap_or_else(||"未记录".into())),
        ("请求时间",fmt::ts(r.ts_ms)),("总耗时",if r.status=="running"{"进行中…".into()}else{fmt::dur(r.latency_ms)}),
        ("首字节延迟",r.first_byte_ms.map(fmt::dur).unwrap_or_else(||"未记录".into())),
        ("输入 Token",known_tokens(r.input_tokens,r.input_tokens_known)),("输出 Token",known_tokens(r.output_tokens,r.output_tokens_known)),
        ("输出方式",if r.streamed{"流式"}else{"非流式"}.into()),
    ]
}
fn breakdown_items(s:&api::Stats,category:usize)->Vec<(String,String,String)> {
    match category {
        1=>s.by_kind.iter().map(|r|(r.label.clone(),
            format!("{} 次调用 · 成功率 {} · 平均耗时 {}",fmt::num(r.requests),fmt::pct(r.ok,r.ok+r.failed+r.client_errors),if r.ok>0{fmt::dur(r.avg_ms)}else{"—".into()}),
            format!("成功 {} · 失败 {} · 客户端错误 {} · 取消 {} · 已记录 Token {} · 未完整报告 {} 次",r.ok,r.failed,r.client_errors,r.canceled,fmt::num(r.input_tokens+r.output_tokens),r.usage.unknown_requests))).collect(),
        2=>s.by_model.iter().map(|r|(r.model.clone(),
            format!("{} 次调用 · 成功率 {}",fmt::num(r.requests),fmt::pct(r.ok,r.ok+r.failed+r.client_errors)),
            format!("成功 {} · 失败 {} · 客户端错误 {} · 已记录 Token {} · 未完整报告 {} 次",r.ok,r.failed,r.client_errors,fmt::num(r.input_tokens+r.output_tokens),r.usage.unknown_requests))).collect(),
        3=>s.by_client.iter().map(|r|(r.client_ip.clone().unwrap_or_else(||"未知来源".into()),
            format!("{} 次调用 · 成功率 {}",fmt::num(r.requests),fmt::pct(r.ok,r.ok+r.failed+r.client_errors)),
            format!("成功 {} · 失败 {} · 客户端错误 {} · 已记录 Token {} · 未完整报告 {} 次",r.ok,r.failed,r.client_errors,fmt::num(r.input_tokens+r.output_tokens),r.usage.unknown_requests))).collect(),
        _=>Vec::new(),
    }
}
#[component]
pub fn UserOverview(ctx:Ctx)->impl IntoView {
    let tasks=crate::page_tasks::PageTasks::new();
    let account_id=ctx.account.get_untracked().map(|a|a.id);
    let active=move||account_id.is_some() && ctx.account.get_untracked().is_some_and(|a|Some(a.id)==account_id&&!a.is_root());
    let category=RwSignal::new(0usize);
    let hours=RwSignal::new(24i64);
    let stats=RwSignal::new(None::<api::StatsResp>);
    let recent=RwSignal::new(api::Logs::default());
    let cursor=RwSignal::new(None::<i64>);
    let busy=RwSignal::new(false);
    let error=RwSignal::new(None::<String>);
    let request_open=RwSignal::new(false);
    let selected=RwSignal::new(None::<i64>);
    let detail=RwSignal::new(None::<api::Detail>);
    let detail_busy=RwSignal::new(false);
    let detail_error=RwSignal::new(None::<String>);
    let generation=RwSignal::new(0u64);
    Effect::new(move |_| {
        let valid=ctx.account.get().is_some_and(|a|Some(a.id)==account_id&&!a.is_root());
        if !valid {stats.set(None);recent.set(api::Logs::default());detail.set(None);selected.set(None);request_open.set(false);}
    });
    let load=move |before:Option<i64>| {
        if !active()||busy.get_untracked(){return;}
        busy.set(true);error.set(None);cursor.set(before);
        let window=hours.get_untracked();
        tasks.spawn(async move {
            crate::page_tasks::parallel([
                Box::pin(async move {let response=api::stats(window).await;if !active(){return;}match response {
                    Ok(v)=>{if hours.get_untracked()==window{stats.set(Some(v));}},
                    Err(e)=>{ctx.note_err(&e);error.set(Some(e.message));}
                }}) as crate::page_tasks::ReadTask,
                Box::pin(async move {let response=api::logs(before).await;if !active(){return;}match response {
                    Ok(v)=>{if cursor.get_untracked()==before{recent.set(v);}},
                    Err(e)=>{ctx.note_err(&e);error.set(Some(e.message));}
                }})
            ]).await;
            busy.set(false);
        });
    };
    load(None);
    let poll=crate::every(5000,move||{
        if active() && crate::page_tasks::foreground() && !busy.get_untracked() && !request_open.get_untracked() && (category.get_untracked()>0||cursor.get_untracked().is_none()){load(cursor.get_untracked());}
    });
    on_cleanup(move||crate::stop_timer(poll));
    let open=move |id:i64| {
        if !active(){return;}
        selected.set(Some(id));request_open.set(true);detail.set(None);detail_error.set(None);detail_busy.set(true);
        generation.update(|v|*v+=1);let current=generation.get_untracked();
        tasks.spawn(async move {
            let result=api::detail(id).await;
            if !active()||selected.get_untracked()!=Some(id)||generation.get_untracked()!=current{return;}
            match result{Ok(v)=>detail.set(Some(v)),Err(e)=>{ctx.note_err(&e);detail_error.set(Some(e.message));}}
            detail_busy.set(false);
        });
    };
    view!{<div class="page">
        <PageHead title="运行总览" desc="仅显示你自己的调用、Key与用量；管理员的上游配置和其他用户数据不会在此展示。">
            <button type="button" class="ghost" disabled=move||busy.get() on:click=move|_|load(cursor.get_untracked())>{move||if busy.get(){"刷新中…"}else{"刷新"}}</button>
        </PageHead>
        <div class="overview-dashboard"><div class="overview-window" role="group" aria-label="时间窗口">
            {[1i64,24,72,168].into_iter().map(|h|view!{<button type="button" class="chip" class:active=move||hours.get()==h aria-pressed=move||(hours.get()==h).to_string() disabled=move||busy.get() on:click=move|_|{hours.set(h);load(None);}>{if h==168{"7天".into()}else if h==72{"3天".into()}else{format!("{h}小时")}}</button>}).collect_view()}
        </div>
        {move||error.get().map(|e|view!{<div class="banner bad" role="alert">{e}</div>})}
        <Show when=move||busy.get()&&stats.get().is_none()><p role="status">"正在读取你的调用…"</p></Show>
        {move||stats.get().map(|s|{let s=s.stats;view!{<section class="overview-summary" aria-label="我的调用统计"><div class="overview-metrics">
            <div class="stat"><div class="stat-title">"请求数"</div><div class="stat-value">{fmt::num(s.total)}</div></div>
            <div class="stat"><div class="stat-title">"成功率"</div><div class="stat-value">{fmt::pct(s.ok,s.ok+s.failed+s.client_errors)}</div></div>
            <div class="stat"><div class="stat-title">"失败数"</div><div class="stat-value">{fmt::num(s.failed)}</div></div>
            <div class="stat"><div class="stat-title">"已记录 Token"</div><div class="stat-value">{fmt::compact_tokens(s.input_tokens+s.output_tokens)}</div></div>
        </div><p class="muted small overview-usage-note">{format!("取消 {} · 客户端错误 {} · 未完整报告用量 {} 次 · 历史来源不确定 {} 次；已记录数值不等于完整实际用量",s.canceled,s.client_errors,s.usage.unknown_requests,s.usage.legacy_requests)}</p></section>}})}
        </div>
        <nav class="overview-tabs" aria-label="我的总览分类">{["最近调用","按类型","按模型","按客户端"].into_iter().enumerate().map(|(i,label)|view!{<button type="button" class:active=move||category.get()==i aria-pressed=move||(category.get()==i).to_string() on:click=move|_|category.set(i)>{label}</button>}).collect_view()}</nav>
        <Show when=move||category.get()==0>
        <section class="card recent-calls"><header class="card-head"><h2>"最近调用"</h2><span class="spacer"></span><span class="muted small">{move||if cursor.get().is_none(){"5秒自动刷新"}else{"较早记录"}}</span></header>
        <div class="recent-list"><Show when=move||!busy.get()&&recent.with(|r|r.logs.is_empty())><div class="empty">"暂无调用"</div></Show>
        <For each=move||recent.with(|p|p.logs.iter().map(|r|r.id).collect::<Vec<_>>()) key=|id|*id children=move|id|{
            let row=Memo::new(move |_|recent.with(|p|p.logs.iter().find(|r|r.id==id).cloned()));
            view!{<article class="recent-call" data-request-id=id>
                {move||row.get().map(|r|{
                    let (label,css)=status_label(&r.status);let actual=actual_model(&r);
                    view!{<div class="recent-main"><span class=format!("tag {css}")>{label}</span><b class="recent-model">{actual}</b><span class="recent-duration">{if r.status=="running"{"进行中…".into()}else{fmt::dur(r.latency_ms)}}</span></div>
                    <div class="recent-key"><span class="muted small">"Key"</span><span>{r.key_display_name.unwrap_or_else(||"未记录 Key".into())}</span></div>}
                })}
                <div class="recent-meta">{move||row.get().map(|r|view!{<time>{fmt::ts(r.ts_ms)}</time><span>{format!("入口：{}",r.model)}</span>})}<button type="button" class="ghost" aria-haspopup="dialog" on:click=move|_|open(id)>"查看请求详情"</button></div>
            </article>}
        }/>
        </div><div class="account-actions"><button type="button" class="ghost" disabled=move||busy.get() on:click=move|_|load(None)>"回到最新"</button><button type="button" class="ghost" disabled=move||busy.get()||recent.get().next_before_id.is_none() on:click=move|_|load(recent.get_untracked().next_before_id)>"较早记录"</button></div></section>
        </Show>
        <Show when=move||(category.get()>0)><section class="card overview-breakdown"><header class="card-head"><h2>{move||["最近调用","按类型","按模型","按客户端"][category.get()]}</h2><span class="spacer"></span><span class="muted small">"仅你自己的调用 · 5秒自动刷新"</span></header>
        <div class="metric-list">{move||stats.get().map(|s|{let items=breakdown_items(&s.stats,category.get());if items.is_empty(){view!{<div class="empty">"这个时间窗口暂无调用"</div>}.into_any()}else{items.into_iter().map(|(title,summary,usage)|view!{<article class="metric-card"><div class="metric-head"><b>{title}</b></div><p>{summary}</p><p class="muted small">{usage}</p></article>}).collect_view().into_any()}})}</div></section></Show>
        <super::ModuleDialog open=request_open title="请求详情".to_string()><section class="request-inspector" aria-live="polite">
            <h3>{move||selected.get().map(|id|format!("请求 #{id}")).unwrap_or_default()}</h3>
            <Show when=move||detail_busy.get()><p role="status">"加载详情…"</p></Show>
            {move||detail_error.get().map(|e|view!{<div class="banner bad" role="alert">{e}</div>})}
            {move||detail.get().map(|d|view!{<dl class="call-facts">{request_fields(&d.request).into_iter().map(|(name,value)|view!{<div class:fact-wide=name=="请求路径"><dt>{name}</dt><dd>{value}</dd></div>}).collect_view()}</dl>
            <h4>"调用尝试"</h4><ul>{d.attempts.into_iter().map(|a|view!{<li>{format!("第{}次 · {} · HTTP {} · {}",a.ordinal+1,attempt_label(&a.outcome),a.http_status.map(|v|v.to_string()).unwrap_or_else(||"未记录".into()),fmt::dur(a.duration_ms))}</li>}).collect_view()}</ul>})}
        </section></super::ModuleDialog>
    </div>}
}

#[cfg(test)]
mod tests {
    #[test]
    fn personal_breakdowns_render_only_own_safe_dimensions_and_recorded_usage() {
        let mut s=crate::account_overview::Stats::default();
        s.by_kind.push(crate::model::KindStat{label:"对话".into(),requests:2,ok:1,..Default::default()});
        s.by_model.push(crate::model::ModelStat{model:"actual-logical".into(),requests:2,..Default::default()});
        s.by_client.push(crate::model::ClientStat{client_ip:Some("synthetic-client".into()),requests:2,..Default::default()});
        assert_eq!(super::breakdown_items(&s,1)[0].0,"对话");
        assert_eq!(super::breakdown_items(&s,2)[0].0,"actual-logical");
        assert_eq!(super::breakdown_items(&s,3)[0].0,"synthetic-client");
        assert!(super::breakdown_items(&s,2)[0].2.contains("已记录 Token"));
        assert!(super::breakdown_items(&s,4).is_empty());
    }
    #[test]
    fn personal_detail_has_only_safe_fields_and_preserves_logical_and_virtual_models() {
        let row:crate::account_overview::RequestRow=serde_json::from_value(serde_json::json!({
            "id":1,"ts_ms":0,"endpoint":"/v1/responses","model":"my-virtual","served_model":"actual-logical",
            "status":"ok","latency_ms":12,"input_tokens":0,"output_tokens":0,
            "key_display_name":"my key","channel_name":"PRIVATE-PROVIDER","channel_display_label":"PRIVATE-KEY",
            "upstream_path":"/PRIVATE-PATH","error":"PRIVATE-ERROR"
        })).unwrap();
        let fields=super::request_fields(&row);let text=format!("{fields:?}");
        assert!(fields.contains(&("实际模型","actual-logical".into())));
        assert!(fields.contains(&("请求入口","my-virtual".into())));
        assert!(fields.contains(&("输入 Token","未报告".into())));
        assert!(!text.contains("PRIVATE"),"{text}");
        let row=crate::account_overview::RequestRow{endpoint:"passthrough".into(),model:"HTTP透传".into(),served_model:"HTTP透传".into(),..Default::default()};
        assert!(super::request_fields(&row).contains(&("请求路径","未记录".into())));
        assert_eq!(super::actual_model(&row),"HTTP透传");
        let row=crate::account_overview::RequestRow{endpoint:"/v1/responses".into(),model:"virtual".into(),status:"running".into(),..Default::default()};
        assert_eq!(super::actual_model(&row),"等待调用上游");
    }
}
