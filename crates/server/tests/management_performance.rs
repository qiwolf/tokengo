//! Representative management workload. Run --release --nocapture for timing evidence.
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
            c.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<60000) INSERT INTO requests(id,ts_ms,endpoint,model,served_model,status,explored) SELECT i,?1-i*100,'chat','auto',printf('fixture-model-%02d',i%47),'ok',i%20=0 FROM n",[now]).unwrap();
            c.execute_batch("INSERT INTO attempts(request_id,ordinal,channel_id,outcome,duration_ms,first_byte_ms,model_name) SELECT id,0,(id%35)+1,CASE WHEN id%17=0 THEN 'channel_fault' ELSE 'success' END,100+(id%200),30+(id%50),served_model FROM requests; COMMIT;").unwrap();
            assert_eq!(c.query_row("SELECT COUNT(*) FROM model_invocations",[],|r|r.get::<_,i64>(0)).unwrap(),60000);
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

#[tokio::test]
async fn representative_management_latency_and_live_mutations() {
    let f=Fixture::new();
    assert_eq!(f.get("/api/model-routes",false).await.0,StatusCode::UNAUTHORIZED);
    let (status,first,_)=f.get("/api/model-routes",true).await;
    assert_eq!(status,StatusCode::OK,"{first}");
    assert_eq!(first["models"].as_array().unwrap().len(),47);
    let before=first["models"].as_array().unwrap().iter().find(|m|m["name"]=="fixture-model-00").unwrap();
    assert_eq!(before["sources"][0]["connection_enabled"],true);
    assert_eq!(before["routed"],true);
    assert!(before["total_calls"].as_i64().unwrap()>1000);
    assert!(before["runtime"]["samples"].as_i64().unwrap()>1000);
    {
        let c=f.state.conn();
        c.execute("UPDATE channels SET enabled=0 WHERE id=1",[]).unwrap();
        db::model_routes::set_visible(&c,"fixture-model-00",false).unwrap();
        c.execute("UPDATE provider_channels SET enabled=0 WHERE id=2",[]).unwrap();
    }
    let (_,response,_)=f.get("/api/model-routes",true).await;
    let provider_changed=response["models"].as_array().unwrap().iter().find(|m|m["name"]=="fixture-model-01").unwrap();
    assert_eq!(provider_changed["sources"][0]["provider_enabled"],false);
    assert_eq!(provider_changed["routed"],false);
    let changed=response["models"].as_array().unwrap().iter().find(|m|m["name"]=="fixture-model-00").unwrap();
    assert_eq!(changed["sources"][0]["connection_enabled"],false);
    assert_eq!(changed["routed"],false);
    assert_eq!(changed["visible"],false);
    assert!(!db::model_routes::channels_for(&f.state.conn(),"fixture-model-00").unwrap()[0].enabled);
    for endpoint in ["/api/model-routes","/api/stats","/api/channels"] {
        let mut samples=Vec::new();
        for _ in 0..20 {
            let (status,value,us)=f.get(endpoint,true).await;
            assert_eq!(status,StatusCode::OK,"{endpoint}: {value}");
            samples.push(us);
        }
        samples.sort_unstable();
        eprintln!("management fixture endpoint={endpoint} n=20 p50_us={} p95_us={} max_us={}",samples[9],samples[18],samples[19]);
        // Debug mode allows instrumentation overhead; release must meet the UI API target.
        let limit_us=if cfg!(debug_assertions) {5_000_000} else {300_000};
        assert!(samples[18]<limit_us,"{endpoint} management P95 exceeded {limit_us}us");
    }
}
