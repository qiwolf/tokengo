//! 打后端接口。**面板里所有请求都从这里走一层**，好处是：
//!
//! - 会话 token 只在这一个地方拼进请求头，不会某个页面漏带；
//! - 401（会话过期）在这一层统一识别，界面能立刻退回登录页，而不是各页面各弹一个错；
//! - 非 2xx 一律变成带人话的 `ApiErr`，页面上直接显示，不需要每处重新解析。

use gloo_net::http::Request;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::model::*;

const TOKEN_KEY: &str = "tokengo_session";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok()).flatten()
}

/// 面板当前会话 token（存在浏览器 localStorage 里）。
pub fn token() -> Option<String> {
    storage()?
        .get_item(TOKEN_KEY)
        .ok()
        .flatten()
        .filter(|t| !t.trim().is_empty())
}

pub fn set_token(t: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(TOKEN_KEY, t);
    }
}

pub fn clear_token() {
    if let Some(s) = storage() {
        let _ = s.remove_item(TOKEN_KEY);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiErr {
    pub status: u16,
    pub message: String,
}

impl ApiErr {
    /// 会话过期/没登录 —— 界面该退回登录页。
    pub fn is_unauthorized(&self) -> bool {
        self.status == 401
    }
}

impl std::fmt::Display for ApiErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// 底层：发一个请求，返回解析后的 JSON。非 2xx 变 `ApiErr`。
async fn call(method: &str, path: &str, body: Option<Value>) -> Result<Value, ApiErr> {
    // 按方法名挑构造函数：gloo-net 的 `.method()` 要 http::Method 枚举，
    // 这里用它的快捷构造函数更直观，也不会把方法名写错
    let mut b = match method {
        "GET" => Request::get(path),
        "POST" => Request::post(path),
        "PUT" => Request::put(path),
        "DELETE" => Request::delete(path),
        other => {
            return Err(ApiErr {
                status: 0,
                message: format!("面板内部错误：不支持的方法 {other}"),
            })
        }
    };
    if let Some(t) = token() {
        b = b.header("authorization", &format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .json(&v)
            .map_err(|e| ApiErr { status: 0, message: format!("请求体打包失败：{e}") })?,
        None => b
            .build()
            .map_err(|e| ApiErr { status: 0, message: format!("请求构造失败：{e}") })?,
    };
    let resp = req.send().await.map_err(|e| ApiErr {
        status: 0,
        message: format!("连不上网关：{e}"),
    })?;

    let status = resp.status();
    // 先取文本再自己解析：出错时正文可能不是 JSON（比如网关被人接了别的东西），
    // 直接 .json() 会丢掉真正的错因，只剩一句解析失败。
    let text = resp.text().await.unwrap_or_default();
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

    if !(200..300).contains(&status) {
        let msg = parsed
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if text.trim().is_empty() {
                    format!("HTTP {status}")
                } else {
                    text.chars().take(300).collect()
                }
            });
        return Err(ApiErr { status, message: msg });
    }
    Ok(parsed)
}

async fn typed<T: DeserializeOwned>(method: &str, path: &str, body: Option<Value>) -> Result<T, ApiErr> {
    let v = call(method, path, body).await?;
    serde_json::from_value(v).map_err(|e| ApiErr {
        status: 0,
        message: format!("返回的 JSON 和面板预期不一致：{e}"),
    })
}

// ───────────────────────── 具体接口 ─────────────────────────

/// 登录页要用的状态（不需要会话）。
pub async fn auth_status() -> Result<AuthStatus, ApiErr> {
    typed("GET", "/api/auth/status", None).await
}

pub async fn login(password: &str) -> Result<SetupResp, ApiErr> {
    typed("POST", "/api/auth/login", Some(json!({ "password": password }))).await
}

/// 首次初始化：设面板口令 + 对外 key（不传 key 就由服务端生成一个）。
pub async fn setup(password: &str, gateway_key: Option<&str>) -> Result<SetupResp, ApiErr> {
    typed(
        "POST",
        "/api/auth/setup",
        Some(json!({ "password": password, "gateway_key": gateway_key })),
    )
    .await
}

pub async fn logout() -> Result<(), ApiErr> {
    let body = json!({ "token": token() });
    call("POST", "/api/auth/logout", Some(body)).await.map(|_| ())
}

pub async fn change_password(old: &str, new: &str) -> Result<SetupResp, ApiErr> {
    typed(
        "PUT",
        "/api/auth/password",
        Some(json!({ "old_password": old, "new_password": new })),
    )
    .await
}

pub async fn channels() -> Result<Vec<Channel>, ApiErr> {
    let r: ChannelsResp = typed("GET", "/api/channels", None).await?;
    Ok(r.channels)
}

/// 建渠道/改渠道。`key` 为 `None` = 不改密钥；`Some("")` = 明确清掉。
///
/// 返回新建渠道的 id（改渠道时返回 `None`）—— 面板新建完要马上能用这条渠道自己的
/// 凭据去拉上游模型清单，所以得知道它的 id。
pub async fn save_channel(
    id: Option<i64>,
    name: &str,
    base_url: &str,
    enabled: bool,
    unwrap_envelope: bool,
    max_concurrent: u32,
    key: Option<&str>,
    headers: &[HeaderPair],
    models: &[ModelPair],
) -> Result<Option<i64>, ApiErr> {
    let body = json!({
        "name": name,
        "base_url": base_url,
        "enabled": enabled,
        "unwrap_envelope": unwrap_envelope,
        "max_concurrent": max_concurrent,
        "key": key,
        "headers": headers,
        "models": models,
    });
    match id {
        Some(i) => call("PUT", &format!("/api/channels/{i}"), Some(body)).await.map(|_| None),
        None => {
            let v = call("POST", "/api/channels", Some(body)).await?;
            Ok(v.get("id").and_then(|x| x.as_i64()))
        }
    }
}

pub async fn delete_channel(id: i64) -> Result<(), ApiErr> {
    call("DELETE", &format!("/api/channels/{id}"), None).await.map(|_| ())
}

/// 测一下连通性。`kind` 是 `chat` 或 `embeddings`。
pub async fn probe_channel(id: i64, kind: &str) -> Result<ProbeResp, ApiErr> {
    typed("POST", &format!("/api/channels/{id}/probe?kind={kind}"), None).await
}

/// 拉这条渠道的上游模型清单（真打它的 `GET /v1/models`）。
/// 拉成功会**落库记住**（响应里的 `saved` / `fetched_at` / `added` / `removed`）。
pub async fn pull_channel_models(id: i64) -> Result<ModelPull, ApiErr> {
    typed("POST", &format!("/api/channels/{id}/models"), None).await
}

/// 这条渠道**上次**问到的上游模型清单（读库，不联网）。
///
/// 面板打开编辑弹窗先读它：省一次上游请求（有的上游要花钱、有限流），
/// 也让「上游这次比上次少了哪些名字」有得比。
pub async fn channel_catalog(id: i64) -> Result<ModelPull, ApiErr> {
    typed("GET", &format!("/api/channels/{id}/catalog"), None).await
}

/// 「测视觉」：给这条渠道真发一张带图的小请求，看它认不认得出来。
///
/// `model` 是这一行的对外模型名（要测哪一行），`upstream` 是发给上游的名字
/// （留空 = 跟保存的语义一样，原样透传）。
pub async fn vision_test(id: i64, model: &str, upstream: Option<&str>) -> Result<VisionTest, ApiErr> {
    typed(
        "POST",
        &format!("/api/channels/{id}/vision-test"),
        Some(json!({ "model": model, "upstream": upstream, "apply": true })),
    )
    .await
}

/// 一键启用/停用一条渠道（面板状态开关）。
pub async fn set_channel_enabled(id: i64, enabled: bool) -> Result<(), ApiErr> {
    call(
        "PUT",
        &format!("/api/channels/{id}/enabled"),
        Some(json!({ "enabled": enabled })),
    )
    .await
    .map(|_| ())
}

/// 组名可能带斜杠（如 `cline-pass/deepseek-v4.1-flash`），拼进路径前要转义。
/// 按 UTF-8 字节转义（组名里有中文时整字符转义会拼出无效路径）。
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 给人手动定一个**模型名**的用途；`auto` = 交还给实测（清空）。
///
/// 名字可能带斜杠（`BAAI/bge-m3`），进路径前必须转义 —— 否则会被当成两段路径。
pub async fn set_model_kind(name: &str, kind: &str) -> Result<(), ApiErr> {
    let body = json!({ "kind": kind });
    call("PUT", &model_path(name), Some(body)).await.map(|_| ())
}

/// **模型名进 URL 的那一段：必须转义。**
///
/// 名字可以带斜杠（`BAAI/bge-m3` 就是这么来的），不转义的话
/// `/api/models/BAAI/bge-m3/kind` 会被当成三段路径，路由匹配不上 → 404。
/// 按 UTF-8 字节转义（名字里有中文时整字符转义会拼出无效路径）。
/// 凡是「名字进 URL」的地方都用这个函数，别手拼。
pub(crate) fn model_path(name: &str) -> String {
    format!("/api/models/{}/kind", urlencode(name))
}

/// 这个名字的回切检查点归零用的路径（不想等回切间隔时用）。
pub(crate) fn reprobe_path(name: &str) -> String {
    format!("/api/routing/{}/reprobe", urlencode(name))
}

/// 面板「模型」页要的全部事实：一行一个模型名 + 它后面那串渠道 + 路由状态 + auto 的候选。
pub async fn model_routes() -> Result<ModelRoutesResp, ApiErr> {
    typed("GET", "/api/model-routes", None).await
}

/// 把一条渠道上移 / 下移一位。
///
/// 顺序全站只有一份（`channels.priority`）：一个模型名下先试哪条也看它，
/// 所以这个按钮在「渠道」页上。
pub async fn move_channel(id: i64, up: bool) -> Result<(), ApiErr> {
    call(
        "POST",
        &format!("/api/channels/{id}/move"),
        Some(json!({ "up": up })),
    )
    .await
    .map(|_| ())
}

pub async fn routing() -> Result<RoutingResp, ApiErr> {
    typed("GET", "/api/routing", None).await
}

pub async fn save_routing_config(cfg: &RoutingCfg) -> Result<(), ApiErr> {
    let v = serde_json::to_value(cfg).map_err(|e| ApiErr { status: 0, message: e.to_string() })?;
    call("PUT", "/api/routing/config", Some(v)).await.map(|_| ())
}

/// 不想等 10 分钟时用：把某个模型名的回切检查点归零。
pub async fn reprobe(model: &str) -> Result<(), ApiErr> {
    call("POST", &reprobe_path(model), None).await.map(|_| ())
}

pub async fn stats(hours: i64) -> Result<StatsResp, ApiErr> {
    typed("GET", &format!("/api/stats?hours={hours}"), None).await
}

pub async fn request_detail(id: i64) -> Result<RequestDetail, ApiErr> {
    typed("GET", &format!("/api/requests/{id}"), None).await
}

pub async fn logs(limit: u32) -> Result<Vec<LogRow>, ApiErr> {
    let r: LogsResp = typed("GET", &format!("/api/logs?limit={limit}"), None).await?;
    Ok(r.logs)
}

pub async fn settings() -> Result<SettingsResp, ApiErr> {
    typed("GET", "/api/settings", None).await
}

pub async fn gateway_keys() -> Result<Vec<GatewayKey>, ApiErr> {
    let r: GatewayKeysResp = typed("GET", "/api/gateway-keys", None).await?;
    Ok(r.keys)
}

pub async fn create_gateway_key(name: &str) -> Result<GatewayKey, ApiErr> {
    typed("POST", "/api/gateway-keys", Some(json!({ "name": name }))).await
}

pub async fn delete_gateway_key(id: i64) -> Result<(), ApiErr> {
    call("DELETE", &format!("/api/gateway-keys/{id}"), None).await.map(|_| ())
}

/// Only a user-requested copy fetches plaintext; lists contain metadata only.
pub async fn reveal_gateway_key(id: i64) -> Result<String, ApiErr> {
    let v = call("GET", &format!("/api/gateway-keys/{id}/reveal"), None).await?;
    v.get("gateway_key")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .ok_or_else(|| ApiErr {
            message: "服务端没回 key".into(),
            status: 0,
        })
}

// ───────────────────────── 账目维护 ─────────────────────────

pub async fn maintenance() -> Result<MaintenanceResp, ApiErr> {
    typed("GET", "/api/maintenance", None).await
}

pub async fn set_retention_days(days: i64) -> Result<(), ApiErr> {
    call(
        "POST",
        "/api/maintenance",
        Some(json!({ "retention_days": days })),
    )
    .await
    .map(|_| ())
}

/// 「立即清理一次」：返回后端那句人话结果（清掉多少、还剩多少）。
pub async fn prune_now() -> Result<PruneResp, ApiErr> {
    typed("POST", "/api/maintenance/prune", None).await
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PruneResp {
    #[serde(default)]
    pub deleted: i64,
    #[serde(default)]
    pub remaining: i64,
    #[serde(default)]
    pub db_bytes: i64,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub notice: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：模型名带斜杠时进 URL 必须转义 —— 不转义就是用户报的「删除失败 HTTP404」
    /// 的老毛病（`/api/models/BAAI/bge-m3/kind` 被当成三段路径，路由匹配不上）。
    #[test]
    fn a_slash_in_a_model_name_is_escaped_before_it_goes_into_the_url() {
        assert_eq!(model_path("BAAI/bge-m3"), "/api/models/BAAI%2Fbge-m3/kind");
        assert_eq!(model_path("auto"), "/api/models/auto/kind", "普通名字不该被改花");
        // 中文、空格、问号、井号都得转义：这些字符不转义会被当成路径/查询的一部分
        assert_eq!(
            model_path("深度/求索"),
            "/api/models/%E6%B7%B1%E5%BA%A6%2F%E6%B1%82%E7%B4%A2/kind"
        );
        assert_eq!(model_path("a b"), "/api/models/a%20b/kind");
        assert_eq!(model_path("a?b"), "/api/models/a%3Fb/kind");
        assert_eq!(model_path("a#b"), "/api/models/a%23b/kind");
        // 合法的普通字符保持原样（`-_.~` 与字母数字）
        assert_eq!(model_path("gpt-4o_mini.v2~x"), "/api/models/gpt-4o_mini.v2~x/kind");
        // 回切那条路径同理
        assert_eq!(reprobe_path("BAAI/bge-m3"), "/api/routing/BAAI%2Fbge-m3/reprobe");
    }
}

/// 拉最近的运行日志（面板「日志」页）。`level` 是门槛（warn = 警告与错误），`q` 是关键词。
pub async fn logs_live(
    level: Option<String>,
    q: Option<String>,
    limit: u32,
) -> Result<crate::model::LiveLogs, ApiErr> {
    let mut url = format!("/api/logs/live?limit={limit}");
    if let Some(l) = level {
        url.push_str(&format!("&level={}", urlencode(&l)));
    }
    if let Some(q) = q {
        url.push_str(&format!("&q={}", urlencode(&q)));
    }
    let v = call("GET", &url, None).await?;
    serde_json::from_value(v).map_err(|e| ApiErr {
        message: format!("日志格式不对：{e}"),
        status: 0,
    })
}

/// 改日志级别（「临时开 debug」走它）。`minutes > 0` = 到点自动收回。
pub async fn set_log_level(
    level: &str,
    minutes: Option<u32>,
) -> Result<crate::model::LogLevel, ApiErr> {
    let v = call(
        "PUT",
        "/api/logs/level",
        Some(serde_json::json!({ "level": level, "minutes": minutes })),
    )
    .await?;
    serde_json::from_value(v).map_err(|e| ApiErr {
        message: format!("回包格式不对：{e}"),
        status: 0,
    })
}


pub async fn auto_policy()->Result<crate::model::AutoPolicyResp,ApiErr>{typed("GET","/api/auto-policy",None).await}
pub async fn save_auto_policy(policy:&crate::model::AutoPolicy)->Result<(),ApiErr>{call("PUT","/api/auto-policy",Some(json!(policy))).await.map(|_|())}
pub async fn move_model(model:&str,up:bool)->Result<(),ApiErr>{call("POST","/api/model-order/move",Some(json!({"model":model,"up":up}))).await.map(|_|())}

pub async fn context_capacity(channel:i64,model:&str,upstream:&str)->Result<Value,ApiErr>{
 call("GET",&format!("/api/channels/{channel}/context?model={}&upstream={}",js_sys::encode_uri_component(model),js_sys::encode_uri_component(upstream)),None).await
}

pub async fn set_context_override(channel:i64,model:&str,upstream:&str,tokens:Option<i64>) -> Result<(),ApiErr> {
    call("PUT",&format!("/api/channels/{channel}/context"),Some(json!({"model":model,"upstream":upstream,"tokens":tokens}))).await?;
    Ok(())
}

pub async fn set_model_enabled(model:&str,enabled:bool)->Result<(),ApiErr>{call("PUT","/api/model-enabled",Some(json!({"model":model,"enabled":enabled}))).await.map(|_|())}

pub async fn retry_account(channel_id:i64,model:String)->Result<(),ApiErr>{call("POST","/api/channel-faults/retry",Some(json!({"channel_id":channel_id,"model":model}))).await.map(|_|())}
pub async fn model_cooldowns()->Result<crate::model::CooldownsResp,ApiErr>{let v=call("GET","/api/model-cooldowns",None).await?;serde_json::from_value(v).map_err(|e|ApiErr{status:0,message:e.to_string()})}
pub async fn clear_cooldowns(model:Option<String>,channel_id:Option<i64>)->Result<(),ApiErr>{call("POST","/api/model-cooldown/reset",Some(json!({"model":model,"channel_id":channel_id}))).await.map(|_|())}
