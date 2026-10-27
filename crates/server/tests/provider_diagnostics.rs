//! W10 explicit compatibility diagnostics; loopback fixtures contain no real credentials.
use axum::{routing::post, Json, Router};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokengo_server::{app, db, AppState, Config, SharedState};
struct Fixture {
    state: SharedState,
    cfg: Config,
    base: String,
    token: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(dir)=self.cfg.db_path.parent() {
            if dir.file_name().is_some_and(|name|name.to_string_lossy().starts_with("provider-test-")) {
                let _=std::fs::remove_dir_all(dir);
            }
        }
    }
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move { axum::serve(l, router).await.unwrap() }),
    )
}
impl Fixture {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("provider-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            ..Config::default()
        };
        let state = AppState::init(cfg.clone()).unwrap();
        db::sessions::set_panel_password(&state.conn(), "test-password-1234").unwrap();
        let token = db::sessions::create_session(&state.conn(), None).unwrap();
        db::sessions::set_gateway_key(&state.conn(), &state.secrets, "provider-test-key").unwrap();
        let mut c = state.routing().cfg.clone();
        c.retry_times = 0;
        c.probe_enabled = false;
        db::routing::save_config(&state.conn(), &c).unwrap();
        state.routing().cfg = c;
        let (base, task) = serve(app(state.clone())).await;
        Self {
            state,
            cfg,
            base,
            token,
            task,
        }
    }
    async fn req(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let r = reqwest::Client::new()
            .request(method.parse().unwrap(), format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn channel(&self, base: &str) -> i64 {
        let(s,v)=self.req("POST","/api/channels",json!({"name":"connection","base_url":base,"key":"fixed-upstream-secret","models":[{"model":"m","upstream":"exact-M","vision":true,"drop_fields":"unused"},{"model":"other","upstream":"other-exact"}]})).await;
        assert_eq!(s, 200, "{v}");
        v["id"].as_i64().unwrap()
    }
}

#[tokio::test]
async fn static_diagnostics_are_bounded_and_do_not_probe_or_mutate() {
    let calls=std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed=calls.clone();
    let (up,task)=serve(Router::new().fallback(move || {let observed=observed.clone();async move {observed.fetch_add(1,std::sync::atomic::Ordering::SeqCst);Json(json!({"error":{"message":"fixed-upstream-secret"}}))}})).await;
    let f=Fixture::new().await; let id=f.channel(&up).await;
    let before:i64=f.state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get(0)).unwrap();
    let(s,v)=f.req("GET",&format!("/api/channels/{id}/models/compatibility?model=m"),Value::Null).await;
    assert_eq!(s,200,"{v}");assert_eq!(v["schema_version"],1);assert_eq!(v["identity"]["upstream_model"],"exact-M");
    assert_eq!(v["availability"]["state"],"never");assert_eq!(v["evidence"]["sample"],"not_asserted");
    assert!(!v.to_string().contains("fixed-upstream-secret"));assert!(!v.to_string().contains(&up));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),0);
    assert_eq!(before,f.state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get::<_,i64>(0)).unwrap());task.abort();
}
#[tokio::test]
async fn verification_is_explicit_deduplicated_cancelable_and_stale() {
    let(up,task)=serve(Router::new().route("/chat/completions",post(||async {tokio::time::sleep(std::time::Duration::from_secs(3)).await;Json(json!({"choices":[{"message":{"role":"assistant","content":"ok"}}]}))}))).await;
    let f=Fixture::new().await;let id=f.channel(&up).await;
    let path=format!("/api/channels/{id}/models/compatibility/verify");
    let(s,job)=f.req("POST",&path,json!({"model":"m","native":"chat"})).await;assert_eq!(s,200,"{job}");assert_eq!(job["state"],"running");
    let(s,_)=f.req("POST",&path,json!({"model":"m","native":"chat"})).await;assert_eq!(s,409);
    let jp=format!("/api/channels/{id}/models/compatibility/jobs/{}",job["id"].as_str().unwrap());
    let(s,v)=f.req("DELETE",&jp,Value::Null).await;assert_eq!(s,200,"{v}");assert_eq!(v["state"],"canceled");
    f.state.conn().execute("UPDATE channels SET binding_generation=binding_generation+1 WHERE id=?1",[id]).unwrap();
    let(s,v)=f.req("GET",&jp,Value::Null).await;assert_eq!(s,200,"{v}");assert_eq!(v["state"],"stale");task.abort();
}
#[tokio::test]
async fn diagnostics_require_management_auth() {
 let f=Fixture::new().await;let id=f.channel("http://127.0.0.1:9").await;
 let r=reqwest::Client::new().get(format!("{}/api/channels/{id}/models/compatibility?model=m",f.base)).send().await.unwrap();assert_eq!(r.status(),401);
}
#[tokio::test]
async fn inference_result_is_source_bound_and_never_echoes_provider_text() {
 let(up,task)=serve(Router::new().route("/chat/completions",post(||async {(axum::http::StatusCode::BAD_REQUEST,Json(json!({"error":{"message":"fixed-upstream-secret https://private.invalid?token=hidden"}})))}))).await;
 let f=Fixture::new().await;let id=f.channel(&up).await;
 let(s,j)=f.req("POST",&format!("/api/channels/{id}/models/compatibility/verify"),json!({"model":"m"})).await;assert_eq!(s,200,"{j}");
 let path=format!("/api/channels/{id}/models/compatibility/jobs/{}",j["id"].as_str().unwrap());
 let mut last=Value::Null;for _ in 0..100 {let(_,v)=f.req("GET",&path,Value::Null).await;last=v;if last["state"]!="running"{break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
 assert_eq!(last["state"],"failed","{last}");assert_eq!(last["http_status"],400);assert!(!last.to_string().contains("private.invalid"));assert!(!last.to_string().contains("fixed-upstream-secret"));
 db::channels::set_secret(&f.state.conn(),&f.state.secrets,tokengo_shared::ChannelId(id),"new-fixed-key").unwrap();
 let(_,v)=f.req("GET",&path,Value::Null).await;assert_eq!(v["state"],"stale");task.abort();
}
#[tokio::test]
async fn runtime_collection_is_unverified_candidate_and_does_not_save_caps() {
 for kind in [tokengo_shared::ModelKind::Embeddings,tokengo_shared::ModelKind::Rerank] {
 let(up,task)=serve(Router::new().route("/nested/api/version",axum::routing::get(||async{Json(json!({"version":"0.13.3"}))})).route("/nested/api/show",post(||async{Json(json!({"template":"private raw template","capabilities":["tools","thinking"]}))}))).await;
 let f=Fixture::new().await;let id=f.channel(&format!("{up}/nested/v1")).await;
 let generation:i64=f.state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get(0)).unwrap();
 let(s,v)=f.req("PUT",&format!("/api/channels/{id}/protocols"),json!({"expected_binding_generation":generation,"protocols":{"native":["chat"],"parameter_profile":"ollama"}})).await;assert_eq!(s,200,"{v}");
 db::kinds::set(&f.state.conn(),"m",kind).unwrap();
 let(s,v)=f.req("POST",&format!("/api/channels/{id}/models/compatibility/verify"),json!({"model":"m","mode":"inference"})).await;assert_eq!(s,400,"Specialized inference remains on its actual endpoint: {v}");
 let before=db::settings::get(&f.state.conn(),&format!("channel_protocols:{id}")).unwrap();
 let(s,j)=f.req("POST",&format!("/api/channels/{id}/models/compatibility/verify"),json!({"model":"m","mode":"runtime_metadata"})).await;assert_eq!(s,200,"{j}");
 let path=format!("/api/channels/{id}/models/compatibility/jobs/{}",j["id"].as_str().unwrap());
 let mut last=Value::Null;for _ in 0..100 {let(_,v)=f.req("GET",&path,Value::Null).await;last=v;if last["state"]!="running"{break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
 assert_eq!(last["state"],"passed","{last}");assert_eq!(last["candidate"]["status"],"unverified");assert_eq!(last["candidate"]["capabilities"],"unknown");assert_eq!(last["candidate"]["origin_verified"],false);assert!(!last.to_string().contains("private raw template"));
 assert_eq!(before,db::settings::get(&f.state.conn(),&format!("channel_protocols:{id}")).unwrap());task.abort();
}
}

#[tokio::test]
async fn verification_total_deadline_bounds_the_entire_request() {
 let(up,task)=serve(Router::new().route("/chat/completions",post(||async {tokio::time::sleep(std::time::Duration::from_secs(30)).await;Json(json!({"choices":[{"message":{"role":"assistant","content":"late"}}]}))}))).await;
 let f=Fixture::new().await;let id=f.channel(&up).await;
 let start=std::time::Instant::now();let(s,j)=f.req("POST",&format!("/api/channels/{id}/models/compatibility/verify"),json!({"model":"m"})).await;assert_eq!(s,200,"{j}");
 assert_eq!(j["deadline_at_ms"].as_i64().unwrap()-j["started_at_ms"].as_i64().unwrap(),15_000);
 let path=format!("/api/channels/{id}/models/compatibility/jobs/{}",j["id"].as_str().unwrap());
 tokio::time::sleep(std::time::Duration::from_millis(15_150)).await;
 let(_,v)=f.req("GET",&path,Value::Null).await;assert_eq!(v["state"],"failed","{v}");assert!(start.elapsed()<std::time::Duration::from_secs(20));task.abort();
}
#[tokio::test]
async fn stale_expected_identity_and_unknown_fields_never_start_network() {
 let f=Fixture::new().await;let id=f.channel("http://127.0.0.1:9").await;
 let(_,v)=f.req("GET",&format!("/api/channels/{id}/models/compatibility?model=m"),Value::Null).await;
 f.state.conn().execute("UPDATE channels SET binding_generation=binding_generation+1 WHERE id=?1",[id]).unwrap();
 let(s,_)=f.req("POST",&format!("/api/channels/{id}/models/compatibility/verify"),json!({"model":"m","expected_identity":v["identity"]})).await;assert_eq!(s,409);
 let(s,_)=f.req("POST",&format!("/api/channels/{id}/models/compatibility/verify"),json!({"model":"m","arbitrary_body":{"secret":"never-forward"}})).await;assert_eq!(s,400);
}

#[tokio::test]
async fn jobs_enforce_channel_scope_and_cookie_csrf_for_writes() {
 let f=Fixture::new().await;let a=f.channel("http://127.0.0.1:9").await;let b=f.channel("http://127.0.0.1:8").await;
 let path=format!("/api/channels/{a}/models/compatibility/verify");
 let root=db::accounts::root(&f.state.conn()).unwrap().unwrap();let session=db::accounts::create_session(&f.state.conn(),&root).unwrap();
 let cookie=format!("tokengo_account_session={session}");let client=reqwest::Client::new();
 let response=client.post(format!("{}{path}",f.base)).header("cookie",&cookie).json(&json!({"model":"m"})).send().await.unwrap();assert!(matches!(response.status().as_u16(),401|403));
 let(s,j)=f.req("POST",&path,json!({"model":"m"})).await;assert_eq!(s,200,"{j}");let job=j["id"].as_str().unwrap();
 for method in ["GET","DELETE"] {let(s,_)=f.req(method,&format!("/api/channels/{b}/models/compatibility/jobs/{job}"),Value::Null).await;assert_eq!(s,404);}
 let own=format!("/api/channels/{a}/models/compatibility/jobs/{job}");
 let response=client.delete(format!("{}{own}",f.base)).header("cookie",&cookie).send().await.unwrap();assert!(matches!(response.status().as_u16(),401|403));
 let response=client.get(format!("{}{own}",f.base)).send().await.unwrap();assert_eq!(response.status(),401);
 let(s,_)=f.req("DELETE",&own,Value::Null).await;assert_eq!(s,200);
}

#[tokio::test]
async fn native_inference_rejects_http_200_errors_and_accepts_only_valid_protocol_bodies() {
 for (native,body,expected) in [
  ("chat",json!({"error":{"message":"synthetic private upstream failure"}}),"failed"),
  ("chat",json!({}),"failed"),
  ("chat",json!({"choices":[{}]}),"failed"),
  ("chat",json!({"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}),"passed"),
  ("responses",json!({"error":{"message":"synthetic private upstream failure"}}),"failed"),
  ("responses",json!({"object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}]}),"passed"),
  ("responses",json!({"object":"response","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"o"}]}]}),"passed"),
  ("messages",json!({"error":{"message":"synthetic private upstream failure"}}),"failed"),
  ("messages",json!({"type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"max_tokens"}),"passed"),
 ] {
  let source=body.clone();let(up,task)=serve(Router::new().fallback(move||{let body=source.clone();async move{Json(body)}})).await;
  let f=Fixture::new().await;let id=f.channel(&up).await;
  let generation:i64=f.state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get(0)).unwrap();
  let(s,v)=f.req("PUT",&format!("/api/channels/{id}/protocols"),json!({"expected_binding_generation":generation,"protocols":{"native":[native]}})).await;assert_eq!(s,200,"{v}");
  let(s,j)=f.req("POST",&format!("/api/channels/{id}/models/compatibility/verify"),json!({"model":"m","native":native})).await;assert_eq!(s,200,"{j}");
  let path=format!("/api/channels/{id}/models/compatibility/jobs/{}",j["id"].as_str().unwrap());
  let mut last=Value::Null;for _ in 0..100 {let(_,v)=f.req("GET",&path,Value::Null).await;last=v;if last["state"]!="running"{break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
  assert_eq!(last["state"],expected,"{native}: {body}: {last}");assert!(!last.to_string().contains("synthetic private upstream failure"));task.abort();
 }
}

#[tokio::test]
async fn dashscope_responses_uses_legal_api_minimum_and_estimates_unverified_native_budgets() {
 for (model,expected) in [("qwen3.8-max",200),("qwen-future-unknown",200),("qwen3.7-max",200)] {
  let sent=std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));let observed=sent.clone();
  let(up,task)=serve(Router::new().route("/responses",post(move|Json(body):Json<Value>|{let observed=observed.clone();async move {
   observed.lock().unwrap().push(body);Json(json!({"object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}]}))
  }}))).await;
  let f=Fixture::new().await;
  let(s,v)=f.req("POST","/api/channels",json!({"name":"dashscope exact contract","base_url":up,"key":"fixed-upstream-secret","models":[{"model":"alias","upstream":model}]})).await;assert_eq!(s,200,"{v}");let id=v["id"].as_i64().unwrap();
  let generation:i64=f.state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get(0)).unwrap();
  let(s,v)=f.req("PUT",&format!("/api/channels/{id}/protocols"),json!({"expected_binding_generation":generation,"protocols":{"native":["responses"],"parameter_profile":"dashscope"}})).await;assert_eq!(s,200,"{v}");
  let(s,description)=f.req("GET",&format!("/api/channels/{id}/models/compatibility?model=alias"),Value::Null).await;assert_eq!(s,200,"{description}");
  let fields=description["fields"].as_array().unwrap();let cap=fields.iter().find(|field|field["field"]=="max_output_tokens").unwrap();
  if model=="qwen3.8-max" {assert_eq!(cap["rule_ids"],json!(["G01"]));assert_eq!(cap["rule_match"],"matched");assert_eq!(cap["status"],"supported_native");}
  if model=="qwen-future-unknown" {assert_eq!(cap["rule_ids"],json!([]));assert_eq!(cap["rule_match"],"none");assert_eq!(cap["status"],"unknown");}
  let background=fields.iter().find(|field|field["field"]=="background").unwrap();assert_eq!(background["rule_ids"],json!(["stateful_executor"]));assert_eq!(background["rule_source"],"gateway_executor");
  let(s,j)=f.req("POST",&format!("/api/channels/{id}/models/compatibility/verify"),json!({"model":"alias","native":"responses"})).await;assert_eq!(s.as_u16(),expected,"{model}: {j}");
  assert_eq!(j["reservation_estimate"]["supplier_hard_cap_verified"],false);
  if model!="qwen3.8-max" {assert_eq!(j["reservation_estimate"]["output_tokens"],4096);assert_eq!(j["reservation_estimate"]["origin"],"gateway_default");assert_eq!(j["reservation_estimate"]["confidence"],"estimate_not_upstream_bound");}
  if expected==200 {
   let path=format!("/api/channels/{id}/models/compatibility/jobs/{}",j["id"].as_str().unwrap());
   let mut last=Value::Null;for _ in 0..100 {let(_,v)=f.req("GET",&path,Value::Null).await;last=v;if last["state"]!="running"{break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
   assert_eq!(last["state"],"passed","{last}");assert_eq!(last["reservation_estimate"]["supplier_hard_cap_verified"],false);assert!(last["note"].as_str().unwrap().contains("本地估算"));let requests=sent.lock().unwrap();assert_eq!(requests.len(),1);assert_eq!(requests[0]["max_output_tokens"],16);assert_eq!(requests[0]["model"],model);
  }
  task.abort();
 }
}
#[tokio::test]
async fn unknown_sources_remain_usable_and_oauth_tools_offer_the_authenticated_existing_entry() {
 let f=Fixture::new().await;let source=f.channel("http://127.0.0.1:9").await;
 let path=format!("/api/channels/{source}/models/compatibility?model=m");
 let(s,v)=f.req("GET",&path,Value::Null).await;assert_eq!(s,200,"{v}");assert_eq!(v["verification_lane"]["kind"],"direct_native");assert_eq!(v["verification_lane"]["direct_native_inference"],true);assert_eq!(v["reasoning"]["status"],"unknown");
 db::kinds::set(&f.state.conn(),"m",tokengo_shared::ModelKind::Rerank).unwrap();
 let(s,description)=f.req("GET",&path,Value::Null).await;assert_eq!(s,200,"{description}");assert_eq!(description["verification_lane"]["kind"],"specialized_gateway");assert_eq!(description["verification_lane"]["recommended_kind"],"rerank");assert_eq!(description["verification_lane"]["direct_native_inference"],false);
 db::kinds::set(&f.state.conn(),"m",tokengo_shared::ModelKind::Unknown).unwrap();
 tokengo_server::search_tools::save(&f.state.conn(),source,&tokengo_server::search_tools::ToolCapabilities{search:true,fetch:false,account_scope:"synthetic-diagnostic-tool-account".into(),qps:10}).unwrap();
 let(s,v)=f.req("GET",&path,Value::Null).await;assert_eq!(s,200,"{v}");assert_eq!(v["verification_lane"]["kind"],"standalone_tool");assert_eq!(v["verification_lane"]["recommended_kind"],"search");assert_eq!(v["verification_lane"]["management_page"],"probes");assert_eq!(v["verification_lane"]["management_hash"],"#probes");assert_eq!(v["verification_lane"]["source_id"],source);assert_eq!(v["verification_lane"]["model"],"m");
 let source=f.channel("http://127.0.0.1:8").await;
 tokengo_server::protocol_config::save(&f.state.conn(),source,&tokengo_server::protocol_config::Capabilities{native:vec![tokengo_server::protocol::Protocol::Responses],..Default::default()}).unwrap();
 {let c=f.state.conn();c.execute("INSERT INTO oauth_accounts(label,issuer,subject,client_id,status,scopes_json,expires_at,earliest_refresh_at) VALUES('synthetic','https://auth.openai.com','synthetic','synthetic','active','[]',9999999999999,9999999999999)",[]).unwrap();let account=c.last_insert_rowid();c.execute("INSERT INTO oauth_channel_bindings(channel_id,account_id) VALUES(?1,?2)",rusqlite::params![source,account]).unwrap();}
 let(s,v)=f.req("GET",&format!("/api/channels/{source}/models/compatibility?model=m"),Value::Null).await;assert_eq!(s,200,"{v}");assert_eq!(v["verification_lane"]["kind"],"authenticated_gateway");assert_eq!(v["verification_lane"]["recommended_kind"],"responses");assert_eq!(v["verification_lane"]["access"],"root_session");assert_eq!(v["verification_lane"]["direct_native_inference"],false);
}

#[tokio::test]
async fn embeddings_lane_opens_a_real_existing_probe_and_executes_embedding_shape() {
 let sent=std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));let observed=sent.clone();
 let(up,task)=serve(Router::new().route("/embeddings",post(move|Json(body):Json<Value>|{let observed=observed.clone();async move{observed.lock().unwrap().push(body);Json(json!({"object":"list","data":[{"index":0,"embedding":[0.1,0.2,0.3]}],"usage":{"prompt_tokens":1,"total_tokens":1}}))}}))).await;
 let f=Fixture::new().await;let source=f.channel(&up).await;
 let(s,p)=f.req("POST","/api/providers",json!({"name":"embedding diagnostic provider"})).await;assert_eq!(s,200,"{p}");let provider=p["provider"]["id"].as_i64().unwrap();
 let(s,v)=f.req("PUT",&format!("/api/channels/{source}/provider"),json!({"provider_id":provider})).await;assert_eq!(s,200,"{v}");
 db::kinds::set(&f.state.conn(),"m",tokengo_shared::ModelKind::Embeddings).unwrap();
 let(s,v)=f.req("GET",&format!("/api/channels/{source}/models/compatibility?model=m"),Value::Null).await;assert_eq!(s,200,"{v}");let lane=&v["verification_lane"];
 assert_eq!(lane["kind"],"specialized_gateway");assert_eq!(lane["recommended_kind"],"embeddings");assert_eq!(lane["direct_native_inference"],false);assert_eq!(lane["management_hash"],"#probes");assert_eq!(lane["source_id"],source);assert_eq!(lane["model"],"m");
 let(s,p)=f.req("POST","/api/custom-probes",json!({"name":"exact embedding diagnostic","source_id":source,"model":"m","kind":"embeddings","input":"x","max_tokens":8,"expected_status":200,"contains":null,"expected_dimensions":3,"timeout_ms":1000,"max_latency_ms":null,"interval_secs":60,"enabled":false})).await;assert_eq!(s,200,"{p}");assert!(sent.lock().unwrap().is_empty(),"Saving does not execute");
 let id=p["probe"]["id"].as_str().unwrap();let(s,v)=f.req("POST",&format!("/api/custom-probes/{id}/run"),Value::Null).await;assert_eq!(s,200,"{v}");assert_eq!(v["result"]["ok"],true,"{v}");let requests=sent.lock().unwrap();assert_eq!(requests.len(),1);assert_eq!(requests[0]["model"],"exact-M");assert_eq!(requests[0]["input"],"x");assert!(requests[0].get("messages").is_none());task.abort();
}

#[tokio::test]
async fn configured_catalog_purpose_is_displayed_separately_without_changing_runtime_or_identity() {
 let f=Fixture::new().await;let source=f.channel("http://127.0.0.1:9").await;
 db::kinds::set(&f.state.conn(),"m",tokengo_shared::ModelKind::Chat).unwrap();
 let path=format!("/api/channels/{source}/models/compatibility?model=m");let(s,before)=f.req("GET",&path,Value::Null).await;assert_eq!(s,200,"{before}");assert_eq!(before["purpose"]["configured"],"");assert_eq!(before["purpose"]["configured_source"],"not_configured");
 db::model_access::set_catalog_purpose(&f.state.conn(),"m","chat").unwrap();
 let(s,after)=f.req("GET",&path,Value::Null).await;assert_eq!(s,200,"{after}");assert_eq!(after["purpose"]["catalog_purpose"],"chat");assert_eq!(after["purpose"]["configured"],"chat");assert_eq!(after["purpose"]["label"],"对话与推理");assert_eq!(after["purpose"]["configured_source"],"operator_configured");
 assert_eq!(after["purpose"]["kind"],"chat");assert_eq!(after["purpose"]["source"],before["purpose"]["source"]);assert_eq!(after["identity"],before["identity"]);assert_eq!(after["verification_lane"],before["verification_lane"]);assert_eq!(after["reasoning"],before["reasoning"]);assert_eq!(after["availability"],before["availability"]);
}
