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
/// Immutable per-successful-request pricing, distinct from token pricing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RequestPrice {
    pub id: i64,
    pub channel_id: i64,
    pub model: String,
    pub micro_per_call: i64,
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
    pub billing_unit: String,
    pub request_units: Option<i64>,
    pub request_price_id: Option<i64>,
    pub request_price_snapshot: Option<RequestPrice>,
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
    super::channels::require_llm_transport(conn,tokengo_shared::ChannelId(p.channel_id))?;
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
const LEDGER_SELECT:&str="SELECT id,attempt_id,user_id,key_id,request_id,channel_id,model,price_id,price_snapshot,input_bound,output_bound,reserved_micro,debit_micro,observed_cost_micro,input_tokens,output_tokens,usage_certainty,status,overrun,created_ms,settled_ms,outcome,auth_snapshot,source_snapshot,billing_unit,request_units,request_price_id,request_price_snapshot FROM billing_ledger";
fn ledger_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Ledger> {
    let raw: Option<String> = r.get(8)?;
    let p = raw
        .map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let request_price_snapshot = r
        .get::<_, Option<String>>(27)?
        .map(|v| serde_json::from_str(&v))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(27, rusqlite::types::Type::Text, Box::new(e))
        })?;
    Ok(Ledger {
        billing_unit: r.get(24)?,
        request_units: r.get(25)?,
        request_price_id: r.get(26)?,
        request_price_snapshot,
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
/// Session-owned compatibility probes charge only the real user's budget. They
/// never impersonate a VKey, create ordinary request rows, or use admin diagnostics.
/// Unknown usage retains the priced hold through the normal settlement/recovery path.
#[allow(clippy::too_many_arguments)]
pub fn reserve_user_diagnostic(conn:&Connection,user_id:i64,attempt:&str,source:i64,model:&str,input:i64,source_snapshot:&serde_json::Value)->Result<i64> {
    if user_id<=0||source<=0||attempt.is_empty()||attempt.len()>256||model.trim().is_empty()||input<0||source_snapshot["source_id"]!=serde_json::json!(source) {return Err(bad("invalid user diagnostic identity"));}
    let c=tx(conn)?;
    let auth=super::user_virtual_routes::owner_auth(&c,user_id).map_err(|_|bad("inactive diagnostic user"))?;
    let model=tokengo_shared::normalize_model_name(model);
    let p=price(&c,&model,source)?;
    if !auth.permits_model(&model)||!auth.permits_channel(source,p.as_ref().is_none_or(|p|p.input_micro_per_million>0||p.output_micro_per_million>0)) {return Err(bad("diagnostic source not authorized"));}
    if p.as_ref().is_some_and(|p|input>p.max_input_tokens) {return Err(bad("diagnostic input exceeds configured pricing bound"));}
    let amount=p.as_ref().map(|p|cost(p,input,0)).transpose()?.unwrap_or(0);
    let existing=c.query_row(&format!("{LEDGER_SELECT} WHERE attempt_id=?1"),[attempt],ledger_row).optional()?;
    if let Some(row)=existing {
        if row.user_id!=user_id||row.key_id!=0||row.request_id!=0||row.channel_id!=source||row.model!=model||row.input_bound!=input||row.output_bound!=0||row.price_id!=p.as_ref().map(|p|p.id)||row.source_snapshot!=*source_snapshot||row.auth_snapshot["kind"]!="user_compatibility" {return Err(bad("user diagnostic attempt identity conflict"));}
        c.commit()?;return Ok(row.id);
    }
    c.execute("INSERT OR IGNORE INTO billing_budgets(scope,scope_id) VALUES('user',?1)",[user_id])?;
    let b=c.query_row("SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen FROM billing_budgets WHERE scope='user' AND scope_id=?1",[user_id],budget_row)?;
    if b.frozen {return Err(bad("billing budget frozen"));}
    if p.is_none()&&b.limit_micro.is_some() {return Err(bad("config_required: budgeted diagnostics require explicit pricing"));}
    let reserved=b.reserved_micro.checked_add(amount).ok_or_else(||bad("diagnostic budget overflow"))?;
    let total=b.spent_micro.checked_add(reserved).ok_or_else(||bad("diagnostic budget overflow"))?;
    if b.limit_micro.is_some_and(|limit|total>limit) {return Err(bad("billing budget exhausted"));}
    c.execute("UPDATE billing_budgets SET reserved_micro=?2 WHERE scope='user' AND scope_id=?1",params![user_id,reserved])?;
    let auth=serde_json::json!({"kind":"user_compatibility","user_id":user_id,"user_policy":auth.user_policy});
    c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,price_id,price_snapshot,input_bound,output_bound,reserved_micro,created_ms,auth_snapshot,source_snapshot) VALUES(?1,?2,0,0,?3,?4,?5,?6,?7,0,?8,?9,?10,?11)",params![attempt,user_id,source,model,p.as_ref().map(|p|p.id),p.as_ref().map(serde_json::to_string).transpose().map_err(|_|bad("diagnostic pricing serialization"))?,input,amount,now_ms(),auth.to_string(),source_snapshot.to_string()])?;
    let id=c.last_insert_rowid();
    c.execute("INSERT INTO billing_allocations(reservation_id,scope,scope_id,reserved_micro) VALUES(?1,'user',?2,?3)",params![id,user_id,amount])?;
    c.commit()?;Ok(id)
}

pub fn settle(
    conn: &Connection,
    id: i64,
    input: Option<i64>,
    output: Option<i64>,
    status: &str,
) -> Result<()> {
    finish(conn, id, input, output, status, false, None, false)
}
pub fn release(conn: &Connection, id: i64, status: &str) -> Result<()> {
    finish(conn, id, None, None, status, true, Some(0), false)
}
fn finish(
    conn: &Connection,
    id: i64,
    input: Option<i64>,
    output: Option<i64>,
    status: &str,
    release: bool,
    request_units: Option<i64>,
    is_request: bool,
) -> Result<()> {
    if request_units.is_some_and(|n| !(0..=1).contains(&n))
        || input.is_some_and(|v| v < 0)
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
    if !release && (row.billing_unit == "request") != is_request {
        return Err(bad("settlement billing unit mismatch"));
    }
    if row.status != "reserved" {
        c.commit()?;
        return Ok(());
    }
    let observed = if row.billing_unit == "request" {
        match (&row.request_price_snapshot, request_units) {
            (Some(p), Some(n)) => Some(
                p.micro_per_call
                    .checked_mul(n)
                    .ok_or_else(|| bad("request cost overflow"))?,
            ),
            _ => None,
        }
    } else {
        match (&row.price_snapshot, input, output) {
            (Some(p), Some(i), Some(o)) => Some(cost(p, i, o)?),
            _ => None,
        }
    };
    let debit = if release {
        0
    } else {
        observed
            .unwrap_or(row.reserved_micro)
            .min(row.reserved_micro)
    };
    // Even partial usage can prove that the reserved upper bound was exceeded.
    let minimum_observed = if row.billing_unit == "request" {
        observed
    } else {
        row.price_snapshot
            .as_ref()
            .map(|p| cost(p, input.unwrap_or(0), output.unwrap_or(0)))
            .transpose()?
    };
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
    let unpriced = (if row.billing_unit == "request" {
        row.request_price_snapshot.is_none()
    } else {
        row.price_snapshot.is_none()
    }) && !release;
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
    c.execute("UPDATE billing_ledger SET debit_micro=?2,observed_cost_micro=?3,input_tokens=?4,output_tokens=?5,usage_certainty=?6,status=?7,overrun=?8,settled_ms=?9,outcome=?10,request_units=?11 WHERE id=?1",params![id,debit,observed,input,output,certainty,terminal,overrun,now_ms(),status,if row.billing_unit=="request" {request_units}else{None}])?;
    c.commit()?;
    Ok(())
}
/// Internal diagnostic accounting never impersonates a user/VKey or allocates their budget.
/// Pending rows are durable before dispatch; absent prices remain unknown, not free.
#[allow(clippy::too_many_arguments)]
pub fn reserve_diagnostic(conn:&Connection,attempt:&str,source:i64,model:&str,request_based:bool,subscription:bool,auth:&serde_json::Value,snapshot:&serde_json::Value)->Result<i64> {
    if source<=0||model.trim().is_empty()||attempt.is_empty()||attempt.len()>256||auth["kind"]!="admin_diagnostic"||auth["subscription"]!=serde_json::json!(subscription) {return Err(bad("invalid diagnostic identity"));}
    let c=tx(conn)?;
    let existing=c.query_row(&format!("{LEDGER_SELECT} WHERE attempt_id=?1"),[attempt],ledger_row).optional()?;
    if let Some(row)=existing {if row.user_id!=0||row.key_id!=0||row.request_id!=0||row.channel_id!=source||row.model!=tokengo_shared::normalize_model_name(model)||row.billing_unit!=if request_based{"request"}else{"token"}||row.auth_snapshot!=*auth||row.source_snapshot!=*snapshot{return Err(bad("diagnostic attempt identity conflict"));}c.commit()?;return Ok(row.id);}
    let token_price=if request_based||subscription{None}else{price(&c,model,source)?};
    let per_request=if request_based{request_price(&c,model,source)?}else{None};
    c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,price_id,price_snapshot,input_bound,output_bound,reserved_micro,created_ms,billing_unit,request_price_id,request_price_snapshot,auth_snapshot,source_snapshot) VALUES(?1,0,0,0,?2,?3,?4,?5,0,0,0,?6,?7,?8,?9,?10,?11)",params![attempt,source,tokengo_shared::normalize_model_name(model),token_price.as_ref().map(|p|p.id),token_price.as_ref().map(serde_json::to_string).transpose().map_err(|_|bad("invalid diagnostic price"))?,now_ms(),if request_based{"request"}else{"token"},per_request.as_ref().map(|p|p.id),per_request.as_ref().map(serde_json::to_string).transpose().map_err(|_|bad("invalid diagnostic price"))?,auth.to_string(),snapshot.to_string()])?;
    let id=c.last_insert_rowid();c.commit()?;Ok(id)
}
/// Refresh immutable dispatch evidence and price versions after credential refresh/queue waits.
pub fn prepare_diagnostic_dispatch(conn:&Connection,id:i64,snapshot:&serde_json::Value)->Result<()> {
    let c=tx(conn)?;let row=c.query_row(&format!("{LEDGER_SELECT} WHERE id=?1"),[id],ledger_row)?;
    if row.user_id!=0||row.key_id!=0||row.request_id!=0||row.auth_snapshot["kind"]!="admin_diagnostic"||row.status!="reserved" {return Err(bad("invalid diagnostic dispatch"));}
    let token=if row.billing_unit=="token"&&row.auth_snapshot["subscription"]!=true {price(&c,&row.model,row.channel_id)?}else{None};
    let request=if row.billing_unit=="request" {request_price(&c,&row.model,row.channel_id)?}else{None};
    c.execute("UPDATE billing_ledger SET source_snapshot=?2,price_id=?3,price_snapshot=?4,request_price_id=?5,request_price_snapshot=?6 WHERE id=?1",params![id,snapshot.to_string(),token.as_ref().map(|p|p.id),token.as_ref().map(serde_json::to_string).transpose().map_err(|_|bad("diagnostic price serialization"))?,request.as_ref().map(|p|p.id),request.as_ref().map(serde_json::to_string).transpose().map_err(|_|bad("diagnostic price serialization"))?])?;
    c.commit()?;Ok(())
}
/// Idempotent system settlement. No allocation rows or user budget updates are permitted.
pub fn settle_diagnostic(conn:&Connection,id:i64,input:Option<i64>,output:Option<i64>,units:Option<i64>,outcome:&str)->Result<()> {
    if input.is_some_and(|n|n<0)||output.is_some_and(|n|n<0)||units.is_some_and(|n|!(0..=1).contains(&n))||outcome.is_empty()||outcome.len()>64||outcome=="reserved" {return Err(bad("invalid diagnostic settlement"));}
    let c=tx(conn)?;
    let row=c.query_row(&format!("{LEDGER_SELECT} WHERE id=?1"),[id],ledger_row).optional()?.ok_or_else(||bad("unknown diagnostic reservation"))?;
    if row.user_id!=0||row.key_id!=0||row.request_id!=0||row.auth_snapshot["kind"]!="admin_diagnostic" {return Err(bad("diagnostic settlement identity mismatch"));}
    if row.status!="reserved" {c.commit()?;return Ok(());}
    let allocated:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM billing_allocations WHERE reservation_id=?1)",[id],|r|r.get(0))?;
    if allocated {return Err(bad("system diagnostic must not hold user allocations"));}
    if (row.billing_unit=="request"&&(input.is_some()||output.is_some()))||(row.billing_unit!="request"&&units.is_some()) {return Err(bad("diagnostic billing unit mismatch"));}
    let not_sent=outcome=="not_sent";
    let units=if not_sent&&row.billing_unit=="request"{Some(0)}else{units};
    let subscription=row.auth_snapshot["subscription"]==true;
    let observed=if not_sent||units==Some(0){Some(0)}else if subscription{None}else if row.billing_unit=="request"{match (&row.request_price_snapshot,units){(Some(p),Some(n))=>Some(p.micro_per_call.checked_mul(n).ok_or_else(||bad("diagnostic request cost overflow"))?),_=>None}}else{match(&row.price_snapshot,input,output){(Some(p),Some(i),Some(o))=>Some(cost(p,i,o)?),_=>None}};
    let certainty=if not_sent{"not_sent"}else if subscription{"subscription"}else if observed.is_some(){"reported"}else if (row.billing_unit=="request"&&row.request_price_snapshot.is_none())||(row.billing_unit!="request"&&row.price_snapshot.is_none()){"unpriced"}else{"unknown"};
    c.execute("UPDATE billing_ledger SET debit_micro=0,observed_cost_micro=?2,input_tokens=?3,output_tokens=?4,request_units=?5,usage_certainty=?6,status=?7,outcome=?7,settled_ms=?8 WHERE id=?1",params![id,observed,input,output,units,certainty,outcome,now_ms()])?;
    c.commit()?;Ok(())
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
        let unit: String = conn.query_row(
            "SELECT billing_unit FROM billing_ledger WHERE id=?1",
            [id],
            |r| r.get(0),
        )?;
        let diagnostic:bool=conn.query_row("SELECT user_id=0 AND key_id=0 AND request_id=0 AND json_extract(auth_snapshot,'$.kind')='admin_diagnostic' FROM billing_ledger WHERE id=?1",[id],|r|r.get(0))?;
        if diagnostic {settle_diagnostic(conn,id,None,None,None,"restart_unknown")?;}
        else if unit == "request" {
            settle_request(conn, id, None, "restart_unknown")?;
        } else {
            settle(conn, id, None, None, "restart_unknown")?;
        }
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

fn request_price_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RequestPrice> {
    Ok(RequestPrice {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        model: r.get(2)?,
        micro_per_call: r.get(3)?,
    })
}
pub fn request_price(
    conn: &Connection,
    model: &str,
    channel_id: i64,
) -> Result<Option<RequestPrice>> {
    Ok(conn.query_row("SELECT id,channel_id,model,micro_per_call FROM billing_request_prices WHERE channel_id=?1 AND model=?2 ORDER BY id DESC LIMIT 1",params![channel_id,tokengo_shared::normalize_model_name(model)],request_price_row).optional()?)
}
pub fn list_request_prices(conn: &Connection) -> Result<Vec<RequestPrice>> {
    let mut s=conn.prepare("SELECT id,channel_id,model,micro_per_call FROM billing_request_prices ORDER BY id DESC LIMIT 1000")?;
    let rows = s
        .query_map([], request_price_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn create_request_price(
    conn: &Connection,
    p: &RequestPrice,
    actor: &str,
) -> Result<RequestPrice> {
    super::channels::require_llm_transport(conn,tokengo_shared::ChannelId(p.channel_id))?;
    if p.channel_id <= 0 || p.model.trim().is_empty() || p.model.len() > 256 || p.micro_per_call < 0
    {
        return Err(bad("invalid request pricing"));
    }
    let c = tx(conn)?;
    let model = tokengo_shared::normalize_model_name(&p.model);
    c.execute("INSERT INTO billing_request_prices(channel_id,model,micro_per_call,created_ms) VALUES(?1,?2,?3,?4)",params![p.channel_id,model,p.micro_per_call,now_ms()])?;
    let out = RequestPrice {
        id: c.last_insert_rowid(),
        channel_id: p.channel_id,
        model,
        micro_per_call: p.micro_per_call,
    };
    audit(
        &c,
        "request_price_created",
        actor,
        &out.id.to_string(),
        &serde_json::to_string(&out).map_err(|_| bad("request price serialization"))?,
    )?;
    c.commit()?;
    Ok(out)
}
/// One request-unit upper bound is held before every source attempt. All user,
/// key and ancestor allocations are committed atomically, like token holds.
pub fn reserve_request(
    conn: &Connection,
    user_id: i64,
    key_id: i64,
    request_id: i64,
    attempt_id: &str,
    p: &RequestPrice,
) -> Result<i64> {
    if user_id <= 0
        || key_id <= 0
        || p.channel_id <= 0
        || p.micro_per_call < 0
        || attempt_id.is_empty()
        || attempt_id.len() > 256
    {
        return Err(bad("invalid request reservation"));
    }
    let c = tx(conn)?;
    let existing:Option<(i64,i64,i64,i64,Option<i64>,String)>=c.query_row("SELECT id,user_id,key_id,request_id,request_price_id,billing_unit FROM billing_ledger WHERE attempt_id=?1",[attempt_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    if let Some((id, u, k, r, price, unit)) = existing {
        if (u, k, r, price, unit) != (user_id, key_id, request_id, Some(p.id), "request".into()) {
            return Err(bad(
                "attempt id conflicts with existing request reservation",
            ));
        }
        c.commit()?;
        return Ok(id);
    }
    let stored = c
        .query_row(
            "SELECT id,channel_id,model,micro_per_call FROM billing_request_prices WHERE id=?1",
            [p.id],
            request_price_row,
        )
        .optional()?
        .ok_or_else(|| bad("unknown request pricing version"))?;
    if stored != *p {
        return Err(bad("request pricing snapshot mismatch"));
    }
    let allocations = allocation_scopes(&c, user_id, key_id)?;
    for (scope, id) in &allocations {
        c.execute(
            "INSERT OR IGNORE INTO billing_budgets(scope,scope_id) VALUES(?1,?2)",
            params![scope, id],
        )?;
        let b=c.query_row("SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen FROM billing_budgets WHERE scope=?1 AND scope_id=?2",params![scope,id],budget_row)?;
        if b.frozen {
            return Err(bad("billing budget frozen"));
        }
        let reserved = b
            .reserved_micro
            .checked_add(p.micro_per_call)
            .ok_or_else(|| bad("request budget reservation overflow"))?;
        let total = b
            .spent_micro
            .checked_add(reserved)
            .ok_or_else(|| bad("request budget total overflow"))?;
        if b.limit_micro.is_some_and(|n| total > n) {
            return Err(bad("billing budget exhausted"));
        }
        c.execute(
            "UPDATE billing_budgets SET reserved_micro=?3 WHERE scope=?1 AND scope_id=?2",
            params![scope, id, reserved],
        )?;
    }
    let snapshot = serde_json::to_string(p).map_err(|_| bad("request price serialization"))?;
    c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,billing_unit,request_price_id,request_price_snapshot) VALUES(?1,?2,?3,?4,?5,?6,0,0,?7,?8,'request',?9,?10)",params![attempt_id,user_id,key_id,request_id,p.channel_id,p.model,p.micro_per_call,now_ms(),p.id,snapshot])?;
    let id = c.last_insert_rowid();
    let auth = super::identity::context(&c, key_id)?.ok_or_else(|| bad("inactive gateway key"))?;
    c.execute(
        "UPDATE billing_ledger SET auth_snapshot=?2 WHERE id=?1",
        params![
            id,
            serde_json::to_string(&auth).map_err(|_| bad("identity snapshot serialization"))?
        ],
    )?;
    for (scope, scope_id) in allocations {
        c.execute("INSERT INTO billing_allocations(reservation_id,scope,scope_id,reserved_micro) VALUES(?1,?2,?3,?4)",params![id,scope,scope_id,p.micro_per_call])?;
    }
    c.commit()?;
    Ok(id)
}
/// Known completed response: units0/1; canceled/read failure: None, never fake0.
pub fn settle_request(conn: &Connection, id: i64, units: Option<i64>, status: &str) -> Result<()> {
    finish(conn, id, None, None, status, false, units, true)
}

#[cfg(test)]
mod user_diagnostic_tests {
    use super::*;
    fn fixture()->(Connection,i64){let c=crate::db::open_in_memory().unwrap();let user=crate::db::identity::create_user(&c,"diagnostic-budget-user","user",&Default::default()).unwrap().id;create_price(&c,&Price{id:0,channel_id:1,model:"synthetic-vector".into(),input_micro_per_million:1_000_000,output_micro_per_million:0,max_input_tokens:1000,max_output_tokens:0},"test").unwrap();(c,user)}
    fn reserve(c:&Connection,user:i64,attempt:&str,input:i64)->Result<i64>{reserve_user_diagnostic(c,user,attempt,1,"synthetic-vector",input,&serde_json::json!({"source_id":1,"compatibility_fingerprint":"test"}))}
    fn budget(c:&Connection,user:i64)->Budget {budgets(c,Some("user"),Some(user)).unwrap().remove(0)}
    #[test] fn user_diagnostic_budget_is_atomic_user_only_and_freeze_blocks_send(){
        let(c,user)=fixture();set_budget(&c,"user",user,Some(50),"test").unwrap();let id=reserve(&c,user,"one",40).unwrap();assert!(reserve(&c,user,"two",11).is_err());assert_eq!(budget(&c,user).reserved_micro,40);
        let keys:i64=c.query_row("SELECT COUNT(*) FROM billing_allocations WHERE scope='key'",[],|r|r.get(0)).unwrap();assert_eq!(keys,0);
        settle(&c,id,Some(7),Some(0),"compatibility_response").unwrap();assert_eq!(budget(&c,user).spent_micro,7);assert_eq!(budget(&c,user).reserved_micro,0);
        c.execute("UPDATE billing_budgets SET frozen=1 WHERE scope='user' AND scope_id=?1",[user]).unwrap();assert!(reserve(&c,user,"frozen",1).is_err());
        let requests:i64=c.query_row("SELECT COUNT(*) FROM requests",[],|r|r.get(0)).unwrap();assert_eq!(requests,0);
    }
    #[test] fn user_diagnostic_unknown_usage_retains_hold_and_restart_recovers(){
        let(c,user)=fixture();set_budget(&c,"user",user,Some(100),"test").unwrap();let id=reserve(&c,user,"unknown",30).unwrap();recover(&c).unwrap();assert_eq!(budget(&c,user).spent_micro,30);assert_eq!(budget(&c,user).reserved_micro,0);
        let(row_user,row_key,certainty,kind):(i64,i64,String,String)=c.query_row("SELECT user_id,key_id,usage_certainty,json_extract(auth_snapshot,'$.kind') FROM billing_ledger WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();assert_eq!((row_user,row_key),(user,0));assert_eq!(certainty,"estimated");assert_eq!(kind,"user_compatibility");
    }
    #[test] fn user_diagnostic_unpriced_cannot_bypass_capped_budget(){let(c,user)=fixture();set_budget(&c,"user",user,Some(100),"test").unwrap();assert!(reserve_user_diagnostic(&c,user,"unpriced",1,"unpriced-vector",10,&serde_json::json!({"source_id":1})).is_err());assert_eq!(budget(&c,user).spent_micro,0);assert_eq!(budget(&c,user).reserved_micro,0);}
}
