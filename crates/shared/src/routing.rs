//! 路由状态机。**纯逻辑，不碰网络**，时间由调用方传进来（unix 毫秒），所以能完整测。
//!
//! 与现网 FreeLLMAPI 的差别（见 `docs/03-路由策略.md`）：
//! - **没有冷却**。渠道失败不打冷却标记，失败就重试，重试耗尽才切走。
//! - **固定优先级**，不做 `balanced` 打分和随机探索，同一个请求结果可预测。
//! - 切走之后 **10 分钟回头试首选**，成功就切回来；不是"冷却到期"才回来。
//! - **客户端自己的错（400/404/422）不计入失败**，否则一次写错的请求会把好渠道打坏。
//!
//! 重试与切换的分工：状态机只管"该按什么顺序试哪些渠道、什么时候回头试首选"；
//! "同一条渠道重试几次"由调用方按 `retry_times` 循环，耗尽后调 `on_channel_exhausted`。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::channel::ChannelId;

/// 可调参数。面板可改，改完立即生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// 首次失败后再重试几次。用户原话「重试3次都失败就自动切换」→ 总尝试数 = 1 + 3。
    pub retry_times: u32,
    /// 单次尝试超时。
    pub attempt_timeout_ms: u64,
    /// 整个请求的重试总预算，超了不再重试。
    pub request_budget_ms: u64,
    /// 切换后隔多久回头试首选。
    pub reprobe_interval_ms: i64,
    /// 无流量时是否后台主动探活首选。
    pub probe_enabled: bool,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            retry_times: 3,
            attempt_timeout_ms: 30_000,
            request_budget_ms: 90_000,
            reprobe_interval_ms: 10 * 60 * 1000,
            probe_enabled: true,
        }
    }
}

impl RoutingConfig {
    /// 一条渠道最多被尝试几次（首次 + 重试）。
    pub fn max_attempts(&self) -> u32 {
        1 + self.retry_times
    }
}

/// 第 `next_attempt` 次尝试**之前**要等多久（毫秒）。`next_attempt` 从 1 起。
///
/// 前两次尝试之间不等待（上游偶尔抖一下就过去了），第 4 次尝试之前等 1 秒 ——
/// 上游真挂了的时候，不加等待会四条渠道一起挤上去，把已经过载的上游再压一层。
/// 见 `docs/03-路由策略.md` 第三节。
pub fn retry_delay_ms(next_attempt: u32) -> u64 {
    if next_attempt >= 4 {
        1000
    } else {
        0
    }
}

/// 一次尝试的结果分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    Success,
    /// 渠道自己的毛病：网络错 / 5xx / 429 / 401 / 403。计入重试与切换。
    ChannelFault,
    /// 客户端自己的毛病：400 / 404 / 422 等。原样返回，**不重试不切换**。
    ClientFault,
    /// 客户端主动断开。不算渠道失败。
    ClientGone,
}

/// HTTP 状态码归类。这一条上一轮踩过：把 4xx 计进冷却，客户端连发几次错模型名
/// 就能把唯一一条向量渠道打进冷却，真流量全断。
pub fn classify_status(status: u16) -> AttemptOutcome {
    if (200..300).contains(&status) {
        AttemptOutcome::Success
    } else if status >= 500 || status == 429 || status == 401 || status == 403 {
        AttemptOutcome::ChannelFault
    } else {
        AttemptOutcome::ClientFault
    }
}

/// 一个模型名的路由状态。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouteState {
    pub active: Option<ChannelId>,
    /// 回切检查时间点；为空表示"首选就绪，无需回切"。
    pub next_probe_at_ms: Option<i64>,
    pub switch_count: u32,
    pub last_switch_at_ms: Option<i64>,
}

impl RouteState {
    pub fn probe_due(&self, now_ms: i64) -> bool {
        matches!(self.next_probe_at_ms, Some(t) if t <= now_ms)
    }
}

/// 全部模型名的路由状态。
#[derive(Debug, Default)]
pub struct Router {
    pub cfg: RoutingConfig,
    states: HashMap<String, RouteState>,
}

impl Router {
    pub fn new(cfg: RoutingConfig) -> Self {
        Self {
            cfg,
            states: HashMap::new(),
        }
    }

    pub fn state(&self, model: &str) -> Option<&RouteState> {
        self.states.get(model)
    }

    pub fn all_states(&self) -> &HashMap<String, RouteState> {
        &self.states
    }

    /// 从库里恢复一组的状态。进程启动时按组调一遍，之后每次状态变更都要写回库
    /// —— 否则重启后"当前渠道"和"回切检查点"就丢了，会立刻回戳刚坏掉的首选。
    pub fn restore(&mut self, model: &str, state: RouteState) {
        self.states.insert(model.to_owned(), state);
    }

    fn entry(&mut self, model: &str) -> &mut RouteState {
        self.states.entry(model.to_owned()).or_default()
    }

    /// 本请求该按什么顺序试哪些渠道。`channels` 必须是**已过滤掉停用/无 base_url**、
    /// 且已按优先级排好序的渠道列表。
    ///
    /// - 常规：`[active, 其余按优先级]`
    /// - 回切检查点到点：`[primary, active, 其余按优先级]`
    pub fn attempt_order(&mut self, model: &str, channels: &[ChannelId], now_ms: i64) -> Vec<ChannelId> {
        let primary = channels.first().copied();
        let probe_due = {
            let st = self.entry(model);
            // active 丢了（渠道被删/停用）或从没定过 → 回到首选
            if st.active.is_none_or(|a| !channels.contains(&a)) {
                st.active = primary;
            }
            st.probe_due(now_ms)
        };
        let active = self.states.get(model).and_then(|s| s.active);

        let mut order: Vec<ChannelId> = Vec::with_capacity(channels.len() + 1);
        if probe_due {
            if let Some(p) = primary {
                order.push(p);
            }
        }
        if let Some(a) = active {
            if !order.contains(&a) {
                order.push(a);
            }
        }
        for l in channels {
            if !order.contains(l) {
                order.push(*l);
            }
        }
        order
    }

    /// 某条渠道上的尝试成功了。
    ///
    /// - 命中的是首选 → 视为已回切：清掉回切检查点。**这是"10 分钟后重试成功就
    ///   切换回第一个选择的模型"的落点。**
    /// - 命中的不是首选 → 记下它是当前渠道；若与之前不同则计一次切换并重设检查点。
    pub fn on_success(&mut self, model: &str, channel: ChannelId, channels: &[ChannelId], now_ms: i64) {
        let primary = channels.first().copied();
        let interval = self.cfg.reprobe_interval_ms;
        let st = self.entry(model);
        let was = st.active;
        st.active = Some(channel);

        if Some(channel) == primary {
            st.next_probe_at_ms = None;
        } else if was != Some(channel) {
            st.next_probe_at_ms = Some(now_ms + interval);
        }

        if was.is_some() && was != Some(channel) {
            st.switch_count += 1;
            st.last_switch_at_ms = Some(now_ms);
        }
    }

    /// 一条渠道把重试次数用完了，仍然失败。
    ///
    /// - 它正是当前渠道 → 切到下一条可用渠道；没有下一条就原地留着（唯一的渠道没得选）。
    /// - 它正是首选、且此刻是回切检查点 → 把检查点往后推一个间隔。
    ///   （不加这一条会出现"每次请求都去戳已经坏掉的首选"的忙探活。）
    pub fn on_channel_exhausted(&mut self, model: &str, channel: ChannelId, channels: &[ChannelId], now_ms: i64) {
        let primary = channels.first().copied();
        let interval = self.cfg.reprobe_interval_ms;
        let st = self.entry(model);

        if st.active == Some(channel) {
            let next = channels
                .iter()
                .position(|l| *l == channel)
                .and_then(|i| channels.get(i + 1))
                .copied();
            match next {
                Some(n) => {
                    st.active = Some(n);
                    st.next_probe_at_ms = Some(now_ms + interval);
                    st.switch_count += 1;
                    st.last_switch_at_ms = Some(now_ms);
                }
                None => {
                    // 没有别的渠道可选，留在原地，下次请求再试它
                    st.active = Some(channel);
                }
            }
        }

        if st.probe_due(now_ms) && Some(channel) == primary {
            st.next_probe_at_ms = Some(now_ms + interval);
        }
    }

    /// 后台探活要处理哪些组（检查点到点的）。
    pub fn models_needing_probe(&self, now_ms: i64) -> Vec<String> {
        let mut v: Vec<String> = self
            .states
            .iter()
            .filter(|(_, s)| s.probe_due(now_ms))
            .map(|(k, _)| k.clone())
            .collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANNELS: [ChannelId; 3] = [ChannelId(44), ChannelId(47), ChannelId(51)];
    const T0: i64 = 1_800_000_000_000;
    const TEN_MIN: i64 = 10 * 60 * 1000;

    fn router() -> Router {
        Router::new(RoutingConfig::default())
    }

    #[test]
    fn attempts_are_first_plus_retries() {
        assert_eq!(RoutingConfig::default().max_attempts(), 4);
    }

    /// 重试节拍：前两次立即，第 4 次尝试前等 1 秒。
    #[test]
    fn retry_pacing_only_waits_before_the_last_attempt() {
        assert_eq!(retry_delay_ms(1), 0, "首次不等待");
        assert_eq!(retry_delay_ms(2), 0);
        assert_eq!(retry_delay_ms(3), 0);
        assert_eq!(retry_delay_ms(4), 1000, "第 4 次之前等 1 秒");
        assert_eq!(retry_delay_ms(5), 1000);
    }

    #[test]
    fn status_classification_matches_the_rule() {
        assert_eq!(classify_status(200), AttemptOutcome::Success);
        assert_eq!(classify_status(500), AttemptOutcome::ChannelFault);
        assert_eq!(classify_status(502), AttemptOutcome::ChannelFault);
        assert_eq!(classify_status(429), AttemptOutcome::ChannelFault);
        assert_eq!(classify_status(401), AttemptOutcome::ChannelFault);
        assert_eq!(classify_status(403), AttemptOutcome::ChannelFault);
        // 客户端自己的错，不能把渠道打坏
        assert_eq!(classify_status(400), AttemptOutcome::ClientFault);
        assert_eq!(classify_status(404), AttemptOutcome::ClientFault);
        assert_eq!(classify_status(422), AttemptOutcome::ClientFault);
    }

    #[test]
    fn starts_on_primary_then_falls_back_in_priority_order() {
        let mut r = router();
        assert_eq!(r.attempt_order("auto", &CHANNELS, T0), CHANNELS.to_vec());
    }

    #[test]
    fn exhausted_active_channel_switches_and_schedules_a_probe() {
        let mut r = router();
        r.attempt_order("auto", &CHANNELS, T0);
        r.on_channel_exhausted("auto", ChannelId(44), &CHANNELS, T0);

        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(ChannelId(47)), "切到第二优先");
        assert_eq!(st.next_probe_at_ms, Some(T0 + TEN_MIN));
        assert_eq!(st.switch_count, 1);
        // 下一个请求先用第二优先，不再戳坏掉的首选
        assert_eq!(
            r.attempt_order("auto", &CHANNELS, T0 + 1000)[0],
            ChannelId(47),
            "检查点没过期前不该再试首选"
        );
    }

    #[test]
    fn after_ten_minutes_the_primary_is_probed_first() {
        let mut r = router();
        r.attempt_order("auto", &CHANNELS, T0);
        r.on_channel_exhausted("auto", ChannelId(44), &CHANNELS, T0);

        let order = r.attempt_order("auto", &CHANNELS, T0 + TEN_MIN);
        assert_eq!(order[0], ChannelId(44), "到点了先试首选");
        assert_eq!(order[1], ChannelId(47), "首选不行就还回当前渠道");
    }

    #[test]
    fn probe_success_switches_back_to_primary() {
        let mut r = router();
        r.attempt_order("auto", &CHANNELS, T0);
        r.on_channel_exhausted("auto", ChannelId(44), &CHANNELS, T0);

        // 10 分钟后探活首选，成功
        r.attempt_order("auto", &CHANNELS, T0 + TEN_MIN);
        r.on_success("auto", ChannelId(44), &CHANNELS, T0 + TEN_MIN + 5);

        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(ChannelId(44)), "回切到第一个选择的模型");
        assert_eq!(st.next_probe_at_ms, None, "回切后清掉检查点");
        assert_eq!(st.switch_count, 2, "切走一次、切回来一次");
        assert_eq!(r.models_needing_probe(T0 + TEN_MIN), Vec::<String>::new());
    }

    #[test]
    fn probe_failure_pushes_the_checkpoint_forward() {
        let mut r = router();
        r.attempt_order("auto", &CHANNELS, T0);
        r.on_channel_exhausted("auto", ChannelId(44), &CHANNELS, T0);
        // 10 分钟后探活首选，仍然坏
        r.attempt_order("auto", &CHANNELS, T0 + TEN_MIN);
        r.on_channel_exhausted("auto", ChannelId(44), &CHANNELS, T0 + TEN_MIN);

        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(ChannelId(47)), "留在当前渠道");
        assert_eq!(
            st.next_probe_at_ms,
            Some(T0 + TEN_MIN * 2),
            "检查点往后推一个间隔"
        );
        // 关键：不会每次请求都去戳坏掉的首选
        assert_eq!(r.attempt_order("auto", &CHANNELS, T0 + TEN_MIN + 1000)[0], ChannelId(47));
    }

    #[test]
    fn second_channel_failing_too_moves_on_to_the_third() {
        let mut r = router();
        r.attempt_order("auto", &CHANNELS, T0);
        r.on_channel_exhausted("auto", ChannelId(44), &CHANNELS, T0);
        r.on_channel_exhausted("auto", ChannelId(47), &CHANNELS, T0);
        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(ChannelId(51)));
        assert_eq!(st.switch_count, 2);
    }

    #[test]
    fn a_client_error_never_touches_routing_state() {
        // 连发 6 次错模型名（400），渠道必须原地不动 —— 反证用例
        let mut r = router();
        r.attempt_order("auto", &CHANNELS, T0);
        for _ in 0..6 {
            assert_eq!(classify_status(400), AttemptOutcome::ClientFault);
        }
        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(ChannelId(44)));
        assert_eq!(st.switch_count, 0);
        assert_eq!(st.next_probe_at_ms, None);
    }

    #[test]
    fn single_channel_group_stays_put_when_it_fails() {
        let mut r = router();
        let one = [ChannelId(44)];
        r.attempt_order("auto", &one, T0);
        r.on_channel_exhausted("auto", ChannelId(44), &one, T0);
        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(ChannelId(44)), "没有别的渠道可选，原地留着");
        assert_eq!(st.switch_count, 0);
    }

    #[test]
    fn removing_the_active_channel_falls_back_to_primary() {
        let mut r = router();
        r.attempt_order("auto", &CHANNELS, T0);
        r.on_channel_exhausted("auto", ChannelId(44), &CHANNELS, T0);
        assert_eq!(r.state("auto").unwrap().active, Some(ChannelId(47)));

        // 47 被停用/删除，只剩 44 和 51
        let left = [ChannelId(44), ChannelId(51)];
        assert_eq!(r.attempt_order("auto", &left, T0 + 1000)[0], ChannelId(44));
    }

    #[test]
    fn each_group_keeps_its_own_state() {
        let mut r = router();
        r.attempt_order("auto", &CHANNELS, T0);
        r.on_channel_exhausted("auto", ChannelId(44), &CHANNELS, T0);
        let embed = [ChannelId(44)];
        assert_eq!(r.attempt_order("bge-m3", &embed, T0)[0], ChannelId(44));
        assert_eq!(r.state("bge-m3").unwrap().active, Some(ChannelId(44)));
        assert_eq!(r.state("auto").unwrap().active, Some(ChannelId(47)));
    }
}
