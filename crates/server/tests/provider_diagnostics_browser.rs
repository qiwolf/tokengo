//! Opt-in real-app browser harness. All credentials and upstream responses are synthetic.
//! Run with --ignored --nocapture and explicit WEB_DIR / private DIR environment settings.
use axum::{extract::Query, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::{Arc, atomic::{AtomicU64, AtomicU16, AtomicBool, Ordering}}, time::Duration};
use tokengo_server::{app, db, AppState, Config};

struct Cleanup { directory: PathBuf, tasks: Vec<tokio::task::JoinHandle<()>> }
impl Drop for Cleanup {
    fn drop(&mut self) { for task in &self.tasks { task.abort(); } let _=std::fs::remove_dir_all(&self.directory); }
}
#[derive(Clone, Default)]
struct Controls { embeddings:Arc<AtomicU64>, rerank:Arc<AtomicU64>, rerank_documents:Arc<AtomicU64>, rerank_expected_input:Arc<AtomicBool>, delay: Arc<AtomicU64>, calls: Arc<AtomicU64>, metadata: Arc<AtomicU64>, status: Arc<AtomicU16> }
#[derive(Deserialize)]
struct Change { delay_ms: Option<u64>, status: Option<u16> }
async fn api(client:&reqwest::Client,base:&str,token:&str,method:&str,path:&str,body:Value)->Value {
    let response=client.request(method.parse().unwrap(),format!("{base}{path}")).bearer_auth(token).json(&body).send().await.unwrap();
    let status=response.status();let result:Value=response.json().await.unwrap();assert!(status.is_success(),"synthetic seed request failed {status}: {result}");result
}
async fn listen(router:Router,port:u16)->tokio::task::JoinHandle<()> {
    let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,port)).await.unwrap();
    tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); })
}
#[tokio::test]
#[ignore = "explicit browser acceptance only; starts a loopback app until finish marker or TTL"]
async fn browser_acceptance_harness() {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let directory=PathBuf::from(std::env::var("TOKENGO_DIAGNOSTICS_BROWSER_DIR").expect("set a new private harness directory"));
    assert!(directory.is_absolute());
    std::fs::DirBuilder::new().mode(0o700).create(&directory).expect("private directory must not already exist");
    let mut cleanup=Cleanup{directory:directory.clone(),tasks:vec![]};
    let web=PathBuf::from(std::env::var("TOKENGO_DIAGNOSTICS_BROWSER_WEB_DIR").expect("set actual built web directory"));
    assert!(web.join("index.html").is_file(),"actual built web index required");
    let port=std::env::var("TOKENGO_DIAGNOSTICS_BROWSER_PORT").unwrap_or_else(|_|"39731".into()).parse::<u16>().unwrap();
    let upstream_port=std::env::var("TOKENGO_DIAGNOSTICS_BROWSER_UPSTREAM_PORT").unwrap_or_else(|_|"39732".into()).parse::<u16>().unwrap();
    assert_ne!(port,upstream_port);
    let controls=Controls::default();controls.status.store(200,Ordering::SeqCst);
    let change=controls.clone();let chat=controls.clone();let version=controls.clone();let show=controls.clone();let models=controls.clone();let embeddings=controls.clone();let rerank=controls.clone();
    let upstream=Router::new()
        .route("/__control",get(move|Query(change_to):Query<Change>|{let c=change.clone();async move {
            if let Some(delay)=change_to.delay_ms {c.delay.store(delay.min(30_000),Ordering::SeqCst);}
            if let Some(status)=change_to.status {if (200..=599).contains(&status){c.status.store(status,Ordering::SeqCst);}}
            Json(json!({"inference_calls":c.calls.load(Ordering::SeqCst),"metadata_calls":c.metadata.load(Ordering::SeqCst),"embedding_calls":c.embeddings.load(Ordering::SeqCst),"rerank_calls":c.rerank.load(Ordering::SeqCst),"rerank_documents":c.rerank_documents.load(Ordering::SeqCst),"rerank_expected_input":c.rerank_expected_input.load(Ordering::SeqCst),"delay_ms":c.delay.load(Ordering::SeqCst),"status":c.status.load(Ordering::SeqCst)}))
        }}))
        .route("/v1/chat/completions",post(move||{let c=chat.clone();async move {
            c.calls.fetch_add(1,Ordering::SeqCst);tokio::time::sleep(Duration::from_millis(c.delay.load(Ordering::SeqCst))).await;
            (axum::http::StatusCode::from_u16(c.status.load(Ordering::SeqCst)).unwrap(),Json(json!({"id":"synthetic-browser-response","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"synthetic ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})))
        }}))
        .route("/v1/embeddings",post(move||{let c=embeddings.clone();async move{c.embeddings.fetch_add(1,Ordering::SeqCst);Json(json!({"object":"list","data":[{"index":0,"embedding":[0.1,0.2,0.3]}],"usage":{"prompt_tokens":1,"total_tokens":1}}))}}))
        .route("/v1/rerank",post(move|Json(body):Json<Value>|{let c=rerank.clone();async move{c.rerank.fetch_add(1,Ordering::SeqCst);let documents=body["documents"].as_array().cloned().unwrap_or_default();c.rerank_documents.store(documents.len() as u64,Ordering::SeqCst);c.rerank_expected_input.store(body["query"]=="browser ranking query"&&documents==vec![json!("document A"),json!("document B")],Ordering::SeqCst);Json(json!({"results":[{"index":0,"relevance_score":0.9}],"usage":{"total_tokens":1}}))}}))
        .route("/api/version",get(move||{let c=version.clone();async move{c.metadata.fetch_add(1,Ordering::SeqCst);Json(json!({"version":"0.13.3"}))}}))
        .route("/api/show",post(move||{let c=show.clone();async move{c.metadata.fetch_add(1,Ordering::SeqCst);Json(json!({"template":"synthetic template only","capabilities":["completion","thinking"],"model_info":{}}))}}))
        .route("/v1/models",get(move||{let c=models.clone();async move{c.metadata.fetch_add(1,Ordering::SeqCst);Json(json!({"object":"list","data":[{"id":"browser-upstream","object":"model","owned_by":"synthetic"}]}))}}));
    cleanup.tasks.push(listen(upstream,upstream_port).await);
    let cfg=Config{db_path:directory.join("app.db"),master_key_path:directory.join("master").to_string_lossy().into(),web_dir:Some(web),..Config::default()};
    let state=AppState::init(cfg).unwrap();
    let panel_password="synthetic-browser-only-password-1234";
    db::sessions::set_panel_password(&state.conn(),panel_password).unwrap();
    let token=db::sessions::create_session(&state.conn(),None).unwrap();
    let root=db::accounts::root(&state.conn()).unwrap().unwrap();
    let cookie=db::accounts::create_session(&state.conn(),&root).unwrap();
    let mut routing=state.routing().cfg.clone();routing.retry_times=0;routing.probe_enabled=false;
    db::routing::save_config(&state.conn(),&routing).unwrap();state.routing().cfg=routing;
    cleanup.tasks.push(listen(app(state.clone()),port).await);
    let base=format!("http://127.0.0.1:{port}");let upstream_base=format!("http://127.0.0.1:{upstream_port}");let client=reqwest::Client::new();
    let provider=api(&client,&base,&token,"POST","/api/providers",json!({"name":"W10 合成浏览器验收","base_url":format!("{upstream_base}/v1"),"enabled":true})).await;
    let provider_id=provider["provider"]["id"].as_i64().unwrap();
    let credential=api(&client,&base,&token,"POST",&format!("/api/providers/{provider_id}/credentials"),json!({"label":"Synthetic browser source","key":"synthetic-upstream-test-key","models":[{"model":"browser-alias","upstream":"browser-upstream","enabled":true},{"model":"browser-vector","upstream":"browser-vector-upstream","enabled":true},{"model":"browser-rerank","upstream":"browser-rerank-upstream","enabled":true}]})).await;
    let source_id=credential["source_id"].as_i64().unwrap();
    db::kinds::set(&state.conn(),"browser-alias",tokengo_shared::ModelKind::Chat).unwrap();
    db::model_routes::set_enabled(&state.conn(),"browser-alias",true).unwrap();
    db::model_routes::set_visible(&state.conn(),"browser-alias",true).unwrap();
    db::model_access::set_catalog_purpose(&state.conn(),"browser-alias","chat").unwrap();
    for (model,kind,purpose) in [("browser-vector",tokengo_shared::ModelKind::Embeddings,"embeddings"),("browser-rerank",tokengo_shared::ModelKind::Rerank,"rerank")] {
        db::kinds::set(&state.conn(),model,kind).unwrap();db::model_routes::set_enabled(&state.conn(),model,true).unwrap();db::model_routes::set_visible(&state.conn(),model,true).unwrap();db::model_access::set_catalog_purpose(&state.conn(),model,purpose).unwrap();
    }
    let generation:i64=state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[source_id],|r|r.get(0)).unwrap();
    api(&client,&base,&token,"PUT",&format!("/api/channels/{source_id}/protocols"),json!({"expected_binding_generation":generation,"protocols":{"native":["chat"],"parameter_profile":"ollama","max_input_tokens":4096,"max_output_tokens":1024}})).await;
    // Prove the same existing root diagnostic path that the browser will use. It needs
    // no customer VKey; missing source pricing is explicitly recorded as unknown cost.
    let eligible=api(&client,&base,&token,"GET","/api/custom-probes",Value::Null).await;
    assert!(eligible["eligible_source_ids"].as_array().unwrap().contains(&json!(source_id)));
    let smoke=api(&client,&base,&token,"POST","/api/custom-probes",json!({"name":"synthetic harness preflight","source_id":source_id,"model":"browser-alias","kind":"chat","input":"Reply OK","max_tokens":8,"expected_status":200,"contains":"synthetic ok","expected_dimensions":null,"timeout_ms":10000,"max_latency_ms":null,"interval_secs":300,"enabled":false})).await;
    let smoke_id=smoke["probe"]["id"].as_str().unwrap();
    let run=api(&client,&base,&token,"POST",&format!("/api/custom-probes/{smoke_id}/run"),Value::Null).await;
    assert_eq!(run["result"]["ok"],true,"synthetic custom-probe preflight must actually pass: {run}");
    assert_eq!(run["result"]["status"],200);
    assert_eq!(run["result"]["billing"]["debit_micro"],0);
    assert!(controls.calls.load(Ordering::SeqCst)>0,"preflight must reach real loopback HTTP");
    let ledger_id=run["result"]["billing"]["ledger_id"].as_i64().unwrap();
    let ledger=db::billing::usage(&state.conn(),&Default::default()).unwrap().into_iter().find(|row|row.id==ledger_id).unwrap();
    assert_eq!((ledger.user_id,ledger.key_id,ledger.request_id),(0,0,0));
    assert_eq!(ledger.auth_snapshot["kind"],"admin_diagnostic");
    api(&client,&base,&token,"DELETE",&format!("/api/custom-probes/{smoke_id}"),json!({"version":smoke["probe"]["version"]})).await;
    let preflight=json!({"passed":true,"kind":"chat","status":200,"source_id":source_id,"user_id":0,"key_id":0,"request_id":0,"debit_micro":0,"cost_status":run["result"]["billing"]["cost_status"],"upstream_calls":controls.calls.load(Ordering::SeqCst),"note":"Executed through existing custom-probe root diagnostic path; no customer VKey or customer quota was created"});
    let report=json!({"synthetic":true,"preflight":preflight,"base_url":base,"provider_id":provider_id,"source_id":source_id,"model":"browser-alias","upstream_model":"browser-upstream","specialized_models":[{"model":"browser-vector","kind":"embeddings"},{"model":"browser-rerank","kind":"rerank"}],"username":"admin","panel_password":panel_password,"account_cookie_name":"tokengo_account_session","account_session":cookie,"control_url":format!("{upstream_base}/__control"),"finish_marker":directory.join("finish"),"note":"All credentials are synthetic and this directory is deleted on normal completion"});
    let report_path=directory.join("browser-session.json");
    std::fs::write(&report_path,serde_json::to_vec_pretty(&report).unwrap()).unwrap();std::fs::set_permissions(&report_path,std::fs::Permissions::from_mode(0o600)).unwrap();
    println!("Synthetic browser harness ready; private report: {}",report_path.display());
    let ttl=std::env::var("TOKENGO_DIAGNOSTICS_BROWSER_TTL_SECONDS").ok().and_then(|s|s.parse::<u64>().ok()).unwrap_or(900).min(3600);
    let start=std::time::Instant::now();
    while !directory.join("finish").exists()&&start.elapsed()<Duration::from_secs(ttl){tokio::time::sleep(Duration::from_millis(250)).await;}
}
