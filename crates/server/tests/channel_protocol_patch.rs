//! Regression scenarios for the protocol-only editor. Authored, not executed in this batch.
use axum::{body::Body,http::Request};
use serde_json::{json,Value};
use tokengo_server::{app,db,AppState,Config,SharedState};
use tower::ServiceExt;
async fn put(state:&SharedState,id:i64,panel:Option<&str>,body:Value)->(u16,Value){
    let mut request=Request::builder().method("PUT").uri(format!("/api/channels/{id}/protocols")).header("content-type","application/json");
    if let Some(panel)=panel{request=request.header("authorization",format!("Bearer {panel}"));}
    let response=app(state.clone()).oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status=response.status().as_u16();let bytes=axum::body::to_bytes(response.into_body(),1<<20).await.unwrap();
    (status,serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn protocol_editor_preserves_credentials_models_and_rejects_stale_generation(){
    let dir=std::env::temp_dir().join(format!("protocol-patch-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
    let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();
    let (id,panel,generation)={let c=state.conn();let id=db::channels::create(&c,&db::channels::NewChannel{name:"protocol-only".into(),base_url:"https://example.invalid/v1".into(),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
        db::channels::set_secret(&c,&state.secrets,id,"SYNTHETIC-ONLY-PATCH-SECRET").unwrap();
        db::channels::set_headers(&c,&state.secrets,id,&[("x-test-header".into(),"preserved".into())]).unwrap();
        db::channels::set_models(&c,id,&[db::channels::ModelMapping::new("public",Some("physical".into()),false)]).unwrap();
        let generation=c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[id.0],|r|r.get::<_,i64>(0)).unwrap();
        (id,db::sessions::create_session(&c,None).unwrap(),generation)};
    let body=json!({"protocols":{"native":["responses"],"allow_conversion":true},"expected_binding_generation":generation});
    assert_eq!(put(&state,id.0,None,body.clone()).await.0,401);
    let (status,saved)=put(&state,id.0,Some(&panel),body.clone()).await;assert_eq!(status,200);assert_eq!(saved["protocols"]["native"],json!(["responses"]));
    assert_eq!(put(&state,id.0,Some(&panel),body).await.0,409,"stale form must not overwrite a changed source");
    {let c=state.conn();assert_eq!(db::channels::secret(&c,&state.secrets,id).unwrap().as_deref(),Some("SYNTHETIC-ONLY-PATCH-SECRET"));assert_eq!(db::channels::headers(&c,&state.secrets,id).unwrap(),vec![("x-test-header".into(),"preserved".into())]);let row=db::channels::get(&c,id).unwrap().unwrap();assert_eq!(row.models.len(),1);assert_eq!(row.models[0].upstream.as_deref(),Some("physical"));}
    drop(state);std::fs::remove_dir_all(dir).ok();
}
#[tokio::test]
async fn malformed_proxy_credentials_are_not_echoed_or_persisted(){
    let dir=std::env::temp_dir().join(format!("proxy-redaction-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
    let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();
    let panel={let c=state.conn();db::sessions::create_session(&c,None).unwrap()};
    let body=json!({"mode":"proxy","proxy_url":"http://127.0.0.1:7890","credential_action":"replace","username":"synthetic","password":1234567890123_i64,"expected_version":0});
    let request=Request::builder().method("PUT").uri("/api/network/proxy-policies/family/gpt").header("content-type","application/json").header("authorization",format!("Bearer {panel}")).body(Body::from(body.to_string())).unwrap();
    let response=app(state.clone()).oneshot(request).await.unwrap();assert_eq!(response.status().as_u16(),400);
    let bytes=axum::body::to_bytes(response.into_body(),1<<20).await.unwrap();let text=String::from_utf8(bytes.to_vec()).unwrap();assert!(!text.contains("1234567890123"));
    {let c=state.conn();assert!(db::settings::get(&c,"egress_policy:family:gpt").unwrap().is_none());}
    drop(state);std::fs::remove_dir_all(dir).ok();
}
#[tokio::test]
async fn generic_background_catalog_cannot_replace_oauth_account_metadata(){
    use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
    let hits=Arc::new(AtomicUsize::new(0));let counted=hits.clone();
    let router=axum::Router::new().route("/v1/models",axum::routing::get(move||{let counted=counted.clone();async move{counted.fetch_add(1,Ordering::SeqCst);axum::Json(json!({"data":[{"id":"unverified-public-model"}]}))}}));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}/v1",listener.local_addr().unwrap());let server=tokio::spawn(async move{axum::serve(listener,router).await.unwrap()});
    let dir=std::env::temp_dir().join(format!("oauth-catalog-isolation-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
    let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();
    {let c=state.conn();let source=db::channels::create(&c,&db::channels::NewChannel{name:"oauth-isolation".into(),base_url:url,enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
        db::channels::set_models(&c,source,&[db::channels::ModelMapping::new("public",None,false)]).unwrap();
        c.execute("INSERT INTO oauth_accounts(label,issuer,subject,client_id,status,scopes_json,expires_at,earliest_refresh_at) VALUES('synthetic','https://auth.openai.com','synthetic','synthetic','active','[]',?1,0)",[db::now_ms()+60000]).unwrap();
        let account=c.last_insert_rowid();c.execute("INSERT INTO oauth_channel_bindings(channel_id,account_id,max_input_tokens,max_output_tokens) VALUES(?1,?2,1000,1000)",rusqlite::params![source.0,account]).unwrap();}
    tokengo_server::capability_detector::tick(&state).await.unwrap();assert_eq!(hits.load(Ordering::SeqCst),0,"OAuth must use its account catalog, not an unauthenticated provider fetch");
    server.abort();drop(state);std::fs::remove_dir_all(dir).ok();
}
