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
pub fn objects(c: &Connection) -> Result<Value> {
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
    // Deleted sources remain selectable by the name captured with their last attempt.
    let deleted=c.prepare("SELECT l.channel_id,COALESCE(json_extract(l.source_snapshot,'$.channel_name'),json_extract(l.source_snapshot,'$.source_name'),'已删除渠道') FROM billing_ledger l JOIN (SELECT channel_id,MAX(id) AS latest_id FROM billing_ledger WHERE channel_id NOT IN (SELECT id FROM channels) GROUP BY channel_id) history ON history.latest_id=l.id ORDER BY l.channel_id")?.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    channel_objects.extend(deleted.into_iter().map(|(id,name)|object(id,name,false,"已删除")));
    Ok(json!({"users":user_objects,"keys":key_objects,"channels":channel_objects}))
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
    let catalog=objects(c)?;
    // Filter before applying the list limit: historical rows cannot hide active records.
    let mut statement=c.prepare("SELECT scope,scope_id FROM billing_budgets WHERE (?1 IS NULL OR scope=?1) AND (?2 IS NULL OR scope_id=?2) ORDER BY scope,scope_id")?;
    if scope.is_some_and(|v|v!="user"&&v!="key") {return Err(bad("invalid budget scope"));}
    let identities=statement.query_map(params![scope,id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut result=Vec::new();
    for (scope,id) in identities {
        let obj=find(&catalog,if scope=="user"{"users"}else{"keys"},id);
        let object_state=obj.map(|v|v["state"].as_str().unwrap_or("historical")).unwrap_or("historical");
        if state!="all"&&state!=object_state {continue;}
        if let Some(row)=billing::budgets(c,Some(&scope),Some(id))?.into_iter().next(){
            let mut row=serde_json::to_value(row).map_err(|_|bad("cannot encode budget"))?;
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
