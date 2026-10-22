//! Opt-in acceptance against real upstreams. Credentials live only in memory.
//! Run explicitly with TOKENGO_REAL_VECTOR_SOURCES and --ignored --nocapture.
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokengo_server::{
    app,
    db::{self, user_virtual_routes as vr},
    vector_compat, AppState, Config, SharedState,
};
use tokengo_shared::{ChannelId, ModelKind};
use tower::ServiceExt;

#[derive(Clone, Deserialize)]
struct Source {
    label: String,
    url: String,
    model: String,
    headers: std::collections::BTreeMap<String, String>,
}
struct AdapterState {
    source: Source,
    failed: AtomicBool,
    client: reqwest::Client,
}
struct Adapter {
    state: Arc<AdapterState>,
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Adapter {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn forward(
    State(state): State<Arc<AdapterState>>,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    if state.failed.load(Ordering::SeqCst) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":{"message":"controlled cloud failure"}})),
        )
            .into_response();
    }
    let endpoint = uri.path().rsplit('/').next().unwrap();
    let mut request = state
        .client
        .post(format!(
            "{}/{}",
            state.source.url.trim_end_matches('/'),
            endpoint
        ))
        .json(&body);
    for (name, value) in &state.source.headers {
        request = request.header(name, value);
    }
    let Ok(response) = request.send().await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
    let Ok(bytes) = response.bytes().await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    // Raw upstream error bodies are intentionally suppressed, including in failed assertions.
    if !status.is_success() {
        return (
            status,
            Json(json!({"error":{"message":"real upstream rejected request"}})),
        )
            .into_response();
    }
    (status, [("content-type", "application/json")], bytes).into_response()
}
impl Adapter {
    async fn start(source: Source) -> Self {
        let state = Arc::new(AdapterState {
            source,
            failed: AtomicBool::new(false),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(45))
                .build()
                .unwrap(),
        });
        let router = Router::new()
            .route("/v1/embeddings", post(forward))
            .route("/v1/rerank", post(forward))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { state, base, task }
    }
}
struct Fixture {
    state: SharedState,
    dir: std::path::PathBuf,
    key: String,
    user: i64,
    config: vr::Config,
    ids: Vec<ChannelId>,
    adapters: Vec<Adapter>,
    kind: ModelKind,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl Fixture {
    async fn new(kind: ModelKind, sources: &[Source]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("vector-real-sources-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        let (user, key) = {
            let c = state.conn();
            let user =
                db::identity::create_user(&c, "real-vector-owner", "user", &Default::default())
                    .unwrap()
                    .id;
            let (_, key) =
                db::identity::create_key(&c, user, "client", &Default::default(), None, None)
                    .unwrap();
            (user, key)
        };
        let mut adapters = Vec::new();
        let mut ids = Vec::new();
        let mut targets = Vec::new();
        for (i, source) in sources.iter().enumerate() {
            let adapter = Adapter::start(source.clone()).await;
            let name = format!("real-vector-{i}");
            let c = state.conn();
            let id = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: name.clone(),
                    base_url: adapter.base.clone(),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap();
            // Actual upstream credentials stay in adapter memory; temporary DB stores no real secret.
            db::channels::set_secret(&c, &state.secrets, id, "synthetic-adapter-only").unwrap();
            db::channels::set_models(
                &c,
                id,
                &[db::channels::ModelMapping::new(
                    &name,
                    Some(source.model.clone()),
                    false,
                )],
            )
            .unwrap();
            db::kinds::set(&c, &name, kind).unwrap();
            db::provider_pool::backfill(&c, &state.secrets).unwrap();
            let provider = db::provider_pool::source_credential(&c, id.0)
                .unwrap()
                .unwrap()
                .provider_id;
            targets.push(vr::Target {
                provider_id: provider,
                logical_model: name.clone(),
                switch_on_429: true,
                switch_on_400: false,
            });
            db::billing::create_price(
                &c,
                &db::billing::Price {
                    id: 0,
                    channel_id: id.0,
                    model: name,
                    input_micro_per_million: 1_000_000,
                    output_micro_per_million: 0,
                    max_input_tokens: 50000,
                    max_output_tokens: 0,
                },
                "real-vector-test",
            )
            .unwrap();
            ids.push(id);
            adapters.push(adapter);
        }
        state.routing().cfg.retry_times = 0;
        let config = vr::Config {
            name: "verified-real-vector".into(),
            enabled: true,
            mode: "ordered".into(),
            timezone: "UTC".into(),
            targets,
            windows: vec![],
            default_positions: vec![],
        };
        Self {
            state,
            dir,
            key,
            user,
            config,
            ids,
            adapters,
            kind,
        }
    }
    async fn call(&self) -> Value {
        let (path, body) = if self.kind == ModelKind::Embeddings {
            (
                "/v1/embeddings",
                json!({"model":self.config.name,"input":["向量检索按语义匹配资料。","Semantic search retrieves relevant documents."],"encoding_format":"float"}),
            )
        } else {
            (
                "/v1/rerank",
                json!({"model":self.config.name,"query":"向量模型有什么作用？","documents":["向量模型把文本转换为数值向量，用于语义检索。","今天天气晴朗。","重排模型为候选文档评分。"],"top_n":2,"return_documents":true}),
            )
        };
        let response = app(self.state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", self.key))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "real route failed; response body suppressed"
        );
        let out: Value = serde_json::from_slice(&bytes).expect("real route returned invalid JSON");
        if self.kind == ModelKind::Embeddings {
            let data = out["data"].as_array().unwrap();
            assert_eq!(data.len(), 2);
            for (i, x) in data.iter().enumerate() {
                assert_eq!(x["index"], i);
                assert_eq!(x["embedding"].as_array().unwrap().len(), 1024);
            }
        } else {
            let results = out["results"].as_array().unwrap();
            assert_eq!(results.len(), 2);
            for x in results {
                let i = x["index"].as_u64().unwrap() as usize;
                assert!(i < 3);
                assert_eq!(x["document"]["text"], body["documents"][i]);
                assert!(x["relevance_score"].as_f64().unwrap().is_finite());
            }
        }
        out
    }
    async fn flush(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while self.state.pending_writes() > 0 {
                self.state.drain_writes();
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}
#[tokio::test]
#[ignore = "requires explicit real-source credentials and makes bounded synthetic upstream calls"]
async fn real_cloud_to_gpu_embedding_and_rerank_fallback() {
    let raw = std::env::var("TOKENGO_REAL_VECTOR_SOURCES")
        .expect("explicit real-source environment required");
    let sources: Vec<Source> = serde_json::from_str(&raw).expect("invalid real-source shape");
    for (kind, labels) in [
        (ModelKind::Embeddings, ["cloud_embedding", "gpu_embedding"]),
        (ModelKind::Rerank, ["cloud_rerank", "gpu_rerank"]),
    ] {
        let selected: Vec<_> = labels
            .iter()
            .map(|label| {
                sources
                    .iter()
                    .find(|s| s.label == *label)
                    .expect("necessary source missing")
                    .clone()
            })
            .collect();
        let f = Fixture::new(kind, &selected).await;
        let result = vector_compat::verify(&f.state, f.user, &f.config).await;
        if let Err(error) = result {
            panic!("real {:?} compatibility rejected: {}", kind, error.code);
        }
        vr::create(&f.state.conn(), f.user, &f.config).expect("verified route did not save");
        {
            let c = f.state.conn();
            let diagnostics:i64=c.query_row("SELECT count(*) FROM billing_ledger WHERE request_id=0 AND key_id=0 AND user_id=?1 AND json_extract(auth_snapshot,'$.kind')='user_compatibility' AND settled_ms IS NOT NULL",[f.user],|r|r.get(0)).unwrap();
            assert!(
                diagnostics >= 4,
                "real compatibility probes must create settled user-owned diagnostic ledger rows"
            );
            let unsettled:i64=c.query_row("SELECT count(*) FROM billing_ledger WHERE request_id=0 AND (user_id<>?1 OR key_id<>0 OR settled_ms IS NULL OR json_extract(auth_snapshot,'$.kind')<>'user_compatibility')",[f.user],|r|r.get(0)).unwrap();
            assert_eq!(
                unsettled, 0,
                "diagnostic ownership and settlement must be complete"
            );
        }
        f.call().await;
        f.flush().await;
        f.adapters[0].state.failed.store(true, Ordering::SeqCst);
        f.call().await;
        f.flush().await;
        let c = f.state.conn();
        let mut query = c
            .prepare(
                "SELECT channel_id FROM attempts WHERE request_id>0 ORDER BY request_id,ordinal",
            )
            .unwrap();
        let attempts = query
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(attempts, vec![f.ids[0].0, f.ids[0].0, f.ids[1].0]);
        let mut query=c.prepare("SELECT channel_id FROM billing_ledger WHERE request_id>0 AND settled_ms IS NOT NULL ORDER BY id").unwrap();
        let billed = query
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(billed, vec![f.ids[0].0, f.ids[0].0, f.ids[1].0]);
        println!("real {:?}: compatibility/save/cloud success/controlled 503/GPU fallback/output/attempt ledger/billing passed",kind);
    }
}
