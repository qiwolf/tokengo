// Included by billing.rs: generic measured-unit prices share its durable budget helpers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UnitPrice {
    pub id:i64,pub channel_id:i64,pub model:String,
    #[serde(default="legacy_currency")] pub currency:String,
    pub unit:String,pub micro_per_unit:i64,
    /// Audio milliseconds for second/minute/hour; 1000 ticks per other scalar unit.
    pub max_quantity_milli:i64,
}
fn unit_divisor(unit:&str)->Result<i64> {match unit {
    "second"=>Ok(1000),"minute"=>Ok(60000),"hour"=>Ok(3600000),
    "character"|"byte"|"image"|"video"=>Ok(1000),_=>Err(bad("unsupported pricing unit"))
}}
pub fn unit_cost(p:&UnitPrice,quantity_milli:i64)->Result<i64> {
    if quantity_milli<0||p.micro_per_unit<0 {return Err(bad("negative unit billing quantity"));}
    let divisor=i128::from(unit_divisor(&p.unit)?);
    let value=i128::from(quantity_milli).checked_mul(i128::from(p.micro_per_unit)).and_then(|n|n.checked_add(divisor-1)).ok_or_else(||bad("unit cost overflow"))?/divisor;
    i64::try_from(value).map_err(|_|bad("unit cost overflow"))
}
fn unit_price_row(r:&rusqlite::Row<'_>)->rusqlite::Result<UnitPrice> {Ok(UnitPrice{id:r.get(0)?,channel_id:r.get(1)?,model:r.get(2)?,currency:r.get(3)?,unit:r.get(4)?,micro_per_unit:r.get(5)?,max_quantity_milli:r.get(6)?})}
const UNIT_PRICE_SELECT:&str="SELECT id,channel_id,model,currency,unit,micro_per_unit,max_quantity_milli FROM billing_unit_prices";
pub fn unit_price(c:&Connection,model:&str,channel_id:i64)->Result<Option<UnitPrice>> {
    Ok(c.query_row(&format!("{UNIT_PRICE_SELECT} WHERE channel_id=?1 AND model=?2 AND currency='CNY' ORDER BY id DESC LIMIT 1"),params![channel_id,tokengo_shared::normalize_model_name(model)],unit_price_row).optional()?)
}
pub fn list_unit_prices(c:&Connection)->Result<Vec<UnitPrice>> {
    Ok(c.prepare(&format!("{UNIT_PRICE_SELECT} ORDER BY id DESC LIMIT 1000"))?.query_map([],unit_price_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
}
pub fn create_unit_price(c:&Connection,p:&UnitPrice,actor:&str)->Result<UnitPrice> {
    super::channels::require_llm_transport(c,tokengo_shared::ChannelId(p.channel_id))?;
    if p.currency!="CNY"||p.channel_id<=0||p.model.trim().is_empty()||p.model.len()>256||p.micro_per_unit<0||p.max_quantity_milli<0 {return Err(bad("invalid CNY unit price"));}
    unit_cost(p,p.max_quantity_milli)?;
    let c=tx(c)?;
    c.execute("INSERT INTO billing_unit_prices(channel_id,model,currency,unit,micro_per_unit,max_quantity_milli,created_ms) VALUES(?1,?2,'CNY',?3,?4,?5,?6)",params![p.channel_id,tokengo_shared::normalize_model_name(&p.model),p.unit,p.micro_per_unit,p.max_quantity_milli,now_ms()])?;
    let out=UnitPrice{id:c.last_insert_rowid(),model:tokengo_shared::normalize_model_name(&p.model),..p.clone()};
    audit(&c,"unit_price_created",actor,&out.id.to_string(),&serde_json::to_string(&out).map_err(|_|bad("unit price serialization"))?)?;
    c.commit()?;Ok(out)
}
fn hold_unit_scopes(c:&Connection,user:i64,key:i64,amount:i64,priced:bool)->Result<Vec<(&'static str,i64)>> {
    let scopes=allocation_scopes(c,user,key)?;
    for (scope,id) in &scopes {
        reject_frozen_scope(c,scope,*id)?;
        c.execute("INSERT OR IGNORE INTO billing_budgets(scope,scope_id,currency) VALUES(?1,?2,'CNY')",params![scope,id])?;
        let budget=c.query_row("SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen,currency FROM billing_budgets WHERE scope=?1 AND scope_id=?2 AND currency='CNY'",params![scope,id],budget_row)?;
        if !priced&&budget.limit_micro.is_some() {return Err(bad("config_required: budgeted attempts require explicit pricing"));}
        let held=budget.reserved_micro.checked_add(amount).ok_or_else(||bad("unit reservation overflow"))?;
        let total=budget.spent_micro.checked_add(held).ok_or_else(||bad("unit budget overflow"))?;
        if budget.limit_micro.is_some_and(|limit|total>limit){return Err(bad("billing budget exhausted"));}
        c.execute("UPDATE billing_budgets SET reserved_micro=?3 WHERE scope=?1 AND scope_id=?2 AND currency='CNY'",params![scope,id,held])?;
    }
    Ok(scopes)
}
pub fn reserve_units(c:&Connection,user:i64,key:i64,request:i64,attempt:&str,p:&UnitPrice,bound_quantity_milli:i64)->Result<i64> {
    if p.currency!="CNY"||bound_quantity_milli<0||bound_quantity_milli>p.max_quantity_milli {return Err(bad("invalid unit reservation bound or currency"));}
    reserve_unit_inner(c,user,key,request,attempt,p.channel_id,&p.model,&p.unit,bound_quantity_milli,Some(p))
}
pub fn reserve_units_unpriced(c:&Connection,user:i64,key:i64,request:i64,attempt:&str,channel:i64,model:&str,bound_quantity_milli:i64)->Result<i64> {
    reserve_unit_inner(c,user,key,request,attempt,channel,model,"second",bound_quantity_milli,None)
}
fn reserve_unit_inner(c:&Connection,user:i64,key:i64,request:i64,attempt:&str,channel:i64,model:&str,unit:&str,bound:i64,price:Option<&UnitPrice>)->Result<i64> {
    if user<=0||key<=0||channel<=0||attempt.is_empty()||attempt.len()>256||model.trim().is_empty()||model.len()>256||bound<0 {return Err(bad("invalid unit reservation identity"));}
    unit_divisor(unit)?;
    let amount=price.map(|p|unit_cost(p,bound)).transpose()?.unwrap_or(0);
    let c=tx(c)?;let model=tokengo_shared::normalize_model_name(model);
    if let Some(row)=c.query_row(&format!("{LEDGER_SELECT} WHERE attempt_id=?1"),[attempt],ledger_row).optional()? {
        if row.user_id!=user||row.key_id!=key||row.request_id!=request||row.channel_id!=channel||row.model!=model||row.billing_unit!=unit||row.bound_quantity_milli!=Some(bound)||row.unit_price_snapshot.as_ref()!=price||row.currency!="CNY" {return Err(bad("attempt id conflicts with existing unit reservation"));}
        c.commit()?;return Ok(row.id);
    }
    if let Some(price)=price {
        let stored=c.query_row(&format!("{UNIT_PRICE_SELECT} WHERE id=?1"),[price.id],unit_price_row).optional()?.ok_or_else(||bad("unknown unit price version"))?;
        if stored!=*price {return Err(bad("unit price snapshot mismatch"));}
    }
    let scopes=hold_unit_scopes(&c,user,key,amount,price.is_some())?;
    let auth=super::identity::context(&c,key)?.ok_or_else(||bad("inactive gateway key"))?;
    let raw=price.map(serde_json::to_string).transpose().map_err(|_|bad("unit price serialization"))?;
    // The legacy CHECK retains token/request as storage discriminator; pricing_unit
    // is the authoritative measured unit and is serialized as billing_unit.
    c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,billing_unit,currency,pricing_unit,unit_price_id,unit_price_snapshot,bound_quantity_milli,auth_snapshot) VALUES(?1,?2,?3,?4,?5,?6,0,0,?7,?8,'request','CNY',?9,?10,?11,?12,?13)",params![attempt,user,key,request,channel,model,amount,now_ms(),unit,price.map(|p|p.id),raw,bound,serde_json::to_string(&auth).map_err(|_|bad("unit auth serialization"))?])?;
    let id=c.last_insert_rowid();
    for (scope,scope_id) in scopes {c.execute("INSERT INTO billing_allocations(reservation_id,scope,scope_id,reserved_micro) VALUES(?1,?2,?3,?4)",params![id,scope,scope_id,amount])?;}
    c.commit()?;Ok(id)
}
pub fn settle_units(c:&Connection,id:i64,quantity_milli:Option<i64>,status:&str)->Result<()> {finish_units(c,id,quantity_milli,status,false)}
fn finish_units(c:&Connection,id:i64,quantity:Option<i64>,status:&str,release:bool)->Result<()> {
    if quantity.is_some_and(|n|n<0)||status.is_empty()||status=="reserved"||status.len()>64 {return Err(bad("invalid unit settlement"));}
    let release=release||(status=="not_sent"&&quantity==Some(0));
    let c=tx(c)?;let row=c.query_row(&format!("{LEDGER_SELECT} WHERE id=?1"),[id],ledger_row).optional()?.ok_or_else(||bad("unknown unit reservation"))?;
    if !matches!(row.billing_unit.as_str(),"second"|"minute"|"hour"|"character"|"byte"|"image"|"video") {return Err(bad("settlement billing unit mismatch"));}
    if row.status!="reserved" {c.commit()?;return Ok(());}
    let observed=if release {Some(0)}else{match(&row.unit_price_snapshot,quantity){(Some(p),Some(n))=>Some(unit_cost(p,n)?),_=>None}};
    let overrun=observed.is_some_and(|n|n>row.reserved_micro);
    let debit=if release {0}else{observed.unwrap_or(row.reserved_micro).min(row.reserved_micro)};
    let scopes=c.prepare("SELECT scope,scope_id FROM billing_allocations WHERE reservation_id=?1")?.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (scope,scope_id) in scopes {
        let budget=c.query_row("SELECT scope,scope_id,limit_micro,spent_micro,reserved_micro,frozen,currency FROM billing_budgets WHERE scope=?1 AND scope_id=?2 AND currency=?3",params![scope,scope_id,row.currency],budget_row)?;
        let held=budget.reserved_micro.checked_sub(row.reserved_micro).filter(|n|*n>=0).ok_or_else(||bad("inconsistent unit hold"))?;
        let spent=budget.spent_micro.checked_add(debit).ok_or_else(||bad("unit spend overflow"))?;
        c.execute("UPDATE billing_budgets SET reserved_micro=?3,spent_micro=?4,frozen=MAX(frozen,?5) WHERE scope=?1 AND scope_id=?2 AND currency=?6",params![scope,scope_id,held,spent,overrun,row.currency])?;
    }
    let unpriced=row.unit_price_snapshot.is_none()&&!release;
    let certainty=if release {"not_sent"}else if unpriced {"unpriced"}else if observed.is_some(){"measured"}else{"estimated"};
    c.execute("UPDATE billing_ledger SET quantity_milli=?2,debit_micro=?3,observed_cost_micro=?4,usage_certainty=?5,status=?6,outcome=?7,overrun=?8,settled_ms=?9 WHERE id=?1",params![id,quantity,if unpriced{None}else{Some(debit)},observed,certainty,if unpriced{"unpriced"}else{status},status,overrun,now_ms()])?;
    c.commit()?;Ok(())
}

#[cfg(test)]
mod currency_unit_tests {
    use super::*;
    fn fixture()->(Connection,i64,UnitPrice) {
        let c=crate::db::open_in_memory().unwrap();
        let(key,_)=crate::db::identity::create_key(&c,1,"ASR",&Default::default(),None,None).unwrap();
        let price=create_unit_price(&c,&UnitPrice{id:0,channel_id:1,model:"ASR-Test".into(),currency:"CNY".into(),unit:"minute".into(),micro_per_unit:120000,max_quantity_milli:60000},"test").unwrap();
        (c,key.id,price)
    }
    #[test]
    fn audio_milliseconds_charge_exact_cny_and_ancestor_hold_is_idempotent() {
        let(c,key,p)=fixture();
        assert_eq!(unit_cost(&p,2500).unwrap(),5000);
        assert_eq!(unit_cost(&p,1).unwrap(),2);
        set_budget(&c,"user",1,Some(10000),"test").unwrap();
        let id=reserve_units(&c,1,key,1,"asr",&p,2500).unwrap();
        assert_eq!(reserve_units(&c,1,key,1,"asr",&p,2500).unwrap(),id);
        assert!(reserve_units(&c,1,key,1,"asr",&p,2501).is_err());
        assert_eq!(budgets(&c,Some("user"),Some(1)).unwrap()[0].reserved_micro,5000);
        settle_units(&c,id,Some(2000),"ok").unwrap();settle_units(&c,id,Some(2500),"ok").unwrap();
        let row=usage(&c,&UsageQuery::default()).unwrap().remove(0);
        assert_eq!((row.currency.as_str(),row.billing_unit.as_str(),row.quantity_milli,row.debit_micro,row.observed_cost_micro,row.usage_certainty.as_str()),("CNY","minute",Some(2000),Some(4000),Some(4000),"measured"));
        let summary=usage_summary(&c,&UsageQuery::default()).unwrap();assert_eq!((summary.unknown_input_entries,summary.unknown_output_entries,summary.total_tokens,summary.known_debit_micro),(0,0,0,4000));
        assert_eq!(budgets(&c,Some("user"),Some(1)).unwrap()[0].spent_micro,4000);
        assert!(reserve_units(&c,1,key,2,"too-large",&p,60001).is_err());
    }
    #[test]
    fn unknown_measured_usage_keeps_hold_and_not_sent_is_known_zero() {
        let(c,key,p)=fixture();
        let id=reserve_units(&c,1,key,1,"unknown",&p,2500).unwrap();
        recover(&c).unwrap();let row=usage(&c,&UsageQuery::default()).unwrap().remove(0);
        assert_eq!((row.quantity_milli,row.debit_micro,row.observed_cost_micro,row.usage_certainty.as_str()),(None,Some(5000),None,"estimated"));
        assert_eq!(row.id,id);
        let id=reserve_units_unpriced(&c,1,key,2,"not-sent",1,"unknown",2500).unwrap();
        settle_units(&c,id,Some(0),"not_sent").unwrap();
        let row=usage(&c,&UsageQuery::default()).unwrap().remove(0);
        assert_eq!((row.debit_micro,row.observed_cost_micro,row.usage_certainty.as_str()),(Some(0),Some(0),"not_sent"));
        set_budget(&c,"key",key,Some(100),"test").unwrap();
        assert!(reserve_units_unpriced(&c,1,key,3,"capped-unknown",1,"unknown",2500).is_err());
    }
    #[test]
    fn unit_overrun_keeps_observed_cost_caps_debit_and_freezes() {
        let(c,key,p)=fixture();let id=reserve_units(&c,1,key,1,"overrun",&p,1000).unwrap();
        settle_units(&c,id,Some(2500),"ok").unwrap();
        let row=usage(&c,&UsageQuery::default()).unwrap().remove(0);
        assert_eq!((row.debit_micro,row.observed_cost_micro,row.overrun),(Some(2000),Some(5000),true));
        assert!(reserve_units(&c,1,key,2,"blocked",&p,1000).is_err());
    }
    #[test]
    fn scalar_units_and_currency_queries_do_not_mix_money() {
        let(c,key,mut p)=fixture();p.unit="byte".into();p.micro_per_unit=1000;
        assert_eq!(unit_cost(&p,3000).unwrap(),3000);
        p.unit="hour".into();p.micro_per_unit=3600000;assert_eq!(unit_cost(&p,1500).unwrap(),1500);
        assert!(unit_cost(&p,-1).is_err());p.unit="invalid".into();assert!(unit_cost(&p,1).is_err());
        c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,currency,debit_micro,status) VALUES('old-dollar',1,?1,1,1,'old',0,0,0,1000,'USD',999999,'ok')",[key]).unwrap();
        let q=UsageQuery{currency:Some("USD".into()),..Default::default()};
        assert_eq!(usage_summary(&c,&q).unwrap().known_debit_micro,999999);
        assert_eq!(usage_summary(&c,&UsageQuery::default()).unwrap().known_debit_micro,0);
        assert!(usage(&c,&UsageQuery{currency:Some("all".into()),..Default::default()}).is_err());
        c.execute("INSERT INTO billing_budgets(scope,scope_id,currency,limit_micro,frozen) VALUES('user',1,'USD',NULL,1)",[]).unwrap();
        assert!(reserve_units_unpriced(&c,1,key,2,"frozen-dollar",1,"x",1).is_err());
        crate::db::billing_management::update_budget_currency(&c,"user",1,None,Some(false),"test","USD").unwrap();
        reserve_units_unpriced(&c,1,key,2,"unfrozen",1,"x",1).unwrap();
    }
    #[test]
    fn migration_preserves_usd_snapshots_pending_holds_and_is_idempotent() {
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL DEFAULT(datetime('now')))").unwrap();
        for(v,sql)in crate::db::schema::MIGRATIONS.iter().filter(|(v,_)|*v<=46){c.execute_batch(sql).unwrap();c.execute("INSERT INTO schema_migrations(version) VALUES(?1)",[v]).unwrap();}
        let(key,_)=crate::db::identity::create_key(&c,1,"old",&Default::default(),None,None).unwrap();
        c.execute_batch("INSERT INTO billing_prices(id,channel_id,model,input_micro_per_million,output_micro_per_million,max_input_tokens,max_output_tokens,created_ms) VALUES(1,1,'legacy',1000000,0,100,0,1);
          INSERT INTO billing_budgets(scope,scope_id,limit_micro,spent_micro,reserved_micro) VALUES('user',1,100,5,20);").unwrap();
        let raw=r#"{"id":1,"channel_id":1,"model":"legacy","input_micro_per_million":1000000,"output_micro_per_million":0,"max_input_tokens":100,"max_output_tokens":0}"#;
        c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,price_id,price_snapshot,input_bound,output_bound,reserved_micro,created_ms) VALUES('pending-dollar',1,?1,1,1,'legacy',1,?2,20,0,20,1)",params![key.id,raw]).unwrap();
        let id=c.last_insert_rowid();c.execute("INSERT INTO billing_allocations(reservation_id,scope,scope_id,reserved_micro) VALUES(?1,'user',1,20)",[id]).unwrap();
        crate::db::migrate(&c).unwrap();crate::db::migrate(&c).unwrap();
        assert_eq!(list_prices(&c).unwrap()[0].currency,"USD");assert!(price(&c,"legacy",1).unwrap().is_none());
        settle(&c,id,Some(3),Some(0),"ok").unwrap();
        let old=budgets_currency(&c,Some("user"),Some(1),"USD").unwrap().remove(0);
        assert_eq!((old.spent_micro,old.reserved_micro,old.limit_micro),(8,0,Some(100)));
        set_budget(&c,"user",1,Some(123),"test").unwrap();let new=budgets(&c,Some("user"),Some(1)).unwrap().remove(0);
        assert_eq!((new.spent_micro,new.limit_micro),(0,Some(123)));
        let row=usage(&c,&UsageQuery{currency:Some("USD".into()),..Default::default()}).unwrap().remove(0);
        assert_eq!(row.currency,"USD");assert_eq!(row.price_snapshot.unwrap().currency,"USD");assert_eq!(row.debit_micro,Some(3));
        assert_eq!(c.query_row("SELECT price_snapshot FROM billing_ledger WHERE id=?1",[id],|r|r.get::<_,String>(0)).unwrap(),raw);
        let old_price:Price=serde_json::from_str(raw).unwrap();
        assert_eq!(reserve(&c,1,key.id,1,"pending-dollar",&old_price,20,0).unwrap(),id);
        assert!(reserve(&c,1,key.id,2,"new-dollar-forbidden",&old_price,20,0).is_err());
    }
}
