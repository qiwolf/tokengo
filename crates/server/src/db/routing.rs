//! 路由状态的持久化。
//!
//! 为什么必须落库：路由状态（当前用哪条渠道、什么时候回头试首选）重启就丢的话，
//! 一次重启会让网关立刻回戳那条刚被判定坏掉的首选渠道 —— 而它的 10 分钟回切间隔
//! 正是设计里防这件事的机制。

use rusqlite::{params, Connection, OptionalExtension};

use tokengo_shared::routing::{RouteState, RoutingConfig};
use tokengo_shared::ChannelId;

use super::Result;

pub fn load_state(conn: &Connection, model: &str) -> Result<Option<RouteState>> {
    let row: Option<(Option<i64>, Option<i64>, i64, Option<i64>)> = conn
        .query_row(
            "SELECT active_channel_id, next_probe_at_ms, switch_count, last_switch_at_ms
               FROM routing_state WHERE group_name = ?1",
            [model],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(row.map(|(active, probe, switches, last)| RouteState {
        active: active.map(ChannelId),
        next_probe_at_ms: probe,
        switch_count: switches as u32,
        last_switch_at_ms: last,
    }))
}

/// 覆盖写一个模型名的状态。路由每次变更后调一次。
pub fn save_state(conn: &Connection, model: &str, st: &RouteState) -> Result<()> {
    conn.execute(
        "INSERT INTO routing_state
             (group_name, active_channel_id, next_probe_at_ms, switch_count, last_switch_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(group_name) DO UPDATE SET
             active_channel_id     = excluded.active_channel_id,
             next_probe_at_ms  = excluded.next_probe_at_ms,
             switch_count      = excluded.switch_count,
             last_switch_at_ms = excluded.last_switch_at_ms",
        params![
            model,
            st.active.map(|l| l.0),
            st.next_probe_at_ms,
            st.switch_count as i64,
            st.last_switch_at_ms
        ],
    )?;
    Ok(())
}

/// 路由参数存 settings 表，改完立即生效、重启不丢。
const CFG_KEY: &str = "routing_config";

pub fn load_config(conn: &Connection) -> Result<RoutingConfig> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [CFG_KEY], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(match raw {
        Some(s) => serde_json::from_str(&s).unwrap_or_else(|_| {
            // 库里的值坏了不能让服务起不来，退回默认值并留痕
            tracing::warn!("settings.{CFG_KEY} 解析失败，改回默认路由参数");
            RoutingConfig::default()
        }),
        None => RoutingConfig::default(),
    })
}

pub fn save_config(conn: &Connection, cfg: &RoutingConfig) -> Result<()> {
    let json = serde_json::to_string(cfg)
        .map_err(|e| super::DbError::Other(format!("路由参数序列化失败: {e}")))?;
    super::settings::set(conn, CFG_KEY, &json)
}

/// 把所有模型名的状态灌进 Router。启动时调一次。
pub fn load_all_into(conn: &Connection, router: &mut tokengo_shared::routing::Router) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT DISTINCT group_name FROM routing_state")?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut n = 0;
    for name in &names {
        if let Some(st) = load_state(conn, name)? {
            router.restore(name, st);
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use tokengo_shared::routing::Router;

    #[test]
    fn state_round_trips_exactly() {
        let conn = open_in_memory().unwrap();
        let st = RouteState {
            active: Some(ChannelId(47)),
            next_probe_at_ms: Some(1_800_000_600_000),
            switch_count: 3,
            last_switch_at_ms: Some(1_800_000_000_000),
        };
        save_state(&conn, "auto", &st).unwrap();
        let got = load_state(&conn, "auto").unwrap().unwrap();
        assert_eq!(got.active, st.active);
        assert_eq!(got.next_probe_at_ms, st.next_probe_at_ms);
        assert_eq!(got.switch_count, st.switch_count);
        assert_eq!(got.last_switch_at_ms, st.last_switch_at_ms);
    }

    #[test]
    fn unknown_group_has_no_state() {
        let conn = open_in_memory().unwrap();
        assert!(load_state(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn save_overwrites_not_appends() {
        let conn = open_in_memory().unwrap();
        save_state(&conn, "auto", &RouteState::default()).unwrap();
        save_state(
            &conn,
            "auto",
            &RouteState {
                active: Some(ChannelId(2)),
                ..Default::default()
            },
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM routing_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(load_state(&conn, "auto").unwrap().unwrap().active, Some(ChannelId(2)));
    }

    /// 这条是重启韧性的核心：状态落库后新进程能原样接上，
    /// 不会重启就去戳那条已经坏掉的首选渠道。
    #[test]
    fn a_fresh_router_restores_the_switched_state_after_restart() {
        let conn = open_in_memory().unwrap();
        let channels = [ChannelId(44), ChannelId(47), ChannelId(51)];
        let t0 = 1_800_000_000_000i64;

        let mut before = Router::new(RoutingConfig::default());
        before.attempt_order("auto", &channels, t0);
        before.on_channel_exhausted("auto", ChannelId(44), &channels, t0);
        save_state(&conn, "auto", before.state("auto").unwrap()).unwrap();

        // 重启：全新 Router，只有库里的状态
        let mut after = Router::new(RoutingConfig::default());
        assert_eq!(load_all_into(&conn, &mut after).unwrap(), 1);
        assert_eq!(after.state("auto").unwrap().active, Some(ChannelId(47)));
        assert_eq!(
            after.attempt_order("auto", &channels, t0 + 60_000)[0],
            ChannelId(47),
            "重启后不该立刻回戳坏掉的首选"
        );
        // 但 10 分钟到了照样会去试首选
        assert_eq!(
            after.attempt_order("auto", &channels, t0 + 600_000)[0],
            ChannelId(44)
        );
    }

    #[test]
    fn routing_config_round_trips() {
        let conn = open_in_memory().unwrap();
        assert_eq!(load_config(&conn).unwrap().retry_times, 3, "默认值");

        let cfg = RoutingConfig {
            retry_times: 5,
            reprobe_interval_ms: 60_000,
            probe_enabled: false,
            ..Default::default()
        };
        save_config(&conn, &cfg).unwrap();
        let got = load_config(&conn).unwrap();
        assert_eq!(got.retry_times, 5);
        assert_eq!(got.reprobe_interval_ms, 60_000);
        assert!(!got.probe_enabled);
    }

    #[test]
    fn corrupt_config_falls_back_instead_of_crashing() {
        let conn = open_in_memory().unwrap();
        crate::db::settings::set(&conn, CFG_KEY, "{这不是合法 JSON").unwrap();
        assert_eq!(load_config(&conn).unwrap().retry_times, 3, "坏值必须能起得来");
    }
}
