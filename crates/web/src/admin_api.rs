//! Route B administration shares the existing panel session transport.
//! Created VKey secrets are never written to browser storage.
use crate::api::{call, ApiErr};
use serde_json::Value;

pub async fn request(method: &str, path: &str, body: Option<Value>) -> Result<Value, ApiErr> {
    let value = call(method, path, body).await?;
    let base = path.split('?').next().unwrap_or(path);
    let required = if matches!(base, "/api/users" | "/api/account/keys") && method == "GET" {
        Some((if base == "/api/users" {"users"} else {"keys"}, true))
    } else if base.starts_with("/api/users/") && base.ends_with("/keys") && method == "GET" {
        Some(("keys", true))
    } else if base == "/api/pricing" {
        Some((
            if method == "GET" { "prices" } else { "price" },
            method == "GET",
        ))
    } else if base == "/api/request-pricing" {
        Some((
            if method == "GET" {
                "request_prices"
            } else {
                "request_price"
            },
            method == "GET",
        ))
    } else if base == "/api/budgets" {
        Some(("budgets", true))
    } else if base == "/api/model-routing" {
        Some(("policy", false))
    } else if base == "/api/usage" {
        Some(("entries", true))
    } else if base.starts_with("/api/vkeys/") || base.ends_with("/keys") {
        Some(("key", false))
    } else if base.starts_with("/api/users") {
        Some(("user", false))
    } else {
        None
    };
    if let Some((field, array)) = required {
        if !(if array {
            value[field].is_array()
        } else {
            value[field].is_object()
        }) {
            return Err(ApiErr {
                status: 0,
                message: format!("返回数据缺少 {field}；请刷新并检查服务端版本。"),
            });
        }
    }
    if method == "POST"
        && base.ends_with("/keys")
        && value["secret"].as_str().is_none_or(str::is_empty)
    {
        return Err(ApiErr {
            status: 0,
            message: "密钥创建返回缺少明文；请刷新列表确认创建结果，勿重复提交。".into(),
        });
    }
    Ok(value)
}
pub fn encode(value: &str) -> String {
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_default()
}
pub fn text(row: &Value, key: &str) -> String {
    match &row[key] {
        Value::String(s) => s.clone(),
        Value::Null => "未知".into(),
        v => v.to_string(),
    }
}
pub fn rows(value: &Value, key: &str) -> Vec<Value> {
    value[key].as_array().cloned().unwrap_or_default()
}
pub fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
pub fn object(raw: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("JSON 格式错误：{e}"))?;
    if !v.is_object() {
        return Err("必须填写 JSON 对象".into());
    }
    Ok(v)
}
pub fn integer(raw: &str, label: &str, positive: bool) -> Result<i64, String> {
    raw.trim()
        .parse::<i64>()
        .ok()
        .filter(|n| if positive { *n > 0 } else { *n >= 0 })
        .ok_or_else(|| {
            format!(
                "{label}必须是{}整数（最大 i64）",
                if positive { "正" } else { "非负" }
            )
        })
}
/// Older token-only servers omit this field; explicit unknown units stay unknown.
pub fn billing_unit(row: &Value) -> &'static str {
    match row.get("billing_unit") {
        Some(Value::String(s)) if s == "request" => "request",
        Some(Value::String(s)) if s == "token" => "token",
        None => "token",
        _ => "unknown",
    }
}
pub fn billing_unit_label(row: &Value) -> &'static str {
    match billing_unit(row) {
        "request" => "request（按次）",
        "token" => "token",
        _ => "未知单位",
    }
}
pub fn billed_requests(row: &Value) -> String {
    if billing_unit(row) == "request" {
        text(row, "request_units")
    } else {
        "不适用".into()
    }
}
pub fn billed_tokens(row: &Value) -> String {
    if billing_unit(row) == "token" {
        format!(
            "{} / {}",
            text(row, "input_tokens"),
            text(row, "output_tokens")
        )
    } else {
        "不适用".into()
    }
}
fn partial_sum(rows: &[&Value], field: &str) -> String {
    let mut sum = 0_i128;
    let mut unknown = 0;
    for row in rows {
        if let Some(n) = row[field].as_i64() {
            sum += i128::from(n);
        } else {
            unknown += 1;
        }
    }
    if unknown == 0 {
        sum.to_string()
    } else if unknown == rows.len() {
        format!("未知（{unknown} 笔）")
    } else {
        format!("{sum}（另 {unknown} 笔未知）")
    }
}
/// Page-local totals only: unknown counts and money are never silently zero-filled.
pub fn billing_page_summary(rows: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for unit in ["token", "request", "unknown"] {
        let group: Vec<_> = rows.iter().filter(|r| billing_unit(r) == unit).collect();
        if group.is_empty() {
            continue;
        }
        let quantity = match unit {
            "token" => format!(
                "输入 / 输出 token：{} / {}",
                partial_sum(&group, "input_tokens"),
                partial_sum(&group, "output_tokens")
            ),
            "request" => format!("收费次数：{}", partial_sum(&group, "request_units")),
            _ => "用量单位未知".into(),
        };
        out.push(format!(
            "当前页 {unit} 计费 {} 笔；{quantity}；扣款 / 成本 microUSD：{} / {}",
            group.len(),
            partial_sum(&group, "debit_micro"),
            partial_sum(&group, "observed_cost_micro")
        ));
    }
    out
}

pub const POLICY: &str = r#"{
  "allowed_models": null,
  "allowed_services": null,
  "allowed_channels": null,
  "allow_paid": true,
  "admin_access": false,
  "rpm": null,
  "tpm": null,
  "max_concurrent": null
}"#;

pub fn protocols(raw: &str) -> Result<Value, String> {
    let v = object(raw)?;
    if v.as_object().is_some_and(|o| {
        o.keys()
            .any(|k| !matches!(k.as_str(), "native" | "allow_conversion" | "native_input_audio" | "chat_reasoning_replay" | "max_input_tokens" | "max_output_tokens" | "parameter_profile" | "runtime_identity"))
    }) {
        return Err("协议配置包含未知字段；请检查原生协议、转换、音频、推理回放、上下文上限与参数适配配置".into());
    }
    let native = v["native"].as_array().ok_or("native 必须是协议数组")?;
    if native.is_empty()
        || native.len() > 3
        || native.iter().enumerate().any(|(i, p)| {
            !matches!(p.as_str(), Some("chat" | "responses" | "messages"))
                || native[..i].contains(p)
        })
    {
        return Err("native 需填写 1..3 个不重复的 chat / responses / messages".into());
    }
    for flag in ["allow_conversion", "native_input_audio", "chat_reasoning_replay"] {
        if v.get(flag).is_some_and(|x| !x.is_boolean()) {
            return Err(format!("{flag} 必须是布尔值"));
        }
    }
    if v["native_input_audio"] == true && !native.iter().any(|p| p == "chat") {
        return Err("native_input_audio 需要原生 chat".into());
    }
    if v["chat_reasoning_replay"] == true && !native.iter().any(|p| p == "chat") {
        return Err("chat_reasoning_replay 需要原生 chat".into());
    }
    if let Some(profile) = v.get("parameter_profile").filter(|value| !value.is_null()) {
        if !matches!(profile.as_str(), Some("openai" | "anthropic" | "gemini" | "deepseek" | "dashscope" | "moonshot" | "zai" | "minimax" | "ark" | "siliconflow" | "mimo" | "ollama" | "vllm" | "native")) {
            return Err("parameter_profile 必须是已支持的服务商适配配置".into());
        }
    }
    // Preserve advanced runtime evidence verbatim. The authoritative server
    // validates matching profile, exact model IDs and digest bounds before saving.
    if let Some(identity)=v.get("runtime_identity").filter(|value|!value.is_null()) {
        if !identity.is_object() || identity.to_string().len()>262_144 {
            return Err("runtime_identity 须为有界对象或 null；来源和摘要由服务端保存前校验".into());
        }
    }
    let input = v.get("max_input_tokens").filter(|value| !value.is_null());
    let output = v.get("max_output_tokens").filter(|value| !value.is_null());
    match (input, output) {
        (None, None) => {},
        (Some(input), Some(output)) if input.as_i64().is_some_and(|n| n > 0) && output.as_i64().is_some_and(|n| n > 0) => {},
        _ => return Err("max_input_tokens 与 max_output_tokens 须同时填写正整数上限".into()),
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_editor_preserves_supported_contract_and_context_fields() {
        let raw = r#"{"native":["chat","responses"],"allow_conversion":true,"native_input_audio":false,"chat_reasoning_replay":true,"parameter_profile":"deepseek","max_input_tokens":128000,"max_output_tokens":64000}"#;
        assert_eq!(protocols(raw).unwrap(), serde_json::from_str::<Value>(raw).unwrap());
        assert!(protocols(r#"{"native":["responses"],"parameter_profile":null,"max_input_tokens":null,"max_output_tokens":null,"chat_reasoning_replay":false}"#).is_ok());
        for raw in [
            r#"{"native":["chat"],"parameter_profile":"made-up"}"#,
            r#"{"native":["chat"],"parameter_profile":true}"#,
            r#"{"native":["responses"],"chat_reasoning_replay":true}"#,
            r#"{"native":["chat"],"chat_reasoning_replay":null}"#,
            r#"{"native":["chat"],"max_input_tokens":1}"#,
            r#"{"native":["chat"],"max_input_tokens":1,"max_output_tokens":0}"#,
            r#"{"native":["chat"],"max_input_tokens":1,"max_output_tokens":1.5}"#,
            r#"{"native":["chat"],"max_input_tokens":"1","max_output_tokens":1}"#,
            r#"{"native":["chat"],"max_input_tokens":9223372036854775808,"max_output_tokens":1}"#,
        ] { assert!(protocols(raw).is_err(), "{raw}"); }
    }
    #[test]
    fn w08_runtime_identity_survives_advanced_protocol_editing() {
        let value=serde_json::json!({"native":["chat"],"parameter_profile":"ollama","allow_conversion":true,
            "runtime_identity":{"runtime":"ollama","evidence":"operator_declared","version":"0.13.3",
                "models":[{"model":"qwen3:8b","template_sha256":"a".repeat(64)}]}});
        assert_eq!(protocols(&value.to_string()).unwrap(),value);
        let mut cleared=value.clone();cleared["runtime_identity"]=Value::Null;
        assert_eq!(protocols(&cleared.to_string()).unwrap(),cleared);
        let mut malformed=value.clone();malformed["runtime_identity"]=serde_json::json!(true);
        assert!(protocols(&malformed.to_string()).is_err());
    }
    #[test]
    fn monetary_inputs_keep_integer_precision_and_reject_overflow() {
        assert_eq!(
            integer("9007199254740993", "预算", false).unwrap(),
            9007199254740993
        );
        assert_eq!(
            integer("9223372036854775807", "预算", false).unwrap(),
            i64::MAX
        );
        for raw in ["9223372036854775808", "1.5", "-1", "NaN", "1e6"] {
            assert!(integer(raw, "预算", false).is_err(), "{raw}");
        }
        assert_eq!(integer("0", "预算", false).unwrap(), 0);
        assert!(integer("0", "ID", true).is_err());
    }
    #[test]
    fn protocol_editor_rejects_invalid_declarations_and_null_boolean() {
        for raw in [
            r#"{"native":[]}"#,
            r#"{"native":["chat","chat"]}"#,
            r#"{"native":["unknown"]}"#,
            r#"{"native":["chat"],"allow_conversion":null}"#,
            r#"{"native":["chat"],"typo":true}"#,
        ] {
            assert!(protocols(raw).is_err(), "{raw}");
        }
        let config =
            protocols(r#"{"native":["messages","chat"],"allow_conversion":true}"#).unwrap();
        assert_eq!(config["native"][0], "messages");
        assert!(protocols(r#"{"native":["responses"]}"#).is_ok());
    }
    #[test]
    fn billing_units_keep_request_counts_separate_from_tokens_and_unknown() {
        let token = serde_json::json!({"billing_unit":"token","input_tokens":0,"output_tokens":7,"request_units":1,"debit_micro":0,"observed_cost_micro":0});
        let request = serde_json::json!({"billing_unit":"request","request_units":1,"input_tokens":999,"debit_micro":750,"observed_cost_micro":750});
        let zero = serde_json::json!({"billing_unit":"request","request_units":0,"debit_micro":0,"observed_cost_micro":0});
        let unknown = serde_json::json!({"billing_unit":"request","request_units":null,"debit_micro":750,"observed_cost_micro":null});
        assert_eq!(billed_tokens(&token), "0 / 7");
        assert_eq!(billed_requests(&token), "不适用");
        assert_eq!(billed_tokens(&request), "不适用");
        assert_eq!(billed_requests(&request), "1");
        assert_eq!(billed_requests(&zero), "0");
        assert_eq!(billed_requests(&unknown), "未知");
        let summary = billing_page_summary(&[token, request, zero, unknown]);
        assert_eq!(summary.len(), 2);
        assert!(summary[0].contains("输入 / 输出 token：0 / 7"));
        assert!(summary[1].contains("收费次数：1（另 1 笔未知）"));
        assert!(summary[1].contains("1500 / 750（另 1 笔未知）"));
        assert!(!summary[1].contains("token"));
        assert_eq!(billing_unit(&serde_json::json!({})), "token");
        assert_eq!(
            billing_unit(&serde_json::json!({"billing_unit":"future"})),
            "unknown"
        );
        assert_eq!(
            billing_unit(&serde_json::json!({"billing_unit":null})),
            "unknown"
        );
        assert!(
            billing_page_summary(&[serde_json::json!({"billing_unit":"request"})])[0]
                .contains("收费次数：未知（1 笔）")
        );
    }
    #[test]
    fn unknown_ledger_cost_is_not_zero() {
        assert_eq!(
            text(&serde_json::json!({"debit_micro":null}), "debit_micro"),
            "未知"
        );
        assert_eq!(
            text(&serde_json::json!({"debit_micro":0}), "debit_micro"),
            "0"
        );
        assert!(object("null").is_err());
        assert!(object("[]").is_err());
        assert!(object(POLICY).is_ok());
    }
}
