//! Bounded upstream metadata discovery; never sends inference or changes routing/accounting.
use crate::{
    db::{self, catalog, channels, now_ms},
    SharedState,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    hash::{Hash, Hasher},
    time::Duration,
};
use tokengo_shared::Channel;

// Exclude learned flags: adopting a vision claim must not schedule another fetch.
fn fingerprint(conn: &Connection, row: &channels::ChannelRow) -> db::Result<String> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&row.base_url, row.enabled, &row.headers).hash(&mut h);
    for m in &row.models {
        (&m.model, &m.upstream).hash(&mut h);
    }
    let sealed = conn
        .query_row(
            "SELECT nonce,cipher FROM channel_secrets WHERE channel_id=?1",
            [row.id.0],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;
    sealed.hash(&mut h);
    Ok(format!("{:016x}", h.finish()))
}

pub fn spawn(state: SharedState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(5));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            timer.tick().await;
            if let Err(e) = tick(&state).await {
                tracing::warn!("Capability metadata discovery failed: {e}");
            }
            if let Err(e) = crate::model_registry::tick(&state).await {
                tracing::warn!("Model registry refresh failed: {e}");
            }
        }
    })
}

/// One serialized worker handles at most one channel per tick, outside the DB lock.
pub async fn tick(state: &SharedState) -> db::Result<()> {
    let job = {
        let conn = state.conn();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS channel_capability_checks (
            channel_id INTEGER PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
            fingerprint TEXT NOT NULL, tries INTEGER NOT NULL, next_at INTEGER NOT NULL)",
        )?;
        let mut job = None;
        for row in channels::list(&conn)? {
            if !row.enabled || row.base_url.trim().is_empty() || row.models.is_empty() {
                continue;
            }
            let fp = fingerprint(&conn, &row)?;
            let old = conn.query_row("SELECT fingerprint,tries,next_at FROM channel_capability_checks WHERE channel_id=?1", [row.id.0],
                |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?))).optional()?;
            let tries = match old {
                Some((f, n, at)) if f == fp => {
                    if at > now_ms() {
                        continue;
                    }
                    n
                }
                _ => 0,
            };
            let secret = channels::secret(&conn, &state.secrets, row.id)?;
            // Reserve before releasing the lock; interrupted work can be retried after a minute.
            conn.execute("INSERT INTO channel_capability_checks VALUES(?1,?2,?3,?4)
                ON CONFLICT(channel_id) DO UPDATE SET fingerprint=excluded.fingerprint,tries=excluded.tries,next_at=excluded.next_at",
                params![row.id.0,fp,tries.saturating_add(1),now_ms()+60_000])?;
            let channel = Channel {
                id: row.id,
                name: row.name,
                base_url: row.base_url,
                enabled: true,
                priority: 0,
                max_concurrent: 0,
                model_alias: None,
                vision: false,
                drop_fields: vec![],
                unwrap_envelope: false,
                headers: row.headers,
            };
            job = Some((channel, secret, fp, tries));
            break;
        }
        job
    };
    let Some((channel, secret, fp, tries)) = job else {
        return Ok(());
    };
    let result = state
        .upstream
        .fetch_models(
            &channel,
            secret.as_deref(),
            Duration::from_secs(15),
            &crate::upstream::new_uuid(),
            now_ms(),
        )
        .await;
    let entries = match result {
        Ok((200..=299, body)) => crate::upstream::parse_model_catalog(&body),
        _ => Vec::new(),
    };
    let conn = state.conn();
    let Some(row) = channels::get(&conn, channel.id)? else {
        return Ok(());
    };
    // Includes sealed credentials and model mappings, so an old request cannot overwrite new config.
    if !row.enabled || fingerprint(&conn, &row)? != fp {
        return Ok(());
    }
    let success = !entries.is_empty();
    let delay = if success {
        24 * 60 * 60 * 1000
    } else {
        match tries {
            0 => 60_000,
            1 => 5 * 60_000,
            2 => 30 * 60_000,
            _ => 6 * 60 * 60_000,
        }
    };
    db::in_tx(&conn, |conn| {
        if success {
            catalog::replace(conn, channel.id, &entries, now_ms())?;
        }
        conn.execute(
            "UPDATE channel_capability_checks SET tries=?2,next_at=?3 WHERE channel_id=?1",
            params![
                channel.id.0,
                if success { 0 } else { tries.saturating_add(1) },
                now_ms() + delay
            ],
        )?;
        Ok(())
    })
}

/// Explicit upstream context for the actual mapped name, never inferred from identifiers.
pub fn mapped_context(
    entries: &[catalog::CatalogEntry],
    mapping: &channels::ModelMapping,
) -> Option<i64> {
    let name = mapping
        .upstream
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&mapping.model);
    entries
        .iter()
        .find(|e| tokengo_shared::same_model_name(&e.model, name))
        .and_then(|e| e.context_window)
        .filter(|n| *n > 0)
}

/// A limit is safe to advertise only when all eligible targets report one.
pub fn conservative_context(values: impl IntoIterator<Item = Option<i64>>) -> Option<i64> {
    let values = values.into_iter().collect::<Option<Vec<_>>>()?;
    values.into_iter().min()
}
