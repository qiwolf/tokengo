//! 后台探活：无流量时主动试首选渠道，成功就切回来。
//!
//! 为什么必须有它：切换之后 "10 分钟后回头试首选" 只在**有流量**时靠下一个请求触发。
//! 半夜没人用的时候，20 分钟、一小时都不会有任何请求 —— 没有后台探活，第二天早上
//! 第一个请求才去戳首选，用户就要等一次失败重试。上游 FreeLLMAPI 靠"冷却到期"
//! 回避这个问题，我们用主动探活（见 `docs/03-路由策略.md` 第三节第 6 条）。
//!
//! 三条纪律：
//!
//! - 探活**不写记账**。它不是用户请求，混进 `requests`/`attempts` 会把"这条渠道被叫了
//!   几次"的账搅浑。只在日志里留痕。
//! - 探活请求要小：对话用 `max_tokens=1`，向量用 1 个字符。
//! - **发出去的是这条渠道真认的模型名**。保留名 `auto` 不是一个真模型名（它由网关现场挑）——
//!   拿 `auto` 去探活必然 400，而 400 按规矩不算渠道的毛病，探活就永远得不出结论。

use std::time::Duration;

use tokengo_shared::channel::Channel;
use tokengo_shared::routing::{classify_status, AttemptOutcome};
use tokengo_shared::ChannelId;

use crate::db::{model_routes, channels, now_ms};
use crate::gateway::EndpointKind;
use crate::SharedState;

/// 探活间隔：每分钟看一遍到点的模型名。
pub const TICK: Duration = Duration::from_secs(60);
/// 探活单次超时上限：别拿 30 秒去等一个探活。
const PROBE_TIMEOUT_CAP: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 首选活了 → 已回切
    SwitchedBack,
    /// 首选还是不行 → 检查点往后推
    StillDown,
    /// 没结论（比如探活请求本身被上游判成 400）→ 什么都不动
    Inconclusive(String),
    /// 没法探（这组没配模型名之类）
    Skipped(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub model: String,
    pub channel: ChannelId,
    pub outcome: ProbeOutcome,
}

/// 起后台任务。main 里调一次。
pub fn spawn(state: SharedState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut t = tokio::time::interval(TICK);
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 第一次 tick 是立即返回的，跳过它（刚启动没必要马上探）
        t.tick().await;
        loop {
            t.tick().await;
            for r in tick(&state, now_ms()).await {
                tracing::info!(
                    "探活 {} → 渠道 {}：{:?}",
                    r.model,
                    r.channel.0,
                    r.outcome
                );
            }
        }
    })
}

/// 走一遍：哪些模型名到点了、挨个探。单独拆出来是为了能测（不用等真定时器）。
pub async fn tick(state: &SharedState, now: i64) -> Vec<ProbeResult> {
    let due = {
        let r = state.routing();
        r.models_needing_probe(now)
    };
    let mut out = Vec::new();
    for model in due {
        let global = state.routing().cfg.clone();
        let enabled = crate::db::route_policy::effective(&state.conn(), &model, global)
            .map(|cfg| cfg.probe_enabled).unwrap_or(false);
        if !enabled { continue; }
        out.push(probe_route(state, &model, now).await);
    }
    out
}

/// 探一个模型名的首选渠道。
pub async fn probe_route(state: &SharedState, name: &str, now: i64) -> ProbeResult {
    let endpoint = state.model_endpoint(name);
    let global = state.routing().cfg.clone();
    let cfg = match crate::db::route_policy::effective(&state.conn(), name, global) {
        Ok(cfg) => cfg,
        Err(_) => return ProbeResult { model: name.into(), channel: ChannelId(0), outcome: ProbeOutcome::Skipped("Invalid routing policy".into()) },
    };

    // 一次性把要用的东西捞齐（同步短锁，绝不在持锁期间 await）
    let loaded = {
        let conn = state.conn();
        let channels_all = model_routes::channels_for(&conn, name).unwrap_or_default();
        let Some(mut primary) = channels_all.into_iter().find(|l| l.routable() && !crate::db::faults::has_fault(&conn,l.id.0,Some(name)).unwrap_or(true)) else {
            return ProbeResult {
                model: name.to_string(),
                channel: ChannelId(0),
                outcome: ProbeOutcome::Skipped("这个名字没有可用渠道".into()),
            };
        };
        // 探活要发的名字：这条渠道为这个模型名配的那个（别名优先，见 probe_model）
        if !crate::protocol_config::background_probe_allowed(&conn,primary.id.0).unwrap_or(false) {
            return ProbeResult {model:name.into(),channel:primary.id,outcome:ProbeOutcome::Skipped("This source requires explicit authenticated and metered verification".into())};
        }
        let model = probe_model(&conn, &primary, name);
        let secret = channels::secret(&conn, &state.secrets, primary.id).unwrap_or(None);
        match channels::headers(&conn, &state.secrets, primary.id) {
            Ok(headers) => primary.headers = headers,
            Err(_) => return ProbeResult { model: name.into(), channel: primary.id, outcome: ProbeOutcome::Skipped("header decryption failed".into()) },
        }
        (primary, model, secret)
    };
    let (channel, model, secret) = loaded;
    let endpoint=match crate::protocol_config::probe_endpoint(&state.conn(),channel.id.0,endpoint) {
        Ok(endpoint)=>endpoint,
        Err(_)=>return ProbeResult{model:name.into(),channel:channel.id,outcome:ProbeOutcome::Skipped("Invalid source protocols".into())},
    };

    let Some(model) = model else {
        return ProbeResult {
            model: name.to_string(),
            channel: channel.id,
            outcome: ProbeOutcome::Skipped(format!(
                "渠道 {} 没配任何模型名，拿不到能发出去的模型名，跳过（不猜）",
                channel.name
            )),
        };
    };

    let ids: Vec<ChannelId> = {
        let conn = state.conn();
        model_routes::channels_for(&conn, name)
            .unwrap_or_default()
            .into_iter()
            .filter(|l| l.routable() && !crate::db::faults::has_fault(&conn,l.id.0,Some(name)).unwrap_or(true))
            .map(|l| l.id)
            .collect()
    };

    let timeout = Duration::from_millis(cfg.attempt_timeout_ms.max(1)).min(PROBE_TIMEOUT_CAP);
    let body = probe_body(endpoint, &model);
    let uuid = crate::upstream::new_uuid();

    let status = state
        .upstream
        .probe(
            &channel,
            secret.as_deref(),
            endpoint.path(),
            body,
            timeout,
            &uuid,
            now_ms(),
        )
        .await;

    let outcome = match status {
        Err(fault) => {
            tracing::warn!("探活 {} 渠道 {} 连不上：{fault}", name, channel.name);
            mark_exhausted(state, name, channel.id, &ids, now);
            ProbeOutcome::StillDown
        }
        Ok(code) => match classify_status(code) {
            AttemptOutcome::Success => {
                {
                    let mut r = state.routing();
                    r.on_success_with_interval(name, channel.id, &ids, now, cfg.reprobe_interval_ms);
                }
                if let Err(e) = state.persist_state(name) {
                    tracing::error!("探活成功后落库失败：{e}");
                }
                ProbeOutcome::SwitchedBack
            }
            AttemptOutcome::ChannelFault => {
                mark_exhausted(state, name, channel.id, &ids, now);
                ProbeOutcome::StillDown
            }
            // 400/404/422：多半是探活用的模型名这条渠道不认（配置问题），不是渠道坏了。
            // **什么都不动** —— 宁可让下次真流量去试，也不能凭一个 400 就把账号切成坏的。
            AttemptOutcome::ClientFault | AttemptOutcome::ClientGone => ProbeOutcome::Inconclusive(
                format!("HTTP {code}（模型名 {model} 这条渠道不认？探活没结论，不动路由状态）"),
            ),
        },
    };

    ProbeResult {
        model: name.to_string(),
        channel: channel.id,
        outcome,
    }
}

fn mark_exhausted(state: &SharedState, model: &str, channel: ChannelId, ids: &[ChannelId], now: i64) {
    let global_interval = state.routing().cfg.reprobe_interval_ms;
    let interval = crate::db::route_policy::load(&state.conn(), model).ok()
        .and_then(|p| p.reprobe_interval_ms).unwrap_or(global_interval);
    {
        let mut r = state.routing();
        r.on_channel_exhausted_with_interval(model, channel, ids, now, interval);
    }
    if let Err(e) = state.persist_state(model) {
        tracing::error!("探活失败后落库失败：{e}");
    }
}

/// 探活该用哪个模型名。优先级：
///   1. 这条渠道为这个模型名配的别名（`v4.1flash` → `deepseek/deepseek-v4.1-flash`）；
///   2. 这条渠道配的任意一个上游模型名；
///   3. 这条渠道配的任意一个模型名（原样透传）。
/// 都没有就不探 —— 拿保留名 `auto` 去探一定被上游 400，探也是白探。
pub fn probe_model(
    conn: &rusqlite::Connection,
    channel: &Channel,
    name: &str,
) -> Option<String> {
    if let Some(a) = channel.model_alias.as_deref() {
        if !a.trim().is_empty() && a != name {
            return Some(a.to_string());
        }
    }
    let row = channels::get(conn, channel.id).ok().flatten()?;
    row.models
        .iter()
        .find_map(|m| m.upstream.clone().filter(|u| !u.trim().is_empty()))
        .or_else(|| {
            row.models
                .iter()
                .map(|m| m.model.clone())
                .find(|n| !n.trim().is_empty())
        })
}

/// 探活请求体：小到不能再小。
pub fn probe_body(endpoint: EndpointKind, model: &str) -> bytes::Bytes {
    let v = match endpoint {
        EndpointKind::Chat => serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1,
            "stream": false,
        }),
        EndpointKind::Responses => serde_json::json!({"model":model,"input":"ping","max_output_tokens":1,"stream":false,"store":false}),
        EndpointKind::Messages => serde_json::json!({"model":model,"messages":[{"role":"user","content":"ping"}],"max_tokens":1,"stream":false}),
        EndpointKind::Embeddings => serde_json::json!({
            "model": model,
            "input": "x",
        }),
        EndpointKind::Rerank => serde_json::json!({
            "model": model,
            "query": "ping",
            "documents": ["x"],
            "top_n": 1,
        }),
        // Standalone paid tools are never automatically probed. Their explicit
        // diagnostic body is kept valid for trusted in-process callers only.
        EndpointKind::Search => serde_json::json!({"query":"DeepSeek","count":1}),
        EndpointKind::Fetch => serde_json::json!({"url":"https://example.com"}),
    };
    bytes::Bytes::from(serde_json::to_vec(&v).unwrap_or_default())
}

/// 「测视觉」用的那张 64×64 小图：四格分别是 左上红 / 右上绿 / 左下蓝 / 右下黄。
///
/// 为什么要造图而不用现成图片：**要能判对错**。问一句"左上角是什么颜色"，
/// 真看了图的模型会答"红"，纯文本模型要么报 400（不支持图片输入），要么答个
/// 别的颜色 —— 三种情况一眼分得清。
///
/// 图**不能太小**：早先那版是 16×16（每格只有 8×8 像素），上游把图标归一化/切块之后
/// 四格糊在一起，回答就容易含糊，判成"看不见"是冤枉它。现在 64×64，四格各 32×32。
/// 压缩后 141 字节（数据 URL 188 个字符），请求还是很小。
pub const VISION_TEST_IMAGE_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAVElEQVR42u3PsQkAMADDsPz/dHpDhwwtAu9GaTItfTwAAAAAAAAAAAAAAAAAAAAAAAAAAACADwHrQ5tpAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADXHfsF7R5fMRs4AAAAAElFTkSuQmCC";

/// 「测视觉」问题的标准问法 + 判对错时认的词。
pub const VISION_TEST_QUESTION: &str = "这张图左上角那一格是什么颜色？只回一个词。";

/// 「测视觉」第一发给多少输出 token。
///
/// **别压到 8**（踩过）：模型不总是乖乖「只回一个词」，先来半句开场白或先写一句
/// 推理，额度就用光了，颜色词还没出口 —— 于是明明看见了却被判成看不见。给够 64。
pub const VISION_TEST_TOKENS: u32 = 64;

/// 第一发被 `finish_reason=length` 截断时，再给这么多重问一次。
pub const VISION_TEST_TOKENS_RETRY: u32 = 256;

/// 「测视觉」的请求体：带图的一发最小对话请求。
///
/// 和探活同一条纪律：模型名是**这条渠道自己认的那个**（拿保留名 `auto` 去问
/// 必然 400，绿点就不真了）。
pub fn vision_test_body(model: &str, max_tokens: u32) -> bytes::Bytes {
    let v = serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": VISION_TEST_QUESTION},
                {"type": "image_url", "image_url": {
                    "url": format!("data:image/png;base64,{}", VISION_TEST_IMAGE_PNG_B64)
                }}
            ]
        }],
        "max_tokens": max_tokens,
        "stream": false,
    });
    bytes::Bytes::from(serde_json::to_vec(&v).unwrap_or_default())
}

/// 上游回的这个 200 是不是被 token 额度截断了（话没说完）。
/// 判成"看不见"之前要看一眼这个：截断属于**我们额度给少了**，不是模型看不见。
pub fn reply_was_truncated(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    v.get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
        .map(|f| f == "length")
        .unwrap_or(false)
}

/// 这个回复算不算"真看见了"：认出红色就算。中文简繁、英文都认。
pub fn answer_says_red(reply: &str) -> bool {
    let t = reply.to_lowercase();
    t.contains('红') || t.contains('紅') || t.contains("red")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{channels, open_in_memory};

    #[test]
    fn probe_body_stays_tiny() {
        let chat = probe_body(EndpointKind::Chat, "m");
        let v: serde_json::Value = serde_json::from_slice(&chat).unwrap();
        assert_eq!(v["max_tokens"], 1);
        assert_eq!(v["model"], "m");
        assert_eq!(v["stream"], false);

        let emb = probe_body(EndpointKind::Embeddings, "bge-m3");
        let v: serde_json::Value = serde_json::from_slice(&emb).unwrap();
        assert_eq!(v["model"], "bge-m3");
        assert_eq!(v["input"], "x");
    }

    #[test]
    fn probe_model_prefers_the_alias_of_that_model_name() {
        let conn = open_in_memory().unwrap();
        let id = channels::create(
            &conn,
            &channels::NewChannel {
                name: "command-go".into(),
                base_url: "https://x/v1".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        let channel = Channel {
            id,
            name: "command-go".into(),
            base_url: "https://x/v1".into(),
            enabled: true,
            priority: 0,
            model_alias: Some("deepseek/deepseek-v4.1-flash".into()),
            vision: false,
            drop_fields: vec![],
            unwrap_envelope: false,
            headers: vec![],
            max_concurrent: 0,
        };
        assert_eq!(
            probe_model(&conn, &channel, "auto").as_deref(),
            Some("deepseek/deepseek-v4.1-flash")
        );
    }

    #[test]
    fn probe_model_falls_back_to_whatever_the_channel_actually_serves() {
        let conn = open_in_memory().unwrap();
        let id = channels::create(
            &conn,
            &channels::NewChannel {
                name: "硅基流动".into(),
                base_url: "https://x/v1".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        channels::set_models(&conn, id, &[channels::ModelMapping::new("bge-m3", Some("BAAI/bge-m3".into()), false)]).unwrap();
        let channel = Channel {
            id,
            name: "硅基流动".into(),
            base_url: "https://x/v1".into(),
            enabled: true,
            priority: 0,
            model_alias: None,
            vision: false,
            drop_fields: vec![],
            unwrap_envelope: false,
            headers: vec![],
            max_concurrent: 0,
        };
        assert_eq!(probe_model(&conn, &channel, "auto").as_deref(), Some("BAAI/bge-m3"));
    }

    /// 反证：什么模型名都没配的渠道，**不许拿保留名 `auto` 去探**（那一定 400）。
    #[test]
    fn probe_model_refuses_to_guess_the_virtual_name() {
        let conn = open_in_memory().unwrap();
        let id = channels::create(
            &conn,
            &channels::NewChannel {
                name: "空渠道".into(),
                base_url: "https://x/v1".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        let channel = Channel {
            id,
            name: "空渠道".into(),
            base_url: "https://x/v1".into(),
            enabled: true,
            priority: 0,
            model_alias: Some("auto".into()),
            vision: false,
            drop_fields: vec![],
            unwrap_envelope: false,
            headers: vec![],
            max_concurrent: 0,
        };
        assert_eq!(probe_model(&conn, &channel, "auto"), None);
    }

    /// 「测视觉」的请求体：必须真带一张图，模型名是这条渠道自己认的那个，
    /// 而且要小（面板按钮点一下不该花掉一堆 token）。
    #[test]
    fn vision_test_body_carries_a_real_image() {
        let b = vision_test_body("glm-4v-flash", VISION_TEST_TOKENS);
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["model"], "glm-4v-flash");
        // **别压到 8**：模型先来半句开场白就把额度用光，颜色词还没出口就被判成「看不见」。
        assert_eq!(v["max_tokens"], 64);
        assert!(v["max_tokens"].as_u64().unwrap() >= 64, "额度要给够一个词 + 一句开场白");
        assert_eq!(v["stream"], false);
        let parts = v["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "image_url");
        let url = parts[1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "得是内嵌图片：{url}");
        assert!(url.len() < 400, "别把请求撑大：{} 字符", url.len());
        assert!(b.len() < 700, "整个请求体要小：{} 字节", b.len());
        // 重问那一发要真的给更多
        let b2 = vision_test_body("m", VISION_TEST_TOKENS_RETRY);
        let v2: serde_json::Value = serde_json::from_slice(&b2).unwrap();
        assert!(v2["max_tokens"].as_u64().unwrap() > 64, "{v2}");
    }

    /// 截断要认得出来：只有 `finish_reason=length` 才算「话没说完」。
    #[test]
    fn truncation_is_recognised_only_from_finish_reason_length() {
        let long = |fr: &str| {
            format!(
                r#"{{"choices":[{{"index":0,"message":{{"role":"assistant","content":"这张图的左上角那一"}},"finish_reason":"{fr}"}}]}}"#
            )
        };
        assert!(reply_was_truncated(&long("length")), "被额度截断要认出来");
        assert!(!reply_was_truncated(&long("stop")), "正常停下来不算截断");
        assert!(!reply_was_truncated("不是 JSON"), "认不出来一律当没截断");
        assert!(!reply_was_truncated(r#"{"choices":[]}"#));
    }

    /// 判对错只认「红」：答别的颜色不算看见了。
    #[test]
    fn only_red_counts_as_seeing_the_image() {
        for yes in ["红", "红色", "紅色", "Red", "RED.", "左上角是红色"] {
            assert!(answer_says_red(yes), "{yes} 应该算看见了");
        }
        for no in ["绿", "蓝色", "yellow", "我不确定", ""] {
            assert!(!answer_says_red(no), "{no} 不该算看见了");
        }
    }
}
