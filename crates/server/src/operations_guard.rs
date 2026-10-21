//! Per-lock machine context and SQLite statement-atomic revocation/audit gates.
//! The single connection mutex prevents another request inheriting a machine context.
use crate::{
    db,
    routes::operations::{CONTEXT, OperationsContext},
};
use rusqlite::{Connection, params};
use std::{
    ops::{Deref, DerefMut},
    sync::MutexGuard,
};

pub fn install(c: &Connection) -> db::Result<()> {
    c.execute_batch("CREATE TEMP TABLE operations_request_context(singleton INTEGER PRIMARY KEY CHECK(singleton=1), token_id INTEGER, request_id TEXT, method TEXT, path TEXT); INSERT INTO operations_request_context(singleton) VALUES(1);")?;
    let tables = c
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (index, table) in tables.iter().enumerate() {
        if matches!(table.as_str(), "audit_events" | "billing_audit") {
            continue;
        }
        let quoted = table.replace('"', "\"\"");
        for (verb, event, row) in [
            ("INSERT", "insert", "NEW"),
            ("UPDATE", "update", "NEW"),
            ("DELETE", "delete", "OLD"),
        ] {
            let deny = matches!(
                table.as_str(),
                "operations_tokens"
                    | "passthrough_routes"
                    | "passthrough_headers"
                    | "passthrough_acl"
                    | "operations_instance"
                    | "schema_migrations"
                    | "panel_sessions"
            );
            let extra = if deny {
                "SELECT RAISE(ABORT,'operations_action_forbidden');".to_string()
            } else if table == "channels" {
                format!(
                    "SELECT CASE WHEN {row}.transport_kind='passthrough' THEN RAISE(ABORT,'operations_action_forbidden') END;"
                )
            } else if table == "panel_accounts" {
                format!(
                    "SELECT CASE WHEN {row}.role='root' THEN RAISE(ABORT,'operations_action_forbidden') END;"
                )
            } else {
                String::new()
            };
            c.execute_batch(&format!(r#"CREATE TEMP TRIGGER ops_gate_{index}_{event} BEFORE {verb} ON main."{quoted}"
    WHEN (SELECT token_id FROM operations_request_context WHERE singleton=1) IS NOT NULL
    BEGIN SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM operations_tokens WHERE id=(SELECT token_id FROM operations_request_context WHERE singleton=1) AND revoked_at_ms IS NULL AND expires_at_ms>CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)) THEN RAISE(ABORT,'operations_token_inactive') END; {extra} END;
    CREATE TEMP TRIGGER ops_audit_{index}_{event} AFTER {verb} ON main."{quoted}"
    WHEN (SELECT token_id FROM operations_request_context WHERE singleton=1) IS NOT NULL
    BEGIN INSERT INTO audit_events(ts_ms,action,target_type,target_id,metadata_json)
     SELECT CAST((julianday('now')-2440587.5)*86400000 AS INTEGER),'operations.{event}','{table}',{row}.rowid,
      json_object('actor_type','operations_token','actor_id',token_id,'request_id',request_id,'method',method,'path',path,'success',json('true')) FROM operations_request_context WHERE singleton=1;
    END;"#))?;
        }
    }
    c.execute_batch(r#"CREATE TEMP TRIGGER ops_audit_actor AFTER INSERT ON main.audit_events
 WHEN (SELECT token_id FROM operations_request_context WHERE singleton=1) IS NOT NULL
 BEGIN UPDATE audit_events SET actor_user_id=NULL,actor_key_id=NULL,metadata_json=json_set(json_remove(CASE WHEN json_valid(NEW.metadata_json) THEN NEW.metadata_json ELSE '{}' END,'$.actor_account_id'),'$.actor_type','operations_token','$.actor_id',(SELECT token_id FROM operations_request_context WHERE singleton=1),'$.request_id',(SELECT request_id FROM operations_request_context WHERE singleton=1),'$.success',json('true')) WHERE id=NEW.id; END;
 CREATE TEMP TRIGGER ops_billing_actor AFTER INSERT ON main.billing_audit
 WHEN (SELECT token_id FROM operations_request_context WHERE singleton=1) IS NOT NULL
 BEGIN UPDATE billing_audit SET actor='operations_token:'||(SELECT token_id FROM operations_request_context WHERE singleton=1) WHERE id=NEW.id; END;"#)?;
    Ok(())
}

pub struct Guard<'a> {
    c: MutexGuard<'a, Connection>,
    context: bool,
}
impl<'a> Guard<'a> {
    pub fn new(c: MutexGuard<'a, Connection>) -> Self {
        let context = CONTEXT.try_with(Clone::clone).ok();
        if let Some(OperationsContext {
            token_id,
            request_id,
            method,
            path,
        }) = &context
        {
            // Failure cannot continue with root context: unwind the request before any handler SQL.
            c.execute("UPDATE temp.operations_request_context SET token_id=?1,request_id=?2,method=?3,path=?4 WHERE singleton=1",params![token_id,request_id,method,path]).expect("machine database context unavailable");
        }
        Self {
            c,
            context: context.is_some(),
        }
    }
}
impl Deref for Guard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.c
    }
}
impl DerefMut for Guard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.c
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if self.context {
            // Reset after any SQL transaction (including a rollback). Failure poisons authority closed:
            // leave the machine context attached; never silently turn a failed reset into root authority.
            if self.c.execute("UPDATE temp.operations_request_context SET token_id=NULL,request_id=NULL,method=NULL,path=NULL WHERE singleton=1",[]).is_err(){panic!("machine database context cleanup failed");}
        }
    }
}
