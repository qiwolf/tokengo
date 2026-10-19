//! Optional per-logical-model overrides. Existing names inherit global behavior.
use super::{settings, DbError, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokengo_shared::routing::RoutingConfig;
use tokengo_shared::{normalize_model_name, Channel, ChannelId};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutePolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_ids: Option<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_times: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reprobe_interval_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_in_auto: Option<bool>,
}

fn key(model: &str) -> String {
    format!("model_routing:{}", normalize_model_name(model))
}

pub fn load(conn: &Connection, model: &str) -> Result<RoutePolicy> {
    match settings::get(conn, &key(model))? {
        None => Ok(RoutePolicy::default()),
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|_| DbError::Other("Invalid persisted model routing policy".into())),
    }
}

pub fn save(conn: &Connection, model: &str, policy: &RoutePolicy) -> Result<()> {
    if policy == &RoutePolicy::default() {
        return delete(conn, model);
    }
    let raw = serde_json::to_string(policy).map_err(|e| DbError::Other(e.to_string()))?;
    settings::set(conn, &key(model), &raw)
}

pub fn delete(conn: &Connection, model: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key=?1", [key(model)])?;
    Ok(())
}

pub fn effective(conn: &Connection, model: &str, mut cfg: RoutingConfig) -> Result<RoutingConfig> {
    let policy = load(conn, model)?;
    if let Some(v) = policy.retry_times {
        cfg.retry_times = v;
    }
    if let Some(v) = policy.reprobe_interval_ms {
        cfg.reprobe_interval_ms = v;
    }
    if let Some(v) = policy.probe_enabled {
        cfg.probe_enabled = v;
    }
    Ok(cfg)
}

pub fn order_ids(policy: &RoutePolicy, ids: &mut Vec<ChannelId>) {
    if let Some(order) = &policy.channel_ids {
        let existing = ids.clone();
        *ids = order
            .iter()
            .map(|id| ChannelId(*id))
            .filter(|id| existing.contains(id))
            .collect();
    }
}

pub fn order_channels(policy: &RoutePolicy, channels: &mut Vec<Channel>) {
    if let Some(order) = &policy.channel_ids {
        channels.retain(|c| order.contains(&c.id.0));
        channels.sort_by_key(|c| order.iter().position(|id| *id == c.id.0));
        for (priority, channel) in channels.iter_mut().enumerate() {
            channel.priority = priority as i32;
        }
    }
}
