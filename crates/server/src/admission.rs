//! Durable rolling-window reservations and cancel-safe user/key in-flight limits.
//! Lock order is database, then admission counts. Drop takes counts only.
use crate::{
    db::{
        self,
        identity::{AuthContext, Policy},
    },
    routes::ApiError,
    SharedState,
};
use axum::http::StatusCode;
use rusqlite::{params, Connection};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Mutex,
};

const WINDOW_MS: i64 = 60_000;
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Scope {
    kind: &'static str,
    id: i64,
}
#[derive(Clone, Copy)]
struct Limits {
    rpm: Option<u32>,
    tpm: Option<u64>,
    max_concurrent: Option<u32>,
}
impl From<&Policy> for Limits {
    fn from(policy: &Policy) -> Self {
        Self {
            rpm: policy.rpm,
            tpm: policy.tpm,
            max_concurrent: policy.max_concurrent,
        }
    }
}
#[derive(Default)]
pub struct AdmissionState {
    in_flight: Mutex<HashMap<Scope, u64>>,
}

/// Holds every admitted scope until the request (including streaming body) ends.
/// Dropping/canceling releases concurrency only; spent rolling-window reservations
/// deliberately remain durable, preventing cancellation/restart from resetting RPM/TPM.
pub struct AdmissionPermit {
    state: SharedState,
    scopes: Vec<Scope>,
}
impl fmt::Debug for AdmissionPermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdmissionPermit")
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}
impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        let mut counts = self
            .state
            .admission
            .in_flight
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for scope in &self.scopes {
            if let Some(count) = counts.get_mut(scope) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    counts.remove(scope);
                }
            }
        }
    }
}
fn rate_error(scope: Scope, metric: &str) -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limit_exceeded",
        format!("{} {metric} limit exceeded", scope.kind),
    )
}
fn storage(error: rusqlite::Error) -> ApiError {
    ApiError::internal(format!("Admission storage failed: {error}"))
}

/// Atomically reserve one request and input+maximum-output tokens for the user,
/// this key, and all parent keys. AuthContext must come from current authentication.
pub fn acquire(
    state: SharedState,
    auth: &AuthContext,
    input_tokens: u64,
    output_tokens: u64,
) -> Result<AdmissionPermit, ApiError> {
    acquire_inner(state, auth, input_tokens, output_tokens, false)
}

/// Opaque HTTP has no token accounting. Never let it bypass an active TPM cap.
pub fn acquire_request_only(state: SharedState, auth: &AuthContext) -> Result<AdmissionPermit, ApiError> {
    acquire_inner(state, auth, 0, 0, true)
}

fn acquire_inner(
    state: SharedState,
    auth: &AuthContext,
    input_tokens: u64,
    output_tokens: u64,
    request_only: bool,
) -> Result<AdmissionPermit, ApiError> {
    let tokens = input_tokens
        .checked_add(output_tokens)
        .and_then(|tokens| i64::try_from(tokens).ok())
        .ok_or_else(|| {
            rate_error(
                Scope {
                    kind: "key",
                    id: auth.key_id,
                },
                "token reservation",
            )
        })?;
    let scopes = {
        let conn = state.conn();
        let transaction = conn.unchecked_transaction().map_err(storage)?;
        // Authentication may have waited behind other requests. Re-read this key,
        // its owner, and the parent chain in the same snapshot as reservations.
        let current = db::identity::context(&transaction, auth.key_id)?
            .filter(|current| current.user_id == auth.user_id)
            .ok_or_else(|| ApiError::unauthorized("Key or user is no longer available"))?;
        let auth = &current;
        let mut scopes = vec![
            (
                Scope {
                    kind: "user",
                    id: auth.user_id,
                },
                Limits::from(&auth.user_policy),
            ),
            (
                Scope {
                    kind: "key",
                    id: auth.key_id,
                },
                Limits::from(&auth.key_policy),
            ),
        ];
        let mut seen = HashSet::from([auth.key_id]);
        if auth.ancestor_key_ids.len() > 32 {
            return Err(ApiError::unauthorized("Invalid parent-key chain"));
        }
        for id in &auth.ancestor_key_ids {
            if !seen.insert(*id) {
                return Err(ApiError::unauthorized("Invalid parent-key chain"));
            }
            let ancestor = db::identity::context(&transaction, *id)?
                .filter(|context| context.user_id == auth.user_id)
                .ok_or_else(|| ApiError::unauthorized("Parent key is no longer available"))?;
            scopes.push((
                Scope {
                    kind: "key",
                    id: *id,
                },
                Limits::from(&ancestor.key_policy),
            ));
        }
        let now = db::now_ms();
        let cutoff = now.saturating_sub(WINDOW_MS);
        let mut counts = state
            .admission
            .in_flight
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // Check every scope before mutating either the DB or in-flight counts.
        for (scope, limits) in &scopes {
            if request_only && limits.tpm.is_some() {
                return Err(ApiError::new(StatusCode::FORBIDDEN, "opaque_tpm_unsupported", "Pass-through requires a key and owner without token-per-minute limits"));
            }
            let concurrent = counts.get(scope).copied().unwrap_or(0);
            if limits
                .max_concurrent
                .is_some_and(|limit| concurrent >= u64::from(limit))
            {
                return Err(rate_error(*scope, "concurrency"));
            }
            if concurrent.checked_add(1).is_none() {
                return Err(rate_error(*scope, "concurrency"));
            }
            if limits.rpm.is_some() || limits.tpm.is_some() {
                let (requests, spent) = window(&transaction, *scope, cutoff)?;
                if let Some(rpm) = limits.rpm {
                    if requests >= u64::from(rpm) {
                        return Err(rate_error(*scope, "RPM"));
                    }
                }
                if let Some(tpm) = limits.tpm {
                    if tpm == 0
                        || spent
                            .checked_add(tokens as u64)
                            .is_none_or(|total| total > tpm)
                    {
                        return Err(rate_error(*scope, "TPM"));
                    }
                }
            }
        }
        transaction
            .execute("DELETE FROM quota_events WHERE at_ms<=?1", [cutoff])
            .map_err(storage)?;
        for (scope, _) in &scopes {
            transaction
                .execute(
                    "INSERT INTO quota_events(scope,scope_id,at_ms,tokens) VALUES(?1,?2,?3,?4)",
                    params![scope.kind, scope.id, now, tokens],
                )
                .map_err(storage)?;
        }
        transaction.commit().map_err(storage)?;
        let scopes: Vec<_> = scopes.into_iter().map(|(scope, _)| scope).collect();
        for scope in &scopes {
            *counts.entry(*scope).or_default() += 1;
        }
        scopes
    };
    Ok(AdmissionPermit { state, scopes })
}
fn window(conn: &Connection, scope: Scope, cutoff: i64) -> Result<(u64, u64), ApiError> {
    let mut query = conn
        .prepare("SELECT tokens FROM quota_events WHERE scope=?1 AND scope_id=?2 AND at_ms>?3")
        .map_err(storage)?;
    let rows = query
        .query_map(params![scope.kind, scope.id, cutoff], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(storage)?;
    let mut requests = 0u64;
    let mut tokens = 0u64;
    for row in rows {
        let value = u64::try_from(row.map_err(storage)?)
            .map_err(|_| ApiError::internal("Invalid admission token reservation"))?;
        requests = requests
            .checked_add(1)
            .ok_or_else(|| rate_error(scope, "RPM"))?;
        tokens = tokens
            .checked_add(value)
            .ok_or_else(|| rate_error(scope, "TPM"))?;
    }
    Ok((requests, tokens))
}
