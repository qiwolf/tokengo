//! Price permission must use the endpoint's actual billing unit.
use axum::{routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokengo_server::{app, db, AppState, Config};
async fn source(axum::extract::State(seen): axum::extract::State<Arc<AtomicUsize>>) -> Json<Value> {
    seen.fetch_add(1, Ordering::SeqCst);
    Json(
        json!({"id":"synthetic","object":"chat.completion","model":"source","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}),
    )
}
#[tokio::test]
async fn free_flat_price_cannot_override_paid_token_permission_and_paid_flat_cannot_hide_free_chat()
{
    for (token_price, flat_price, expected) in [(1000, 0, 403), (0, 1000, 200)] {
        let dir =
            std::env::temp_dir().join(format!("tokengo-unit-permission-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            ..Default::default()
        })
        .unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sourceurl = format!("http://{}/v1", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/v1/chat/completions", post(source))
            .with_state(seen.clone());
        let upstream = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let key = {
            let c = state.conn();
            let policy = db::identity::Policy {
                allow_paid: false,
                ..Default::default()
            };
            let user = db::identity::create_user(&c, "unit-permission", "user", &policy).unwrap();
            let (_, key) =
                db::identity::create_key(&c, user.id, "test", &policy, None, None).unwrap();
            let channel = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "mixed billing source".into(),
                    base_url: sourceurl,
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 0,
                },
            )
            .unwrap();
            db::channels::set_models(
                &c,
                channel,
                &[db::channels::ModelMapping::new(
                    "mixed-unit",
                    Some("source".into()),
                    false,
                )],
            )
            .unwrap();
            db::billing::create_price(
                &c,
                &db::billing::Price {
                    id: 0,
                    channel_id: channel.0,
                    model: "mixed-unit".into(),
                    input_micro_per_million: token_price,
                    output_micro_per_million: token_price,
                    max_input_tokens: 8192,
                    max_output_tokens: 64,
                },
                "unit-test",
            )
            .unwrap();
            db::billing::create_request_price(
                &c,
                &db::billing::RequestPrice {
                    id: 0,
                    channel_id: channel.0,
                    model: "mixed-unit".into(),
                    micro_per_call: flat_price,
                },
                "unit-test",
            )
            .unwrap();
            db::route_policy::save(
                &c,
                "mixed-unit",
                &db::route_policy::RoutePolicy {
                    probe_enabled: Some(false),
                    retry_times: Some(0),
                    ..Default::default()
                },
            )
            .unwrap();
            key
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let gateway = tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
        let r=reqwest::Client::new().post(url).bearer_auth(key).json(&json!({"model":"mixed-unit","messages":[{"role":"user","content":"hi"}],"max_tokens":8})).send().await.unwrap();
        let code = r.status().as_u16();
        let body = r.text().await.unwrap();
        assert_eq!(
            code, expected,
            "token={token_price},flat={flat_price}: {body}"
        );
        assert_eq!(seen.load(Ordering::SeqCst), usize::from(expected == 200));
        gateway.abort();
        upstream.abort();
        std::fs::remove_dir_all(dir).ok();
    }
}

async fn search_source(
    axum::extract::State(seen): axum::extract::State<Arc<AtomicUsize>>,
) -> Json<Value> {
    seen.fetch_add(1, Ordering::SeqCst);
    Json(
        json!({"results":[{"title":"Synthetic","url":"https://example.com","snippet":"s","summary":"s","content":"","rerank_score":2.5,"authority":0,"site_name":null,"site_icon":null,"date":null}]}),
    )
}

#[tokio::test]
async fn standalone_tools_respect_ancestor_rpm_and_budget_without_spending_token_quota() {
    let dir = std::env::temp_dir().join(format!("tokengo-tool-ancestor-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = AppState::init(Config {
        db_path: dir.join("db"),
        master_key_path: dir.join("master").to_string_lossy().into(),
        ..Default::default()
    })
    .unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/v1/tools/search", post(search_source))
        .with_state(seen.clone());
    let upstream = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (key, parent) = {
        let c = state.conn();
        let channel = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "tool ancestor fixture".into(),
                base_url: base,
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        db::channels::set_models(
            &c,
            channel,
            &[db::channels::ModelMapping::new(
                "deepseek-search",
                None,
                false,
            )],
        )
        .unwrap();
        tokengo_server::search_tools::save(
            &c,
            channel.0,
            &tokengo_server::search_tools::ToolCapabilities {
                search: true,
                account_scope: "tool-ancestor-test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        db::billing::create_request_price(
            &c,
            &db::billing::RequestPrice {
                id: 0,
                channel_id: channel.0,
                model: "deepseek-search".into(),
                micro_per_call: 1,
            },
            "test",
        )
        .unwrap();
        let policy = db::identity::Policy {
            tpm: Some(1),
            rpm: Some(12),
            allowed_models: Some(vec!["deepseek-search".into()]),
            allowed_channels: Some(vec![channel.0]),
            ..Default::default()
        };
        let user = db::identity::create_user(&c, "tool-ancestor-owner", "user", &policy).unwrap();
        let (parent, _) = db::identity::create_key(
            &c,
            user.id,
            "parent",
            &db::identity::Policy {
                tpm: Some(1),
                rpm: Some(1),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
        let child_policy = db::identity::context(&c, parent.id)
            .unwrap()
            .unwrap()
            .effective_policy();
        let (_, key) =
            db::identity::create_key(&c, user.id, "child", &child_policy, None, Some(parent.id))
                .unwrap();
        c.execute(
            "INSERT INTO billing_budgets(scope,scope_id,limit_micro) VALUES('key',?1,10)",
            [parent.id],
        )
        .unwrap();
        db::route_policy::save(
            &c,
            "deepseek-search",
            &db::route_policy::RoutePolicy {
                probe_enabled: Some(false),
                retry_times: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        (key, parent.id)
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/tools/search", listener.local_addr().unwrap());
    let gateway_state = state.clone();
    let gateway =
        tokio::spawn(async move { axum::serve(listener, app(gateway_state)).await.unwrap() });
    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .bearer_auth(&key)
        .json(&json!({"query":"one"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let _: Value = response.json().await.unwrap();
    let response = client
        .post(&url)
        .bearer_auth(&key)
        .json(&json!({"query":"two"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 429);
    let error: Value = response.json().await.unwrap();
    assert_eq!(error["error"]["type"], "rate_limit_exceeded");
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    for _ in 0..100 {
        state.drain_writes();
        tokengo_server::billing_writer::replay(&state.conn(), &state.cfg.db_path).unwrap();
        if state
            .conn()
            .query_row(
                "SELECT count(*) FROM billing_ledger WHERE status='reserved'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            == 0
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let c = state.conn();
    let quota: (i64, i64) = c
        .query_row("SELECT count(*),sum(tokens) FROM quota_events", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(quota, (3, 0));
    let parent_spent: i64 = c
        .query_row(
            "SELECT spent_micro FROM billing_budgets WHERE scope='key' AND scope_id=?1",
            [parent],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parent_spent, 1);
    let units: (String, i64, Option<i64>, Option<i64>) = c
        .query_row(
            "SELECT billing_unit,request_units,input_tokens,output_tokens FROM billing_ledger",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(units, ("request".into(), 1, None, None));
    drop(c);
    gateway.abort();
    upstream.abort();
    std::fs::remove_dir_all(dir).ok();
}
