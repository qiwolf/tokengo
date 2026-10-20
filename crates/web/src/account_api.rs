//! Browser account transport. Session credentials remain in HttpOnly cookies.
use crate::api::{ApiErr, call, typed};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Root,
    User,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    pub user_id: Option<i64>,
    pub enabled: bool,
    pub created_at_ms: i64,
}
impl Account {
    pub fn is_root(&self) -> bool {
        self.role == Role::Root
    }
    pub fn label(&self) -> String {
        if self.display_name.is_empty() {
            self.username.clone()
        } else {
            self.display_name.clone()
        }
    }
}
#[derive(Deserialize)]
struct AccountResp {
    account: Account,
}
#[derive(Deserialize)]
struct AccountsResp {
    accounts: Vec<Account>,
}
pub async fn me() -> Result<Account, ApiErr> {
    Ok(typed::<AccountResp>("GET", "/api/account/me", None)
        .await?
        .account)
}
pub async fn login(username: &str, password: &str) -> Result<Account, ApiErr> {
    Ok(typed::<AccountResp>(
        "POST",
        "/api/account/login",
        Some(json!({"username":username,"password":password})),
    )
    .await?
    .account)
}
#[derive(Clone, Deserialize)]
pub struct SetupResponse {
    pub account: Account,
    pub gateway_key: String,
    pub gateway_key_generated: bool,
}
fn setup_body(password: &str, display_name: &str) -> serde_json::Value {
    let mut body = json!({"password": password});
    if !display_name.trim().is_empty() {
        body["display_name"] = json!(display_name);
    }
    body
}
pub async fn setup(password: &str, display_name: &str) -> Result<SetupResponse, ApiErr> {
    typed::<SetupResponse>(
        "POST",
        "/api/account/setup",
        Some(setup_body(password, display_name)),
    )
    .await
}
pub async fn logout() -> Result<(), ApiErr> {
    call("POST", "/api/account/logout", Some(json!({})))
        .await
        .map(|_| ())
}
pub async fn password(old: &str, new: &str) -> Result<(), ApiErr> {
    call(
        "PUT",
        "/api/account/password",
        Some(json!({"old_password":old,"new_password":new})),
    )
    .await
    .map(|_| ())
}
pub async fn accounts() -> Result<Vec<Account>, ApiErr> {
    Ok(typed::<AccountsResp>("GET", "/api/accounts", None)
        .await?
        .accounts)
}
pub async fn create(
    username: &str,
    password: &str,
    display_name: &str,
    user_id: i64,
) -> Result<Account, ApiErr> {
    Ok(typed::<AccountResp>("POST","/api/accounts",Some(json!({"username":username,"password":password,"display_name":display_name,"user_id":user_id,"role":"user"}))).await?.account)
}
pub async fn update(
    id: i64,
    display_name: &str,
    enabled: bool,
    password: Option<&str>,
) -> Result<Account, ApiErr> {
    let mut body = json!({"display_name":display_name,"enabled":enabled});
    if let Some(p) = password {
        body["password"] = json!(p);
    }
    Ok(
        typed::<AccountResp>("PUT", &format!("/api/accounts/{id}"), Some(body))
            .await?
            .account,
    )
}
pub fn valid_username(s: &str) -> bool {
    (3..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
pub fn valid_password(s: &str) -> bool {
    (12..=256).contains(&s.len())
}
#[derive(Debug, Clone, Deserialize)]
pub struct PortalKey {
    pub id: i64,
    pub name: String,
    pub hint: String,
    pub enabled: bool,
    pub revoked_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub parent_key_id: Option<i64>,
    pub created_at_ms: i64,
}
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UsageSummary {
    pub request_count: i64,
    pub ledger_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub unknown_input_entries: i64,
    pub unknown_output_entries: i64,
    pub unpriced_entries: i64,
    pub known_debit_micro: i64,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub id: i64,
    pub request_id: i64,
    pub key_id: i64,
    pub model: String,
    pub billing_unit: String,
    pub status: String,
    pub outcome: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub usage_certainty: String,
    pub debit_micro: Option<i64>,
    pub created_ms: i64,
    pub settled_ms: Option<i64>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct PortalUsage {
    pub summary: UsageSummary,
    pub entries: Vec<Entry>,
    pub next_before_id: Option<i64>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Portal {
    pub account: Account,
    pub keys: Vec<PortalKey>,
    pub usage: PortalUsage,
}
pub async fn portal(before: Option<i64>) -> Result<Portal, ApiErr> {
    let path = before
        .map(|id| format!("/api/account/portal?limit=25&before_id={id}"))
        .unwrap_or_else(|| "/api/account/portal?limit=25".into());
    typed("GET", &path, None).await
}
pub fn entry_tokens(e: &Entry) -> String {
    if e.billing_unit == "request" {
        "不适用（按次）".into()
    } else {
        format!(
            "{} / {}",
            e.input_tokens
                .map(crate::fmt::num)
                .unwrap_or_else(|| "未知".into()),
            e.output_tokens
                .map(crate::fmt::num)
                .unwrap_or_else(|| "未知".into())
        )
    }
}
pub fn debit(v: Option<i64>) -> String {
    v.map(|n| format!("{} 微单位", crate::fmt::num(n)))
        .unwrap_or_else(|| "未知 / 未定价".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roles_are_explicit_and_unknown_roles_rejected() {
        assert!(serde_json::from_value::<Role>(json!("admin")).is_err());
        assert_eq!(
            serde_json::from_value::<Role>(json!("user")).unwrap(),
            Role::User
        );
    }
    #[test]
    fn credentials_validate_bytes_without_rejecting_legacy_login() {
        assert!(valid_username("Andy.demo-1"));
        assert!(!valid_username("../用户"));
        assert!(!valid_password("short"));
        assert!(valid_password("123456789012"));
        assert!(valid_password("密码安全"));
        assert!(!valid_password(&"x".repeat(257)));
    }
    #[test]
    fn optional_setup_display_name_omits_blank_values_for_server_default() {
        for name in ["", "   ", "\t"] {
            let body = setup_body("fixture-password", name);
            assert!(body.get("display_name").is_none());
            assert_eq!(body["password"], "fixture-password");
        }
        assert_eq!(setup_body("fixture-password", "我的网关")["display_name"], "我的网关");
    }
    #[test]
    fn setup_keeps_one_time_gateway_key_separate_from_account() {
        let v:SetupResponse=serde_json::from_value(json!({"account":{"id":1,"username":"admin","display_name":"管理员","role":"root","user_id":null,"enabled":true,"created_at_ms":0},"gateway_key":"fixture-not-a-live-key","gateway_key_generated":true})).unwrap();
        assert_eq!(v.gateway_key,"fixture-not-a-live-key");
        assert!(v.gateway_key_generated);
        assert!(v.account.is_root());
    }
    #[test]
    fn request_units_never_look_like_token_zero() {
        let e:Entry=serde_json::from_value(json!({"id":1,"request_id":2,"key_id":3,"model":"search","billing_unit":"request","status":"unpriced","outcome":"ok","input_tokens":null,"output_tokens":null,"usage_certainty":"unpriced","debit_micro":null,"created_ms":0,"settled_ms":1})).unwrap();
        assert_eq!(entry_tokens(&e), "不适用（按次）");
        assert_eq!(debit(None), "未知 / 未定价");
        assert_eq!(debit(Some(0)), "0 微单位");
    }
}
/// Root changes its own login identifier only after confirming the current password.
pub async fn rename(username: &str, old_password: &str) -> Result<(), ApiErr> {
    call(
        "PUT",
        "/api/account/profile",
        Some(json!({"username":username,"old_password":old_password})),
    )
    .await
    .map(|_| ())
}
