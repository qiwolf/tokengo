//! 后端接口的数据形状。**字段名必须和服务端 JSON 一致** —— 服务端改了字段
//! 这里跟着改，否则面板会出现"某个值永远是空的"这种不报错的坏现象。
//!
//! 全部走 `serde` 反序列化，缺字段会退回 `Default`（`#[serde(default)]`），
//! 这样服务端加字段不会把面板打挂。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct AuthStatus {
    pub needs_setup: bool,
    pub has_password: bool,
    pub gateway_key_set: bool,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub version: String,
}

/// 面板初始化/改口令的返回：token 之外还可能回显一次明文对外 key。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct SetupResp {
    pub token: String,
    #[serde(default)]
    pub gateway_key: Option<String>,
    #[serde(default)]
    pub gateway_key_generated: bool,
    #[serde(default)]
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct HeaderPair {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct ModelPair {
    pub model: String,
    #[serde(default)]
    pub upstream: Option<String>,
    /// 这个模型名在这条渠道上能不能吃图片输入（面板上的「能看图」勾选）。
    #[serde(default)]
    pub vision: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub has_secret: bool,
    #[serde(default)]
    pub headers: Vec<HeaderPair>,
    #[serde(default)]
    pub models: Vec<ModelPair>,
    /// 这条渠道上「能看图」的模型名有几个（服务端算好的，列表里直接显示）。
    #[serde(default)]
    pub vision_count: i64,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct ChannelsResp {
    #[serde(default)]
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Group {
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub channels: Vec<i64>,
    /// 这一组现在有没有「能看图」的可用渠道。带图的请求能不能走这条组就看它。
    #[serde(default)]
    pub can_see: bool,
}

/// 一个真能被调用的模型名 + 它能不能吃图。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Servable {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vision: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct GroupsResp {
    #[serde(default)]
    pub groups: Vec<Group>,
    /// 真能被调用的模型名（组开着、且组里至少有一条可用渠道）。
    #[serde(default)]
    pub servable: Vec<Servable>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct RoutingCfg {
    pub retry_times: u32,
    pub attempt_timeout_ms: u64,
    pub request_budget_ms: u64,
    pub reprobe_interval_ms: i64,
    pub probe_enabled: bool,
}

impl RoutingCfg {
    /// 回切间隔按分钟显示/填写 —— 界面上写毫秒没人愿意算。
    pub fn reprobe_minutes(&self) -> i64 {
        self.reprobe_interval_ms / 60_000
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct RoutingGroup {
    pub group: String,
    pub enabled: bool,
    pub primary_channel_id: Option<i64>,
    pub active_channel_id: Option<i64>,
    pub switched: bool,
    pub switch_count: i64,
    pub last_switch_at_ms: Option<i64>,
    pub next_probe_at_ms: Option<i64>,
    pub now_ms: i64,
    #[serde(default)]
    pub channels: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct RoutingResp {
    pub config: RoutingCfg,
    #[serde(default)]
    pub groups: Vec<RoutingGroup>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct ChannelStat {
    pub channel_id: i64,
    #[serde(default)]
    pub channel_name: Option<String>,
    pub attempts: i64,
    pub successes: i64,
    pub channel_faults: i64,
    pub avg_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// 服务端算好的成功率（百分比，一位小数）。
    #[serde(default)]
    pub success_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Stats {
    pub since_ms: i64,
    pub total: i64,
    pub ok: i64,
    pub failed: i64,
    pub canceled: i64,
    pub client_errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default)]
    pub by_channel: Vec<ChannelStat>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct StatsResp {
    pub window_hours: i64,
    pub stats: Stats,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct LogRow {
    pub id: i64,
    pub ts_ms: i64,
    pub endpoint: String,
    pub model: String,
    pub status: String,
    pub http_status: Option<i64>,
    pub channel_id: Option<i64>,
    pub latency_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct LogsResp {
    #[serde(default)]
    pub logs: Vec<LogRow>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct ProbeResp {
    pub ok: bool,
    pub channel_id: i64,
    #[serde(default)]
    pub channel_name: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub model_sent: Option<String>,
    #[serde(default)]
    pub http_status: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<i64>,
    #[serde(default)]
    pub has_secret: bool,
    #[serde(default)]
    pub note: Option<String>,
}

/// 「拉取上游模型清单」的响应。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct ModelPull {
    pub ok: bool,
    pub channel_id: i64,
    #[serde(default)]
    pub channel_name: Option<String>,
    /// 真去打的 URL（界面上写出来，好让人自己核）
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub http_status: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<i64>,
    #[serde(default)]
    pub count: i64,
    /// 上游报的模型名。**拉不到就是空**，界面不许自己编。
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// 「测视觉」的响应：真发一张图之后，上游到底说了什么。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct VisionTest {
    pub ok: bool,
    pub channel_id: i64,
    #[serde(default)]
    pub channel_name: Option<String>,
    /// 测的是哪个对外模型名（通常就是组名）。
    #[serde(default)]
    pub model: Option<String>,
    /// 真发给上游的名字。
    #[serde(default)]
    pub model_sent: Option<String>,
    #[serde(default)]
    pub http_status: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<i64>,
    /// 上游回的话（原样，不加工）—— 人自己判断它是不是真看见了。
    #[serde(default)]
    pub reply: Option<String>,
    /// 测通了、有没有顺手把这行标上。
    #[serde(default)]
    pub applied: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct SettingsResp {
    pub routing_config: RoutingCfg,
    #[serde(default)]
    pub gateway_key_hint: Option<String>,
    #[serde(default)]
    pub active_sessions: i64,
    #[serde(default)]
    pub reprobe_interval_minutes: i64,
}

/// 「账目维护」这一块的状态（`GET /api/maintenance`）。
///
/// 面板上只让用户填一个数：保留多少天。剩下三行是只读的实话：
/// 库现在多大、最老一条是什么时候、上次清掉多少 —— 用户靠这三行判断
/// 「清扫员到底干没干活」。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MaintenanceResp {
    /// 0 = 不清理
    #[serde(default)]
    pub retention_days: i64,
    #[serde(default)]
    pub total_requests: i64,
    #[serde(default)]
    pub oldest_ts_ms: Option<i64>,
    #[serde(default)]
    pub db_bytes: i64,
    #[serde(default)]
    pub last_prune_at_ms: Option<i64>,
    #[serde(default)]
    pub last_prune_count: i64,
}

/// 请求终态的中文说法 + 配色类名。**面板里所有地方都走这一个映射**，
/// 免得"成功"在一个页面写中文、在另一个页面写英文。
pub fn status_label(status: &str) -> (&'static str, &'static str) {
    match status {
        "ok" => ("成功", "ok"),
        "failed" => ("失败", "bad"),
        "client_error" => ("客户端错", "warn"),
        "canceled" => ("取消", "muted"),
        _ => ("进行中", "muted"),
    }
}

/// 端点短名：流水表里列宽有限。
pub fn endpoint_label(e: &str) -> &str {
    match e {
        "chat" | "chat/completions" => "对话",
        "embeddings" => "向量",
        "models" => "模型清单",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_server_json_parses() {
        // 直接拿服务端真会吐的 JSON 当样本，字段名对不上就会在这里炸。
        let channels: ChannelsResp = serde_json::from_str(
            r#"{"channels":[{"id":44,"name":"硅基流动","base_url":"https://api.siliconflow.cn/v1",
                "enabled":true,"has_secret":true,
                "headers":[{"name":"x-a","value":"{{uuid}}"}],
                "models":[{"model":"auto","upstream":"BAAI/bge-m3"}]}]}"#,
        )
        .unwrap();
        assert_eq!(channels.channels[0].id, 44);
        assert!(channels.channels[0].has_secret);
        assert_eq!(channels.channels[0].models[0].upstream.as_deref(), Some("BAAI/bge-m3"));

        let logs: LogsResp = serde_json::from_str(
            r#"{"logs":[{"id":1,"ts_ms":1000,"endpoint":"embeddings","model":"bge-m3",
                "status":"ok","http_status":200,"channel_id":44,"latency_ms":310,
                "input_tokens":11,"output_tokens":0,"error":null}]}"#,
        )
        .unwrap();
        assert_eq!(logs.logs[0].endpoint, "embeddings");
        assert_eq!(logs.logs[0].http_status, Some(200));

        let st: StatsResp = serde_json::from_str(
            r#"{"window_hours":24,"stats":{"since_ms":0,"total":3,"ok":2,"failed":1,
                "canceled":0,"client_errors":0,"input_tokens":5,"output_tokens":7,
                "by_channel":[{"channel_id":44,"attempts":3,"successes":2,"channel_faults":1,
                "avg_ms":120,"input_tokens":5,"output_tokens":7,"channel_name":"硅基流动",
                "success_rate":66.7}]}}"#,
        )
        .unwrap();
        assert_eq!(st.stats.by_channel[0].channel_name.as_deref(), Some("硅基流动"));
        assert_eq!(st.stats.by_channel[0].success_rate, Some(66.7));
    }

    #[test]
    fn unknown_status_does_not_panic() {
        // 服务端以后加了新的终态，面板也不该崩，只显示个通用说法。
        let (label, class) = status_label("something_new");
        assert_eq!(label, "进行中");
        assert_eq!(class, "muted");
    }

    #[test]
    fn reprobe_minutes_round_trips() {
        let c = RoutingCfg {
            reprobe_interval_ms: 10 * 60 * 1000,
            ..Default::default()
        };
        assert_eq!(c.reprobe_minutes(), 10);
    }
}
