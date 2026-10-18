//! 总览：一段时间的账 + 最近调用 + 各条渠道的分布 + 能被调用的模型名。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{LogRow, NameRoute, StatsResp, status_label};
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
    let sessions = RwSignal::new(0i64);
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
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
            match api::settings().await {
                Ok(s) => sessions.set(s.active_sessions),
                Err(e) => ctx.note_err(&e),
            }
            busy.set(false);
        });
    };
    reload();

    let recent_busy = RwSignal::new(false);
    let expanded = RwSignal::new(None::<i64>);
    let refresh_active = move || {
        if busy.get_untracked() || recent_busy.get_untracked() { return; }
        recent_busy.set(true);
        let selected=category.get_untracked();
        let window=hours.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            if selected==0 {
                match api::logs(12).await { Ok(v)=>recent.set(v),Err(e)=>ctx.note_err(&e) }
            } else {
                match api::stats(window).await { Ok(v)=>stats.set(v),Err(e)=>ctx.note_err(&e) }
            }
            recent_busy.set(false);
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
                <span class="muted small">"在线会话 " <b>{move || sessions.get()}</b></span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            // 四个数：并排的数字条（不是卡片，靠竖线分隔）
            <div class="stats">
                <div class="stat">
                    <div class="stat-title">"请求数"</div>
                    <div class="stat-value">{move || fmt::num(stats.get().stats.total)}</div>
                    <div class="stat-sub">{move || format!("窗口 {} 小时", stats.get().window_hours)}</div>
                </div>
                <div class="stat">
                    <div class="stat-title">"成功率"</div>
                    <div class="stat-value">
                        {move || {
                            let s = stats.get().stats;
                            fmt::pct(s.ok, s.total)
                        }}
                    </div>
                    <div class="stat-sub">{move || format!("成功 {}", fmt::num(stats.get().stats.ok))}</div>
                </div>
                <div class="stat">
                    <div class="stat-title">"失败的请求"</div>
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
                {["最近调用","按类型","按渠道","按模型","按客户端","错误分布"].into_iter().enumerate().map(|(i,label)| view! {
                    <button class:active=move || category.get()==i aria-pressed=move || category.get()==i on:click=move |_| {category.set(i); refresh_active();}>{label}</button>
                }).collect_view()}
            </nav>
            <Show when=move || category.get()==0>
            <section class="card recent-calls">
                <header class="card-head"><h2>"最近调用"</h2><span class="spacer"></span><span class="muted small">"5 秒自动刷新"</span></header>
                <div class="recent-list">
                    {move || {
                        let rows = recent.get();
                        if rows.is_empty() { return view! { <div class="empty">"暂无调用"</div> }.into_any(); }
                        rows.into_iter().map(|r| {
                            let (label,css) = status_label(&r.status);
                            let id=r.id;
                            let opened=expanded.get()==Some(id);
                            let channel=channel_name(r.channel_id);
                            let time=fmt::ts(r.ts_ms);
                            let clock=time.get(11..).unwrap_or(&time).to_string();
                            let served=r.served_model.clone();
                            view! {
                                <article class="recent-call">
                                    <div class="recent-main">
                                        <span class=format!("tag {css}")>{label}</span>
                                        <b class="recent-model">{r.model.clone()}</b>
                                        <span class="recent-duration">{fmt::dur(r.latency_ms)}</span>
                                    </div>
                                    <div class="recent-meta">
                                        <time title=time>{clock}</time><span class="recent-channel">{channel.clone()}</span>
                                        {r.has_image.then(||view! { <span class="tag vision">"带图"</span> })}
                                        <button class="ghost" aria-expanded=opened on:click=move |_| expanded.update(|v| *v=if *v==Some(id) {None} else {Some(id)})>{if opened {"收起"} else {"详情"}}</button>
                                    </div>
                                    {opened.then(||view! {
                                        <div class="recent-detail">
                                            <span>{format!("请求 #{} · {}",id,r.endpoint)}</span>
                                            <span>{format!("时间：{}",fmt::ts(r.ts_ms))}</span>
                                            <span>{format!("渠道：{channel}")}</span>
                                            <span>{format!("首字节延迟：{}",r.first_byte_ms.map(fmt::dur).unwrap_or_else(||"未记录".into()))}</span>
                                            <span>{format!("总耗时：{}",fmt::dur(r.latency_ms))}</span>
                                            <span>{format!("请求应用（User-Agent）：{}",r.user_agent.as_deref().unwrap_or("未提供"))}</span>
                                            <span>{format!("来源 IP：{}",r.client_ip.as_deref().unwrap_or("未提供"))}</span>
                                            <span>{if r.streamed {"输出方式：流式"} else {"输出方式：非流式"}}</span>
                                            {(!served.is_empty()).then(||view! { <span>{format!("实际模型：{served}")}</span> })}
                                            <span>{format!("Token 入 {} / 出 {}",fmt::num(r.input_tokens),fmt::num(r.output_tokens))}</span>
                                            {r.http_status.map(|h|view! { <span>{format!("HTTP {h}")}</span> })}
                                            {r.error.filter(|e| !e.is_empty()).map(|e|view! { <p class="bad-text">{e}</p> })}
                                        </div>
                                    })}
                                </article>
                            }
                        }).collect_view().into_any()
                    }}
                </div>
            </section>

            </Show>
            <Show when=move || category.get()!=0>
                <section class="card overview-breakdown">
                    <header class="card-head"><h2>{move || ["最近调用","按类型","按渠道","按模型","按客户端","错误分布"][category.get()]}</h2><span class="spacer"></span><span class="muted small">"5 秒自动刷新"</span></header>
                    <div class="metric-list">
                    {move || {
                        let st=stats.get().stats;
                        let items:Vec<(String,String,String,String)>=match category.get() {
                            1=>st.by_kind.into_iter().map(|k| (k.label,format!("{} 次调用 · 成功率 {}",fmt::num(k.requests),fmt::pct(k.ok,k.requests)),format!("平均耗时 {} · Token {}",if k.ok>0 {fmt::dur(k.avg_ms)} else {"—".into()},fmt::num(k.input_tokens+k.output_tokens)),format!("成功 {} · 失败 {} · 客户端错 {} · 取消 {} · 输入 Token {} · 输出 Token {}",k.ok,k.failed,k.client_errors,k.canceled,k.input_tokens,k.output_tokens))).collect(),
                            2=>st.by_channel.into_iter().map(|c| (channel_name(Some(c.channel_id)),format!("{} 次尝试 · 成功率 {}",fmt::num(c.attempts),fmt::pct(c.successes,c.attempts)),format!("平均耗时 {}",fmt::dur(c.avg_ms)),format!("成功 {} · 渠道失败 {} · 输入 Token {} · 输出 Token {}",c.successes,c.channel_faults,c.input_tokens,c.output_tokens))).collect(),
                            3=>st.by_model.into_iter().map(|m| (m.model,format!("{} 次调用 · 成功率 {}",fmt::num(m.requests),fmt::pct(m.ok,m.requests)),format!("Token {}",fmt::num(m.input_tokens+m.output_tokens)),format!("成功 {} · 失败 {} · 客户端错 {} · 输入 Token {} · 输出 Token {}",m.ok,m.failed,m.client_errors,m.input_tokens,m.output_tokens))).collect(),
                            4=>st.by_client.into_iter().map(|c| (c.client_ip.unwrap_or_else(||"未知来源".into()),format!("{} 次调用 · 成功率 {}",fmt::num(c.requests),fmt::pct(c.ok,c.requests)),format!("Token {}",fmt::num(c.input_tokens+c.output_tokens)),format!("成功 {} · 失败 {} · 客户端错 {} · 输入 Token {} · 输出 Token {}",c.ok,c.failed,c.client_errors,c.input_tokens,c.output_tokens))).collect(),
                            5=>st.errors.into_iter().map(|e| (e.error.lines().next().unwrap_or("未记录原因").chars().take(90).collect(),format!("{} 次",e.count),String::new(),e.error)).collect(),
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

/// 流水页也用得到的「状态」小标签在这里定义一次。
#[component]
pub fn StatusTag(status: String) -> impl IntoView {
    let (label, class) = status_label(&status);
    view! { <span class=format!("tag {class}")>{label}</span> }
}

#[cfg(test)]
mod tests {
    use crate::model::KindStat;

    /// 成功率的分子分母要跟总览四张卡一个口径：分母是「打进来几次」，不是「试了几次」。
    #[test]
    fn kind_success_rate_uses_requests_as_the_denominator() {
        let mut k = KindStat {
            kind: "embeddings".into(),
            label: "向量".into(),
            requests: 8,
            ok: 7,
            ..Default::default()
        };
        assert_eq!(k.success_rate(), 87.5);
        k.requests = 0;
        assert_eq!(k.success_rate(), 0.0, "没请求时面板显示「—」，不该算出个百分比");
        // 全成功 / 全失败两个边界
        k.requests = 3;
        k.ok = 3;
        assert_eq!(k.success_rate(), 100.0);
        k.ok = 0;
        assert_eq!(k.success_rate(), 0.0);
    }
}
