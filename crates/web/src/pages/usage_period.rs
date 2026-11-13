//! Browser-local date ranges with optional clock boundaries. Empty means today/all day.
use leptos::prelude::*;

fn date_parts(value:&str)->Result<(u32,i32,i32),String> {
    let bits=value.split('-').collect::<Vec<_>>();
    if bits.len()!=3||bits[0].len()!=4||bits[1].len()!=2||bits[2].len()!=2{return Err("日期格式应为年-月-日。".into());}
    let year=bits[0].parse::<u32>().map_err(|_|"日期无效。")?;
    let month=bits[1].parse::<i32>().map_err(|_|"日期无效。")?;
    let day=bits[2].parse::<i32>().map_err(|_|"日期无效。")?;
    let leap=year%4==0&&(year%100!=0||year%400==0);
    let days=match month {1|3|5|7|8|10|12=>31,4|6|9|11=>30,2=>if leap{29}else{28},_=>0};
    if year<1970||day<1||day>days{return Err("请选择有效日期。".into());} Ok((year,month-1,day))
}
fn clock_parts(value:&str)->Result<(i32,i32,i32),String> {
    if value.is_empty(){return Ok((0,0,0));}
    let bits=value.split(':').collect::<Vec<_>>();
    if !(2..=3).contains(&bits.len())||bits.iter().any(|s|s.len()!=2||!s.bytes().all(|b|b.is_ascii_digit())){return Err("时刻格式应为时:分。".into());}
    let h=bits[0].parse::<i32>().map_err(|_|"时刻无效。")?;let m=bits[1].parse::<i32>().map_err(|_|"时刻无效。")?;let s=if bits.len()==3{bits[2].parse::<i32>().map_err(|_|"时刻无效。")?}else{0};
    if h>23||m>59||s>59{return Err("请选择有效时刻。".into());}Ok((h,m,s))
}
fn chosen_dates<'a>(today:&'a str,start:&'a str,end:&'a str)->Result<(&'a str,&'a str),String> {
    let from=if !start.is_empty(){start}else if !end.is_empty(){end}else{today};
    let until=if !end.is_empty(){end}else{from};date_parts(from)?;date_parts(until)?;Ok((from,until))
}
pub fn period_query(period:&str,start:&str,end:&str,start_time:&str,end_time:&str)->Result<String,String> {
    if period=="all"{return Ok(String::new());}
    let now=js_sys::Date::new_0();let year=now.get_full_year();let month=now.get_month() as i32;let day=now.get_date() as i32;
    let today=format!("{year:04}-{:02}-{day:02}",month+1);
    let (from,until)=if period=="custom" {
        let (a,b)=chosen_dates(&today,start,end)?;(date_parts(a)?,date_parts(b)?)
    }else{match period {
        "today"=>((year,month,day),(year,month,day)),
        "week"=>((year,month,day-((now.get_day()+6)%7) as i32),(year,month,day)),
        "month"=>((year,month,1),(year,month,day)),
        _=>return Err("请选择有效时间范围。".into()),
    }};
    let (ah,am,asec)=clock_parts(start_time)?;let (bh,bm,bsec)=clock_parts(end_time)?;
    let lower=js_sys::Date::new_with_year_month_day_hr_min_sec(from.0,from.1,from.2,ah,am,asec).get_time();
    // An omitted end clock includes the selected end date's whole day.
    let upper=js_sys::Date::new_with_year_month_day_hr_min_sec(until.0,until.1,until.2+i32::from(end_time.is_empty()),bh,bm,bsec).get_time();
    if !lower.is_finite()||!upper.is_finite()||lower<0.0||lower>=upper{return Err("结束日期和时刻必须晚于开始日期和时刻。".into());}
    Ok(format!("&start_at={}&end_at={}",(lower/1000.0) as i64,(upper/1000.0) as i64))
}
pub fn default_period_query()->String{period_query("today","","","","").unwrap_or_default()}

#[component]
pub fn UsagePeriod(period:RwSignal<String>,start:RwSignal<String>,end:RwSignal<String>,start_time:RwSignal<String>,end_time:RwSignal<String>,#[prop(default=false)] compact:bool)->impl IntoView {
    view!{
      <div class="billing-field usage-period-choice"><label for="usage-period">"时间范围"</label><select id="usage-period" prop:value=period on:change=move|e|{let next=event_target_value(&e);if next!="custom"{start.set(String::new());end.set(String::new());start_time.set(String::new());end_time.set(String::new());}period.set(next);}>
        <option value="today">"今天"</option><option value="week">"本周"</option><option value="month">"本月"</option><option value="custom">"自定义日期和时间"</option><option value="all">"全部时间"</option>
      </select></div>
      {move||(period.get()=="custom").then(||view!{
        <div class="usage-custom-range" role="group" aria-label="自定义日期和时间">
          <div class="billing-field"><label for="usage-start">"开始日期"</label><input id="usage-start" type="date" prop:value=start aria-describedby="usage-time-help" on:input=move|e|start.set(event_target_value(&e))/></div>
          <div class="billing-field"><label for="usage-start-clock">"开始时刻（可选）"</label><input id="usage-start-clock" type="time" step="60" prop:value=start_time aria-describedby="usage-time-help" on:input=move|e|start_time.set(event_target_value(&e))/></div>
          <span class="usage-range-to" aria-hidden="true">"至"</span>
          <div class="billing-field"><label for="usage-end">"结束日期"</label><input id="usage-end" type="date" prop:value=end aria-describedby="usage-time-help" on:input=move|e|end.set(event_target_value(&e))/></div>
          <div class="billing-field"><label for="usage-end-clock">"结束时刻（可选）"</label><input id="usage-end-clock" type="time" step="60" prop:value=end_time aria-describedby="usage-time-help" on:input=move|e|end_time.set(event_target_value(&e))/></div>
          <p id="usage-time-help" class="muted small usage-range-help">"日期不填按今天；时刻不填按全天，包含结束日期。填写时刻后按起止日期和时刻查询。"</p>
        </div>
      })}
      <span class=if compact{"usage-time-sr"}else{"muted small"}>"按请求发起时间和浏览器本地时区查询；本周、本月统计至今天。"</span>
    }
}

/// Format actual submitted boundaries, rather than the editable draft.
pub fn applied_period(query:&str)->String {
    let bound=|key:&str|query.split('&').find_map(|part|part.strip_prefix(key)).and_then(|s|s.parse::<i64>().ok());
    let date=|seconds:i64|js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(seconds as f64*1000.0));
    let day=|d:&js_sys::Date|format!("{:04}-{:02}-{:02}",d.get_full_year(),d.get_month()+1,d.get_date());
    let clock=|d:&js_sys::Date|format!("{:02}:{:02}:{:02}",d.get_hours(),d.get_minutes(),d.get_seconds());
    match(bound("start_at="),bound("end_at=")){
      (Some(a),Some(b))=>{let start=date(a);let end=date(b);if clock(&start)=="00:00:00"&&clock(&end)=="00:00:00"{let last=date(b-1);if day(&start)==day(&last){format!("{} 全天",day(&start))}else{format!("{} 至 {} · 全天",day(&start),day(&last))}}else{format!("{} {} 至 {} {}（不含）",day(&start),clock(&start),day(&end),clock(&end))}},
      _=>"全部时间".into(),
    }
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn optional_dates_default_to_today_and_selected_single_day(){assert_eq!(chosen_dates("2026-11-13","","").unwrap(),("2026-11-13","2026-11-13"));assert_eq!(chosen_dates("2026-11-13","2026-11-01","").unwrap(),("2026-11-01","2026-11-01"));assert_eq!(chosen_dates("2026-11-13","","2026-11-03").unwrap(),("2026-11-03","2026-11-03"));}
 #[test]fn dates_and_optional_clocks_validate_real_calendar_values(){assert!(date_parts("2025-02-29").is_err());assert!(date_parts("2024-02-29").is_ok());assert!(date_parts("2026-11-31").is_err());assert_eq!(clock_parts("").unwrap(),(0,0,0));assert_eq!(clock_parts("09:30").unwrap(),(9,30,0));assert!(clock_parts("24:00").is_err());}
}
