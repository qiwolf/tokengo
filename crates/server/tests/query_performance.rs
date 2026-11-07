//! Large-history HTTP and shared-lock regression. Synthetic data only.
//! Synthetic values only: 9 providers, 35 connections, 47 mappings and 60k real-attempt rows.
use axum::{body::Body, http::{Request, StatusCode}};
use serde_json::Value;
use std::{path::PathBuf, time::Instant};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tokengo_shared::ModelKind;
use tower::ServiceExt;

struct Fixture { state: SharedState, dir: PathBuf, token: String }
impl Drop for Fixture { fn drop(&mut self) { let _=std::fs::remove_dir_all(&self.dir); } }
impl Fixture {
    fn new() -> Self {
        let dir=std::env::temp_dir().join(format!("management-perf-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config { db_path:dir.join("db"), master_key_path:dir.join("key").to_string_lossy().into(), ..Default::default() }).unwrap();
        let token;
        {
            let c=state.conn();
            db::sessions::set_panel_password(&c,"fixture-password-123").unwrap();
            token=db::sessions::create_session(&c,None).unwrap();
            c.execute("INSERT OR IGNORE INTO gateway_keys(id,name,hint,created_at_ms,key_hash,user_id) VALUES(1,'Synthetic key','fixture',0,'synthetic-hash',1)",[]).unwrap();
            c.execute_batch("BEGIN").unwrap();
            for id in 1..=9 {
                c.execute("INSERT INTO provider_channels(id,name,created_ms) VALUES(?1,?2,0)",rusqlite::params![id,format!("provider-{id}")]).unwrap();
            }
            for id in 1..=35 {
                c.execute("INSERT INTO channels(id,name,base_url,enabled,priority) VALUES(?1,?2,'http://fixture.invalid/v1',1,?1)",rusqlite::params![id,format!("source-{id}")]).unwrap();
                db::provider_channels::set_membership(&c,id,Some((id-1)%9+1)).unwrap();
            }
            for i in 0..47 {
                let name=format!("fixture-model-{i:02}");
                c.execute("INSERT INTO channel_models(channel_id,model_name,upstream_model) VALUES(?1,?2,?3)",rusqlite::params![i%35+1,name,format!("wire-{i}")]).unwrap();
                db::kinds::set(&c,&name,match i%3 {0=>ModelKind::Chat,1=>ModelKind::Embeddings,_=>ModelKind::Rerank}).unwrap();
            }
            let now=db::now_ms();
            let count=std::env::var("TOKENGO_PERF_ROWS").ok().and_then(|x|x.parse::<i64>().ok()).unwrap_or(1_000_000).max(100_000);
            // Wide old rows expose the table-scan cost hidden by earlier narrow 60k fixtures.
            c.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<?1) INSERT INTO requests(id,ts_ms,endpoint,model,served_model,status,channel_id,user_id,key_id,explored,error,input_tokens,output_tokens,input_tokens_known,output_tokens_known,usage_source) SELECT i,?2-CASE WHEN i<=?1-70000 THEN 2592000000+i ELSE (?1-i)*1000 END,'/v1/chat/completions','auto',printf('fixture-model-%02d',i%47),'ok',(i%35)+1,1,1,i%100=0,printf('%01024d',0),10,2,1,1,'upstream' FROM n",rusqlite::params![count,now]).unwrap();
            c.execute_batch("INSERT INTO attempts(request_id,ordinal,channel_id,outcome,duration_ms,first_byte_ms,model_name) SELECT id,0,channel_id,CASE WHEN id%17=0 THEN 'channel_fault' ELSE 'success' END,100+(id%200),30+(id%50),served_model FROM requests;").unwrap();
            c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,debit_micro,input_tokens,output_tokens,status,created_ms,source_snapshot) SELECT 'fixture-'||id,1,1,id,CASE WHEN id%100=0 THEN 900+(id%7) ELSE channel_id END,replace(served_model,'-',' '),20,10,0,1,10,2,'settled',ts_ms,json_object('channel_name','synthetic historical source','padding',printf('%01024d',0)) FROM requests",[]).unwrap();
            c.execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
            // Match startup prewarm in new versions, and record this one-time cost separately.
            let start=Instant::now(); let totals=db::scores::total_calls(&c).unwrap();
            assert_eq!(totals.values().sum::<i64>(),count);
            let pages:i64=c.query_row("PRAGMA page_count",[],|r|r.get(0)).unwrap();
            let size:i64=c.query_row("PRAGMA page_size",[],|r|r.get(0)).unwrap();
            eprintln!("FIXTURE requests={count} attempts={count} ledger={count} bytes={} prewarm_ms={}",pages*size,start.elapsed().as_millis());

        }
        Self {state,dir,token}
    }
    async fn get(&self,path:&str,authenticated:bool)->(StatusCode,Value,u128) {
        let mut request=Request::builder().uri(path);
        if authenticated {request=request.header("authorization",format!("Bearer {}",self.token));}
        let start=Instant::now();
        let response=app(self.state.clone()).oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
        let status=response.status();
        let bytes=axum::body::to_bytes(response.into_body(),4<<20).await.unwrap();
        (status,serde_json::from_slice(&bytes).unwrap(),start.elapsed().as_micros())
    }
}


// Opt-in on a dedicated host; exact semantics are also exercised by ordinary suites.
#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore = "million-row wide-history performance gate; run on build-test in release mode"]
async fn large_history_http_queries_and_concurrent_navigation() {
    let f=Fixture::new();
    profile_model_components(&f);
    let observe_only=std::env::var_os("TOKENGO_PERF_OBSERVE_ONLY").is_some();
    let mut failures=Vec::new();
    // Cover each management query family, with window/list/filter variants.
    for endpoint in ["/api/model-routes","/api/providers","/api/channels","/api/stats?hours=1","/api/stats?hours=24","/api/stats?hours=168","/api/logs?limit=50","/api/requests/1","/api/model-cooldowns","/api/auto-policy","/api/routing","/api/model-kinds","/api/gateway-keys","/api/managed-users","/api/settings","/api/pricing","/api/request-pricing","/api/billing/objects","/api/budgets?state=active","/api/usage?limit=50&state=active","/api/usage?limit=50&state=historical","/api/usage?limit=50&state=all","/api/usage?limit=50&state=all&user_id=1","/api/usage?limit=50&state=all&channel_id=1","/api/usage?limit=50&state=all&model=fixture-model-00","/api/usage?limit=50&state=active&model=fixture-model-00","/api/usage?limit=50&state=historical&model=fixture-model-00","/api/usage?limit=50&state=active&user_id=1&key_id=1&channel_id=1","/api/passthrough-routes"] {
        let mut timings=Vec::new();
        for _ in 0..5 {
            let(status,body,us)=f.get(endpoint,true).await;
            assert_eq!(status,StatusCode::OK,"{endpoint}: {body}");
            if endpoint.starts_with("/api/usage?") {
                let matches=expected_usage_matches(endpoint);
                assert_eq!(body["entries"].as_array().unwrap().len(),matches.min(50),"fixture generation oracle: {endpoint}");
                assert_eq!(!body["next_before_id"].is_null(),matches>50,"continuation must reflect real remaining rows: {endpoint}");
            }
            timings.push(us);
        }
        timings.sort_unstable();
        let limit=if endpoint.starts_with("/api/stats") {1_000_000} else {300_000};
        eprintln!("PERF endpoint={endpoint} n=5 p50_us={} max_us={} limit_us={limit}",timings[2],timings[4]);
        if timings[4]>limit {failures.push(format!("{endpoint}: {}us > {limit}us",timings[4]));}
    }
    // Actual loopback HTTP: overlap model navigation with light polling and DB writes.
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=listener.local_addr().unwrap();
    let router=app(f.state.clone());
    let server=tokio::spawn(async move {axum::serve(listener,router).await.unwrap()});
    let client=reqwest::Client::new();
    for round in 0..3 {
      let start_gate=std::sync::Arc::new(tokio::sync::Barrier::new(14));
      let mut pending=Vec::new();
      for i in 0..12 {
        let client=client.clone();let auth=f.token.clone();let gate=start_gate.clone();
        let path=match i%3 {0=>"/api/model-routes",1=>"/api/logs?limit=12",_=>"/api/usage?limit=50&state=active"};
        pending.push(tokio::spawn(async move {gate.wait().await;let start=Instant::now();let response=client.get(format!("http://{address}{path}")).bearer_auth(auth).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);response.bytes().await.unwrap();(path,start.elapsed().as_micros())}));
      }
      let state=f.state.clone();let gate=start_gate.clone();let runtime=tokio::runtime::Handle::current();
      let writer=tokio::task::spawn_blocking(move || {runtime.block_on(gate.wait());std::thread::sleep(std::time::Duration::from_millis(5));let start=Instant::now();let c=state.conn();let wait_us=start.elapsed().as_micros();let id=db::requests::start(&c,"/v1/embeddings","fixture-live",None,false).unwrap();assert!(id>0);(wait_us,start.elapsed().as_micros())});
      start_gate.wait().await;
      for job in pending {let(path,us)=job.await.unwrap();eprintln!("CONCURRENT round={round} endpoint={path} latency_us={us}");if us>1_000_000{failures.push(format!("concurrent {path}: {us}us"));}}
      let(wait,us)=writer.await.unwrap();eprintln!("CONCURRENT round={round} writer_wait_us={wait} writer_us={us}");if us>1_000_000{failures.push(format!("writer: {us}us"));}
    }
    server.abort();
    assert!(observe_only||failures.is_empty(),"large-history latency gate failed: {failures:?}");
    eprintln!("PERF_GATE observe_only={observe_only} violations={failures:?}");
}

// Independent arithmetic oracle for the synthetic ledger generator. Smaller
// histories legitimately have fewer than 50 rare historical/model matches.
fn expected_usage_matches(endpoint:&str)->usize {
    let rows=std::env::var("TOKENGO_PERF_ROWS").ok().and_then(|v|v.parse::<i64>().ok()).unwrap_or(1_000_000).max(100_000);
    let args=endpoint.split_once('?').unwrap().1.split('&').filter_map(|p|p.split_once('=')).collect::<std::collections::HashMap<_,_>>();
    let model=args.get("model").map(|name|name.rsplit('-').next().unwrap().parse::<i64>().unwrap());
    let channel=args.get("channel_id").map(|id|id.parse::<i64>().unwrap());
    (1..=rows).filter(|id| {
        let id=*id;let historical=id%100==0;
        if args.get("user_id").is_some_and(|id|*id!="1")||args.get("key_id").is_some_and(|id|*id!="1") {return false;}
        if model.is_some_and(|model|id%47!=model) {return false;}
        let source=if historical {900+id%7}else{id%35+1};
        if channel.is_some_and(|channel|source!=channel) {return false;}
        match args.get("state").copied().unwrap_or("all") {"active"=>!historical,"historical"=>historical,_=>true}
    }).count()
}

// Diagnostic-only trace attaches to the synthetic fixture's connection. It
// records statement text without bindings and SQLite's own profile durations.
// No trace, callback, or instrumentation is installed in production code.
type SqlProfiles=std::sync::Mutex<std::collections::HashMap<String,(u64,u64,u64)>>;
unsafe extern "C" fn model_sql_profile(mask:std::os::raw::c_uint,context:*mut std::os::raw::c_void,statement:*mut std::os::raw::c_void,elapsed:*mut std::os::raw::c_void)->std::os::raw::c_int {
    if mask!=rusqlite::ffi::SQLITE_TRACE_PROFILE as u32 || statement.is_null() || elapsed.is_null() {return 0;}
    let sql=unsafe{rusqlite::ffi::sqlite3_sql(statement.cast())};
    if sql.is_null(){return 0;}
    let text=unsafe{std::ffi::CStr::from_ptr(sql)}.to_string_lossy().into_owned();
    let ns=unsafe{*elapsed.cast::<u64>()};
    let profiles=unsafe{&*context.cast::<SqlProfiles>()};
    if let Ok(mut profiles)=profiles.lock(){let row=profiles.entry(text).or_default();row.0+=1;row.1+=ns;row.2=row.2.max(ns);}
    0
}
struct ModelSqlTrace {state:SharedState,profiles:Box<SqlProfiles>}
impl ModelSqlTrace {
    fn new(state:SharedState)->Self {
        let mut profiles=Box::<SqlProfiles>::default();
        let context=(&mut *profiles as *mut SqlProfiles).cast();
        {let c=state.conn();assert_eq!(unsafe{rusqlite::ffi::sqlite3_trace_v2(c.handle(),rusqlite::ffi::SQLITE_TRACE_PROFILE as u32,Some(model_sql_profile),context)},rusqlite::ffi::SQLITE_OK);}
        Self{state,profiles}
    }
    fn report(&self) {
        let profiles=self.profiles.lock().unwrap();let mut rows=profiles.iter().collect::<Vec<_>>();rows.sort_by_key(|(_,p)|std::cmp::Reverse(p.1));
        eprintln!("MODEL_SQL_TOTAL ns={} statements={}",rows.iter().map(|(_,p)|p.1).sum::<u64>(),rows.iter().map(|(_,p)|p.0).sum::<u64>());
        for (sql,(count,total,max)) in rows.into_iter().take(35){eprintln!("MODEL_SQL count={count} total_ns={total} max_ns={max} sql={sql:?}");}
    }
}
impl Drop for ModelSqlTrace {
    fn drop(&mut self){let c=self.state.conn();unsafe{rusqlite::ffi::sqlite3_trace_v2(c.handle(),0,None,std::ptr::null_mut());}}
}

fn profile_model_components(f:&Fixture) {
    fn measure<T>(label:&str,mut work:impl FnMut()->T){for run in 0..3{let start=Instant::now();let result=work();let us=start.elapsed().as_micros();std::hint::black_box(&result);eprintln!("MODEL_STAGE label={label} run={run} us={us}");}}
    {
        let c=f.state.conn();let now=db::now_ms();
        measure("channels",||db::channels::list(&c).unwrap());
        measure("routes_with_channels",||db::model_routes::list(&c).unwrap());
        measure("scores_all",||db::scores::all(&c,now).unwrap());
        measure("lifetime_totals_clean",||db::scores::total_calls(&c).unwrap());
        let exploration="SELECT served_model,COUNT(*) FROM requests WHERE explored=1 AND served_model IS NOT NULL GROUP BY served_model";
        let score="SELECT a.model_name,a.outcome,a.duration_ms,r.ts_ms,a.error,a.first_byte_ms FROM requests r INDEXED BY requests_stats_window CROSS JOIN attempts a INDEXED BY idx_attempts_request ON a.request_id=r.id WHERE r.ts_ms>=?1 AND a.model_name IS NOT NULL AND a.model_name<>'' AND a.outcome IN ('success','channel_fault','empty_reply') AND r.status<>'canceled' AND (?2 IS NULL OR a.channel_id=?2) ORDER BY a.id";
        measure("exploration_counts",||{let mut q=c.prepare(exploration).unwrap();q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()});
        measure("score_sql_decode_without_normalize",||{let mut q=c.prepare(score).unwrap();let mut rows=q.query(rusqlite::params![now-db::scores::WINDOW_MS,Option::<i64>::None]).unwrap();let mut count=0u64;while let Some(r)=rows.next().unwrap(){for i in 0..6{match r.get_ref(i).unwrap(){rusqlite::types::ValueRef::Text(v)=>count=count.wrapping_add(v.len() as u64),rusqlite::types::ValueRef::Integer(v)=>count=count.wrapping_add(v as u64),_=>{}}}}count});
        let routes=db::model_routes::list(&c).unwrap();
        measure("per_model_metadata_without_sources",||{for r in &routes{std::hint::black_box((db::model_access::display_name(&c,&r.name).unwrap(),db::model_access::catalog_purpose(&c,&r.name).unwrap(),db::model_routes::visible(&c,&r.name).unwrap(),db::model_access::internal_only(&c,&r.name).unwrap(),db::policy::rows(&c,&r.name).unwrap()));}});
        // source_json loads channels per invocation (public API). This is an
        // explicit upper bound, unlike the endpoint's shared channel snapshot.
        measure("sources_public_upper_bound",||routes.iter().map(|r|db::provider_channels::source_json(&c,&r.name).unwrap()).collect::<Vec<_>>());
        for (label,sql,args) in [("exploration",exploration,Vec::<rusqlite::types::Value>::new()),("score",score,vec![(now-db::scores::WINDOW_MS).into(),rusqlite::types::Value::Null])] {
            let mut q=c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let plan=q.query_map(rusqlite::params_from_iter(args.iter()),|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            eprintln!("MODEL_PLAN label={label} plan={plan:?}");
            let mut q=c.prepare(&format!("EXPLAIN {sql}")).unwrap();
            for row in q.query_map(rusqlite::params_from_iter(args.iter()),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<String>>(5)?))).unwrap(){eprintln!("MODEL_VDBE label={label} row={:?}",row.unwrap());}
        }
    }
}

#[tokio::test]
#[ignore="million-row component/SQLite-profile diagnostic; build-test release only"]
async fn profile_remaining_model_list_costs() {
    if std::env::var_os("TOKENGO_QUERY_PROFILE").is_none() {eprintln!("diagnostic profile disabled; set TOKENGO_QUERY_PROFILE=1 explicitly");return;}
    let f=Fixture::new();
    profile_model_components(&f);
    let trace=ModelSqlTrace::new(f.state.clone());
    for run in 0..3{let(status,body,us)=f.get("/api/model-routes",true).await;assert_eq!(status,StatusCode::OK,"{body}");eprintln!("MODEL_HTTP_PROFILE run={run} us={us}");}
    trace.report();drop(trace);
}
