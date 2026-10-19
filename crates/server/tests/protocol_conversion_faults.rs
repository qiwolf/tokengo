//! Valid native source output must not be cooled for a local conversion mismatch.
use axum::{
    body::Body,
    extract::{OriginalUri, State},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
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
    Http {
        url,
        task: tokio::spawn(async move { axum::serve(l, router).await.unwrap() }),
    }
}
struct Source {
    mode: usize,
    bad: AtomicUsize,
    good: AtomicUsize,
}
fn frame(delta: Value, output: i64) -> String {
    format!(
        "data: {}\n\n",
        json!({"id":"chat_review","object":"chat.completion.chunk","model":"source","choices":[{"index":0,"delta":delta,"finish_reason":null}],"usage":{"prompt_tokens":7,"completion_tokens":output,"total_tokens":7+output}})
    )
}
async fn source(
    State(s): State<Arc<Source>>,
    OriginalUri(uri): OriginalUri,
    Json(v): Json<Value>,
) -> Response {
    let good = uri.path().starts_with("/good");
    if good {
        s.good.fetch_add(1, Ordering::SeqCst);
    } else {
        s.bad.fetch_add(1, Ordering::SeqCst);
    }
    if v["stream"] == true {
        if s.mode == 3 {
            return (
                [("content-type", "text/event-stream")],
                "data: not-json\n\n",
            )
                .into_response();
        }
        let first = if good || s.mode == 2 {
            frame(json!({"role":"assistant","content":"first text"}), 1)
        } else {
            frame(
                json!({"role":"assistant","reasoning_content":"valid native thought"}),
                3,
            )
        };
        let second = if !good && s.mode == 2 {
            frame(json!({"reasoning_content":"valid late native thought"}), 3)
        } else {
            frame(json!({"content":"answer"}), 3)
        };
        let tail = format!(
            "{}data: {}\n\ndata: [DONE]\n\n",
            second,
            json!({"id":"chat_review","object":"chat.completion.chunk","model":"source","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})
        );
        let stream = futures_util::stream::unfold((0, first, tail), |(n, a, b)| async move {
            match n {
                0 => Some((
                    Ok::<_, std::io::Error>(bytes::Bytes::from(a.clone())),
                    (1, a, b),
                )),
                1 => {
                    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                    Some((Ok(bytes::Bytes::from(b.clone())), (2, a, b)))
                }
                _ => None,
            }
        });
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response();
    }
    let mut message = json!({"role":"assistant","content":"answer"});
    if !good {
        message["reasoning_content"] = json!("valid native thought");
    }
    Json(json!({"id":"chat_review","object":"chat.completion","model":"source","choices":[{"index":0,"message":message,"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}})).into_response()
}
struct Fixture {
    state: SharedState,
    gateway: Http,
    _source: Http,
    source: Arc<Source>,
    key: String,
    id: ChannelId,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
impl Fixture {
    async fn new(mode: usize, fallback: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-conversion-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            web_dir: None,
            log_dir: None,
        })
        .unwrap();
        let src = Arc::new(Source {
            mode,
            bad: AtomicUsize::new(0),
            good: AtomicUsize::new(0),
        });
        let upstream = serve(
            Router::new()
                .route("/{kind}/v1/chat/completions", post(source))
                .with_state(src.clone()),
        )
        .await;
        let (key, id) = {
            let c = state.conn();
            let user =
                db::identity::create_user(&c, "conversion-review", "user", &Default::default())
                    .unwrap();
            let (_, key) =
                db::identity::create_key(&c, user.id, "review", &Default::default(), None, None)
                    .unwrap();
            let mut ids = vec![];
            for kind in if fallback {
                vec!["bad", "good"]
            } else {
                vec!["bad"]
            } {
                let id = db::channels::create(
                    &c,
                    &db::channels::NewChannel {
                        name: kind.into(),
                        base_url: format!("{}/{kind}/v1", upstream.url),
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
                        "review",
                        Some("source".into()),
                        false,
                    )],
                )
                .unwrap();
                tokengo_server::protocol_config::save(
                    &c,
                    id.0,
                    &tokengo_server::protocol_config::Capabilities {
                        allow_conversion: true,
                        ..Default::default()
                    },
                )
                .unwrap();
                db::billing::create_price(
                    &c,
                    &db::billing::Price {
                        id: 0,
                        channel_id: id.0,
                        model: "review".into(),
                        input_micro_per_million: 1,
                        output_micro_per_million: 1,
                        max_input_tokens: 8192,
                        max_output_tokens: 64,
                    },
                    "review",
                )
                .unwrap();
                ids.push(id.0);
            }
            db::route_policy::save(
                &c,
                "review",
                &db::route_policy::RoutePolicy {
                    channel_ids: Some(ids.clone()),
                    retry_times: Some(2),
                    probe_enabled: Some(false),
                    include_in_auto: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
            (key, ChannelId(ids[0]))
        };
        let gateway = serve(app(state.clone())).await;
        Self {
            state,
            gateway,
            _source: upstream,
            source: src,
            key,
            id,
            dir,
        }
    }
    async fn call(&self, path: &str, stream: bool) -> (u16, String) {
        let body = if path.ends_with("responses") {
            json!({"model":"review","input":"hi","max_output_tokens":16,"stream":stream})
        } else {
            json!({"model":"review","messages":[{"role":"user","content":"hi"}],"max_tokens":16,"stream":stream})
        };
        let r = reqwest::Client::new()
            .post(format!("{}{}", self.gateway.url, path))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    }
    fn health(&self) {
        self.state.drain_writes();
        let c = self.state.conn();
        assert!(db::policy::rows(&c, "review")
            .unwrap()
            .iter()
            .filter(|r| r.channel_id == self.id.0)
            .all(|r| r.failures == 0 && r.until_ms == 0));
    }
    fn ledger(&self, n: usize) {
        self.state.drain_writes();
        let c = self.state.conn();
        let rows:Vec<(String,String,i64,i64)>=c.prepare("SELECT a.outcome,l.outcome,l.input_tokens,l.output_tokens FROM attempts a JOIN billing_ledger l ON l.attempt_id=CAST(a.request_id AS TEXT)||':'||CAST(a.ordinal AS TEXT) WHERE a.channel_id=? ORDER BY a.id").unwrap().query_map([self.id.0],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().map(Result::unwrap).collect();
        assert_eq!(rows.len(), n);
        assert!(
            rows.iter()
                .all(|r| r == &("protocol_fault".into(), "protocol_fault".into(), 7, 3)),
            "{rows:?}"
        );
    }
}
#[tokio::test]
async fn nonstream_conversion_mismatch_does_not_trip_native_source() {
    for path in ["/v1/responses", "/v1/messages"] {
        let f = Fixture::new(0, false).await;
        for _ in 0..3 {
            let (code, text) = f.call(path, false).await;
            assert_eq!(code, 400, "{text}");
        }
        f.health();
        f.ledger(3);
        assert_eq!(f.source.bad.load(Ordering::SeqCst), 3);
        assert_eq!(f.call("/v1/chat/completions", false).await.0, 200);
    }
}
#[tokio::test]
async fn precommit_conversion_mismatch_falls_back_without_source_cooldown() {
    for path in ["/v1/responses", "/v1/messages"] {
        let f = Fixture::new(1, true).await;
        let (code, text) = f.call(path, true).await;
        assert_eq!(code, 200, "{text}");
        assert!(text.contains("answer"));
        assert_eq!(f.source.bad.load(Ordering::SeqCst), 1);
        assert_eq!(f.source.good.load(Ordering::SeqCst), 1);
        f.health();
        f.ledger(1);
        assert_eq!(f.call("/v1/chat/completions", false).await.0, 200);
    }
}
#[tokio::test]
async fn postcommit_conversion_mismatch_errors_without_replay_or_source_cooldown() {
    for path in ["/v1/responses", "/v1/messages"] {
        let f = Fixture::new(2, true).await;
        let (code, text) = f.call(path, true).await;
        assert_eq!(code, 200, "{text}");
        assert!(
            text.contains("first text") && text.contains("event: error"),
            "{text}"
        );
        assert!(!text.contains("response.completed"));
        assert_eq!(f.source.bad.load(Ordering::SeqCst), 1);
        assert_eq!(f.source.good.load(Ordering::SeqCst), 0);
        f.health();
        f.ledger(1);
        assert_eq!(f.call("/v1/chat/completions", false).await.0, 200);
    }
}

#[tokio::test]
async fn malformed_source_sse_retains_channel_fault_and_cooldown() {
    let f = Fixture::new(3, false).await;
    let (code, _) = f.call("/v1/responses", true).await;
    assert_eq!(code, 502);
    f.state.drain_writes();
    let c = f.state.conn();
    assert_eq!(db::policy::rows(&c, "review").unwrap()[0].failures, 3);
    let count: i64 = c
        .query_row(
            "SELECT count(*) FROM attempts WHERE outcome='channel_fault'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
}
