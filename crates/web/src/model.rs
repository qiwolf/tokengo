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
    /// 发上游前要丢掉的请求体顶层字段，逗号分隔（面板上的「要丢的字段」输入框）。
    ///
    /// 例：opencode 的 `beam-free` 不认 `reasoning_effort`，不丢就必然 400。
    #[serde(default)]
    pub drop_fields: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub has_secret: bool,
    /// 非流式响应外面套了一层壳时自动剥掉（面板上的「响应解包」勾选）。
    #[serde(default)]
    pub unwrap_envelope: bool,
    /// 这条渠道「同时最多几发」（0 = 不限）—— 上游按凭据限并发时靠它防 429。
    #[serde(default)]
    pub max_concurrent: u32,
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

/// 一个模型名（一行）—— 服务端 `/api/model-routes` 算好的全部事实。
///
/// **一个名字 = 渠道里勾了它的那些渠道**（v0.11.0 起没有「模型组」这一层），
/// 按渠道顺序先试先后。名字不落库：勾上就出现、取消就消失。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct NameRoute {
    #[serde(default)]
    pub runtime: RuntimeScore,
    #[serde(default)]
    pub detection_status: String,
    #[serde(default)]
    pub kind_conflict: bool,
    pub name: String,
    /// `""` / `chat` / `embeddings` / `rerank` / `image` / `video` / `audio`。
    #[serde(default)]
    pub kind: String,
    /// 服务端给的中文名（面板不自己维护一份对照表）。
    #[serde(default)]
    pub kind_label: String,
    /// 谁定的：`""` / `auto`（实测学的）/ `manual`（人选的）。
    #[serde(default)]
    pub kind_source: String,
    /// 有没有启用的渠道勾了它、并且标了「能看图」。
    #[serde(default)]
    pub can_see: bool,
    /// 可用渠道的上下文长度（tokens）；缺失时不按模型名猜测。
    #[serde(default)]
    pub context_window: Option<i64>,
    /// `upstream` = 上游声明；未知来源不能当作实测。
    #[serde(default)]
    pub context_source: String,
    /// 现在有没有渠道能用（`false` = 客户端打这个名字会 404）。
    #[serde(default)]
    pub routed: bool,
    /// 谁勾了它（含**停用**的渠道），按渠道顺序。
    #[serde(default)]
    pub declared_by: Vec<i64>,
    /// 现在真能用的那串渠道，按先试顺序（就是请求的尝试顺序）。
    #[serde(default)]
    pub channels: Vec<i64>,
    /// 现在走的那条（切过就在这上面）。
    #[serde(default)]
    pub active_channel_id: Option<i64>,
    #[serde(default)]
    pub switch_count: i64,
    #[serde(default)]
    pub next_probe_at_ms: Option<i64>,
}

#[derive(Debug,Clone,PartialEq,Default,Deserialize)]
pub struct RuntimeScore {
    #[serde(default)] pub avg_first_byte_ms:Option<i64>,
    #[serde(default)] pub samples:i64,
    #[serde(default)] pub successes:i64,
    #[serde(default)] pub faults:i64,
    #[serde(default)] pub avg_ms:Option<i64>,
    #[serde(default)] pub score:Option<f64>,
    #[serde(default)] pub last_at:Option<i64>,
    #[serde(default)] pub last_error:Option<String>,
}

/// 保留名 `auto` 现在会挑谁 —— 服务端按实测成绩排好序给的。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct AutoRoutes {
    /// 一个勾选过的模型都没有 → `false`，这时候客户端清单里**不会**出现 auto。
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub chat: Vec<String>,
    #[serde(default)]
    pub embeddings: Vec<String>,
    #[serde(default)]
    pub rerank: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct ModelRoutesResp {
    #[serde(default)]
    pub models: Vec<NameRoute>,
    #[serde(default)]
    pub auto: AutoRoutes,
    #[serde(default)]
    pub now_ms: i64,
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

/// 路由状态里的一个模型名（面板「现在走哪条」「换过几次」用它）。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct RouteState {
    pub model: String,
    /// 这个名字现在该走的第一条（= 勾它的渠道里排最前的那条）。
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
    pub models: Vec<RouteState>,
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

/// 按模型名的账（看的是客户端打了什么名字）。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct ModelStat {
    pub model: String,
    pub requests: i64,
    pub ok: i64,
    pub failed: i64,
    pub client_errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 按客户端的账。`client_ip` 为空 = 没拿到来源地址。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct ClientStat {
    #[serde(default)]
    pub client_ip: Option<String>,
    pub requests: i64,
    pub ok: i64,
    pub failed: i64,
    pub client_errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 错误分布的一行。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct ErrorStat {
    pub error: String,
    pub count: i64,
}

/// 按端点类型的账：对话 / 向量 / 重排 / 别的。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct KindStat {
    /// `chat` / `embeddings` / `rerank` / `""`
    #[serde(default)]
    pub kind: String,
    /// 服务端给的中文名（跟着服务端的口径走，面板不自己维护一份）
    #[serde(default)]
    pub label: String,
    pub requests: i64,
    pub ok: i64,
    pub failed: i64,
    pub client_errors: i64,
    pub canceled: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// 只算成功请求的平均耗时。
    pub avg_ms: i64,
}

impl KindStat {
    /// 成功率（%）。分母是「打进来几次」，与总览四张卡一个口径。
    pub fn success_rate(&self) -> f64 {
        if self.requests > 0 {
            (self.ok as f64 / self.requests as f64 * 1000.0).round() / 10.0
        } else {
            0.0
        }
    }
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
    /// 按端点类型，固定顺序：对话 → 向量 → 重排（缺的也有一行 0）。
    #[serde(default)]
    pub by_kind: Vec<KindStat>,
    #[serde(default)]
    pub by_model: Vec<ModelStat>,
    #[serde(default)]
    pub by_client: Vec<ClientStat>,
    #[serde(default)]
    pub errors: Vec<ErrorStat>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct StatsResp {
    pub window_hours: i64,
    pub stats: Stats,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct LogRow {
    #[serde(default)]
    pub error_code:Option<String>,
    #[serde(default)]
    pub first_byte_ms: Option<i64>,
    #[serde(default)]
    pub client_ip: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub streamed: bool,
    #[serde(default)]
    pub served_model: String,
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
    /// 这条请求是不是在让模型看图（网关记的，跟路由判断同一口径）。
    #[serde(default)]
    pub has_image: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct LogsResp {
    #[serde(default)]
    pub logs: Vec<LogRow>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct AttemptRow {
    #[serde(default)] pub model_name:Option<String>,
    #[serde(default)] pub upstream_model:Option<String>,
    pub ordinal: i64,
    pub channel_id: i64,
    pub channel_name: Option<String>,
    pub outcome: String,
    pub http_status: Option<i64>,
    pub duration_ms: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct RequestDetail {
    pub request: LogRow,
    pub attempts: Vec<AttemptRow>,
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
    /// 上游报的模型名 + **上游自己给**的说明（归属方、上下文长度、价钱、它自称能不能收图）。
    /// 上游没说的字段就是空 —— 界面上一个标签都不显示，绝不替它猜。
    #[serde(default)]
    pub info: Vec<UpstreamModel>,
    #[serde(default)]
    pub note: Option<String>,
    /// 这次拿到的清单有没有**落库**（拉失败 = false，库里还是上次那份）。
    #[serde(default)]
    pub saved: bool,
    /// 这次落库的时间（毫秒；0 = 从来没问过）。
    #[serde(default)]
    pub fetched_at: i64,
    /// 跟上次记住的比：多了哪些名字。
    #[serde(default)]
    pub added: Vec<String>,
    /// 跟上次记住的比：少了哪些名字（上游改名/下线是常事，一次问答看不出漂移）。
    #[serde(default)]
    pub removed: Vec<String>,
}

/// 上游 `/v1/models` 里的一条。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct UpstreamModel {
    /// 上游认的名字。
    pub id: String,
    /// 归属方（有的上游不报）。
    #[serde(default)]
    pub owned_by: Option<String>,
    /// 上游自报的上下文长度（tokens）。
    #[serde(default)]
    pub context_window: Option<i64>,
    /// 价格提示：`free` 或 `$1.25/M in $2/M out`。
    #[serde(default)]
    pub price_note: Option<String>,
    /// 每一档价都是 0（或上游直接写 free）。
    #[serde(default)]
    pub is_free: bool,
    /// 上游**自称**能不能收图。`None` = 它没说。**这不是我们的「能看图」标记** ——
    /// 路由标记也可能由声明或人工设置采用，不能仅凭标记声称已经实测。
    #[serde(default)]
    pub vision: Option<bool>,
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
    /// 这次给了多少输出额度（被截断时能看出来是我们额度给少了）。
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// 第一发被截断（`finish_reason=length`）、又给更多额度重问过一次。
    #[serde(default)]
    pub retried_with_more_tokens: bool,
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
    /// 能不能一键复制（老库只有哈希时为 false）。
    #[serde(default)]
    pub gateway_key_copyable: bool,
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

    /// 回归：面板「模型」页读的就是这份 JSON —— 字段名对不上会在这里炸。
    #[test]
    fn the_model_routes_json_from_the_server_parses() {
        let r: ModelRoutesResp = serde_json::from_str(
            r#"{"models":[{"name":"BAAI/bge-m3","kind":"embeddings","kind_label":"嵌入",
                 "kind_source":"manual","can_see":false,"routed":true,
                 "declared_by":[44,45],"channels":[44],"active_channel_id":44,
                 "switch_count":2,"next_probe_at_ms":1800000000000}],
                "auto":{"available":true,"chat":["v4.1flash"],"embeddings":["BAAI/bge-m3"],
                 "rerank":["BAAI/bge-reranker-v2-m3"]},
                "now_ms":1800000000000}"#,
        )
        .unwrap();
        assert_eq!(r.models.len(), 1);
        assert_eq!(r.models[0].name, "BAAI/bge-m3");
        assert_eq!(r.models[0].channels, vec![44], "先试的那串");
        assert_eq!(r.models[0].declared_by, vec![44, 45], "勾过它但停用/不可用的也算");
        assert!(r.auto.available);
        assert_eq!(r.auto.chat, vec!["v4.1flash"]);
        assert_eq!(r.now_ms, r.models[0].next_probe_at_ms.unwrap());

        // auto 不可用时一个候选都没有，客户端清单里也不出现 auto
        let empty: ModelRoutesResp =
            serde_json::from_str(r#"{"models":[],"auto":{"available":false},"now_ms":1}"#).unwrap();
        assert!(!empty.auto.available);
        assert!(empty.auto.chat.is_empty());
    }

    #[test]
    fn context_metadata_is_optional_and_never_inferred() {
        let old: NameRoute = serde_json::from_str(r#"{"name":"huge-128k-vision"}"#).unwrap();
        assert_eq!(old.context_window, None);
        assert!(old.context_source.is_empty());
        assert!(!old.can_see);
        let declared: NameRoute = serde_json::from_str(
            r#"{"name":"alias","context_window":131072,"context_source":"upstream","can_see":true}"#,
        ).unwrap();
        assert_eq!(declared.context_window, Some(131072));
        assert_eq!(declared.context_source, "upstream");
        assert!(declared.can_see);
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

/// 面板「日志」页：一次拉回来的运行日志 + 当前级别。
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct LiveLogs {
    pub lines: Vec<String>,
    /// 内存里一共留了多少行（让用户知道"再往上就没啦"）
    #[serde(default)]
    pub kept: usize,
    #[serde(default)]
    pub limit: usize,
    #[serde(default)]
    pub level: String,
    /// 限时 debug 的到期时间（毫秒时间戳）
    #[serde(default)]
    pub revert_at_ms: Option<i64>,
    #[serde(default)]
    pub file: Option<String>,
}

/// 日志级别（改完之后服务端回这个）。
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct LogLevel {
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub revert_at_ms: Option<i64>,
}
