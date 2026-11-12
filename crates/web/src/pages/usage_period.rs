//! Shared browser-local calendar ranges for both ledger views.
use leptos::prelude::*;

pub fn period_query(period: &str, start: &str, end: &str) -> Result<String, String> {
    if period == "all" { return Ok(String::new()); }
    let (from, until) = if period == "custom" {
        if start.is_empty() || end.is_empty() { return Err("请填写开始时间和结束时间。".into()); }
        (js_sys::Date::new(&start.into()).get_time(), js_sys::Date::new(&end.into()).get_time())
    } else {
        let now = js_sys::Date::new_0();
        let year = now.get_full_year(); let month = now.get_month() as i32; let day = now.get_date() as i32;
        let date = |m, d| js_sys::Date::new_with_year_month_day_hr_min_sec(year, m, d, 0, 0, 0).get_time();
        match period {
            "today" => (date(month, day), date(month, day + 1)),
            "week" => { let monday = day - ((now.get_day() + 6) % 7) as i32; (date(month, monday), date(month, monday + 7)) },
            "month" => (date(month, 1), date(month + 1, 1)),
            _ => return Err("请选择有效时间范围。".into()),
        }
    };
    if !from.is_finite() || !until.is_finite() || from < 0.0 || from >= until {
        return Err("时间范围无效：结束时间必须晚于开始时间。".into());
    }
    Ok(format!("&start_at={}&end_at={}", (from / 1000.0) as i64, (until / 1000.0) as i64))
}
pub fn default_period_query() -> String { period_query("today", "", "").unwrap_or_default() }

#[component]
pub fn UsagePeriod(period: RwSignal<String>, start: RwSignal<String>, end: RwSignal<String>) -> impl IntoView {
    view! {
        <div class="billing-field"><label for="usage-period">"时间范围"</label>
            <select id="usage-period" prop:value=period on:change=move |e|period.set(event_target_value(&e))>
                <option value="today">"今天"</option><option value="week">"本周"</option><option value="month">"本月"</option><option value="custom">"自定义"</option><option value="all">"全部时间"</option>
            </select>
        </div>
        {move || (period.get() == "custom").then(|| view! {
            <div class="billing-field"><label for="usage-start">"开始时间（含）"</label><input id="usage-start" type="datetime-local" required step="1" prop:value=start aria-describedby="usage-time-help" on:input=move |e| start.set(event_target_value(&e))/></div>
            <div class="billing-field"><label for="usage-end">"结束时间（不含）"</label><input id="usage-end" type="datetime-local" required step="1" prop:value=end aria-describedby="usage-time-help" on:input=move |e| end.set(event_target_value(&e))/></div>
        })}
        <p id="usage-time-help" class="muted small">"按请求发起时间查询，使用浏览器本地时区；本周从周一开始。自定义范围包含开始、不含结束。"</p>
    }
}

/// Format the actual submitted boundaries rather than the editable draft.
pub fn applied_period(query: &str) -> String {
    let bound = |key: &str| query.split('&').find_map(|part| part.strip_prefix(key)).and_then(|s|s.parse::<i64>().ok());
    let label = |seconds: i64| {
        let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(seconds as f64 * 1000.0));
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}",d.get_full_year(),d.get_month()+1,d.get_date(),d.get_hours(),d.get_minutes(),d.get_seconds())
    };
    match (bound("start_at="), bound("end_at=")) {
        (Some(start), Some(end)) => format!("{} 至 {}（不含）", label(start), label(end)),
        _ => "全部时间".into(),
    }
}
