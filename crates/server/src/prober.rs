//! 后台探活：无流量时主动试首选腿，成功就切回来。
//!
//! 为什么必须有它：切换之后 "10 分钟后回头试首选" 只在**有流量**时靠下一个请求触发。
//! 半夜没人用的时候，20 分钟、一小时都不会有任何请求 —— 没有后台探活，第二天早上
//! 第一个请求才去戳首选，用户就要等一次失败重试。上游 FreeLLMAPI 靠"冷却到期"
//! 回避这个问题，我们用主动探活（见 `docs/03-路由策略.md` 第三节第 6 条）。
//!
//! 三条纪律：
//!
//! - 探活**不写记账**。它不是用户请求，混进 `requests`/`attempts` 会把"这条腿被叫了
//!   几次"的账搅浑。只在日志里留痕。
//! - 探活请求要小：对话用 `max_tokens=1`，向量用 1 个字符。
//! - **发出去的是这条腿真认的模型名**。组名 `auto` 是个虚拟名，上游不认 —— 拿 `auto`
//!   去探活必然 400，而 400 按规矩不算腿的毛病，探活就永远得不出结论。

use std::time::Duration;

use tokengo_shared::leg::Leg;
use tokengo_shared::routing::{classify_status, AttemptOutcome};
use tokengo_shared::LegId;

use crate::db::{groups, legs, now_ms};
use crate::gateway::EndpointKind;
use crate::SharedState;

/// 探活间隔：每分钟看一遍到点的模型组。
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
    pub group: String,
    pub leg: LegId,
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
                    "探活 {} → 腿 {}：{:?}",
                    r.group,
                    r.leg.0,
                    r.outcome
                );
            }
        }
    })
}

/// 走一遍：哪些组到点了、挨个探。单独拆出来是为了能测（不用等真定时器）。
pub async fn tick(state: &SharedState, now: i64) -> Vec<ProbeResult> {
    let due = {
        let r = state.routing();
        if !r.cfg.probe_enabled {
            return Vec::new();
        }
        r.groups_needing_probe(now)
    };
    let mut out = Vec::new();
    for group in due {
        out.push(probe_group(state, &group, now).await);
    }
    out
}

/// 探一个组的首选腿。
pub async fn probe_group(state: &SharedState, group: &str, now: i64) -> ProbeResult {
    let endpoint = state.group_endpoint(group);

    // 一次性把要用的东西捞齐（同步短锁，绝不在持锁期间 await）
    let loaded = {
        let conn = state.conn();
        let legs_all = groups::resolve_legs(&conn, group).unwrap_or_default();
        let Some(primary) = legs_all.into_iter().find(|l| l.routable()) else {
            return ProbeResult {
                group: group.to_string(),
                leg: LegId(0),
                outcome: ProbeOutcome::Skipped("这组没有可用腿".into()),
            };
        };
        let model = probe_model(&conn, &primary, group);
        let secret = legs::secret(&conn, &state.secrets, primary.id).unwrap_or(None);
        (primary, model, secret)
    };
    let (leg, model, secret) = loaded;

    let Some(model) = model else {
        return ProbeResult {
            group: group.to_string(),
            leg: leg.id,
            outcome: ProbeOutcome::Skipped(format!(
                "腿 {} 没配任何模型名，拿不到能发出去的模型名，跳过（不猜）",
                leg.name
            )),
        };
    };

    let ids: Vec<LegId> = {
        let conn = state.conn();
        groups::resolve_legs(&conn, group)
            .unwrap_or_default()
            .into_iter()
            .filter(|l| l.routable())
            .map(|l| l.id)
            .collect()
    };

    let timeout = {
        let cfg = state.routing().cfg.clone();
        Duration::from_millis(cfg.attempt_timeout_ms.max(1)).min(PROBE_TIMEOUT_CAP)
    };
    let body = probe_body(endpoint, &model);
    let uuid = crate::upstream::new_uuid();

    let status = state
        .upstream
        .probe(
            &leg,
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
            tracing::warn!("探活 {} 腿 {} 连不上：{fault}", group, leg.name);
            mark_exhausted(state, group, leg.id, &ids, now);
            ProbeOutcome::StillDown
        }
        Ok(code) => match classify_status(code) {
            AttemptOutcome::Success => {
                {
                    let mut r = state.routing();
                    r.on_success(group, leg.id, &ids, now);
                }
                if let Err(e) = state.persist_state(group) {
                    tracing::error!("探活成功后落库失败：{e}");
                }
                ProbeOutcome::SwitchedBack
            }
            AttemptOutcome::LegFault => {
                mark_exhausted(state, group, leg.id, &ids, now);
                ProbeOutcome::StillDown
            }
            // 400/404/422：多半是探活用的模型名这条腿不认（配置问题），不是腿坏了。
            // **什么都不动** —— 宁可让下次真流量去试，也不能凭一个 400 就把账号切成坏的。
            AttemptOutcome::ClientFault | AttemptOutcome::ClientGone => ProbeOutcome::Inconclusive(
                format!("HTTP {code}（模型名 {model} 这条腿不认？探活没结论，不动路由状态）"),
            ),
        },
    };

    ProbeResult {
        group: group.to_string(),
        leg: leg.id,
        outcome,
    }
}

fn mark_exhausted(state: &SharedState, group: &str, leg: LegId, ids: &[LegId], now: i64) {
    {
        let mut r = state.routing();
        r.on_leg_exhausted(group, leg, ids, now);
    }
    if let Err(e) = state.persist_state(group) {
        tracing::error!("探活失败后落库失败：{e}");
    }
}

/// 探活该用哪个模型名。优先级：
///   1. 这条腿为这个组名配的别名（`auto` → `deepseek/deepseek-v4.1-flash`）；
///   2. 这条腿配的任意一个上游模型名；
///   3. 这条腿配的任意一个模型名（原样透传）。
/// 都没有就不探 —— 拿组名 `auto` 去探一定被上游 400，探也是白探。
pub fn probe_model(
    conn: &rusqlite::Connection,
    leg: &Leg,
    group: &str,
) -> Option<String> {
    if let Some(a) = leg.model_alias.as_deref() {
        if !a.trim().is_empty() && a != group {
            return Some(a.to_string());
        }
    }
    let row = legs::get(conn, leg.id).ok().flatten()?;
    let _ = group;
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
        EndpointKind::Embeddings => serde_json::json!({
            "model": model,
            "input": "x",
        }),
    };
    bytes::Bytes::from(serde_json::to_vec(&v).unwrap_or_default())
}

/// 「测视觉」用的那张 16×16 小图：四格分别是 左上红 / 右上绿 / 左下蓝 / 右下黄。
///
/// 为什么要造图而不用现成图片：**要能判对错**。问一句"左上角是什么颜色"，
/// 真看了图的模型会答"红"，纯文本模型要么报 400（不支持图片输入），要么答个
/// 别的颜色 —— 三种情况一眼分得清。91 字节，数据 URL 一共 124 个字符。
pub const VISION_TEST_IMAGE_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAIklEQVR42mP4z8CAFTH8JxGNaqBIAy6Z//8ZsKJRDTTRAACmXD7QsJ/a+gAAAABJRU5ErkJggg==";

/// 「测视觉」问题的标准问法 + 判对错时认的词。
pub const VISION_TEST_QUESTION: &str = "这张图左上角那一格是什么颜色？只回一个词。";

/// 「测视觉」的请求体：带图的一发最小对话请求。
///
/// 和探活同一条纪律：模型名是**这条腿自己认的那个**（拿虚拟组名 `auto` 去问
/// 必然 400，绿点就不真了）；`max_tokens` 给 8 够回一个颜色词。
pub fn vision_test_body(model: &str) -> bytes::Bytes {
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
        "max_tokens": 8,
        "stream": false,
    });
    bytes::Bytes::from(serde_json::to_vec(&v).unwrap_or_default())
}

/// 这个回复算不算"真看见了"：认出红色就算。中文简繁、英文都认。
pub fn answer_says_red(reply: &str) -> bool {
    let t = reply.to_lowercase();
    t.contains('红') || t.contains('紅') || t.contains("red")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{legs, open_in_memory};

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
    fn probe_model_prefers_the_alias_for_this_group() {
        let conn = open_in_memory().unwrap();
        let id = legs::create(
            &conn,
            &legs::NewLeg {
                name: "command-go".into(),
                base_url: "https://x/v1".into(),
                enabled: true,
            },
        )
        .unwrap();
        let leg = Leg {
            id,
            name: "command-go".into(),
            base_url: "https://x/v1".into(),
            enabled: true,
            priority: 0,
            model_alias: Some("deepseek/deepseek-v4.1-flash".into()),
            vision: false,
            headers: vec![],
        };
        assert_eq!(
            probe_model(&conn, &leg, "auto").as_deref(),
            Some("deepseek/deepseek-v4.1-flash")
        );
    }

    #[test]
    fn probe_model_falls_back_to_whatever_the_leg_actually_serves() {
        let conn = open_in_memory().unwrap();
        let id = legs::create(
            &conn,
            &legs::NewLeg {
                name: "硅基流动".into(),
                base_url: "https://x/v1".into(),
                enabled: true,
            },
        )
        .unwrap();
        legs::set_models(&conn, id, &[legs::ModelMapping::new("bge-m3", Some("BAAI/bge-m3".into()), false)]).unwrap();
        let leg = Leg {
            id,
            name: "硅基流动".into(),
            base_url: "https://x/v1".into(),
            enabled: true,
            priority: 0,
            model_alias: None,
            vision: false,
            headers: vec![],
        };
        assert_eq!(probe_model(&conn, &leg, "auto").as_deref(), Some("BAAI/bge-m3"));
    }

    /// 反证：什么模型名都没配的腿，**不许拿组名 `auto` 去探**（那一定 400）。
    #[test]
    fn probe_model_refuses_to_guess_the_virtual_name() {
        let conn = open_in_memory().unwrap();
        let id = legs::create(
            &conn,
            &legs::NewLeg {
                name: "空腿".into(),
                base_url: "https://x/v1".into(),
                enabled: true,
            },
        )
        .unwrap();
        let leg = Leg {
            id,
            name: "空腿".into(),
            base_url: "https://x/v1".into(),
            enabled: true,
            priority: 0,
            model_alias: Some("auto".into()),
            vision: false,
            headers: vec![],
        };
        assert_eq!(probe_model(&conn, &leg, "auto"), None);
    }

    /// 「测视觉」的请求体：必须真带一张图，模型名是这条腿自己认的那个，
    /// 而且要小（面板按钮点一下不该花掉一堆 token）。
    #[test]
    fn vision_test_body_carries_a_real_image() {
        let b = vision_test_body("glm-4v-flash");
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["model"], "glm-4v-flash");
        assert_eq!(v["max_tokens"], 8);
        assert_eq!(v["stream"], false);
        let parts = v["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "image_url");
        let url = parts[1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "得是内嵌图片：{url}");
        assert!(url.len() < 400, "别把请求撑大：{} 字符", url.len());
        assert!(b.len() < 700, "整个请求体要小：{} 字节", b.len());
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
