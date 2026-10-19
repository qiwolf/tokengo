//! Route B administration shares the existing panel session transport.
//! Created VKey secrets are never written to browser storage.
use crate::api::{call, ApiErr};
use serde_json::Value;

pub async fn request(method: &str, path: &str, body: Option<Value>) -> Result<Value, ApiErr> {
    let value = call(method, path, body).await?;
    let base = path.split('?').next().unwrap_or(path);
    let required = if base == "/api/users" && method == "GET" {
        Some(("users", true))
    } else if base.starts_with("/api/users/") && base.ends_with("/keys") && method == "GET" {
        Some(("keys", true))
    } else if base == "/api/pricing" {
        Some((
            if method == "GET" { "prices" } else { "price" },
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
pub const POLICY: &str = r#"{
  "allowed_models": null,
  "allowed_channels": null,
  "allow_paid": true,
  "admin_access": false,
  "rpm": null,
  "tpm": null,
  "max_concurrent": null
}"#;

pub fn protocols(raw: &str) -> Result<Value, String> {
    let v = object(raw)?;
    if v.as_object()
        .is_some_and(|o| o.keys().any(|k| k != "native" && k != "allow_conversion"))
    {
        return Err("协议配置仅支持 native 与 allow_conversion".into());
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
    if v.get("allow_conversion").is_some_and(|x| !x.is_boolean()) {
        return Err("allow_conversion 必须是布尔值".into());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
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
