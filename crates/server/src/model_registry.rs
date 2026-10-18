//! Nominal models.dev metadata. Never a channel-capacity measurement or routing limit.
use crate::{
    db::{self, channels, now_ms},
    SharedState,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::BTreeMap, time::Duration};

const SOURCES: &[&str] = &[
    "https://models.dev/api.json",
    "https://models.dev/_api.json",
];
const MAX_BYTES: usize = 12 * 1024 * 1024;
const MAX_KEYS: usize = 100_000;
const DAY: i64 = 86_400_000;
const MAX_AGE: i64 = 30 * DAY;

#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub tokens: Option<i64>,
    pub reason: String,
}

/// Strict case-sensitive registry identifiers only; no family, suffix or fuzzy matching.
pub fn parse(body: &[u8]) -> Result<BTreeMap<String, Context>, String> {
    if body.len() > MAX_BYTES {
        return Err("registry exceeds byte limit".into());
    }
    let root: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let providers = root
        .as_object()
        .ok_or("registry is not a provider object")?;
    let mut out: BTreeMap<String, Context> = BTreeMap::new();
    for (provider, value) in providers {
        let Some(models) = value.get("models").and_then(|v| v.as_object()) else {
            continue;
        };
        for (key, model) in models {
            let id = model.get("id").and_then(|v| v.as_str()).unwrap_or(key);
            let tokens = model
                .pointer("/limit/context")
                .and_then(|v| v.as_i64())
                .filter(|n| *n > 0);
            let mut ids = vec![id.to_owned(), format!("{provider}/{id}")];
            if let Some(canonical) = model.get("canonical_model_id").and_then(|v| v.as_str()) {
                ids.push(canonical.to_owned());
            }
            for id in ids {
                if id.is_empty() || id.len() > 512 {
                    continue;
                }
                let candidate = Context {
                    tokens,
                    reason: if tokens.is_some() {
                        "精确登记 ID；仅标称上下文，渠道容量未验证"
                    } else {
                        "登记记录没有上下文长度"
                    }
                    .into(),
                };
                out.entry(id).and_modify(|old| {
                    if old != &candidate {
                        *old = Context { tokens: None, reason: "登记 ID 有歧义：各提供方上下文冲突或缺失".into() };
                    }
                }).or_insert(candidate);
                if out.len() > MAX_KEYS {
                    return Err("registry exceeds identifier limit".into());
                }
            }
        }
    }
    if out.is_empty() {
        return Err("no recognized models.dev provider models".into());
    }
    Ok(out)
}

pub fn upstream(mapping: &channels::ModelMapping) -> &str {
    mapping.upstream.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&mapping.model)
}

pub fn context_override(conn: &Connection, channel: tokengo_shared::ChannelId, mapping: &channels::ModelMapping) -> db::Result<Option<i64>> {
    Ok(conn.query_row("SELECT context_window FROM channel_context_overrides WHERE channel_id=?1 AND upstream_model=?2",
        params![channel.0, upstream(mapping)], |r| r.get(0)).optional()?)
}

pub fn associated(conn: &Connection, channel: tokengo_shared::ChannelId, mapping: &channels::ModelMapping) -> db::Result<Option<String>> {
    Ok(conn.query_row("SELECT model_id FROM registry_associations WHERE channel_id=?1 AND model_name=?2 AND upstream_model=?3",
        params![channel.0, mapping.model, upstream(mapping)], |r| r.get(0)).optional()?)
}

pub fn mapped(conn: &Connection, channel: tokengo_shared::ChannelId, mapping: &channels::ModelMapping) -> db::Result<Context> {
    let identity = associated(conn, channel, mapping)?;
    let name = identity.as_deref().unwrap_or_else(|| upstream(mapping));
    let (at, error): (i64, String) = conn.query_row(
        "SELECT fetched_at,error FROM registry_context_status WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if at == 0 || now_ms().saturating_sub(at) > MAX_AGE {
        return Ok(Context {
            tokens: None,
            reason: format!(
                "models.dev {}{}",
                if at == 0 {
                    "缓存尚未获取"
                } else {
                    "缓存已过期"
                },
                if error.is_empty() {
                    String::new()
                } else {
                    format!(": {error}")
                }
            ),
        });
    }
    Ok(conn
        .query_row(
            "SELECT context_window,reason FROM registry_context WHERE model_id=?1",
            [name],
            |r| {
                Ok(Context {
                    tokens: r.get(0)?,
                    reason: r.get(1)?,
                })
            },
        )
        .optional()?
        .unwrap_or(Context {
            tokens: None,
            reason: format!("{name} 没有精确 models.dev ID；不会推测自定义别名"),
        }))
}

async fn download(
    client: &reqwest::Client,
    url: &str,
) -> Result<BTreeMap<String, Context>, String> {
    let mut response = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| "registry transport failure".to_string())?;
    if !response.status().is_success() {
        return Err(format!("registry HTTP {}", response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("registry exceeds byte limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "registry body read failed")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err("registry exceeds byte limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    parse(&body)
}

pub async fn tick(state: &SharedState) -> db::Result<()> {
    refresh(state, SOURCES).await
}

/// Sources are supplied by code, not by a panel/user URL (no new SSRF surface).
pub async fn refresh(state: &SharedState, sources: &[&str]) -> db::Result<()> {
    let tries = {
        let conn = state.conn();
        // Avoid registry traffic until at least one usable mapping lacks upstream context.
        let needed = channels::list(&conn)?
            .into_iter()
            .filter(|c| c.enabled && !c.base_url.trim().is_empty())
            .any(|c| {
                db::catalog::list(&conn, c.id)
                    .map(|entries| {
                        c.models.iter().any(|m| {
                            crate::capability_detector::mapped_context(&entries, m).is_none()
                        })
                    })
                    .unwrap_or(false)
            });
        if !needed {
            return Ok(());
        }
        let (next, tries): (i64, i64) = conn.query_row(
            "SELECT next_at,tries FROM registry_context_status WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if next > now_ms() {
            return Ok(());
        }
        conn.execute(
            "UPDATE registry_context_status SET next_at=?1 WHERE id=1",
            [now_ms() + 60_000],
        )?;
        tries
    };
    let client = reqwest::Client::builder()
        .user_agent("tokengo models.dev context metadata")
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| db::DbError::Other(e.to_string()))?;
    let mut error = "no registry source".to_string();
    // At most two downloads per refresh, each bounded by size and total time.
    for url in sources.iter().take(2) {
        match download(&client, url).await {
            Ok(entries) => {
                let conn = state.conn();
                return db::in_tx(&conn, |conn| {
                    conn.execute("DELETE FROM registry_context", [])?;
                    let mut stmt = conn.prepare("INSERT INTO registry_context VALUES(?1,?2,?3)")?;
                    for (id, context) in entries {
                        stmt.execute(params![id, context.tokens, context.reason])?;
                    }
                    conn.execute("UPDATE registry_context_status SET fetched_at=?1,next_at=?2,tries=0,origin=?3,error='' WHERE id=1", params![now_ms(),now_ms()+DAY,url])?;
                    Ok(())
                });
            }
            Err(e) => error = e,
        }
    }
    let delay = match tries {
        0 => 60_000,
        1 => 300_000,
        2 => 1_800_000,
        _ => 21_600_000,
    };
    state.conn().execute(
        "UPDATE registry_context_status SET next_at=?1,tries=?2,error=?3 WHERE id=1",
        params![now_ms() + delay, tries.saturating_add(1), error],
    )?;
    Ok(())
}
