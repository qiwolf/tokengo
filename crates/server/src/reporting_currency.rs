//! Reference-currency display only. Ledger amounts, snapshots and settlement stay unchanged.
use crate::{db, SharedState};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

pub const SOURCE_URL:&str="https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";
const SETTING:&str="reporting_fx:USD:CNY";
const NANO:i128=1_000_000_000;
const CACHE_MS:i64=6*60*60*1000;
const RETRY_MS:i64=5*60*1000;
const MAX_BODY:usize=128*1024;

#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
pub struct Quote {
    pub rate_nano:i64,
    pub date:String,
    pub source_url:String,
    pub fetched_at_ms:i64,
    pub stale:bool,
    pub status:String,
}
impl Quote {
    fn at(&self,now:i64)->Self {
        let mut q=self.clone();q.stale=now.saturating_sub(q.fetched_at_ms)>=CACHE_MS;
        q.status=if q.stale {"stale"}else{"ok"}.into();q
    }
    fn valid(&self,now:i64)->bool {
        self.rate_nano>0 && valid_date(&self.date) && self.source_url==SOURCE_URL && self.fetched_at_ms>0 && self.fetched_at_ms<=now
    }
}
#[derive(Default)]
pub(crate) struct Cache {last_good:Option<Quote>,next_attempt_ms:i64}
impl Cache {
    pub(crate) fn from_settings(c:&Connection)->Self {
        let now=db::now_ms();
        let last_good=db::settings::get(c,SETTING).ok().flatten().filter(|raw|raw.len()<8192)
            .and_then(|raw|serde_json::from_str::<Quote>(&raw).ok()).filter(|q|q.valid(now));
        let next_attempt_ms=last_good.as_ref().map(|q|q.fetched_at_ms.saturating_add(CACHE_MS)).unwrap_or(0);
        Self{last_good,next_attempt_ms}
    }
}
/// One process-wide refresh at a time; the database guard never crosses network await.
pub async fn quote(state:&SharedState)->Option<Quote> {
    let mut cache=state.reporting_fx.lock().await;
    let now=db::now_ms();
    if now<cache.next_attempt_ms {return cache.last_good.as_ref().map(|q|q.at(now));}
    if let Some(q)=fetch().await {
        cache.next_attempt_ms=q.fetched_at_ms.saturating_add(CACHE_MS);cache.last_good=Some(q.clone());
        if let Ok(raw)=serde_json::to_string(&q) {
            // Persist only the successful public reference quote, never a synthetic fallback.
            let _=db::settings::set(&state.conn(),SETTING,&raw);
        }
    }else{cache.next_attempt_ms=db::now_ms().saturating_add(RETRY_MS);}
    cache.last_good.as_ref().map(|q|q.at(db::now_ms()))
}
async fn fetch()->Option<Quote> {
    let client=reqwest::Client::builder().timeout(Duration::from_secs(4)).redirect(reqwest::redirect::Policy::none()).user_agent("TokenGo reference currency reporting").build().ok()?;
    let mut response=client.get(SOURCE_URL).header(reqwest::header::ACCEPT,"application/xml,text/xml").send().await.ok()?.error_for_status().ok()?;
    if !response.status().is_success(){return None;}
    if response.content_length().is_some_and(|len|len>MAX_BODY as u64){return None;}
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await.ok()? {
        if bytes.len().checked_add(chunk.len())?>MAX_BODY{return None;}
        bytes.extend_from_slice(&chunk);
    }
    parse_quote(std::str::from_utf8(&bytes).ok()?,db::now_ms())
}
fn valid_date(date:&str)->bool {
    let b=date.as_bytes();
    if b.len()!=10||b[4]!=b'-'||b[7]!=b'-'||!b.iter().enumerate().all(|(i,v)|i==4||i==7||v.is_ascii_digit()){return false;}
    let Ok(year)=date[..4].parse::<u32>() else{return false};let Ok(month)=date[5..7].parse::<u32>() else{return false};let Ok(day)=date[8..].parse::<u32>() else{return false};
    let leap=year%4==0&&(year%100!=0||year%400==0);
    let max=match month{1|3|5|7|8|10|12=>31,4|6|9|11=>30,2=>if leap{29}else{28},_=>0};
    year>0&&day>0&&day<=max
}
fn decimal_nano(value:&str)->Option<i64> {
    let mut parts=value.split('.');let whole=parts.next()?;let fraction=parts.next().unwrap_or("");
    if whole.is_empty()||!whole.bytes().all(|b|b.is_ascii_digit())||fraction.len()>9||!fraction.bytes().all(|b|b.is_ascii_digit())||parts.next().is_some()||value.ends_with('.') {return None;}
    let a=whole.parse::<i64>().ok()?.checked_mul(NANO as i64)?;
    let b=if fraction.is_empty(){0}else{fraction.parse::<i64>().ok()?.checked_mul(10_i64.checked_pow((9-fraction.len()) as u32)?)?};
    a.checked_add(b).filter(|n|*n>0)
}
fn attrs(mut tag:&str)->Option<BTreeMap<&str,&str>> {
    let mut out=BTreeMap::new();
    loop {
        tag=tag.trim_start();
        if tag.is_empty()||tag=="/" {return Some(out);}
        let end=tag.find(|c:char|c=='='||c.is_whitespace())?;let name=&tag[..end];
        if name.is_empty()||!matches!(name,"time"|"currency"|"rate"){return None;}
        tag=tag[end..].trim_start().strip_prefix('=')?.trim_start();
        let quote=tag.as_bytes().first().copied()?;if quote!=b'\''&&quote!=b'"'{return None;}
        tag=&tag[1..];let end=tag.find(quote as char)?;let value=&tag[..end];
        if out.insert(name,value).is_some(){return None;}
        tag=&tag[end+1..];
        if !tag.is_empty()&&!tag.starts_with(char::is_whitespace)&&tag!="/" {return None;}
    }
}
fn parse_quote(xml:&str,fetched_at_ms:i64)->Option<Quote> {
    if xml.len()>MAX_BODY||xml.contains("<!DOCTYPE")||xml.contains("<!ENTITY"){return None;}
    // Ignore comments so quoted-looking Cube text cannot be mistaken for a rate.
    let mut clean=String::new();let mut rest=xml;
    while let Some(start)=rest.find("<!--") {clean.push_str(&rest[..start]);rest=&rest[start+4..];let end=rest.find("-->")?;rest=&rest[end+3..];}
    clean.push_str(rest);let mut rest=clean.as_str();let mut date=None;let mut rates=BTreeMap::new();let mut seen=BTreeSet::new();
    while let Some(start)=rest.find("<Cube") {
        rest=&rest[start+5..];
        if !rest.starts_with(|c:char|c.is_whitespace()||c=='>'||c=='/'){return None;}
        let end=rest.find('>')?;let a=attrs(&rest[..end])?;rest=&rest[end+1..];
        if let Some(value)=a.get("time") {
            if a.len()!=1||date.is_some()||!valid_date(value){return None;}
            date=Some((*value).to_string());
        }else if let Some(currency)=a.get("currency") {
            if date.is_none()||a.len()!=2||currency.len()!=3||!currency.bytes().all(|b|b.is_ascii_uppercase())||!seen.insert(*currency){return None;}
            let rate=decimal_nano(a.get("rate")?)?;rates.insert(*currency,rate);
        }else if !a.is_empty(){return None;}
    }
    let usd=i128::from(*rates.get("USD")?);let cny=i128::from(*rates.get("CNY")?);
    let rate_nano=i64::try_from(cny.checked_mul(NANO)?.checked_add(usd/2)?/usd).ok()?.max(0);
    if rate_nano==0{return None;}
    Some(Quote{rate_nano,date:date?,source_url:SOURCE_URL.into(),fetched_at_ms,stale:false,status:"ok".into()})
}
fn converted(amount:i64,currency:&str,quote:Option<&Quote>)->Option<i64> {
    match currency {
        "CNY"=>Some(amount),
        "USD" if amount==0=>Some(0),
        "USD"=>{
            let q=quote.filter(|q|q.rate_nano>0)?;let product=i128::from(amount).checked_mul(i128::from(q.rate_nano))?;
            let magnitude=product.checked_abs()?.checked_add(NANO/2)?/NANO;
            i64::try_from(if product<0{-magnitude}else{magnitude}).ok()
        },
        _=>None,
    }
}
/// Avoid a network dependency when only quantities, CNY, unknown amounts or zero USD are present.
pub fn needs_quote(value:&Value)->bool {
    match value {
        Value::Array(rows)=>rows.iter().any(needs_quote),
        Value::Object(row)=>{
            let usd=row.get("currency").and_then(Value::as_str)==Some("USD");
            usd&&["known_debit_micro","debit_micro","observed_cost_micro","reserved_micro"].iter().any(|key|row.get(*key).and_then(Value::as_i64).is_some_and(|n|n!=0))||row.values().any(needs_quote)
        },
        _=>false,
    }
}
fn metadata(quote:Option<&Quote>,required:bool)->Value {
    if let Some(q)=quote {serde_json::to_value(q).unwrap_or(Value::Null)}
    else{json!({"status":if required{"unavailable"}else{"not_required"},"rate_nano":null,"date":null,"source_url":SOURCE_URL,"fetched_at_ms":null,"stale":false})}
}
/// Adds only reporting fields. All original amounts and currencies are retained verbatim.
pub fn apply(value:&mut Value,quote:Option<&Quote>) {
    let meta=metadata(quote,needs_quote(value));
    fn walk(value:&mut Value,quote:Option<&Quote>,meta:&Value,attach:bool) {
        match value {
            Value::Array(rows)=>for row in rows {walk(row,quote,meta,false)},
            Value::Object(row)=>{
                for (key,child) in row.iter_mut(){walk(child,quote,meta,key=="summary"||key=="analytics");}
                if let Some(amounts)=row.get("amounts_by_currency").and_then(Value::as_array) {
                    let total=amounts.iter().try_fold(0_i64,|sum,amount|sum.checked_add(converted(amount["known_debit_micro"].as_i64()?,amount["currency"].as_str()?,quote)?));
                    row.insert("known_debit_cny_micro".into(),json!(total));row.insert("display_currency".into(),json!("CNY"));
                }
                if let Some(currency)=row.get("currency").and_then(Value::as_str).map(str::to_string) {
                    for (raw,display) in [("debit_micro","display_debit_micro"),("observed_cost_micro","display_observed_cost_micro"),("reserved_micro","display_reserved_micro")] {
                        if let Some(amount)=row.get(raw){let n=amount.as_i64().and_then(|n|converted(n,&currency,quote));row.insert(display.into(),json!(n));row.insert("display_currency".into(),json!("CNY"));}
                    }
                }
                if attach{row.insert("reporting_fx".into(),meta.clone());}
            },
            _=>{},
        }
    }
    walk(value,quote,&meta,true);
}

#[cfg(test)]
mod tests {
    use super::*;
    const XML:&str="<gesmes:Envelope><Cube><Cube time='2026-11-12'><Cube currency='USD' rate='1.1081'/><Cube currency='CNY' rate='7.3891'/></Cube></Cube></gesmes:Envelope>";
    fn q()->Quote {parse_quote(XML,db::now_ms()).unwrap()}
    #[test] fn ecb_cross_rate_uses_integer_nanos_and_calendar_date(){let q=q();assert_eq!(q.date,"2026-11-12");assert_eq!(q.rate_nano,6_668_260_987);assert_eq!(converted(1_000_000,"USD",Some(&q)),Some(6_668_261));assert!(valid_date("2024-02-29"));assert!(!valid_date("2026-02-29"));assert!(!valid_date("2026-13-01"));}
    #[test] fn invalid_duplicate_nonpositive_and_oversized_rates_are_rejected(){
        for xml in [XML.replace("1.1081","0"),XML.replace("1.1081","-1"),XML.replace("1.1081","1e2"),XML.replace("1.1081","999999999999999999999999999"),XML.replace("2026-11-12","2026-02-30"),XML.replace(" currency='CNY'"," currency='USD'"),XML.replace("rate='1.1081'","rate='1.1081' rate='2'"),XML.replace("</Cube></Cube></gesmes:Envelope>","<Cube time='2026-11-13'/></Cube></Cube></gesmes:Envelope>")] {assert!(parse_quote(&xml,1).is_none(),"{xml}");}
        assert!(parse_quote(&" ".repeat(MAX_BODY+1),1).is_none());
    }
    #[test] fn cache_reads_last_good_without_rewriting_money(){
        let c=Connection::open_in_memory().unwrap();c.execute("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT)",[]).unwrap();let mut old=q();old.fetched_at_ms=db::now_ms()-CACHE_MS-1;db::settings::set(&c,SETTING,&serde_json::to_string(&old).unwrap()).unwrap();let cached=Cache::from_settings(&c);let restored=cached.last_good.unwrap().at(db::now_ms());assert!(restored.stale);assert_eq!(restored.status,"stale");assert_eq!(restored.rate_nano,old.rate_nano);
        db::settings::set(&c,SETTING,"not-json").unwrap();assert!(Cache::from_settings(&c).last_good.is_none());
    }
    #[test] fn mixed_summary_and_entries_get_reference_fields_but_original_values_survive(){
        let mut v=json!({"usage":{"summary":{"currency":"all","known_debit_micro":null,"total_tokens":123,"amounts_by_currency":[{"currency":"USD","known_debit_micro":1_000_000},{"currency":"CNY","known_debit_micro":12}]},"analytics":{"buckets":[{"amounts_by_currency":[{"currency":"USD","known_debit_micro":1}]}]},"entries":[{"currency":"USD","debit_micro":1_000_000,"observed_cost_micro":null,"reserved_micro":2}]}});assert!(needs_quote(&v));apply(&mut v,Some(&q()));assert_eq!(v["usage"]["summary"]["known_debit_cny_micro"],6_668_273);assert_eq!(v["usage"]["summary"]["known_debit_micro"],Value::Null);assert_eq!(v["usage"]["summary"]["total_tokens"],123);assert_eq!(v["usage"]["entries"][0]["currency"],"USD");assert_eq!(v["usage"]["entries"][0]["debit_micro"],1_000_000);assert_eq!(v["usage"]["entries"][0]["display_debit_micro"],6_668_261);assert_eq!(v["usage"]["entries"][0]["display_observed_cost_micro"],Value::Null);assert_eq!(v["usage"]["analytics"]["buckets"][0]["known_debit_cny_micro"],7);assert_eq!(v["usage"]["summary"]["reporting_fx"]["status"],"ok");assert_eq!(v["usage"]["analytics"]["reporting_fx"]["date"],"2026-11-12");
    }
    #[test] fn unavailable_quote_keeps_usage_and_never_turns_unknown_usd_into_zero(){
        let mut v=json!({"summary":{"total_tokens":99,"amounts_by_currency":[{"currency":"CNY","known_debit_micro":3},{"currency":"USD","known_debit_micro":1}]}});apply(&mut v,None);assert_eq!(v["summary"]["known_debit_cny_micro"],Value::Null);assert_eq!(v["summary"]["total_tokens"],99);assert_eq!(v["reporting_fx"]["status"],"unavailable");let mut zero=json!({"summary":{"amounts_by_currency":[{"currency":"USD","known_debit_micro":0},{"currency":"CNY","known_debit_micro":3}]}});assert!(!needs_quote(&zero));apply(&mut zero,None);assert_eq!(zero["summary"]["known_debit_cny_micro"],3);assert_eq!(zero["reporting_fx"]["status"],"not_required");assert_eq!(converted(i64::MAX,"USD",Some(&q())),None);assert_eq!(converted(5,"EUR",Some(&q())),None);
    }
}
