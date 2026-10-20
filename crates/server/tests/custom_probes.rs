//! Real loopback requests prove persistence, bounded execution, permissions and race safety.
use axum::{Router, Json, extract::{State, OriginalUri}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, routing::post};
use serde_json::{Value,json};
use std::sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}};
use tokengo_server::{app,db,AppState,Config,SharedState,custom_probes};
#[derive(Clone,Default)]
struct Mock {calls:Arc<AtomicUsize>,captures:Arc<Mutex<Vec<(String,String,String,Value)>>>,started:Arc<tokio::sync::Notify>,release:Arc<tokio::sync::Notify>}
async fn upstream(State(mock):State<Mock>,OriginalUri(uri):OriginalUri,headers:HeaderMap,Json(body):Json<Value>)->Response {
    mock.calls.fetch_add(1,Ordering::SeqCst);
    mock.captures.lock().unwrap().push((uri.path().into(),headers.get("authorization").and_then(|h|h.to_str().ok()).unwrap_or("").into(),headers.get("x-api-key").and_then(|h|h.to_str().ok()).unwrap_or("").into(),body.clone()));
    let input=body["input"].as_str().or_else(||body.pointer("/messages/0/content").and_then(Value::as_str)).unwrap_or("");
    if input=="hold" {mock.started.notify_one();mock.release.notified().await;}
    if input=="timeout" {tokio::time::sleep(std::time::Duration::from_millis(1300)).await;}
    if input=="slowbody" {let bytes=bytes::Bytes::from_static(br#"{"choices":[{"message":{"role":"assistant","content":"probe-ok"}}]}"#);let stream=futures_util::stream::once(async move{tokio::time::sleep(std::time::Duration::from_millis(1300)).await;Ok::<_,std::io::Error>(bytes)});return Response::new(axum::body::Body::from_stream(stream));}
    if input=="error" {return(StatusCode::TOO_MANY_REQUESTS,Json(json!({"error":{"message":"rate limit fixed-upstream-secret private-header-value"}}))).into_response();}
    if input=="redirect" {return(StatusCode::TEMPORARY_REDIRECT,[("location","/v1/leak")],"redirect").into_response();}
    if input=="oversize" {return Json(json!({"choices":[{"message":{"role":"assistant","content":"x".repeat(300000)}}]})).into_response();}
    if input=="echo-only" {return Json(json!({"model":"needle","echo":"needle","choices":[{"message":{"role":"assistant","content":"different answer"}}]})).into_response();}
    match uri.path() {
        "/v1/embeddings"=>Json(json!({"object":"list","data":[{"index":0,"embedding":[0.1,0.2,0.3]}]})).into_response(),
        "/v1/messages"=>Json(json!({"id":"msg_fixture","type":"message","role":"assistant","model":"wire-model","content":[{"type":"text","text":"probe-ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}})).into_response(),
        "/v1/responses"=>Json(json!({"id":"resp_fixture","object":"response","status":"completed","output":[{"id":"msg_fixture","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"probe-ok","annotations":[]}]}]})).into_response(),
        _=>Json(json!({"choices":[{"message":{"role":"assistant","content":"probe-ok"},"finish_reason":"stop"}]})).into_response(),
    }
}
struct Fixture {state:SharedState,base:String,token:String,source:i64,mock:Mock,tasks:Vec<tokio::task::JoinHandle<()>>}
impl Drop for Fixture {fn drop(&mut self){for task in &self.tasks{task.abort();}}}
async fn serve(router:Router)->(String,tokio::task::JoinHandle<()>) {let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}",listener.local_addr().unwrap());(url,tokio::spawn(async move{axum::serve(listener,router).await.unwrap()}))}
impl Fixture {
    async fn new()->Self {
        let dir=std::env::temp_dir().join(format!("custom-probes-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("key").to_string_lossy().into(),..Default::default()}).unwrap();
        db::sessions::set_panel_password(&state.conn(),"fixture-password-123").unwrap();let token=db::sessions::create_session(&state.conn(),None).unwrap();
        let mock=Mock::default();let(up,t1)=serve(Router::new().route("/v1/{*path}",post(upstream)).with_state(mock.clone())).await;
        let(base,t2)=serve(app(state.clone())).await;
        let mut f=Self{state,base,token,source:0,mock,tasks:vec![t1,t2]};
        let(status,v)=f.req("POST","/api/channels",json!({"name":"probe source","base_url":format!("{up}/v1"),"key":"fixed-upstream-secret","headers":[{"name":"X-Trace","value":"private-header-value"}],"models":[{"model":"local-chat","upstream":"wire-model"},{"model":"local-vector","upstream":"wire-vector"}],"protocols":{"native":["chat","responses","messages"],"allow_conversion":false}})).await;assert_eq!(status,200,"{v}");f.source=v["id"].as_i64().unwrap();
        let(_,p)=f.req("POST","/api/providers",json!({"name":"probe provider"})).await;let pid=p["provider"]["id"].as_i64().unwrap();assert_eq!(f.req("PUT",&format!("/api/channels/{}/provider",f.source),json!({"provider_id":pid})).await.0,200);
        db::kinds::set(&f.state.conn(),"local-chat",tokengo_shared::ModelKind::Chat).unwrap();db::kinds::set(&f.state.conn(),"local-vector",tokengo_shared::ModelKind::Embeddings).unwrap();f
    }
    async fn req(&self,method:&str,path:&str,body:Value)->(u16,Value){let response=reqwest::Client::new().request(method.parse().unwrap(),format!("{}{path}",self.base)).bearer_auth(&self.token).json(&body).send().await.unwrap();let status=response.status().as_u16();(status,response.json().await.unwrap_or(Value::Null))}
    fn config(&self)->Value {json!({"name":"fixture probe","source_id":self.source,"model":"local-chat","kind":"chat","input":"ping","max_tokens":8,"expected_status":200,"contains":"probe-ok","expected_dimensions":null,"timeout_ms":1000,"max_latency_ms":null,"interval_secs":60,"enabled":false})}
    async fn create(&self,config:Value)->Value {let(status,v)=self.req("POST","/api/custom-probes",config).await;assert_eq!(status,200,"{v}");v["probe"].clone()}
    async fn run(&self,p:&Value)->Value {let(status,v)=self.req("POST",&format!("/api/custom-probes/{}/run",p["id"].as_str().unwrap()),json!(null)).await;assert_eq!(status,200,"{v}");v["result"].clone()}
}
fn protected(c:&rusqlite::Connection)->Vec<(String,Vec<Vec<String>>)> {
    ["users","gateway_keys","channels","channel_models","channel_secrets","channel_headers","provider_credentials","provider_connections","requests","attempts","billing_ledger","billing_budgets","billing_prices","billing_request_prices","routing_state","model_cooldowns","channel_faults"].into_iter().map(|table|{let mut q=c.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();let columns=q.column_count();let rows=q.query_map([],|row|Ok((0..columns).map(|i|format!("{:?}",row.get_ref(i).unwrap())).collect())).unwrap().collect::<rusqlite::Result<Vec<Vec<String>>>>().unwrap();(table.into(),rows)}).collect()
}
#[tokio::test]
async fn custom_probe_crud_optimistic_versions_persist_without_dispatch() {
    let f=Fixture::new().await;let p=f.create(f.config()).await;assert_eq!(p["version"],1);assert!(p["last_result"].is_null());assert_eq!(f.mock.calls.load(Ordering::SeqCst),0);
    let(_,all)=f.req("GET","/api/custom-probes",json!(null)).await;assert_eq!(all["probes"].as_array().unwrap().len(),1);assert!(all["eligible_source_ids"].as_array().unwrap().contains(&json!(f.source)));
    let id=p["id"].as_str().unwrap();let mut changed=p.clone();changed["name"]=json!("renamed");let(status,v)=f.req("PUT",&format!("/api/custom-probes/{id}"),changed).await;assert_eq!(status,200,"{v}");assert_eq!(v["probe"]["version"],2);
    assert_eq!(f.req("PUT",&format!("/api/custom-probes/{id}"),p.clone()).await.0,409);assert_eq!(f.req("DELETE",&format!("/api/custom-probes/{id}"),json!({"version":1})).await.0,409);
    assert_eq!(custom_probes::list(&f.state.conn()).unwrap()[0].config.name,"renamed");
    assert_eq!(f.req("DELETE",&format!("/api/custom-probes/{id}"),json!({"version":2})).await.0,200);assert!(custom_probes::list(&f.state.conn()).unwrap().is_empty());assert_eq!(f.mock.calls.load(Ordering::SeqCst),0);
}
#[tokio::test]
async fn custom_probe_native_shapes_mapping_contains_dimensions_and_no_ledger_or_fallback_writes() {
    let f=Fixture::new().await;let before=protected(&f.state.conn());
    for kind in ["chat","responses","messages","embeddings"] {let mut config=f.config();config["kind"]=json!(kind);if kind=="embeddings"{config["model"]=json!("local-vector");config["contains"]=Value::Null;config["expected_dimensions"]=json!(3);}let probe=f.create(config).await;let result=f.run(&probe).await;assert_eq!(result["ok"],true,"{kind}: {result}");assert_eq!(result["status"],200);}
    let captures=f.mock.captures.lock().unwrap();assert_eq!(captures.len(),4);assert_eq!(captures[0].3["model"],"wire-model");assert_eq!(captures[0].3["max_tokens"],8);assert_eq!(captures[1].3["max_output_tokens"],8);assert_eq!(captures[1].3["store"],false);assert_eq!(captures[2].2,"fixed-upstream-secret");assert_eq!(captures[2].1,"");assert_eq!(captures[3].3["model"],"wire-vector");drop(captures);
    assert_eq!(protected(&f.state.conn()),before,"custom probes must not alter history, identity or fallback state");
    let mut config=f.config();config["input"]=json!("echo-only");config["contains"]=json!("needle");assert_eq!(f.run(&f.create(config).await).await["ok"],false,"metadata or reflected input must not satisfy assistant output matcher");
    let mut config=f.config();config["kind"]=json!("embeddings");config["model"]=json!("local-vector");config["contains"]=Value::Null;config["expected_dimensions"]=json!(4);assert_eq!(f.run(&f.create(config).await).await["ok"],false);
}
#[tokio::test]
async fn custom_probe_status_timeout_body_limit_redirect_and_safe_results() {
    let f=Fixture::new().await;
    for input in ["timeout","slowbody","oversize"] {let mut config=f.config();config["input"]=json!(input);let p=f.create(config).await;let start=std::time::Instant::now();let result=f.run(&p).await;assert_eq!(result["ok"],false,"{input}: {result}");assert!(start.elapsed()<std::time::Duration::from_secs(2));}
    let mut config=f.config();config["input"]=json!("error");config["expected_status"]=json!(429);config["contains"]=json!("rate limit");let result=f.run(&f.create(config).await).await;assert_eq!(result["ok"],true);assert!(!result.to_string().contains("fixed-upstream-secret"));assert!(!result.to_string().contains("private-header-value"));
    let mut config=f.config();config["input"]=json!("redirect");config["expected_status"]=json!(307);config["contains"]=Value::Null;let before=f.mock.calls.load(Ordering::SeqCst);let result=f.run(&f.create(config).await).await;assert_eq!(result["ok"],true);assert_eq!(f.mock.calls.load(Ordering::SeqCst),before+1,"redirect must not dispatch credentials to another path");
}
#[tokio::test]
async fn custom_probe_reentry_disabled_deleted_inflight_and_source_removal_are_safe() {
    let f=Fixture::new().await;let mut config=f.config();config["input"]=json!("hold");config["timeout_ms"]=json!(5000);let p=f.create(config).await;let id=p["id"].as_str().unwrap().to_string();
    let state=f.state.clone();let run_id=id.clone();let task=tokio::spawn(async move{custom_probes::execute(&state,&run_id,false,db::now_ms()).await.unwrap()});f.mock.started.notified().await;
    assert_eq!(f.req("POST",&format!("/api/custom-probes/{id}/run"),json!(null)).await.0,409);
    let mut disabled=p.clone();disabled["enabled"]=json!(false);let(status,v)=f.req("PUT",&format!("/api/custom-probes/{id}"),disabled).await;assert_eq!(status,200,"{v}");
    f.mock.release.notify_one();task.await.unwrap();assert!(custom_probes::list(&f.state.conn()).unwrap()[0].last_result.is_none(),"old result cannot overwrite updated configuration");
    let mut updated=v["probe"].clone();updated["input"]=json!("hold");let(_,v)=f.req("PUT",&format!("/api/custom-probes/{id}"),updated).await;
    let state=f.state.clone();let run_id=id.clone();let task=tokio::spawn(async move{custom_probes::execute(&state,&run_id,false,db::now_ms()).await.unwrap()});f.mock.started.notified().await;
    assert_eq!(f.req("DELETE",&format!("/api/custom-probes/{id}"),json!({"version":v["probe"]["version"]})).await.0,200);f.mock.release.notify_one();task.await.unwrap();assert!(custom_probes::list(&f.state.conn()).unwrap().is_empty(),"finishing deleted probe must never recreate it");
    let mut binding=f.config();binding["input"]=json!("hold");binding["timeout_ms"]=json!(5000);let binding=f.create(binding).await;let binding_id=binding["id"].as_str().unwrap().to_string();let state=f.state.clone();
    let task=tokio::spawn(async move{custom_probes::execute(&state,&binding_id,false,db::now_ms()).await.unwrap()});f.mock.started.notified().await;
    db::channels::set_enabled(&f.state.conn(),tokengo_shared::ChannelId(f.source),false).unwrap();f.mock.release.notify_one();assert!(!task.await.unwrap().ok,"an old source response cannot validate a newly disabled binding");
    let before=f.mock.calls.load(Ordering::SeqCst);assert_eq!(f.run(&binding).await["ok"],false);assert_eq!(f.mock.calls.load(Ordering::SeqCst),before);db::channels::set_enabled(&f.state.conn(),tokengo_shared::ChannelId(f.source),true).unwrap();
    let p=f.create(f.config()).await;assert_eq!(f.req("DELETE",&format!("/api/channels/{}",f.source),json!(null)).await.0,200);let before=f.mock.calls.load(Ordering::SeqCst);let result=f.run(&p).await;assert_eq!(result["ok"],false);assert!(result["note"].as_str().unwrap().contains("已删除"));assert_eq!(f.mock.calls.load(Ordering::SeqCst),before);
    let mut disabled=p;disabled["enabled"]=json!(false);assert_eq!(f.req("PUT",&format!("/api/custom-probes/{}",disabled["id"].as_str().unwrap()),disabled.clone()).await.0,200);
}
#[tokio::test]
async fn custom_probe_scheduler_only_runs_due_enabled_and_slow_probe_does_not_block_other_due_probes() {
    let f=Fixture::new().await;let mut slow=f.config();slow["enabled"]=json!(true);slow["input"]=json!("hold");slow["timeout_ms"]=json!(5000);let slow=f.create(slow).await;let mut fast=f.config();fast["enabled"]=json!(true);let fast=f.create(fast).await;let disabled=f.create(f.config()).await;
    {let c=f.state.conn();for p in [&slow,&fast,&disabled] {let mut probe=custom_probes::list(&c).unwrap().into_iter().find(|row|row.id==p["id"]).unwrap();probe.next_run_at_ms=0;db::settings::set(&c,&format!("custom_probe:{}",probe.id),&serde_json::to_string(&probe).unwrap()).unwrap();}}
    let state=f.state.clone();let task=tokio::spawn(async move{custom_probes::tick(&state,db::now_ms()).await});f.mock.started.notified().await;
    tokio::time::timeout(std::time::Duration::from_millis(700),async{loop{if custom_probes::list(&f.state.conn()).unwrap().iter().find(|p|p.id==fast["id"]).unwrap().last_result.is_some(){break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.expect("fast due probe starved behind slow probe");
    f.mock.release.notify_one();assert_eq!(task.await.unwrap(),2);assert_eq!(f.mock.calls.load(Ordering::SeqCst),2);assert_eq!(custom_probes::tick(&f.state,db::now_ms()).await,0);assert!(custom_probes::list(&f.state.conn()).unwrap().iter().find(|p|p.id==disabled["id"]).unwrap().last_result.is_none());
}
#[tokio::test]
async fn custom_probe_root_only_csrf_and_invalid_configuration_do_not_dispatch() {
    let f=Fixture::new().await;let client=reqwest::Client::new();assert_eq!(client.get(format!("{}/api/custom-probes",f.base)).send().await.unwrap().status(),401);
    let root=db::accounts::root(&f.state.conn()).unwrap().unwrap();let session=db::accounts::create_session(&f.state.conn(),&root).unwrap();
    let r=client.post(format!("{}/api/custom-probes",f.base)).header("cookie",format!("tokengo_account_session={session}")).json(&f.config()).send().await.unwrap();assert!(matches!(r.status().as_u16(),401|403),"cookie writes require CSRF");
    let cookie=format!("tokengo_account_session={session}");
    assert_eq!(client.get(format!("{}/api/custom-probes",f.base)).header("cookie",&cookie).send().await.unwrap().status(),200,"valid root browser session must authenticate");
    assert_eq!(client.post(format!("{}/api/custom-probes",f.base)).header("cookie",&cookie).header("x-tokengo-panel","1").json(&f.config()).send().await.unwrap().status(),200);
    let user_session={let c=f.state.conn();let policy:db::identity::Policy=serde_json::from_value(json!({})).unwrap();let user=db::identity::create_user(&c,"ordinary probe user","user",&policy).unwrap();let account=db::accounts::create(&c,"ordinary-probe","Ordinary",user.id,&root.hash).unwrap();let credentials=db::accounts::by_id(&c,account.id).unwrap().unwrap();db::accounts::create_session(&c,&credentials).unwrap()};
    let cookie=format!("tokengo_account_session={user_session}");
    for method in ["GET","POST"] {assert_eq!(client.request(method.parse().unwrap(),format!("{}/api/custom-probes",f.base)).header("cookie",&cookie).header("x-tokengo-panel","1").json(&f.config()).send().await.unwrap().status(),403,"ordinary panel users must not manage diagnostics");}
    for(field,value)in [("max_tokens",json!(257)),("timeout_ms",json!(999)),("interval_secs",json!(59)),("model",json!("auto")),("source_id",json!(999999)),("input",json!("x".repeat(4097))),("kind",json!("search"))] {let mut config=f.config();config[field]=value;let(status,v)=f.req("POST","/api/custom-probes",config).await;assert_eq!(status,400,"{field}: {v}");}
    let mut wrong_kind=f.config();wrong_kind["kind"]=json!("embeddings");wrong_kind["contains"]=Value::Null;assert_eq!(f.req("POST","/api/custom-probes",wrong_kind).await.0,400);
    assert_eq!(f.mock.calls.load(Ordering::SeqCst),0);
}
