//! Independent real HTTP acceptance of opaque issuance and cancellation/denial ledgers.
use axum::{
    body::Body,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tokengo_shared::ChannelId;
const MODEL: &str = "safety-model";
const FULL: &str = "synthetic-signature-complete";
struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Server {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
    Server { url, task }
}
#[derive(Default)]
struct Source {
    seen: Mutex<Vec<Value>>,
}
fn frame(v: Value) -> String {
    format!("data: {v}\n\n")
}
async fn source(State(s): State<Arc<Source>>, Json(body): Json<Value>) -> Response {
    s.seen.lock().unwrap().push(body.clone());
    let mode = body["x_test_mode"].as_str().unwrap_or("plain");
    if body["stream"] == true {
        let mut out = frame(
            json!({"type":"message_start","message":{"id":"msg_safety","type":"message","role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
        );
        if mode != "wait_pre" {
            out += &frame(
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            );
            out += &frame(
                json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"first text"}}),
            );
        }
        if matches!(mode, "issue" | "prefix") {
            out += &frame(
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"thought","signature":"synthetic-signature-"}}),
            );
            if mode == "issue" {
                out += &frame(
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"complete"}}),
                );
                out += &frame(json!({"type":"content_block_stop","index":0}));
            }
        }
        if mode == "late_issue" {
            let tail = frame(
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","signature":"synthetic-signature-"}}),
            ) + &frame(
                json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"complete"}}),
            ) + &frame(json!({"type":"content_block_stop","index":0}))
                + &frame(json!({"type":"content_block_stop","index":1}))
                + &frame(
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
                )
                + &frame(json!({"type":"message_stop"}));
            let pieces = std::collections::VecDeque::from([
                bytes::Bytes::from(out),
                bytes::Bytes::from(tail),
            ]);
            let stream = futures_util::stream::unfold(pieces, |mut pieces| async move {
                let next = pieces.pop_front()?;
                if pieces.is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                }
                Some((Ok::<_, std::convert::Infallible>(next), pieces))
            });
            return (
                [("content-type", "text/event-stream")],
                Body::from_stream(stream),
            )
                .into_response();
        }
        let pending = mode.starts_with("wait_");
        if !pending {
            out += &frame(json!({"type":"content_block_stop","index":1}));
            out += &frame(
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
            );
            out += &frame(json!({"type":"message_stop"}));
        }
        if pending {
            let data = bytes::Bytes::from(out);
            let stream = futures_util::stream::unfold(Some(data), |part| async move {
                match part {
                    Some(b) => Some((Ok::<_, std::convert::Infallible>(b), None)),
                    None => {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        None
                    }
                }
            });
            return (
                [("content-type", "text/event-stream")],
                Body::from_stream(stream),
            )
                .into_response();
        }
        return ([("content-type", "text/event-stream")], out).into_response();
    }
    let content = if mode == "issue" {
        json!([{"type":"thinking","thinking":"thought","signature":FULL},{"type":"text","text":"ready"}])
    } else {
        json!([{"type":"text","text":"plain"}])
    };
    Json(json!({"id":"msg_safety","type":"message","role":"assistant","content":content,"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}})).into_response()
}
struct Fixture {
    state: SharedState,
    gateway: Server,
    _source: Server,
    seen: Arc<Source>,
    key: String,
    key_id: i64,
    channel: ChannelId,
}
impl Fixture {
    async fn new(priced: bool) -> Self {
        let seen = Arc::new(Source::default());
        let upstream = serve(
            Router::new()
                .route("/v1/messages", post(source))
                .with_state(seen.clone()),
        )
        .await;
        let dir =
            std::env::temp_dir().join(format!("tokengo-protocol-safety-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db.sqlite"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();
        state.routing().cfg.retry_times = 0;
        state.routing().cfg.probe_enabled = false;
        let (key_id, key) = db::identity::create_key(
            &state.conn(),
            1,
            "safety-client",
            &Default::default(),
            None,
            None,
        )
        .unwrap();
        let channel = {
            let c = state.conn();
            let id = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "safety-source".into(),
                    base_url: format!("{}/v1", upstream.url),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 1,
                },
            )
            .unwrap();
            db::channels::set_models(
                &c,
                id,
                &[db::channels::ModelMapping::new(
                    MODEL,
                    Some("ExactNative".into()),
                    false,
                )],
            )
            .unwrap();
            db::channels::set_secret(&c, &state.secrets, id, "SYNTHETIC-SOURCE-KEY").unwrap();
            tokengo_server::protocol_config::save(
                &c,
                id.0,
                &tokengo_server::protocol_config::Capabilities {
                    parameter_profile: None,
                    runtime_identity: None,
                    native: vec![tokengo_server::protocol::Protocol::Messages],
                    allow_conversion: false,
                    native_input_audio: false,
                    chat_reasoning_replay: false,
                    max_input_tokens: Some(5000),
                    max_output_tokens: Some(100),
                },
            )
            .unwrap();
            if priced {
                db::billing::create_price(
                    &c,
                    &db::billing::Price{currency:"CNY".into(),
                        id: 0,
                        channel_id: id.0,
                        model: MODEL.into(),
                        input_micro_per_million: 0,
                        output_micro_per_million: 0,
                        max_input_tokens: 5000,
                        max_output_tokens: 100,
                    },
                    "test",
                )
                .unwrap();
            }
            id
        };
        let gateway = serve(app(state.clone())).await;
        Self {
            state,
            gateway,
            _source: upstream,
            seen,
            key,
            key_id: key_id.id,
            channel,
        }
    }
    fn body(mode: &str, stream: bool) -> Value {
        json!({"model":MODEL,"max_tokens":32,"stream":stream,"x_test_mode":mode,"messages":[{"role":"user","content":"hello"}]})
    }
    fn replay(signature: &str) -> Value {
        json!({"model":MODEL,"max_tokens":32,"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"thought","signature":signature}]},{"role":"user","content":"continue"}]})
    }
    async fn call(&self, key: &str, session: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/v1/messages", self.gateway.url))
            .header("x-api-key", key)
            .header("x-claude-code-session-id", session)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    fn calls(&self) -> usize {
        self.seen.seen.lock().unwrap().len()
    }
    async fn terminal(&self) -> (i64, String, Vec<(i64, String)>) {
        for _ in 0..100 {
            self.state.drain_writes();
            let result = {
                let c = self.state.conn();
                let row = c
                    .query_row(
                        "SELECT id,status FROM requests ORDER BY id DESC LIMIT 1",
                        [],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
                    )
                    .ok();
                row.map(|(id,status)|{let attempts=c.prepare("SELECT ordinal,outcome FROM attempts WHERE request_id=?1 ORDER BY ordinal").unwrap().query_map([id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();(id,status,attempts)})
            };
            if let Some(row) = result {
                if row.1 != "running" {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    let c = self.state.conn();
                    let attempts=c.prepare("SELECT ordinal,outcome FROM attempts WHERE request_id=?1 ORDER BY ordinal").unwrap().query_map([row.0],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
                    return (row.0, row.1, attempts);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("request did not terminate");
    }
    async fn wait_calls(&self, count: usize) {
        for _ in 0..100 {
            if self.calls() >= count {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("upstream not reached");
    }
    async fn wait_running(&self) {
        for _ in 0..100 {
            let n: i64 = self
                .state
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM requests WHERE status='running'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if n > 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("request not queued");
    }
}
#[tokio::test]
async fn native_opaque_history_is_not_bound_to_gateway_session_or_key_issuance() {
    for stream in [false, true] {
        let f = Fixture::new(true).await;
        let response=f.call(&f.key,"owner",Fixture::body("issue",stream)).await;
        assert_eq!(response.status(),200);response.bytes().await.unwrap();
        let (_,other)=db::identity::create_key(&f.state.conn(),1,"other",&Default::default(),None,None).unwrap();
        for (key,session) in [(&f.key,"owner"),(&other,"owner"),(&f.key,"different")] {
            let response=f.call(key,session,Fixture::replay(FULL)).await;
            let status=response.status();let text=response.text().await.unwrap();assert_eq!(status,200,"{text}");
        }
        db::channels::set_secret(&f.state.conn(),&f.state.secrets,f.channel,"SYNTHETIC-ROTATED-KEY").unwrap();
        let response=f.call(&f.key,"owner",Fixture::replay(FULL)).await;assert_eq!(response.status(),200);response.bytes().await.unwrap();
        let c=f.state.conn();
        assert_eq!(c.query_row("SELECT count(*) FROM protocol_sessions",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(c.query_row("SELECT count(*) FROM protocol_artifacts",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }
}

#[tokio::test]
async fn opaque_history_is_forwarded_without_a_local_issuance_registry() {
    let f=Fixture::new(true).await;
    let response=f.call(&f.key,"prefix",Fixture::body("prefix",true)).await;
    assert_eq!(response.status(),200);response.bytes().await.unwrap();
    let before=f.calls();let response=f.call(&f.key,"prefix",Fixture::replay("synthetic-signature-")).await;
    assert_eq!(response.status(),200);response.bytes().await.unwrap();assert_eq!(f.calls(),before+1);
}

#[tokio::test]
async fn cancellation_before_and_after_headers_records_exactly_one_canceled_attempt() {
    for postcommit in [false, true] {
        let f = Fixture::new(true).await;
        if postcommit {
            let mut response = f
                .call(&f.key, "cancel", Fixture::body("wait_post", true))
                .await;
            assert_eq!(response.status(), 200);
            assert!(response.chunk().await.unwrap().is_some());
            drop(response);
        } else {
            let url = format!("{}/v1/messages", f.gateway.url);
            let key = f.key.clone();
            let task = tokio::spawn(async move {
                reqwest::Client::new()
                    .post(url)
                    .header("x-api-key", key)
                    .json(&Fixture::body("wait_pre", true))
                    .send()
                    .await
            });
            f.wait_calls(1).await;
            task.abort();
            let _ = task.await;
        }
        let (_, status, attempts) = f.terminal().await;
        assert_eq!(status, "canceled");
        assert_eq!(attempts, vec![(0, "canceled".into())]);
    }
}
#[tokio::test]
async fn legacy_local_budget_does_not_block_an_unpriced_native_request() {
    let f=Fixture::new(false).await;
    f.state.conn().execute("INSERT INTO billing_budgets(scope,scope_id,limit_micro,frozen) VALUES('key',?1,0,1)",[f.key_id]).unwrap();
    let response=f.call(&f.key,"pricing",Fixture::body("plain",false)).await;
    assert_eq!(response.status(),200);response.bytes().await.unwrap();assert_eq!(f.calls(),1);
    let (_,status,attempts)=f.terminal().await;assert_eq!(status,"ok");assert_eq!(attempts,vec![(0,"success".into())]);
}

#[tokio::test]
async fn queued_request_cannot_outlive_model_disable() {
    let f = Fixture::new(true).await;
    let permit = f
        .state
        .channel_gate(f.channel.0, 1)
        .unwrap()
        .try_acquire_owned()
        .unwrap();
    let url = format!("{}/v1/messages", f.gateway.url);
    let key = f.key.clone();
    let task = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .header("x-api-key", key)
            .json(&Fixture::body("plain", false))
            .send()
            .await
            .unwrap()
    });
    f.wait_running().await;
    db::model_routes::set_enabled(&f.state.conn(), MODEL, false).unwrap();
    drop(permit);
    let response = task.await.unwrap();
    assert_eq!(
        response.status(),
        403,
        "model disabled while queued must block new upstream admission"
    );
    assert_eq!(f.calls(), 0);
}
#[tokio::test]
async fn revoked_queued_key_is_a_client_error_not_a_client_cancel() {
    let f = Fixture::new(true).await;
    let permit = f
        .state
        .channel_gate(f.channel.0, 1)
        .unwrap()
        .try_acquire_owned()
        .unwrap();
    let url = format!("{}/v1/messages", f.gateway.url);
    let key = f.key.clone();
    let task = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .header("x-api-key", key)
            .json(&Fixture::body("plain", false))
            .send()
            .await
            .unwrap()
    });
    f.wait_running().await;
    let mut key = db::identity::key(&f.state.conn(), f.key_id)
        .unwrap()
        .unwrap();
    key.revoked_at = Some(db::now_ms());
    db::identity::update_key(&f.state.conn(), &key).unwrap();
    drop(permit);
    assert_eq!(task.await.unwrap().status(), 401);
    assert_eq!(f.calls(), 0);
    let (_, status, attempts) = f.terminal().await;
    assert_eq!(status, "client_error");
    assert!(attempts.is_empty());
}
#[tokio::test]
async fn unavailable_legacy_artifact_storage_does_not_affect_native_calls() {
    for stream in [false,true] {
        let f=Fixture::new(true).await;
        f.state.conn().execute_batch("CREATE TRIGGER local_issue_abort BEFORE INSERT ON protocol_artifacts BEGIN SELECT RAISE(ABORT,'synthetic persistence error'); END").unwrap();
        let response=f.call(&f.key,"local-fault",Fixture::body("issue",stream)).await;
        assert_eq!(response.status(),200);response.bytes().await.unwrap();assert_eq!(f.calls(),1);
        let (_,status,attempts)=f.terminal().await;assert_eq!(status,"ok");assert_eq!(attempts,vec![(0,"success".into())]);
        assert!(db::faults::rows(&f.state.conn()).unwrap().is_empty());
    }
}

#[tokio::test]
async fn late_native_artifacts_do_not_write_legacy_storage() {
    let f=Fixture::new(true).await;
    f.state.conn().execute_batch("CREATE TRIGGER late_issue_abort BEFORE INSERT ON protocol_artifacts BEGIN SELECT RAISE(ABORT,'synthetic persistence error'); END").unwrap();
    let response=f.call(&f.key,"late-local",Fixture::body("late_issue",true)).await;
    assert_eq!(response.status(),200);response.bytes().await.unwrap();
    let (_,status,attempts)=f.terminal().await;assert_eq!(status,"ok");assert_eq!(attempts,vec![(0,"success".into())]);
}

#[tokio::test]
async fn unavailable_legacy_pin_storage_does_not_block_streaming() {
    let f=Fixture::new(true).await;
    f.state.conn().execute_batch("CREATE TRIGGER pin_storage_abort BEFORE INSERT ON protocol_sessions BEGIN SELECT RAISE(ABORT,'synthetic persistence error'); END").unwrap();
    let response=f.call(&f.key,"pin-store-failed",Fixture::body("issue",true)).await;
    assert_eq!(response.status(),200);response.bytes().await.unwrap();
    let (_,status,attempts)=f.terminal().await;assert_eq!(status,"ok");assert_eq!(attempts,vec![(0,"success".into())]);
}

async fn known_unavailable_pin_case(cooldown:bool) {
    let f=Fixture::new(true).await;
    let backup;
    {let c=f.state.conn();let row=db::channels::get(&c,f.channel).unwrap().unwrap();
     let generation:i64=c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[f.channel.0],|r|r.get(0)).unwrap();
     let owner=tokengo_server::protocol_sessions::key(&tokengo_shared::short_hash(&f.key),"pinned-unavailable",MODEL,tokengo_server::protocol::Protocol::Messages);
     let pin=tokengo_server::protocol_sessions::Pin{channel:f.channel.0,generation,upstream_model:"ExactNative".into(),protocol:tokengo_server::protocol::Protocol::Messages};
     assert!(tokengo_server::protocol_sessions::claim(&c,&owner,&pin).unwrap());
     backup=db::channels::create(&c,&db::channels::NewChannel{name:"current-backup".into(),base_url:row.base_url,enabled:true,unwrap_envelope:false,max_concurrent:1}).unwrap();
     db::channels::set_models(&c,backup,&[db::channels::ModelMapping::new(MODEL,Some("BackupExact".into()),false)]).unwrap();
     db::channels::set_secret(&c,&f.state.secrets,backup,"SYNTHETIC-BACKUP").unwrap();
     tokengo_server::protocol_config::save(&c,backup.0,&tokengo_server::protocol_config::Capabilities{native:vec![tokengo_server::protocol::Protocol::Messages],allow_conversion:false,max_input_tokens:Some(5000),max_output_tokens:Some(100),..Default::default()}).unwrap();
     if cooldown {
         c.execute("INSERT INTO provider_channels(name,enabled,created_ms) VALUES('fixture-pool',1,0)",[]).unwrap();let provider=c.last_insert_rowid();
         db::provider_channels::set_membership(&c,f.channel.0,Some(provider)).unwrap();
         db::provider_pool::backfill(&c,&f.state.secrets).unwrap();
         let now=db::now_ms();let generation=db::policy::claim(&c,f.channel.0,MODEL,"ExactNative",now).unwrap().unwrap();
         for _ in 0..3 {db::policy::complete(&c,f.channel.0,MODEL,"ExactNative",generation,false,"synthetic transient failure",0,now).unwrap();}
     } else {c.execute("UPDATE channels SET enabled=0 WHERE id=?1",[f.channel.0]).unwrap();}}
    let response=f.call(&f.key,"pinned-unavailable",Fixture::replay(FULL)).await;
    let status=response.status();let text=response.text().await.unwrap();assert_eq!(status,200,"{text}");
    assert_eq!(f.seen.seen.lock().unwrap().last().unwrap()["model"],"BackupExact");
    assert_eq!(f.calls(),1,"disabled or cooling historical source must not be called");
    let (request,_,_)=f.terminal().await;
    let served:i64=f.state.conn().query_row("SELECT channel_id FROM attempts WHERE request_id=?1",[request],|r|r.get(0)).unwrap();assert_eq!(served,backup.0);
}

#[tokio::test]
async fn historical_pin_and_opaque_history_do_not_block_current_available_fallback() {
    known_unavailable_pin_case(false).await;
    known_unavailable_pin_case(true).await;
}

#[tokio::test]
async fn capacity_only_change_keeps_issued_opaque_history_and_applies_new_budget() {
    let f=Fixture::new(false).await;
    let issued=f.call(&f.key,"capacity-owner",Fixture::body("issue",false)).await;assert_eq!(issued.status(),200);let _=issued.bytes().await.unwrap();
    let generation=||f.state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[f.channel.0],|r|r.get::<_,i64>(0)).unwrap();let before=generation();
    let mut caps=tokengo_server::protocol_config::load(&f.state.conn(),f.channel.0).unwrap();caps.max_input_tokens=Some(272000);caps.max_output_tokens=Some(272000);
    {let c=f.state.conn();tokengo_server::protocol_config::save(&c,f.channel.0,&caps).unwrap();}
    assert_eq!(generation(),before);
    let replay=f.call(&f.key,"capacity-owner",Fixture::replay(FULL)).await;let status=replay.status();let body:Value=replay.json().await.unwrap();assert_eq!(status,200,"{body}");f.terminal().await;
    let bound:i64=f.state.conn().query_row("SELECT input_bound FROM billing_ledger ORDER BY id DESC LIMIT 1",[],|r|r.get(0)).unwrap();assert_eq!(bound,272000,"capacity must apply immediately without pin rewrite");
    let calls=f.calls();caps.allow_conversion=true;tokengo_server::protocol_config::save(&f.state.conn(),f.channel.0,&caps).unwrap();assert_eq!(generation(),before+1);let denied=f.call(&f.key,"capacity-owner",Fixture::replay(FULL)).await;assert_eq!(denied.status(),200);denied.bytes().await.unwrap();assert_eq!(f.calls(),calls+1,"current identity must apply without a stale session restriction");
}


#[tokio::test]
async fn unpriced_opaque_history_requires_an_input_bound_only_for_an_explicit_tpm_limit() {
    let f=Fixture::new(false).await;
    {let c=f.state.conn();let mut caps=tokengo_server::protocol_config::load(&c,f.channel.0).unwrap();
        caps.max_input_tokens=None;caps.max_output_tokens=None;tokengo_server::protocol_config::save(&c,f.channel.0,&caps).unwrap();}
    let response=f.call(&f.key,"unpriced-history",Fixture::replay(FULL)).await;
    let status=response.status();let text=response.text().await.unwrap();assert_eq!(status,200,"{text}");assert_eq!(f.calls(),1);
    {let c=f.state.conn();let mut user=db::identity::user(&c,1).unwrap().unwrap();user.policy.tpm=Some(10000);db::identity::update_user(&c,&user).unwrap();}
    let response=f.call(&f.key,"unpriced-history",Fixture::replay(FULL)).await;
    let status=response.status();let text=response.text().await.unwrap();assert_eq!(status,403,"{text}");assert_eq!(f.calls(),1,"explicit TPM still requires a configured input bound before sending");
}
