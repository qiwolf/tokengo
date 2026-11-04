//! 总览：一段时间的账 + 最近调用 + 各条渠道的分布 + 能被调用的模型名。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{LogRow, NameRoute, RequestDetail, StatsResp, status_label};
use crate::pages::PageHead;
use crate::Ctx;

#[component]
pub fn Overview(ctx: Ctx) -> impl IntoView {
    if ctx.is_root() {view!{<AdminOverview ctx=ctx/>}.into_any()}
    else {view!{<super::user_overview::UserOverview ctx=ctx/>}.into_any()}
}

#[component]
fn AdminOverview(ctx: Ctx) -> impl IntoView {
    let tasks = crate::page_tasks::PageTasks::new();
    let (hours, set_hours) = signal(24i64);
    let stats = RwSignal::new(StatsResp::default());
    let callable = RwSignal::new(Vec::<NameRoute>::new());
    // 客户端清单里有没有 auto（一个模型都没勾选时它不出现）
    let auto_ready = RwSignal::new(false);
    let recent = RwSignal::new(Vec::<LogRow>::new());
    let channels = RwSignal::new(Vec::<crate::model::Channel>::new());
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let cooling=RwSignal::new(Vec::<crate::model::ModelCooldown>::new());
    let faults=RwSignal::new(Vec::<crate::model::AccountFault>::new());
    let cooldown_now=RwSignal::new(0i64);
    let clear_busy=RwSignal::new(false);
    let category = RwSignal::new(0usize);
    let metric_open = RwSignal::new(false);
    let metric_detail = RwSignal::new((String::new(), String::new()));
    let stats_open = RwSignal::new(false);
    let callable_open = RwSignal::new(false);

    let reload = move || {
        if busy.get_untracked() { return; }
        busy.set(true);
        tasks.spawn(async move {
            let h = hours.get_untracked();
            crate::page_tasks::parallel([
                Box::pin(async move {
                    match api::stats(h).await {
                        Ok(s) => {if !metric_open.get_untracked() && hours.get_untracked()==h {stats.set(s);} err.set(None);}
                        Err(e) => {ctx.note_err(&e);err.set(Some(e.message));}
                    }
                }) as crate::page_tasks::ReadTask,
                Box::pin(async move {
                    match api::model_routes().await {
                        Ok(r) => {callable.set(r.models);auto_ready.set(r.auto.available);}
                        Err(e) => ctx.note_err(&e),
                    }
                }),
                Box::pin(async move {
                    if category.get_untracked()==0 {
                        match api::logs(12).await {Ok(v)=>recent.set(v),Err(e)=>ctx.note_err(&e)}
                    }
                }),
                Box::pin(async move {
                    match api::channels().await {Ok(v)=>channels.set(v),Err(e)=>ctx.note_err(&e)}
                }),
                Box::pin(async move {
                    match api::model_cooldowns().await {
                        Ok(r)=>{faults.set(r.faults);cooling.set(r.cooldowns);cooldown_now.set(r.now_ms);}
                        Err(e)=>ctx.note_err(&e),
                    }
                }),
            ]).await;
            busy.set(false);
        });
    };
    reload();

    let recent_busy = RwSignal::new(false);
    let expanded = RwSignal::new(None::<i64>);
    let request_open = RwSignal::new(false);
    let detail = RwSignal::new(None::<RequestDetail>);
    let detail_error = RwSignal::new(None::<String>);
    let detail_busy = RwSignal::new(false);
    let detail_generation = RwSignal::new(0u64);
    let open_detail = move |id: i64| {
        detail_generation.update(|v| *v += 1);
        let generation = detail_generation.get_untracked();
        expanded.set(Some(id));
        request_open.set(true);
        detail.set(None);
        detail_error.set(None);
        detail_busy.set(true);
        tasks.spawn(async move {
            let result = api::request_detail(id).await;
            if expanded.get_untracked() != Some(id) || detail_generation.get_untracked() != generation { return; }
            match result {
                Ok(v) => detail.set(Some(v)),
                Err(e) => { ctx.note_err(&e); detail_error.set(Some(e.message)); }
            }
            detail_busy.set(false);
        });
    };
    let refresh_active = move || {
        if !crate::page_tasks::foreground() || busy.get_untracked() || recent_busy.get_untracked() || metric_open.get_untracked() || request_open.get_untracked() { return; }
        recent_busy.set(true);
        let selected=category.get_untracked();
        let window=hours.get_untracked();
        tasks.spawn(async move {
            if selected==0 {
                match api::logs(12).await { Ok(v)=>recent.set(v),Err(e)=>ctx.note_err(&e) }
            } else if selected==5 {
                match api::model_cooldowns().await {Ok(r)=>{faults.set(r.faults);cooling.set(r.cooldowns);cooldown_now.set(r.now_ms);},Err(e)=>ctx.note_err(&e)}
            } else {
                match api::stats(window).await { Ok(v)=>{if hours.get_untracked()==window {stats.set(v);}},Err(e)=>ctx.note_err(&e) }
            }
            recent_busy.set(false);
        });
    };
    let clear=move |model:Option<String>,id:Option<i64>|{
        if clear_busy.get_untracked(){return;}
        if !web_sys::window().and_then(|w|w.confirm_with_message("解除模型冷却？历史调用记录保持不变。").ok()).unwrap_or(false){return;}
        clear_busy.set(true);
        tasks.spawn(async move {
            match api::clear_cooldowns(model,id).await {Ok(())=>reload(),Err(e)=>ctx.say(e.message)}
            clear_busy.set(false);
        });
    };
    let retry_account=move |id:i64,model:String|{
        if clear_busy.get_untracked(){return;}
        if !web_sys::window().and_then(|w|w.confirm_with_message("允许下一条真实调用恢复试用？").ok()).unwrap_or(false){return;}
        clear_busy.set(true);
        tasks.spawn(async move{match api::retry_account(id,model).await{Ok(())=>reload(),Err(e)=>ctx.say(e.message)}clear_busy.set(false);});
    };
    let refresh_timer = crate::every(5000, refresh_active);
    on_cleanup(move || crate::stop_timer(refresh_timer));

    let pick = move |h: i64| {
        set_hours.set(h);
        if busy.get_untracked() {
            // A window change must not get lost behind an unrelated initial read.
            tasks.spawn(async move {
                match api::stats(h).await {
                    Ok(s)=>{if hours.get_untracked()==h && !metric_open.get_untracked(){stats.set(s);err.set(None);}},
                    Err(e)=>{ctx.note_err(&e);if hours.get_untracked()==h {err.set(Some(e.message));}},
                }
            });
        } else { reload(); }
    };

    let channel_name = move |id: Option<i64>| resolved_channel_name(id, None, &channels.get());

    view! {
        <div class="page">
            <PageHead
                title="运行总览"
                desc="最近这段时间的请求、各条渠道的表现，以及客户端能打的模型名。"
            >
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
            </PageHead>

            <div class="overview-dashboard">
            <div class="overview-window" role="group" aria-label="时间窗口">
                    {[1i64, 24, 72, 168]
                    .into_iter()
                    .map(|h| {
                        let label = match h {
                            1 => "1 小时".to_string(),
                            24 => "24 小时".to_string(),
                            72 => "3 天".to_string(),
                            _ => "7 天".to_string(),
                        };
                        view! {
                            <button
                                class="chip"
                                class:active=move || hours.get() == h
                                aria-pressed=move || if hours.get()==h {"true"} else {"false"}
                                on:click=move |_| pick(h)
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <section class="overview-summary" aria-label="调用统计">
                <div class="overview-metrics">
                    <div class="stat"><div class="stat-title">"请求数"</div><div class="stat-value">{move || fmt::num(stats.get().stats.total)}</div></div>
                    <div class="stat"><div class="stat-title">"成功率"</div><div class="stat-value">{move || {let s=stats.get().stats; fmt::pct(s.ok,s.ok+s.failed+s.client_errors)}}</div></div>
                    <div class="stat"><div class="stat-title">"失败数"</div><div class="stat-value">{move || fmt::num(stats.get().stats.failed)}</div></div>
                    <div class="stat"><div class="stat-title">"Token"</div><div class="stat-value">{move || {let s=stats.get().stats; fmt::compact_tokens(s.input_tokens+s.output_tokens)}}</div></div>
                </div>
                <div class="overview-stat-details"><button type="button" class="ghost" aria-haspopup="dialog" on:click=move |_|stats_open.set(true)>"查看统计详情"</button></div>
                <super::ModuleDialog open=stats_open title="统计详情".to_string()><div class="overview-stat-details">
                    <dl>
                        <div><dt>"成功"</dt><dd>{move || fmt::num(stats.get().stats.ok)}</dd></div>
                        <div><dt>"取消"</dt><dd>{move || fmt::num(stats.get().stats.canceled)}</dd></div>
                        <div><dt>"客户端错误"</dt><dd>{move || fmt::num(stats.get().stats.client_errors)}</dd></div>
                        <div><dt>"Token 合计"</dt><dd>{move || {let s=stats.get().stats;fmt::num(s.input_tokens+s.output_tokens)}}</dd></div>
                        <div><dt>"输入 Token"</dt><dd>{move || fmt::num(stats.get().stats.input_tokens)}</dd></div>
                        <div><dt>"输出 Token"</dt><dd>{move || fmt::num(stats.get().stats.output_tokens)}</dd></div>
                        <div><dt>"未完整报告"</dt><dd>{move || format!("{} 次",fmt::num(stats.get().stats.usage.unknown_requests))}</dd></div>
                        <div><dt>"历史来源不确定"</dt><dd>{move || format!("{} 次",fmt::num(stats.get().stats.usage.legacy_requests))}</dd></div>
                    </dl>
                </div></super::ModuleDialog>
            </section>
            </div>

            <nav class="overview-tabs" aria-label="总览分类">
                {["最近调用","按类型","按渠道","按模型","按客户端","调用异常"].into_iter().enumerate().map(|(i,label)| view! {
                    <button class:active=move || category.get()==i aria-pressed=move || category.get()==i on:click=move |_| {category.set(i); refresh_active();}>{label}</button>
                }).collect_view()}
            </nav>
            <Show when=move || category.get()==0>
            <section class="card recent-calls">
                <header class="card-head"><h2>"最近调用"</h2><span class="spacer"></span><span class="muted small">"5 秒自动刷新"</span></header>
                <div class="recent-list">
                    <Show when=move || recent.with(|rows| rows.is_empty())>
                        <div class="empty">"暂无调用"</div>
                    </Show>
                    // Key by request ID: detail toggles and polling must retain the row DOM.
                    <For
                        each=move || {
                            let mut ids = recent.with(|rows| rows.iter().map(|r| r.id).collect::<Vec<_>>());
                            // Keep an inspected request when new calls push it out of the window.
                            if let Some(id) = expanded.get() {
                                if !ids.contains(&id) { ids.push(id); }
                            }
                            ids
                        }
                        key=|id| *id
                        children=move |id| {
                            let row = Memo::new(move |_| recent.with(|rows| rows.iter().find(|r| r.id == id).cloned()).or_else(|| detail.get().filter(|d| d.request.id == id).map(|d| d.request)));
                            view! {
                                <article class="recent-call" data-request-id=id>
                                    {move || row.get().map(|r| {
                                        let (base_label,css) = status_label(&r.status);
                                        let label=match r.error_code.as_deref() {Some("model_not_configured")=>"模型未配置",Some("auto_no_candidate")=>"无可用候选",Some("vision_not_ready")=>"视觉未就绪",Some("request_invalid")=>"请求参数错误",Some("models_cooling")=>"模型冷却中",_=>base_label};
                                        view! {
                                            <div class="recent-main">
                                                <span class=format!("tag {css}")>{label}</span>
                                                <b class="recent-model">{actual_model(&r)}</b>
                                                <span class="recent-duration">{if r.status=="running" {"进行中…".into()} else {fmt::dur(r.latency_ms)}}</span>
                                            </div>
                                        }
                                    })}
                                    <div class="recent-key">
                                        <span class="muted small">"Key"</span>
                                        {move || row.get().map(|r| {
                                            let label = r.key_display_name.unwrap_or_else(|| "未记录 Key".into());
                                            let title = label.clone();
                                            view! { <span title=title>{label}</span> }
                                        })}
                                    </div>
                                    <div class="recent-meta">
                                        {move || row.get().map(|r| {
                                            let time = fmt::ts(r.ts_ms);
                                            let clock = time.get(11..).unwrap_or(&time).to_string();
                                            let channel = actual_channel(&r);
                                            let channel_title = channel.clone();
                                            view! {
                                                <time title=time>{clock}</time><span class="recent-channel" title=channel_title>{channel}</span>
                                                {r.has_image.then(|| view! { <span class="tag vision">"带图"</span> })}
                                            }
                                        })}
                                    <button type="button" class="ghost" aria-haspopup="dialog" on:click=move |_|open_detail(id)>"查看请求详情"</button>
                                    </div>
                                </article>
                            }
                        }
                    />
                </div>
            </section>

            </Show>
            <Show when=move || {category.get()>0 && category.get()<5}>
                <section class="card overview-breakdown">
                    <header class="card-head"><h2>{move || ["最近调用","按类型","按渠道","按模型","按客户端","调用异常"][category.get()]}</h2><span class="spacer"></span><span class="muted small">"5 秒自动刷新"</span></header>
                    <Show when=move ||category.get()==2><p class="muted small">"按接入连接汇总：供应商 · Key 备注。同一 Key 的不同连接分别统计，与按模型统计区分；透传显示服务名。"</p></Show>
                    <div class="metric-list">
                    {move || {
                        let st=stats.get().stats;
                        let items:Vec<(String,String,String,String)>=match category.get() {
                            1=>st.by_kind.into_iter().map(|k| (k.label,format!("{} 次调用 · 成功率 {}",fmt::num(k.requests),fmt::pct(k.ok,k.ok+k.failed+k.client_errors)),format!("平均耗时 {} · Token {}",if k.ok>0 {fmt::dur(k.avg_ms)} else {"—".into()},format!("{} · 未完整报告 {} 次",fmt::num(k.input_tokens+k.output_tokens),k.usage.unknown_requests)),format!("成功 {} · 失败 {} · 客户端错 {} · 取消 {} · 输入 Token {} · 输出 Token {}",k.ok,k.failed,k.client_errors,k.canceled,k.input_tokens,k.output_tokens))).collect(),
                            2=>st.by_channel.into_iter().map(|c| (statistic_channel_name(&c,&channels.get()),format!("{} 次尝试 · 成功率 {}",fmt::num(c.attempts),fmt::pct(c.successes,c.attempts)),format!("连接 #{} · 平均耗时 {} · 未完整报告 {} 次",c.channel_id,fmt::dur(c.avg_ms),c.usage.unknown_requests),format!("成功 {} · 渠道失败 {} · 输入 Token {} · 输出 Token {}",c.successes,c.channel_faults,c.input_tokens,c.output_tokens))).collect(),
                            3=>st.by_model.into_iter().map(|m| (m.model,format!("{} 次调用 · 成功率 {}",fmt::num(m.requests),fmt::pct(m.ok,m.ok+m.failed+m.client_errors)),format!("Token {}",format!("{} · 未完整报告 {} 次",fmt::num(m.input_tokens+m.output_tokens),m.usage.unknown_requests)),format!("成功 {} · 失败 {} · 客户端错 {} · 输入 Token {} · 输出 Token {}",m.ok,m.failed,m.client_errors,m.input_tokens,m.output_tokens))).collect(),
                            4=>st.by_client.into_iter().map(|c| (c.client_ip.unwrap_or_else(||"未知来源".into()),format!("{} 次调用 · 成功率 {}",fmt::num(c.requests),fmt::pct(c.ok,c.ok+c.failed+c.client_errors)),format!("Token {}",format!("{} · 未完整报告 {} 次",fmt::num(c.input_tokens+c.output_tokens),c.usage.unknown_requests)),format!("成功 {} · 失败 {} · 客户端错 {} · 输入 Token {} · 输出 Token {}",c.ok,c.failed,c.client_errors,c.input_tokens,c.output_tokens))).collect(),
                            _=>Vec::new(),
                        };
                        if items.is_empty(){return view!{<div class="empty">"当前时间窗口暂无记录"</div>}.into_any();}
                        items.into_iter().map(|(title,primary,secondary,detail)|{
                            let title_for_action=title.clone();
                            view!{<article class="metric-card"><div class="metric-head"><b>{title}</b><button type="button" class="ghost" aria-haspopup="dialog" on:click=move |_| {metric_detail.set((title_for_action.clone(),detail.clone()));metric_open.set(true);}>"查看详情"</button></div><div class="metric-primary">{primary}</div>{(!secondary.is_empty()).then(||view!{<div class="metric-secondary">{secondary}</div>})}</article>}
                        }).collect_view().into_any()
                    }}
                    </div>
                </section>
            </Show>
            <Show when=move || category.get()==5>
                <section class="card overview-account-faults">
                    <header class="card-head"><h2>"账号与权限异常"</h2></header>
                    <div class="metric-list">{move ||{
                        let rows=faults.get();
                        if rows.is_empty(){return view!{<div class="empty">"暂无账号异常"</div>}.into_any();}
                        rows.into_iter().map(|r|{
                            let id=r.channel_id;let model=r.model.clone();
                            let status=if r.lease_until>cooldown_now.get(){"恢复试用中"}else if r.ready{"等待恢复试用"}else{"已暂停"};
                            view!{<article class="metric-card cooldown-card"><div class="metric-head"><b>{channel_name(Some(id))}</b><button disabled=move ||clear_busy.get() on:click=move |_|retry_account(id,model.clone())>"恢复试用"</button></div><div class="cooldown-facts"><span class="tag">{r.reason}</span><span>{status}</span><span>{if r.model.is_empty(){"整个渠道".to_string()}else{format!("模型：{}",r.model)}}</span><span>{fmt::ts(r.occurred_ms)}</span></div></article>}
                        }).collect_view().into_any()
                    }}</div>
                </section>
                <section class="card overview-cooldowns">
                    <header class="card-head"><h2>"模型冷却"</h2><span class="spacer"></span><button disabled=move ||clear_busy.get() ||cooling.get().is_empty() on:click=move |_|clear(None,None)>"解除全部"</button></header>
                    <div class="metric-list">{move ||{
                        let rows=cooling.get();
                        if rows.is_empty(){return view!{<div class="empty">"暂无冷却中的模型"</div>}.into_any();}
                        rows.into_iter().map(|r|{
                            let model=r.model.clone();let id=r.channel_id;
                            let remaining=(r.until_ms-cooldown_now.get()).max(0);
                            let status=if r.lease_until>cooldown_now.get(){"恢复试用中".to_string()}else if remaining>0{format!("剩余 {}",fmt::span(remaining))}else{"等待恢复试用".into()};
                            view!{<article class="metric-card cooldown-card"><div class="metric-head"><b>{r.model}</b><button disabled=move ||clear_busy.get() on:click=move |_|clear(Some(model.clone()),Some(id))>"解除冷却"</button></div><div class="cooldown-facts"><span>{format!("渠道：{}",channel_name(Some(id)))}</span><span>{format!("上游：{}",r.upstream)}</span><span class="tag">{status}</span><span>{format!("连续失败：{}",r.failures)}</span></div><p class="cooldown-reason">{r.reason}</p></article>}
                        }).collect_view().into_any()
                    }}</div>
                </section>
            </Show>
            <button type="button" class="ghost" aria-haspopup="dialog" on:click=move |_|callable_open.set(true)>"查看可调用模型"</button>
            <super::ModuleDialog open=callable_open title="可调用模型".to_string()>
                <header class="card-head">

                </header>
                <div class="card-body">
            <div class="chips">
                {move || {
                    let v: Vec<NameRoute> = callable
                        .get()
                        .into_iter()
                        .filter(|m| m.routed)
                        .collect();
                    if v.is_empty() {
                        return view! {
                            <span class="muted">
                                "一个都没有 —— 先去「渠道」页勾上要用的模型名"
                            </span>
                        }
                            .into_any();
                    }
                    // auto 是一个保留名：勾选过的模型一个都没有时它不出现，这里也不列。
                    let auto_chip = if auto_ready.get() {
                        view! {
                            <code
                                class="chip code"
                                title="保留名：客户端打它就在你勾选过的模型里挑一个最好的"
                            >
                                "auto"
                                <span class="tag">"自动挑"</span>
                            </code>
                        }
                            .into_any()
                    } else {
                        ().into_any()
                    };
                    view! {
                        <>
                            {auto_chip}
                            {v
                                .into_iter()
                                .map(|m| {
                                    view! {
                                        <code
                                            class="chip code"
                                            title=if m.can_see {
                                                "收得下图片输入"
                                            } else {
                                                "纯文本：带图的请求会被回 422"
                                            }
                                        >
                                            {m.name}
                                            {m
                                                .can_see
                                                .then(|| view! { <span class="tag vision">"视觉"</span> })}
                                        </code>
                                    }
                                        .into_any()
                                })
                                .collect_view()}
                        </>
                    }
                        .into_any()
                }}
            </div>
                </div>
            </super::ModuleDialog>
            <super::ModuleDialog open=metric_open title="统计明细".to_string()><h3>{move ||metric_detail.get().0}</h3><p>{move ||metric_detail.get().1}</p></super::ModuleDialog>
            <super::ModuleDialog open=request_open title="请求详情".to_string()>
                <section class="request-inspector" aria-live="polite">
                    <h3>{move ||expanded.get().map(|id|format!("请求 #{id}")).unwrap_or_default()}</h3>
                    <Show when=move ||detail_busy.get()><p class="muted">"加载详情…"</p></Show>
                    {move ||detail_error.get().map(|e|view!{<div class="banner bad" role="alert">{e}</div>})}
                    {move ||detail.get().filter(|d|Some(d.request.id)==expanded.get()).map(|mut d|{if let Some(r)=recent.with(|rows|rows.iter().find(|r|r.id==d.request.id).cloned()){d.request=r;}view!{<RequestDetailView detail=d/>}})}
                </section>
            </super::ModuleDialog>
        </div>
    }
}

fn statistic_channel_name(row: &crate::model::ChannelStat, channels: &[crate::model::Channel]) -> String {
    let label = row.display_label.as_deref().map(str::trim).filter(|v| !v.is_empty())
        .or(row.channel_name.as_deref());
    resolved_channel_name(Some(row.channel_id),label,channels)
}

fn resolved_channel_name(id: Option<i64>, provided: Option<&str>, channels: &[crate::model::Channel]) -> String {
    let Some(id) = id else { return "—".into(); };
    provided.map(str::trim).filter(|name| !name.is_empty())
        .or_else(|| channels.iter().find(|c| c.id == id).map(|c| c.name.trim()).filter(|name| !name.is_empty()))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("渠道 #{id}（名称未记录）"))
}

fn actual_model(r:&LogRow)->String {
    if is_passthrough(r) {r.channel_name.as_deref().map(str::trim).filter(|s|!s.is_empty()).unwrap_or("未记录").to_owned()}
    else if !r.served_model.trim().is_empty(){r.served_model.clone()}
    else if r.status=="running" {"等待调用上游".into()}
    else if r.channel_id.is_none(){"未调用上游".into()}
    else if r.model.eq_ignore_ascii_case("auto"){"实际模型未记录".into()}
    else {r.model.clone()}
}

fn is_passthrough(r:&LogRow)->bool {r.endpoint.starts_with("passthrough:")}

fn recorded_path(path:Option<&str>)->String {
    path.filter(|s|!s.trim().is_empty()).unwrap_or("未记录").to_owned()
}

/// These four fields are used directly by the request detail view. Never infer
/// historical paths from a current channel, route prefix or upstream setting.
fn request_identity_fields(r:&LogRow)->[(&'static str,String);4] {
    if is_passthrough(r) {
        [("透传服务（当前名称）",actual_model(r)),("实际渠道",actual_channel(r)),
         ("请求入口",recorded_path(r.request_path.as_deref())),
         ("上游接口",recorded_path(r.upstream_path.as_deref()))]
    } else {
        [("实际模型",actual_model(r)),("实际渠道",actual_channel(r)),
         ("请求入口",r.model.clone()),("接口",r.endpoint.clone())]
    }
}

fn actual_channel(r: &LogRow) -> String {
    match r.channel_id {
        Some(id) => match r.channel_display_label.as_deref().map(str::trim).filter(|s| !s.is_empty())
            .or_else(|| r.channel_name.as_deref().map(str::trim).filter(|s| !s.is_empty())) {
            Some(name) => format!("{name}（#{id}）"),
            None => format!("渠道 #{id}（名称未记录）"),
        },
        None if r.status == "running" => "等待调用上游".into(),
        None => "未调用上游".into(),
    }
}

#[component]
fn RequestDetailView(detail: RequestDetail) -> impl IntoView {
    let r=detail.request;
    let [(model_label,actual),(channel_label,channel),(entry_label,entry),(interface_label,interface)]=request_identity_fields(&r);
    let label=match r.error_code.as_deref(){Some("model_not_configured")=>"模型未配置",Some("vision_not_ready")=>"视觉未就绪",Some("models_cooling")=>"模型冷却中",Some("auto_no_candidate")=>"无可用候选",Some("request_invalid")=>"请求参数错误",_=>status_label(&r.status).0};
    let fields=vec![
        (model_label,actual),(channel_label,channel),(entry_label,entry),("结果",label.into()),
        ("请求时间",fmt::ts(r.ts_ms)),("总耗时",if r.status=="running" {"进行中…".into()}else{fmt::dur(r.latency_ms)}),
        ("首字节延迟",if r.status=="running" {"等待上游响应".into()}else if r.channel_id.is_none(){"不适用".into()}else{r.first_byte_ms.map(fmt::dur).unwrap_or_else(||"未记录".into())}),
        ("输入 Token",usage_number(r.input_tokens,r.input_tokens_known,&r.usage_source)),("输出 Token",usage_number(r.output_tokens,r.output_tokens_known,&r.usage_source)),
        ("用量来源",if r.usage_source=="legacy" {"历史记录，零值来源不确定"}else if r.usage_source=="upstream" {"上游已报告字段"}else{"上游未报告"}.into()),
        ("输出方式",if r.streamed{"流式"}else{"非流式"}.into()),
        ("HTTP 状态",r.http_status.map(|n|n.to_string()).unwrap_or_else(||"未记录".into())),
        ("来源 IP",r.client_ip.unwrap_or_else(||"未提供".into())),
        ("请求应用",r.user_agent.unwrap_or_else(||"未提供".into())),
        (interface_label,interface),
    ];
    view!{
        <dl class="call-facts">{fields.into_iter().map(|(k,v)|view!{<div class:fact-wide=matches!(k,"请求应用"|"接口"|"请求入口"|"上游接口")><dt>{k}</dt><dd>{v}</dd></div>}).collect_view()}</dl>
        <div class="call-flags">{r.has_image.then(||view!{<span class="tag vision">"图片输入"</span>})}{r.explored.then(||view!{<span class="tag">"模型探索"</span>})}</div>
        {r.error.filter(|e|!e.trim().is_empty()).map(|e|view!{<div class="call-error"><b>"错误信息"</b><p>{e}</p></div>})}
    }
}

/// 流水页也用得到的「状态」小标签在这里定义一次。
#[component]
pub fn StatusTag(status: String) -> impl IntoView {
    let (label, class) = status_label(&status);
    view! { <span class=format!("tag {class}")>{label}</span> }
}

#[cfg(test)]
mod tests {
    #[test]
    fn recent_and_detail_channels_prefer_unified_provider_key_label() {
        let row:crate::model::LogRow=serde_json::from_value(serde_json::json!({
            "id":1,"ts_ms":0,"endpoint":"/v1/chat/completions","model":"client-model","status":"ok",
            "channel_id":59,"channel_name":"2号","channel_display_label":"供应商 · 2号",
            "latency_ms":1,"input_tokens":0,"output_tokens":0
        })).unwrap();
        assert_eq!(super::actual_channel(&row),"供应商 · 2号（#59）");
        assert_eq!(super::request_identity_fields(&row)[1],("实际渠道","供应商 · 2号（#59）".into()));
    }

    use crate::model::KindStat;

    #[test]
    fn channel_statistics_prefer_connection_labels_without_replacing_original_names() {
        let stats: crate::model::StatsResp = serde_json::from_value(serde_json::json!({
            "window_hours":24,"stats":{"since_ms":0,"total":1,"ok":1,"failed":0,
            "canceled":0,"client_errors":0,"input_tokens":0,"output_tokens":0,
            "by_channel":[{"channel_id":7,"channel_name":"原始模型连接",
            "display_label":"供应商 · 生产Key","attempts":1,"successes":1,"channel_faults":0,
            "avg_ms":1,"input_tokens":0,"output_tokens":0}]}
        })).unwrap();
        let mut row=stats.stats.by_channel[0].clone();
        let old_directory=[crate::model::Channel{id:7,name:"缓存旧名".into(),..Default::default()}];
        assert_eq!(super::statistic_channel_name(&row,&old_directory),"供应商 · 生产Key");
        assert_eq!(row.channel_name.as_deref(),Some("原始模型连接"));
        row.display_label=None;
        assert_eq!(super::statistic_channel_name(&row,&old_directory),"原始模型连接");
        row.display_label=Some(" \t".into());
        assert_eq!(super::statistic_channel_name(&row,&old_directory),"原始模型连接");
        row.channel_name=None;
        assert_eq!(super::statistic_channel_name(&row,&[]),"渠道 #7（名称未记录）");
        let recent=crate::model::LogRow{channel_id:Some(7),channel_name:Some("原始模型连接".into()),..Default::default()};
        assert_eq!(super::actual_channel(&recent),"原始模型连接（#7）");
    }

    #[test]
    fn channel_labels_use_statistics_names_for_passthrough_and_never_assume_deletion() {
        use super::resolved_channel_name;
        let llm_channels = [crate::model::Channel { id: 7, name: "旧名称".into(), ..Default::default() }];
        assert_eq!(resolved_channel_name(Some(51), Some("GPU03 CLIP"), &llm_channels), "GPU03 CLIP");
        assert_eq!(resolved_channel_name(Some(7), Some("新名称"), &llm_channels), "新名称");
        assert_eq!(resolved_channel_name(Some(7), Some(" \t"), &llm_channels), "旧名称");
        assert_eq!(resolved_channel_name(Some(51), None, &llm_channels), "渠道 #51（名称未记录）");
        assert_eq!(resolved_channel_name(Some(51), Some(" \t"), &[]), "渠道 #51（名称未记录）");
        assert_eq!(resolved_channel_name(None, None, &llm_channels), "—");
    }

    #[test]
    fn actual_channel_keeps_virtual_route_destination_and_unknown_channel_identity() {
        let mut r = crate::model::LogRow {
            model: "deepseek-fallback".into(), served_model: "DeepSeek-V4.1-Flash".into(),
            channel_id: Some(17), channel_name: Some("备用渠道".into()), status: "ok".into(),
            ..Default::default()
        };
        assert_eq!(super::actual_channel(&r), "备用渠道（#17）");
        r.channel_name = None;
        assert_eq!(super::actual_channel(&r), "渠道 #17（名称未记录）");
        r.channel_name = Some("  ".into());
        assert_eq!(super::actual_channel(&r), "渠道 #17（名称未记录）");
        r.channel_id = None;
        assert_eq!(super::actual_channel(&r), "未调用上游");
        r.status = "running".into();
        assert_eq!(super::actual_channel(&r), "等待调用上游");
    }

    #[test]
    fn passthrough_request_identity_never_displays_internal_model_identifiers() {
        let mut row=crate::model::LogRow{endpoint:"passthrough:11".into(),model:"passthrough:11".into(),served_model:"passthrough:11".into(),channel_name:Some("synthetic photo service".into()),status:"ok".into(),..Default::default()};
        assert_eq!(super::actual_model(&row),"synthetic photo service");
        row.channel_name=None;
        assert_eq!(super::actual_model(&row),"未记录");
    }

    #[test]
    fn passthrough_detail_fields_keep_both_real_paths_and_neutral_history() {
        let mut row=crate::model::LogRow{endpoint:"passthrough:11".into(),model:"passthrough:11".into(),served_model:"passthrough:11".into(),channel_id:Some(51),channel_name:Some("synthetic photo service".into()),request_path:Some("/photos/clip/txt".into()),upstream_path:Some("/engine/clip/txt".into()),status:"ok".into(),..Default::default()};
        assert_eq!(super::request_identity_fields(&row),[
            ("透传服务（当前名称）","synthetic photo service".into()),
            ("实际渠道","synthetic photo service（#51）".into()),
            ("请求入口","/photos/clip/txt".into()),("上游接口","/engine/clip/txt".into())]);
        row.request_path=None;row.upstream_path=None;
        let historical=super::request_identity_fields(&row);
        assert_eq!(historical[0],("透传服务（当前名称）","synthetic photo service".into()));
        assert_eq!(historical[2],("请求入口","未记录".into()));
        assert_eq!(historical[3],("上游接口","未记录".into()));
        assert!(historical.iter().all(|(_,v)|!v.contains("passthrough:")));
        row.channel_name=None;
        assert_eq!(super::request_identity_fields(&row)[0],("透传服务（当前名称）","未记录".into()));
    }

    #[test]
    fn llm_detail_identity_preserves_actual_model_virtual_entry_and_endpoint() {
        let row=crate::model::LogRow{endpoint:"/v1/responses".into(),model:"virtual-fallback".into(),served_model:"selected-logical-model".into(),channel_id:Some(7),channel_name:Some("synthetic provider".into()),request_path:Some("/must-not-replace-llm-entry".into()),upstream_path:Some("/must-not-replace-llm-interface".into()),status:"ok".into(),..Default::default()};
        assert_eq!(super::request_identity_fields(&row),[
            ("实际模型","selected-logical-model".into()),("实际渠道","synthetic provider（#7）".into()),
            ("请求入口","virtual-fallback".into()),("接口","/v1/responses".into())]);
    }

    #[test]
    fn actual_model_never_presents_auto_as_executed_model(){
        let mut r=crate::model::LogRow::default();r.model="auto".into();
        assert_eq!(super::actual_model(&r),"未调用上游");
        r.status="running".into();assert_eq!(super::actual_model(&r),"等待调用上游");
        r.status="failed".into();
        r.channel_id=Some(3);assert_eq!(super::actual_model(&r),"实际模型未记录");
        r.served_model="real-model".into();assert_eq!(super::actual_model(&r),"real-model");
    }

    /// 成功率的分子分母要跟总览四张卡一个口径：分母是「打进来几次」，不是「试了几次」。
    #[test]
    fn kind_success_rate_uses_requests_as_the_denominator() {
        let mut k = KindStat {
            kind: "embeddings".into(),
            label: "向量".into(),
            requests: 8,
            ok: 7,
            failed:1,
            ..Default::default()
        };
        assert_eq!(k.success_rate(), 87.5);
        k.requests = 0; k.ok=0; k.failed=0;
        assert_eq!(k.success_rate(), 0.0, "没请求时面板显示「—」，不该算出个百分比");
        // 全成功 / 全失败两个边界
        k.requests = 3;
        k.ok = 3;
        assert_eq!(k.success_rate(), 100.0);
        k.ok = 0;
        assert_eq!(k.success_rate(), 0.0);
    }
}

fn usage_number(value:i64, known:bool, source:&str)->String {
    if known {fmt::num(value)} else if source=="legacy" {"历史来源不确定".into()} else {"未报告".into()}
}
