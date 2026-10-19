//! Durable per-attempt billing. Money uses integer micro-units; no floating point.
//! Ledger identity snapshots intentionally have no FK to diagnostic or mutable tables.
use super::{now_ms, DbError, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub id: i64,
    pub channel_id: i64,
    pub model: String,
    pub input_micro_per_million: i64,
    pub output_micro_per_million: i64,
    pub max_input_tokens: i64,
    pub max_output_tokens: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct Budget {
    pub scope: String,
    pub scope_id: i64,
    pub limit_micro: Option<i64>,
    pub spent_micro: i64,
    pub reserved_micro: i64,
    pub frozen: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Ledger {
    pub id: i64,
    pub attempt_id: String,
    pub user_id: i64,
    pub key_id: i64,
    pub request_id: i64,
    pub channel_id: i64,
    pub model: String,
    pub price_id: Option<i64>,
    pub price_snapshot: Option<Price>,
    pub auth_snapshot: serde_json::Value,
    pub source_snapshot: serde_json::Value,
    pub input_bound: i64,
    pub output_bound: i64,
    pub reserved_micro: i64,
    pub debit_micro: Option<i64>,
    pub observed_cost_micro: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub usage_certainty: String,
    pub status: String,
    pub outcome: Option<String>,
    pub overrun: bool,
    pub created_ms: i64,
    pub settled_ms: Option<i64>,
}
fn bad(message: &str) -> DbError {
    DbError::Other(message.into())
}
fn tx(conn: &Connection) -> Result<Transaction<'_>> {
    // A financial hold must survive power loss before an upstream send. WAL
    // NORMAL is insufficient for this contract; retain FULL on this connection.
    conn.pragma_update(None, "synchronous", "FULL")?;
    Ok(Transaction::new_unchecked(
        conn,
        TransactionBehavior::Immediate,
    )?)
}
fn scope_ok(scope: &str, id: i64) -> Result<()> {
    if !matches!(scope, "user" | "key") || id <= 0 {
        return Err(bad("invalid budget scope"));
    }
    Ok(())
}
fn audit(conn: &Connection, event: &str, actor: &str, object: &str, detail: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO billing_audit(event,actor,object,detail,created_ms) VALUES(?1,?2,?3,?4,?5)",
        params![event, actor, object, detail, now_ms()],
    )?;
    Ok(())
}
fn price_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Price> {
    Ok(Price {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        model: r.get(2)?,
        input_micro_per_million: r.get(3)?,
        output_micro_per_million: r.get(4)?,
        max_input_tokens: r.get(5)?,
        max_output_tokens: r.get(6)?,
    })
}
pub fn price(conn: &Connection, model: &str, channel_id: i64) -> Result<Option<Price>> {
    Ok(conn.query_row("SELECT id,channel_id,model,input_micro_per_million,output_micro_per_million,max_input_tokens,max_output_tokens FROM billing_prices WHERE channel_id=?1 AND model=?2 ORDER BY id DESC LIMIT 1",params![channel_id,tokengo_shared::normalize_model_name(model)],price_row).optional()?)
}
pub fn list_prices(conn: &Connection) -> Result<Vec<Price>> {
    let mut s=conn.prepare("SELECT id,channel_id,model,input_micro_per_million,output_micro_per_million,max_input_tokens,max_output_tokens FROM billing_prices ORDER BY id DESC LIMIT 1000")?;
    let rows = s
        .query_map([], price_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn create_price(conn: &Connection, p: &Price, actor: &str) -> Result<Price> {
    if p.channel_id <= 0
        || p.model.trim().is_empty()
        || p.model.len() > 256
        || [
            p.input_micro_per_million,
            p.output_micro_per_million,
            p.max_input_tokens,
            p.max_output_tokens,
        ]
        .iter()
        .any(|n| *n < 0)
    {
        return Err(bad("invalid pricing values"));
    }
    // Validate even the maximum reservation before publishing a version.
    cost(p, p.max_input_tokens, p.max_output_tokens)?;
    let c = tx(conn)?;
    c.execute("INSERT INTO billing_prices(channel_id,model,input_micro_per_million,output_micro_per_million,max_input_tokens,max_output_tokens,created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![p.channel_id,tokengo_shared::normalize_model_name(&p.model),p.input_micro_per_million,p.output_micro_per_million,p.max_input_tokens,p.max_output_tokens,now_ms()])?;
    let out = Price {
        id: c.last_insert_rowid(),
        model: tokengo_shared::normalize_model_name(&p.model),
        ..p.clone()
    };
    audit(
        &c,
        "price_created",
        actor,
        &out.id.to_string(),
        &serde_json::to_string(&out).map_err(|_| bad("price serialization failed"))?,
    )?;
    c.commit()?;
    Ok(out)
}
fn budget_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Budget> {
    Ok(Budget {
        scope: r.get(0)?,
        scope_id: r.get(1)?,
        limit_micro: r.get(2)?,
        spent_micro: r.get(3)?,
        reserved_micro: r.get(4)?,
        frozen: r.get(5)?,
    })
}
pub fn budgets(conn: &Connection, scope: Option<&str>, id: Option<i64>) -> Result<Vec<Budget>> {
    if let Some(s) = scope {
        if !matches!(s, "user" | "key") {
            return Err(bad("invalid budget scope"));
        }
    }
    let mut s=conn.prepare("SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen FROM billing_budgets WHERE (?1 IS NULL OR scope=?1) AND (?2 IS NULL OR scope_id=?2) ORDER BY scope,scope_id LIMIT 1000")?;
    let out = s
        .query_map(params![scope, id], budget_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(out)
}
pub fn set_budget(
    conn: &Connection,
    scope: &str,
    id: i64,
    limit: Option<i64>,
    actor: &str,
) -> Result<()> {
    scope_ok(scope, id)?;
    if limit.is_some_and(|n| n < 0) {
        return Err(bad("negative budget"));
    }
    let c = tx(conn)?;
    c.execute("INSERT INTO billing_budgets(scope,scope_id,limit_micro) VALUES(?1,?2,?3) ON CONFLICT(scope,scope_id) DO UPDATE SET limit_micro=excluded.limit_micro",params![scope,id,limit])?;
    audit(
        &c,
        "budget_updated",
        actor,
        &format!("{scope}:{id}"),
        &serde_json::json!({"limit_micro":limit}).to_string(),
    )?;
    c.commit()?;
    Ok(())
}
/// Ceiling of the combined input/output charge, in integer micro-units.
pub fn cost(p: &Price, input: i64, output: i64) -> Result<i64> {
    if input < 0 || output < 0 || p.input_micro_per_million < 0 || p.output_micro_per_million < 0 {
        return Err(bad("negative billing quantity"));
    }
    let sum = i128::from(input)
        .checked_mul(i128::from(p.input_micro_per_million))
        .and_then(|v| {
            v.checked_add(i128::from(output).checked_mul(i128::from(p.output_micro_per_million))?)
        })
        .and_then(|v| v.checked_add(999_999))
        .ok_or_else(|| bad("billing cost overflow"))?
        / 1_000_000;
    i64::try_from(sum).map_err(|_| bad("billing cost overflow"))
}
fn allocation_scopes(
    c: &Connection,
    user_id: i64,
    key_id: i64,
) -> Result<Vec<(&'static str, i64)>> {
    let auth = super::identity::context(c, key_id)?.ok_or_else(|| bad("inactive gateway key"))?;
    if auth.user_id != user_id {
        return Err(bad("billing identity mismatch"));
    }
    let mut allocations = vec![("user", user_id)];
    let mut seen = std::collections::HashSet::new();
    let mut cursor = Some(key_id);
    while let Some(id) = cursor {
        if !seen.insert(id) || seen.len() > 64 {
            return Err(bad("invalid key ancestry"));
        }
        allocations.push(("key", id));
        cursor = c
            .query_row(
                "SELECT parent_key_id FROM gateway_keys WHERE id=?1",
                [id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .ok_or_else(|| bad("unknown gateway key"))?;
    }
    Ok(allocations)
}
pub fn reserve(
    conn: &Connection,
    user_id: i64,
    key_id: i64,
    request_id: i64,
    attempt_id: &str,
    p: &Price,
    input: i64,
    output: i64,
) -> Result<i64> {
    if user_id <= 0
        || key_id <= 0
        || attempt_id.is_empty()
        || attempt_id.len() > 256
        || input < 0
        || output < 0
        || input > p.max_input_tokens
        || output > p.max_output_tokens
    {
        return Err(bad("invalid reservation bounds or identity"));
    }
    let amount = cost(p, input, output)?;
    let c = tx(conn)?;
    let existing:Option<(i64,i64,i64,i64,i64,i64,i64)>=c.query_row("SELECT id,user_id,key_id,request_id,price_id,input_bound,output_bound FROM billing_ledger WHERE attempt_id=?1",[attempt_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    if let Some((id, u, k, r, price_id, i, o)) = existing {
        if (u, k, r, price_id, i, o) != (user_id, key_id, request_id, p.id, input, output) {
            return Err(bad("attempt id conflicts with existing reservation"));
        }
        c.commit()?;
        return Ok(id);
    }
    let stored=c.query_row("SELECT id,channel_id,model,input_micro_per_million,output_micro_per_million,max_input_tokens,max_output_tokens FROM billing_prices WHERE id=?1",[p.id],price_row).optional()?.ok_or_else(||bad("unknown pricing version"))?;
    if stored != *p {
        return Err(bad("pricing snapshot mismatch"));
    }
    let allocations = allocation_scopes(&c, user_id, key_id)?;
    for (scope, id) in &allocations {
        c.execute(
            "INSERT OR IGNORE INTO billing_budgets(scope,scope_id) VALUES(?1,?2)",
            params![scope, id],
        )?;
        let b=c.query_row("SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen FROM billing_budgets WHERE scope=?1 AND scope_id=?2",params![scope,id],budget_row)?;
        // A reconciliation freeze is independent of the current spending cap.
        if b.frozen {
            return Err(bad("billing budget frozen"));
        }
        let reserved = b
            .reserved_micro
            .checked_add(amount)
            .ok_or_else(|| bad("budget reservation overflow"))?;
        let total = b
            .spent_micro
            .checked_add(reserved)
            .ok_or_else(|| bad("budget total overflow"))?;
        if b.limit_micro.is_some_and(|limit| total > limit) {
            return Err(bad("billing budget exhausted"));
        }
        c.execute(
            "UPDATE billing_budgets SET reserved_micro=?3 WHERE scope=?1 AND scope_id=?2",
            params![scope, id, reserved],
        )?;
    }
    c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,price_id,price_snapshot,input_bound,output_bound,reserved_micro,created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![attempt_id,user_id,key_id,request_id,p.channel_id,p.model,p.id,serde_json::to_string(p).map_err(|_|bad("price serialization failed"))?,input,output,amount,now_ms()])?;
    let id = c.last_insert_rowid();
    let auth = super::identity::context(&c, key_id)?.ok_or_else(|| bad("inactive gateway key"))?;
    let snapshot =
        serde_json::to_string(&auth).map_err(|_| bad("identity snapshot serialization failed"))?;
    c.execute(
        "UPDATE billing_ledger SET auth_snapshot=?2 WHERE id=?1",
        params![id, snapshot],
    )?;
    for (scope, scope_id) in allocations {
        c.execute("INSERT INTO billing_allocations(reservation_id,scope,scope_id,reserved_micro) VALUES(?1,?2,?3,?4)",params![id,scope,scope_id,amount])?;
    }
    c.commit()?;
    Ok(id)
}
const LEDGER_SELECT:&str="SELECT id,attempt_id,user_id,key_id,request_id,channel_id,model,price_id,price_snapshot,input_bound,output_bound,reserved_micro,debit_micro,observed_cost_micro,input_tokens,output_tokens,usage_certainty,status,overrun,created_ms,settled_ms,outcome,auth_snapshot,source_snapshot FROM billing_ledger";
fn ledger_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Ledger> {
    let raw: Option<String> = r.get(8)?;
    let p = raw
        .map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?;
    Ok(Ledger {
        id: r.get(0)?,
        attempt_id: r.get(1)?,
        user_id: r.get(2)?,
        key_id: r.get(3)?,
        request_id: r.get(4)?,
        channel_id: r.get(5)?,
        model: r.get(6)?,
        price_id: r.get(7)?,
        price_snapshot: p,
        input_bound: r.get(9)?,
        output_bound: r.get(10)?,
        reserved_micro: r.get(11)?,
        debit_micro: r.get(12)?,
        observed_cost_micro: r.get(13)?,
        input_tokens: r.get(14)?,
        output_tokens: r.get(15)?,
        usage_certainty: r.get(16)?,
        status: r.get(17)?,
        overrun: r.get(18)?,
        created_ms: r.get(19)?,
        settled_ms: r.get(20)?,
        outcome: r.get(21)?,
        source_snapshot: serde_json::from_str(&r.get::<_, String>(23)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(23, rusqlite::types::Type::Text, Box::new(e))
        })?,
        auth_snapshot: serde_json::from_str(&r.get::<_, String>(22)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(22, rusqlite::types::Type::Text, Box::new(e))
        })?,
    })
}
/// Immutable terminal settlement. Unknown usage conservatively consumes the hold.
pub fn settle(
    conn: &Connection,
    id: i64,
    input: Option<i64>,
    output: Option<i64>,
    status: &str,
) -> Result<()> {
    finish(conn, id, input, output, status, false)
}
pub fn release(conn: &Connection, id: i64, status: &str) -> Result<()> {
    finish(conn, id, None, None, status, true)
}
fn finish(
    conn: &Connection,
    id: i64,
    input: Option<i64>,
    output: Option<i64>,
    status: &str,
    release: bool,
) -> Result<()> {
    if input.is_some_and(|v| v < 0)
        || output.is_some_and(|v| v < 0)
        || status.is_empty()
        || status == "reserved"
        || status.len() > 64
    {
        return Err(bad("invalid settlement"));
    }
    let c = tx(conn)?;
    let row = c
        .query_row(&format!("{LEDGER_SELECT} WHERE id=?1"), [id], ledger_row)
        .optional()?
        .ok_or_else(|| bad("unknown reservation"))?;
    if row.status != "reserved" {
        c.commit()?;
        return Ok(());
    }
    let observed = match (&row.price_snapshot, input, output) {
        (Some(p), Some(i), Some(o)) => Some(cost(p, i, o)?),
        _ => None,
    };
    let debit = if release {
        0
    } else {
        observed
            .unwrap_or(row.reserved_micro)
            .min(row.reserved_micro)
    };
    // Even partial usage can prove that the reserved upper bound was exceeded.
    let minimum_observed = row
        .price_snapshot
        .as_ref()
        .map(|p| cost(p, input.unwrap_or(0), output.unwrap_or(0)))
        .transpose()?;
    let overrun = minimum_observed.is_some_and(|v| v > row.reserved_micro);
    let allocations = {
        let mut s =
            c.prepare("SELECT scope,scope_id FROM billing_allocations WHERE reservation_id=?1")?;
        let rows = s
            .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (scope, scope_id) in allocations {
        let b=c.query_row("SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen FROM billing_budgets WHERE scope=?1 AND scope_id=?2",params![scope,scope_id],budget_row)?;
        let reserved = b
            .reserved_micro
            .checked_sub(row.reserved_micro)
            .filter(|v| *v >= 0)
            .ok_or_else(|| bad("inconsistent billing hold"))?;
        let spent = b
            .spent_micro
            .checked_add(debit)
            .ok_or_else(|| bad("billing spend overflow"))?;
        c.execute("UPDATE billing_budgets SET reserved_micro=?3,spent_micro=?4,frozen=MAX(frozen,?5) WHERE scope=?1 AND scope_id=?2",params![scope,scope_id,reserved,spent,overrun])?;
    }
    let unpriced = row.price_snapshot.is_none() && !release;
    let certainty = if release {
        "not_sent"
    } else if unpriced {
        "unpriced"
    } else if observed.is_some() {
        "reported"
    } else {
        "estimated"
    };
    let debit = if unpriced { None } else { Some(debit) };
    let terminal = if unpriced { "unpriced" } else { status };
    c.execute("UPDATE billing_ledger SET debit_micro=?2,observed_cost_micro=?3,input_tokens=?4,output_tokens=?5,usage_certainty=?6,status=?7,overrun=?8,settled_ms=?9,outcome=?10 WHERE id=?1",params![id,debit,observed,input,output,certainty,terminal,overrun,now_ms(),status])?;
    c.commit()?;
    Ok(())
}
/// Outstanding sends may have reached the provider. Never release unknown cost at restart.
pub fn recover(conn: &Connection) -> Result<usize> {
    let ids = {
        let mut s =
            conn.prepare("SELECT id FROM billing_ledger WHERE status='reserved' ORDER BY id")?;
        let rows = s
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let n = ids.len();
    for id in ids {
        settle(conn, id, None, None, "restart_unknown")?;
    }
    Ok(n)
}
#[derive(Debug, Default, Deserialize)]
pub struct UsageQuery {
    pub user_id: Option<i64>,
    pub key_id: Option<i64>,
    pub channel_id: Option<i64>,
    pub model: Option<String>,
    pub before_id: Option<i64>,
    pub limit: Option<u32>,
}
pub fn usage(conn: &Connection, q: &UsageQuery) -> Result<Vec<Ledger>> {
    let sql=format!("{LEDGER_SELECT} WHERE (?1 IS NULL OR user_id=?1) AND (?2 IS NULL OR key_id=?2) AND (?3 IS NULL OR channel_id=?3) AND (?4 IS NULL OR model=?4) AND (?5 IS NULL OR id<?5) ORDER BY id DESC LIMIT ?6");
    let mut s = conn.prepare(&sql)?;
    let rows = s
        .query_map(
            params![
                q.user_id,
                q.key_id,
                q.channel_id,
                q.model.as_deref().map(tokengo_shared::normalize_model_name),
                q.before_id,
                q.limit.unwrap_or(50).clamp(1, 200)
            ],
            ledger_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Compatibility for unbudgeted existing keys. Unknown price is never a zero price.
pub fn reserve_unpriced(
    conn: &Connection,
    user_id: i64,
    key_id: i64,
    request_id: i64,
    attempt_id: &str,
    channel_id: i64,
    model: &str,
) -> Result<i64> {
    if user_id <= 0
        || key_id <= 0
        || channel_id <= 0
        || attempt_id.is_empty()
        || attempt_id.len() > 256
        || model.trim().is_empty()
        || model.len() > 256
    {
        return Err(bad("invalid unpriced reservation"));
    }
    let c = tx(conn)?;
    let model = tokengo_shared::normalize_model_name(model);
    let existing:Option<(i64,i64,i64,i64,i64,String,Option<i64>)>=c.query_row("SELECT id,user_id,key_id,request_id,channel_id,model,price_id FROM billing_ledger WHERE attempt_id=?1",[attempt_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    if let Some((id, u, k, r, ch, m, p)) = existing {
        if (u, k, r, ch, m, p) != (user_id, key_id, request_id, channel_id, model, None) {
            return Err(bad("attempt id conflicts with existing reservation"));
        }
        c.commit()?;
        return Ok(id);
    }
    let allocations = allocation_scopes(&c, user_id, key_id)?;
    for (scope, id) in &allocations {
        let budget = c
            .query_row(
                "SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen FROM billing_budgets WHERE scope=?1 AND scope_id=?2",
                params![scope, id],
                budget_row,
            )
            .optional()?;
        if let Some(budget) = budget {
            if budget.frozen {
                return Err(bad("billing budget frozen"));
            }
            if budget.limit_micro.is_some() {
                return Err(bad(
                    "config_required: budgeted attempts require explicit pricing",
                ));
            }
        }
        c.execute(
            "INSERT OR IGNORE INTO billing_budgets(scope,scope_id) VALUES(?1,?2)",
            params![scope, id],
        )?;
    }
    c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms) VALUES(?1,?2,?3,?4,?5,?6,0,0,0,?7)",params![attempt_id,user_id,key_id,request_id,channel_id,model,now_ms()])?;
    let id = c.last_insert_rowid();
    let auth = super::identity::context(&c, key_id)?.ok_or_else(|| bad("inactive gateway key"))?;
    let snapshot =
        serde_json::to_string(&auth).map_err(|_| bad("identity snapshot serialization failed"))?;
    c.execute(
        "UPDATE billing_ledger SET auth_snapshot=?2 WHERE id=?1",
        params![id, snapshot],
    )?;
    for (scope, scope_id) in allocations {
        c.execute("INSERT INTO billing_allocations(reservation_id,scope,scope_id,reserved_micro) VALUES(?1,?2,?3,0)",params![id,scope,scope_id])?;
    }
    c.commit()?;
    Ok(id)
}
