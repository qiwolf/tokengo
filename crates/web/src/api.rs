//! 打后端接口。**面板里所有请求都从这里走一层**，好处是：
//!
//! - 会话 token 只在这一个地方拼进请求头，不会某个页面漏带；
//! - 401（会话过期）在这一层统一识别，界面能立刻退回登录页，而不是各页面各弹一个错；
//! - 非 2xx 一律变成带人话的 `ApiErr`，页面上直接显示，不需要每处重新解析。

use gloo_net::http::Request;
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
    key: Option<&str>,
    headers: &[HeaderPair],
    models: &[ModelPair],
) -> Result<Option<i64>, ApiErr> {
    let body = json!({
        "name": name,
        "base_url": base_url,
        "enabled": enabled,
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
pub async fn pull_channel_models(id: i64) -> Result<ModelPull, ApiErr> {
    typed("POST", &format!("/api/channels/{id}/models"), None).await
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

pub async fn groups() -> Result<GroupsResp, ApiErr> {
    typed("GET", "/api/groups", None).await
}

pub async fn save_group(name: &str, enabled: bool, channels: &[i64]) -> Result<(), ApiErr> {
    call(
        "POST",
        "/api/groups",
        Some(json!({ "name": name, "enabled": enabled, "channels": channels })),
    )
    .await
    .map(|_| ())
}

pub async fn delete_group(name: &str) -> Result<(), ApiErr> {
    call("DELETE", &format!("/api/groups/{name}"), None).await.map(|_| ())
}

pub async fn routing() -> Result<RoutingResp, ApiErr> {
    typed("GET", "/api/routing", None).await
}

pub async fn save_routing_config(cfg: &RoutingCfg) -> Result<(), ApiErr> {
    let v = serde_json::to_value(cfg).map_err(|e| ApiErr { status: 0, message: e.to_string() })?;
    call("PUT", "/api/routing/config", Some(v)).await.map(|_| ())
}

/// 不想等 10 分钟时用：把某组的回切检查点归零。
pub async fn reprobe(group: &str) -> Result<(), ApiErr> {
    call("POST", &format!("/api/routing/{group}/reprobe"), None).await.map(|_| ())
}

pub async fn stats(hours: i64) -> Result<StatsResp, ApiErr> {
    typed("GET", &format!("/api/stats?hours={hours}"), None).await
}

pub async fn logs(limit: u32) -> Result<Vec<LogRow>, ApiErr> {
    let r: LogsResp = typed("GET", &format!("/api/logs?limit={limit}"), None).await?;
    Ok(r.logs)
}

pub async fn settings() -> Result<SettingsResp, ApiErr> {
    typed("GET", "/api/settings", None).await
}

pub async fn set_gateway_key(key: &str) -> Result<(), ApiErr> {
    call("PUT", "/api/settings", Some(json!({ "gateway_key": key }))).await.map(|_| ())
}
