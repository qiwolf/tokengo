//! 时间与数字的显示。**不引 chrono** —— 面板只需要"什么时候"和"多久"，
//! 自己算比多一个依赖划算，而且这些函数是纯的，能在本机（非 wasm）跑单测。

/// 东八区偏移。面板是自用的，不做时区切换。
const CST_OFFSET_MS: i64 = 8 * 3600 * 1000;

/// 毫秒时间戳 → `2026-10-17 03:21:07`（东八区）。
pub fn ts(ms: i64) -> String {
    let local = ms + CST_OFFSET_MS;
    let days = local.div_euclid(86_400_000);
    let rem = local.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3_600_000, (rem / 60_000) % 60, (rem / 1000) % 60);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
}

/// 毫秒时间戳 → `10-17 03:21`（表格里省年份）。
pub fn ts_short(ms: i64) -> String {
    let s = ts(ms);
    s.get(5..).unwrap_or(&s).to_string()
}

/// `3 分钟前` / `2 小时前` / `5 天前`；未来时间会说"还有 …"。
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    let d = now_ms - then_ms;
    if d < 0 {
        return format!("还有 {}", span(-d));
    }
    format!("{}前", span(d))
}

/// 一段时长的人话：`45 秒` / `3 分钟` / `2 小时 5 分钟` / `3 天`。
pub fn span(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    if secs < 60 {
        return format!("{secs} 秒");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins} 分钟");
    }
    let hours = mins / 60;
    if hours < 24 {
        let r = mins % 60;
        return if r == 0 {
            format!("{hours} 小时")
        } else {
            format!("{hours} 小时 {r} 分钟")
        };
    }
    format!("{} 天", hours / 24)
}

/// 耗时：`340 毫秒` / `1.2 秒`。
pub fn dur(ms: i64) -> String {
    if ms < 1000 {
        format!("{ms} 毫秒")
    } else {
        format!("{:.1} 秒", ms as f64 / 1000.0)
    }
}

/// 千分位。
pub fn num(v: i64) -> String {
    let s = v.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if v < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Compact headline only; exact recorded usage remains in statistics details.
pub fn compact_tokens(n: i64) -> String {
    if n >= 100_000_000 {
        format!("{:.2}亿", n as f64 / 100_000_000.0)
    } else if n >= 10_000 {
        format!("{:.0}万", n as f64 / 10_000.0)
    } else {
        num(n)
    }
}

/// 上下文长度的人话刻度：`8,192` → `8K`、`163840` → `164K`、`1_000_000` → `1M`。
///
/// 上游报的是 token 数（有的写得很大），面板上是一行小标签，别塞六个数字进去。
pub fn ctx(n: i64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        let s = format!("{m:.1}");
        return format!("{}M", s.trim_end_matches(".0"));
    }
    format!("{}K", (n + 500) / 1000)
}

/// 百分数，一位小数；分母是 0 时回 `—`（不假装 0%）。
pub fn pct(part: i64, whole: i64) -> String {
    if whole <= 0 {
        return "—".into();
    }
    format!("{:.1}%", part as f64 * 100.0 / whole as f64)
}

/// 天数 → 年月日（Howard Hinnant 的 civil_from_days，UTC 天数）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_shown_in_cst() {
        // 2026-10-17 03:21:07 CST = 2026-10-16 19:21:07 UTC
        let ms = 1_792_178_467_000i64;
        assert_eq!(ts(ms), "2026-10-17 03:21:07");
        assert_eq!(ts_short(ms), "10-17 03:21:07");
    }

    #[test]
    fn epochs_and_month_boundaries_line_up() {
        assert_eq!(ts(0), "1970-01-01 08:00:00", "UTC 0 点在东八区是早上 8 点");
        // 2026-03-01 00:00:00 CST（非闰年，跨 2 月）
        assert_eq!(ts(1_772_294_400_000), "2026-03-01 00:00:00");
    }

    #[test]
    fn durations_read_like_a_person_wrote_them() {
        assert_eq!(span(45_000), "45 秒");
        assert_eq!(span(3 * 60_000), "3 分钟");
        assert_eq!(span(125 * 60_000), "2 小时 5 分钟");
        assert_eq!(span(72 * 3600_000), "3 天");
        assert_eq!(dur(340), "340 毫秒");
        assert_eq!(dur(1240), "1.2 秒");
    }

    #[test]
    fn ago_handles_both_directions() {
        assert_eq!(ago(100_000, 40_000), "1 分钟前");
        assert_eq!(ago(40_000, 100_000), "还有 1 分钟");
    }

    #[test]
    fn percentages_never_lie_about_zero() {
        assert_eq!(pct(0, 0), "—", "没请求过的时候不能显示 0%");
        assert_eq!(pct(2, 3), "66.7%");
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(num(0), "0");
        assert_eq!(num(999), "999");
        assert_eq!(num(1000), "1,000");
        assert_eq!(num(1234567), "1,234,567");
        assert_eq!(num(-1234), "-1,234");
    }

    #[test]
    fn token_headlines_are_compact_but_small_counts_stay_exact() {
        for (n, expected) in [(0,"0"),(9999,"9,999"),(10000,"1万"),(91_481_699,"9148万"),(100_000_000,"1.00亿"),(i64::MAX,"92233720368.55亿")] {
            assert_eq!(compact_tokens(n), expected);
        }
    }

    #[test]
    fn context_lengths_read_as_a_short_badge() {
        assert_eq!(ctx(512), "512");
        assert_eq!(ctx(8192), "8K");
        assert_eq!(ctx(163_840), "164K");
        assert_eq!(ctx(1_000_000), "1M");
        assert_eq!(ctx(1_048_576), "1M", "一位小数，整数不留 .0");
    }
}

/// Exact decimal USD projection from integer microUSD, without floating-point loss.
pub fn usd_input(n:i64)->String {let n=i128::from(n);let sign=if n<0{"-"}else{""};let n=n.abs();let mut s=format!("{sign}{}.{:06}",n/1_000_000,n%1_000_000);while s.ends_with('0'){s.pop();}if s.ends_with('.') {s.pop();}s}

/// Missing currency identifies pre-currency snapshots, which were USD.
pub fn legacy_currency() -> String {"USD".into()}
pub fn currency_symbol(currency:&str)->&str {match currency {"CNY"=>"¥","USD"=>"$",_=>""}}
pub fn currency_of(row:&serde_json::Value)->&str {row["currency"].as_str().unwrap_or("USD")}
pub fn money(micro:Option<i64>,currency:&str)->String {
    micro.map(|n|format!("{} {}",usd_input(n),currency)).unwrap_or_else(||"未知".into())
}
pub fn unit_label(unit:&str)->&'static str {match unit {"token"=>"Token","request"=>"按次","second"=>"按秒","minute"=>"按分钟","hour"=>"按小时","character"=>"按字符","byte"=>"按字节","image"=>"按张","video"=>"按视频",_=>"未知单位"}}
pub fn quantity(unit:&str,quantity_milli:Option<i64>)->String {
    let Some(q)=quantity_milli else{return "用量未知".into()};
    let mut amount=format!("{}.{:03}",q/1000,q%1000);while amount.ends_with('0'){amount.pop();}if amount.ends_with('.') {amount.pop();}
    let label=match unit {"second"|"minute"|"hour"=>"秒音频","character"=>"字符","byte"=>"字节","image"=>"张","video"=>"个视频",_=>"单位"};
    format!("{amount} {label}")
}
#[cfg(test)] mod currency_quantity_tests {
 use super::*;
 #[test] fn money_does_not_change_historical_currency(){assert_eq!(money(Some(1234567),"CNY"),"1.234567 CNY");assert_eq!(money(Some(1234567),"USD"),"1.234567 USD");assert_eq!(currency_of(&serde_json::json!({})),"USD");}
 #[test] fn duration_quantity_is_audio_time_even_when_rate_is_per_hour(){assert_eq!(quantity("hour",Some(1500)),"1.5 秒音频");assert_eq!(quantity("character",Some(3000)),"3 字符");assert_eq!(quantity("second",None),"用量未知");}
}
