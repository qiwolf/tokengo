//! Monetary configuration never blocks sends; usage and actual cost remain auditable.
use tokengo_server::db::{self,billing::{self,Price,RequestPrice,UnitPrice,UsageQuery},identity::Policy};
use rusqlite::Connection;
struct Fixture{c:Connection,parent:i64,key:i64,token:Price,unit:UnitPrice,request:RequestPrice}
impl Fixture{
 fn new()->Self{let c=db::open_in_memory().unwrap();let(parent,_)=db::identity::create_key(&c,1,"observation-parent",&Policy::default(),None,None).unwrap();let(key,_)=db::identity::create_key(&c,1,"observation-child",&Policy::default(),None,Some(parent.id)).unwrap();let token=billing::create_price(&c,&Price{currency:"CNY".into(),id:0,channel_id:1,model:"observed-token".into(),input_micro_per_million:1000000,output_micro_per_million:1000000,max_input_tokens:10,max_output_tokens:2},"test").unwrap();let unit=billing::create_unit_price(&c,&UnitPrice{currency:"CNY".into(),id:0,channel_id:1,model:"observed-unit".into(),unit:"second".into(),micro_per_unit:1000,max_quantity_milli:1000},"test").unwrap();let request=billing::create_request_price(&c,&RequestPrice{currency:"CNY".into(),id:0,channel_id:1,model:"observed-request".into(),micro_per_call:750},"test").unwrap();Self{c,parent:parent.id,key:key.id,token,unit,request}}
 fn seed_legacy_controls(&self){for(scope,id)in[("user",1),("key",self.parent),("key",self.key)]{for coin in ["CNY","USD"]{self.c.execute("INSERT INTO billing_budgets(scope,scope_id,currency,limit_micro,spent_micro,reserved_micro,frozen) VALUES(?1,?2,?3,0,123,0,1) ON CONFLICT(scope,scope_id,currency) DO UPDATE SET limit_micro=0,spent_micro=123,reserved_micro=0,frozen=1",rusqlite::params![scope,id,coin]).unwrap();}}}
 fn row(&self,id:i64)->billing::Ledger{billing::usage(&self.c,&UsageQuery::default()).unwrap().into_iter().find(|r|r.id==id).unwrap()}
 fn hold(&self,id:i64,amount:i64){self.c.execute("UPDATE billing_allocations SET reserved_micro=?2 WHERE reservation_id=?1",rusqlite::params![id,amount]).unwrap();self.c.execute("UPDATE billing_ledger SET reserved_micro=?2 WHERE id=?1",rusqlite::params![id,amount]).unwrap();for(scope,id)in[("user",1),("key",self.parent),("key",self.key)]{self.c.execute("UPDATE billing_budgets SET reserved_micro=reserved_micro+?3 WHERE scope=?1 AND scope_id=?2 AND currency='CNY'",rusqlite::params![scope,id,amount]).unwrap();}}
 fn no_new_holds_or_freeze_writes(&self){for(scope,id)in[("user",1),("key",self.parent),("key",self.key)]{for coin in ["CNY","USD"]{let b=billing::budgets_currency(&self.c,Some(scope),Some(id),coin).unwrap().remove(0);assert_eq!(b.reserved_micro,0);assert!(b.frozen,"legacy flag remains historical metadata, not an execution gate");assert_eq!(b.limit_micro,Some(0));}}}
}
#[test]
fn finite_frozen_user_key_ancestors_never_block_token_unit_request_or_unpriced_calls(){
 let f=Fixture::new();f.seed_legacy_controls();
 let a=billing::reserve(&f.c,1,f.key,1,"token",&f.token,20,4).unwrap();assert_eq!(billing::reserve(&f.c,1,f.key,1,"token",&f.token,20,4).unwrap(),a);billing::settle(&f.c,a,Some(100),Some(20),"ok").unwrap();assert_eq!((f.row(a).debit_micro,f.row(a).observed_cost_micro),(Some(120),Some(120)));
 let b=billing::reserve_units(&f.c,1,f.key,2,"unit",&f.unit,2000).unwrap();billing::settle_units(&f.c,b,Some(3000),"ok").unwrap();assert_eq!((f.row(b).debit_micro,f.row(b).observed_cost_micro),(Some(3000),Some(3000)));
 let c=billing::reserve_request(&f.c,1,f.key,3,"request",&f.request).unwrap();billing::settle_request(&f.c,c,Some(1),"ok").unwrap();assert_eq!(f.row(c).debit_micro,Some(750));
 let d=billing::reserve_unpriced(&f.c,1,f.key,4,"unpriced",1,"unknown-price").unwrap();billing::settle(&f.c,d,Some(5),Some(6),"ok").unwrap();assert_eq!(f.row(d).status,"unpriced");assert_eq!(f.row(d).debit_micro,None);assert_eq!(f.row(d).input_tokens,Some(5));
 let e=billing::reserve_units_unpriced(&f.c,1,f.key,5,"unknown-unit",1,"unknown-unit",2000).unwrap();billing::settle_units(&f.c,e,Some(3000),"ok").unwrap();assert_eq!(f.row(e).debit_micro,None);assert_eq!(f.row(e).quantity_milli,Some(3000));
 f.no_new_holds_or_freeze_writes();let budget=billing::budgets(&f.c,Some("key"),Some(f.parent)).unwrap().remove(0);assert_eq!(budget.spent_micro,123+120+3000+750);
}
#[test]
fn actual_cost_above_estimate_is_not_capped_and_does_not_create_new_freeze(){
 let f=Fixture::new();let id=billing::reserve(&f.c,1,f.key,1,"above-estimate",&f.token,1,0).unwrap();billing::settle(&f.c,id,Some(100),Some(20),"ok").unwrap();let row=f.row(id);assert_eq!(row.reserved_micro,1,"original estimate remains an audit datum");assert_eq!(row.debit_micro,Some(120));assert!(row.overrun,"estimate mismatch may remain diagnostic only");for scope in [("user",1),("key",f.key),("key",f.parent)]{let b=billing::budgets(&f.c,Some(scope.0),Some(scope.1)).unwrap().remove(0);assert!(!b.frozen);assert_eq!(b.reserved_micro,0);}assert!(billing::reserve(&f.c,1,f.key,2,"after-estimate",&f.token,1000,1000).is_ok());
}
#[test]
fn old_pending_cash_holds_release_exactly_and_recovery_preserves_unknown_cost(){
 let f=Fixture::new();f.seed_legacy_controls();let t=billing::reserve(&f.c,1,f.key,1,"legacy-token",&f.token,1,0).unwrap();let u=billing::reserve_units(&f.c,1,f.key,2,"legacy-unit",&f.unit,1000).unwrap();let r=billing::reserve_request(&f.c,1,f.key,3,"legacy-request",&f.request).unwrap();for(id,hold)in[(t,10),(u,20),(r,30)]{f.hold(id,hold);}
 assert_eq!(billing::recover(&f.c).unwrap(),3);assert_eq!(billing::recover(&f.c).unwrap(),0);for id in [t,u,r]{let row=f.row(id);assert_eq!(row.status,"restart_unknown");assert_eq!(row.debit_micro,None);assert_eq!(row.observed_cost_micro,None);assert_eq!(row.usage_certainty,"unknown");}
 f.no_new_holds_or_freeze_writes();assert_eq!(billing::budgets(&f.c,Some("user"),Some(1)).unwrap()[0].spent_micro,123);
 let snapshot=serde_json::to_value(f.row(t)).unwrap();billing::settle(&f.c,t,Some(10),Some(10),"late").unwrap();assert_eq!(serde_json::to_value(f.row(t)).unwrap(),snapshot,"a terminal historical row is immutable");
}
#[test]
fn old_pending_holds_settle_full_known_cost_and_leave_no_residual_cash_hold(){
 let f=Fixture::new();f.seed_legacy_controls();let t=billing::reserve(&f.c,1,f.key,1,"old-known-token",&f.token,1,0).unwrap();let u=billing::reserve_units(&f.c,1,f.key,2,"old-known-unit",&f.unit,1000).unwrap();let r=billing::reserve_request(&f.c,1,f.key,3,"old-known-request",&f.request).unwrap();for(id,hold)in[(t,1),(u,1),(r,1)]{f.hold(id,hold);}
 billing::settle(&f.c,t,Some(100),Some(20),"ok").unwrap();billing::settle_units(&f.c,u,Some(3000),"ok").unwrap();billing::settle_request(&f.c,r,Some(1),"ok").unwrap();assert_eq!(f.row(t).debit_micro,Some(120));assert_eq!(f.row(u).debit_micro,Some(3000));assert_eq!(f.row(r).debit_micro,Some(750));f.no_new_holds_or_freeze_writes();
}
#[test]
fn retired_budget_mutation_api_is_disabled_without_rewriting_legacy_records(){
 let f=Fixture::new();f.seed_legacy_controls();let before=billing::budgets(&f.c,None,None).unwrap().into_iter().map(|b|serde_json::to_value(b).unwrap()).collect::<Vec<_>>();let error=db::billing_management::update_budget_currency(&f.c,"user",1,None,Some(false),"test","CNY").unwrap_err();assert!(error.to_string().contains("budget_enforcement_disabled"));let after=billing::budgets(&f.c,None,None).unwrap().into_iter().map(|b|serde_json::to_value(b).unwrap()).collect::<Vec<_>>();assert_eq!(after,before);
}
