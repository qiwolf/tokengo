//! System diagnostics report provider cost without borrowing any user's identity or budget.
use rusqlite::Connection;
use serde_json::{Value, json};
use tokengo_server::db::{self, billing::{self, Ledger, Price, RequestPrice, UsageQuery}, identity::{self, Policy}};

const SOURCE: i64 = 7;
fn token_price(c: &Connection, model: &str) -> Price {
    billing::create_price(c, &Price {id:0,channel_id:SOURCE,model:model.into(),input_micro_per_million:1_000_000,output_micro_per_million:2_000_000,max_input_tokens:100,max_output_tokens:100}, "fixture").unwrap()
}
fn request_price(c: &Connection, model: &str) -> RequestPrice {
    billing::create_request_price(c, &RequestPrice{id:0,channel_id:SOURCE,model:model.into(),micro_per_call:77}, "fixture").unwrap()
}
fn auth(subscription: bool) -> Value { json!({"kind":"admin_diagnostic","subscription":subscription,"probe_id":"synthetic-probe"}) }
fn source() -> Value { json!({"source_id":SOURCE,"provider_name":"synthetic supplier"}) }
fn reserve(c: &Connection, attempt: &str, model: &str, request: bool, subscription: bool) -> i64 {
    billing::reserve_diagnostic(c,attempt,SOURCE,model,request,subscription,&auth(subscription),&source()).unwrap()
}
fn row(c: &Connection, id: i64) -> Ledger {
    billing::usage(c,&UsageQuery{limit:Some(200),..Default::default()}).unwrap().into_iter().find(|r|r.id==id).unwrap()
}
fn protected(c: &Connection) -> Vec<Vec<Vec<String>>> {
    ["billing_budgets","billing_allocations"].iter().map(|table| {
        let mut q=c.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();let n=q.column_count();
        let rows=q.query_map([],|r|Ok((0..n).map(|i|format!("{:?}",r.get_ref(i).unwrap())).collect())).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap(); rows
    }).collect()
}
fn fixture() -> Connection {
    let c=db::open_in_memory().unwrap();
    let (key,_)=identity::create_key(&c,1,"real-user-key",&Policy::default(),None,None).unwrap();
    billing::set_budget(&c,"user",1,Some(1000),"fixture").unwrap();billing::set_budget(&c,"key",key.id,Some(1000),"fixture").unwrap();
    let price=token_price(&c,"user-model");let id=billing::reserve(&c,1,key.id,99,"user-attempt",&price,10,10).unwrap();
    billing::settle(&c,id,Some(3),Some(4),"ok").unwrap();c
}
fn system_identity(r: &Ledger) {
    assert_eq!((r.user_id,r.key_id,r.request_id),(0,0,0));assert_eq!(r.reserved_micro,0);assert_eq!(r.debit_micro,Some(0));
    assert_eq!(r.auth_snapshot["kind"],"admin_diagnostic");assert!(r.settled_ms.is_some());
}

#[test]
fn known_tokens_use_immutable_provider_price_but_never_debit_a_user() {
    let c=fixture();let before=protected(&c);let p=token_price(&c,"chat-model");
    let id=reserve(&c,"diag-known","chat-model",false,false);
    let mut next=p.clone();next.input_micro_per_million=9_000_000;billing::create_price(&c,&next,"fixture").unwrap();
    billing::settle_diagnostic(&c,id,Some(3),Some(4),None,"ok").unwrap();let r=row(&c,id);system_identity(&r);
    assert_eq!(r.price_snapshot,Some(p));assert_eq!(r.observed_cost_micro,Some(11));assert_eq!(r.input_tokens,Some(3));assert_eq!(r.output_tokens,Some(4));
    assert_eq!(r.usage_certainty,"reported");assert_eq!(r.outcome.as_deref(),Some("ok"));assert_eq!(protected(&c),before);
}

#[test]
fn unknown_price_or_partial_token_usage_never_becomes_a_fabricated_zero() {
    for (priced,input,output,cost,certainty) in [
        (false,Some(3),Some(4),None,"unpriced"),(false,None,None,None,"unpriced"),
        (true,None,None,None,"unknown"),(true,Some(3),None,None,"unknown"),
        (true,Some(0),Some(0),Some(0),"reported"),
    ] {
        let c=fixture();let before=protected(&c);if priced {token_price(&c,"model");}
        let id=reserve(&c,"diag-token","model",false,false);
        billing::settle_diagnostic(&c,id,input,output,None,"transport_unknown").unwrap();let r=row(&c,id);system_identity(&r);
        assert_eq!(r.observed_cost_micro,cost);assert_eq!(r.usage_certainty,certainty);assert_eq!((r.input_tokens,r.output_tokens),(input,output));assert_eq!(protected(&c),before);
    }
}

#[test]
fn search_request_units_zero_one_and_unknown_have_distinct_cost_semantics() {
    for (priced,units,cost,certainty) in [(true,Some(0),Some(0),"reported"),(true,Some(1),Some(77),"reported"),(true,None,None,"unknown"),(false,Some(1),None,"unpriced"),(false,Some(0),Some(0),"reported")] {
        let c=fixture();let before=protected(&c);let price=priced.then(||request_price(&c,"search"));
        let id=reserve(&c,"diag-search","search",true,false);
        billing::settle_diagnostic(&c,id,None,None,units,"tool_result").unwrap();let r=row(&c,id);system_identity(&r);
        assert_eq!(r.billing_unit,"request");assert_eq!(r.request_units,units);assert_eq!(r.request_price_snapshot,price);assert_eq!(r.observed_cost_micro,cost);assert_eq!(r.usage_certainty,certainty);assert_eq!(protected(&c),before);
    }
}

#[test]
fn subscription_reports_usage_without_claiming_a_zero_dollar_provider_cost() {
    let c=fixture();let before=protected(&c);token_price(&c,"oauth-model");
    let id=reserve(&c,"diag-subscription","oauth-model",false,true);
    billing::settle_diagnostic(&c,id,Some(3),Some(4),None,"ok").unwrap();let r=row(&c,id);system_identity(&r);
    assert!(r.observed_cost_micro.is_none());assert!(r.price_snapshot.is_none());assert_eq!(r.usage_certainty,"subscription");assert_eq!((r.input_tokens,r.output_tokens),(Some(3),Some(4)));assert_eq!(protected(&c),before);
}

#[test]
fn reservation_and_settlement_are_idempotent_without_replacing_known_usage() {
    let c=fixture();token_price(&c,"model");let before=protected(&c);
    let id=reserve(&c,"diag-retry","model",false,false);assert_eq!(reserve(&c,"diag-retry","model",false,false),id);
    assert!(billing::reserve_diagnostic(&c,"diag-retry",SOURCE,"model",false,false,&json!({"kind":"admin_diagnostic","subscription":false,"probe_id":"other"}),&source()).is_err());
    assert!(billing::reserve_diagnostic(&c,"diag-retry",SOURCE,"model",false,false,&auth(false),&json!({"source_id":8})).is_err());
    billing::settle_diagnostic(&c,id,Some(3),Some(4),None,"ok").unwrap();let settled=serde_json::to_value(row(&c,id)).unwrap();
    billing::settle_diagnostic(&c,id,None,None,None,"cancelled_unknown").unwrap();
    assert_eq!(serde_json::to_value(row(&c,id)).unwrap(),settled);assert_eq!(protected(&c),before);
}

#[test]
fn not_sent_is_zero_but_restart_of_pending_sends_remains_unknown() {
    let c=fixture();let before=protected(&c);token_price(&c,"chat");request_price(&c,"search");
    let unsent=reserve(&c,"diag-unsent","chat",false,false);
    billing::settle_diagnostic(&c,unsent,None,None,None,"not_sent").unwrap();let r=row(&c,unsent);system_identity(&r);
    assert_eq!(r.observed_cost_micro,Some(0));assert_eq!(r.usage_certainty,"not_sent");
    let ids=[reserve(&c,"diag-restart-token","chat",false,false),reserve(&c,"diag-restart-search","search",true,false),reserve(&c,"diag-restart-oauth","oauth",false,true)];
    assert_eq!(billing::recover(&c).unwrap(),3);assert_eq!(billing::recover(&c).unwrap(),0);
    for id in ids {let r=row(&c,id);system_identity(&r);assert_eq!(r.outcome.as_deref(),Some("restart_unknown"));assert!(r.observed_cost_micro.is_none());assert!(r.input_tokens.is_none());assert!(r.output_tokens.is_none());assert!(r.request_units.is_none());assert_ne!(r.usage_certainty,"not_sent");}
    assert_eq!(protected(&c),before);
}

#[test]
fn invalid_units_and_cross_unit_settlement_cannot_finish_a_reservation() {
    let c=fixture();let token=reserve(&c,"diag-invalid-token","chat",false,false);let search=reserve(&c,"diag-invalid-search","search",true,false);
    for result in [billing::settle_diagnostic(&c,token,Some(-1),Some(0),None,"ok"),billing::settle_diagnostic(&c,token,None,None,Some(1),"ok"),billing::settle_diagnostic(&c,search,Some(1),None,Some(1),"ok"),billing::settle_diagnostic(&c,search,None,None,Some(2),"ok"),billing::settle_diagnostic(&c,search,None,None,None,"reserved")] {assert!(result.is_err());}
    assert_eq!(row(&c,token).status,"reserved");assert_eq!(row(&c,search).status,"reserved");
}

#[test]
fn reused_attempt_must_match_model_source_and_billing_mode_not_only_snapshots() {
    let c=fixture();reserve(&c,"diag-context","model",false,false);
    for (source_id,model,request,subscription) in [(SOURCE,"different-model",false,false),(SOURCE+1,"model",false,false),(SOURCE,"model",true,false),(SOURCE,"model",false,true)] {
        assert!(billing::reserve_diagnostic(&c,"diag-context",source_id,model,request,subscription,&auth(false),&source()).is_err(),"same attempt cannot change model/source/unit/subscription with unchanged snapshots");
    }
}
