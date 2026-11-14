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
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct UsageSummary {
    #[serde(default="crate::fmt::legacy_currency")] pub currency:String,
    pub request_count: i64,
    pub ledger_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub unknown_input_entries: i64,
    pub unknown_output_entries: i64,
    pub unpriced_entries: i64,
    pub known_debit_micro: Option<i64>,
    #[serde(default)] pub known_debit_cny_micro: Option<i64>,
    #[serde(default)] pub display_currency: Option<String>,
    #[serde(default)] pub reporting_fx: serde_json::Value,
    #[serde(default)] pub amounts_by_currency: Vec<serde_json::Value>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    #[serde(default="crate::fmt::legacy_currency")] pub currency:String,
    #[serde(default)] pub display_debit_micro:Option<i64>,
    #[serde(default)] pub quantity_milli:Option<i64>,
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
    #[serde(default)] pub analytics: serde_json::Value,
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
pub async fn portal(before: Option<i64>, query: &str, limit: u32) -> Result<Portal, ApiErr> {
    let mut path = format!("/api/account/portal?limit={limit}{query}");
    if let Some(id) = before { path.push_str(&format!("&before_id={id}")); }
    typed("GET", &path, None).await
}
pub fn entry_tokens(e: &Entry) -> String {
    if !matches!(e.billing_unit.as_str(),"token"|"request") {
        crate::fmt::quantity(&e.billing_unit,e.quantity_milli)
    } else if e.billing_unit == "request" {
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
pub fn debit_with_currency(v:Option<i64>,currency:&str)->String {v.map(|n|format!("{} {}",crate::fmt::usd_input(n),currency)).unwrap_or_else(||"未知 / 未定价".into())}
pub fn debit(v: Option<i64>) -> String {
    v.map(|n| format!("{} USD", crate::fmt::usd_input(n)))
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
        assert_eq!(debit(Some(0)), "0 USD");
        assert_eq!(debit(Some(21)), "0.000021 USD");
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

/// Reporting display is always CNY; raw currency remains in the original entry.
pub fn reporting_debit(entry:&Entry)->String {
    if let Some(value)=entry.display_debit_micro {return format!("{} 元",crate::fmt::usd_input(value));}
    if entry.debit_micro.is_none(){return "未知 / 未定价".into();}
    if entry.currency=="CNY" {return format!("{} 元",crate::fmt::usd_input(entry.debit_micro.unwrap()));}
    "汇率暂不可用".into()
}

/// Pagination is not part of an interval statistic's identity.
pub(crate) fn usage_statistics_filter(query: &str) -> String {
    let mut parts = query.trim_start_matches('?').split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("limit=") && !p.starts_with("before_id="))
        .collect::<Vec<_>>();
    parts.sort_unstable();
    parts.join("&")
}
#[derive(Debug, Clone, Default)]
pub(crate) struct UsageStatisticsRequest {
    filter: Option<String>,
    generation: u64,
}
impl UsageStatisticsRequest {
    pub fn begin(&mut self, query: &str, refresh: bool) -> Option<(u64, String)> {
        let filter = usage_statistics_filter(query);
        if !refresh && self.filter.as_ref() == Some(&filter) { return None; }
        self.generation += 1;
        self.filter = Some(filter.clone());
        Some((self.generation, filter))
    }
    pub fn accepts(&self, generation: u64) -> bool { self.generation == generation && self.filter.is_some() }
    pub fn failed(&mut self, generation: u64) { if self.accepts(generation) { self.generation += 1; } }
    pub fn not_started(&mut self, generation: u64) { if self.accepts(generation) { self.filter = None; } }
}
#[derive(Debug, Clone, Deserialize)]
pub struct UsageStatistics {
    pub summary: serde_json::Value,
    pub analytics: serde_json::Value,
}
pub async fn usage_statistics(query: &str) -> Result<UsageStatistics, ApiErr> {
    typed("GET", &format!("/api/account/usage/analytics?{}",usage_statistics_filter(query)), None).await
}
#[cfg(test)]
mod usage_statistics_request_tests {
    use super::*;
    #[test]
    fn paging_and_page_size_do_not_recalculate_full_statistics() {
        let mut state=UsageStatisticsRequest::default();
        let first=state.begin("limit=25&state=all&start_at=10",false).unwrap();
        assert!(state.begin("start_at=10&before_id=90&limit=100&state=all",false).is_none());
        assert!(state.accepts(first.0));
        assert!(state.begin("state=all&start_at=10",true).is_some());
        assert!(!state.accepts(first.0));
    }
    #[test]
    fn changed_filters_reject_old_slow_results_and_errors() {
        let mut state=UsageStatisticsRequest::default();
        let old=state.begin("start_at=10&model=old",false).unwrap();
        let current=state.begin("start_at=10&model=new",false).unwrap();
        assert!(!state.accepts(old.0));
        state.failed(old.0);
        assert!(state.accepts(current.0));
        state.failed(current.0);
        assert!(!state.accepts(current.0));
        assert!(state.begin("model=new&start_at=10&before_id=20",false).is_none(), "paging must not retry a failed full scan");
        assert!(state.begin("model=new&start_at=10",true).is_some());
    }
}
