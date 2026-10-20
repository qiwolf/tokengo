//! Actual HTTP standalone tools share identity, permits, financial holds and outbox.
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU16, Ordering},
    Arc, Mutex,
};
use tokengo_server::{app, db, AppState, Config, SharedState};
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
    seen: Mutex<Vec<(String, HeaderMap, Value)>>,
    status: AtomicU16,
}
async fn source(
    State(state): State<Arc<Source>>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    state
        .seen
        .lock()
        .unwrap()
        .push((uri.path().into(), headers, body.clone()));
    if body["query"] == "stall-error" {
        let stream = futures_util::stream::unfold(
            Some(bytes::Bytes::from_static(b"{")),
            |first| async move {
                match first {
                    Some(first) => Some((Ok::<_, std::convert::Infallible>(first), None)),
                    None => {
                        std::future::pending::<()>().await;
                        None
                    }
                }
            },
        );
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("content-type", "application/json")],
            axum::body::Body::from_stream(stream),
        )
            .into_response();
    }
    let status = state.status.load(Ordering::SeqCst);
    if status >= 400 {
        return(StatusCode::from_u16(status).unwrap(),Json(json!({"error":{"type":"invalid_request_error","message":"synthetic tool rejection"}}))).into_response();
    }
    if uri.path().ends_with("fetch") {
        return Json(json!({"title":"Page","content":"# Synthetic page\nPublic text"}))
            .into_response();
    }
    let rows = if body["query"] == "no matches" {
        vec![]
    } else {
        vec![
            json!({"rerank_score":2.5,"title":"Synthetic result","url":"https://example.com","snippet":"snippet","summary":"summary","content":"","site_name":null,"site_icon":null,"date":null,"authority":3}),
        ]
    };
    Json(json!({"results":rows})).into_response()
}
struct Fixture {
    state: SharedState,
    gateway: Server,
    _source: Server,
    seen: Arc<Source>,
    key: String,
    channel: i64,
    key_id: i64,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
impl Fixture {
    async fn new() -> Self {
        let seen = Arc::new(Source::default());
        let upstream = serve(
            Router::new()
                .route("/v1/tools/search", post(source))
                .route("/v1/tools/fetch", post(source))
                .with_state(seen.clone()),
        )
        .await;
        let dir =
            std::env::temp_dir().join(format!("tokengo-search-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into(),
            ..Default::default()
        })
        .unwrap();
        state.routing().cfg.retry_times = 0;
        state.routing().cfg.probe_enabled = false;
        let (key_row, key) = db::identity::create_key(
            &state.conn(),
            1,
            "tool-client",
            &Default::default(),
            None,
            None,
        )
        .unwrap();
        let key_id = key_row.id;
        let channel = {
            let c = state.conn();
            let id = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "DeepSeek tools".into(),
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
                &[
                    db::channels::ModelMapping::new("deepseek-search", None, false),
                    db::channels::ModelMapping::new("deepseek-fetch", None, false),
                ],
            )
            .unwrap();
            db::channels::set_secret(&c, &state.secrets, id, "SYNTHETIC-SOURCE-ONLY").unwrap();
            tokengo_server::search_tools::save(
                &c,
                id.0,
                &tokengo_server::search_tools::ToolCapabilities {
                    search: true,
                    fetch: true,
                    account_scope: uuid::Uuid::new_v4().to_string(),
                    qps: 100,
                },
            )
            .unwrap();
            for model in ["deepseek-search", "deepseek-fetch"] {
                db::billing::create_request_price(
                    &c,
                    &db::billing::RequestPrice {
                        id: 0,
                        channel_id: id.0,
                        model: model.into(),
                        micro_per_call: 750,
                    },
                    "test",
                )
                .unwrap();
            }
            id.0
        };
        let gateway = serve(app(state.clone())).await;
        Self {
            state,
            gateway,
            _source: upstream,
            seen,
            key,
            channel,
            key_id,
            dir,
        }
    }
    async fn send(&self, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/v1/tools/{path}", self.gateway.url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    fn settle(&self) {
        self.state.drain_writes();
        tokengo_server::billing_writer::replay(&self.state.conn(), &self.state.cfg.db_path)
            .unwrap();
    }
}
#[tokio::test]
async fn successful_search_empty_results_and_fetch_use_request_fees_never_tokens() {
    let f = Fixture::new().await;
    for (path, body) in [
        (
            "search",
            json!({"query":"DeepSeek site:example.com lang:zh","count":1,"unexpected":"discard"}),
        ),
        ("search", json!({"query":"no matches"})),
        ("fetch", json!({"url":"https://example.com/article"})),
    ] {
        let r = f.send(path, body).await;
        assert_eq!(r.status(), StatusCode::OK);
        let _: Value = r.json().await.unwrap();
    }
    let seen = f.seen.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    for (_, headers, body) in seen.iter() {
        assert_eq!(headers["authorization"], "Bearer SYNTHETIC-SOURCE-ONLY");
        assert_ne!(
            headers["authorization"].to_str().unwrap(),
            format!("Bearer {}", f.key)
        );
        assert!(body.get("model").is_none());
        assert!(body.get("unexpected").is_none());
    }
    assert_eq!(seen[0].2["query"], "DeepSeek site:example.com lang:zh");
    drop(seen);
    f.settle();
    let rows:Vec<(Option<i64>,Option<i64>,Option<i64>,Option<i64>)>=f.state.conn().prepare("SELECT request_units,debit_micro,input_tokens,output_tokens FROM billing_ledger ORDER BY id").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(
        rows,
        vec![
            (Some(1), Some(750), None, None),
            (Some(0), Some(0), None, None),
            (Some(1), Some(750), None, None)
        ]
    );
}
#[tokio::test]
async fn nonbillable_errors_do_not_suspend_model_account_and_invalid_requests_never_send() {
    let f = Fixture::new().await;
    for (status, path) in [(429, "search"), (403, "fetch"), (404, "fetch")] {
        f.seen.status.store(status, Ordering::SeqCst);
        let body = if path == "search" {
            json!({"query":"test"})
        } else {
            json!({"url":"https://example.com"})
        };
        let r = f.send(path, body).await;
        assert_eq!(r.status().as_u16(), status);
        let _: Value = r.json().await.unwrap();
    }
    f.settle();
    let c = f.state.conn();
    assert_eq!(c.query_row("SELECT count(*) FROM billing_ledger WHERE billing_unit='request' AND request_units=0 AND debit_micro=0",[],|r|r.get::<_,i64>(0)).unwrap(),3);
    assert!(!db::faults::has_fault(&c, f.channel, None).unwrap());
    drop(c);
    assert_eq!(
        f.send("search", json!({"query":" "})).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.send("fetch", json!({"url":"http://user:pass@example.com"}))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.seen.seen.lock().unwrap().len(), 3);
}
#[tokio::test]
async fn request_budget_and_live_key_revocation_prevent_another_source_call() {
    let f = Fixture::new().await;
    f.state.conn().execute("INSERT OR REPLACE INTO billing_budgets(scope,scope_id,limit_micro) VALUES('key',?1,750)",[f.key_id]).unwrap();
    assert_eq!(
        f.send("search", json!({"query":"first"})).await.status(),
        StatusCode::OK
    );
    f.settle();
    let denied = f.send("search", json!({"query":"second"})).await;
    assert_eq!(denied.status(), StatusCode::TOO_MANY_REQUESTS);
    let error: Value = denied.json().await.unwrap();
    assert_eq!(error["error"]["type"], "budget_exceeded");
    assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
    f.state
        .conn()
        .execute("UPDATE gateway_keys SET enabled=0 WHERE id=?1", [f.key_id])
        .unwrap();
    assert_eq!(
        f.send("search", json!({"query":"revoked"})).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn capabilities_are_explicit_and_background_probes_do_not_make_unmetered_calls() {
    let f = Fixture::new().await;
    let outcome =
        tokengo_server::prober::probe_route(&f.state, "deepseek-search", db::now_ms()).await;
    assert!(matches!(
        outcome.outcome,
        tokengo_server::prober::ProbeOutcome::Skipped(_)
    ));
    tokengo_server::search_tools::save(&f.state.conn(), f.channel, &Default::default()).unwrap();
    assert_eq!(
        f.send("search", json!({"query":"not enabled"}))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.seen.seen.lock().unwrap().len(), 0);
}
#[tokio::test]
async fn free_request_price_cannot_authorize_paid_token_inference() {
    let mut f = Fixture::new().await;
    let (_, key) = db::identity::create_key(
        &f.state.conn(),
        1,
        "free-only",
        &db::identity::Policy {
            allow_paid: false,
            ..Default::default()
        },
        None,
        None,
    )
    .unwrap();
    f.key = key;
    db::billing::create_request_price(
        &f.state.conn(),
        &db::billing::RequestPrice {
            id: 0,
            channel_id: f.channel,
            model: "deepseek-search".into(),
            micro_per_call: 0,
        },
        "test",
    )
    .unwrap();
    db::billing::create_price(
        &f.state.conn(),
        &db::billing::Price {
            id: 0,
            channel_id: f.channel,
            model: "deepseek-search".into(),
            input_micro_per_million: 100,
            output_micro_per_million: 100,
            max_input_tokens: 32768,
            max_output_tokens: 4096,
        },
        "test",
    )
    .unwrap();
    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", f.gateway.url))
        .bearer_auth(&f.key)
        .json(&json!({"model":"deepseek-search","messages":[{"role":"user","content":"hello"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(f.seen.seen.lock().unwrap().len(), 0);
    assert_eq!(
        f.send("search", json!({"query":"free standalone route"}))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn stalled_tool_error_is_bounded_failed_not_canceled_and_known_nonbillable() {
    let f = Fixture::new().await;
    f.state.routing().cfg.nonstream_timeout_ms = 100;
    f.state.routing().cfg.request_budget_ms = 300;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        f.send("search", json!({"query":"stall-error"})),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    f.settle();
    let c = f.state.conn();
    let attempt: (i64, String) = c
        .query_row("SELECT count(*),outcome FROM attempts", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(attempt, (1, "tool_error".into()));
    assert_eq!(
        c.query_row("SELECT status FROM requests", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "failed"
    );
    assert_eq!(
        c.query_row(
            "SELECT request_units,debit_micro FROM billing_ledger",
            [],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?))
        )
        .unwrap(),
        (Some(0), Some(0))
    );
    assert!(!db::faults::has_fault(&c, f.channel, None).unwrap());
}
