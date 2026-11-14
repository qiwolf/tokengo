//! Interval aggregates and a keyboard-readable trend shared by both account roles.
use crate::fmt;
use leptos::prelude::*;
use serde_json::Value;

fn value(row: &Value, name: &str) -> i64 { row[name].as_i64().unwrap_or(0) }
fn count(row: &Value, name: &str) -> String {
    row[name].as_i64().map(fmt::num).unwrap_or_else(|| "—".into())
}
fn compact(row: &Value, name: &str) -> String {
    row[name].as_i64().map(fmt::compact_tokens).unwrap_or_else(|| "—".into())
}
fn cny_amount(row:&Value,name:&str,currency:&str)->Option<i64> {
    // Reporting conversion is authoritative, including explicit null when FX is unavailable.
    if let Some(amount)=row.get("known_debit_cny_micro") {return amount.as_i64();}
    if currency=="CNY" {row[name].as_i64()}else{None}
}
fn money_for(row:&Value,name:&str,currency:&str)->String {
    if !row.is_object() || row.get(name).is_none() && row.get("known_debit_cny_micro").is_none() {return "—".into();}
    cny_amount(row,name,currency).map(|n|format!("¥{}",fmt::usd_input(n))).unwrap_or_else(||"汇率暂不可用".into())
}
fn money(row:&Value,name:&str)->String {money_for(row,name,fmt::currency_of(row))}
fn headline_money(row: &Value, name: &str) -> String {
    let Some(micro)=cny_amount(row,name,fmt::currency_of(row)) else {return money(row,name);};
    let yuan=micro as f64/1_000_000.0;
    if yuan.abs()>=1_000_000.0 {format!("¥{:.2}M",yuan/1_000_000.0)}
    else if yuan.abs()>=1000.0 {format!("¥{:.2}K",yuan/1000.0)}
    else {money(row,name)}
}
fn reporting_fx_note(summary:&Value,analytics:&Value)->String {
    if !summary.is_object(){return "金额以人民币显示".into();}
    if summary.get("known_debit_cny_micro").is_some_and(Value::is_null){return "汇率暂不可用，人民币金额暂无法计算；调用次数和 tokens 仍完整统计。".into();}
    let fx=summary.get("reporting_fx").filter(|v|v.is_object()).or_else(||analytics.get("reporting_fx").filter(|v|v.is_object()));
    if let Some(fx)=fx {
        if fx["status"]=="not_required" {return "金额统一以人民币显示。".into();}
        if let Some(rate)=fx["rate_nano"].as_i64().filter(|n|*n>0) {
            let mut display=format!("{:.9}",rate as f64/1_000_000_000.0);
            while display.ends_with('0'){display.pop();}if display.ends_with('.'){display.pop();}
            let date=fx["date"].as_str().unwrap_or("日期未知");
            let stale=if fx["stale"].as_bool().unwrap_or(false){"（缓存汇率）"}else{""};
            return format!("历史美元已自动折合人民币 · 1 USD = {display} CNY · {date} · 来源 ECB{stale}");
        }
    }
    if cny_amount(summary,"known_debit_micro",fmt::currency_of(summary)).is_some() {"金额以人民币显示；汇率暂不可用时，美元金额不作为 0 计入。".into()}
    else {"汇率暂不可用，人民币金额暂无法计算；调用次数和 tokens 仍完整统计。".into()}
}
fn rate(row: &Value, name: &str) -> String {
    row[name].as_f64().filter(|v|v.is_finite()).map(|v| {
        if v>=1000.0 {fmt::compact_tokens(v.round() as i64)} else {format!("{v:.2}")}
    }).unwrap_or_else(||"—".into())
}
fn rows(row: &Value, name: &str) -> Vec<Value> { row[name].as_array().cloned().unwrap_or_default() }
fn local_time(seconds: i64, detailed: bool) -> String {
    let d=js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(seconds as f64*1000.0));
    if detailed {format!("{:02}-{:02} {:02}:{:02}",d.get_month()+1,d.get_date(),d.get_hours(),d.get_minutes())}
    else {format!("{:02}-{:02}",d.get_month()+1,d.get_date())}
}
fn full_time(seconds: i64) -> String {
    let d=js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(seconds as f64*1000.0));
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}",d.get_full_year(),d.get_month()+1,d.get_date(),d.get_hours(),d.get_minutes(),d.get_seconds())
}
fn bucket_label(bucket: &Value, analytics: &Value) -> String {
    let start=value(bucket,"start_at");
    let end=(start+value(analytics,"bucket_seconds")).min(value(analytics,"end_at"));
    format!("{} 至 {}（不含）",full_time(start),full_time(end))
}
fn selected_value(row: &Value, metric: &str) -> f64 {
    match metric {"requests"=>value(row,"ledger_count") as f64,"debit"=>cny_amount(row,"known_debit_micro",fmt::currency_of(row)).unwrap_or(0) as f64/1_000_000.0,_=>value(row,"total_tokens") as f64}
}
fn axis_label_for(n: f64, metric: &str, _currency:&str) -> String {
    if metric=="debit" {format!("¥{}",fmt::usd_input((n*1_000_000.0).round() as i64))}
    else {fmt::compact_tokens(n.round() as i64)}
}
fn interval_detail(bucket: &Value, analytics: &Value) -> String {
    format!("{} · {} 次调用（含重试） · 已知输入 {} / 输出 {} tokens · 已知扣款 {}{}",
        bucket_label(bucket,analytics),count(bucket,"ledger_count"),count(bucket,"input_tokens"),count(bucket,"output_tokens"),money_for(bucket,"known_debit_micro",fmt::currency_of(analytics)),
        if value(bucket,"unknown_input_entries")+value(bucket,"unknown_output_entries")>0 {" · 含未知用量"}else{""})
}
fn granularity(seconds: i64) -> String {
    if seconds<86400 {format!("每 {} 小时",seconds/3600)}else if seconds==86400 {"每 24 小时（固定间隔）".into()}else{format!("每 {} 天（固定间隔）",seconds/86400)}
}

#[component]
pub fn UsageDashboard(summary: RwSignal<Value>, analytics: RwSignal<Value>) -> impl IntoView {
    let metric=RwSignal::new("tokens");
    let selected=RwSignal::new(None::<usize>);
    Effect::new(move |_| {analytics.track();selected.set(None);});
    view! {
    <section class="usage-dashboard" aria-label="所选区间用量看板">
        <div class="usage-kpis">
            <section class="usage-kpi"><span>"去重请求"</span><strong>{move||count(&summary.get(),"request_count")}</strong><small>{move||format!("{} 笔调用尝试",count(&summary.get(),"ledger_count"))}</small></section>
            <section class="usage-kpi"><span>"已知扣款"</span><strong class:usage-money-unavailable=move||summary.get().is_object()&&cny_amount(&summary.get(),"known_debit_micro",fmt::currency_of(&summary.get())).is_none() title=move||money(&summary.get(),"known_debit_micro")>{move||headline_money(&summary.get(),"known_debit_micro")}</strong><small>"人民币 · 账目累计"</small></section>
            <section class="usage-kpi"><span>"总 tokens"</span><strong title=move||count(&summary.get(),"total_tokens")>{move||compact(&summary.get(),"total_tokens")}</strong><small>"已知输入 + 输出"</small></section>
            <section class="usage-kpi"><span>"输入 tokens"</span><strong title=move||count(&summary.get(),"input_tokens")>{move||compact(&summary.get(),"input_tokens")}</strong><small>{move||format!("{} 笔输入未知",count(&summary.get(),"unknown_input_entries"))}</small></section>
            <section class="usage-kpi"><span>"输出 tokens"</span><strong title=move||count(&summary.get(),"output_tokens")>{move||compact(&summary.get(),"output_tokens")}</strong><small>{move||format!("{} 笔输出未知",count(&summary.get(),"unknown_output_entries"))}</small></section>
            <section class="usage-kpi"><span>"平均 RPM"</span><strong>{move||rate(&analytics.get(),"average_rpm")}</strong><small title="按所选开始时间至结束时间计算；结束时间最晚取当前时间，不含未来时长。">"每分钟去重请求数"</small></section>
            <section class="usage-kpi"><span>"平均 TPM"</span><strong>{move||rate(&analytics.get(),"average_tpm")}</strong><small title="按所选开始时间至结束时间计算；结束时间最晚取当前时间，不含未来时长。">"每分钟已知 tokens"</small></section>
        </div>
        <p class="usage-fx-note">{move||reporting_fx_note(&summary.get(),&analytics.get())}</p>
        <div class="usage-dashboard-note"><span>"汇总覆盖全部匹配记录，翻页不改变统计区间。"</span><span>{move||format!("未定价 {} 笔；未知用量和未知金额未作为 0 计入。",count(&summary.get(),"unpriced_entries"))}</span></div>
        <div class="usage-analysis-grid">
            <section class="usage-trend" aria-label="区间用量趋势">
                <header class="usage-analysis-head"><div><h3>"用量趋势"</h3><span class="muted small">{move||if analytics.get().is_object(){"按调用开始时间 · 自动汇总所选范围".to_string()}else{"正在读取时间分布…".into()}}</span></div>
                    <div class="usage-metric-switch" aria-label="趋势统计指标">{[("tokens","Tokens"),("requests","调用次数"),("debit","扣款")].into_iter().map(|(key,label)|view!{<button type="button" class:active=move||metric.get()==key aria-pressed=move||(metric.get()==key).to_string() on:click=move |_|{metric.set(key);selected.set(None);}>{label}</button>}).collect_view()}</div>
                </header>
                {move||{
                    let data=analytics.get();let buckets=rows(&data,"buckets");let m=metric.get();
                    if !data.is_object() {return view!{<p class="usage-chart-empty" role="status">"暂无时间分布，请重新查询。"</p>}.into_any();}
                    if buckets.is_empty()||value(&summary.get(),"ledger_count")==0 {return view!{<p class="usage-chart-empty" role="status">"所选区间没有调用记录，可调整时间后查询。"</p>}.into_any();}
                    if m=="debit"&&cny_amount(&summary.get(),"known_debit_micro",fmt::currency_of(&summary.get())).is_none(){return view!{<p class="usage-chart-empty" role="status">"汇率暂不可用，暂不能绘制人民币扣款趋势。调用次数与 tokens 仍完整显示。"</p>}.into_any();}
                    let observed_max=buckets.iter().map(|b|selected_value(b,m)).fold(0.0f64,f64::max);
                    let max=if observed_max>0.0{observed_max}else{1.0};
                    let n=buckets.len();let slot=960.0/n as f64;let bar_width=(slot*0.72).max(1.0);
                    let first=local_time(value(&buckets[0],"start_at"),value(&data,"bucket_seconds")<86400);
                    let middle=local_time(value(&buckets[n/2],"start_at"),value(&data,"bucket_seconds")<86400);
                    let last=local_time(value(&buckets[n-1],"start_at"),value(&data,"bucket_seconds")<86400);
                    let chart_label=format!("{}：{}个时间段。可聚焦柱形查看区间明细。",if m=="tokens"{"输入与输出 tokens"}else if m=="debit"{"已知扣款"}else{"调用次数"},n);
                    view! {
                        <div class="usage-chart-layout">
                            <div class="usage-y-axis" aria-hidden="true"><span>{axis_label_for(max,m,fmt::currency_of(&data))}</span><span>{axis_label_for(max/2.0,m,fmt::currency_of(&data))}</span><span>{axis_label_for(0.0,m,fmt::currency_of(&data))}</span></div>
                            <div class="usage-plot"><svg class="usage-chart" viewBox="0 0 960 240" preserveAspectRatio="none" role="group" aria-label=chart_label>
                                <line x1="0" y1="1" x2="960" y2="1" class="usage-grid-line"/><line x1="0" y1="120" x2="960" y2="120" class="usage-grid-line"/><line x1="0" y1="239" x2="960" y2="239" class="usage-grid-line"/>
                                {buckets.iter().enumerate().map(|(i,b)|{
                                    let x=i as f64*slot+(slot-bar_width)/2.0;let h=(selected_value(b,m)/max*232.0).clamp(0.0,232.0);
                                    let input_h=if m=="tokens"{(value(b,"input_tokens") as f64/max*232.0).clamp(0.0,h)}else{h};
                                    let output_h=(h-input_h).max(0.0);let info=interval_detail(b,&data);
                                    view!{<g class="usage-chart-bucket" role="button" tabindex="0" aria-label=info.clone() on:mouseenter=move |_|selected.set(Some(i)) on:mouseleave=move |_|selected.set(None) on:focus=move |_|selected.set(Some(i)) on:blur=move |_|selected.set(None) on:click=move |_|selected.set(Some(i)) on:keydown=move|ev:leptos::ev::KeyboardEvent|{if matches!(ev.key().as_str(),"Enter"|" "){ev.prevent_default();selected.set(Some(i));}}>
                                        <title>{info.clone()}</title><rect x=(i as f64*slot).to_string() y="0" width=slot.to_string() height="240" class="usage-bar-hit" aria-hidden="true"/>
                                        <line x1=(i as f64*slot+slot/2.0).to_string() x2=(i as f64*slot+slot/2.0).to_string() y1="1" y2="239" class="usage-bucket-guide" aria-hidden="true"/>
                                        <rect x=x.to_string() y=(239.0-input_h).to_string() width=bar_width.to_string() height=input_h.to_string() class="usage-bar-input"/>
                                        <rect x=x.to_string() y=(239.0-input_h-output_h).to_string() width=bar_width.to_string() height=output_h.to_string() class="usage-bar-output"/>
                                    </g>}
                                }).collect_view()}
                            </svg><div class="usage-x-axis" aria-hidden="true"><span>{first}</span><span>{middle}</span><span>{last}</span></div></div>
                        </div>
                        <div class="usage-chart-footer">{if m=="tokens"{Some(view!{<div class="usage-chart-legend"><span><i class="usage-legend-input" aria-hidden="true"></i>"输入"</span><span><i class="usage-legend-output" aria-hidden="true"></i>"输出"</span></div>})}else{None}}
                        </div>
                    }.into_any()
                }}
                <p class="usage-bucket-detail" role="status" aria-live="polite">{move||{let a=analytics.get();selected.get().and_then(|i|rows(&a,"buckets").get(i).cloned()).map(|b|interval_detail(&b,&a)).unwrap_or_else(||"悬停或聚焦柱形，查看该区间用量。".into())}}</p>
            </section>
            <section class="usage-models" aria-label="模型用量分布"><header class="usage-analysis-head"><div><h3>"模型分布"</h3><span class="muted small">"所选区间 · 已知 tokens"</span></div></header>
                <div class="usage-model-list">{move||{
                    let all=rows(&analytics.get(),"models");let max=all.iter().map(|r|value(r,"total_tokens")).max().unwrap_or(0).max(1) as f64;
                    if all.is_empty(){return view!{<p class="muted small">"所选区间暂无模型用量。"</p>}.into_any();}
                    all.into_iter().map(|row|{
                        let name=row["model"].as_str().unwrap_or("未记录模型").to_string();let tokens=count(&row,"total_tokens");let requests=count(&row,"ledger_count");let cost=money_for(&row,"known_debit_micro",fmt::currency_of(&analytics.get()));let width=(value(&row,"total_tokens") as f64/max*100.0).clamp(0.0,100.0);
                        view!{<div class="usage-model-row"><div><strong title=name.clone()>{name.clone()}</strong><span>{tokens}" tokens"</span></div><div class="usage-model-track" aria-hidden="true"><span style=format!("width:{width:.3}%")></span></div><small>{requests}" 次调用 · "{cost}</small></div>}
                    }).collect_view().into_any()
                }}</div>
            </section>
        </div>
        <ModelCallAnalysis analytics=analytics/>
    </section>
    }
}

#[cfg(test)] fn axis_label(n:f64,metric:&str)->String{axis_label_for(n,metric,"USD")}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn absent_usage_stays_unknown_while_recorded_zero_stays_zero() {
        assert_eq!(count(&json!({"input_tokens":null}),"input_tokens"),"—");
        assert_eq!(count(&json!({"input_tokens":0}),"input_tokens"),"0");
        assert_eq!(money(&json!({}),"known_debit_micro"),"—");
        assert_eq!(money(&json!({"currency":"CNY","known_debit_micro":0}),"known_debit_micro"),"¥0");
    }
    #[test]
    fn chart_money_preserves_subcent_costs_and_selected_metric_units() {
        let row=json!({"currency":"CNY","request_count":2,"ledger_count":3,"total_tokens":12,"known_debit_micro":1});
        assert_eq!(selected_value(&row,"requests"),3.0);
        assert_eq!(selected_value(&row,"tokens"),12.0);
        assert_eq!(selected_value(&row,"debit"),0.000001);
        assert_eq!(axis_label(selected_value(&row,"debit"),"debit"),"¥0.000001");
        assert_eq!(axis_label(0.0,"debit"),"¥0");
    }
    #[test]
    fn reporting_cny_is_authoritative_and_missing_fx_is_not_zero() {
        let row=json!({"currency":"all","known_debit_micro":null,"known_debit_cny_micro":7_123_456,"total_tokens":123,"ledger_count":7});
        assert_eq!(money(&row,"known_debit_micro"),"¥7.123456");
        assert_eq!(selected_value(&row,"debit"),7.123456);
        let unavailable=json!({"currency":"all","known_debit_micro":999,"known_debit_cny_micro":null,"total_tokens":123,"ledger_count":7});
        assert_eq!(money(&unavailable,"known_debit_micro"),"汇率暂不可用");
        assert_eq!(selected_value(&unavailable,"tokens"),123.0);
        assert_eq!(selected_value(&unavailable,"requests"),7.0);
        assert_eq!(money(&json!({"currency":"USD","known_debit_micro":1}),"known_debit_micro"),"汇率暂不可用");
    }
    #[test]
    fn fx_note_reports_actual_rate_date_source_and_staleness() {
        let row=json!({"reporting_fx":{"rate_nano":7_123_456_789_i64,"date":"2026-11-12","stale":true}});
        assert_eq!(reporting_fx_note(&row,&Value::Null),"历史美元已自动折合人民币 · 1 USD = 7.123456789 CNY · 2026-11-12 · 来源 ECB（缓存汇率）");
        assert!(reporting_fx_note(&json!({"known_debit_cny_micro":null}),&Value::Null).contains("调用次数和 tokens 仍完整统计"));
    }
    #[test]
    fn model_calls_count_attempts_and_keep_other_tail_in_shares() {
        let a=json!({"model":"a","request_count":2,"ledger_count":3,"buckets":[{"start_at":20,"ledger_count":2},{"start_at":10,"ledger_count":1}]});
        let other=json!({"is_other":true,"request_count":1,"ledger_count":1,"buckets":[{"start_at":10,"ledger_count":1}]});
        let total=call_total(&[a.clone(),other.clone()]);
        assert_eq!(total,4);
        assert_eq!(call_share(value(&a,"ledger_count"),total),75.0);
        assert_eq!(call_share(value(&other,"ledger_count"),total),25.0);
        assert_eq!(model_bucket_count(&a,10),1);
        assert_eq!(model_bucket_count(&a,20),2);
        assert_eq!(model_bucket_count(&a,30),0);
        assert_eq!(model_name(&other),"其他模型");
    }
    #[test]
    fn large_money_headlines_remain_compact_and_exact_values_remain_available() {
        let row=json!({"currency":"CNY","known_debit_micro":40_030_000});
        assert_eq!(headline_money(&row,"known_debit_micro"),"¥40.03");
        let row=json!({"currency":"CNY","known_debit_micro":1_234_567_890});
        assert_eq!(headline_money(&row,"known_debit_micro"),"¥1.23K");
        assert_eq!(money(&row,"known_debit_micro"),"¥1234.56789");
    }
}

fn model_name(series: &Value) -> String {
    if series["is_other"].as_bool().unwrap_or(false) {"其他模型".into()}
    else {series["model"].as_str().unwrap_or("未记录模型").to_string()}
}
fn model_bucket_count(series: &Value, start_at: i64) -> i64 {
    rows(series,"buckets").iter().find(|b|value(b,"start_at")==start_at).map(|b|value(b,"ledger_count")).unwrap_or(0)
}
fn call_total(series: &[Value]) -> i64 { series.iter().fold(0i64,|sum,s|sum.saturating_add(value(s,"ledger_count").max(0))) }
fn call_share(calls: i64, total: i64) -> f64 {if total<=0 {0.0}else{(calls.max(0) as f64/total as f64*100.0).clamp(0.0,100.0)}}
fn series_color(i: usize) -> String {format!("var(--usage-series-{})",i%9)}
fn series_dash(i: usize) -> &'static str {["none","8 4","2 3","12 4 2 4","4 3 1 3","16 5","1 4","10 3 3 3","3 3"][i%9]}

#[component]
fn ModelCallAnalysis(analytics: RwSignal<Value>) -> impl IntoView {
    let tab=RwSignal::new("trend");let focused=RwSignal::new(None::<usize>);let selected=RwSignal::new(None::<usize>);
    Effect::new(move |_|{analytics.track();focused.set(None);selected.set(None);});
    view! {
        <section class="usage-model-analysis" aria-label="模型调用分析">
            <header class="usage-analysis-head"><div><h3>"模型调用分析"</h3><span class="muted small">"按实际调用尝试计数，包含重试；与上方去重请求数口径不同。"</span></div>
                <nav class="usage-model-tabs" aria-label="模型调用分析视图">{[("trend","调用趋势"),("distribution","调用次数分布"),("ranking","调用次数排行")].into_iter().map(|(key,label)|view!{<button type="button" class:active=move||tab.get()==key aria-pressed=move||(tab.get()==key).to_string() on:click=move |_|tab.set(key)>{label}</button>}).collect_view()}</nav>
            </header>
            {move||{
                let data=analytics.get();let series=rows(&data,"model_series");let total=call_total(&series);
                if series.is_empty()||total==0 {return view!{<p class="usage-chart-empty" role="status">"所选区间暂无模型调用，可调整时间后查询。"</p>}.into_any();}
                match tab.get() {
                    "distribution"=>view!{<div class="usage-call-distribution"><div class="usage-distribution-total">"共 "<strong>{fmt::num(total)}</strong>" 次调用（含重试）"</div>{series.iter().enumerate().map(|(i,s)|{
                        let name=model_name(s);let calls=value(s,"ledger_count");let share=call_share(calls,total);let label=format!("{}：{}次调用，占比{share:.1}%",name,fmt::num(calls));
                        view!{<div class="usage-distribution-row" aria-label=label><div><strong title=name.clone()>{name.clone()}</strong><span>{fmt::num(calls)}" 次 · "{format!("{share:.1}%")}</span></div><div class="usage-distribution-track" aria-hidden="true"><span style=format!("width:{share:.4}%;background:{}",series_color(i))></span></div></div>}
                    }).collect_view()}</div>}.into_any(),
                    "ranking"=>view!{<div class="usage-call-ranking table-wrap"><table><thead><tr><th>"排名"</th><th>"模型"</th><th class="num">"调用次数"</th><th class="num">"占比"</th></tr></thead><tbody>{series.iter().enumerate().map(|(i,s)|{
                        let name=model_name(s);let calls=value(s,"ledger_count");let share=call_share(calls,total);let rank=if s["is_other"].as_bool().unwrap_or(false){"—".to_string()}else{(i+1).to_string()};
                        view!{<tr><td data-label="排名">{rank}</td><td data-label="模型"><strong title=name.clone()>{name.clone()}</strong></td><td data-label="调用次数" class="num">{fmt::num(calls)}</td><td data-label="占比" class="num">{format!("{share:.1}%")}</td></tr>}
                    }).collect_view()}</tbody><tfoot><tr><th colspan="2">"合计（含重试）"</th><th class="num">{fmt::num(total)}</th><th class="num">"100%"</th></tr></tfoot></table></div>}.into_any(),
                    _=>{
                        let buckets=rows(&data,"buckets");let n=buckets.len();
                        if n==0{return view!{<p class="usage-chart-empty">"暂无调用时间分布，请重新查询。"</p>}.into_any();}
                        let top=series.iter().flat_map(|s|rows(s,"buckets")).map(|b|value(&b,"ledger_count")).max().unwrap_or(0).max(1) as f64;
                        let x=|i:usize|if n==1{480.0}else{i as f64/(n-1) as f64*960.0};let hit_width=960.0/n as f64;
                        let first=local_time(value(&buckets[0],"start_at"),value(&data,"bucket_seconds")<86400);
                        let middle=local_time(value(&buckets[n/2],"start_at"),value(&data,"bucket_seconds")<86400);
                        let last=local_time(value(&buckets[n-1],"start_at"),value(&data,"bucket_seconds")<86400);
                        let focus=focused.get();
                        view!{<div class="usage-model-trend">
                            <div class="usage-chart-layout"><div class="usage-y-axis" aria-hidden="true"><span>{axis_label_for(top,"requests","CNY")}</span><span>{axis_label_for(top/2.0,"requests","CNY")}</span><span>"0"</span></div>
                                <div class="usage-plot"><svg class="usage-chart usage-model-chart" viewBox="0 0 960 240" preserveAspectRatio="none" role="group" aria-label="各模型调用次数趋势。聚焦时间段可查看每个模型的调用次数。">
                                    <line x1="0" y1="1" x2="960" y2="1" class="usage-grid-line"/><line x1="0" y1="120" x2="960" y2="120" class="usage-grid-line"/><line x1="0" y1="239" x2="960" y2="239" class="usage-grid-line"/>
                                    {series.iter().enumerate().map(|(i,s)|{
                                        let name=model_name(s);let points=buckets.iter().enumerate().map(|(j,b)|format!("{},{}",x(j),239.0-model_bucket_count(s,value(b,"start_at")) as f64/top*232.0)).collect::<Vec<_>>().join(" ");let y=239.0-model_bucket_count(s,value(&buckets[0],"start_at")) as f64/top*232.0;let opacity=if focus.is_none_or(|f|f==i){"1"}else{"0.15"};
                                        view!{<g opacity=opacity aria-label=name.clone()><polyline class="usage-model-line" data-model=name.clone() points=points fill="none" stroke=series_color(i) stroke-width="2" stroke-dasharray=series_dash(i) vector-effect="non-scaling-stroke"/><Show when=move||n==1><circle cx="480" cy=y.to_string() r="3" fill=series_color(i)/></Show></g>}
                                    }).collect_view()}
                                    {buckets.iter().enumerate().map(|(i,b)|{
                                        let label=bucket_label(b,&data);let lx=i as f64*hit_width;
                                        view!{<g class="usage-chart-bucket usage-model-bucket" role="button" tabindex="0" aria-label=label.clone() on:mouseenter=move |_|selected.set(Some(i)) on:mouseleave=move |_|selected.set(None) on:focus=move |_|selected.set(Some(i)) on:blur=move |_|selected.set(None) on:click=move |_|selected.set(Some(i)) on:keydown=move|ev:leptos::ev::KeyboardEvent|{if matches!(ev.key().as_str(),"Enter"|" "){ev.prevent_default();selected.set(Some(i));}}><title>{label.clone()}</title><rect x=lx.to_string() y="0" width=hit_width.to_string() height="240" class="usage-bar-hit" aria-hidden="true"/><line x1=(lx+hit_width/2.0).to_string() x2=(lx+hit_width/2.0).to_string() y1="1" y2="239" class="usage-bucket-guide" aria-hidden="true"/></g>}
                                    }).collect_view()}
                                </svg><div class="usage-x-axis" aria-hidden="true"><span>{first}</span><span>{middle}</span><span>{last}</span></div></div>
                            </div>
                            <div class="usage-model-legend" aria-label="选择单个模型查看趋势">{series.iter().enumerate().map(|(i,s)|{
                                let name=model_name(s);let calls=count(s,"ledger_count");let description=format!("{}：{}次调用。点击单独查看，再次点击恢复全部。",name,calls);
                                view!{<button type="button" title=description aria-pressed=move||(focused.get()==Some(i)).to_string() class:active=move||focused.get()==Some(i) on:click=move |_|focused.update(|f|*f=if *f==Some(i){None}else{Some(i)})><i style=format!("border-color:{};border-top-style:{}",series_color(i),if i==0{"solid"}else{"dashed"}) aria-hidden="true"></i><span>{name.clone()}</span><small>{calls}</small></button>}
                            }).collect_view()}</div>
                            <p class="usage-bucket-detail usage-model-bucket-detail" role="status" aria-live="polite">{move||{
                                let a=analytics.get();let bs=rows(&a,"buckets");let ss=rows(&a,"model_series");
                                selected.get().and_then(|i|bs.get(i).cloned()).map(|b|{
                                    let details=ss.iter().enumerate().filter(|(i,_)|focused.get().is_none_or(|f|f==*i)).map(|(_,s)|format!("{} {}次",model_name(s),fmt::num(model_bucket_count(s,value(&b,"start_at"))))).collect::<Vec<_>>().join("；");
                                    format!("{} · {}",bucket_label(&b,&a),details)
                                }).unwrap_or_else(||"悬停或聚焦时间点，查看各模型调用次数；点击图例可单独查看模型。".into())
                            }}</p>
                        </div>}.into_any()
                    }
                }
            }}
        </section>
    }
}
