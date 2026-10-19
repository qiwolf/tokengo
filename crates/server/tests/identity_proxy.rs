//! HTTP acceptance of authorization and accounting at the real forwarding boundary.
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
        for j in &self.jobs {
            j.abort();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl Fixture {
    async fn new() -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let upstream=Router::new().route("/v1/chat/completions",post(move |Json(body):Json<Value>| {let c=c.clone();async move {c.fetch_add(1,Ordering::SeqCst);
    use axum::response::IntoResponse;
    if body["stream"]==true {
        let chunks=futures_util::stream::iter([Ok::<_,std::io::Error>(bytes::Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\n"))]);
        let stream= futures_util::StreamExt::chain(chunks,futures_util::stream::pending());
        return axum::response::Response::builder().header("content-type","text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap();
    }
    Json(json!({"choices":[{"message":{"content":"hello"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}})).into_response()
  } })).route("/v1/embeddings",post(||async {Json(json!({"data":[{"index":0,"embedding":[0.1,0.2]}],"usage":{"prompt_tokens":12}}))}))
  .route("/missing/v1/chat/completions",post(||async {Json(json!({"choices":[{"message":{"content":"no usage"}}]}))}))
  .route("/fault/v1/chat/completions",post(||async{(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"message":"failed"},"usage":{"prompt_tokens":3,"completion_tokens":1}})))}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let up = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let dir =
            std::env::temp_dir().join(format!("tokengo-identity-proxy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db.sqlite"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        let key = db::gateway_keys::generate();
        {
            let conn = state.conn();
            db::gateway_keys::create(&conn, &state.secrets, "caller", &key, false).unwrap();
            let id = db::channels::create(
                &conn,
                &db::channels::NewChannel {
                    name: "source".into(),
                    base_url: url,
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap();
            db::channels::set_models(
                &conn,
                id,
                &[
                    db::channels::ModelMapping::new("allowed", None, false),
                    db::channels::ModelMapping::new("secret", None, false),
                ],
            )
            .unwrap();
            db::settings::set(&conn, "retry_times", "0").unwrap();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = app(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
        Self {
            state,
            base,
            key,
            calls,
            dir,
            jobs: vec![up, server],
        }
    }
    async fn send(&self, path: &str, body: Value) -> (u16, Value) {
        let r = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
            .post(format!("{}{path}", self.base))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap())
    }
    fn policy(&self, value: Value) {
        self.state
            .conn()
            .execute(
                "UPDATE gateway_keys SET policy_json=?1",
                [value.to_string()],
            )
            .unwrap();
    }
}
fn body(model: &str) -> Value {
    json!({"model":model,"messages":[{"role":"user","content":"hi"}],"max_tokens":20})
}
#[tokio::test]
async fn model_permission_is_checked_before_call_and_hidden_from_list() {
    let f = Fixture::new().await;
    f.policy(json!({"allowed_models":["allowed"],"allow_paid":true}));
    let (s, v) = f.send("/v1/chat/completions", body("secret")).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    let r = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/v1/models", f.base))
        .bearer_auth(&f.key)
        .send()
        .await
        .unwrap();
    let v: Value = r.json().await.unwrap();
    let ids: Vec<_> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["allowed"]);
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
}
#[tokio::test]
async fn channel_and_paid_permission_cannot_be_bypassed_by_alias() {
    let f = Fixture::new().await;
    f.policy(json!({"allowed_channels":[],"allow_paid":true}));
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 403);
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    f.policy(json!({"allow_paid":false}));
    // Unknown tariffs are never assumed free.
    assert_eq!(f.send("/v1/chat/completions", body("secret")).await.0, 403);
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn revoked_key_stops_after_cached_success() {
    let f = Fixture::new().await;
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
    f.state
        .conn()
        .execute("UPDATE gateway_keys SET revoked_at=1", [])
        .unwrap();
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 401);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}
fn tariff(f: &Fixture) {
    let conn = f.state.conn();
    db::billing::create_price(
        &conn,
        &db::billing::Price {
            id: 0,
            channel_id: 1,
            model: "allowed".into(),
            input_micro_per_million: 1_000_000,
            output_micro_per_million: 2_000_000,
            max_input_tokens: 10_000,
            max_output_tokens: 1_000,
        },
        "test",
    )
    .unwrap();
}
#[tokio::test]
async fn real_usage_settles_both_budgets_with_identity_and_price_snapshot() {
    let f = Fixture::new().await;
    tariff(&f);
    {
        let c = f.state.conn();
        db::billing::set_budget(&c, "user", 1, Some(2000), "test").unwrap();
        db::billing::set_budget(&c, "key", 1, Some(1000), "test").unwrap();
    }
    let (s, v) = f.send("/v1/chat/completions", body("allowed")).await;
    assert_eq!(s, 200, "{v}");
    let c = f.state.conn();
    let ledger = db::billing::usage(&c, &Default::default()).unwrap();
    assert_eq!(ledger.len(), 1);
    let entry = &ledger[0];
    assert_eq!(entry.source_snapshot["upstream_model"], "allowed");
    assert_eq!(entry.source_snapshot["channel_name"], "source");
    assert_eq!(entry.user_id, 1);
    assert_eq!(entry.key_id, 1);
    assert_eq!(entry.input_tokens, Some(10));
    assert_eq!(entry.output_tokens, Some(2));
    assert_eq!(entry.observed_cost_micro, Some(14));
    assert_eq!(entry.debit_micro, Some(14));
    assert!(entry.price_id.is_some());
    for b in db::billing::budgets(&c, None, None).unwrap() {
        assert_eq!(b.spent_micro, 14);
        assert_eq!(b.reserved_micro, 0);
    }
    let row: (i64, i64) = c
        .query_row(
            "SELECT user_id,key_id FROM requests ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row, (1, 1));
}
#[tokio::test]
async fn budget_denial_precedes_any_provider_call() {
    let f = Fixture::new().await;
    tariff(&f);
    db::billing::set_budget(&f.state.conn(), "key", 1, Some(10), "test").unwrap();
    let (s, v) = f.send("/v1/chat/completions", body("allowed")).await;
    assert_eq!(s, 429, "{v}");
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn unpriced_usage_is_unknown_and_finite_budget_requires_pricing() {
    let f = Fixture::new().await;
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
    {
        let c = f.state.conn();
        let rows = db::billing::usage(&c, &Default::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].observed_cost_micro, None);
        assert_eq!(rows[0].debit_micro, None);
        assert_eq!(rows[0].input_tokens, Some(10));
        db::billing::set_budget(&c, "key", 1, Some(100), "test").unwrap();
    }
    let (s, _) = f.send("/v1/chat/completions", body("allowed")).await;
    assert_eq!(s, 403);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn key_request_rate_is_enforced_before_forwarding() {
    // Admission Drop behavior has dedicated tests; this proves the gateway owns its permit.
    let f = Fixture::new().await;
    f.policy(json!({"rpm":1}));
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
    let (s, v) = f.send("/v1/chat/completions", body("allowed")).await;
    assert_eq!(s, 429, "{v}");
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn fallback_charges_each_attempt_without_reusing_previous_usage() {
    let f = Fixture::new().await;
    let original = {
        let c = f.state.conn();
        c.query_row("SELECT base_url FROM channels WHERE id=1", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap()
    };
    {
        let c = f.state.conn();
        c.execute(
            "UPDATE channels SET base_url=?1 WHERE id=1",
            [original.replace("/v1", "/fault/v1")],
        )
        .unwrap();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "backup".into(),
                base_url: original,
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        db::channels::set_models(
            &c,
            id,
            &[db::channels::ModelMapping::new("allowed", None, false)],
        )
        .unwrap();
        db::route_policy::save(
            &c,
            "allowed",
            &db::route_policy::RoutePolicy {
                retry_times: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        for channel in [1, 2] {
            db::billing::create_price(
                &c,
                &db::billing::Price {
                    id: 0,
                    channel_id: channel,
                    model: "allowed".into(),
                    input_micro_per_million: 1_000_000,
                    output_micro_per_million: 2_000_000,
                    max_input_tokens: 10_000,
                    max_output_tokens: 1_000,
                },
                "test",
            )
            .unwrap();
        }
    }
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
    let c = f.state.conn();
    let rows = db::billing::usage(&c, &Default::default()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().map(|r| r.debit_micro.unwrap()).sum::<i64>(), 19);
    assert!(rows.iter().all(|r| r.status != "reserved"));
    let attempts: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM attempts WHERE user_id=1 AND key_id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 2);
}
#[tokio::test]
async fn stream_cancel_settles_partial_usage_and_returns_concurrency_slot() {
    use futures_util::StreamExt;
    let f = Fixture::new().await;
    tariff(&f);
    f.policy(json!({"max_concurrent":1}));
    let mut b = body("allowed");
    b["stream"] = json!(true);
    let r = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{}/v1/chat/completions", f.base))
        .bearer_auth(&f.key)
        .json(&b)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let mut stream = r.bytes_stream();
    let _ = stream.next().await.unwrap().unwrap();
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 429);
    drop(stream);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let settled = {
            let c = f.state.conn();
            db::billing::usage(&c, &Default::default())
                .unwrap()
                .iter()
                .any(|r| r.outcome.as_deref() == Some("canceled"))
        };
        if settled {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "stream cancellation must settle durable hold"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
    let rows = db::billing::usage(&f.state.conn(), &Default::default()).unwrap();
    assert!(rows.iter().all(|r| r.status != "reserved"));
    assert_eq!(
        rows.iter()
            .find(|r| r.outcome.as_deref() == Some("canceled"))
            .unwrap()
            .observed_cost_micro,
        Some(14)
    );
}
#[tokio::test]
async fn auto_and_diagnostic_aliases_never_escape_model_permission() {
    let f = Fixture::new().await;
    {
        let c = f.state.conn();
        for name in ["allowed", "secret"] {
            db::kinds::set(&c, name, tokengo_shared::ModelKind::Chat).unwrap();
        }
    }
    f.policy(json!({"allowed_models":["auto","allowed"],"allow_paid":true}));
    let (s, v) = f.send("/v1/chat/completions", body("auto")).await;
    assert_eq!(s, 200, "{v}");
    f.state.drain_writes();
    let served: String = f
        .state
        .conn()
        .query_row(
            "SELECT served_model FROM requests ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(served, "allowed");
    f.policy(json!({"allowed_models":["auto"],"allow_paid":true}));
    assert_eq!(f.send("/v1/chat/completions", body("auto")).await.0, 403);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn successful_unpriced_call_does_not_grant_ability_to_spend_on_paid_fallback() {
    let f = Fixture::new().await;
    {
        let c = f.state.conn();
        db::billing::create_price(
            &c,
            &db::billing::Price {
                id: 0,
                channel_id: 1,
                model: "allowed".into(),
                input_micro_per_million: 0,
                output_micro_per_million: 0,
                max_input_tokens: 10_000,
                max_output_tokens: 1000,
            },
            "test",
        )
        .unwrap();
    }
    f.policy(json!({"allow_paid":false}));
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
    // A new immutable pricing version is observed on the next attempt despite the warm key cache.
    tariff(&f);
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 403);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn embedding_usage_and_tool_rounds_are_distinct_attributed_ledger_entries() {
    let f = Fixture::new().await;
    tariff(&f);
    let (s, v) = f
        .send(
            "/v1/embeddings",
            json!({"model":"allowed","input":["中文资料","another"]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    {
        let c = f.state.conn();
        let rows = db::billing::usage(&c, &Default::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].observed_cost_micro, Some(12));
        assert_eq!(rows[0].output_tokens, Some(0));
    }
    let g = Fixture::new().await;
    tariff(&g);
    assert_eq!(g.send("/v1/chat/completions", body("allowed")).await.0, 200);
    let mut tool = body("allowed");
    tool["messages"] = json!([{"role":"tool","tool_call_id":"call1","content":"result"}]);
    assert_eq!(g.send("/v1/chat/completions", tool).await.0, 200);
    let rows = db::billing::usage(&g.state.conn(), &Default::default()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].request_id, rows[1].request_id);
    assert_eq!(rows.iter().map(|r| r.debit_micro.unwrap()).sum::<i64>(), 28);
}
#[tokio::test]
async fn missing_provider_usage_never_becomes_zero_cost() {
    let f = Fixture::new().await;
    tariff(&f);
    {
        let c = f.state.conn();
        let url: String = c
            .query_row("SELECT base_url FROM channels WHERE id=1", [], |r| r.get(0))
            .unwrap();
        c.execute(
            "UPDATE channels SET base_url=?1 WHERE id=1",
            [url.replace("/v1", "/missing/v1")],
        )
        .unwrap();
    }
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
    let rows = db::billing::usage(&f.state.conn(), &Default::default()).unwrap();
    assert_eq!(rows.len(), 1);
    let e = &rows[0];
    assert_eq!(e.usage_certainty, "estimated");
    assert_eq!(e.observed_cost_micro, None);
    assert_eq!(e.debit_micro, Some(e.reserved_micro));
    assert!(e.reserved_micro > 0);
}
#[tokio::test]
async fn unaffordable_source_is_skipped_without_poisoning_health_when_free_fallback_exists() {
    let f = Fixture::new().await;
    tariff(&f);
    {
        let c = f.state.conn();
        let url: String = c
            .query_row("SELECT base_url FROM channels WHERE id=1", [], |r| r.get(0))
            .unwrap();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "free".into(),
                base_url: url,
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        db::channels::set_models(
            &c,
            id,
            &[db::channels::ModelMapping::new("allowed", None, false)],
        )
        .unwrap();
        db::billing::create_price(
            &c,
            &db::billing::Price {
                id: 0,
                channel_id: id.0,
                model: "allowed".into(),
                input_micro_per_million: 0,
                output_micro_per_million: 0,
                max_input_tokens: 10_000,
                max_output_tokens: 1000,
            },
            "test",
        )
        .unwrap();
        db::billing::set_budget(&c, "key", 1, Some(0), "test").unwrap();
    }
    let (s, v) = f.send("/v1/chat/completions", body("allowed")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    let c = f.state.conn();
    let rows = db::billing::usage(&c, &Default::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].channel_id, 2);
    assert_eq!(rows[0].debit_micro, Some(0));
    assert!(
        db::policy::rows(&c, "allowed")
            .unwrap()
            .iter()
            .all(|r| r.failures == 0),
        "quota refusal is not a provider health failure"
    );
}
#[tokio::test]
async fn multiple_choices_reserve_total_generation_and_cannot_bypass_budget() {
    let f = Fixture::new().await;
    tariff(&f);
    db::billing::set_budget(&f.state.conn(), "key", 1, Some(600), "test").unwrap();
    let mut b = body("allowed");
    b["n"] = json!(10);
    let (s, v) = f.send("/v1/chat/completions", b).await;
    assert_eq!(s, 429, "{v}");
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    for n in [json!(0), json!(-1), json!(129), json!("2"), Value::Null] {
        let mut b = body("allowed");
        b["n"] = n;
        assert_eq!(f.send("/v1/chat/completions", b).await.0, 400);
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn immutable_identity_policy_snapshot_follows_each_billed_attempt() {
    let f = Fixture::new().await;
    tariff(&f);
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
    f.state.drain_writes();
    let first: String = f
        .state
        .conn()
        .query_row(
            "SELECT auth_snapshot FROM billing_ledger ORDER BY id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let first: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["key_id"], 1);
    assert!(first["policy_version"]
        .as_str()
        .is_some_and(|v| !v.is_empty()));
    f.policy(json!({"allowed_models":["allowed"],"allow_paid":true}));
    assert_eq!(f.send("/v1/chat/completions", body("allowed")).await.0, 200);
    f.state.drain_writes();
    let c = f.state.conn();
    let snapshots: Vec<String> = c
        .prepare("SELECT auth_snapshot FROM billing_ledger ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(snapshots.len(), 2);
    let latest: Value = serde_json::from_str(&snapshots[1]).unwrap();
    assert_ne!(first["policy_version"], latest["policy_version"]);
    let a: String = c
        .query_row(
            "SELECT auth_snapshot FROM attempts ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let a: Value = serde_json::from_str(&a).unwrap();
    assert_eq!(a["policy_version"], latest["policy_version"]);
}
