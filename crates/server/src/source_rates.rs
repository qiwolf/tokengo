//! Local conservative request-rate policy, grouped by explicit provider account.
use crate::search_tools::ToolKind;
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct SourceRates(Mutex<HashMap<(String, bool), VecDeque<Instant>>>);
impl SourceRates {
    pub fn admit(&self, account: &str, kind: ToolKind, qps: u32) -> bool {
        if account.is_empty() || qps == 0 {
            return false;
        }
        let now = Instant::now();
        let mut windows = self.0.lock().unwrap_or_else(|e| e.into_inner());
        windows.retain(|_, times| {
            times
                .back()
                .is_some_and(|last| now.duration_since(*last) < Duration::from_secs(1))
        });
        let key = (account.to_string(), kind == ToolKind::Search);
        if windows.len() >= 4096 && !windows.contains_key(&key) {
            return false;
        }
        let times = windows.entry(key).or_default();
        while times
            .front()
            .is_some_and(|first| now.duration_since(*first) >= Duration::from_secs(1))
        {
            times.pop_front();
        }
        if times.len() >= qps as usize {
            return false;
        }
        times.push_back(now);
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_account_shares_rate_but_tool_and_other_account_have_separate_windows() {
        let rates = SourceRates::default();
        assert!(rates.admit("a", ToolKind::Search, 1));
        assert!(!rates.admit("a", ToolKind::Search, 1));
        assert!(rates.admit("a", ToolKind::Fetch, 1));
        assert!(rates.admit("b", ToolKind::Search, 1));
    }
}
