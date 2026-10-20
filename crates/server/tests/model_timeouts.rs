//! Logical timeout overrides: root-only API and real loopback upstream deadlines.
use axum::{routing::post, Json, Router};
use serde_json::{json, Value};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tokengo_shared::routing::RoutingConfig;
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
async fn slow(Json(body): Json<Value>) -> axum::response::Response {
    use axum::response::IntoResponse;
    tokio::time::sleep(std::time::Duration::from_millis(160)).await;
    if body["stream"] == true {
        return ([("content-type","text/event-stream")],"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").into_response();
    }
    Json(json!({"id":"timeout-fixture","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})).into_response()
}
struct Fixture {
    state: SharedState,
    cfg: Config,
    gateway: Server,
    _source: Server,
    root: String,
    key: String,
}
impl Fixture {
    async fn new() -> Self {
        let source = serve(Router::new().route("/v1/chat/completions", post(slow))).await;
        let dir = std::env::temp_dir().join(format!("tokengo-timeouts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let state = AppState::init(cfg.clone()).unwrap();
        {
            let mut r = state.routing();
            r.cfg.retry_times = 0;
            r.cfg.probe_enabled = false;
            r.cfg.attempt_timeout_ms = 35;
            r.cfg.nonstream_timeout_ms = 35;
            r.cfg.request_budget_ms = 90;
        }
        let root = db::sessions::create_session(&state.conn(), None).unwrap();
        let (_, key) = db::identity::create_key(
            &state.conn(),
            1,
            "deadline-client",
            &Default::default(),
            None,
            None,
        )
        .unwrap();
        {
            let c = state.conn();
            let id = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "deadline-source".into(),
                    base_url: format!("{}/v1", source.url),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap();
            db::channels::set_models(
                &c,
                id,
                &[
                    db::channels::ModelMapping::new("override", None, false),
                    db::channels::ModelMapping::new("inherit", None, false),
                ],
            )
            .unwrap();
            db::channels::set_secret(&c, &state.secrets, id, "SYNTHETIC-DEADLINE-SOURCE").unwrap();
        }
        let gateway = serve(app(state.clone())).await;
        Self {
            state,
            cfg,
            gateway,
            _source: source,
            root,
            key,
        }
    }
    async fn api(
        &self,
        method: &str,
        key: Option<&str>,
        body: Value,
    ) -> (reqwest::StatusCode, Value) {
        let mut req = reqwest::Client::new()
            .request(
                method.parse().unwrap(),
                format!("{}/api/model-routing?model=override", self.gateway.url),
            )
            .json(&body);
        if let Some(key) = key {
            req = req.bearer_auth(key);
        }
        let res = req.send().await.unwrap();
        let status = res.status();
        (status, res.json().await.unwrap())
    }
    async fn call(&self, model: &str, stream: bool) -> reqwest::Response {
        reqwest::Client::new().post(format!("{}/v1/chat/completions",self.gateway.url)).bearer_auth(&self.key).json(&json!({"model":model,"stream":stream,"max_tokens":4,"messages":[{"role":"user","content":"hello"}]})).send().await.unwrap()
    }
}
#[tokio::test]
async fn timeout_api_validates_persists_restarts_deletes_and_preserves_inheritance() {
    let f = Fixture::new().await;
    let old = RoutingConfig::default();
    let legacy: db::route_policy::RoutePolicy =
        serde_json::from_value(json!({"retry_times":0,"probe_enabled":false})).unwrap();
    db::route_policy::save(&f.state.conn(), "inherit", &legacy).unwrap();
    let inherited = db::route_policy::effective(&f.state.conn(), "inherit", old.clone()).unwrap();
    assert_eq!(inherited.attempt_timeout_ms, old.attempt_timeout_ms);
    assert_eq!(inherited.nonstream_timeout_ms, old.nonstream_timeout_ms);
    assert_eq!(inherited.request_budget_ms, old.request_budget_ms);
    assert_eq!(f.api("GET", None, json!({})).await.0, 401);
    assert_eq!(
        f.api("PUT", Some(&f.key), json!({"attempt_timeout_ms":100}))
            .await
            .0,
        401
    );
    let target = json!({"attempt_timeout_ms":900000,"nonstream_timeout_ms":300000,"request_budget_ms":120000});
    let (status, response) = f.api("PUT", Some(&f.root), target.clone()).await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["policy"], target);
    assert_eq!(
        f.api("GET", Some(&f.root), json!({})).await.1["policy"],
        target
    );
    for field in [
        "attempt_timeout_ms",
        "nonstream_timeout_ms",
        "request_budget_ms",
    ] {
        for invalid in [
            json!(0),
            json!(3600001u64),
            json!(-1),
            json!(1.5),
            json!("100"),
            json!(u64::MAX),
        ] {
            let mut body = target.clone();
            body[field] = invalid;
            let (status, body) = f.api("PUT", Some(&f.root), body).await;
            assert_eq!(status, 400, "{field}: {body}");
            assert_eq!(
                f.api("GET", Some(&f.root), json!({})).await.1["policy"],
                target
            );
        }
    }
    let restarted = AppState::init(f.cfg.clone()).unwrap();
    let effective =
        db::route_policy::effective(&restarted.conn(), "override", old.clone()).unwrap();
    assert_eq!(effective.attempt_timeout_ms, 900000);
    assert_eq!(effective.nonstream_timeout_ms, 300000);
    assert_eq!(effective.request_budget_ms, 120000);
    assert_eq!(f.api("DELETE", Some(&f.root), json!({})).await.0, 200);
    let effective = db::route_policy::effective(&f.state.conn(), "override", old.clone()).unwrap();
    assert_eq!(effective.attempt_timeout_ms, old.attempt_timeout_ms);
    assert_eq!(effective.nonstream_timeout_ms, old.nonstream_timeout_ms);
    assert_eq!(effective.request_budget_ms, old.request_budget_ms);
    assert_eq!(
        f.api(
            "PUT",
            Some(&f.root),
            json!({"attempt_timeout_ms":1,"nonstream_timeout_ms":3600000,"request_budget_ms":1})
        )
        .await
        .0,
        200
    );
}
#[tokio::test]
async fn actual_stream_and_nonstream_deadlines_apply_only_to_selected_model() {
    let f = Fixture::new().await;
    let (status, body) = f
        .api(
            "PUT",
            Some(&f.root),
            json!({"attempt_timeout_ms":1000,"nonstream_timeout_ms":1000,"request_budget_ms":1500}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    for stream in [false, true] {
        let res = f.call("override", stream).await;
        let status = res.status();
        let body = res.text().await.unwrap();
        assert_eq!(status, 200, "override stream={stream}: {body}");
        let res = f.call("inherit", stream).await;
        assert!(
            res.status().is_server_error(),
            "inherit must retain short global deadline"
        );
        res.bytes().await.unwrap();
    }
    let global = f.state.routing().cfg.clone();
    assert_eq!(global.attempt_timeout_ms, 35);
    assert_eq!(global.nonstream_timeout_ms, 35);
    assert_eq!(global.request_budget_ms, 90);
    // Budget may be shorter than attempt; gateway must use its existing min constraint.
    assert_eq!(
        f.api(
            "PUT",
            Some(&f.root),
            json!({"attempt_timeout_ms":1000,"nonstream_timeout_ms":1000,"request_budget_ms":35})
        )
        .await
        .0,
        200
    );
    let res = f.call("override", false).await;
    assert!(
        res.status().is_server_error(),
        "logical total budget must bound a longer attempt"
    );
}
