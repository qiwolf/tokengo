//! Typed, lossless policy controls shared by administrators and key owners.
use leptos::prelude::*;
use serde_json::{Value, json};

pub fn default_policy() -> Value {
    serde_json::from_str(crate::admin_api::POLICY).unwrap()
}
pub fn validate(p: &Value) -> Result<(), String> {
    let Some(o) = p.as_object() else {
        return Err("策略数据无效，请重新读取。".into());
    };
    if o.keys().any(|k| {
        !matches!(
            k.as_str(),
            "allowed_models"
                | "allowed_channels"
                | "allow_paid"
                | "admin_access"
                | "rpm"
                | "tpm"
                | "max_concurrent"
        )
    }) {
        return Err("策略含当前页面不认识的字段，已阻止有损保存；请升级或刷新。".into());
    }
    for k in ["rpm", "tpm", "max_concurrent"] {
        if !p[k].is_null()
            && p[k].as_u64().is_none_or(|n| {
                n == 0
                    || n > if k == "tpm" {
                        i64::MAX as u64
                    } else {
                        u32::MAX as u64
                    }
            })
        {
            return Err("限流必须留空或填写正整数。".into());
        }
    }
    for k in ["allow_paid", "admin_access"] {
        if p.get(k).is_some_and(|v| !v.is_boolean()) {
            return Err("权限开关数据无效。".into());
        }
    }
    if !p["allowed_models"].is_null()
        && p["allowed_models"]
            .as_array()
            .is_none_or(|a| a.len() > 256 || a.iter().any(|v| v.as_str().is_none_or(str::is_empty)))
    {
        return Err("模型授权数据无效。".into());
    }
    if !p["allowed_channels"].is_null()
        && p["allowed_channels"]
            .as_array()
            .is_none_or(|a| a.len() > 256 || a.iter().any(|v| v.as_i64().is_none_or(|n| n <= 0)))
    {
        return Err("来源授权数据无效。".into());
    }
    Ok(())
}
fn mode(p: &Value, k: &str) -> &'static str {
    if p[k].is_null() { "all" } else { "selected" }
}
fn set_mode(p: RwSignal<Value>, k: &'static str, v: &str) {
    p.update(|p| p[k] = if v == "all" { Value::Null } else { json!([]) })
}
fn toggle(p: RwSignal<Value>, k: &str, value: Value, on: bool) {
    p.update(|p| {
        let mut a = p[k].as_array().cloned().unwrap_or_default();
        a.retain(|x| x != &value);
        if on {
            a.push(value)
        }
        p[k] = json!(a);
    })
}
pub fn inherit_policy(mut p: Value, parent: &Value) -> Value {
    for field in [
        "allowed_models",
        "allowed_channels",
        "rpm",
        "tpm",
        "max_concurrent",
    ] {
        if p[field].is_null() {
            p[field] = parent[field].clone();
        }
    }
    p["admin_access"] = json!(false);
    p
}
pub use crate::fmt::usd_input as money_input;
pub fn optional_limit(s:&str)->Result<Option<i64>,String>{let s=s.trim();if s.is_empty(){return Ok(None)}let parts=s.split('.').collect::<Vec<_>>();let bad=||"金额需为非负数，最多6位小数；不支持科学计数。".to_string();if parts.len()>2||parts[0].is_empty()||!parts[0].bytes().all(|c|c.is_ascii_digit()){return Err(bad())}let whole=parts[0].parse::<i64>().map_err(|_|bad())?;let fraction=parts.get(1).copied().unwrap_or("");if fraction.len()>6||!fraction.bytes().all(|c|c.is_ascii_digit()){return Err(bad())}let mut decimals=fraction.to_owned();while decimals.len()<6{decimals.push('0');}let decimal=decimals.parse::<i64>().map_err(|_|bad())?;whole.checked_mul(1_000_000).and_then(|n|n.checked_add(decimal)).map(Some).ok_or_else(bad)}
pub fn expiry_value(s: &str) -> Result<Option<i64>, String> {
    if s.is_empty() {
        return Ok(None);
    }
    let n = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(s)).get_time();
    if !n.is_finite() {
        return Err("请选择有效到期日期。".into());
    }
    Ok(Some(n as i64))
}
pub fn expiry_input(ms: Option<i64>) -> String {
    ms.map(|n| {
        let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(n as f64));
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}",
            d.get_full_year(),
            d.get_month() + 1,
            d.get_date(),
            d.get_hours(),
            d.get_minutes()
        )
    })
    .unwrap_or_default()
}
pub fn budget_label(b:&Value)->String{if b.is_null(){return "未设累计消费上限".into()}format!("上限 {} · 已用 {} · 预占 {} USD{}",b["limit_micro"].as_i64().map(money_input).unwrap_or_else(||"不限".into()),b["spent_micro"].as_i64().map(money_input).unwrap_or_else(||"未知".into()),b["reserved_micro"].as_i64().map(money_input).unwrap_or_else(||"未知".into()),if b["frozen"]==true{" · 已冻结"}else{""})}
#[component]
pub fn PolicyEditor(
    policy: RwSignal<Value>,
    models: RwSignal<Vec<String>>,
    channels: RwSignal<Vec<(i64, String)>>,
    prefix: &'static str,
    #[prop(default = false)] admin: bool,
) -> impl IntoView {
    let search = RwSignal::new(String::new());
    view! {<div class="policy-editor">
    <Show when=move||validate(&policy.get()).is_err()><p class="banner bad" role="alert">{move||validate(&policy.get()).err().unwrap_or_default()}</p></Show>
    <div class="account-field"><label for=format!("{prefix}-models")>"模型授权"</label><select id=format!("{prefix}-models") prop:value=move||mode(&policy.get(),"allowed_models") on:change=move|e|set_mode(policy,"allowed_models",&event_target_value(&e))><option value="all">{if admin{"所有当前及未来模型"}else{"继承用户的模型权限"}}</option><option value="selected">"仅勾选的模型（未选即禁止）"</option></select></div>
    <Show when=move||!policy.get()["allowed_models"].is_null()><label class="account-field"><span>"搜索模型"</span><input type="search" prop:value=search on:input=move|e|search.set(event_target_value(&e))/></label><div class="policy-options">{move||{let p=policy.get();let current=p["allowed_models"].as_array().cloned().unwrap_or_default();let catalog=models.get();let mut options=catalog.clone();for v in &current{if let Some(n)=v.as_str(){if !options.iter().any(|o|o==n){options.push(n.into())}}}options.sort();options.dedup();let query=search.get().to_lowercase();options.into_iter().filter(|n|n.to_lowercase().contains(&query)).map(|n|{let checked=current.iter().any(|x|x.as_str()==Some(n.as_str()));let label=if catalog.contains(&n){n.clone()}else{format!("{n} · 已有兼容授权")};view!{<label class="account-check"><input type="checkbox" prop:checked=checked on:change=move|e|toggle(policy,"allowed_models",json!(n),event_target_checked(&e))/>{label}</label>}}).collect_view()}}</div><p class="account-help">"未勾选任何模型表示禁止调用；已有隐藏或兼容授权保留在列表中。"</p></Show>
    <label class="account-check"><input type="checkbox" prop:checked=move||policy.get()["allow_paid"].as_bool().unwrap_or(true) on:change=move|e|policy.update(|p|p["allow_paid"]=json!(event_target_checked(&e)))/>"允许使用付费来源（仍受用户和 Key 余额共同约束）"</label>
    <div class="policy-limits">{[("rpm","每分钟请求数"),("tpm","每分钟 Token 上限"),("max_concurrent","最大并发")].into_iter().map(|(key,label)|view!{<label class="account-field"><span>{label}</span><input type="number" min="1" step="1" placeholder="继承 / 不额外限制" prop:value=move||policy.get()[key].as_u64().map(|n|n.to_string()).unwrap_or_default() on:input=move|e|{let s=event_target_value(&e);policy.update(|p|p[key]=if s.is_empty(){Value::Null}else{s.parse::<u64>().map(|n|json!(n)).unwrap_or_else(|_|json!(s))})}/></label>}).collect_view()}</div>
    <details><summary>"高级：来源范围"</summary><label class="account-field"><span>"可用来源"</span><select prop:value=move||mode(&policy.get(),"allowed_channels") on:change=move|e|set_mode(policy,"allowed_channels",&event_target_value(&e))><option value="all">"继承 / 不额外限制"</option><option value="selected">"仅勾选的来源（未选即禁止）"</option></select></label><Show when=move||!policy.get()["allowed_channels"].is_null()><div class="policy-options">{move||{let p=policy.get();let current=p["allowed_channels"].as_array().cloned().unwrap_or_default();let mut options=channels.get();for id in current.iter().filter_map(Value::as_i64){if !options.iter().any(|(n,_)|*n==id){options.push((id,format!("已有来源 #{id}")))}}options.into_iter().map(|(id,label)|{let checked=current.contains(&json!(id));view!{<label class="account-check"><input type="checkbox" prop:checked=checked on:change=move|e|toggle(policy,"allowed_channels",json!(id),event_target_checked(&e))/>{label}</label>}}).collect_view()}}</div></Show></details>
    <Show when=move||admin&&policy.get()["admin_access"]==true><p class="account-help">"此旧用户/密钥保留管理接口权限；本页面不会静默删除该权限。"</p></Show>
    <p class="account-help">"限流留空表示继承；用户、Key 和父密钥限制同时生效。TPM 是速率限制，不是套餐 Token 余额。"</p>
    </div>}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_other_fields_preserves_null_empty_and_hidden_acl() {
        let mut p = default_policy();
        p["allowed_models"] = json!(["hidden-legacy-high"]);
        p["allowed_channels"] = json!([]);
        p["rpm"] = json!(4);
        assert!(validate(&p).is_ok());
        assert_eq!(p["allowed_models"], json!(["hidden-legacy-high"]));
        assert_eq!(mode(&p, "allowed_channels"), "selected");
        p["allowed_models"] = Value::Null;
        assert_eq!(mode(&p, "allowed_models"), "all");
    }
    #[test]
    fn unknown_fields_and_invalid_limits_cannot_be_saved() {
        let mut p = default_policy();
        p["future_permission"] = json!(true);
        assert!(validate(&p).is_err());
        let mut p = default_policy();
        p["rpm"] = json!(0);
        assert!(validate(&p).is_err());
        p["rpm"] = json!(u64::MAX);
        assert!(validate(&p).is_err());
    }
    #[test]
    fn self_service_inherits_limits_without_widening_or_erasing_empty_acl() {
        let parent = json!({"allowed_models":["hidden"],"allowed_channels":[],"rpm":3,"tpm":null,"max_concurrent":2});
        let child = inherit_policy(default_policy(), &parent);
        assert_eq!(child["allowed_models"], json!(["hidden"]));
        assert_eq!(child["allowed_channels"], json!([]));
        assert_eq!(child["rpm"], 3);
        assert_eq!(child["admin_access"], false);
    }
    #[test]
    fn money_zero_is_not_unlimited() {
        assert_eq!(optional_limit("0").unwrap(), Some(0));
        assert_eq!(optional_limit("0.000001").unwrap(),Some(1));
        assert_eq!(optional_limit("12.345678").unwrap(),Some(12_345_678));
        assert_eq!(optional_limit(&money_input(80_000)).unwrap(),Some(80_000));
        assert!(optional_limit("1.0000001").is_err());
        assert!(optional_limit("1e6").is_err());
        assert!(optional_limit("9223372036854775807").is_err());
        assert_eq!(optional_limit("").unwrap(), None);
        assert!(optional_limit("-1").is_err());
    }
}
