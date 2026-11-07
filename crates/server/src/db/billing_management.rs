//! Administration labels and lifecycle state. Historical ledger rows remain intact.
use super::{billing, identity, lifecycle, now_ms, DbError, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

fn bad(message: &str) -> DbError { DbError::Other(message.into()) }
pub fn validated_state<'a>(state: Option<&'a str>, default: &'a str) -> Result<&'a str> {
    match state.unwrap_or(default) {
        value @ ("all" | "active" | "historical") => Ok(value),
        _ => Err(bad("state must be active, historical or all")),
    }
}
fn object(id: i64, name: String, active: bool, label: &str) -> Value {
    json!({"id":id,"name":name,"state":if active {"active"}else{"historical"},"status_label":label})
}
pub fn current_objects(c: &Connection) -> Result<Value> {
    let users = c.prepare("SELECT id,name,enabled FROM users ORDER BY id")?.query_map([], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut user_objects=Vec::new();
    for (id,name,enabled) in users {
        let archived=lifecycle::archived_user(c,id)?;
        user_objects.push(object(id,name,enabled&&!archived,if archived {"已归档"}else if !enabled {"已停用"}else{"有效"}));
    }
    let keys=c.prepare("SELECT id,name,user_id,enabled,revoked_at,expires_at FROM gateway_keys ORDER BY id")?.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,bool>(3)?,r.get::<_,Option<i64>>(4)?,r.get::<_,Option<i64>>(5)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut key_objects=Vec::new();
    for (id,name,user_id,enabled,revoked,expires) in keys {
        let deleted=identity::key_deleted(c,id)?;
        let active=!deleted && identity::context(c,id)?.is_some();
        let label=if deleted {"已删除"}else if revoked.is_some(){"已撤销"}else if expires.is_some_and(|v|v<=now_ms()){"已过期"}else if !enabled{"已停用"}else if !active{"用户或父级 Key 无效"}else{"有效"};
        let mut value=object(id,name,active,label);
        value["user_id"]=json!(user_id);
        value["user_name"]=user_objects.iter().find(|v|v["id"]==user_id).map(|v|v["name"].clone()).unwrap_or(json!("已删除用户"));
        key_objects.push(value);
    }
    let channels=c.prepare("SELECT c.id,c.name,c.enabled,COALESCE(p.enabled,1),p.name,k.label,k.id FROM channels c LEFT JOIN provider_connections pc ON pc.channel_id=c.id LEFT JOIN provider_channels p ON p.id=pc.provider_id LEFT JOIN source_credentials sc ON sc.source_id=c.id LEFT JOIN provider_credentials k ON k.id=sc.credential_id ORDER BY c.id")?.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?,r.get::<_,bool>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<i64>>(6)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut channel_objects=Vec::new();
    for (id,source_name,enabled,parent,provider,credential,credential_id) in channels {
        let credential_valid=if credential_id.is_some() {super::provider_pool::source_credential(c,id)?.is_some_and(|v|v.enabled)}else{true};
        let name=[provider.as_deref(),credential.as_deref(),Some(source_name.as_str())].into_iter().flatten().filter(|v|!v.trim().is_empty()).collect::<Vec<_>>().join(" · ");
        let mut value=object(id,name,enabled&&parent&&credential_valid,if !enabled{"已停用"}else if !parent{"供应方已停用"}else if !credential_valid{"上游凭据不可用"}else{"有效"});
        value["source_name"]=json!(source_name);value["provider_name"]=json!(provider);value["credential_name"]=json!(credential);
        channel_objects.push(value);
    }
    Ok(json!({"users":user_objects,"keys":key_objects,"channels":channel_objects}))
}
// Seek once per distinct channel instead of visiting every historical ledger row.
// Only removed channels load a snapshot, always from their greatest ledger ID.
pub(super) const HISTORICAL_CHANNELS_SQL: &str = "WITH RECURSIVE channel_ids(channel_id) AS (SELECT MIN(channel_id) FROM billing_ledger UNION ALL SELECT (SELECT MIN(channel_id) FROM billing_ledger WHERE channel_id>channel_ids.channel_id) FROM channel_ids WHERE channel_id IS NOT NULL) SELECT l.channel_id,COALESCE(json_extract(l.source_snapshot,'$.channel_name'),json_extract(l.source_snapshot,'$.source_name'),'已删除渠道') FROM channel_ids h JOIN billing_ledger l ON l.id=(SELECT id FROM billing_ledger WHERE channel_id=h.channel_id ORDER BY id DESC LIMIT 1) WHERE h.channel_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM channels c WHERE c.id=h.channel_id) ORDER BY l.channel_id";
pub fn objects(c: &Connection) -> Result<Value> {
    let mut catalog=current_objects(c)?;
    let deleted=c.prepare(HISTORICAL_CHANNELS_SQL)?.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    catalog["channels"].as_array_mut().unwrap().extend(deleted.into_iter().map(|(id,name)|object(id,name,false,"已删除")));
    Ok(catalog)
}
fn find<'a>(catalog: &'a Value, kind: &str, id: i64) -> Option<&'a Value> {
    catalog[kind].as_array()?.iter().find(|v|v["id"]==id)
}
pub fn require_active_object(c: &Connection, scope: &str, id: i64) -> Result<()> {
    let active=match scope {
        "user" => identity::user(c,id)?.is_some_and(|u|u.enabled),
        "key" => !identity::key_deleted(c,id)? && identity::context(c,id)?.is_some(),
        _ => false,
    };
    if id<=0 || !active { return Err(bad("请选择当前有效的用户或 API Key；历史对象不能设置预算")); }
    Ok(())
}
pub fn budgets(c: &Connection, scope: Option<&str>, id: Option<i64>, state: Option<&str>) -> Result<Vec<Value>> {
    let state=validated_state(state,"active")?;
    let catalog=current_objects(c)?;
    // Filter before applying the list limit: historical rows cannot hide active records.
    if scope.is_some_and(|v|v!="user"&&v!="key") {return Err(bad("invalid budget scope"));}
    let mut sql="SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen FROM billing_budgets WHERE 1=1".to_string();
    let mut args=Vec::<rusqlite::types::Value>::new();
    if let Some(scope)=scope {sql.push_str(" AND scope=?");args.push(scope.to_string().into());}
    if let Some(id)=id {sql.push_str(" AND scope_id=?");args.push(id.into());}
    sql.push_str(" ORDER BY scope,scope_id");
    let mut statement=c.prepare(&sql)?;
    let rows=statement.query_map(rusqlite::params_from_iter(args),billing::budget_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut result=Vec::new();
    for budget in rows {
        let scope=budget.scope.as_str();
        let id=budget.scope_id;
        let obj=find(&catalog,if scope=="user"{"users"}else{"keys"},id);
        let object_state=obj.map(|v|v["state"].as_str().unwrap_or("historical")).unwrap_or("historical");
        if state!="all"&&state!=object_state {continue;}
        {
            let mut row=serde_json::to_value(budget).map_err(|_|bad("cannot encode budget"))?;
            row["object_name"]=obj.map(|v|v["name"].clone()).unwrap_or(json!("已删除对象"));
            row["object_state"]=json!(object_state);
            row["status_label"]=obj.map(|v|v["status_label"].clone()).unwrap_or(json!("已删除"));
            row["user_name"]=obj.map(|v|v["user_name"].clone()).unwrap_or(Value::Null);
            result.push(row);
            if result.len()>=1000 {break;}
        }
    }
    Ok(result)
}
pub fn enrich_entries(c:&Connection,entries:Vec<billing::Ledger>)->Result<Vec<Value>> {
    let catalog=objects(c)?;
    enrich_entries_with_catalog(entries,&catalog)
}
pub fn enrich_entries_with_catalog(entries:Vec<billing::Ledger>,catalog:&Value)->Result<Vec<Value>> {
    entries.into_iter().map(|entry|{
        let mut row=serde_json::to_value(entry).map_err(|_|bad("cannot encode ledger"))?;
        let mut active=true;
        for (field,kind,fallback) in [("user","users","已删除用户"),("key","keys","已删除 Key"),("channel","channels","已删除渠道")] {
            let id=row[format!("{field}_id")].as_i64().unwrap_or(0);
            let obj=find(&catalog,kind,id);
            let mut name=obj.map(|v|v["name"].clone()).unwrap_or(json!(fallback));
            if field=="channel" && obj.is_none(){
                if let Some(snapshot_name)=row["source_snapshot"]["channel_name"].as_str().or_else(||row["source_snapshot"]["source_name"].as_str()){name=json!(snapshot_name);}
            }
            if field=="user" && id==0 {name=json!("系统诊断");}
            if field=="key" && id==0 {name=json!("无需 API Key");}
            let state=obj.map(|v|v["state"].clone()).unwrap_or(json!("historical"));
            active &= state=="active";
            row[format!("{field}_name")]=name;
            row[format!("{field}_state")]=state;
            row[format!("{field}_status_label")]=obj.map(|v|v["status_label"].clone()).unwrap_or(json!("历史对象"));
        }
        row["record_state"]=json!(if active{"active"}else{"historical"});
        Ok(row)
    }).collect()
}
pub fn update_budget(c:&Connection,scope:&str,id:i64,limit:Option<i64>,frozen:Option<bool>,actor:&str)->Result<()> {
    super::in_tx(c,|c|{
        require_active_object(c,scope,id)?;
        if limit.is_some_and(|v|v<0){return Err(bad("预算不能为负数"));}
        if frozen==Some(false){
            let (spent,reserved):(i64,i64)=c.query_row("SELECT COALESCE((SELECT spent_micro FROM billing_budgets WHERE scope=?1 AND scope_id=?2),0),COALESCE((SELECT reserved_micro FROM billing_budgets WHERE scope=?1 AND scope_id=?2),0)",params![scope,id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if limit.is_some_and(|limit|i128::from(limit)<i128::from(spent)+i128::from(reserved)){return Err(bad("解冻前请提高限额或解除限额，以覆盖已扣和预占金额"));}
        }
        c.execute("INSERT INTO billing_budgets(scope,scope_id,limit_micro,frozen) VALUES(?1,?2,?3,COALESCE(?4,0)) ON CONFLICT(scope,scope_id) DO UPDATE SET limit_micro=excluded.limit_micro,frozen=COALESCE(?4,billing_budgets.frozen)",params![scope,id,limit,frozen])?;
        c.execute("INSERT INTO billing_audit(event,actor,object,detail,created_ms) VALUES('budget_updated',?1,?2,?3,?4)",params![actor,format!("{scope}:{id}"),json!({"limit_micro":limit,"frozen":frozen}).to_string(),now_ms()])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const LEGACY_HISTORICAL_CHANNELS_SQL:&str="SELECT l.channel_id,COALESCE(json_extract(l.source_snapshot,'$.channel_name'),json_extract(l.source_snapshot,'$.source_name'),'已删除渠道') FROM billing_ledger l JOIN (SELECT channel_id,MAX(id) AS latest_id FROM billing_ledger WHERE channel_id NOT IN (SELECT id FROM channels) GROUP BY channel_id) history ON history.latest_id=l.id ORDER BY l.channel_id";
    fn historical_rows_and_steps(c:&Connection,sql:&str)->(Vec<(i64,String)>,i32) {
        let mut statement=c.prepare(sql).unwrap();
        let rows=statement.query_map([],|row|Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        (rows,statement.get_status(rusqlite::StatementStatus::VmStep))
    }
    #[test]
    fn historical_channel_seeks_match_legacy_empty_extreme_ids_and_latest_null_fallback() {
        let(c,user,key)=fixture();
        assert_eq!(historical_rows_and_steps(&c,HISTORICAL_CHANNELS_SQL).0,historical_rows_and_steps(&c,LEGACY_HISTORICAL_CHANNELS_SQL).0);
        assert!(historical_rows_and_steps(&c,HISTORICAL_CHANNELS_SQL).0.is_empty());
        c.execute("INSERT INTO channels(id,name,base_url,enabled,priority) VALUES(0,'zero-current','http://fixture.invalid',1,0)",[]).unwrap();
        let mut ordinal=0;
        for (channel,snapshot) in [
            (-20,r#"{"channel_name":"negative-old"}"#),
            (-20,r#"{"channel_name":"negative-latest"}"#),
            (0,r#"{"channel_name":"current-zero-excluded"}"#),
            (1,r#"{"channel_name":"current-one-excluded"}"#),
            (99,r#"{"channel_name":"old-nonnull-name"}"#),
            (99,r#"{"channel_name":null,"source_name":"latest-source-fallback"}"#),
            (100,r#"{"channel_name":"must-not-reuse-this-old-name"}"#),
            (100,r#"{"channel_name":null,"source_name":null}"#),
            (i64::MAX,r#"{"source_name":"maximum-id-source"}"#),
        ] {
            ordinal+=1;
            c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,source_snapshot) VALUES(?1,?2,?3,0,?4,'synthetic',0,0,0,0,'settled',?5)",params![format!("seek-case-{ordinal}"),user,key,channel,snapshot]).unwrap();
        }
        let(rows,steps)=historical_rows_and_steps(&c,HISTORICAL_CHANNELS_SQL);
        assert_eq!(rows,historical_rows_and_steps(&c,LEGACY_HISTORICAL_CHANNELS_SQL).0);
        assert_eq!(rows,vec![(-20,"negative-latest".into()),(99,"latest-source-fallback".into()),(100,"已删除渠道".into()),(i64::MAX,"maximum-id-source".into())]);
        assert!(steps<2000,"six distinct channels must use bounded seeks: {steps}");
    }
    #[test]
    fn historical_channel_vm_steps_follow_distinct_channels_not_ledger_length() {
        let mut measurements=Vec::new();
        for count in [100,100000] {
            // Isolate the production SQL and index from unrelated migration/setup cost.
            let c=Connection::open_in_memory().unwrap();
            c.execute_batch("CREATE TABLE channels(id INTEGER PRIMARY KEY);INSERT INTO channels VALUES(1),(2),(3);CREATE TABLE billing_ledger(id INTEGER PRIMARY KEY,channel_id INTEGER NOT NULL,source_snapshot TEXT NOT NULL,user_id INTEGER,key_id INTEGER,model TEXT);CREATE INDEX billing_ledger_channel ON billing_ledger(channel_id,id DESC,user_id,key_id,model)").unwrap();
            c.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<?1) INSERT INTO billing_ledger(channel_id,source_snapshot) SELECT i%7,json_object('channel_name','latest-'||i) FROM n",[count]).unwrap();
            let(rows,steps)=historical_rows_and_steps(&c,HISTORICAL_CHANNELS_SQL);
            assert_eq!(rows,historical_rows_and_steps(&c,LEGACY_HISTORICAL_CHANNELS_SQL).0);
            assert_eq!(rows.len(),4);
            assert!(steps<2000,"seven distinct channels, {count} rows: {steps} VM steps");
            let plan=c.prepare(&format!("EXPLAIN QUERY PLAN {HISTORICAL_CHANNELS_SQL}")).unwrap().query_map([],|row|row.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap().join("\n");
            assert!(plan.contains("COVERING INDEX billing_ledger_channel (channel_id>?)"),"{plan}");
            assert!(plan.contains("COVERING INDEX billing_ledger_channel (channel_id=?)"),"{plan}");
            assert!(!plan.lines().any(|line|line.starts_with("SCAN billing_ledger")),"{plan}");
            measurements.push(steps);
        }
        assert!(measurements[1]<=measurements[0]+64,"fixed distinct count must not scale with ledger length: {measurements:?}");
        eprintln!("historical channel seeks, 7 distinct IDs, 100/100000 ledger rows: VM steps={measurements:?}");
    }
    #[test]
    fn indexed_history_preserves_latest_snapshot_and_large_ledger_pagination() {
        let(c,user,key)=fixture();
        // The payload makes a table scan materially different from the compact index.
        c.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<6000) INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,source_snapshot) SELECT 'bulk-'||i,?1,?2,i,99,'history model',0,0,0,i,'ok',json_object('channel_name','历史上游 '||i,'padding',printf('%4096s','')) FROM n",params![user,key]).unwrap();
        let id=billing::reserve_unpriced(&c,user,key,7001,"current-last",1,"current-model").unwrap();
        billing::settle(&c,id,Some(1),Some(1),"ok").unwrap();
        let catalog=objects(&c).unwrap();
        assert_eq!(find(&catalog,"channels",99).unwrap()["name"],"历史上游 6000");
        assert!(find(&current_objects(&c).unwrap(),"channels",99).is_none());
        let plan=c.prepare(&format!("EXPLAIN QUERY PLAN {HISTORICAL_CHANNELS_SQL}")).unwrap().query_map([],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap().join("\n");
        assert!(plan.contains("COVERING INDEX billing_ledger_channel"),"{plan}");
        assert!(!plan.contains("SCAN billing_ledger\n"),"{plan}");
        let(rows,steps)=historical_rows_and_steps(&c,HISTORICAL_CHANNELS_SQL);
        assert_eq!(rows,historical_rows_and_steps(&c,LEGACY_HISTORICAL_CHANNELS_SQL).0);
        assert!(steps<2000,"two distinct channels across 6001 wide rows: {steps} VM steps");
        for state in ["all","active","historical"] {
            for limit in [0,1,2,200] {
                let q=billing::UsageQuery{state:Some(state.into()),limit:Some(limit),..Default::default()};
                let expected=billing::usage(&c,&q).unwrap();
                let expected_next=billing::usage_next_before(&c,&q,&expected).unwrap();
                let (actual,next)=billing::usage_page_with_catalog(&c,&q,Some(&catalog)).unwrap();
                assert_eq!(actual.iter().map(|r|r.id).collect::<Vec<_>>(),expected.iter().map(|r|r.id).collect::<Vec<_>>());
                assert_eq!(next,expected_next);
                let matching:Vec<i64>=match state {"active"=>vec![id],"historical"=>(1..=6000).rev().collect(),_=>(1..=id).rev().collect()};
                let count=(limit.clamp(1,200) as usize).min(matching.len());
                assert_eq!(actual.iter().map(|r|r.id).collect::<Vec<_>>(),matching[..count]);
                assert_eq!(next,if matching.len()>count {Some(matching[count-1])}else{None});
            }
        }
        let q=billing::UsageQuery{state:Some("active".into()),..Default::default()};
        assert_eq!(billing::usage_page(&c,&q).unwrap().0.len(),1);
        // A new request must observe the mutation, rather than cache valid IDs.
        c.execute("UPDATE gateway_keys SET enabled=0 WHERE id=?1",[key]).unwrap();
        assert!(billing::usage_page(&c,&q).unwrap().0.is_empty());
        assert_eq!(billing::usage(&c,&billing::UsageQuery{channel_id:Some(99),limit:Some(1),..Default::default()}).unwrap().len(),1);
    }
    #[test]
    fn budget_filtering_happens_before_cap_without_historical_ledger_reads() {
        let(c,user,key)=fixture();
        c.execute("WITH RECURSIVE n(i) AS (VALUES(10000) UNION ALL SELECT i+1 FROM n WHERE i<11010) INSERT INTO billing_budgets(scope,scope_id,limit_micro) SELECT 'key',i,100 FROM n",[]).unwrap();
        update_budget(&c,"user",user,Some(42),None,"test").unwrap();
        update_budget(&c,"key",key,Some(17),None,"test").unwrap();
        let rows=budgets(&c,None,None,Some("active")).unwrap();
        assert_eq!(rows.len(),2);
        assert!(rows.iter().any(|r|r["scope"]=="user"&&r["limit_micro"]==42));
        assert_eq!(budgets(&c,None,None,Some("historical")).unwrap().len(),1000);
        assert_eq!(budgets(&c,Some("key"),Some(key),Some("all")).unwrap().len(),1);
    }
    #[test]
    fn lifecycle_pagination_handles_ancient_active_and_no_matching_active_rows() {
        let(c,user,key)=fixture();
        let old=billing::reserve_unpriced(&c,user,key,1,"ancient-active",1,"synthetic").unwrap();
        billing::settle(&c,old,Some(1),Some(1),"ok").unwrap();
        c.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<4000) INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,source_snapshot) SELECT 'sparse-'||i,?1,?2,i,99,'synthetic',0,0,0,i,'ok',json_object('channel_name','removed','padding',printf('%4096s','')) FROM n",params![user,key]).unwrap();
        let q=billing::UsageQuery{state:Some("active".into()),limit:Some(1),..Default::default()};
        let(rows,next)=billing::usage_page(&c,&q).unwrap();
        assert_eq!(rows.iter().map(|r|r.id).collect::<Vec<_>>(),vec![old]);
        assert_eq!(next,None);
        let historical=billing::UsageQuery{state:Some("historical".into()),limit:Some(1),before_id:Some(1000),..Default::default()};
        let(rows,next)=billing::usage_page(&c,&historical).unwrap();
        assert_eq!(rows[0].id,999);assert_eq!(next,Some(999));
        c.execute("UPDATE gateway_keys SET enabled=0 WHERE id=?1",[key]).unwrap();
        assert!(billing::usage_page(&c,&q).unwrap().0.is_empty()); // No active Key: skip ledger entirely.
        identity::create_key(&c,user,"new live key without history",&Default::default(),None,None).unwrap();
        let(rows,next)=billing::usage_page(&c,&q).unwrap(); // Active identities exist, but none match any ledger.
        assert!(rows.is_empty());assert_eq!(next,None);
        let last=billing::UsageQuery{state:Some("historical".into()),before_id:Some(2),limit:Some(1),..Default::default()};
        let(rows,next)=billing::usage_page(&c,&last).unwrap();
        assert_eq!(rows[0].id,old);assert_eq!(next,None);
    }
    fn fixture()->(Connection,i64,i64) {
        let c=super::super::open_in_memory().unwrap();
        let source=super::super::channels::create(&c,&super::super::channels::NewChannel {
            name:"管理测试上游".into(),base_url:"http://127.0.0.1:19999/v1".into(),
            enabled:true,unwrap_envelope:false,max_concurrent:0,
        }).unwrap();
        assert_eq!(source.0,1,"fresh fixture must create the source referenced by its ledgers");
        let user=identity::create_user(&c,"额度管理用户","user",&Default::default()).unwrap();
        let (key,_)=identity::create_key(&c,user.id,"应用 Key",&Default::default(),None,None).unwrap();
        (c,user.id,key.id)
    }
    #[test]
    fn budget_management_rejects_unknown_objects_and_preserves_money_and_audit() {
        let(c,user,key)=fixture();
        assert!(billing::set_budget(&c,"user",99999,Some(100),"test").is_err());
        assert!(update_budget(&c,"key",99999,Some(100),None,"test").is_err());
        update_budget(&c,"user",user,Some(100),Some(true),"test").unwrap();
        c.execute("UPDATE billing_budgets SET spent_micro=50,reserved_micro=20 WHERE scope='user' AND scope_id=?1",[user]).unwrap();
        assert!(update_budget(&c,"user",user,Some(60),Some(false),"test").is_err());
        assert!(billing::budgets(&c,Some("user"),Some(user)).unwrap()[0].frozen);
        update_budget(&c,"user",user,None,Some(false),"test").unwrap();
        let rows=budgets(&c,None,None,None).unwrap();
        assert_eq!(rows[0]["object_name"],"额度管理用户");
        assert_eq!(rows[0]["spent_micro"],50);
        assert_eq!(rows[0]["reserved_micro"],20);
        assert_eq!(rows[0]["frozen"],false);
        assert!(rows[0]["limit_micro"].is_null());
        let events:i64=c.query_row("SELECT count(*) FROM billing_audit WHERE event='budget_updated'",[],|r|r.get(0)).unwrap();
        assert_eq!(events,2);
        update_budget(&c,"key",key,Some(10),None,"test").unwrap();
        let mut k=identity::key(&c,key).unwrap().unwrap();k.enabled=false;identity::update_key(&c,&k).unwrap();
        assert!(update_budget(&c,"key",key,None,Some(false),"test").is_err());
        assert_eq!(budgets(&c,None,None,None).unwrap().len(),1);
        assert_eq!(budgets(&c,None,None,Some("historical")).unwrap()[0]["object_name"],"应用 Key");
    }
    #[test]
    fn ledger_names_and_state_survive_archival_and_filter_before_pagination() {
        let(c,user,key)=fixture();
        let id=billing::reserve_unpriced(&c,user,key,101,"management-history",1,"synthetic").unwrap();
        billing::settle(&c,id,Some(1),Some(1),"ok").unwrap();
        let entries=billing::usage(&c,&billing::UsageQuery{state:Some("active".into()),..Default::default()}).unwrap();
        assert_eq!(entries.len(),1);
        let entries=enrich_entries(&c,entries).unwrap();
        assert_eq!(entries[0]["user_name"],"额度管理用户");
        assert_eq!(entries[0]["key_name"],"应用 Key");
        assert_eq!(entries[0]["record_state"],"active");
        lifecycle::archive_user(&c,user,1).unwrap();
        assert!(billing::usage(&c,&billing::UsageQuery{state:Some("active".into()),..Default::default()}).unwrap().is_empty());
        let historic=billing::usage(&c,&billing::UsageQuery{state:Some("historical".into()),limit:Some(1),..Default::default()}).unwrap();
        assert_eq!(historic.len(),1);
        let historic=enrich_entries(&c,historic).unwrap();
        assert_eq!(historic[0]["user_name"],"额度管理用户");
        assert_eq!(historic[0]["record_state"],"historical");
        assert!(require_active_object(&c,"user",user).is_err());
        assert!(require_active_object(&c,"key",key).is_err());
        assert!(budgets(&c,None,None,None).unwrap().is_empty());
        assert_eq!(budgets(&c,None,None,Some("historical")).unwrap().len(),2);
        assert!(billing::usage(&c,&billing::UsageQuery{state:Some("garbage".into()),..Default::default()}).is_err());
    }
    #[test]
    fn usage_cursor_requires_a_real_next_row_and_preserves_state_filters() {
        let(c,user,key)=fixture();
        let (historical_key,_)=identity::create_key(&c,user,"历史应用",&Default::default(),None,None).unwrap();
        for (attempt,entry_key) in [("old-historical",historical_key.id),("current-one",key),("current-two",key)] {
            let id=billing::reserve_unpriced(&c,user,entry_key,101,attempt,1,"chosen-model").unwrap();
            billing::settle(&c,id,Some(1),Some(1),"ok").unwrap();
        }
        let mut old=identity::key(&c,historical_key.id).unwrap().unwrap();old.enabled=false;identity::update_key(&c,&old).unwrap();
        let mut q=billing::UsageQuery{user_id:Some(user),channel_id:Some(1),model:Some("chosen-model".into()),state:Some("active".into()),limit:Some(3),..Default::default()};
        let rows=billing::usage(&c,&q).unwrap();
        assert_eq!(rows.len(),2); // Unfilled page is already the last page.
        assert_eq!(billing::usage_next_before(&c,&q,&rows).unwrap(),None);
        q.limit=Some(2);
        let rows=billing::usage(&c,&q).unwrap();
        assert_eq!(rows.len(),2); // Exactly full, but no further active row exists.
        assert_eq!(billing::usage_next_before(&c,&q,&rows).unwrap(),None);
        q.limit=Some(1);
        let rows=billing::usage(&c,&q).unwrap();
        let next=billing::usage_next_before(&c,&q,&rows).unwrap();
        assert_eq!(next,Some(rows[0].id));
        q.before_id=next;
        let second=billing::usage(&c,&q).unwrap();
        assert_eq!(second.len(),1);
        assert!(second[0].id<rows[0].id);
        assert_eq!(billing::usage_next_before(&c,&q,&second).unwrap(),None); // Older historical row must not create a false cursor.
        q.before_id=None;q.state=Some("all".into());q.limit=Some(2);
        let rows=billing::usage(&c,&q).unwrap();
        assert_eq!(billing::usage_next_before(&c,&q,&rows).unwrap(),Some(rows[1].id));
        q.state=Some("historical".into());q.limit=Some(1);
        let rows=billing::usage(&c,&q).unwrap();
        assert_eq!(rows.len(),1);
        assert_eq!(rows[0].key_id,historical_key.id);
        assert_eq!(billing::usage_next_before(&c,&q,&rows).unwrap(),None);
    }
    #[test]
    fn source_names_disambiguate_credentials_and_disabled_credentials_are_historical() {
        let(c,_,_)=fixture();
        c.execute("INSERT INTO provider_channels(name,enabled,created_ms,canonical_state) VALUES('供应方',1,0,'canonical')",[]).unwrap();
        let provider=c.last_insert_rowid();
        c.execute("INSERT INTO provider_connections(channel_id,provider_id) VALUES(1,?1) ON CONFLICT(channel_id) DO UPDATE SET provider_id=excluded.provider_id",[provider]).unwrap();
        c.execute("INSERT INTO provider_credentials(provider_id,kind,label,enabled,identity_hash,created_at_ms) VALUES(?1,'anonymous','生产凭据',1,'test-identity',0)",[provider]).unwrap();
        let credential=c.last_insert_rowid();
        c.execute("INSERT INTO source_credentials(source_id,credential_id) VALUES(1,?1) ON CONFLICT(source_id) DO UPDATE SET credential_id=excluded.credential_id",[credential]).unwrap();
        let catalog=objects(&c).unwrap();
        let source=find(&catalog,"channels",1).unwrap();
        assert!(source["name"].as_str().unwrap().contains("供应方 · 生产凭据"));
        assert_eq!(source["state"],"active");
        c.execute("UPDATE provider_credentials SET enabled=0 WHERE id=?1",[credential]).unwrap();
        let catalog=objects(&c).unwrap();
        assert_eq!(find(&catalog,"channels",1).unwrap()["state"],"historical");
        assert_eq!(find(&catalog,"channels",1).unwrap()["status_label"],"上游凭据不可用");
    }
    #[test]
    fn expired_or_disabled_ancestor_keys_are_historical() {
        let(c,user,parent)=fixture();
        let (child,_)=identity::create_key(&c,user,"子 Key",&Default::default(),None,Some(parent)).unwrap();
        c.execute("UPDATE gateway_keys SET expires_at=?2 WHERE id=?1",params![parent,now_ms()-1]).unwrap();
        let catalog=objects(&c).unwrap();
        assert_eq!(find(&catalog,"keys",child.id).unwrap()["state"],"historical");
        assert!(require_active_object(&c,"key",child.id).is_err());
    }
}
