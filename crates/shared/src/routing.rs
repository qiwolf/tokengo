//! 路由状态机。**纯逻辑，不碰网络**，时间由调用方传进来（unix 毫秒），所以能完整测。
//!
//! 与现网 FreeLLMAPI 的差别（见 `docs/03-路由策略.md`）：
//! - **没有冷却**。腿失败不打冷却标记，失败就重试，重试耗尽才切走。
//! - **固定优先级**，不做 `balanced` 打分和随机探索，同一个请求结果可预测。
//! - 切走之后 **10 分钟回头试首选**，成功就切回来；不是"冷却到期"才回来。
//! - **客户端自己的错（400/404/422）不计入失败**，否则一次写错的请求会把好腿打坏。
//!
//! 重试与切换的分工：状态机只管"该按什么顺序试哪些腿、什么时候回头试首选"；
//! "同一条腿重试几次"由调用方按 `retry_times` 循环，耗尽后调 `on_leg_exhausted`。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::leg::LegId;

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
    /// 一条腿最多被尝试几次（首次 + 重试）。
    pub fn max_attempts(&self) -> u32 {
        1 + self.retry_times
    }
}

/// 一次尝试的结果分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    Success,
    /// 腿自己的毛病：网络错 / 5xx / 429 / 401 / 403。计入重试与切换。
    LegFault,
    /// 客户端自己的毛病：400 / 404 / 422 等。原样返回，**不重试不切换**。
    ClientFault,
    /// 客户端主动断开。不算腿失败。
    ClientGone,
}

/// HTTP 状态码归类。这一条上一轮踩过：把 4xx 计进冷却，客户端连发几次错模型名
/// 就能把唯一一条向量腿打进冷却，真流量全断。
pub fn classify_status(status: u16) -> AttemptOutcome {
    if (200..300).contains(&status) {
        AttemptOutcome::Success
    } else if status >= 500 || status == 429 || status == 401 || status == 403 {
        AttemptOutcome::LegFault
    } else {
        AttemptOutcome::ClientFault
    }
}

/// 一个模型组的路由状态。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupState {
    pub active: Option<LegId>,
    /// 回切检查时间点；为空表示"首选就绪，无需回切"。
    pub next_probe_at_ms: Option<i64>,
    pub switch_count: u32,
    pub last_switch_at_ms: Option<i64>,
}

impl GroupState {
    pub fn probe_due(&self, now_ms: i64) -> bool {
        matches!(self.next_probe_at_ms, Some(t) if t <= now_ms)
    }
}

/// 全部模型组的路由状态。
#[derive(Debug, Default)]
pub struct Router {
    pub cfg: RoutingConfig,
    states: HashMap<String, GroupState>,
}

impl Router {
    pub fn new(cfg: RoutingConfig) -> Self {
        Self {
            cfg,
            states: HashMap::new(),
        }
    }

    pub fn state(&self, group: &str) -> Option<&GroupState> {
        self.states.get(group)
    }

    pub fn all_states(&self) -> &HashMap<String, GroupState> {
        &self.states
    }

    /// 从库里恢复一组的状态。进程启动时按组调一遍，之后每次状态变更都要写回库
    /// —— 否则重启后"当前腿"和"回切检查点"就丢了，会立刻回戳刚坏掉的首选。
    pub fn restore(&mut self, group: &str, state: GroupState) {
        self.states.insert(group.to_owned(), state);
    }

    fn entry(&mut self, group: &str) -> &mut GroupState {
        self.states.entry(group.to_owned()).or_default()
    }

    /// 本请求该按什么顺序试哪些腿。`legs` 必须是**已过滤掉停用/无 base_url**、
    /// 且已按优先级排好序的腿列表。
    ///
    /// - 常规：`[active, 其余按优先级]`
    /// - 回切检查点到点：`[primary, active, 其余按优先级]`
    pub fn attempt_order(&mut self, group: &str, legs: &[LegId], now_ms: i64) -> Vec<LegId> {
        let primary = legs.first().copied();
        let probe_due = {
            let st = self.entry(group);
            // active 丢了（腿被删/停用）或从没定过 → 回到首选
            if st.active.is_none_or(|a| !legs.contains(&a)) {
                st.active = primary;
            }
            st.probe_due(now_ms)
        };
        let active = self.states.get(group).and_then(|s| s.active);

        let mut order: Vec<LegId> = Vec::with_capacity(legs.len() + 1);
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
        for l in legs {
            if !order.contains(l) {
                order.push(*l);
            }
        }
        order
    }

    /// 某条腿上的尝试成功了。
    ///
    /// - 命中的是首选 → 视为已回切：清掉回切检查点。**这是"10 分钟后重试成功就
    ///   切换回第一个选择的模型"的落点。**
    /// - 命中的不是首选 → 记下它是当前腿；若与之前不同则计一次切换并重设检查点。
    pub fn on_success(&mut self, group: &str, leg: LegId, legs: &[LegId], now_ms: i64) {
        let primary = legs.first().copied();
        let interval = self.cfg.reprobe_interval_ms;
        let st = self.entry(group);
        let was = st.active;
        st.active = Some(leg);

        if Some(leg) == primary {
            st.next_probe_at_ms = None;
        } else if was != Some(leg) {
            st.next_probe_at_ms = Some(now_ms + interval);
        }

        if was.is_some() && was != Some(leg) {
            st.switch_count += 1;
            st.last_switch_at_ms = Some(now_ms);
        }
    }

    /// 一条腿把重试次数用完了，仍然失败。
    ///
    /// - 它正是当前腿 → 切到下一条可用腿；没有下一条就原地留着（唯一的腿没得选）。
    /// - 它正是首选、且此刻是回切检查点 → 把检查点往后推一个间隔。
    ///   （不加这一条会出现"每次请求都去戳已经坏掉的首选"的忙探活。）
    pub fn on_leg_exhausted(&mut self, group: &str, leg: LegId, legs: &[LegId], now_ms: i64) {
        let primary = legs.first().copied();
        let interval = self.cfg.reprobe_interval_ms;
        let st = self.entry(group);

        if st.active == Some(leg) {
            let next = legs
                .iter()
                .position(|l| *l == leg)
                .and_then(|i| legs.get(i + 1))
                .copied();
            match next {
                Some(n) => {
                    st.active = Some(n);
                    st.next_probe_at_ms = Some(now_ms + interval);
                    st.switch_count += 1;
                    st.last_switch_at_ms = Some(now_ms);
                }
                None => {
                    // 没有别的腿可选，留在原地，下次请求再试它
                    st.active = Some(leg);
                }
            }
        }

        if st.probe_due(now_ms) && Some(leg) == primary {
            st.next_probe_at_ms = Some(now_ms + interval);
        }
    }

    /// 后台探活要处理哪些组（检查点到点的）。
    pub fn groups_needing_probe(&self, now_ms: i64) -> Vec<String> {
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

    const LEGS: [LegId; 3] = [LegId(44), LegId(47), LegId(51)];
    const T0: i64 = 1_800_000_000_000;
    const TEN_MIN: i64 = 10 * 60 * 1000;

    fn router() -> Router {
        Router::new(RoutingConfig::default())
    }

    #[test]
    fn attempts_are_first_plus_retries() {
        assert_eq!(RoutingConfig::default().max_attempts(), 4);
    }

    #[test]
    fn status_classification_matches_the_rule() {
        assert_eq!(classify_status(200), AttemptOutcome::Success);
        assert_eq!(classify_status(500), AttemptOutcome::LegFault);
        assert_eq!(classify_status(502), AttemptOutcome::LegFault);
        assert_eq!(classify_status(429), AttemptOutcome::LegFault);
        assert_eq!(classify_status(401), AttemptOutcome::LegFault);
        assert_eq!(classify_status(403), AttemptOutcome::LegFault);
        // 客户端自己的错，不能把腿打坏
        assert_eq!(classify_status(400), AttemptOutcome::ClientFault);
        assert_eq!(classify_status(404), AttemptOutcome::ClientFault);
        assert_eq!(classify_status(422), AttemptOutcome::ClientFault);
    }

    #[test]
    fn starts_on_primary_then_falls_back_in_priority_order() {
        let mut r = router();
        assert_eq!(r.attempt_order("auto", &LEGS, T0), LEGS.to_vec());
    }

    #[test]
    fn exhausted_active_leg_switches_and_schedules_a_probe() {
        let mut r = router();
        r.attempt_order("auto", &LEGS, T0);
        r.on_leg_exhausted("auto", LegId(44), &LEGS, T0);

        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(LegId(47)), "切到第二优先");
        assert_eq!(st.next_probe_at_ms, Some(T0 + TEN_MIN));
        assert_eq!(st.switch_count, 1);
        // 下一个请求先用第二优先，不再戳坏掉的首选
        assert_eq!(
            r.attempt_order("auto", &LEGS, T0 + 1000)[0],
            LegId(47),
            "检查点没过期前不该再试首选"
        );
    }

    #[test]
    fn after_ten_minutes_the_primary_is_probed_first() {
        let mut r = router();
        r.attempt_order("auto", &LEGS, T0);
        r.on_leg_exhausted("auto", LegId(44), &LEGS, T0);

        let order = r.attempt_order("auto", &LEGS, T0 + TEN_MIN);
        assert_eq!(order[0], LegId(44), "到点了先试首选");
        assert_eq!(order[1], LegId(47), "首选不行就还回当前腿");
    }

    #[test]
    fn probe_success_switches_back_to_primary() {
        let mut r = router();
        r.attempt_order("auto", &LEGS, T0);
        r.on_leg_exhausted("auto", LegId(44), &LEGS, T0);

        // 10 分钟后探活首选，成功
        r.attempt_order("auto", &LEGS, T0 + TEN_MIN);
        r.on_success("auto", LegId(44), &LEGS, T0 + TEN_MIN + 5);

        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(LegId(44)), "回切到第一个选择的模型");
        assert_eq!(st.next_probe_at_ms, None, "回切后清掉检查点");
        assert_eq!(st.switch_count, 2, "切走一次、切回来一次");
        assert_eq!(r.groups_needing_probe(T0 + TEN_MIN), Vec::<String>::new());
    }

    #[test]
    fn probe_failure_pushes_the_checkpoint_forward() {
        let mut r = router();
        r.attempt_order("auto", &LEGS, T0);
        r.on_leg_exhausted("auto", LegId(44), &LEGS, T0);
        // 10 分钟后探活首选，仍然坏
        r.attempt_order("auto", &LEGS, T0 + TEN_MIN);
        r.on_leg_exhausted("auto", LegId(44), &LEGS, T0 + TEN_MIN);

        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(LegId(47)), "留在当前腿");
        assert_eq!(
            st.next_probe_at_ms,
            Some(T0 + TEN_MIN * 2),
            "检查点往后推一个间隔"
        );
        // 关键：不会每次请求都去戳坏掉的首选
        assert_eq!(r.attempt_order("auto", &LEGS, T0 + TEN_MIN + 1000)[0], LegId(47));
    }

    #[test]
    fn second_leg_failing_too_moves_on_to_the_third() {
        let mut r = router();
        r.attempt_order("auto", &LEGS, T0);
        r.on_leg_exhausted("auto", LegId(44), &LEGS, T0);
        r.on_leg_exhausted("auto", LegId(47), &LEGS, T0);
        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(LegId(51)));
        assert_eq!(st.switch_count, 2);
    }

    #[test]
    fn a_client_error_never_touches_routing_state() {
        // 连发 6 次错模型名（400），腿必须原地不动 —— 反证用例
        let mut r = router();
        r.attempt_order("auto", &LEGS, T0);
        for _ in 0..6 {
            assert_eq!(classify_status(400), AttemptOutcome::ClientFault);
        }
        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(LegId(44)));
        assert_eq!(st.switch_count, 0);
        assert_eq!(st.next_probe_at_ms, None);
    }

    #[test]
    fn single_leg_group_stays_put_when_it_fails() {
        let mut r = router();
        let one = [LegId(44)];
        r.attempt_order("auto", &one, T0);
        r.on_leg_exhausted("auto", LegId(44), &one, T0);
        let st = r.state("auto").unwrap();
        assert_eq!(st.active, Some(LegId(44)), "没有别的腿可选，原地留着");
        assert_eq!(st.switch_count, 0);
    }

    #[test]
    fn removing_the_active_leg_falls_back_to_primary() {
        let mut r = router();
        r.attempt_order("auto", &LEGS, T0);
        r.on_leg_exhausted("auto", LegId(44), &LEGS, T0);
        assert_eq!(r.state("auto").unwrap().active, Some(LegId(47)));

        // 47 被停用/删除，只剩 44 和 51
        let left = [LegId(44), LegId(51)];
        assert_eq!(r.attempt_order("auto", &left, T0 + 1000)[0], LegId(44));
    }

    #[test]
    fn each_group_keeps_its_own_state() {
        let mut r = router();
        r.attempt_order("auto", &LEGS, T0);
        r.on_leg_exhausted("auto", LegId(44), &LEGS, T0);
        let embed = [LegId(44)];
        assert_eq!(r.attempt_order("bge-m3", &embed, T0)[0], LegId(44));
        assert_eq!(r.state("bge-m3").unwrap().active, Some(LegId(44)));
        assert_eq!(r.state("auto").unwrap().active, Some(LegId(47)));
    }
}
