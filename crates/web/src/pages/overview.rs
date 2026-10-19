//! 总览：一段时间的账 + 最近调用 + 各条渠道的分布 + 能被调用的模型名。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{LogRow, NameRoute, RequestDetail, StatsResp, status_label};
use crate::pages::PageHead;
use crate::Ctx;

#[component]
pub fn Overview(ctx: Ctx) -> impl IntoView {
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
    let cooldown_now=RwSignal::new(0i64);
    let clear_busy=RwSignal::new(false);
    let category = RwSignal::new(0usize);
    let metric_open = RwSignal::new(None::<String>);

    let reload = move || {
        if busy.get_untracked() { return; }
        busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let h = hours.get_untracked();
            match api::stats(h).await {
                Ok(s) => {
                    stats.set(s);
                    err.set(None);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(Some(e.message));
                }
            }
            match api::model_routes().await {
                Ok(r) => {
                    callable.set(r.models);
                    auto_ready.set(r.auto.available);
                }
                Err(e) => ctx.note_err(&e),
            }
            if category.get_untracked()==0 { match api::logs(12).await {
                Ok(v) => recent.set(v),
                Err(e) => ctx.note_err(&e),
            }
            }
            match api::channels().await {
                Ok(v) => channels.set(v),
                Err(e) => ctx.note_err(&e),
            }
            if let Ok(r)=api::model_cooldowns().await {cooling.set(r.cooldowns);cooldown_now.set(r.now_ms);}
            busy.set(false);
        });
    };
    reload();

    let recent_busy = RwSignal::new(false);
    let expanded = RwSignal::new(None::<i64>);
    let detail = RwSignal::new(None::<RequestDetail>);
    let detail_error = RwSignal::new(None::<String>);
    let detail_busy = RwSignal::new(false);
    let detail_generation = RwSignal::new(0u64);
    let open_detail = move |id: i64| {
        detail_generation.update(|v| *v += 1);
        let generation = detail_generation.get_untracked();
        expanded.set(Some(id));
        detail.set(None);
        detail_error.set(None);
        detail_busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
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
        if busy.get_untracked() || recent_busy.get_untracked() { return; }
        recent_busy.set(true);
        let selected=category.get_untracked();
        let window=hours.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            if selected==0 {
                match api::logs(12).await { Ok(v)=>recent.set(v),Err(e)=>ctx.note_err(&e) }
            } else if selected==5 {
                match api::model_cooldowns().await {Ok(r)=>{cooling.set(r.cooldowns);cooldown_now.set(r.now_ms);},Err(e)=>ctx.note_err(&e)}
            } else {
                match api::stats(window).await { Ok(v)=>stats.set(v),Err(e)=>ctx.note_err(&e) }
            }
            recent_busy.set(false);
        });
    };
    let clear=move |model:Option<String>,id:Option<i64>|{
        if clear_busy.get_untracked(){return;}
        if !web_sys::window().and_then(|w|w.confirm_with_message("解除模型冷却？历史调用记录保持不变。").ok()).unwrap_or(false){return;}
        clear_busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api::clear_cooldowns(model,id).await {Ok(())=>reload(),Err(e)=>ctx.say(e.message)}
            clear_busy.set(false);
        });
    };
    let refresh_timer = crate::every(5000, refresh_active);
    on_cleanup(move || crate::stop_timer(refresh_timer));

    let pick = move |h: i64| {
        set_hours.set(h);
        reload();
    };

    let channel_name = move |id: Option<i64>| match id {
        None => "—".to_string(),
        Some(i) => channels
            .get()
            .iter()
            .find(|l| l.id == i)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("已删 #{i}")),
    };

    view! {
        <div class="page">
            <PageHead
                title="总览"
                desc="最近这段时间的请求、各条渠道的表现，以及客户端能打的模型名。"
            >
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
            </PageHead>

            <div class="toolbar">
                <span class="seg">
                    <span class="seg-label">"时间窗口"</span>
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
                                on:click=move |_| pick(h)
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
                </span>
                <span class="spacer"></span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            // 四个数：并排的数字条（不是卡片，靠竖线分隔）
            <div class="overview-metrics">
                <div class="stat">
                    <div class="stat-title">"请求数"</div>
                    <div class="stat-value">{move || fmt::num(stats.get().stats.total)}</div>
                    <div class="stat-sub">"当前时间窗口"</div>
                </div>
                <div class="stat">
                    <div class="stat-title">"成功率"</div>
                    <div class="stat-value">
                        {move || {
                            let s = stats.get().stats;
                            fmt::pct(s.ok, s.ok+s.failed+s.client_errors)
                        }}
                    </div>
                    <div class="stat-sub">{move || format!("成功 {}", fmt::num(stats.get().stats.ok))}</div>
                </div>
                <div class="stat">
                    <div class="stat-title">"失败"</div>
                    <div class="stat-value">{move || fmt::num(stats.get().stats.failed)}</div>
                    <div class="stat-sub">
                        {move || {
                            let s = stats.get().stats;
                            format!("取消 {} · 客户端错 {}", fmt::num(s.canceled), fmt::num(s.client_errors))
                        }}
                    </div>
                </div>
                <div class="stat">
                    <div class="stat-title">"Token"</div>
                    <div class="stat-value">
                        {move || {
                            let s = stats.get().stats;
                            fmt::num(s.input_tokens + s.output_tokens)
                        }}
                    </div>
                    <div class="stat-sub">
                        {move || {
                            let s = stats.get().stats;
                            format!("入 {} · 出 {}", fmt::num(s.input_tokens), fmt::num(s.output_tokens))
                        }}
                    </div>
                </div>
            </div>

            <nav class="overview-tabs" aria-label="总览分类">
                {["最近调用","按类型","按渠道","按模型","按客户端","模型冷却"].into_iter().enumerate().map(|(i,label)| view! {
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
                            let opened = move || expanded.get() == Some(id);
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
                                    <div class="recent-meta">
                                        {move || row.get().map(|r| {
                                            let time = fmt::ts(r.ts_ms);
                                            let clock = time.get(11..).unwrap_or(&time).to_string();
                                            view! {
                                                <time title=time>{clock}</time><span class="recent-channel">{channel_name(r.channel_id)}</span>
                                                {r.has_image.then(|| view! { <span class="tag vision">"带图"</span> })}
                                            }
                                        })}
                                    <button class="ghost" aria-expanded=opened on:click=move |_| {
                                        if opened() { expanded.set(None); }
                                        else { open_detail(id); }
                                    }>{move || if opened() {"收起"} else {"详情"}}</button>
                                    </div>
                                    <Show when=opened>
                                        <section class="request-inspector" aria-label="请求详情" aria-live="polite">
                                            <header><h3>{format!("请求 #{id}")}</h3></header>
                                            <Show when=move || detail_busy.get()><p class="muted">"加载详情…"</p></Show>
                                            {move || detail_error.get().map(|e| view! { <div class="banner bad" role="alert">{e}</div> })}
                                            {move || detail.get().filter(|d|d.request.id==id).map(|mut d| {if let Some(r)=recent.with(|rows|rows.iter().find(|r|r.id==id).cloned()){d.request=r;} view! { <RequestDetailView detail=d/> }})}
                                        </section>
                                    </Show>
                                </article>
                            }
                        }
                    />
                </div>
            </section>

            </Show>
            <Show when=move || {category.get()>0 && category.get()<5}>
                <section class="card overview-breakdown">
                    <header class="card-head"><h2>{move || ["最近调用","按类型","按渠道","按模型","按客户端","模型冷却"][category.get()]}</h2><span class="spacer"></span><span class="muted small">"5 秒自动刷新"</span></header>
                    <div class="metric-list">
                    {move || {
                        let st=stats.get().stats;
                        let items:Vec<(String,String,String,String)>=match category.get() {
                            1=>st.by_kind.into_iter().map(|k| (k.label,format!("{} 次调用 · 成功率 {}",fmt::num(k.requests),fmt::pct(k.ok,k.ok+k.failed+k.client_errors)),format!("平均耗时 {} · Token {}",if k.ok>0 {fmt::dur(k.avg_ms)} else {"—".into()},fmt::num(k.input_tokens+k.output_tokens)),format!("成功 {} · 失败 {} · 客户端错 {} · 取消 {} · 输入 Token {} · 输出 Token {}",k.ok,k.failed,k.client_errors,k.canceled,k.input_tokens,k.output_tokens))).collect(),
                            2=>st.by_channel.into_iter().map(|c| (channel_name(Some(c.channel_id)),format!("{} 次尝试 · 成功率 {}",fmt::num(c.attempts),fmt::pct(c.successes,c.attempts)),format!("平均耗时 {}",fmt::dur(c.avg_ms)),format!("成功 {} · 渠道失败 {} · 输入 Token {} · 输出 Token {}",c.successes,c.channel_faults,c.input_tokens,c.output_tokens))).collect(),
                            3=>st.by_model.into_iter().map(|m| (m.model,format!("{} 次调用 · 成功率 {}",fmt::num(m.requests),fmt::pct(m.ok,m.ok+m.failed+m.client_errors)),format!("Token {}",fmt::num(m.input_tokens+m.output_tokens)),format!("成功 {} · 失败 {} · 客户端错 {} · 输入 Token {} · 输出 Token {}",m.ok,m.failed,m.client_errors,m.input_tokens,m.output_tokens))).collect(),
                            4=>st.by_client.into_iter().map(|c| (c.client_ip.unwrap_or_else(||"未知来源".into()),format!("{} 次调用 · 成功率 {}",fmt::num(c.requests),fmt::pct(c.ok,c.ok+c.failed+c.client_errors)),format!("Token {}",fmt::num(c.input_tokens+c.output_tokens)),format!("成功 {} · 失败 {} · 客户端错 {} · 输入 Token {} · 输出 Token {}",c.ok,c.failed,c.client_errors,c.input_tokens,c.output_tokens))).collect(),
                            _=>Vec::new(),
                        };
                        if items.is_empty(){return view!{<div class="empty">"当前时间窗口暂无记录"</div>}.into_any();}
                        items.into_iter().enumerate().map(|(i,(title,primary,secondary,detail))|{
                            let key=format!("{}:{i}:{title}",category.get());let opened=metric_open.get().as_deref()==Some(&key);let clickkey=key.clone();
                            view!{<article class="metric-card"><div class="metric-head"><b>{title}</b><button class="ghost" aria-expanded=opened on:click=move |_| metric_open.update(|v|*v=if v.as_ref()==Some(&clickkey){None}else{Some(clickkey.clone())})>{if opened {"收起"} else {"详情"}}</button></div><div class="metric-primary">{primary}</div>{(!secondary.is_empty()).then(||view!{<div class="metric-secondary">{secondary}</div>})}{opened.then(||view!{<div class="metric-detail">{detail}</div>})}</article>}
                        }).collect_view().into_any()
                    }}
                    </div>
                </section>
            </Show>
            <Show when=move || category.get()==5>
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
            <details class="fold callable-models">
                <summary>"能被调用的模型名"</summary>
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
            </details>
        </div>
    }
}

fn actual_model(r:&LogRow)->String {
    if !r.served_model.trim().is_empty(){r.served_model.clone()}
    else if r.status=="running" {"等待调用上游".into()}
    else if r.channel_id.is_none(){"未调用上游".into()}
    else if r.model.eq_ignore_ascii_case("auto"){"实际模型未记录".into()}
    else {r.model.clone()}
}

#[component]
fn RequestDetailView(detail: RequestDetail) -> impl IntoView {
    let r=detail.request;
    let actual=actual_model(&r);
    let label=match r.error_code.as_deref(){Some("model_not_configured")=>"模型未配置",Some("vision_not_ready")=>"视觉未就绪",Some("models_cooling")=>"模型冷却中",Some("auto_no_candidate")=>"无可用候选",Some("request_invalid")=>"请求参数错误",_=>status_label(&r.status).0};
    let fields=vec![
        ("实际模型",actual),("请求入口",r.model.clone()),("结果",label.into()),
        ("请求时间",fmt::ts(r.ts_ms)),("总耗时",if r.status=="running" {"进行中…".into()}else{fmt::dur(r.latency_ms)}),
        ("首字节延迟",if r.status=="running" {"等待上游响应".into()}else if r.channel_id.is_none(){"不适用".into()}else{r.first_byte_ms.map(fmt::dur).unwrap_or_else(||"未记录".into())}),
        ("输入 Token",fmt::num(r.input_tokens)),("输出 Token",fmt::num(r.output_tokens)),
        ("输出方式",if r.streamed{"流式"}else{"非流式"}.into()),
        ("HTTP 状态",r.http_status.map(|n|n.to_string()).unwrap_or_else(||"未记录".into())),
        ("来源 IP",r.client_ip.unwrap_or_else(||"未提供".into())),
        ("请求应用",r.user_agent.unwrap_or_else(||"未提供".into())),
        ("接口",r.endpoint),
    ];
    view!{
        <dl class="call-facts">{fields.into_iter().map(|(k,v)|view!{<div><dt>{k}</dt><dd>{v}</dd></div>}).collect_view()}</dl>
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
    use crate::model::KindStat;

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
