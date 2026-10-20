use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use std::future::IntoFuture;
use tokengo_server::{db, litellm_import, AppState, Config, SharedState};
use tower::ServiceExt;
struct Fixture {
    state: SharedState,
    token: String,
    cfg: Config,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let state = AppState::init(cfg.clone()).unwrap();
        let token = db::sessions::create_session(&state.conn(), None).unwrap();
        Self { state, token, cfg }
    }
    async fn call(&self, action: &str, credential: Option<&str>, v: Value) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method("POST")
            .uri(format!("/api/import/litellm/{action}"))
            .header("content-type", "application/json");
        if let Some(k) = credential {
            req = req.header("authorization", format!("Bearer {k}"));
        }
        let res = litellm_import::router()
            .with_state(self.state.clone())
            .oneshot(req.body(Body::from(v.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn root(&self, action: &str, v: Value) -> Value {
        let (s, v) = self.call(action, Some(&self.token), v).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }
}
fn config() -> Value {
    json!({"import_options":{"name":"test-import","routing_mode":"ordered_fallback"},"model_list":[{"model_name":"logical","litellm_params":{"model":"openai/Exact/Upstream:A","api_base":"https://source.invalid/v1","api_key":"TEST-SECRET-NEVER-REPORT","http_headers":{"x-source-secret":"TEST-HEADER-NEVER-REPORT"}},"model_info":{"id":"a","mode":"chat"}},{"model_name":"logical","litellm_params":{"model":"openai/Exact/Upstream:B","api_base":"https://second.invalid/v1","api_key":"TEST-SECRET-NEVER-REPORT"},"model_info":{"id":"b","mode":"chat"}},{"model_name":"claude","litellm_params":{"model":"anthropic/claude-Exact","api_base":"https://anthropic.invalid","api_key":"TEST-SECRET-NEVER-REPORT"},"model_info":{"id":"c","mode":"chat"}}],"router_settings":{},"litellm_settings":{}})
}
#[tokio::test]
async fn strict_preview_apply_idempotent_update_restart_and_secret_redaction() {
    let f = Fixture::new();
    let before = db::channels::list(&f.state.conn()).unwrap().len();
    let preview = f.root("preview", config()).await;
    assert_eq!(preview["supported"], true);
    assert_eq!(preview["applied"], false);
    assert_eq!(db::channels::list(&f.state.conn()).unwrap().len(), before);
    for action in ["preview", "apply", "apply"] {
        let report = f.root(action, config()).await;
        assert_eq!(report["supported"], true, "{report}");
        let text = report.to_string();
        assert!(!text.contains("TEST-SECRET"));
        assert!(!text.contains("TEST-HEADER"));
    }
    let rows = db::channels::list(&f.state.conn()).unwrap();
    assert_eq!(rows.len(), before + 3);
    let a = rows
        .iter()
        .find(|r| {
            r.models
                .iter()
                .any(|m| m.upstream.as_deref() == Some("Exact/Upstream:A"))
        })
        .unwrap();
    let id = a.id;
    let policy = db::route_policy::load(&f.state.conn(), "logical").unwrap();
    assert_eq!(policy.channel_ids.as_ref().unwrap().len(), 2);
    assert_eq!(policy.retry_times, Some(0));
    assert_eq!(policy.probe_enabled, Some(false));
    assert_eq!(policy.include_in_auto, Some(false));
    assert_eq!(
        tokengo_server::protocol_config::load(&f.state.conn(), id.0)
            .unwrap()
            .native,
        vec![tokengo_server::protocol::Protocol::Chat]
    );
    let c = rows
        .iter()
        .find(|r| r.models.iter().any(|m| m.model == "claude"))
        .unwrap();
    assert_eq!(c.base_url, "https://anthropic.invalid/v1");
    assert_eq!(
        tokengo_server::protocol_config::load(&f.state.conn(), c.id.0)
            .unwrap()
            .native,
        vec![tokengo_server::protocol::Protocol::Messages]
    );
    assert!(rows
        .iter()
        .all(|r| r.headers.iter().all(|(_, v)| !v.contains("TEST-HEADER"))));
    let conn = f.state.conn();
    let ciphertexts: Vec<Vec<u8>> = conn
        .prepare("SELECT cipher FROM channel_secrets")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let needle = b"TEST-SECRET-NEVER-REPORT";
    assert!(ciphertexts
        .iter()
        .all(|bytes| !bytes.windows(needle.len()).any(|w| w == needle)));
    let metadata: String = conn
        .query_row("SELECT group_concat(value,'') FROM settings", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!metadata.contains("TEST-SECRET"));
    assert!(!metadata.contains("TEST-HEADER"));
    let audit: String = conn
        .query_row(
            "SELECT group_concat(metadata_json,'') FROM audit_events",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!audit.contains("TEST-SECRET"));
    assert!(!audit.contains("TEST-HEADER"));
    drop(conn);
    let mut changed = config();
    changed["model_list"][0]["litellm_params"]["model"] = json!("openai/New:Exact");
    changed["model_list"][0]["litellm_params"]["api_key"] = json!("TEST-ROTATED");
    assert_eq!(f.root("apply", changed).await["applied"], true);
    assert_eq!(
        db::channels::list(&f.state.conn()).unwrap().len(),
        rows.len()
    );
    let after = db::channels::get(&f.state.conn(), id).unwrap().unwrap();
    assert_eq!(after.models[0].upstream.as_deref(), Some("New:Exact"));
    assert_eq!(
        db::channels::secret(&f.state.conn(), &f.state.secrets, id)
            .unwrap()
            .as_deref(),
        Some("TEST-ROTATED")
    );
    let restart = AppState::init(f.cfg.clone()).unwrap();
    assert_eq!(
        db::channels::get(&restart.conn(), id)
            .unwrap()
            .unwrap()
            .models[0]
            .upstream
            .as_deref(),
        Some("New:Exact")
    );
}
#[tokio::test]
async fn unknown_fallback_env_provider_and_semantics_fail_closed_without_values() {
    let f = Fixture::new();
    for mutate in 0..8 {
        let mut v = config();
        match mutate {
            0 => v["router_settings"]["fallbacks"] = json!([{"logical":["claude"]}]),
            1 => v["model_list"][0]["litellm_params"]["temperature"] = json!("TEST-SECRET-VALUE"),
            2 => v["model_list"][0]["litellm_params"]["api_key"] = json!("os.environ/PRIVATE"),
            3 => v["model_list"][0]["litellm_params"]["model"] = json!("ollama/qwen"),
            4 => v["model_list"][0]["model_info"]["mode"] = Value::Null,
            5 => v["import_options"]["routing_mode"] = json!("simple-shuffle"),
            6 => v["model_list"][1]["model_info"]["id"] = json!("a"),
            _ => {
                v["model_list"][0]["litellm_params"]["TEST-SECRET-FIELD"] =
                    json!("TEST-SECRET-VALUE")
            }
        };
        for action in ["preview", "apply"] {
            let report = f.root(action, v.clone()).await;
            assert_eq!(report["supported"], false, "case {mutate}: {report}");
            assert_eq!(report["applied"], false);
            assert!(!report.to_string().contains("TEST-SECRET"));
        }
        assert!(db::channels::list(&f.state.conn()).unwrap().is_empty());
    }
}
#[tokio::test]
async fn panel_only_manual_conflicts_and_database_failure_roll_back() {
    let f = Fixture::new();
    assert_eq!(
        f.call("preview", None, config()).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, vkey) = db::identity::create_key(
        &f.state.conn(),
        1,
        "import-forbidden",
        &Default::default(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        f.call("apply", Some(&vkey), config()).await.0,
        StatusCode::UNAUTHORIZED
    );
    f.state.conn().execute_batch("CREATE TRIGGER import_abort BEFORE INSERT ON channel_models WHEN NEW.upstream_model='Exact/Upstream:B' BEGIN SELECT RAISE(ABORT,'TEST-SECRET-SQL-ERROR'); END;").unwrap();
    let (status, error) = f.call("apply", Some(&f.token), config()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!error.to_string().contains("TEST-SECRET"));
    assert!(db::channels::list(&f.state.conn()).unwrap().is_empty());
    let count: i64 = f
        .state
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key LIKE 'litellm_import:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    f.state
        .conn()
        .execute_batch("DROP TRIGGER import_abort")
        .unwrap();
    assert_eq!(f.root("apply", config()).await["applied"], true);
    let mut policy = db::route_policy::load(&f.state.conn(), "logical").unwrap();
    policy.retry_times = Some(1);
    db::route_policy::save(&f.state.conn(), "logical", &policy).unwrap();
    assert_eq!(f.root("apply", config()).await["supported"], false);
}
#[tokio::test]
async fn removals_preserve_sources_and_manual_aliases() {
    let f = Fixture::new();
    assert_eq!(f.root("apply", config()).await["applied"], true);
    let rows = db::channels::list(&f.state.conn()).unwrap();
    let id = rows[0].id;
    let mut models = rows[0].models.clone();
    models.push(db::channels::ModelMapping::new("manual-alias", None, false));
    db::channels::set_models(&f.state.conn(), id, &models).unwrap();
    let mut v = config();
    v["model_list"].as_array_mut().unwrap().remove(1);
    let r = f.root("apply", v).await;
    assert_eq!(r["applied"], true, "{r}");
    assert_eq!(r["retained_removed_deployments"], 1);
    assert_eq!(db::channels::list(&f.state.conn()).unwrap().len(), 3);
    assert!(db::channels::get(&f.state.conn(), id)
        .unwrap()
        .unwrap()
        .models
        .iter()
        .any(|m| m.model == "manual-alias"));
}
#[tokio::test]
async fn imported_native_protocols_use_source_credentials_and_exact_names() {
    use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};
    type Seen = Arc<Mutex<Vec<(HeaderMap, Value)>>>;
    async fn source(
        State(seen): State<Seen>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        seen.lock().unwrap().push((headers, body.clone()));
        if body["model"] == "claude-Exact" {
            Json(
                json!({"id":"msg_fixture","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}),
            )
        } else {
            Json(
                json!({"id":"chat_fixture","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}),
            )
        }
    }
    let f = Fixture::new();
    let seen: Seen = Default::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(source))
                .route("/v1/messages", post(source))
                .with_state(seen.clone()),
        )
        .into_future(),
    );
    let mut v = config();
    v["model_list"].as_array_mut().unwrap().remove(1);
    v["model_list"][0]["litellm_params"]["api_base"] = json!(format!("{base}/v1"));
    v["model_list"][1]["litellm_params"]["api_base"] = json!(base);
    assert_eq!(f.root("apply", v).await["applied"], true);
    let (_, key) = db::identity::create_key(
        &f.state.conn(),
        1,
        "native-import-client",
        &Default::default(),
        None,
        None,
    )
    .unwrap();
    for (path, body) in [
        (
            "chat/completions",
            json!({"model":"logical","messages":[{"role":"user","content":"hi"}],"max_tokens":4}),
        ),
        (
            "messages",
            json!({"model":"claude","messages":[{"role":"user","content":"hi"}],"max_tokens":4}),
        ),
    ] {
        let req = Request::builder()
            .method("POST")
            .uri(format!("/v1/{path}"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {key}"))
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = tokengo_server::app(f.state.clone())
            .oneshot(req)
            .await
            .unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    }
    let observed = seen.lock().unwrap();
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0].1["model"], "Exact/Upstream:A");
    assert_eq!(
        observed[0].0["authorization"],
        "Bearer TEST-SECRET-NEVER-REPORT"
    );
    assert_eq!(observed[1].1["model"], "claude-Exact");
    assert_eq!(observed[1].0["x-api-key"], "TEST-SECRET-NEVER-REPORT");
    assert_eq!(observed[1].0["anthropic-version"], "2023-06-01");
    assert!(!observed[1].0.contains_key("authorization"));
    task.abort();
}
fn fallback_config() -> Value {
    let mut v = config();
    v["model_list"][0]["model_name"] = json!("A");
    v["model_list"][1]["model_name"] = json!("B");
    v["model_list"][2]["model_name"] = json!("C");
    v["model_list"][2]["litellm_params"]["model"] = json!("openai/C-exact");
    v["router_settings"] = json!({"fallbacks":[{"A":["B","C"]},{"B":["C"]}]});
    v
}
#[tokio::test]
async fn multilevel_fallback_maps_exact_names_and_rejects_invalid_graphs() {
    let f = Fixture::new();
    let v = fallback_config();
    let report = f.root("preview", v.clone()).await;
    assert_eq!(report["supported"], true, "{report}");
    assert_eq!(f.root("apply", v.clone()).await["applied"], true);
    let rows = db::channels::list(&f.state.conn()).unwrap();
    let a = rows
        .iter()
        .find(|r| {
            r.models
                .iter()
                .any(|m| m.upstream.as_deref() == Some("Exact/Upstream:A"))
        })
        .unwrap();
    let b = rows
        .iter()
        .find(|r| {
            r.models
                .iter()
                .any(|m| m.upstream.as_deref() == Some("Exact/Upstream:B"))
        })
        .unwrap();
    let c = rows
        .iter()
        .find(|r| {
            r.models
                .iter()
                .any(|m| m.upstream.as_deref() == Some("C-exact"))
        })
        .unwrap();
    assert_eq!(
        db::route_policy::load(&f.state.conn(), "A")
            .unwrap()
            .channel_ids,
        Some(vec![a.id.0, b.id.0, c.id.0])
    );
    assert_eq!(
        db::route_policy::load(&f.state.conn(), "B")
            .unwrap()
            .channel_ids,
        Some(vec![b.id.0, c.id.0])
    );
    assert_eq!(
        db::route_policy::load(&f.state.conn(), "C")
            .unwrap()
            .channel_ids,
        Some(vec![c.id.0])
    );
    assert!(c
        .models
        .iter()
        .any(|m| m.model == "A" && m.upstream.as_deref() == Some("C-exact")));
    assert_eq!(f.root("apply", v.clone()).await["applied"], true);
    for case in 0..6 {
        let mut invalid = v.clone();
        match case {
            0 => invalid["router_settings"]["fallbacks"] = json!([{"A":["B"]},{"B":["A"]}]),
            1 => invalid["router_settings"]["fallbacks"] = json!([{"A":["MISSING"]}]),
            2 => invalid["model_list"][1]["model_info"]["mode"] = json!("embedding"),
            3 => invalid["router_settings"]["fallbacks"] = json!([{"A":["B","B"]}]),
            4 => invalid["router_settings"]["context_window_fallbacks"] = json!([{"A":["B"]}]),
            _ => invalid["litellm_settings"]["fallbacks"] = json!([{"A":["B"]}]),
        };
        let r = f.root("apply", invalid).await;
        assert_eq!(r["supported"], false, "case {case}:{r}");
        assert_eq!(
            db::route_policy::load(&f.state.conn(), "A")
                .unwrap()
                .channel_ids,
            Some(vec![a.id.0, b.id.0, c.id.0])
        );
    }
}
#[tokio::test]
async fn imported_fallback_chain_executes_a_b_c_once_with_global_retries_unchanged() {
    use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};
    type Seen = Arc<Mutex<Vec<String>>>;
    async fn source(State(seen): State<Seen>, Json(body): Json<Value>) -> axum::response::Response {
        let model = body["model"].as_str().unwrap().to_string();
        seen.lock().unwrap().push(model.clone());
        if model != "C-exact" {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error":{"message":"synthetic rate limit","type":"rate_limit_error"}})),
            )
                .into_response();
        }
        Json(json!({"id":"chat_fallback","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"C answered"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})).into_response()
    }
    let f = Fixture::new();
    f.state.routing().cfg.retry_times = 3;
    let seen: Seen = Default::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(source))
                .with_state(seen.clone()),
        )
        .into_future(),
    );
    let mut v = fallback_config();
    for d in v["model_list"].as_array_mut().unwrap() {
        d["litellm_params"]["api_base"] = json!(base);
    }
    assert_eq!(f.root("apply", v).await["applied"], true);
    assert_eq!(f.state.routing().cfg.retry_times, 3);
    let (_, key) = db::identity::create_key(
        &f.state.conn(),
        1,
        "fallback-import-client",
        &Default::default(),
        None,
        None,
    )
    .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {key}"))
        .body(Body::from(
            json!({"model":"A","messages":[{"role":"user","content":"hi"}],"max_tokens":4})
                .to_string(),
        ))
        .unwrap();
    let response = tokengo_server::app(f.state.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["Exact/Upstream:A", "Exact/Upstream:B", "C-exact"]
    );
    assert_eq!(f.state.routing().cfg.retry_times, 3);
    task.abort();
}
#[tokio::test]
async fn reimport_preserves_source_disable_and_detects_deleted_policy() {
    let f = Fixture::new();
    assert_eq!(f.root("apply", config()).await["applied"], true);
    let row = db::channels::list(&f.state.conn()).unwrap().remove(0);
    db::channels::update(
        &f.state.conn(),
        row.id,
        &row.name,
        &row.base_url,
        false,
        row.unwrap_envelope,
        2,
    )
    .unwrap();
    assert_eq!(f.root("apply", config()).await["applied"], true);
    let after = db::channels::get(&f.state.conn(), row.id).unwrap().unwrap();
    assert!(
        !after.enabled,
        "import must not reenable a source disabled outside import"
    );
    assert_eq!(after.max_concurrent, 2);
    db::route_policy::delete(&f.state.conn(), "logical").unwrap();
    assert_eq!(f.root("preview", config()).await["supported"], false);
}
