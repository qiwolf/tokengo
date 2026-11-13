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
pub(super) fn budget_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Budget> {
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
    let mut sql="SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen FROM billing_budgets WHERE 1=1".to_string();
    let mut args=Vec::<rusqlite::types::Value>::new();
    if let Some(scope)=scope {sql.push_str(" AND scope=?");args.push(scope.to_string().into());}
    if let Some(id)=id {sql.push_str(" AND scope_id=?");args.push(id.into());}
    sql.push_str(" ORDER BY scope,scope_id LIMIT 1000");
    let mut s=conn.prepare(&sql)?;
    let out = s
        .query_map(rusqlite::params_from_iter(args), budget_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(out)
}
/// Read only this owner's Key budgets, without the generic administration list cap.
/// Key-list responses must not turn every row into another HTTP round trip.
pub fn key_budgets_for_user(conn: &Connection, user_id: i64) -> Result<Vec<Budget>> {
    let mut query = conn.prepare(
        "SELECT b.scope,b.scope_id,b.limit_micro,b.spent_micro,b.reserved_micro,b.frozen
         FROM gateway_keys g JOIN billing_budgets b ON b.scope='key' AND b.scope_id=g.id
         WHERE g.user_id=?1 ORDER BY g.id",
    )?;
    let rows = query.query_map([user_id], budget_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn set_budget(
    conn: &Connection,
    scope: &str,
    id: i64,
    limit: Option<i64>,
    actor: &str,
) -> Result<()> {
    scope_ok(scope, id)?;
    super::billing_management::require_active_object(conn, scope, id)?;
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
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UsageQuery {
    pub user_id: Option<i64>,
    pub key_id: Option<i64>,
    pub channel_id: Option<i64>,
    pub model: Option<String>,
    pub before_id: Option<i64>,
    pub limit: Option<u32>,
    pub state: Option<String>,
    /// Unix seconds; creation time is filtered as [start_at, end_at).
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
}
/// Validate before fast empty-result paths as well as before SQL execution.
pub fn validate_usage_time(q:&UsageQuery)->Result<()> {
    if [q.start_at,q.end_at].into_iter().flatten().any(|v|v<0||v.checked_mul(1000).is_none()) {
        return Err(bad("时间必须为有效的 Unix 秒时间戳"));
    }
    if matches!((q.start_at,q.end_at),(Some(start),Some(end)) if start>=end) {
        return Err(bad("开始时间必须早于结束时间"));
    }
    Ok(())
}
fn usage_predicates(q:&UsageQuery,state:&str,catalog:Option<&serde_json::Value>,include_cursor:bool)->Result<(String,Vec<rusqlite::types::Value>)> {
    use rusqlite::types::Value;
    validate_usage_time(q)?;
    let mut sql=String::new();
    let mut args=Vec::<Value>::new();
    for (column,value) in [("user_id",q.user_id),("key_id",q.key_id),("channel_id",q.channel_id)] {
        if let Some(value)=value {sql.push_str(&format!(" AND {column}=?"));args.push(value.into());}
    }
    if let Some(model)=&q.model {sql.push_str(" AND model=?");args.push(tokengo_shared::normalize_model_name(model).into());}
    if let Some(start)=q.start_at {sql.push_str(" AND created_ms>=?");args.push((start*1000).into());}
    if let Some(end)=q.end_at {sql.push_str(" AND created_ms<?");args.push((end*1000).into());}
    if include_cursor {if let Some(id)=q.before_id {sql.push_str(" AND id<?");args.push(id.into());}}
    if state!="all" {
        let catalog=catalog.ok_or_else(||bad("missing billing object catalog"))?;
        sql.push_str(" AND ((user_id IN (SELECT value FROM json_each(?)) AND key_id IN (SELECT value FROM json_each(?)) AND channel_id IN (SELECT value FROM json_each(?))) = ?)");
        for kind in ["users","keys","channels"] {
            let ids=catalog[kind].as_array().ok_or_else(||bad("invalid billing object catalog"))?.iter().filter(|v|v["state"]=="active").filter_map(|v|v["id"].as_i64()).collect::<Vec<_>>();
            args.push(serde_json::to_string(&ids).map_err(|_|bad("invalid billing object ids"))?.into());
        }
        args.push(i64::from(state=="active").into());
    }
    Ok((sql,args))
}
#[derive(Debug, Clone, Serialize)]
pub struct UsageSummary {
    pub request_count:i64,
    pub ledger_count:i64,
    pub input_tokens:i64,
    pub output_tokens:i64,
    pub total_tokens:i64,
    pub unknown_input_entries:i64,
    pub unknown_output_entries:i64,
    pub unpriced_entries:i64,
    pub known_debit_micro:i64,
    pub unknown_debit_entries:i64,
}
/// Full matching aggregate: paging cursors and limits never narrow the summary.
pub fn usage_summary_with_catalog(conn:&Connection,q:&UsageQuery,catalog:Option<&serde_json::Value>)->Result<UsageSummary> {
    validate_usage_time(q)?;
    let state=super::billing_management::validated_state(q.state.as_deref(),"all")?;
    let owned;
    let catalog=if state!="all"&&catalog.is_none() {owned=super::billing_management::current_objects(conn)?;Some(&owned)}else{catalog};
    let (predicate,args)=usage_predicates(q,state,catalog,false)?;
    // Separate grouped request count preserves the covering owner index plan
    // for lifetime queries rather than creating a COUNT(DISTINCT) temp tree.
    let table=usage_summary_table(q);
    let sql=format!("SELECT (SELECT count(*) FROM (SELECT request_id FROM {table} WHERE request_id IS NOT NULL{predicate} GROUP BY request_id)),count(*),coalesce(sum(input_tokens),0),coalesce(sum(output_tokens),0),coalesce(sum(input_tokens IS NULL),0),coalesce(sum(output_tokens IS NULL),0),coalesce(sum(price_id IS NULL AND request_price_id IS NULL),0),coalesce(sum(debit_micro),0),coalesce(sum(debit_micro IS NULL),0) FROM {table} WHERE 1=1{predicate}");
    let bindings=args.iter().chain(args.iter());
    Ok(conn.query_row(&sql,rusqlite::params_from_iter(bindings),|r| {
        let input_tokens:i64=r.get(2)?;
        let output_tokens:i64=r.get(3)?;
        Ok(UsageSummary {request_count:r.get(0)?,ledger_count:r.get(1)?,input_tokens,output_tokens,total_tokens:input_tokens.saturating_add(output_tokens),unknown_input_entries:r.get(4)?,unknown_output_entries:r.get(5)?,unpriced_entries:r.get(6)?,known_debit_micro:r.get(7)?,unknown_debit_entries:r.get(8)?})
    })?)
}
pub fn usage_summary(conn:&Connection,q:&UsageQuery)->Result<UsageSummary> {
    usage_summary_with_catalog(conn,q,None)
}
fn usage_summary_table(q:&UsageQuery)->&'static str {
    if q.start_at.is_some()||q.end_at.is_some() {
        if q.user_id.is_some() {"billing_ledger INDEXED BY billing_ledger_owner_time_summary"}
        else {"billing_ledger INDEXED BY billing_ledger_time_summary"}
    } else {"billing_ledger"}
}
/// Compact time buckets and model totals use the same authorization/filter
/// predicates as the full-range summary. Unknown usage is kept alongside sums.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageBucket {
    pub start_at:i64,
    pub request_count:i64,
    pub ledger_count:i64,
    pub input_tokens:i64,
    pub output_tokens:i64,
    pub total_tokens:i64,
    pub known_debit_micro:i64,
    pub unknown_input_entries:i64,
    pub unknown_output_entries:i64,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageModel {
    pub model:String,
    pub is_other:bool,
    pub request_count:i64,
    pub ledger_count:i64,
    pub input_tokens:i64,
    pub output_tokens:i64,
    pub total_tokens:i64,
    pub known_debit_micro:i64,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageModelBucket {
    pub start_at:i64,
    pub ledger_count:i64,
    pub request_count:i64,
    pub total_tokens:i64,
    pub known_debit_micro:i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct UsageModelSeries {
    pub model:String,
    pub is_other:bool,
    pub ledger_count:i64,
    /// Per-model request attribution; retries can span buckets/models.
    pub request_count:i64,
    pub buckets:Vec<UsageModelBucket>,
}
#[derive(Debug, Clone, Serialize)]
pub struct UsageAnalytics {
    pub start_at:i64,
    pub end_at:i64,
    pub effective_end_at:i64,
    pub bucket_seconds:i64,
    pub buckets:Vec<UsageBucket>,
    pub models:Vec<UsageModel>,
    pub model_series:Vec<UsageModelSeries>,
    pub average_rpm:f64,
    pub average_tpm:f64,
}
fn usage_bucket_seconds(span:i64)->i64 {
    // At most 120 buckets even for an unbounded retained history. Calendar
    // presets arrive with browser-local start boundaries; bins are elapsed time.
    for seconds in [3600,21600,86400,604800,2592000] {
        if span.div_euclid(seconds)+(i64::from(span.rem_euclid(seconds)>0))<=120 {
            if seconds==3600&&span>48*3600 {continue;}
            if seconds==21600&&span>14*86400 {continue;}
            return seconds;
        }
    }
    (span/120+i64::from(span%120>0)).max(2592000)
}
pub fn usage_analytics_with_catalog(conn:&Connection,q:&UsageQuery,catalog:Option<&serde_json::Value>,summary:&UsageSummary)->Result<UsageAnalytics> {
    usage_analytics_at(conn,q,catalog,summary,now_ms()/1000)
}
pub fn usage_analytics(conn:&Connection,q:&UsageQuery,summary:&UsageSummary)->Result<UsageAnalytics> {
    usage_analytics_with_catalog(conn,q,None,summary)
}
fn usage_analytics_at(conn:&Connection,q:&UsageQuery,catalog:Option<&serde_json::Value>,summary:&UsageSummary,now:i64)->Result<UsageAnalytics> {
    validate_usage_time(q)?;
    let state=super::billing_management::validated_state(q.state.as_deref(),"all")?;
    let owned;
    let catalog=if state!="all"&&catalog.is_none() {owned=super::billing_management::current_objects(conn)?;Some(&owned)}else{catalog};
    let (predicate,args)=usage_predicates(q,state,catalog,false)?;
    let table=usage_summary_table(q);
    let (first,last)=if q.start_at.is_none()||q.end_at.is_none() {
        conn.query_row(&format!("SELECT min(created_ms),max(created_ms) FROM {table} WHERE 1=1{predicate}"),rusqlite::params_from_iter(args.iter()),|r|Ok((r.get::<_,Option<i64>>(0)?,r.get::<_,Option<i64>>(1)?)))?
    } else {(None,None)};
    let start=q.start_at.or_else(||first.map(|ms|ms.div_euclid(1000))).unwrap_or_else(||q.end_at.unwrap_or(now).min(now));
    let end=q.end_at.unwrap_or_else(||last.map(|ms|ms.div_euclid(1000).saturating_add(1)).unwrap_or(now).max(now).max(start));
    let span=end.saturating_sub(start).max(0);
    let bucket_seconds=usage_bucket_seconds(span);
    let effective_end_at=end.min(now);
    let elapsed=effective_end_at.saturating_sub(start);
    let mut analytics=UsageAnalytics {start_at:start,end_at:end,effective_end_at,bucket_seconds,buckets:Vec::new(),models:Vec::new(),model_series:Vec::new(),average_rpm:if elapsed>0 {summary.request_count as f64*60.0/elapsed as f64}else{0.0},average_tpm:if elapsed>0 {summary.total_tokens as f64*60.0/elapsed as f64}else{0.0}};
    if span==0 {return Ok(analytics);}
    let count=span/bucket_seconds+i64::from(span%bucket_seconds>0);
    analytics.buckets=(0..count).map(|i|UsageBucket{start_at:start+i*bucket_seconds,..Default::default()}).collect();
    let mut bindings=vec![rusqlite::types::Value::Integer(start*1000),rusqlite::types::Value::Integer(bucket_seconds*1000)];
    bindings.extend(args.iter().cloned());
    let sql=format!("SELECT (created_ms-?)/? AS bucket,count(DISTINCT request_id),count(*),coalesce(sum(input_tokens),0),coalesce(sum(output_tokens),0),coalesce(sum(debit_micro),0),coalesce(sum(input_tokens IS NULL),0),coalesce(sum(output_tokens IS NULL),0) FROM {table} WHERE 1=1{predicate} GROUP BY bucket ORDER BY bucket");
    let mut statement=conn.prepare(&sql)?;
    let buckets=statement.query_map(rusqlite::params_from_iter(bindings),|r|{
        let input_tokens:i64=r.get(3)?;let output_tokens:i64=r.get(4)?;
        Ok((r.get::<_,i64>(0)?,UsageBucket{start_at:0,request_count:r.get(1)?,ledger_count:r.get(2)?,input_tokens,output_tokens,total_tokens:input_tokens.saturating_add(output_tokens),known_debit_micro:r.get(5)?,unknown_input_entries:r.get(6)?,unknown_output_entries:r.get(7)?}))
    })?;
    for row in buckets {let(index,mut bucket)=row?;if let Some(slot)=analytics.buckets.get_mut(index as usize) {bucket.start_at=slot.start_at;*slot=bucket;}}
    let sql=format!("SELECT model,count(DISTINCT request_id),count(*),coalesce(sum(input_tokens),0),coalesce(sum(output_tokens),0),coalesce(sum(debit_micro),0) FROM {table} WHERE 1=1{predicate} GROUP BY model ORDER BY coalesce(sum(input_tokens),0)+coalesce(sum(output_tokens),0) DESC,count(*) DESC,model");
    let mut statement=conn.prepare(&sql)?;
    let models=statement.query_map(rusqlite::params_from_iter(args.iter()),|r|{
        let input_tokens:i64=r.get(3)?;let output_tokens:i64=r.get(4)?;
        Ok(UsageModel{model:r.get(0)?,is_other:false,request_count:r.get(1)?,ledger_count:r.get(2)?,input_tokens,output_tokens,total_tokens:input_tokens.saturating_add(output_tokens),known_debit_micro:r.get(5)?})
    })?;
    let mut other=UsageModel{model:"其他模型".into(),is_other:true,..Default::default()};
    let mut call_leaders=Vec::<UsageModel>::new();let mut all_model_requests=0i64;
    for model in models {
        let model=model?;all_model_requests=all_model_requests.saturating_add(model.request_count);
        // Bounded top calls selection independent of token rankings.
        call_leaders.push(model.clone());
        call_leaders.sort_by(|a,b|b.ledger_count.cmp(&a.ledger_count).then_with(||a.model.cmp(&b.model)));
        call_leaders.truncate(8);
        if analytics.models.len()<8 {analytics.models.push(model);}else{
            other.request_count=other.request_count.saturating_add(model.request_count);other.ledger_count=other.ledger_count.saturating_add(model.ledger_count);other.input_tokens=other.input_tokens.saturating_add(model.input_tokens);other.output_tokens=other.output_tokens.saturating_add(model.output_tokens);other.total_tokens=other.total_tokens.saturating_add(model.total_tokens);other.known_debit_micro=other.known_debit_micro.saturating_add(model.known_debit_micro);
        }
    }
    if other.ledger_count>0 {analytics.models.push(other);}
    let leading_calls=call_leaders.iter().map(|m|m.ledger_count).sum::<i64>();
    let leading_requests=call_leaders.iter().map(|m|m.request_count).sum::<i64>();
    let blank_buckets=analytics.buckets.iter().map(|b|UsageModelBucket{start_at:b.start_at,..Default::default()}).collect::<Vec<_>>();
    analytics.model_series=call_leaders.iter().map(|m|UsageModelSeries{model:m.model.clone(),is_other:false,ledger_count:m.ledger_count,request_count:m.request_count,buckets:blank_buckets.clone()}).collect();
    if summary.ledger_count>leading_calls {
        analytics.model_series.push(UsageModelSeries{model:"其他模型".into(),is_other:true,ledger_count:summary.ledger_count-leading_calls,request_count:all_model_requests-leading_requests,buckets:blank_buckets.clone()});
    }
    if !analytics.model_series.is_empty() {
        // Numeric series indices avoid collisions with a real model named
        // "其他模型"; only the globally selected eight keep individual lines.
        let mut case="CASE model".to_string();let mut bindings=Vec::<rusqlite::types::Value>::new();
        for (index,model) in call_leaders.iter().enumerate() {
            case.push_str(&format!(" WHEN ? THEN {index}"));bindings.push(model.model.clone().into());
        }
        case.push_str(&format!(" ELSE {} END",call_leaders.len()));
        bindings.push((start*1000).into());bindings.push((bucket_seconds*1000).into());bindings.extend(args.iter().cloned());
        let sql=format!("SELECT {case} AS series,(created_ms-?)/? AS bucket,count(DISTINCT request_id),count(*),coalesce(sum(input_tokens),0)+coalesce(sum(output_tokens),0),coalesce(sum(debit_micro),0) FROM {table} WHERE 1=1{predicate} GROUP BY series,bucket ORDER BY series,bucket");
        let mut statement=conn.prepare(&sql)?;
        let grouped=statement.query_map(rusqlite::params_from_iter(bindings),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,UsageModelBucket{start_at:0,request_count:r.get(2)?,ledger_count:r.get(3)?,total_tokens:r.get(4)?,known_debit_micro:r.get(5)?})))?;
        for row in grouped {
            let(series,index,mut bucket)=row?;
            let (Ok(series),Ok(index))=(usize::try_from(series),usize::try_from(index)) else {continue;};
            if let Some(slot)=analytics.model_series.get_mut(series).and_then(|s|s.buckets.get_mut(index)) {bucket.start_at=slot.start_at;*slot=bucket;}
        }
    }
    Ok(analytics)
}
/// Build bound predicates only for supplied filters, so owner/source/model indexes
/// remain usable. Catalogs are request-local observations, never authorization caches.
fn usage_sql(q:&UsageQuery,state:&str,catalog:Option<&serde_json::Value>,limit:u32)->Result<(String,Vec<rusqlite::types::Value>)> {
    // A sparse lifecycle/object/model filter can reject large snapshots. Select
    // IDs from a covering index first; the IN subquery's LIMIT bounds wide reads.
    let dated=q.start_at.is_some()||q.end_at.is_some();
    let lifecycle_page=dated||state!="all"||q.user_id.is_some()||q.key_id.is_some()||q.channel_id.is_some()||q.model.is_some();
    let lifecycle_index=if dated {if q.user_id.is_some(){"billing_ledger_owner_time_summary"}else{"billing_ledger_time_summary"}}
        else if q.key_id.is_some() {"billing_ledger_key_lifecycle"}
        else if q.user_id.is_some() {"billing_ledger_user_lifecycle"}
        else if q.channel_id.is_some() {"billing_ledger_channel"}
        else if q.model.is_some() {"billing_ledger_model"}
        else {"billing_ledger_lifecycle_recent"};
    let mut sql=if lifecycle_page {
        format!("SELECT id FROM billing_ledger INDEXED BY {lifecycle_index} WHERE 1=1")
    }else{format!("{LEDGER_SELECT} WHERE 1=1")};
    let (predicate,mut args)=usage_predicates(q,state,catalog,true)?;
    sql.push_str(&predicate);
    sql.push_str(" ORDER BY id DESC LIMIT ?");args.push(i64::from(limit).into());
    if lifecycle_page {sql=format!("{LEDGER_SELECT} WHERE id IN ({sql}) ORDER BY id DESC");}
    Ok((sql,args))
}
fn usage_rows(conn:&Connection,q:&UsageQuery,catalog:Option<&serde_json::Value>,limit:u32)->Result<Vec<Ledger>> {
    validate_usage_time(q)?;
    let state=super::billing_management::validated_state(q.state.as_deref(),"all")?;
    let owned;
    let catalog=if state!="all"&&catalog.is_none() {owned=super::billing_management::current_objects(conn)?;Some(&owned)}else{catalog};
    if state=="active"&&catalog.is_some_and(|catalog|["users","keys","channels"].iter().any(|kind|catalog[*kind].as_array().is_some_and(|objects|!objects.iter().any(|object|object["state"]=="active")))) {
        return Ok(Vec::new());
    }
    let (sql,args)=usage_sql(q,state,catalog,limit)?;
    let mut s=conn.prepare(&sql)?;
    let rows=s.query_map(rusqlite::params_from_iter(args),ledger_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn usage(conn:&Connection,q:&UsageQuery)->Result<Vec<Ledger>> {
    usage_rows(conn,q,None,q.limit.unwrap_or(50).clamp(1,200))
}
/// One bounded read determines both entries and a real continuation cursor.
pub fn usage_page_with_catalog(conn:&Connection,q:&UsageQuery,catalog:Option<&serde_json::Value>)->Result<(Vec<Ledger>,Option<i64>)> {
    let limit=q.limit.unwrap_or(50).clamp(1,200) as usize;
    let mut rows=usage_rows(conn,q,catalog,(limit+1) as u32)?;
    let next=if rows.len()>limit {rows.truncate(limit);rows.last().map(|r|r.id)}else{None};
    Ok((rows,next))
}
pub fn usage_page(conn:&Connection,q:&UsageQuery)->Result<(Vec<Ledger>,Option<i64>)> {
    usage_page_with_catalog(conn,q,None)
}

/// Offer a cursor only when another row exists under the exact same filters.
pub fn usage_next_before(conn: &Connection, q: &UsageQuery, entries: &[Ledger]) -> Result<Option<i64>> {
    let Some(last) = entries.last() else { return Ok(None); };
    let mut next_query = q.clone();
    next_query.before_id = Some(last.id);
    next_query.limit = Some(1);
    Ok(if usage(conn, &next_query)?.is_empty() { None } else { Some(last.id) })
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
mod query_performance_tests {
    use super::*;
    fn plan(c:&Connection,sql:&str,args:Vec<rusqlite::types::Value>)->String {
        c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap()
            .query_map(rusqlite::params_from_iter(args),|r|r.get::<_,String>(3)).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap().join("\n")
    }
    #[test]
    fn selective_usage_and_money_queries_use_existing_or_covering_indexes() {
        let c=crate::db::open_in_memory().unwrap();
        for (q,index) in [
            (UsageQuery{user_id:Some(2),..Default::default()},"billing_ledger_user_lifecycle"),
            (UsageQuery{key_id:Some(3),..Default::default()},"billing_ledger_key_lifecycle"),
            (UsageQuery{channel_id:Some(4),..Default::default()},"billing_ledger_channel"),
            (UsageQuery{model:Some("synthetic".into()),..Default::default()},"billing_ledger_model"),
        ] {
            let(sql,args)=usage_sql(&q,"all",None,51).unwrap();
            let p=plan(&c,&sql,args);
            assert!(p.contains(&format!("COVERING INDEX {index}")),"{p}");
            assert!(p.contains("SEARCH billing_ledger USING INTEGER PRIMARY KEY"),"{p}");
        }
        let(sql,args)=usage_sql(&UsageQuery{before_id:Some(123),..Default::default()},"all",None,51).unwrap();
        assert!(plan(&c,&sql,args).contains("SEARCH billing_ledger USING INTEGER PRIMARY KEY"));
        for (sql,index) in [
            ("SELECT id FROM billing_ledger WHERE attempt_id=?1","sqlite_autoindex_billing_ledger"),
            ("SELECT scope FROM billing_allocations WHERE reservation_id=?1","sqlite_autoindex_billing_allocations"),
            ("SELECT id FROM billing_ledger WHERE status=?1 ORDER BY id","billing_ledger_pending"),
        ] {
            let p=plan(&c,sql,vec![1.into()]);assert!(p.contains(index),"{p}");
        }
        let p=plan(&c,"SELECT id FROM billing_request_prices WHERE channel_id=?1 AND model=?2 ORDER BY id DESC LIMIT 1",vec![1.into(),"synthetic".to_string().into()]);
        assert!(p.contains("billing_request_prices_latest"),"{p}");
    }
    #[test]
    fn sparse_lifecycle_queries_read_covering_ids_before_bounded_wide_rows() {
        let c=crate::db::open_in_memory().unwrap();
        let catalog=serde_json::json!({"users":[{"id":1,"state":"active"}],"keys":[{"id":1,"state":"active"}],"channels":[{"id":1,"state":"active"}]});
        for state in ["active","historical"] {
            for before_id in [None,Some(5000)] {
                let q=UsageQuery{state:Some(state.into()),before_id,..Default::default()};
                let(sql,args)=usage_sql(&q,state,Some(&catalog),51).unwrap();
                let p=plan(&c,&sql,args);
                assert!(p.contains("COVERING INDEX billing_ledger_lifecycle_recent"),"{p}");
                assert!(p.contains("SEARCH billing_ledger USING INTEGER PRIMARY KEY"),"{p}");
                assert!(!p.lines().any(|line|line=="SCAN billing_ledger"),"{p}");
            }
        }
    }
    #[test]
    fn all_lifecycle_filter_combinations_cover_sparse_and_empty_matches() {
        let c=crate::db::open_in_memory().unwrap();
        c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,source_snapshot) VALUES('old-active',1,1,1,1,'synthetic',0,0,0,0,'ok','{}')",[]).unwrap();
        // Every newer row differs from the old valid triple in at least one
        // lifecycle ID. Unspecified filters therefore expose sparse matches.
        c.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<7000) INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,source_snapshot) SELECT 'combo-'||i,1+((i%7+1)&1),1+(((i%7+1)>>1)&1),i,1+(((i%7+1)>>2)&1),'synthetic',0,0,0,i,'ok',json_object('padding',printf('%4096s','')) FROM n",[]).unwrap();
        let metadata=c.prepare("SELECT id,user_id,key_id,channel_id FROM billing_ledger ORDER BY id DESC").unwrap().query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        let mut max_steps=0;
        for mask in 0..16 {
            for live_channel in [1,3] {
                let catalog=serde_json::json!({"users":[{"id":1,"state":"active"}],"keys":[{"id":1,"state":"active"}],"channels":[{"id":live_channel,"state":"active"}]});
                for state in ["active","historical","all"] {
                    for before_id in [None,Some(3501)] {
                        let q=UsageQuery{user_id:(mask&1!=0).then_some(1),key_id:(mask&2!=0).then_some(1),channel_id:(mask&4!=0).then_some(1),model:(mask&8!=0).then(||"SYNTHETIC".into()),state:Some(state.into()),before_id,limit:Some(2),..Default::default()};
                        let index=if q.key_id.is_some(){"billing_ledger_key_lifecycle"}else if q.user_id.is_some(){"billing_ledger_user_lifecycle"}else if q.channel_id.is_some(){"billing_ledger_channel"}else if q.model.is_some(){"billing_ledger_model"}else{"billing_ledger_lifecycle_recent"};
                        let(sql,args)=usage_sql(&q,state,Some(&catalog),3).unwrap();
                        let p=plan(&c,&sql,args.clone());
                        if state!="all"||mask!=0 {
                            assert!(p.contains(&format!("COVERING INDEX {index}")),"mask={mask} state={state} {p}");
                            assert!(p.contains("SEARCH billing_ledger USING INTEGER PRIMARY KEY"),"{p}");
                            assert!(!p.lines().any(|line|line=="SCAN billing_ledger"),"{p}");
                        } // all without filters remains a direct, LIMIT-bounded rowid scan.
                        let mut statement=c.prepare(&sql).unwrap();
                        let actual=statement.query_map(rusqlite::params_from_iter(args),|r|r.get::<_,i64>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
                        let steps=statement.get_status(rusqlite::StatementStatus::VmStep);
                        max_steps=max_steps.max(steps);
                        assert!(steps<7001*150+10000,"mask={mask} state={state} steps={steps}");
                        let expected=metadata.iter().filter(|&&(id,user,key,channel)| {
                            q.user_id.is_none_or(|v|v==user)&&q.key_id.is_none_or(|v|v==key)&&q.channel_id.is_none_or(|v|v==channel)&&q.before_id.is_none_or(|v|id<v)&&(state=="all"||((user==1&&key==1&&channel==live_channel)==(state=="active")))
                        }).map(|row|row.0).take(3).collect::<Vec<_>>();
                        assert_eq!(actual,expected,"mask={mask} state={state} live_channel={live_channel}");
                        let(rows,next)=usage_page_with_catalog(&c,&q,Some(&catalog)).unwrap();
                        assert_eq!(rows.iter().map(|row|row.id).collect::<Vec<_>>(),expected.iter().copied().take(2).collect::<Vec<_>>());
                        assert_eq!(next,if expected.len()>2{Some(expected[1])}else{None});
                    }
                }
            }
        }
        // Synthetic setup for a model appearing only in the oldest wide row.
        c.execute("UPDATE billing_ledger SET model='needle model' WHERE id=1",[]).unwrap();
        for (q,expected) in [
            (UsageQuery{state:Some("all".into()),user_id:Some(1),model:Some("Needle-Model".into()),..Default::default()},vec![1]),
            (UsageQuery{state:Some("all".into()),user_id:Some(2),model:Some("Needle-Model".into()),..Default::default()},vec![]),
            (UsageQuery{state:Some("all".into()),user_id:Some(1),model:Some("missing model".into()),..Default::default()},vec![]),
            (UsageQuery{state:Some("all".into()),model:Some("missing model".into()),..Default::default()},vec![]),
        ] {
            let(sql,args)=usage_sql(&q,"all",None,51).unwrap();
            let p=plan(&c,&sql,args.clone());
            let index=if q.user_id.is_some(){"billing_ledger_user_lifecycle"}else{"billing_ledger_model"};
            assert!(p.contains(&format!("COVERING INDEX {index}")),"{p}");
            assert!(p.contains("SEARCH billing_ledger USING INTEGER PRIMARY KEY"),"{p}");
            let mut statement=c.prepare(&sql).unwrap();
            let actual=statement.query_map(rusqlite::params_from_iter(args),|r|r.get::<_,i64>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            let steps=statement.get_status(rusqlite::StatementStatus::VmStep);
            assert!(steps<7001*150+10000,"all sparse user/model steps={steps}");
            assert_eq!(actual,expected);
            let(rows,next)=usage_page(&c,&q).unwrap();
            assert_eq!(rows.iter().map(|row|row.id).collect::<Vec<_>>(),expected);assert_eq!(next,None);
        }
        eprintln!("7001 wide ledger rows, 192 lifecycle filter/state/cursor combinations plus 4 sparse all user/model cases: max VM steps={max_steps}");
    }
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

#[cfg(test)]
mod usage_time_tests {
    use super::*;
    fn fixture()->Connection {
        let c=crate::db::open_in_memory().unwrap();
        // Millisecond edges, retries, foreign owner/key, historical object, and
        // unknown usage all matter independently of page size and row ID order.
        for (attempt,user,key,request,channel,created,input,output) in [
            ("before",1,1,1,1,9999,Some(900),Some(900)),
            ("start",1,1,2,1,10000,Some(10),Some(2)),
            ("retry",1,1,2,1,19999,None,Some(3)),
            ("end",1,1,3,1,20000,Some(800),Some(800)),
            ("foreign",2,2,4,1,15000,Some(700),Some(700)),
            ("other-key",1,3,5,1,15000,Some(20),Some(4)),
            ("historical",1,1,6,99,15000,Some(30),Some(5)),
        ] {
            c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,input_tokens,output_tokens,debit_micro) VALUES(?1,?2,?3,?4,?5,'synthetic',0,0,0,?6,'ok',?7,?8,?9)",params![attempt,user,key,request,channel,created,input,output,input.map(|_|7)]).unwrap();
        }
        c
    }
    #[test]
    fn time_range_summary_matches_all_rows_not_cursor_and_respects_every_filter() {
        let c=fixture();
        let catalog=serde_json::json!({"users":[{"id":1,"state":"active"}],"keys":[{"id":1,"state":"active"},{"id":3,"state":"active"}],"channels":[{"id":1,"state":"active"}]});
        let mut q=UsageQuery{user_id:Some(1),start_at:Some(10),end_at:Some(20),limit:Some(1),..Default::default()};
        let (rows,next)=usage_page(&c,&q).unwrap();assert_eq!(rows.len(),1);assert!(next.is_some());
        let summary=usage_summary(&c,&q).unwrap();
        assert_eq!((summary.request_count,summary.ledger_count,summary.input_tokens,summary.output_tokens,summary.total_tokens),(3,4,60,14,74));
        assert_eq!((summary.unknown_input_entries,summary.unknown_output_entries,summary.known_debit_micro,summary.unknown_debit_entries),(1,0,21,1));
        q.before_id=next;let second=usage_summary(&c,&q).unwrap();assert_eq!(second.total_tokens,summary.total_tokens);assert_eq!(second.ledger_count,4);
        q.key_id=Some(1);q.channel_id=Some(1);q.model=Some("SYNTHETIC".into());
        let summary=usage_summary(&c,&q).unwrap();assert_eq!((summary.request_count,summary.ledger_count,summary.total_tokens),(1,2,15));
        q.key_id=None;q.channel_id=None;q.state=Some("active".into());
        let summary=usage_summary_with_catalog(&c,&q,Some(&catalog)).unwrap();assert_eq!((summary.ledger_count,summary.total_tokens),(3,39));
        q.state=Some("historical".into());
        let summary=usage_summary_with_catalog(&c,&q,Some(&catalog)).unwrap();assert_eq!((summary.ledger_count,summary.total_tokens),(1,35));
        q.start_at=Some(30);q.end_at=Some(40);
        let summary=usage_summary_with_catalog(&c,&q,Some(&catalog)).unwrap();assert_eq!((summary.ledger_count,summary.total_tokens,summary.unknown_input_entries),(0,0,0));
    }
    #[test]
    fn dated_lookups_select_covering_time_index_before_reading_snapshots() {
        let c=fixture();
        for owner in [None,Some(1)] {
            let q=UsageQuery{user_id:owner,start_at:Some(10),end_at:Some(20),..Default::default()};
            let (sql,args)=usage_sql(&q,"all",None,2).unwrap();
            let plan=c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap().query_map(rusqlite::params_from_iter(args),|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap().join("\n");
            let index=if owner.is_some(){"billing_ledger_owner_time_summary"}else{"billing_ledger_time_summary"};
            assert!(plan.contains(&format!("COVERING INDEX {index}")),"{plan}");
            assert!(plan.contains("created_ms>? AND created_ms<?"),"{plan}");
        }
    }
    #[test]
    fn analytics_zero_fills_time_buckets_and_ignores_cursor_with_identical_scope() {
        let c=fixture();
        // Same owner, a second hour and another model; the end boundary must
        // exclude its row even though its ID is newer than all included rows.
        c.execute("UPDATE billing_ledger SET created_ms=CASE attempt_id WHEN 'start' THEN 3600000 WHEN 'retry' THEN 7199999 WHEN 'other-key' THEN 10800000 WHEN 'historical' THEN 10800000 WHEN 'foreign' THEN 10800000 ELSE created_ms END",[]).unwrap();
        c.execute("UPDATE billing_ledger SET model='second-model' WHERE attempt_id='other-key'",[]).unwrap();
        let q=UsageQuery{user_id:Some(1),start_at:Some(3600),end_at:Some(14400),before_id:Some(2),limit:Some(1),..Default::default()};
        let summary=usage_summary(&c,&q).unwrap();
        let analytics=usage_analytics_at(&c,&q,None,&summary,14400).unwrap();
        assert_eq!(analytics.bucket_seconds,3600);assert_eq!(analytics.buckets.len(),3);
        assert_eq!(analytics.buckets.iter().map(|b|b.start_at).collect::<Vec<_>>(),vec![3600,7200,10800]);
        assert_eq!(analytics.buckets.iter().map(|b|b.total_tokens).collect::<Vec<_>>(),vec![15,0,59]);
        assert_eq!(analytics.buckets.iter().map(|b|b.ledger_count).collect::<Vec<_>>(),vec![2,0,2]);
        assert_eq!(analytics.buckets.iter().map(|b|b.total_tokens).sum::<i64>(),summary.total_tokens);
        assert_eq!(analytics.buckets.iter().map(|b|b.known_debit_micro).sum::<i64>(),summary.known_debit_micro);
        assert_eq!(analytics.models.iter().map(|m|m.total_tokens).sum::<i64>(),summary.total_tokens);
        assert_eq!(analytics.buckets[0].unknown_input_entries,1);
        assert!((analytics.average_rpm-3.0/180.0).abs()<1e-12);
        assert!((analytics.average_tpm-74.0/180.0).abs()<1e-12);
        assert!(analytics.models.iter().any(|m|m.model=="second-model"&&m.total_tokens==24));
        assert_eq!(analytics.model_series.len(),2);
        for (i,bucket) in analytics.buckets.iter().enumerate() {
            assert_eq!(analytics.model_series.iter().map(|s|s.buckets[i].ledger_count).sum::<i64>(),bucket.ledger_count);
            assert_eq!(analytics.model_series.iter().map(|s|s.buckets[i].total_tokens).sum::<i64>(),bucket.total_tokens);
            assert_eq!(analytics.model_series.iter().map(|s|s.buckets[i].known_debit_micro).sum::<i64>(),bucket.known_debit_micro);
            assert!(analytics.model_series.iter().all(|s|s.buckets[i].start_at==bucket.start_at));
        }
        assert_eq!(analytics.model_series.iter().map(|s|s.ledger_count).sum::<i64>(),summary.ledger_count);
        let mut first=q.clone();first.before_id=None;first.limit=Some(200);
        let same=usage_analytics_at(&c,&first,None,&summary,14400).unwrap();
        assert_eq!(serde_json::to_value(analytics).unwrap(),serde_json::to_value(same).unwrap());
        let catalog=serde_json::json!({"users":[{"id":1,"state":"active"}],"keys":[{"id":1,"state":"active"},{"id":3,"state":"active"}],"channels":[{"id":1,"state":"active"}]});
        first.state=Some("active".into());first.key_id=Some(1);first.model=Some("SYNTHETIC".into());
        let summary=usage_summary_with_catalog(&c,&first,Some(&catalog)).unwrap();
        let analytics=usage_analytics_at(&c,&first,Some(&catalog),&summary,14400).unwrap();
        assert_eq!(analytics.buckets.iter().map(|b|b.total_tokens).sum::<i64>(),15);
        assert_eq!(analytics.models.len(),1);assert_eq!(analytics.models[0].ledger_count,2);
        assert_eq!(analytics.model_series.len(),1);assert_eq!(analytics.model_series[0].ledger_count,2);
    }
    #[test]
    fn analytics_bounds_empty_future_and_long_ranges_are_finite_and_bounded() {
        let c=fixture();
        let mut q=UsageQuery{user_id:Some(999),start_at:Some(3600),end_at:Some(14400),..Default::default()};
        let summary=usage_summary(&c,&q).unwrap();
        let empty=usage_analytics_at(&c,&q,None,&summary,7200).unwrap();
        assert_eq!(empty.buckets.len(),3);assert!(empty.models.is_empty());assert!(empty.model_series.is_empty());assert_eq!(empty.effective_end_at,7200);
        assert_eq!(empty.average_rpm,0.0);assert_eq!(empty.average_tpm,0.0);
        q.user_id=Some(1);q.start_at=Some(10);q.end_at=Some(20);
        let summary=usage_summary(&c,&q).unwrap();let partial=usage_analytics_at(&c,&q,None,&summary,15).unwrap();
        assert_eq!(partial.effective_end_at,15);assert_eq!(partial.average_tpm,summary.total_tokens as f64*12.0);
        q.start_at=Some(100);q.end_at=Some(200);
        let summary=usage_summary(&c,&q).unwrap();let future=usage_analytics_at(&c,&q,None,&summary,50).unwrap();
        assert_eq!(future.average_rpm,0.0);assert_eq!(future.average_tpm,0.0);assert_eq!(future.effective_end_at,50);
        q.start_at=Some(0);q.end_at=Some(100*365*86400);
        let summary=usage_summary(&c,&q).unwrap();let long=usage_analytics_at(&c,&q,None,&summary,100).unwrap();assert!(long.buckets.len()<=120);
        q.start_at=None;q.end_at=None;
        let inferred=usage_analytics_at(&c,&q,None,&summary,100).unwrap();assert_eq!((inferred.start_at,inferred.end_at),(9,100));
    }
    #[test]
    fn analytics_model_tail_is_combined_without_losing_usage() {
        let c=fixture();
        for i in 0..12 {c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,input_tokens,output_tokens,debit_micro) VALUES(?1,3,3,?2,1,?3,0,0,0,10000,'ok',10,2,1)",params![format!("tail-{i}"),i,format!("model-{i:02}")]).unwrap();}
        let q=UsageQuery{user_id:Some(3),start_at:Some(10),end_at:Some(20),..Default::default()};
        let summary=usage_summary(&c,&q).unwrap();let analytics=usage_analytics_at(&c,&q,None,&summary,20).unwrap();
        assert_eq!(analytics.models.len(),9);let tail=analytics.models.last().unwrap();assert!(tail.is_other);assert_eq!(tail.total_tokens,48);assert_eq!(tail.ledger_count,4);
        assert_eq!(analytics.models.iter().map(|m|m.total_tokens).sum::<i64>(),144);
        assert_eq!(analytics.model_series.len(),9);assert_eq!(analytics.model_series.last().unwrap().ledger_count,4);
        assert_eq!(analytics.model_series.iter().map(|s|s.buckets[0].ledger_count).sum::<i64>(),12);
        // A low-token model with more attempts leads calls but not tokens.
        c.execute("UPDATE billing_ledger SET input_tokens=1,output_tokens=0 WHERE user_id=3 AND model='model-11'",[]).unwrap();
        for i in 0..2 {c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,input_tokens,output_tokens,debit_micro) VALUES(?1,3,3,11,1,'model-11',0,0,0,10000,'ok',0,0,1)",[format!("more-calls-{i}")]).unwrap();}
        let summary=usage_summary(&c,&q).unwrap();let analytics=usage_analytics_at(&c,&q,None,&summary,20).unwrap();
        assert_eq!(analytics.models[0].model,"model-00");
        assert_eq!(analytics.model_series[0].model,"model-11");assert_eq!(analytics.model_series[0].ledger_count,3);
        assert_eq!(analytics.model_series[0].request_count,1);assert_eq!(analytics.model_series[0].buckets[0].ledger_count,3);
        assert_eq!(analytics.model_series.iter().map(|s|s.ledger_count).sum::<i64>(),14);
        assert_eq!(analytics.model_series.iter().map(|s|s.buckets[0].total_tokens).sum::<i64>(),133);
        assert_eq!(analytics.model_series.iter().map(|s|s.buckets[0].known_debit_micro).sum::<i64>(),summary.known_debit_micro);
    }
    #[test]
    fn invalid_time_ranges_fail_even_for_empty_active_catalog() {
        let c=fixture();let catalog=serde_json::json!({"users":[],"keys":[],"channels":[]});
        for (start,end) in [(Some(20),Some(10)),(Some(10),Some(10)),(Some(-1),None),(None,Some(i64::MAX))] {
            let q=UsageQuery{start_at:start,end_at:end,state:Some("active".into()),..Default::default()};
            assert!(usage_page_with_catalog(&c,&q,Some(&catalog)).is_err());
            assert!(usage_summary_with_catalog(&c,&q,Some(&catalog)).is_err());
        }
    }
}
