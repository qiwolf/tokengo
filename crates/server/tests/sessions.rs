//! Stable conversation identities across real HTTP gateway and upstream requests.
use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokengo_server::{app, AppState, Config};

struct Fixture {
    state: Arc<AppState>,
    base: String,
    key: String,
    calls: Arc<AtomicUsize>,
    dir: std::path::PathBuf,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl Fixture {
    async fn new(template: bool) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let router=Router::new().route("/v1/chat/completions",post(|State(calls):State<Arc<AtomicUsize>>, headers: HeaderMap| async move {
            calls.fetch_add(1,Ordering::SeqCst);
            let content=json!({"session":headers.get("x-opencode-session").and_then(|v|v.to_str().ok()),"authorization":headers.get("authorization").and_then(|v|v.to_str().ok())}).to_string();
            Json(json!({"choices":[{"message":{"content":content}}]}))
        }))
        .route("/v1/embeddings", post(|State(calls): State<Arc<AtomicUsize>>| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Json(json!({"data":[{"index":0,"embedding":[0.1,0.2]}]}))
        }))
        .route("/v1/rerank", post(|State(calls): State<Arc<AtomicUsize>>| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Json(json!({"results":[{"index":0,"relevance_score":0.9}]}))
        })).with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}/v1", listener.local_addr().unwrap());
        let upstream_task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let dir = std::env::temp_dir().join(format!("tokengo-sessions-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        let key = tokengo_server::db::gateway_keys::generate();
        {
            let c = state.conn();
            tokengo_server::db::gateway_keys::create(&c, &state.secrets, "client", &key, false)
                .unwrap();
            let id = tokengo_server::db::channels::create(
                &c,
                &tokengo_server::db::channels::NewChannel {
                    name: "upstream".into(),
                    base_url: upstream,
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap();
            tokengo_server::db::channels::set_models(
                &c,
                id,
                &[tokengo_server::db::channels::ModelMapping::new(
                    "test", None, false,
                )],
            )
            .unwrap();
            tokengo_server::db::channels::set_secret(&c, &state.secrets, id, "upstream-secret")
                .unwrap();
            if template {
                tokengo_server::db::channels::set_headers(
                    &c,
                    &state.secrets,
                    id,
                    &[("x-opencode-session".into(), "{{session_id}}".into())],
                )
                .unwrap();
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = app(state.clone());
        let gateway_task = tokio::spawn(async move {
            axum::serve(listener, server).await.unwrap();
        });
        Self {
            state,
            base,
            key,
            calls,
            dir,
            tasks: vec![upstream_task, gateway_task],
        }
    }
    async fn send(&self, key: &str, headers: &[(&str, &str)], mut body: Value) -> (u16, Value) {
        body["model"] = json!("test");
        let mut request = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
            .post(format!("{}/v1/chat/completions", self.base))
            .bearer_auth(key)
            .json(&body);
        for (n, v) in headers {
            request = request.header(*n, *v);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let value = response.json().await.unwrap();
        (status, value)
    }
    async fn identity(&self, headers: &[(&str, &str)], body: Value) -> String {
        let (status, value) = self.send(&self.key, headers, body).await;
        assert_eq!(status, 200, "{value}");
        let content: Value =
            serde_json::from_str(value["choices"][0]["message"]["content"].as_str().unwrap())
                .unwrap();
        assert_eq!(
            content["authorization"], "Bearer upstream-secret",
            "client Authorization must not be forwarded"
        );
        content["session"].as_str().unwrap().to_owned()
    }
}
fn messages() -> Value {
    json!([{"role":"user","content":"same opening"}])
}
#[tokio::test]
async fn headers_and_metadata_share_explicit_identity_with_defined_priority() {
    let f = Fixture::new(true).await;
    let baseline = f
        .identity(&[], json!({"session_id":"stable","messages":messages()}))
        .await;
    for name in [
        "x-opencode-session",
        "session_id",
        "x-session-id",
        "x-conversation-id",
    ] {
        let sid=f.identity(&[(name,"stable")],json!({"session_id":"loser","metadata":{"conversation_id":"also-loser"},"messages":messages()})).await;
        assert_eq!(sid, baseline, "header {name} must take precedence");
    }
    for metadata in [
        json!({"conversation_id":"stable"}),
        json!({"session_id":"stable"}),
    ] {
        assert_eq!(
            f.identity(&[], json!({"metadata":metadata,"messages":messages()}))
                .await,
            baseline
        );
    }
    assert_eq!(
        f.identity(
            &[
                ("x-opencode-session", "stable"),
                ("session_id", "second"),
                ("x-session-id", "third"),
                ("x-conversation-id", "fourth")
            ],
            json!({"messages":messages()})
        )
        .await,
        baseline
    );
    assert_eq!(f.identity(&[],json!({"session_id":"stable","conversation_id":"second","metadata":{"conversation_id":"third"},"messages":messages()})).await,baseline);
    assert_eq!(f.identity(&[],json!({"metadata":{"conversation_id":"stable","session_id":"second"},"messages":messages()})).await,baseline);
    let boundary = "x".repeat(256);
    assert_eq!(
        f.identity(
            &[("x-session-id", &boundary)],
            json!({"messages":messages()})
        )
        .await,
        f.identity(&[], json!({"session_id":boundary,"messages":messages()}))
            .await
    );
}
#[tokio::test]
async fn tool_only_rounds_require_explicit_identity_only_for_session_templates() {
    let f = Fixture::new(true).await;
    let initial = f
        .identity(
            &[("x-opencode-session", "conversation")],
            json!({"messages":messages()}),
        )
        .await;
    let tool = json!({"messages":[{"role":"tool","tool_call_id":"call-1","content":"result"}]});
    assert_eq!(
        f.identity(&[("x-opencode-session", "conversation")], tool.clone())
            .await,
        initial
    );
    let calls = f.calls.load(Ordering::SeqCst);
    let (s, v) = f.send(&f.key, &[], tool.clone()).await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(f.calls.load(Ordering::SeqCst), calls);
    f.state.drain_writes();
    let ledger: String = f
        .state
        .conn()
        .query_row(
            "SELECT status FROM requests ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ledger, "client_error");
    let no_template = Fixture::new(false).await;
    assert_eq!(no_template.send(&no_template.key, &[], tool).await.0, 200);
}
#[tokio::test]
async fn invalid_explicit_ids_are_400_before_upstream() {
    let f = Fixture::new(true).await;
    for id in [
        json!(""),
        json!(" "),
        json!("x".repeat(257)),
        json!("line\nbreak"),
        json!(7),
        Value::Null,
    ] {
        for body in [
            json!({"session_id":id,"messages":messages()}),
            json!({"metadata":{"conversation_id":id},"messages":messages()}),
        ] {
            let (s, v) = f.send(&f.key, &[], body).await;
            assert_eq!(s, 400, "{v}");
        }
    }
    for id in ["", " ", &"x".repeat(257)] {
        assert_eq!(
            f.send(
                &f.key,
                &[("x-session-id", id)],
                json!({"messages":messages()})
            )
            .await
            .0,
            400
        );
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn same_explicit_identity_is_namespaced_by_authenticated_gateway_key() {
    let f = Fixture::new(true).await;
    let other = tokengo_server::db::gateway_keys::generate();
    tokengo_server::db::gateway_keys::create(
        &f.state.conn(),
        &f.state.secrets,
        "other",
        &other,
        false,
    )
    .unwrap();
    let body = json!({"metadata":{"session_id":"stable"},"messages":messages()});
    let (_, a) = f.send(&f.key, &[], body.clone()).await;
    let (_, b) = f.send(&other, &[], body).await;
    assert_ne!(
        a["choices"][0]["message"]["content"],
        b["choices"][0]["message"]["content"]
    );
}


async fn nonchat_without_explicit_session(endpoint: &str, body: Value) -> Value {
    // Each endpoint starts with an independent model to avoid learned-kind effects.
    let fixture = Fixture::new(true).await;
    let response = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap()
        .post(format!("{}/v1/{endpoint}", fixture.base))
        .bearer_auth(&fixture.key).json(&body).send().await.unwrap();
    let status = response.status();
    let value: Value = response.json().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "non-chat session template must not require user messages: {value}");
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    value
}

#[tokio::test]
async fn embeddings_with_session_template_need_no_conversation_identity() {
    let value = nonchat_without_explicit_session("embeddings", json!({"model":"test","input":"hello"})).await;
    assert_eq!(value["data"][0]["embedding"], json!([0.1,0.2]));
}

#[tokio::test]
async fn rerank_with_session_template_needs_no_conversation_identity() {
    let value = nonchat_without_explicit_session("rerank", json!({"model":"test","query":"hello","documents":["world"]})).await;
    assert_eq!(value["results"][0]["relevance_score"], json!(0.9));
}
