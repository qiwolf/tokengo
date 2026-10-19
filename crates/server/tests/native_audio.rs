//! Native Chat input-audio is an explicit, bounded capability, never a text conversion.
use axum::{extract::State, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tokengo_shared::ChannelId;
struct Http {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Http {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Http {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
    Http { url, task }
}
async fn source(State(seen): State<Arc<Mutex<Vec<Value>>>>, Json(v): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().push(v);
    Json(
        json!({"id":"chat_audio","object":"chat.completion","model":"source-asr","choices":[{"index":0,"message":{"role":"assistant","content":"识别文本"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}),
    )
}
struct Fixture {
    state: SharedState,
    gateway: Http,
    _upstream: Http,
    seen: Arc<Mutex<Vec<Value>>>,
    key: String,
    channel: ChannelId,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
impl Fixture {
    async fn new(audio: bool, priced: bool, tpm: Option<u64>) -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-audio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        let seen = Arc::new(Mutex::new(vec![]));
        let upstream = serve(
            Router::new()
                .route("/v1/chat/completions", post(source))
                .with_state(seen.clone()),
        )
        .await;
        let (key, channel) = {
            let c = state.conn();
            let user = db::identity::create_user(
                &c,
                "audio-owner",
                "user",
                &db::identity::Policy {
                    tpm,
                    ..Default::default()
                },
            )
            .unwrap();
            let (_, key) = db::identity::create_key(
                &c,
                user.id,
                "audio-client",
                &Default::default(),
                None,
                None,
            )
            .unwrap();
            let id = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "native-asr".into(),
                    base_url: format!("{}/v1", upstream.url),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap();
            db::channels::set_models(
                &c,
                id,
                &[db::channels::ModelMapping::new(
                    "asr",
                    Some("source-asr".into()),
                    false,
                )],
            )
            .unwrap();
            db::settings::set(
                &c,
                &format!("channel_protocols:{}", id.0),
                &json!({"native":["chat"],"allow_conversion":true,"native_input_audio":audio})
                    .to_string(),
            )
            .unwrap();
            db::route_policy::save(
                &c,
                "asr",
                &db::route_policy::RoutePolicy {
                    retry_times: Some(0),
                    probe_enabled: Some(false),
                    include_in_auto: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
            if priced {
                db::billing::create_price(
                    &c,
                    &db::billing::Price {
                        id: 0,
                        channel_id: id.0,
                        model: "asr".into(),
                        input_micro_per_million: 0,
                        output_micro_per_million: 0,
                        max_input_tokens: 1000,
                        max_output_tokens: 64,
                    },
                    "audio-test",
                )
                .unwrap();
            }
            (key, id)
        };
        let gateway = serve(app(state.clone())).await;
        Self {
            state,
            gateway,
            _upstream: upstream,
            seen,
            key,
            channel,
            dir,
        }
    }
    async fn call(&self, path: &str, v: Value) -> (u16, Value) {
        let r = reqwest::Client::new()
            .post(format!("{}{}", self.gateway.url, path))
            .bearer_auth(&self.key)
            .json(&v)
            .send()
            .await
            .unwrap();
        let code = r.status().as_u16();
        (code, r.json().await.unwrap())
    }
}
fn request() -> Value {
    json!({"model":"asr","max_tokens":16,"messages":[{"role":"user","content":[{"type":"input_audio","input_audio":{"data":"UklGRgQAAABXQVZF","format":"wav"}}]}]})
}
#[tokio::test]
async fn declared_native_audio_preserves_payload_and_records_reported_tokens() {
    let f = Fixture::new(true, true, None).await;
    let body = request();
    let (code, reply) = f.call("/v1/chat/completions", body.clone()).await;
    assert_eq!(code, 200, "{reply}");
    assert_eq!(f.seen.lock().unwrap()[0]["messages"], body["messages"]);
    f.state.drain_writes();
    let c = f.state.conn();
    let row: (String, i64, i64, String) = c
        .query_row(
            "SELECT usage_certainty,input_tokens,output_tokens,status FROM billing_ledger",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row, ("reported".into(), 7, 3, "success".into()));
}
#[tokio::test]
async fn native_audio_requires_priced_context_bound_before_any_upstream_call() {
    let f = Fixture::new(true, false, None).await;
    let (code, reply) = f.call("/v1/chat/completions", request()).await;
    assert_eq!(code, 403, "{reply}");
    assert!(f.seen.lock().unwrap().is_empty());
}
#[tokio::test]
async fn undeclared_source_is_rejected_even_with_free_pricing() {
    let f = Fixture::new(false, true, None).await;
    let (code, _) = f.call("/v1/chat/completions", request()).await;
    assert_eq!(code, 400);
    assert!(f.seen.lock().unwrap().is_empty());
}
#[tokio::test]
async fn malformed_audio_and_audio_conversion_never_reach_source() {
    let f = Fixture::new(true, true, None).await;
    for malformed in [
        json!({"data":"not-base64!","format":"wav"}),
        json!({"data":"UklGRgQAAABXQVZF","format":"video"}),
    ] {
        let mut v = request();
        v["messages"][0]["content"][0]["input_audio"] = malformed;
        assert_eq!(f.call("/v1/chat/completions", v).await.0, 400);
    }
    let v = json!({"model":"asr","max_output_tokens":16,"input":[{"role":"user","content":[{"type":"input_audio","input_audio":{"data":"UklGRgQAAABXQVZF","format":"wav"}}]}]});
    assert_eq!(f.call("/v1/responses", v).await.0, 400);
    assert!(f.seen.lock().unwrap().is_empty());
}
#[tokio::test]
async fn audio_admission_reserves_full_context_against_finite_tpm() {
    let f = Fixture::new(true, true, Some(50)).await;
    let (code, reply) = f.call("/v1/chat/completions", request()).await;
    assert_eq!(code, 429, "{reply}");
    assert!(f.seen.lock().unwrap().is_empty());
    assert!(f.channel.0 > 0);
}
#[tokio::test]
async fn audio_skips_an_undeclared_primary_without_spending_an_attempt() {
    let f = Fixture::new(false, true, None).await;
    let second = {
        let c = f.state.conn();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "audio-capable-backup".into(),
                base_url: format!("{}/v1", f._upstream.url),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        db::channels::set_models(
            &c,
            id,
            &[db::channels::ModelMapping::new(
                "asr",
                Some("source-asr".into()),
                false,
            )],
        )
        .unwrap();
        db::settings::set(
            &c,
            &format!("channel_protocols:{}", id.0),
            &json!({"native":["chat"],"native_input_audio":true}).to_string(),
        )
        .unwrap();
        db::billing::create_price(
            &c,
            &db::billing::Price {
                id: 0,
                channel_id: id.0,
                model: "asr".into(),
                input_micro_per_million: 0,
                output_micro_per_million: 0,
                max_input_tokens: 1000,
                max_output_tokens: 64,
            },
            "audio-test",
        )
        .unwrap();
        db::route_policy::save(
            &c,
            "asr",
            &db::route_policy::RoutePolicy {
                channel_ids: Some(vec![f.channel.0, id.0]),
                retry_times: Some(0),
                probe_enabled: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        id
    };
    let (code, body) = f.call("/v1/chat/completions", request()).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(f.seen.lock().unwrap().len(), 1);
    f.state.drain_writes();
    let c = f.state.conn();
    let (count, channel): (i64, i64) = c
        .query_row("SELECT count(*),channel_id FROM attempts", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((count, channel), (1, second.0));
}

#[tokio::test]
async fn assistant_audio_id_history_never_reaches_undeclared_source() {
    let f = Fixture::new(false, true, None).await;
    let body = json!({"model":"asr","max_tokens":16,"messages":[{"role":"user","content":"transcribe"},{"role":"assistant","content":null,"audio":{"id":"audio_external_unissued"}},{"role":"user","content":"repeat the previous audio"}]});
    let (code, reply) = f.call("/v1/chat/completions", body).await;
    assert_eq!(code, 400, "unsupported provider audio state reached source; reply={reply}");
    assert!(f.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_input_audio_capability_does_not_enable_audio_output() {
    let f = Fixture::new(true, true, None).await;
    for patch in [json!({"audio":{"voice":"alloy","format":"wav"}}), json!({"modalities":["text","audio"]})] {
        let mut body = request();
        for (key, value) in patch.as_object().unwrap() {
            body[key] = value.clone();
        }
        let (code, reply) = f.call("/v1/chat/completions", body).await;
        assert_eq!(code, 400, "{reply}");
    }
    assert!(f.seen.lock().unwrap().is_empty());
}
