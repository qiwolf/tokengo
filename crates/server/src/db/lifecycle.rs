//! Delete live configuration atomically, keeping immutable request and financial attribution.
//! Callers hold the database guard and an enclosing transaction. No historical row is rewritten.
use super::{DbError, Result, channels, identity::Policy, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use tokengo_shared::{ChannelId, same_model_name};

fn invalid(s: &str) -> DbError {
    DbError::Other(s.into())
}
fn ids(c: &Connection, sql: &str, id: i64) -> Result<Vec<i64>> {
    Ok(c.prepare(sql)?
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
fn json(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(|_| invalid("配置内容无效，删除已回滚，请先修复配置"))
}
/// A live request can already have selected a source before persisting its channel ID.
/// Conservatively wait for all running requests, rather than delete under a selected fallback.
pub fn require_idle(c: &Connection) -> Result<()> {
    let busy: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM requests WHERE status='running') OR EXISTS(SELECT 1 FROM billing_ledger WHERE status='reserved') OR EXISTS(SELECT 1 FROM model_cooldowns WHERE lease_until>?1) OR EXISTS(SELECT 1 FROM channel_faults WHERE lease_until>?1)", [now_ms()], |r|r.get(0))?;
    if busy {
        return Err(invalid(
            "仍有进行中的请求、额度结算或探测，请完成后重试删除",
        ));
    }
    Ok(())
}
fn clean_policies(c: &Connection, removed: &[i64], orphan_models: &[String]) -> Result<()> {
    for table in ["users", "gateway_keys"] {
        let rows = c
            .prepare(&format!("SELECT id,policy_json FROM {table}"))?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, raw) in rows {
            let mut policy: Policy =
                serde_json::from_str(&raw).map_err(|_| invalid("用户授权配置无效，删除已回滚"))?;
            let old = policy.clone();
            if let Some(ids) = &mut policy.allowed_channels {
                ids.retain(|id| !removed.contains(id));
            }
            if let Some(models) = &mut policy.allowed_models {
                models.retain(|n| !orphan_models.iter().any(|o| same_model_name(o, n)));
            }
            // Keep Some([]): removing the last explicit grant must never grant all sources/models.
            if policy != old {
                c.execute(
                    &format!("UPDATE {table} SET policy_json=?2 WHERE id=?1"),
                    params![id, serde_json::to_string(&policy).unwrap()],
                )?;
            }
        }
    }
    let rows = c
        .prepare("SELECT key,value FROM settings WHERE key LIKE 'model_routing:%'")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (key, raw) in rows {
        let mut p = json(&raw)?;
        if let Some(list) = p.get_mut("channel_ids").and_then(Value::as_array_mut) {
            list.retain(|v| !v.as_i64().is_some_and(|id| removed.contains(&id)));
            c.execute(
                "UPDATE settings SET value=?2 WHERE key=?1",
                params![key, p.to_string()],
            )?;
        }
    }
    Ok(())
}
/// Keep remaining virtual targets in the same order, remapping every schedule/default index.
fn clean_virtual_targets(
    c: &Connection,
    provider: i64,
    only_models: Option<&[String]>,
) -> Result<()> {
    let vms = ids(
        c,
        "SELECT DISTINCT virtual_model_id FROM user_virtual_targets WHERE provider_id=?1",
        provider,
    )?;
    for vm in vms {
        let rows = c.prepare("SELECT position,provider_id,logical_model,switch_on_429,switch_on_400 FROM user_virtual_targets WHERE virtual_model_id=?1 ORDER BY position")?.query_map([vm],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?,r.get::<_,bool>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let kept = rows
            .into_iter()
            .filter(|r| {
                r.1 != provider
                    || only_models
                        .is_some_and(|models| !models.iter().any(|m| same_model_name(m, &r.2)))
            })
            .collect::<Vec<_>>();
        let remap = |raw: &str| -> Result<String> {
            let array = json(raw)?;
            let array = array
                .as_array()
                .ok_or_else(|| invalid("虚拟模型调用顺序无效"))?;
            Ok(Value::Array(
                array
                    .iter()
                    .filter_map(|n| {
                        n.as_i64()
                            .and_then(|old| kept.iter().position(|r| r.0 == old))
                            .map(|i| json!(i + 1))
                    })
                    .collect(),
            )
            .to_string())
        };
        let defaults: String = c.query_row(
            "SELECT default_positions_json FROM user_virtual_models WHERE id=?1",
            [vm],
            |r| r.get(0),
        )?;
        let windows=c.prepare("SELECT position,target_positions_json FROM user_virtual_windows WHERE virtual_model_id=?1")?.query_map([vm],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut incomplete_window = false;
        for (position, raw) in windows {
            let mapped = remap(&raw)?;
            // Keep the now-empty window: removing it would silently activate the default
            // chain for that period. Disable the alias until the user repairs the slot.
            incomplete_window |= mapped == "[]";
            c.execute("UPDATE user_virtual_windows SET target_positions_json=?3 WHERE virtual_model_id=?1 AND position=?2",params![vm,position,mapped])?;
        }
        c.execute(
            "DELETE FROM user_virtual_targets WHERE virtual_model_id=?1",
            [vm],
        )?;
        for (i, (_, parent, model, on429, on400)) in kept.iter().enumerate() {
            c.execute(
                "INSERT INTO user_virtual_targets VALUES(?1,?2,?3,?4,?5,?6)",
                params![vm, (i + 1) as i64, parent, model, on429, on400],
            )?;
        }
        c.execute("UPDATE user_virtual_models SET default_positions_json=?2,enabled=CASE WHEN ?3 THEN 0 ELSE enabled END,version=version+1,updated_ms=?4 WHERE id=?1",params![vm,remap(&defaults)?,kept.is_empty() || incomplete_window,now_ms()])?;
    }
    Ok(())
}
/// Reparenting old configuration may leave an unused credential carrying this source ID.
/// Such empty credentials are configuration, not a reason to keep a deleted source alive.
fn remove_empty_carriers(c: &Connection, id: i64) -> Result<()> {
    c.execute("DELETE FROM credential_affinity WHERE credential_id IN (SELECT id FROM provider_credentials WHERE secret_source_id=?1 AND NOT EXISTS(SELECT 1 FROM source_credentials WHERE credential_id=provider_credentials.id))",[id])?;
    c.execute("DELETE FROM provider_credentials WHERE secret_source_id=?1 AND NOT EXISTS(SELECT 1 FROM source_credentials WHERE credential_id=provider_credentials.id)",[id])?;
    Ok(())
}
fn delete_source_config(c: &Connection, id: i64) -> Result<Vec<String>> {
    let row = channels::get(c, ChannelId(id))?.ok_or_else(|| invalid("模型接入不存在"))?;
    // Native history handles are live bindings; remove them, not historical request/usage rows.
    let sessions = c
        .prepare("SELECT key,pin_json FROM protocol_sessions")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (key, raw) in sessions {
        if json(&raw)?.get("channel").and_then(Value::as_i64) == Some(id) {
            c.execute("DELETE FROM protocol_sessions WHERE key=?1", [key])?;
        }
    }
    c.execute("DELETE FROM routing_state WHERE active_channel_id=?1", [id])?;
    remove_empty_carriers(c, id)?;
    channels::delete(c, ChannelId(id))?;
    Ok(row.models.into_iter().map(|m| m.model).collect())
}
fn orphaned(c: &Connection, names: &[String]) -> Result<Vec<String>> {
    let remaining = channels::list(c)?;
    Ok(names
        .iter()
        .filter(|n| {
            !remaining
                .iter()
                .any(|ch| ch.models.iter().any(|m| same_model_name(&m.model, n)))
        })
        .cloned()
        .collect())
}
pub fn delete_provider(c: &Connection, id: i64) -> Result<Vec<String>> {
    require_idle(c)?;
    let name: String = c.query_row(
        "SELECT name FROM provider_channels WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    let sources = ids(
        c,
        "SELECT channel_id FROM provider_connections WHERE provider_id=?1",
        id,
    )?;
    let unsafe_link:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM source_credentials s JOIN provider_credentials k ON k.id=s.credential_id LEFT JOIN provider_connections p ON p.channel_id=s.source_id WHERE (k.provider_id=?1 AND (p.provider_id IS NULL OR p.provider_id<>?1)) OR (p.provider_id=?1 AND k.provider_id<>?1))",[id],|r|r.get(0))?;
    if unsafe_link {
        return Err(invalid("Key归属与渠道不一致，删除已回滚，请先修复归属"));
    }
    clean_virtual_targets(c, id, None)?;
    c.execute("DELETE FROM credential_affinity WHERE provider_id=?1", [id])?;
    c.execute("DELETE FROM source_credentials WHERE credential_id IN (SELECT id FROM provider_credentials WHERE provider_id=?1)",[id])?;
    c.execute(
        "DELETE FROM provider_credentials WHERE provider_id=?1",
        [id],
    )?;
    let mut names = Vec::new();
    let mut snapshots = Vec::new();
    for source in &sources {
        let row = channels::get(c, ChannelId(*source))?.unwrap();
        snapshots.push(json!({"source_id":source,"name":row.name,"models":row.models.iter().map(|m|&m.model).collect::<Vec<_>>()}));
        names.extend(delete_source_config(c, *source)?);
    }
    clean_policies(c, &sources, &orphaned(c, &names)?)?;
    c.execute("DELETE FROM provider_channels WHERE id=?1", [id])?;
    c.execute("INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json) VALUES(?1,'provider.delete','provider',?2,?3)",params![now_ms(),id,json!({"name":name,"sources":snapshots,"history_preserved":true}).to_string()])?;
    Ok(names)
}
pub fn delete_source(c: &Connection, id: i64) -> Result<Vec<String>> {
    require_idle(c)?;
    let credential: Option<i64> = c
        .query_row(
            "SELECT credential_id FROM source_credentials WHERE source_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(key) = credential {
        let others = ids(
            c,
            "SELECT source_id FROM source_credentials WHERE credential_id=?1 ORDER BY source_id",
            key,
        )?
        .into_iter()
        .filter(|s| *s != id)
        .collect::<Vec<_>>();
        c.execute(
            "DELETE FROM credential_affinity WHERE credential_id=?1",
            [key],
        )?;
        c.execute("DELETE FROM source_credentials WHERE source_id=?1", [id])?;
        if let Some(next) = others.first() {
            c.execute("INSERT OR IGNORE INTO channel_secrets(channel_id,nonce,cipher) SELECT ?2,nonce,cipher FROM channel_secrets WHERE channel_id=?1",params![id,next])?;
            c.execute("UPDATE provider_credentials SET secret_source_id=CASE WHEN secret_source_id=?1 THEN ?2 ELSE secret_source_id END,revision=revision+1 WHERE id=?3",params![id,next,key])?;
        } else {
            c.execute("DELETE FROM provider_credentials WHERE id=?1", [key])?;
        }
    }
    // Refuse corrupt cross-credential carriers instead of deleting another Key's secret.
    remove_empty_carriers(c, id)?;
    let carriers: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM provider_credentials WHERE secret_source_id=?1)",
        [id],
        |r| r.get(0),
    )?;
    if carriers {
        return Err(invalid("密钥承载归属异常，删除已回滚"));
    }
    let parent: Option<i64> = c
        .query_row(
            "SELECT provider_id FROM provider_connections WHERE channel_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let names = delete_source_config(c, id)?;
    if let Some(parent) = parent {
        let remaining = channels::list(c)?;
        let missing = names
            .iter()
            .filter(|n| {
                !remaining.iter().any(|ch| {
                    ch.provider_id == Some(parent)
                        && ch.models.iter().any(|m| same_model_name(&m.model, n))
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            clean_virtual_targets(c, parent, Some(&missing))?;
        }
    }
    clean_policies(c, &[id], &orphaned(c, &names)?)?;
    c.execute("INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json) VALUES(?1,'source.delete','source',?2,?3)",params![now_ms(),id,json!({"models":names,"history_preserved":true}).to_string()])?;
    Ok(names)
}

pub fn archived_user(c: &Connection, id: i64) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key=?1)",
        [format!("deleted_user:{id}")],
        |r| r.get(0),
    )?)
}
pub fn archive_user(c: &Connection, id: i64, actor: i64) -> Result<()> {
    let u = super::identity::user(c, id)?.ok_or_else(|| invalid("用户不存在"))?;
    if id == 1 {
        return Err(invalid("管理员身份不能删除，请在账号安全中管理"));
    }
    let busy:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM requests WHERE user_id=?1 AND status='running') OR EXISTS(SELECT 1 FROM billing_ledger WHERE user_id=?1 AND status='reserved') OR EXISTS(SELECT 1 FROM billing_budgets WHERE scope='user' AND scope_id=?1 AND reserved_micro>0)",[id],|r|r.get(0))?;
    if busy {
        return Err(invalid("该用户仍有进行中的请求或未结算额度，完成后再删除"));
    }
    let logins = c
        .prepare("SELECT username FROM panel_accounts WHERE user_id=?1")?
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    c.execute("UPDATE users SET enabled=0,policy_json=?2 WHERE id=?1",params![id,json!({"allowed_models":[],"allowed_channels":[],"allow_paid":false,"admin_access":false}).to_string()])?;
    c.execute("UPDATE gateway_keys SET enabled=0,revoked_at=COALESCE(revoked_at,?2),policy_json=?3 WHERE user_id=?1",params![id,now_ms(),json!({"allowed_models":[],"allowed_channels":[],"allow_paid":false}).to_string()])?;
    c.execute("DELETE FROM panel_accounts WHERE user_id=?1", [id])?;
    c.execute("DELETE FROM credential_affinity WHERE user_id=?1", [id])?;
    c.execute("DELETE FROM user_virtual_models WHERE user_id=?1", [id])?;
    super::settings::set(c, &format!("deleted_user:{id}"), &now_ms().to_string())?;
    c.execute("INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json) VALUES(?1,'managed_user.delete','user',?2,?3)",params![now_ms(),id,json!({"actor_account_id":actor,"name":u.name,"usernames":logins,"history_preserved":true}).to_string()])?;
    Ok(())
}
