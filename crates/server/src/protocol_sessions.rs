//! Key-scoped ownership for opaque native histories; no prompt or provider state is stored.
use crate::{db, protocol::Protocol};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub channel: i64,
    pub generation: i64,
    pub upstream_model: String,
    pub protocol: Protocol,
}
pub fn key(namespace: &str, session: &str, model: &str, protocol: Protocol) -> String {
    format!(
        "protocol_session:{}",
        tokengo_shared::short_hash(
            &serde_json::json!([
                namespace,
                session,
                tokengo_shared::normalize_model_name(model),
                protocol
            ])
            .to_string()
        )
    )
}
/// Source ownership for clients which do not supply a conversation identity.
/// Separate from explicit sessions; cache hints and prompt text never participate.
/// Same-key anonymous conversations may share a source, but never another source's artifacts.
pub fn anonymous_key(namespace: &str, model: &str, pin: &Pin) -> String {
    use sha2::{Digest, Sha256};
    let scope = serde_json::json!([
        namespace,
        tokengo_shared::normalize_model_name(model),
        Protocol::Responses,
        pin
    ]).to_string();
    format!("protocol_anonymous:{:x}", Sha256::digest(scope.as_bytes()))
}

/// Resolve only against currently authorized native Responses sources. Every supplied
/// artifact must have been issued to the same anonymous scope. Explicit owners are never
/// searched, and ambiguity fails closed rather than choosing an account arbitrarily.
pub fn resolve_anonymous(
    c: &rusqlite::Connection,
    namespace: &str,
    model: &str,
    candidates: &[Pin],
    body: &Value,
) -> db::Result<Option<(String, Pin)>> {
    if !matches!(bounded_values(body), Ok(values) if !values.is_empty()) {
        return Ok(None);
    }
    let mut found: Option<(String, Pin)> = None;
    let mut visited = std::collections::BTreeSet::new();
    for pin in candidates.iter().filter(|p| p.protocol == Protocol::Responses) {
        let owner = anonymous_key(namespace, model, pin);
        if !visited.insert(owner.clone()) {
            continue;
        }
        if verify_artifacts(c, &owner, pin, body)? {
            if found.is_some() {
                return Ok(None);
            }
            found = Some((owner, pin.clone()));
        }
    }
    Ok(found)
}

pub const SQL: &str = include_str!("db/protocol_state.sql");
pub const INACTIVE_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// A soft LRU floor, not a lifetime quota. Recently used histories may exceed it.
pub const RECENT_ARTIFACTS_PER_OWNER: usize = 500;
pub const COLD_IDLE_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_COLD_PRUNE_BATCH: usize = 1024;
pub const MAX_TOTAL_ARTIFACTS: usize = 1_000_000;
pub const MAX_TOTAL_SESSIONS: usize = 100_000;
#[derive(Clone, Copy)]
struct Limits {
    recent: usize,
    artifacts: usize,
    sessions: usize,
}
const LIMITS: Limits = Limits {
    recent: RECENT_ARTIFACTS_PER_OWNER,
    artifacts: MAX_TOTAL_ARTIFACTS,
    sessions: MAX_TOTAL_SESSIONS,
};
fn error(message: &str) -> db::DbError {
    db::DbError::Other(message.into())
}
/// Retain a useful failure category without exposing arbitrary database diagnostics.
/// Only exact internal messages are recognized; SQLite details and unknown strings
/// may contain caller data, SQL values or credentials and must never reach logs.
pub fn persistence_note(error: &db::DbError) -> String {
    let reason = match error {
        db::DbError::Other(message) => match message.as_str() {
            "Opaque artifact capacity reached; no active history was evicted"
            | "Native session capacity reached; no active history was evicted" => "capacity",
            "Native session source ownership changed" => "source_changed",
            "Opaque artifact count or size bound exceeded" => "artifact_bounds",
            "Invalid native session ownership" => "ownership_invalid",
            _ => "storage_failure",
        },
        db::DbError::Sqlite(_) => "storage_failure",
    };
    format!("local_state: opaque artifact ownership could not be persisted (reason={reason})")
}
fn cleanup(c: &rusqlite::Connection, now: i64) -> db::Result<()> {
    let cutoff = now.saturating_sub(INACTIVE_TTL_MS);
    c.execute(
        "DELETE FROM protocol_sessions WHERE last_used_ms<=?1",
        [cutoff],
    )?;
    c.execute(
        "DELETE FROM protocol_artifacts WHERE last_used_ms<=?1",
        [cutoff],
    )?;
    Ok(())
}
/// Retire only idle records outside the most recently used floor in this exact
/// authenticated scope. The current request's verified inputs are refreshed before
/// issuance reaches here. Bounded deletion avoids a large cold pool blocking the DB.
fn prune_cold_owner(c: &rusqlite::Connection, owner: &str, now: i64, recent: usize) -> db::Result<()> {
    c.execute(
        "DELETE FROM protocol_artifacts WHERE owner=?1 AND key_digest IN (
            SELECT key_digest FROM protocol_artifacts WHERE owner=?1 AND last_used_ms<=?2
            AND key_digest NOT IN (
                SELECT key_digest FROM protocol_artifacts WHERE owner=?1
                ORDER BY last_used_ms DESC,issued_ms DESC,key_digest DESC LIMIT ?3
            ) ORDER BY last_used_ms,issued_ms,key_digest LIMIT ?4
        )",
        rusqlite::params![owner, now.saturating_sub(COLD_IDLE_MS), recent as i64, MAX_COLD_PRUNE_BATCH as i64],
    )?;
    Ok(())
}
fn read_pin(c: &rusqlite::Connection, key: &str) -> db::Result<Option<Pin>> {
    use rusqlite::OptionalExtension;
    let value: Option<String> = c
        .query_row(
            "SELECT pin_json FROM protocol_sessions WHERE key=?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    value
        .map(|v| serde_json::from_str(&v).map_err(|_| error("Invalid native session ownership")))
        .transpose()
}
fn load_at(c: &rusqlite::Connection, key: &str, now: i64) -> db::Result<Option<Pin>> {
    cleanup(c, now)?;
    let pin = read_pin(c, key)?;
    if pin.is_some() {
        c.execute(
            "UPDATE protocol_sessions SET last_used_ms=?2 WHERE key=?1",
            rusqlite::params![key, now],
        )?;
    }
    Ok(pin)
}
pub fn load(c: &rusqlite::Connection, key: &str) -> db::Result<Option<Pin>> {
    load_at(c, key, db::now_ms())
}
fn claim_inner(
    c: &rusqlite::Connection,
    key: &str,
    pin: &Pin,
    now: i64,
    limits: Limits,
) -> db::Result<bool> {
    if let Some(existing) = read_pin(c, key)? {
        if existing != *pin {
            return Ok(false);
        }
        c.execute(
            "UPDATE protocol_sessions SET last_used_ms=?2 WHERE key=?1",
            rusqlite::params![key, now],
        )?;
        return Ok(true);
    }
    let count: i64 = c.query_row("SELECT COUNT(*) FROM protocol_sessions", [], |r| r.get(0))?;
    if count >= limits.sessions as i64 {
        return Err(error(
            "Native session capacity reached; no active history was evicted",
        ));
    }
    let encoded =
        serde_json::to_string(pin).map_err(|_| error("Invalid native session ownership"))?;
    c.execute(
        "INSERT INTO protocol_sessions(key,pin_json,last_used_ms) VALUES(?1,?2,?3)",
        rusqlite::params![key, encoded, now],
    )?;
    Ok(true)
}
/// Atomic first-writer wins; expired state alone is eligible for replacement.
pub fn claim(c: &rusqlite::Connection, key: &str, pin: &Pin) -> db::Result<bool> {
    db::in_tx(c, |c| {
        let now = db::now_ms();
        cleanup(c, now)?;
        claim_inner(c, key, pin, now, LIMITS)
    })
}
pub fn opaque_history(protocol: Protocol, body: &Value) -> bool {
    let items = body
        .get(if protocol == Protocol::Responses {
            "input"
        } else {
            "messages"
        })
        .and_then(Value::as_array);
    items.is_some_and(|items| {
        items.iter().any(|m| {
            (protocol == Protocol::Responses
                && if m["type"] == "reasoning" {
                    !crate::protocol::is_plain_reasoning(m)
                } else {
                    m.get("encrypted_content").is_some()
                })
                || m.get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|p| {
                            matches!(
                                p["type"].as_str(),
                                Some("thinking" | "redacted_thinking" | "reasoning")
                            ) || p.get("signature").is_some()
                                || p.get("encrypted_content").is_some()
                        })
                    })
        })
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistence_notes_distinguish_fixed_internal_failures() {
        for (message, reason) in [
            ("Opaque artifact capacity reached; no active history was evicted", "capacity"),
            ("Native session capacity reached; no active history was evicted", "capacity"),
            ("Native session source ownership changed", "source_changed"),
            ("Opaque artifact count or size bound exceeded", "artifact_bounds"),
            ("Invalid native session ownership", "ownership_invalid"),
        ] {
            assert_eq!(
                persistence_note(&error(message)),
                format!("local_state: opaque artifact ownership could not be persisted (reason={reason})"),
            );
        }
    }

    #[test]
    fn persistence_notes_do_not_expose_unknown_or_sqlite_details() {
        let secret = "PRIVATE-KEY-DO-NOT-LOG";
        let expected = "local_state: opaque artifact ownership could not be persisted (reason=storage_failure)";
        for error in [
            error(secret),
            // A known prefix is insufficient: do not echo added dynamic context.
            error(&format!("Native session source ownership changed: {secret}")),
            db::DbError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(secret.into()),
            )),
        ] {
            let note = persistence_note(&error);
            assert_eq!(note, expected);
            assert!(!note.contains(secret));
        }
    }

    #[test]
    fn session_ownership_is_key_scoped_and_cannot_be_overwritten() {
        let c = db::open_in_memory().unwrap();
        let a = Pin {
            channel: 1,
            generation: 1,
            upstream_model: "Exact:Model".into(),
            protocol: Protocol::Messages,
        };
        let k = key("key-a", "session", "M", Protocol::Messages);
        assert!(claim(&c, &k, &a).unwrap());
        assert!(claim(&c, &k, &a).unwrap());
        let b = Pin {
            channel: 2,
            ..a.clone()
        };
        assert!(!claim(&c, &k, &b).unwrap());
        assert_eq!(load(&c, &k).unwrap(), Some(a.clone()));
        assert_eq!(
            load(&c, &key("key-b", "session", "M", Protocol::Messages)).unwrap(),
            None
        );
        assert!(!claim(&c, &k, &Pin { generation: 2, ..a }).unwrap());
    }
    #[test]
    fn colon_delimiters_cannot_alias_session_and_model() {
        assert_ne!(
            key("owner", "a:b", "c", Protocol::Messages),
            key("owner", "a", "b:c", Protocol::Messages)
        );
    }
    #[test]
    fn opaque_blocks_are_not_confused_with_tool_argument_text() {
        assert!(opaque_history(
            Protocol::Messages,
            &serde_json::json!({"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"private","signature":"sig"}]}]})
        ));
        assert!(opaque_history(
            Protocol::Responses,
            &serde_json::json!({"input":[{"type":"reasoning","encrypted_content":"opaque"}]})
        ));
        assert!(!opaque_history(
            Protocol::Messages,
            &serde_json::json!({"messages":[{"role":"user","content":[{"type":"tool_result","content":"signature"}]}]})
        ));
    }
    #[test]
    fn only_valid_plain_reasoning_is_exempt_from_opaque_ownership() {
        let plain = serde_json::json!({"type":"reasoning","id":"rs_plain","status":"completed","summary":[],"content":[{"type":"reasoning_text","text":"public reasoning"}]});
        for encrypted in [false, true] {
            let mut item = plain.clone();
            if encrypted { item["encrypted_content"] = Value::Null; }
            assert!(!opaque_history(Protocol::Responses, &serde_json::json!({"input":[item]})));
        }
        for (field, value) in [
            ("encrypted_content", serde_json::json!("ciphertext")),
            ("summary", serde_json::json!([{"type":"summary_text","text":"summary"}])),
            ("content", serde_json::json!([{"type":"reasoning_text","text":17}])),
            ("content", serde_json::json!([{"type":"reasoning_text","text":"x","signature":"opaque"}])),
            ("status", serde_json::json!("in_progress")),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut item = plain.clone(); item[field] = value;
            assert!(opaque_history(Protocol::Responses, &serde_json::json!({"input":[item]})), "{field}");
        }
    }
}

/// Opaque artifacts are issued to the exact key/session/source generation, not just an account.
fn artifact_key(owner: &str, pin: &Pin, value: &str) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::json!([owner, pin, value]).to_string();
    format!("{:x}", Sha256::digest(bytes.as_bytes()))
}
fn walk_artifacts(v: &Value, visit: &mut impl FnMut(&str) -> bool) -> bool {
    match v {
        Value::Array(items) => {
            for item in items {
                if !walk_artifacts(item, visit) {
                    return false;
                }
            }
        }
        Value::Object(map) => {
            let field = match map.get("type").and_then(Value::as_str) {
                Some("thinking") => Some("signature"),
                Some("redacted_thinking") => Some("data"),
                Some("reasoning") => Some("encrypted_content"),
                _ => None,
            };
            if let Some(value) = field
                .and_then(|f| map.get(f))
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                if !visit(value) {
                    return false;
                }
            }
            for key in [
                "response",
                "message",
                "output",
                "input",
                "content",
                "messages",
                "content_block",
                "item",
            ] {
                if let Some(value) = map.get(key) {
                    if !walk_artifacts(value, visit) {
                        return false;
                    }
                }
            }
        }
        _ => {}
    }
    true
}
/// Raw extraction helper. Security-sensitive callers below use bounded_values instead.
pub fn artifact_values(v: &Value) -> Vec<String> {
    let mut values = vec![];
    walk_artifacts(v, &mut |s| {
        values.push(s.to_string());
        true
    });
    values
}
fn bounded_values(v: &Value) -> Result<Vec<String>, ()> {
    let mut values = vec![];
    let mut bytes = 0usize;
    let complete = walk_artifacts(v, &mut |s| {
        if values.len() >= 512 || s.len() > 1 << 20 || bytes.saturating_add(s.len()) > 16 << 20 {
            return false;
        }
        bytes += s.len();
        values.push(s.to_string());
        true
    });
    if complete {
        Ok(values)
    } else {
        Err(())
    }
}
pub fn verify_artifacts(
    c: &rusqlite::Connection,
    owner: &str,
    pin: &Pin,
    body: &Value,
) -> db::Result<bool> {
    let Ok(values) = bounded_values(body) else {
        return Ok(false);
    };
    db::in_tx(c, |c| {
        let now = db::now_ms();
        cleanup(c, now)?;
        if read_pin(c, owner)?.as_ref() != Some(pin) {
            return Ok(false);
        }
        let digests: std::collections::BTreeSet<_> =
            values.iter().map(|v| artifact_key(owner, pin, v)).collect();
        for digest in &digests {
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM protocol_artifacts WHERE owner=?1 AND key_digest=?2)",
                rusqlite::params![owner, digest],
                |r| r.get(0),
            )?;
            if !exists {
                return Ok(false);
            }
        }
        for digest in &digests {
            c.execute(
                "UPDATE protocol_artifacts SET last_used_ms=?3 WHERE owner=?1 AND key_digest=?2",
                rusqlite::params![owner, digest, now],
            )?;
        }
        c.execute(
            "UPDATE protocol_sessions SET last_used_ms=?2 WHERE key=?1",
            rusqlite::params![owner, now],
        )?;
        Ok(true)
    })
}
#[derive(Default)]
pub struct ArtifactCollector {
    signatures: std::collections::HashMap<u64, String>,
    redacted: std::collections::HashMap<u64, String>,
    pending: Vec<String>,
    overflowed: bool,
}
impl ArtifactCollector {
    pub fn value(&mut self, v: &Value) {
        if self.overflowed {
            return;
        }
        match bounded_values(v) {
            Ok(values) => {
                if self.pending.len() + values.len() > 512
                    || self
                        .pending
                        .iter()
                        .chain(values.iter())
                        .map(String::len)
                        .sum::<usize>()
                        > 16 << 20
                {
                    self.overflowed = true;
                } else {
                    self.pending.extend(values);
                }
            }
            Err(()) => self.overflowed = true,
        }
    }
    pub fn frames(&mut self, frames: &[crate::protocol::sse::Frame]) -> Result<(), String> {
        for frame in frames {
            let Ok(v) = serde_json::from_str::<Value>(&frame.data) else {
                continue;
            };
            let event = v
                .get("type")
                .and_then(Value::as_str)
                .or(frame.event.as_deref())
                .unwrap_or("");
            let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
            // Start frames may contain only a prefix. Issue signatures only when
            // their block closes, and Responses artifacts only from final snapshots.
            if !matches!(
                event,
                "content_block_start"
                    | "content_block_delta"
                    | "response.output_item.added"
                    | "response.created"
                    | "message_start"
            ) {
                self.value(&v);
            }

            if event == "content_block_start" && v["content_block"]["type"] == "redacted_thinking" {
                if self.signatures.len() + self.redacted.len() >= 128 {
                    return Err("Too many opaque blocks".into());
                }
                self.redacted.insert(
                    index,
                    v["content_block"]["data"].as_str().unwrap_or("").into(),
                );
            }
            if event == "content_block_start" && v["content_block"]["type"] == "thinking" {
                if self.signatures.len() + self.redacted.len() >= 128 {
                    return Err("Too many signature blocks".into());
                }
                self.signatures.insert(
                    index,
                    v["content_block"]["signature"]
                        .as_str()
                        .unwrap_or("")
                        .into(),
                );
            }
            if event == "content_block_delta" && v["delta"]["type"] == "signature_delta" {
                if !self.signatures.contains_key(&index)
                    && self.signatures.len() + self.redacted.len() >= 128
                {
                    return Err("Too many signature blocks".into());
                }
                let signature = self.signatures.entry(index).or_default();
                signature.push_str(v["delta"]["signature"].as_str().unwrap_or(""));
                if signature.len() > 1 << 20 {
                    return Err("Signature exceeds 1 MiB".into());
                }
            }
            if event == "content_block_stop" {
                if let Some(value) = self.signatures.remove(&index).filter(|v| !v.is_empty()) {
                    self.pending.push(value);
                }
                if let Some(value) = self.redacted.remove(&index).filter(|v| !v.is_empty()) {
                    self.pending.push(value);
                }
            }
        }
        if self.overflowed
            || self.pending.len() > 512
            || self.pending.iter().map(String::len).sum::<usize>()
                + self.signatures.values().map(String::len).sum::<usize>()
                + self.redacted.values().map(String::len).sum::<usize>()
                > 16 << 20
        {
            return Err("Opaque artifact memory bound exceeded".into());
        }
        Ok(())
    }
    pub fn persist(&mut self, c: &rusqlite::Connection, owner: &str, pin: &Pin) -> db::Result<()> {
        self.persist_at(c, owner, pin, db::now_ms(), LIMITS)
    }
    fn persist_at(
        &mut self,
        c: &rusqlite::Connection,
        owner: &str,
        pin: &Pin,
        now: i64,
        limits: Limits,
    ) -> db::Result<()> {
        if self.overflowed
            || self.pending.len() > 512
            || self.pending.iter().any(|v| v.len() > 1 << 20)
            || self.pending.iter().map(String::len).sum::<usize>()
                + self.signatures.values().map(String::len).sum::<usize>()
                + self.redacted.values().map(String::len).sum::<usize>()
                > 16 << 20
        {
            return Err(error("Opaque artifact count or size bound exceeded"));
        }
        let digests: std::collections::BTreeSet<_> = self
            .pending
            .iter()
            .map(|v| artifact_key(owner, pin, v))
            .collect();
        db::in_tx(c, |c| {
            cleanup(c, now)?;
            if !claim_inner(c, owner, pin, now, limits)? {
                return Err(error("Native session source ownership changed"));
            }
            let mut additions = 0usize;
            for digest in &digests {
                let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM protocol_artifacts WHERE owner=?1 AND key_digest=?2)",rusqlite::params![owner,digest],|r|r.get(0))?;
                additions += usize::from(!exists);
            }
            for digest in &digests {
                c.execute("INSERT INTO protocol_artifacts(owner,key_digest,issued_ms,last_used_ms) VALUES(?1,?2,?3,?3) ON CONFLICT(owner,key_digest) DO UPDATE SET last_used_ms=excluded.last_used_ms",rusqlite::params![owner,digest,now])?;
            }
            if !digests.is_empty() {
                prune_cold_owner(c, owner, now, limits.recent)?;
            }
            // Preserve the global safety bound, but don't repeatedly count the whole
            // ledger for plain SSE chunks or reissued values that add no storage.
            if additions > 0 {
                let total: i64 = c.query_row("SELECT COUNT(*) FROM protocol_artifacts", [], |r| r.get(0))?;
                if total as usize > limits.artifacts {
                    return Err(error("Opaque artifact capacity reached; no active history was evicted"));
                }
            }
            Ok(())
        })?;
        self.pending.clear();
        Ok(())
    }
}
#[cfg(test)]
mod artifact_tests {
    use super::*;
    #[test]
    fn signature_is_usable_only_after_issuance_to_same_key_session_source() {
        let c = db::open_in_memory().unwrap();
        let pin = Pin {
            channel: 1,
            generation: 2,
            upstream_model: "M".into(),
            protocol: Protocol::Messages,
        };
        let body = serde_json::json!({"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"thought","signature":"opaque-sig"}]}]});
        assert!(!verify_artifacts(&c, "key-a-session", &pin, &body).unwrap());
        let mut collector = ArtifactCollector::default();
        collector.value(&body);
        collector.persist(&c, "key-a-session", &pin).unwrap();
        assert!(verify_artifacts(&c, "key-a-session", &pin, &body).unwrap());
        assert!(!verify_artifacts(&c, "key-b-session", &pin, &body).unwrap());
        assert!(!verify_artifacts(
            &c,
            "key-a-session",
            &Pin {
                generation: 3,
                ..pin
            },
            &body
        )
        .unwrap());
        let serialized: String = c
            .query_row(
                "SELECT group_concat(owner||key_digest) FROM protocol_artifacts",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!serialized.contains("opaque-sig"));
    }
    #[test]
    fn signature_deltas_are_registered_as_one_complete_signature() {
        let c = db::open_in_memory().unwrap();
        let pin = Pin {
            channel: 1,
            generation: 1,
            upstream_model: "M".into(),
            protocol: Protocol::Messages,
        };
        let mut collector = ArtifactCollector::default();
        let values = [
            serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"part1"}}),
            serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"part2"}}),
            serde_json::json!({"type":"content_block_stop","index":0}),
        ];
        for v in values {
            collector
                .frames(&[crate::protocol::sse::Frame {
                    event: None,
                    data: v.to_string(),
                }])
                .unwrap();
        }
        collector.persist(&c, "owner", &pin).unwrap();
        assert!(verify_artifacts(
            &c,
            "owner",
            &pin,
            &serde_json::json!({"type":"thinking","signature":"part1part2"})
        )
        .unwrap());
        assert!(!verify_artifacts(
            &c,
            "owner",
            &pin,
            &serde_json::json!({"type":"thinking","signature":"part1"})
        )
        .unwrap());
    }
}

#[cfg(test)]
mod artifact_boundary_tests {
    use super::*;
    #[test]
    fn a_signature_prefix_is_not_issued_before_block_stop() {
        let c = db::open_in_memory().unwrap();
        let pin = Pin {
            channel: 1,
            generation: 1,
            upstream_model: "M".into(),
            protocol: Protocol::Messages,
        };
        let mut collector = ArtifactCollector::default();
        collector.frames(&[crate::protocol::sse::Frame{event:None,data:serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","signature":"prefix"}}).to_string()}]).unwrap();
        collector.persist(&c, "owner", &pin).unwrap();
        assert!(!verify_artifacts(
            &c,
            "owner",
            &pin,
            &serde_json::json!({"type":"thinking","signature":"prefix"})
        )
        .unwrap());
    }
    #[test]
    fn nonstream_artifact_count_has_the_same_bound_as_stream() {
        let c = db::open_in_memory().unwrap();
        let pin = Pin {
            channel: 1,
            generation: 1,
            upstream_model: "M".into(),
            protocol: Protocol::Responses,
        };
        let mut collector = ArtifactCollector::default();
        collector.value(&serde_json::json!({"output":vec![serde_json::json!({"type":"reasoning","encrypted_content":"test"});513]}));
        assert!(collector.persist(&c, "owner", &pin).is_err());
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM protocol_artifacts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    fn pin() -> Pin {
        Pin {
            channel: 1,
            generation: 1,
            upstream_model: "M".into(),
            protocol: Protocol::Messages,
        }
    }
    fn collector(value: &str) -> ArtifactCollector {
        let mut c = ArtifactCollector::default();
        c.value(&serde_json::json!({"type":"thinking","signature":value}));
        c
    }
    fn count(c: &rusqlite::Connection, table: &str) -> i64 {
        c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    #[test]
    fn inactive_sessions_expire_cascade_and_cannot_revive_opaque_history() {
        let c = db::open_in_memory().unwrap();
        let now = db::now_ms();
        let p = pin();
        collector("old")
            .persist_at(&c, "old-owner", &p, now - INACTIVE_TTL_MS - 1, LIMITS)
            .unwrap();
        assert_eq!(count(&c, "protocol_artifacts"), 1);
        assert_eq!(load(&c, "old-owner").unwrap(), None);
        assert_eq!(count(&c, "protocol_artifacts"), 0);
        assert!(!verify_artifacts(
            &c,
            "old-owner",
            &p,
            &serde_json::json!({"type":"thinking","signature":"old"})
        )
        .unwrap());
        let next = Pin { generation: 2, ..p };
        assert!(claim(&c, "old-owner", &next).unwrap());
    }
    #[test]
    fn artifact_expiry_is_independent_of_a_recently_active_session() {
        let c = db::open_in_memory().unwrap();
        let now = db::now_ms();
        let p = pin();
        collector("inactive").persist(&c, "owner", &p).unwrap();
        c.execute(
            "UPDATE protocol_artifacts SET last_used_ms=?1",
            [now - INACTIVE_TTL_MS],
        )
        .unwrap();
        assert!(load(&c, "owner").unwrap().is_some());
        assert_eq!(count(&c, "protocol_artifacts"), 0);
        assert!(!verify_artifacts(
            &c,
            "owner",
            &p,
            &serde_json::json!({"type":"thinking","signature":"inactive"})
        )
        .unwrap());
    }
    #[test]
    fn global_and_session_limits_protect_active_state_without_an_owner_lifetime_quota() {
        let c=db::open_in_memory().unwrap();let p=pin();let now=db::now_ms();
        let small=Limits{recent:1,artifacts:2,sessions:2};
        collector("one").persist_at(&c,"a",&p,now,small).unwrap();
        collector("one").persist_at(&c,"a",&p,now,small).unwrap();
        collector("two").persist_at(&c,"a",&p,now,small).unwrap();
        assert!(collector("three").persist_at(&c,"b",&p,now,small).is_err());
        assert_eq!(read_pin(&c,"b").unwrap(),None);
        assert!(claim_inner(&c,"b",&p,now,small).unwrap());
        assert!(claim_inner(&c,"c",&p,now,small).is_err());
        assert_eq!(count(&c,"protocol_artifacts"),2);
        assert_eq!(count(&c,"protocol_sessions"),2);
        for value in ["one","two"] {
            assert!(verify_artifacts(&c,"a",&p,&serde_json::json!({"type":"thinking","signature":value})).unwrap());
        }
    }
    #[test]
    fn failed_issuance_rolls_back_pin_and_artifacts_and_retains_pending_for_retry() {
        let c = db::open_in_memory().unwrap();
        let p = pin();
        let mut a = collector("not-issued");
        c.execute_batch("CREATE TRIGGER opaque_abort BEFORE INSERT ON protocol_artifacts BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
        assert!(a.persist(&c, "owner", &p).is_err());
        assert_eq!(count(&c, "protocol_sessions"), 0);
        assert_eq!(count(&c, "protocol_artifacts"), 0);
        c.execute_batch("DROP TRIGGER opaque_abort").unwrap();
        a.persist(&c, "owner", &p).unwrap();
        assert_eq!(count(&c, "protocol_artifacts"), 1);
        assert!(collector("wrong-source")
            .persist(
                &c,
                "owner",
                &Pin {
                    generation: 2,
                    ..p.clone()
                }
            )
            .is_err());
        assert!(!verify_artifacts(
            &c,
            "owner",
            &Pin { generation: 2, ..p },
            &serde_json::json!({"type":"thinking","signature":"not-issued"})
        )
        .unwrap());
    }
    #[test]
    fn redacted_thinking_stream_data_is_issued_only_when_block_stops() {
        let c = db::open_in_memory().unwrap();
        let p = pin();
        let mut a = ArtifactCollector::default();
        let body = serde_json::json!({"type":"redacted_thinking","data":"redacted-opaque"});
        a.frames(&[crate::protocol::sse::Frame {
            event: None,
            data: serde_json::json!({"type":"content_block_start","index":2,"content_block":body})
                .to_string(),
        }])
        .unwrap();
        a.persist(&c, "owner", &p).unwrap();
        assert!(!verify_artifacts(&c, "owner", &p, &body).unwrap());
        a.frames(&[crate::protocol::sse::Frame {
            event: None,
            data: serde_json::json!({"type":"content_block_stop","index":2}).to_string(),
        }])
        .unwrap();
        a.persist(&c, "owner", &p).unwrap();
        assert!(verify_artifacts(&c, "owner", &p, &body).unwrap());
    }
    #[test]
    fn closed_signature_is_issued_but_foreign_session_and_unclosed_prefix_are_not() {
        let c = db::open_in_memory().unwrap();
        let p = pin();
        let owner = key("key", "session", "M", Protocol::Messages);
        let foreign = key("key", "foreign-session", "M", Protocol::Messages);
        let mut a = ArtifactCollector::default();
        for value in [
            serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","signature":"start"}}),
            serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"end"}}),
            serde_json::json!({"type":"content_block_stop","index":0}),
        ] {
            a.frames(&[crate::protocol::sse::Frame {
                event: None,
                data: value.to_string(),
            }])
            .unwrap();
        }
        a.persist(&c, &owner, &p).unwrap();
        let full = serde_json::json!({"type":"thinking","signature":"startend"});
        assert!(verify_artifacts(&c, &owner, &p, &full).unwrap());
        assert!(!verify_artifacts(&c, &foreign, &p, &full).unwrap());
        assert!(!verify_artifacts(
            &c,
            &owner,
            &p,
            &serde_json::json!({"type":"thinking","signature":"start"})
        )
        .unwrap());
        let stored: String = c
            .query_row(
                "SELECT group_concat(owner||key_digest) FROM protocol_artifacts",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!stored.contains("startend"));
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    #[test]
    fn legacy_pins_migrate_but_unowned_artifact_digests_are_invalidated() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);").unwrap();
        let p = Pin {
            channel: 1,
            generation: 2,
            upstream_model: "Exact".into(),
            protocol: Protocol::Messages,
        };
        let owner = key("client", "session", "M", Protocol::Messages);
        c.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)",
            rusqlite::params![owner, serde_json::to_string(&p).unwrap()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO settings(key,value) VALUES(?1,'issued')",
            [format!(
                "protocol_artifact:{}",
                artifact_key(&owner, &p, "legacy-opaque")
            )],
        )
        .unwrap();
        c.execute_batch(SQL).unwrap();
        assert_eq!(load(&c, &owner).unwrap(), Some(p.clone()));
        assert!(!verify_artifacts(
            &c,
            &owner,
            &p,
            &serde_json::json!({"type":"thinking","signature":"legacy-opaque"})
        )
        .unwrap());
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM settings", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[cfg(test)]
mod anonymous_tests {
    use super::*;
    use serde_json::json;

    fn pin(channel: i64) -> Pin {
        Pin { channel, generation: 1, upstream_model: "upstream".into(), protocol: Protocol::Responses }
    }
    fn body(value: &str) -> Value {
        json!({"input":[{"type":"reasoning","encrypted_content":value}]})
    }
    fn issue(c: &rusqlite::Connection, namespace: &str, model: &str, pin: &Pin, value: &str) -> String {
        let owner = anonymous_key(namespace, model, pin);
        let mut artifacts = ArtifactCollector::default();
        artifacts.value(&body(value));
        artifacts.persist(c, &owner, pin).unwrap();
        owner
    }

    #[test]
    fn anonymous_resolution_requires_all_artifacts_from_one_authorized_scope() {
        let c = db::open_in_memory().unwrap();
        let a = pin(1);
        let b = pin(2);
        let owner = issue(&c, "key-a", "model", &a, "a");
        issue(&c, "key-a", "model", &b, "b");
        let candidates = [a.clone(), a.clone(), b.clone()];
        assert_eq!(resolve_anonymous(&c, "key-a", "model", &candidates, &body("a")).unwrap(), Some((owner, a.clone())));
        for (namespace, model, values) in [
            ("key-b", "model", body("a")),
            ("key-a", "other-model", body("a")),
            ("key-a", "model", body("unknown")),
            ("key-a", "model", json!({"input":[{"type":"reasoning","encrypted_content":"a"},{"type":"reasoning","encrypted_content":"b"}]})),
            ("key-a", "model", body("")),
            ("key-a", "model", json!({"input":[{"type":"reasoning","summary":[]}]})),
        ] {
            assert!(resolve_anonymous(&c, namespace, model, &candidates, &values).unwrap().is_none());
        }
        assert!(resolve_anonymous(&c, "key-a", "model", &[b], &body("a")).unwrap().is_none());
        let mut changed = a.clone();
        changed.generation += 1;
        assert!(resolve_anonymous(&c, "key-a", "model", &[changed], &body("a")).unwrap().is_none());
        let mut changed = a.clone();
        changed.upstream_model = "other".into();
        assert!(resolve_anonymous(&c, "key-a", "model", &[changed], &body("a")).unwrap().is_none());
        let mut changed = a;
        changed.protocol = Protocol::Messages;
        assert!(resolve_anonymous(&c, "key-a", "model", &[changed], &body("a")).unwrap().is_none());
    }

    #[test]
    fn anonymous_resolution_rejects_ambiguous_sources_and_expired_artifacts() {
        let c = db::open_in_memory().unwrap();
        let a = pin(1);
        let b = pin(2);
        let owner = issue(&c, "key", "model", &a, "same");
        issue(&c, "key", "model", &b, "same");
        assert!(resolve_anonymous(&c, "key", "model", &[a.clone(), b], &body("same")).unwrap().is_none());
        c.execute("UPDATE protocol_artifacts SET last_used_ms=0 WHERE owner=?1", [&owner]).unwrap();
        assert!(resolve_anonymous(&c, "key", "model", &[a.clone()], &body("same")).unwrap().is_none());
        issue(&c, "key", "model", &a, "new");
        c.execute("UPDATE protocol_sessions SET last_used_ms=0 WHERE key=?1", [&owner]).unwrap();
        assert!(resolve_anonymous(&c, "key", "model", &[a], &body("new")).unwrap().is_none());
    }

    #[test]
    fn anonymous_resolution_never_imports_explicit_ownership_or_oversized_input() {
        let c = db::open_in_memory().unwrap();
        let p = pin(1);
        let owner = key("key", "real-session", "model", Protocol::Responses);
        let mut artifacts = ArtifactCollector::default();
        artifacts.value(&body("explicit"));
        artifacts.persist(&c, &owner, &p).unwrap();
        assert!(resolve_anonymous(&c, "key", "model", &[p.clone()], &body("explicit")).unwrap().is_none());
        issue(&c, "key", "model", &p, "anonymous");
        assert!(!verify_artifacts(&c, &owner, &p, &body("anonymous")).unwrap());
        let oversized = json!({"input":vec![json!({"type":"reasoning","encrypted_content":"anonymous"});513]});
        assert!(resolve_anonymous(&c, "key", "model", &[p], &oversized).unwrap().is_none());
    }
}

#[cfg(test)]
mod cold_retention_tests {
    use super::*;
    fn seed(c: &rusqlite::Connection, owner: &str, pin: &Pin, count: i64, last_used: i64) {
        assert!(claim(c, owner, pin).unwrap());
        for i in 0..count {
            c.execute("INSERT INTO protocol_artifacts(owner,key_digest,issued_ms,last_used_ms) VALUES(?1,?2,?3,?3)",rusqlite::params![owner,artifact_key(owner,pin,&format!("cold-{i}")),last_used-i]).unwrap();
        }
    }
    fn p() -> Pin {Pin{channel:1,generation:1,upstream_model:"m".into(),protocol:Protocol::Responses}}
    fn issue(c:&rusqlite::Connection,owner:&str,pin:&Pin,now:i64) {
        let mut a=ArtifactCollector::default();a.value(&serde_json::json!({"type":"reasoning","encrypted_content":"new"}));a.persist_at(c,owner,pin,now,LIMITS).unwrap();
    }
    fn exists(c:&rusqlite::Connection,owner:&str,pin:&Pin,value:&str)->bool {
        c.query_row("SELECT EXISTS(SELECT 1 FROM protocol_artifacts WHERE owner=?1 AND key_digest=?2)",rusqlite::params![owner,artifact_key(owner,pin,value)],|r|r.get(0)).unwrap()
    }
    #[test]
    fn cold_retention_prunes_only_idle_excess_and_keeps_500_recent_values() {
        let c=db::open_in_memory().unwrap();let pin=p();let now=db::now_ms();
        seed(&c,"a",&pin,501,now-25*60*60*1000);issue(&c,"a",&pin,now);
        let n:i64=c.query_row("SELECT COUNT(*) FROM protocol_artifacts WHERE owner='a'",[],|r|r.get(0)).unwrap();
        assert_eq!(n,500);assert!(exists(&c,"a",&pin,"new"));assert!(exists(&c,"a",&pin,"cold-498"));assert!(!exists(&c,"a",&pin,"cold-499"));assert!(!exists(&c,"a",&pin,"cold-500"));
    }
    #[test]
    fn cold_retention_input_reuse_refreshes_old_history_before_lru_eviction() {
        let c=db::open_in_memory().unwrap();let pin=p();let now=db::now_ms();seed(&c,"a",&pin,501,now-25*60*60*1000);
        assert!(verify_artifacts(&c,"a",&pin,&serde_json::json!({"input":[{"type":"reasoning","encrypted_content":"cold-500"}]})).unwrap());
        issue(&c,"a",&pin,db::now_ms()+1);
        assert!(exists(&c,"a",&pin,"cold-500"));assert!(!exists(&c,"a",&pin,"cold-499"));
    }
    #[test]
    fn cold_retention_recent_activity_can_exceed_soft_window_and_other_owners_are_isolated() {
        let c=db::open_in_memory().unwrap();let pin=p();let now=db::now_ms();seed(&c,"hot",&pin,501,now);seed(&c,"other",&pin,501,now-25*60*60*1000);
        issue(&c,"hot",&pin,now);assert!(exists(&c,"other",&pin,"cold-500"));
        assert_eq!(c.query_row("SELECT COUNT(*) FROM protocol_artifacts WHERE owner='hot'",[],|r|r.get::<_,i64>(0)).unwrap(),502);
    }
    #[test]
    fn cold_retention_500_floor_and_idle_boundary_are_separate_from_hard_ttl() {
        let c=db::open_in_memory().unwrap();let pin=p();let now=db::now_ms();seed(&c,"floor",&pin,3,now-25*60*60*1000);issue(&c,"floor",&pin,now);assert!(exists(&c,"floor",&pin,"cold-2"));
        seed(&c,"boundary",&pin,501,now-24*60*60*1000+1000);issue(&c,"boundary",&pin,now);assert!(exists(&c,"boundary",&pin,"cold-500"));
        issue(&c,"boundary",&pin,now+2000);assert!(!exists(&c,"boundary",&pin,"cold-500"));
    }
    #[test]
    fn cold_retention_pruning_is_bounded_and_failed_issuance_restores_deleted_history() {
        let c=db::open_in_memory().unwrap();let pin=p();let now=db::now_ms();
        seed(&c,"batch",&pin,2000,now-25*60*60*1000);
        let mut a=ArtifactCollector::default();a.value(&serde_json::json!({"type":"reasoning","encrypted_content":"new"}));
        assert!(a.persist_at(&c,"batch",&pin,now,Limits{recent:500,artifacts:500,sessions:100}).is_err());
        assert_eq!(c.query_row("SELECT COUNT(*) FROM protocol_artifacts",[],|r|r.get::<_,i64>(0)).unwrap(),2000,"cold deletions must roll back with a failed global-capacity check");
        assert!(exists(&c,"batch",&pin,"cold-1999"));assert!(!exists(&c,"batch",&pin,"new"));
        a.persist_at(&c,"batch",&pin,now,LIMITS).unwrap();
        assert_eq!(c.query_row("SELECT COUNT(*) FROM protocol_artifacts",[],|r|r.get::<_,i64>(0)).unwrap(),977,"single issuance prunes at most 1024 rows");
        assert!(exists(&c,"batch",&pin,"new"));assert!(exists(&c,"batch",&pin,"cold-0"));assert!(!exists(&c,"batch",&pin,"cold-1999"));
        issue(&c,"batch",&pin,now+1);
        assert_eq!(c.query_row("SELECT COUNT(*) FROM protocol_artifacts",[],|r|r.get::<_,i64>(0)).unwrap(),500);
    }
    #[test]
    fn cold_retention_reused_state_and_evictions_survive_database_reopen() {
        let path=std::env::temp_dir().join(format!("tokengo-cold-retention-{}",uuid::Uuid::new_v4()));
        let pin=p();let now=db::now_ms();
        {
            let c=db::open(&path).unwrap();seed(&c,"restart",&pin,501,now-25*60*60*1000);
            assert!(verify_artifacts(&c,"restart",&pin,&serde_json::json!({"input":[{"type":"reasoning","encrypted_content":"cold-500"}]})).unwrap());
            issue(&c,"restart",&pin,db::now_ms()+1);
        }
        {
            let c=db::open(&path).unwrap();assert!(exists(&c,"restart",&pin,"cold-500"));assert!(!exists(&c,"restart",&pin,"cold-499"));
            assert!(verify_artifacts(&c,"restart",&pin,&serde_json::json!({"input":[{"type":"reasoning","encrypted_content":"cold-500"}]})).unwrap());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cold_retention_large_pool_records_cleanup_cost_without_a_timing_gate() {
        let c=db::open_in_memory().unwrap();let pin=p();let now=db::now_ms();
        for (owner,time) in [("large-hot",now),("large-cold",now-25*60*60*1000)] {
            assert!(claim(&c,owner,&pin).unwrap());
            c.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<20000) INSERT INTO protocol_artifacts(owner,key_digest,issued_ms,last_used_ms) SELECT ?1,printf('%064x',i),?2,?2 FROM n",rusqlite::params![owner,time]).unwrap();
            let started=std::time::Instant::now();issue(&c,owner,&pin,now);
            let count=c.query_row("SELECT COUNT(*) FROM protocol_artifacts WHERE owner=?1",[owner],|r|r.get::<_,i64>(0)).unwrap();
            eprintln!("history-owner={owner} initial=20000 remaining={count} issuance_ms={}",started.elapsed().as_millis());
            assert_eq!(count,if owner=="large-hot"{20001}else{18977});
        }
    }

    #[test]
    fn cold_retention_lru_lookup_uses_its_covering_index_without_a_sort() {
        let c=db::open_in_memory().unwrap();
        let mut statement=c.prepare("EXPLAIN QUERY PLAN SELECT key_digest FROM protocol_artifacts WHERE owner=?1 ORDER BY last_used_ms DESC,issued_ms DESC,key_digest DESC LIMIT 500").unwrap();
        let rows=statement.query_map(["owner"],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert!(rows.iter().any(|s|s.contains("protocol_artifacts_owner_recent")),"{rows:?}");
        assert!(!rows.iter().any(|s|s.contains("TEMP B-TREE")),"{rows:?}");
    }

}
