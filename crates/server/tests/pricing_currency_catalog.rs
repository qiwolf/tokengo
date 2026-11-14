//! Control-plane currency and active-model contract, without external requests.
use axum::{body::{Body,to_bytes},http::{Request,StatusCode}};
use serde_json::{json,Value};
use tokengo_server::{app,db,AppState,Config,SharedState};
use tower::ServiceExt;
async fn call(state:&SharedState,path:&str,bearer:Option<&str>,cookie:Option<&str>,body:Option<Value>)->(StatusCode,Value){
 let mut r=Request::builder().uri(path).method(if body.is_some(){"POST"}else{"GET"}).header("content-type","application/json");
 if let Some(token)=bearer {r=r.header("authorization",format!("Bearer {token}"));}if let Some(cookie)=cookie {r=r.header("cookie",format!("tokengo_account_session={cookie}"));}
 let response=app(state.clone()).oneshot(r.body(body.map(|v|Body::from(v.to_string())).unwrap_or_else(Body::empty)).unwrap()).await.unwrap();
 let status=response.status();let bytes=to_bytes(response.into_body(),4<<20).await.unwrap();(status,serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn catalog_global_model_state_and_portal_currency_match_new_editors(){
 let dir=std::env::temp_dir().join(format!("tokengo-price-currency-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
 let cfg=Config{db_path:dir.join("db"),master_key_path:dir.join("key").to_string_lossy().into_owned(),..Default::default()};
 // Synthetic persisted quote makes the HTTP display contract deterministic and offline.
 {let c=db::open(&cfg.db_path).unwrap();db::settings::set(&c,"reporting_fx:USD:CNY",&json!({"rate_nano":7000000000_i64,"date":"2026-11-12","source_url":tokengo_server::reporting_currency::SOURCE_URL,"fetched_at_ms":db::now_ms(),"stale":false,"status":"ok"}).to_string()).unwrap();}
 let state=AppState::init(cfg).unwrap();
 let(admin,channel,cookie,user)={let c=state.conn();let admin=db::sessions::create_session(&c,None).unwrap();let channel=db::channels::create(&c,&db::channels::NewChannel{name:"ASR current".into(),base_url:"http://fixture.invalid/v1".into(),enabled:true,unwrap_envelope:false,max_concurrent:1}).unwrap();db::channels::set_models(&c,channel,&[db::channels::ModelMapping::new("asr-local",Some("FunAudioLLM/SenseVoiceSmall".into()),false)]).unwrap();db::model_routes::set_enabled(&c,"asr-local",false).unwrap();let user=db::identity::create_user(&c,"currency fixture","user",&Default::default()).unwrap().id;let account=db::accounts::create(&c,"currency-fixture","Fixture",user,"synthetic-stored-hash").unwrap().id;let cookie=db::accounts::create_session(&c,&db::accounts::by_id(&c,account).unwrap().unwrap()).unwrap();(admin,channel,cookie,user)};
 let(status,catalog)=call(&state,"/api/channels",Some(&admin),None,None).await;assert_eq!(status,StatusCode::OK);let source=catalog["channels"].as_array().unwrap().iter().find(|s|s["id"]==channel.0).unwrap();assert_eq!(source["models"][0]["enabled"],true);assert_eq!(source["models"][0]["pricing_enabled"],false);
 db::model_routes::set_enabled(&state.conn(),"asr-local",true).unwrap();
 let(status,catalog)=call(&state,"/api/channels",Some(&admin),None,None).await;assert_eq!(status,StatusCode::OK);let source=catalog["channels"].as_array().unwrap().iter().find(|s|s["id"]==channel.0).unwrap();assert_eq!(source["models"][0]["pricing_enabled"],true);
 let(status,price)=call(&state,"/api/unit-pricing",Some(&admin),None,Some(json!({"channel_id":channel.0,"model":"asr-local","unit":"hour","micro_per_unit":200000,"max_quantity_milli":60000,"currency":"CNY"}))).await;assert_eq!(status,StatusCode::OK,"{price}");assert_eq!(price["unit_price"]["currency"],"CNY");
 let(status,_)=call(&state,"/api/unit-pricing",Some(&admin),None,Some(json!({"channel_id":channel.0,"model":"asr-local","unit":"hour","micro_per_unit":200000,"max_quantity_milli":60000,"currency":"USD"}))).await;assert_eq!(status,StatusCode::BAD_REQUEST);
 for(coin,money)in[("CNY",111),("USD",999)] {state.conn().execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,currency,debit_micro) VALUES(?1,?2,0,1,?3,'asr-local',0,0,0,1000,'ok',?1,?4)",rusqlite::params![coin,user,channel.0,money]).unwrap();}
 for(coin,money)in[("CNY",111),("USD",999)] {let(status,v)=call(&state,&format!("/api/account/portal?currency={coin}"),None,Some(&cookie),None).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["usage"]["summary"]["currency"],coin);assert_eq!(v["usage"]["summary"]["known_debit_micro"],money);assert_eq!(v["usage"]["entries"].as_array().unwrap().len(),1);assert_eq!(v["usage"]["entries"][0]["currency"],coin);}
 // Omitted currency is the complete usage view, including original USD records.
 let(status,v)=call(&state,"/api/account/portal",None,Some(&cookie),None).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["usage"]["summary"]["currency"],"all");assert_eq!(v["usage"]["summary"]["ledger_count"],2);assert!(v["usage"]["summary"]["known_debit_micro"].is_null());assert_eq!(v["usage"]["summary"]["amounts_by_currency"].as_array().unwrap().len(),2);assert_eq!(v["usage"]["summary"]["known_debit_cny_micro"],7104);assert_eq!(v["usage"]["summary"]["display_currency"],"CNY");assert_eq!(v["usage"]["summary"]["reporting_fx"]["rate_nano"],7000000000_i64);
 let(status,v)=call(&state,&format!("/api/usage?user_id={user}&limit=1"),Some(&admin),None,None).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["entries"].as_array().unwrap().len(),1);assert!(v.get("summary").is_none());
 let(status,v)=call(&state,&format!("/api/usage/analytics?user_id={user}&limit=1"),Some(&admin),None,None).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["summary"]["ledger_count"],2);assert_eq!(v["summary"]["request_count"],1);
 let(key,secret)=db::identity::create_key(&state.conn(),user,"usage currency key",&Default::default(),None,None).unwrap();state.conn().execute("UPDATE billing_ledger SET key_id=?1 WHERE user_id=?2",rusqlite::params![key.id,user]).unwrap();
 for coin in ["CNY","USD"] {state.conn().execute("INSERT OR IGNORE INTO billing_budgets(scope,scope_id,currency) VALUES('user',?1,?2)",rusqlite::params![user,coin]).unwrap();}
 let(status,v)=call(&state,"/v1/usage?currency=all&user_id=999&key_id=999",Some(&secret),None,None).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["entries"].as_array().unwrap().len(),2);assert!(v.get("summary").is_none());assert!(v["budgets"].as_array().unwrap().iter().any(|b|b["currency"]=="USD"));assert!(v["budgets"].as_array().unwrap().iter().any(|b|b["currency"]=="CNY"));assert!(v["entries"].as_array().unwrap().iter().all(|row|row["user_id"]==user&&row["key_id"]==key.id));
 let(status,v)=call(&state,"/v1/usage/analytics?currency=all&user_id=999&key_id=999",Some(&secret),None,None).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["summary"]["currency"],"all");assert_eq!(v["summary"]["ledger_count"],2);
 state.conn().execute("UPDATE billing_ledger SET billing_unit='request',pricing_unit='hour',quantity_milli=61234,bound_quantity_milli=120000 WHERE currency='CNY'",[]).unwrap();
 let(status,v)=call(&state,"/api/account/portal?currency=CNY",None,Some(&cookie),None).await;assert_eq!(status,StatusCode::OK);assert_eq!(v["usage"]["entries"][0]["quantity_milli"],61234);assert_eq!(v["usage"]["entries"][0]["bound_quantity_milli"],120000);assert_eq!(v["usage"]["entries"][0]["billing_unit"],"hour");
 drop(state);std::fs::remove_dir_all(dir).unwrap();
}
