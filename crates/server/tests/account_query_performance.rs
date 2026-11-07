//! Owner-heavy history gates. Run on build-test with --release --ignored --nocapture.
//! Synthetic data: no password KDF, credentials, network upstream, or personal data.
use axum::{body::Body,http::{Request,StatusCode}};
use serde_json::{json,Value};
use std::{path::PathBuf,time::Instant};
use tokengo_server::{app,db,AppState,Config,SharedState};
use tower::ServiceExt;

struct Fixture {state:SharedState,dir:PathBuf,session:String,admin_session:String,owner:i64,rows:i64}
impl Drop for Fixture {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.dir);}}
impl Fixture {
 fn new()->Self {
  let rows=std::env::var("TOKENGO_ACCOUNT_PERF_ROWS").ok().and_then(|n|n.parse().ok()).unwrap_or(500_000i64).clamp(500_000,1_000_000);
  let dir=std::env::temp_dir().join(format!("tokengo-account-perf-{}",uuid::Uuid::new_v4()));
  std::fs::create_dir_all(&dir).unwrap();
  let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("key").to_string_lossy().into_owned(),..Default::default()}).unwrap();
  let (session,admin_session,owner)={
   let c=state.conn();
   let owner=db::identity::create_user(&c,"Synthetic owner","user",&Default::default()).unwrap().id;
   let foreign=db::identity::create_user(&c,"Foreign owner","user",&Default::default()).unwrap().id;
   let account=db::accounts::create(&c,"account-perf","Synthetic owner",owner,"synthetic-stored-hash").unwrap().id;
   let session=db::accounts::create_session(&c,&db::accounts::by_id(&c,account).unwrap().unwrap()).unwrap();
   // No configured root/hash exists in this synthetic database. Preserve the
   // supported legacy (None,None) session proof without password hashing.
   let admin_session=db::sessions::create_session(&c,None).unwrap();
   c.execute_batch("BEGIN; INSERT INTO channels(id,name,base_url) VALUES(1,'synthetic','http://fixture.invalid');").unwrap();
   let now=db::now_ms();
   // The same owner owns every large-history row. Uniform seven-day timestamps
   // exercise both bounded windows and an owner's entire retained history.
   c.execute("WITH RECURSIVE n(i) AS(VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<?1)
    INSERT INTO requests(id,user_id,ts_ms,endpoint,model,served_model,status,channel_id,input_tokens,output_tokens,input_tokens_known,output_tokens_known,usage_source,error,client_ip)
    SELECT i,?2,?3-i*(604800000/?1),'/v1/chat/completions','owner-model','owner-model','ok',1,10,2,1,1,'upstream',printf('%0512d',0),'synthetic-client' FROM n",rusqlite::params![rows,owner,now]).unwrap();
   // Distinct request count differs from ledger count; NULL usage/price/debit
   // deliberately exercises the exact existing lifetime accounting contract.
   c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,debit_micro,input_tokens,output_tokens,price_id,request_price_id,status,created_ms,source_snapshot)
    SELECT 'owner-'||id,?1,0,(id+1)/2,1,'owner-model',10,2,0,CASE WHEN id%13=0 THEN NULL ELSE 3 END,CASE WHEN id%7=0 THEN NULL ELSE 10 END,CASE WHEN id%11=0 THEN NULL ELSE 2 END,CASE WHEN id%3=0 THEN 1 END,CASE WHEN id%5=0 THEN 1 END,'settled',ts_ms,json_object('channel_name','synthetic','padding',printf('%0512d',0)) FROM requests WHERE user_id=?1",[owner]).unwrap();
   c.execute("WITH RECURSIVE n(i) AS(VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<1000)
    INSERT INTO requests(id,user_id,ts_ms,endpoint,model,served_model,status,input_tokens,output_tokens)
    SELECT ?1+i,?2,?3,'/v1/chat/completions','foreign-only','foreign-only','ok',999999,999999 FROM n",rusqlite::params![rows,foreign,now]).unwrap();
   c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,debit_micro,input_tokens,output_tokens,status,created_ms,source_snapshot)
    SELECT 'foreign-'||id,?1,0,id,1,'foreign-only',1,1,0,999999,999999,999999,'settled',ts_ms,'{}' FROM requests WHERE user_id=?1",[foreign]).unwrap();
   // Admin seven-day statistics must aggregate attempts for the entire owner
   // history, rather than relying on a fixture with only a small recent tail.
   c.execute_batch("INSERT INTO attempts(request_id,ordinal,channel_id,outcome,duration_ms,first_byte_ms,model_name)
    SELECT id,0,1,'success',100,25,served_model FROM requests;").unwrap();
   c.execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
   let pages:i64=c.query_row("PRAGMA page_count",[],|r|r.get(0)).unwrap();
   let size:i64=c.query_row("PRAGMA page_size",[],|r|r.get(0)).unwrap();
   assert!(pages*size<3*1024*1024*1024i64,"fixture must remain below 3GiB");
   eprintln!("ACCOUNT_FIXTURE owner_rows={rows} foreign_rows=1000 attempts={} bytes={}",rows+1000,pages*size);
   (session,admin_session,owner)
  };
  Self{state,dir,session,admin_session,owner,rows}
 }
 async fn get(&self,path:&str)->(StatusCode,Value,u128) {
  let request=Request::builder().uri(path).header("cookie",format!("tokengo_account_session={}",self.session)).body(Body::empty()).unwrap();
  let start=Instant::now();
  let response=app(self.state.clone()).oneshot(request).await.unwrap();
  let status=response.status();
  let bytes=axum::body::to_bytes(response.into_body(),4<<20).await.unwrap();
  (status,serde_json::from_slice(&bytes).unwrap(),start.elapsed().as_micros())
 }
 async fn get_admin(&self,path:&str)->(StatusCode,Value,u128) {
  let request=Request::builder().uri(path).header("authorization",format!("Bearer {}",self.admin_session)).body(Body::empty()).unwrap();
  let start=Instant::now();
  let response=app(self.state.clone()).oneshot(request).await.unwrap();
  let status=response.status();
  let bytes=axum::body::to_bytes(response.into_body(),4<<20).await.unwrap();
  (status,serde_json::from_slice(&bytes).unwrap(),start.elapsed().as_micros())
 }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="500k to 1m owner rows with wide requests and ledger; release on build-test"]
async fn owner_heavy_portal_and_overview_preserve_accounting_and_latency() {
 let f=Fixture::new();let n=f.rows;
 let expected=json!({"request_count":(n+1)/2,"ledger_count":n,"input_tokens":(n-n/7)*10,"output_tokens":(n-n/11)*2,"unknown_input_entries":n/7,"unknown_output_entries":n/11,"unpriced_entries":n-n/3-n/5+n/15,"known_debit_micro":(n-n/13)*3});
 let observe=std::env::var_os("TOKENGO_PERF_OBSERVE_ONLY").is_some();let mut failures=Vec::new();
 for endpoint in ["/api/account/portal?limit=50","/api/account/overview/stats?hours=1","/api/account/overview/stats?hours=24","/api/account/overview/stats?hours=168","/api/account/overview/stats?hours=2160","/api/account/overview/logs?limit=50"] {
  let mut samples=Vec::new();
  for _ in 0..3 {
   let(status,body,us)=f.get(endpoint).await;assert_eq!(status,StatusCode::OK,"{endpoint}: {body}");
   if endpoint.contains("/portal") {
    assert_eq!(body["usage"]["summary"],expected);
    assert_eq!(body["usage"]["entries"].as_array().unwrap().len(),50);
    assert!(body["usage"]["entries"].as_array().unwrap().iter().all(|r|r["model"]=="owner-model"));
   } else if endpoint.contains("/stats") {
    assert!(body["stats"]["total"].as_i64().unwrap()>0);
    assert!(body["stats"]["by_model"].as_array().unwrap().iter().all(|r|r["model"]=="owner-model"));
    if endpoint.ends_with("hours=2160") {assert_eq!(body["stats"]["total"],n);}
   } else {assert!(body["logs"].as_array().unwrap().iter().all(|r|r["model"]=="owner-model"));}
   samples.push(us);
  }
  samples.sort_unstable();let limit=if endpoint.contains("/logs") {300_000} else {1_000_000};
  eprintln!("ACCOUNT_PERF endpoint={endpoint} rows={n} n=3 p50_us={} max_us={} limit_us={limit}",samples[1],samples[2]);
  if samples[2]>limit {failures.push(format!("{endpoint}: {}us > {limit}us",samples[2]));}
 }
 // Admin hours accepts 1..=24*90, so 2160 is the actual maximum window.
 for hours in [1,24,168,2160] {
  let endpoint=format!("/api/stats?hours={hours}");let mut samples=Vec::new();
  for _ in 0..3 {
   let(status,body,us)=f.get_admin(&endpoint).await;assert_eq!(status,StatusCode::OK,"{endpoint}: {body}");
   assert_eq!(body["window_hours"],hours);
   let stats=&body["stats"];let total=stats["total"].as_i64().unwrap();
   assert!(total>1000&&total<=n+1000,"{endpoint}: {stats}");
   assert_eq!(stats["ok"],total);assert_eq!(stats["failed"],0);assert_eq!(stats["canceled"],0);assert_eq!(stats["client_errors"],0);
   let channels=stats["by_channel"].as_array().unwrap();assert_eq!(channels.len(),1);
   assert_eq!(channels[0]["attempts"],total);assert_eq!(channels[0]["successes"],total);assert_eq!(channels[0]["channel_faults"],0);
   if hours>=168 {
    assert_eq!(total,n+1000,"seven-day window must actually contain all owner+foreign history");
    assert_eq!(stats["input_tokens"],n*10+999999*1000);
    assert_eq!(stats["output_tokens"],n*2+999999*1000);
    assert_eq!(stats["usage"]["unknown_requests"],1000);
    assert_eq!(channels[0]["input_tokens"],n*10);
   }
   samples.push(us);
  }
  samples.sort_unstable();let limit=1_000_000;
  eprintln!("ADMIN_ACCOUNT_PERF endpoint={endpoint} owner_rows={n} attempts={} n=3 p50_us={} max_us={} limit_us={limit}",n+1000,samples[1],samples[2]);
  if samples[2]>limit {failures.push(format!("admin {endpoint}: {}us > {limit}us",samples[2]));}
 }
 let foreign_path=format!("/api/account/overview/requests/{}",n+1);
 assert_eq!(f.get(&foreign_path).await.0,StatusCode::NOT_FOUND);
 // Revocation after successful history reads must take effect on the next read.
 f.state.conn().execute("UPDATE users SET enabled=0 WHERE id=?1",[f.owner]).unwrap();
 for path in ["/api/account/portal","/api/account/overview/stats?hours=24"] {
  assert_eq!(f.get(path).await.0,StatusCode::UNAUTHORIZED);
 }
 eprintln!("ACCOUNT_PERF_GATE observe_only={observe} violations={failures:?}");
 assert!(observe||failures.is_empty(),"owner history gate failed: {failures:?}");
}
