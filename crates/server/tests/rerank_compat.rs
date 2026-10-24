//! Controlled HTTP acceptance for Jina/SiliconFlow response compatibility and accounting.
use axum::{routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokengo_server::{app, db, AppState, Config};

struct Fixture {
    state: Arc<AppState>,
    base: String,
    key: String,
    calls: Arc<AtomicUsize>,
    dir: std::path::PathBuf,
    jobs: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for job in &self.jobs {
            job.abort();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl Fixture {
    async fn new(raw: Value) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let upstream = Router::new().route(
            "/v1/rerank",
            post(move |Json(body): Json<Value>| {
                let raw = raw.clone();
                let counter = counter.clone();
                async move {
                    assert_eq!(body["model"], "provider-rerank");
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(raw)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = format!("http://{}/v1", listener.local_addr().unwrap());
        let upstream = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let dir =
            std::env::temp_dir().join(format!("tokengo-rerank-compat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db.sqlite"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        let key = db::gateway_keys::generate();
        {
            let c = state.conn();
            db::gateway_keys::create(&c, &state.secrets, "rerank-caller", &key, false).unwrap();
            let id = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "controlled-reranker".into(),
                    base_url: source,
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
                    "public-rerank",
                    Some("provider-rerank".into()),
                    false,
                )],
            )
            .unwrap();
            db::billing::set_budget(&c, "user", 1, Some(1_000_000), "test").unwrap();
            db::billing::set_budget(&c, "key", 1, Some(1_000_000), "test").unwrap();
            db::kinds::set(&c, "public-rerank", tokengo_shared::ModelKind::Rerank).unwrap();
            db::billing::create_price(
                &c,
                &db::billing::Price {
                    id: 0,
                    channel_id: id.0,
                    model: "public-rerank".into(),
                    input_micro_per_million: 1_000_000,
                    output_micro_per_million: 0,
                    max_input_tokens: 10_000,
                    max_output_tokens: 0,
                },
                "test",
            )
            .unwrap();
        }
        state.routing().cfg.retry_times = 0;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let gateway = app(state.clone());
        let gateway = tokio::spawn(async move { axum::serve(listener, gateway).await.unwrap() });
        Self {
            state,
            base,
            key,
            calls,
            dir,
            jobs: vec![upstream, gateway],
        }
    }
    async fn send(&self) -> (u16, Value) {
        let response=reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(10)).build().unwrap()
            .post(format!("{}/v1/rerank",self.base)).bearer_auth(&self.key)
            .json(&json!({"model":"public-rerank","query":"query","documents":["first document","second document","third document"],"return_documents":false}))
            .send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap())
    }
    fn latest(&self) -> db::billing::Ledger {
        self.state.drain_writes();
        db::billing::usage(
            &self.state.conn(),
            &db::billing::UsageQuery {
                user_id: None,
                key_id: None,
                channel_id: None,
                model: None,
                before_id: None,
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap()
        .remove(0)
    }
}
#[tokio::test]
async fn rerank_string_document_id_meta_and_raw_usage_survive_real_http() {
    let usage = json!({"input_tokens":17,"output_tokens":0,"search_units":2,"total_tokens":17,"provider_extra":"retained"});
    let f=Fixture::new(json!({"id":"original-provider-id","results":[{"index":0,"relevance_score":0.8,"document":"first document","provider_extension":true}],"usage":usage,"provider_top":"retained"})).await;
    let (status, v) = f.send().await;
    assert_eq!(status, 200);
    assert_eq!(
        v["results"][0]["document"],
        json!({"text":"first document"})
    );
    assert_eq!(v["id"], "original-provider-id");
    assert_eq!(v["usage"], usage);
    assert_eq!(
        v["meta"]["tokens"],
        json!({"input_tokens":17,"output_tokens":0})
    );
    assert_eq!(
        v["meta"]["billed_units"],
        json!({"search_units":2,"total_tokens":17})
    );
    assert_eq!(v["results"][0]["provider_extension"], true);
    assert_eq!(v["provider_top"], "retained");
    let entry = f.latest();
    assert_eq!(entry.input_tokens, Some(17));
    assert_eq!(entry.output_tokens, Some(0));
    assert_eq!(entry.usage_certainty, "reported");
    assert_eq!(entry.debit_micro, Some(17));
    let budgets = db::billing::budgets(&f.state.conn(), None, None).unwrap();
    assert_eq!(budgets.len(), 2);
    assert!(budgets
        .iter()
        .all(|b| b.reserved_micro == 0 && b.spent_micro == 17));
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn rerank_missing_id_gets_fresh_uuid_and_unknown_usage_stays_unknown() {
    let f=Fixture::new(json!({"results":[{"index":0,"relevance_score":0.8,"document":{"text":"already structured","language":"zh"}}]})).await;
    let (_, first) = f.send().await;
    let (_, second) = f.send().await;
    let a = uuid::Uuid::parse_str(first["id"].as_str().expect("missing generated id")).unwrap();
    let b = uuid::Uuid::parse_str(second["id"].as_str().unwrap()).unwrap();
    assert_ne!(a, b);
    assert_eq!(
        first["results"][0]["document"],
        json!({"text":"already structured","language":"zh"})
    );
    assert_eq!(first["meta"], json!({"billed_units":{},"tokens":{}}));
    let entry = f.latest();
    assert_eq!(entry.input_tokens, None);
    assert_eq!(entry.usage_certainty, "estimated");
    assert!(entry.debit_micro.unwrap() > 0);
}
#[tokio::test]
async fn rerank_keeps_existing_null_missing_and_empty_document_refill_contract() {
    let f=Fixture::new(json!({"id":"old-behavior","results":[{"index":0,"relevance_score":0.9,"document":null},{"index":1,"relevance_score":0.8},{"index":2,"relevance_score":0.7,"document":{}}]})).await;
    let (status, v) = f.send().await;
    assert_eq!(status, 200);
    for (i, text) in ["first document", "second document", "third document"]
        .iter()
        .enumerate()
    {
        assert_eq!(v["results"][i]["document"], json!({"text":text}));
    }
}
#[tokio::test]
async fn rerank_total_tokens_is_reported_input_and_settles_real_ledger() {
    let usage = json!({"total_tokens":23,"provider_meter":"original"});
    let f =
        Fixture::new(json!({"results":[{"index":0,"relevance_score":0.5}],"usage":usage})).await;
    let (status, v) = f.send().await;
    assert_eq!(status, 200);
    assert_eq!(v["usage"], usage);
    assert_eq!(v["meta"]["billed_units"]["total_tokens"], 23);
    let entry = f.latest();
    assert_eq!(entry.usage_certainty, "reported");
    assert_eq!(entry.input_tokens, Some(23));
    assert_eq!(entry.output_tokens, Some(0));
    assert_eq!(entry.debit_micro, Some(23));
    assert_eq!(entry.observed_cost_micro, Some(23));
    let budgets = db::billing::budgets(&f.state.conn(), None, None).unwrap();
    assert!(budgets.iter().all(|b| b.reserved_micro == 0));
}
#[tokio::test]
async fn rerank_negative_total_tokens_is_rejected_and_never_billed_as_reported_zero() {
    let f = Fixture::new(
        json!({"results":[{"index":0,"relevance_score":0.5}],"usage":{"total_tokens":-1}}),
    )
    .await;
    let (status, _) = f.send().await;
    assert_eq!(status, 502);
    let entry = f.latest();
    assert_ne!(entry.usage_certainty, "reported");
    assert_eq!(entry.input_tokens, None);
    assert!(entry.debit_micro.unwrap() > 0);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}
