//! Real upstream HTTP cases; included below proxy.rs to reuse its request/ledger fixture.
use super::*;
use tokengo_server::db::{self, identity, provider_pool, user_virtual_routes as virtuals};

fn user_keys(f: &Fixture, label: &str) -> (i64, Vec<(i64, String)>) {
    let c = f.state.conn();
    let user = identity::create_user(&c, label, "user", &identity::Policy::default()).unwrap();
    let keys = (0..2)
        .map(|n| {
            let (key, secret) = identity::create_key(
                &c,
                user.id,
                &format!("key-{n}"),
                &identity::Policy::default(),
                None,
                None,
            )
            .unwrap();
            (key.id, secret)
        })
        .collect();
    (user.id, keys)
}
fn pool_backfill(f: &Fixture, source: ChannelId, strategy: &str, on429: bool, on400: bool) -> i64 {
    let c = f.state.conn();
    provider_pool::backfill(&c, &f.state.secrets).unwrap();
    let credential = provider_pool::source_credential(&c, source.0)
        .unwrap()
        .unwrap();
    c.execute("UPDATE provider_channels SET key_strategy=?2,switch_on_429=?3,switch_on_400=?4 WHERE id=?1",rusqlite::params![credential.provider_id,strategy,on429,on400]).unwrap();
    credential.provider_id
}
fn two_keys(f: &Fixture, m: &Mock) -> (ChannelId, ChannelId) {
    let a = f.add_channel(
        "key-one",
        &m.base,
        ChannelSpec {
            secret: Some("upstream-one".into()),
            ..Default::default()
        },
    );
    let b = f.add_channel(
        "key-two",
        &m.base,
        ChannelSpec {
            secret: Some("upstream-two".into()),
            ..Default::default()
        },
    );
    f.serve("real-model", &[a, b]);
    f.state.routing().cfg.retry_times = 0;
    (a, b)
}
fn alias(f: &Fixture, user: i64, targets: Vec<virtuals::Target>) -> virtuals::VirtualModel {
    virtuals::create(
        &f.state.conn(),
        user,
        &virtuals::Config {
            name: "my-private-model".into(),
            enabled: true,
            mode: "ordered".into(),
            timezone: "Asia/Shanghai".into(),
            default_positions: vec![],
            targets,
            windows: vec![],
        },
    )
    .unwrap()
}
async fn call(f: &Fixture, key: &str, name: &str) -> StatusCode {
    let (status, _, _) = post(
        f,
        CHAT,
        Some(key),
        json!({"model":name,"messages":[{"role":"user","content":"Hi"}],"max_tokens":16}),
    )
    .await;
    status
}

#[tokio::test]
async fn same_user_two_vkeys_keep_one_upstream_identity_and_switch_once() {
    let f = fixture();
    let m = start_mock(Reply::Ok).await;
    let (a, b) = two_keys(&f, &m);
    let provider = pool_backfill(&f, a, "ordered", true, false);
    let (user, keys) = user_keys(&f, "pool-owner");
    assert_eq!(call(&f, &keys[0].1, "real-model").await, StatusCode::OK);
    assert_eq!(call(&f, &keys[1].1, "real-model").await, StatusCode::OK);
    assert_eq!(
        m.seen()
            .iter()
            .map(|s| s.auth.as_deref().unwrap())
            .collect::<Vec<_>>(),
        vec!["Bearer upstream-one"; 2]
    );
    m.inner
        .replies
        .lock()
        .unwrap()
        .push_back(Reply::Status(429, "rate limited"));
    assert_eq!(call(&f, &keys[0].1, "real-model").await, StatusCode::OK);
    assert_eq!(call(&f, &keys[1].1, "real-model").await, StatusCode::OK);
    let seen = m.seen();
    assert_eq!(seen[2].auth.as_deref(), Some("Bearer upstream-one"));
    assert_eq!(seen[3].auth.as_deref(), Some("Bearer upstream-two"));
    assert_eq!(seen[4].auth, seen[3].auth);
    {
        let c = f.state.conn();
        assert_eq!(
            provider_pool::affinity_token(&c, user, provider)
                .unwrap()
                .unwrap()
                .credential_id,
            provider_pool::source_credential(&c, b.0)
                .unwrap()
                .unwrap()
                .id
        );
    }
    assert_eq!(
        f.state.credential_load(
            provider_pool::source_credential(&f.state.conn(), a.0)
                .unwrap()
                .unwrap()
                .id
        ),
        0
    );
    assert_eq!(
        f.state.credential_load(
            provider_pool::source_credential(&f.state.conn(), b.0)
                .unwrap()
                .unwrap()
                .id
        ),
        0
    );
}

#[tokio::test]
async fn concurrent_strategy_distributes_users_without_vkey_randomization() {
    let f = fixture();
    let m = start_mock(Reply::Ok).await;
    let (a, _) = two_keys(&f, &m);
    pool_backfill(&f, a, "concurrent", true, false);
    let (_, alice) = user_keys(&f, "alice");
    let (_, bob) = user_keys(&f, "bob");
    for key in [&alice[0].1, &bob[0].1, &alice[1].1, &bob[1].1] {
        assert_eq!(call(&f, key, "real-model").await, StatusCode::OK);
    }
    let seen = m.seen();
    assert_ne!(seen[0].auth, seen[1].auth);
    assert_eq!(seen[0].auth, seen[2].auth);
    assert_eq!(seen[1].auth, seen[3].auth);
}

#[tokio::test]
async fn explicit_400_is_request_local_and_disabled_switches_stop() {
    for (status, on429, on400, expected_calls) in [
        (400, false, false, 1),
        (400, false, true, 2),
        (429, false, false, 1),
        (429, true, false, 2),
    ] {
        let f = fixture();
        let m = start_mock(Reply::Ok).await;
        let (a, _) = two_keys(&f, &m);
        pool_backfill(&f, a, "ordered", on429, on400);
        let (_, keys) = user_keys(&f, &format!("owner-{status}-{on400}-{on429}"));
        m.inner
            .replies
            .lock()
            .unwrap()
            .push_back(Reply::Status(status, "ordinary request rejection"));
        let result = call(&f, &keys[0].1, "real-model").await;
        assert_eq!(
            m.count(),
            expected_calls,
            "HTTP {status}, 429={on429}, 400={on400}"
        );
        assert_eq!(result.is_success(), expected_calls == 2);
        if status == 400 {
            assert!(db::faults::rows(&f.state.conn()).unwrap().is_empty());
        }
    }
}

async fn target_chain(
    f: &Fixture,
    count: usize,
    status: u16,
) -> (Vec<Mock>, Vec<virtuals::Target>, Vec<ChannelId>) {
    let mut mocks = vec![];
    let mut targets = vec![];
    let mut sources = vec![];
    for index in 0..count {
        let m=start_mock(if index+1==count {Reply::Ok}else{Reply::Status(status,r#"{"error":{"message":"controlled target failure"},"usage":{"prompt_tokens":3,"completion_tokens":1}}"#)}).await;
        let name = format!("target-{index}");
        let source = f.add_channel(&name, &m.base, ChannelSpec::default());
        f.serve(&name, &[source]);
        db::kinds::set(&f.state.conn(), &name, tokengo_shared::ModelKind::Chat).unwrap();
        db::billing::create_price(
            &f.state.conn(),
            &db::billing::Price {
                id: 0,
                channel_id: source.0,
                model: name.clone(),
                input_micro_per_million: 1_000_000,
                output_micro_per_million: 1_000_000,
                max_input_tokens: 10000,
                max_output_tokens: 100,
            },
            "fixture",
        )
        .unwrap();
        let provider = pool_backfill(f, source, "ordered", false, false);
        targets.push(virtuals::Target {
            provider_id: provider,
            logical_model: name,
            switch_on_429: true,
            switch_on_400: true,
        });
        mocks.push(m);
        sources.push(source);
    }
    f.state.routing().cfg.retry_times = 0;
    (mocks, targets, sources)
}

#[tokio::test]
async fn virtual_chains_one_two_three_four_preserve_alias_and_real_attempts() {
    for length in 1..=4 {
        let f = fixture();
        let (mocks, targets, sources) = target_chain(&f, length, 400).await;
        let (user, keys) = user_keys(&f, &format!("chain-{length}"));
        let route = alias(&f, user, targets);
        assert_eq!(
            call(&f, &keys[0].1, &route.config.name).await,
            StatusCode::OK
        );
        f.state.drain_writes();
        assert_eq!(
            mocks.iter().map(Mock::count).collect::<Vec<_>>(),
            vec![1; length]
        );
        assert_eq!(
            f.attempts()
                .iter()
                .map(|attempt| attempt.0)
                .collect::<Vec<_>>(),
            sources.iter().map(|source| source.0).collect::<Vec<_>>()
        );
        let c = f.state.conn();
        let requested: String = c
            .query_row("SELECT model FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(requested, "my-private-model");
        let mut q = c
            .prepare("SELECT model,channel_id,source_snapshot FROM billing_ledger ORDER BY id")
            .unwrap();
        let ledger = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(ledger.len(), length);
        for (index, (model, source, snapshot)) in ledger.iter().enumerate() {
            assert_eq!(
                model,
                &tokengo_shared::normalize_model_name(&format!("target-{index}"))
            );
            assert_eq!(*source, sources[index].0);
            let snapshot: Value = serde_json::from_str(snapshot).unwrap();
            assert_eq!(snapshot["virtual_model_id"], route.id);
            assert_eq!(snapshot["target_position"], index + 1);
        }
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM billing_ledger WHERE settled_ms IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            c.query_row("SELECT SUM(debit_micro) FROM billing_ledger", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            4 * (length as i64 - 1) + 18
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM billing_ledger WHERE price_snapshot IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn virtual_chain_does_not_inherit_network_or_5xx_fallback() {
    let f = fixture();
    let (mocks, targets, _) = target_chain(&f, 2, 500).await;
    let (user, keys) = user_keys(&f, "no-implicit");
    alias(&f, user, targets);
    assert_eq!(
        call(&f, &keys[0].1, "my-private-model").await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(mocks[0].count(), 1);
    assert_eq!(mocks[1].count(), 0);
}

#[tokio::test]
async fn private_alias_visibility_and_target_acl_follow_each_vkey() {
    let f = fixture();
    let (_, targets, _) = target_chain(&f, 1, 400).await;
    let (alice, a) = user_keys(&f, "private-alice");
    let (_, b) = user_keys(&f, "private-bob");
    alias(&f, alice, targets);
    let list = |key: String| {
        let state = f.state.clone();
        async move {
            let response = app(state)
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/v1/models")
                        .header("authorization", format!("Bearer {key}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            serde_json::from_slice::<Value>(
                &axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .unwrap()
        }
    };
    let alice_list = list(a[0].1.clone()).await;
    let bob_list = list(b[0].1.clone()).await;
    assert!(alice_list["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["id"] == "my-private-model"));
    assert!(!bob_list["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["id"] == "my-private-model"));
    let c = f.state.conn();
    let mut key = identity::key(&c, a[1].0).unwrap().unwrap();
    key.policy.allowed_models = Some(vec!["my-private-model".into()]);
    identity::update_key(&c, &key).unwrap();
    drop(c);
    assert_eq!(
        call(&f, &a[1].1, "my-private-model").await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn scheduled_alias_uses_current_window_and_rejects_no_default() {
    let f = fixture();
    let (mocks, targets, _) = target_chain(&f, 2, 400).await;
    let (user, keys) = user_keys(&f, "scheduled-owner");
    let config = virtuals::Config {
        name: "scheduled-model".into(),
        enabled: true,
        mode: "scheduled".into(),
        timezone: "UTC".into(),
        targets,
        windows: vec![virtuals::Window {
            days_mask: 127,
            start_minute: 0,
            end_minute: 1440,
            target_positions: vec![2],
        }],
        default_positions: vec![],
    };
    virtuals::create(&f.state.conn(), user, &config).unwrap();
    assert_eq!(
        call(&f, &keys[0].1, "scheduled-model").await,
        StatusCode::OK
    );
    assert_eq!(mocks[0].count(), 0);
    assert_eq!(mocks[1].count(), 1);
    let minute = (db::now_ms() / 60_000).rem_euclid(1440) as u16;
    let start = if minute < 720 { 800 } else { 100 };
    let mut unavailable = config;
    unavailable.name = "unavailable-model".into();
    unavailable.windows = vec![virtuals::Window {
        days_mask: 127,
        start_minute: start,
        end_minute: start + 1,
        target_positions: vec![1],
    }];
    virtuals::create(&f.state.conn(), user, &unavailable).unwrap();
    assert_eq!(
        call(&f, &keys[0].1, "unavailable-model").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(mocks[0].count(), 0);
    assert_eq!(mocks[1].count(), 1);
}

#[tokio::test]
async fn live_user_revocation_between_targets_prevents_second_attempt() {
    let f = fixture();
    let (mocks, targets, _) = target_chain(&f, 2, 400).await;
    let (user, keys) = user_keys(&f, "revoke-owner");
    alias(&f, user, targets);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    mocks[0]
        .inner
        .replies
        .lock()
        .unwrap()
        .push_back(Reply::HeldStatus(entered.clone(), release.clone(), 400));
    let state = f.state.clone();
    let key = keys[0].1.clone();
    let request = tokio::spawn(async move {
        app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(CHAT)
                    .header("authorization", format!("Bearer {key}"))
                    .body(axum::body::Body::from(
                        json!({"model":"my-private-model","messages":[],"max_tokens":16})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    {
        let c = f.state.conn();
        let mut owner = identity::user(&c, user).unwrap().unwrap();
        owner.policy.allowed_models = Some(vec!["target-0".into()]);
        identity::update_user(&c, &owner).unwrap();
    }
    release.notify_one();
    assert_eq!(request.await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(mocks[1].count(), 0);
}

#[tokio::test]
async fn changed_alias_version_between_targets_stops_stale_plan() {
    let f = fixture();
    let (mocks, targets, _) = target_chain(&f, 2, 400).await;
    let (user, keys) = user_keys(&f, "version-owner");
    let route = alias(&f, user, targets);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    mocks[0]
        .inner
        .replies
        .lock()
        .unwrap()
        .push_back(Reply::HeldStatus(entered.clone(), release.clone(), 400));
    let state = f.state.clone();
    let key = keys[0].1.clone();
    let request = tokio::spawn(async move {
        app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(CHAT)
                    .header("authorization", format!("Bearer {key}"))
                    .body(axum::body::Body::from(
                        json!({"model":"my-private-model","messages":[],"max_tokens":16})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    f.state
        .conn()
        .execute(
            "UPDATE user_virtual_models SET version=version+1 WHERE id=?1",
            [route.id],
        )
        .unwrap();
    release.notify_one();
    assert_eq!(request.await.unwrap().status(), StatusCode::CONFLICT);
    assert_eq!(mocks[1].count(), 0);
}

#[tokio::test]
async fn committed_sse_never_replays_on_another_credential_and_releases_gates() {
    let f = fixture();
    let m = start_mock(Reply::BrokenSse).await;
    let (a, b) = two_keys(&f, &m);
    pool_backfill(&f, a, "ordered", true, true);
    f.state
        .conn()
        .execute("UPDATE provider_credentials SET max_concurrent=1", [])
        .unwrap();
    let (_, keys) = user_keys(&f, "stream-owner");
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(CHAT)
        .header("authorization", format!("Bearer {}", keys[0].1))
        .body(axum::body::Body::from(
            json!({"model":"real-model","messages":[],"stream":true}).to_string(),
        ))
        .unwrap();
    let response = app(f.state.clone()).oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let active_credential = provider_pool::source_credential(&f.state.conn(), a.0)
        .unwrap()
        .unwrap();
    assert_eq!(
        f.state.credential_load(active_credential.id),
        1,
        "credential permit must outlive response headers"
    );
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;
    assert_eq!(m.count(), 1);
    for source in [a, b] {
        let credential = provider_pool::source_credential(&f.state.conn(), source.0)
            .unwrap()
            .unwrap();
        assert_eq!(f.state.credential_load(credential.id), 0);
    }
}

#[tokio::test]
async fn native_pin_overrides_affinity_and_prevents_key_switch_replay() {
    use tokengo_server::{
        gateway::{self, EndpointKind, ForwardRequest},
        protocol::Protocol,
        protocol_config,
        protocol_sessions::{self, Pin},
    };
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = count.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let upstream=Router::new().route("/v1/responses",route_post(move |headers:HeaderMap|{let counted=counted.clone();async move {
        counted.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        assert_eq!(headers.get("authorization").unwrap(),"Bearer upstream-one");
        (StatusCode::TOO_MANY_REQUESTS,axum::Json(json!({"error":{"message":"rate limited"},"usage":{"input_tokens":0,"output_tokens":0}})))
    }}));
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let f = fixture();
    let a = f.add_channel(
        "native-one",
        &base,
        ChannelSpec {
            secret: Some("upstream-one".into()),
            ..Default::default()
        },
    );
    let b = f.add_channel(
        "native-two",
        &base,
        ChannelSpec {
            secret: Some("upstream-two".into()),
            ..Default::default()
        },
    );
    f.serve("native-model", &[a, b]);
    f.state.routing().cfg.retry_times = 0;
    let provider = pool_backfill(&f, a, "ordered", true, true);
    let (user, keys) = user_keys(&f, "native-owner");
    let auth = {
        let c = f.state.conn();
        for source in [a, b] {
            protocol_config::save(
                &c,
                source.0,
                &protocol_config::Capabilities {
                    native: vec![Protocol::Responses],
                    max_input_tokens: Some(10000),
                    max_output_tokens: Some(100),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let credential = provider_pool::source_credential(&c, b.0).unwrap().unwrap();
        provider_pool::ensure_affinity_for_selection(&c, user, provider, credential.id).unwrap();
        let generation = c
            .query_row(
                "SELECT binding_generation FROM channels WHERE id=?1",
                [a.0],
                |r| r.get(0),
            )
            .unwrap();
        let owner = protocol_sessions::key(
            "test-native-owner",
            "native-session",
            "native-model",
            Protocol::Responses,
        );
        let pin = Pin {
            channel: a.0,
            generation,
            upstream_model: "native-model".into(),
            protocol: Protocol::Responses,
        };
        assert!(protocol_sessions::claim(&c, &owner, &pin).unwrap());
        let mut artifacts = protocol_sessions::ArtifactCollector::default();
        artifacts.value(&json!({"output":[{"type":"reasoning","encrypted_content":"owned-native-reasoning","summary":[]}]}));
        artifacts.persist(&c, &owner, &pin).unwrap();
        identity::context(&c, keys[0].0).unwrap().unwrap()
    };
    let response=gateway::forward(f.state.clone(),ForwardRequest{auth:Some(auth),session_namespace:"test-native-owner".into(),explicit_session:Some("native-session".into()),endpoint:EndpointKind::Responses,model:"native-model".into(),body:json!({"model":"native-model","input":[{"type":"reasoning","encrypted_content":"owned-native-reasoning","summary":[]},{"role":"user","content":"hi"}],"max_output_tokens":16,"store":false}),stream:false,client_ip:None,user_agent:None}).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    let c = f.state.conn();
    assert_eq!(
        provider_pool::affinity_token(&c, user, provider)
            .unwrap()
            .unwrap()
            .credential_id,
        provider_pool::source_credential(&c, b.0)
            .unwrap()
            .unwrap()
            .id
    );
    server.abort();
}

#[tokio::test]
async fn concurrent_old_429_responses_cannot_push_affinity_twice() {
    let f = fixture();
    let m = start_mock(Reply::Ok).await;
    let (a, b) = two_keys(&f, &m);
    let provider = pool_backfill(&f, a, "ordered", true, false);
    let (user, keys) = user_keys(&f, "cas-owner");
    let entered_a = Arc::new(tokio::sync::Notify::new());
    let release_a = Arc::new(tokio::sync::Notify::new());
    let entered_b = Arc::new(tokio::sync::Notify::new());
    let release_b = Arc::new(tokio::sync::Notify::new());
    m.inner.replies.lock().unwrap().push_back(Reply::HeldStatus(
        entered_a.clone(),
        release_a.clone(),
        429,
    ));
    m.inner.replies.lock().unwrap().push_back(Reply::HeldStatus(
        entered_b.clone(),
        release_b.clone(),
        429,
    ));
    let make_request = |key: String| {
        let state = f.state.clone();
        tokio::spawn(async move {
            app(state)
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri(CHAT)
                        .header("authorization", format!("Bearer {key}"))
                        .body(axum::body::Body::from(
                            json!({"model":"real-model","messages":[],"max_tokens":16}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap()
        })
    };
    let first = make_request(keys[0].1.clone());
    tokio::time::timeout(std::time::Duration::from_secs(30), entered_a.notified())
        .await
        .unwrap();
    let second = make_request(keys[1].1.clone());
    tokio::time::timeout(std::time::Duration::from_secs(30), entered_b.notified())
        .await
        .unwrap();
    release_a.notify_one();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
    release_b.notify_one();
    assert_eq!(second.await.unwrap().status(), StatusCode::CONFLICT);
    assert_eq!(m.count(), 3);
    let c = f.state.conn();
    let token = provider_pool::affinity_token(&c, user, provider)
        .unwrap()
        .unwrap();
    assert_eq!(token.generation, 2);
    assert_eq!(
        token.credential_id,
        provider_pool::source_credential(&c, b.0)
            .unwrap()
            .unwrap()
            .id
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM billing_ledger WHERE settled_ms IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn explicit_429_respects_retry_after_for_the_rejected_model_key() {
    let f = fixture();
    let m = start_mock(Reply::Ok).await;
    let (a, _) = two_keys(&f, &m);
    pool_backfill(&f, a, "ordered", true, false);
    let (_, keys) = user_keys(&f, "retry-after-owner");
    m.inner
        .replies
        .lock()
        .unwrap()
        .push_back(Reply::RateLimited(60));
    assert_eq!(call(&f, &keys[0].1, "real-model").await, StatusCode::OK);
    let rows = db::policy::rows(&f.state.conn(), "real-model").unwrap();
    let row = rows.iter().find(|row| row.channel_id == a.0).unwrap();
    assert!(row.until_ms > db::now_ms() + 55_000);
    assert_eq!(m.count(), 2);
    assert!(
        db::faults::rows(&f.state.conn()).unwrap().is_empty(),
        "model 429 must not suspend the whole account"
    );
}

#[tokio::test]
async fn selected_credential_disable_between_attempts_rejects_stale_snapshot() {
    let f = fixture();
    let m = start_mock(Reply::Ok).await;
    let (a, _) = two_keys(&f, &m);
    pool_backfill(&f, a, "ordered", true, false);
    let (_, keys) = user_keys(&f, "disabled-during-request");
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    m.inner.replies.lock().unwrap().push_back(Reply::HeldStatus(
        entered.clone(),
        release.clone(),
        429,
    ));
    let state = f.state.clone();
    let key = keys[0].1.clone();
    let request = tokio::spawn(async move {
        app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(CHAT)
                    .header("authorization", format!("Bearer {key}"))
                    .body(axum::body::Body::from(
                        json!({"model":"real-model","messages":[],"max_tokens":16}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    {
        let c = f.state.conn();
        let credential = provider_pool::source_credential(&c, a.0).unwrap().unwrap();
        c.execute(
            "UPDATE provider_credentials SET enabled=0,revision=revision+1 WHERE id=?1",
            [credential.id],
        )
        .unwrap();
    }
    release.notify_one();
    assert_eq!(request.await.unwrap().status(), StatusCode::CONFLICT);
    assert_eq!(m.count(), 1);
}

#[tokio::test]
async fn live_vkey_revocation_between_targets_prevents_second_attempt() {
    let f = fixture();
    let (mocks, targets, _) = target_chain(&f, 2, 400).await;
    let (user, keys) = user_keys(&f, "revoke-key-owner");
    alias(&f, user, targets);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    mocks[0]
        .inner
        .replies
        .lock()
        .unwrap()
        .push_back(Reply::HeldStatus(entered.clone(), release.clone(), 400));
    let state = f.state.clone();
    let key = keys[0].1.clone();
    let request = tokio::spawn(async move {
        app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(CHAT)
                    .header("authorization", format!("Bearer {key}"))
                    .body(axum::body::Body::from(
                        json!({"model":"my-private-model","messages":[],"max_tokens":16})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    identity::revoke(&f.state.conn(), keys[0].0).unwrap();
    release.notify_one();
    assert_eq!(request.await.unwrap().status(), StatusCode::UNAUTHORIZED);
    assert_eq!(mocks[1].count(), 0);
}

#[tokio::test]
async fn standard_models_list_and_calls_exclude_disabled_pool_keys() {
    let f = fixture();
    let m = start_mock(Reply::Ok).await;
    let (a, _) = two_keys(&f, &m);
    pool_backfill(&f, a, "ordered", true, false);
    let (_, keys) = user_keys(&f, "model-list-owner");
    f.state
        .conn()
        .execute(
            "UPDATE provider_credentials SET enabled=0,revision=revision+1",
            [],
        )
        .unwrap();
    let (status, list) = get(&f, "/v1/models", Some(&keys[0].1)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!list["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["id"] == "real-model"));
    assert!(!call(&f, &keys[0].1, "real-model").await.is_success());
    assert_eq!(m.count(), 0);
}

#[tokio::test]
async fn embedding_endpoint_rejects_unknown_kind_multi_target_alias() {
    let f = fixture();
    let a_mock = start_mock(Reply::Ok).await;
    let b_mock = start_mock(Reply::Ok).await;
    let a = f.add_channel("embedding-a", &a_mock.base, ChannelSpec::default());
    let b = f.add_channel("embedding-b", &b_mock.base, ChannelSpec::default());
    f.serve("unknown-vector", &[a, b]);
    let a_provider = pool_backfill(&f, a, "ordered", true, true);
    let b_provider = pool_backfill(&f, b, "ordered", true, true);
    let (user, keys) = user_keys(&f, "vector-owner");
    alias(
        &f,
        user,
        vec![
            virtuals::Target {
                provider_id: a_provider,
                logical_model: "unknown-vector".into(),
                switch_on_429: true,
                switch_on_400: true,
            },
            virtuals::Target {
                provider_id: b_provider,
                logical_model: "unknown-vector".into(),
                switch_on_429: true,
                switch_on_400: true,
            },
        ],
    );
    let (status, _, _) = post(
        &f,
        "/v1/embeddings",
        Some(&keys[0].1),
        json!({"model":"my-private-model","input":"x"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(a_mock.count() + b_mock.count(), 0);
}

#[tokio::test]
async fn virtual_chain_does_not_inherit_connection_failure_fallback() {
    let f = fixture();
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable = format!("http://{}/v1", closed.local_addr().unwrap());
    drop(closed);
    let good = start_mock(Reply::Ok).await;
    let a = f.add_channel("offline-target", &unavailable, ChannelSpec::default());
    let b = f.add_channel("online-target", &good.base, ChannelSpec::default());
    f.serve("real-model", &[a, b]);
    f.state.routing().cfg.retry_times = 0;
    let a_provider = pool_backfill(&f, a, "ordered", true, true);
    let b_provider = pool_backfill(&f, b, "ordered", true, true);
    let (user, keys) = user_keys(&f, "offline-owner");
    alias(
        &f,
        user,
        vec![
            virtuals::Target {
                provider_id: a_provider,
                logical_model: "real-model".into(),
                switch_on_429: true,
                switch_on_400: true,
            },
            virtuals::Target {
                provider_id: b_provider,
                logical_model: "real-model".into(),
                switch_on_429: true,
                switch_on_400: true,
            },
        ],
    );
    assert_eq!(
        call(&f, &keys[0].1, "my-private-model").await,
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(good.count(), 0);
    assert_eq!(f.attempts().len(), 1);
}

#[tokio::test]
async fn pooled_standard_models_keep_global_provider_failover_preference() {
    let f = fixture();
    let first = start_mock(Reply::Status(500, "provider down")).await;
    let second = start_mock(Reply::Ok).await;
    let a = f.add_channel("provider-a", &first.base, ChannelSpec::default());
    let b = f.add_channel("provider-b", &second.base, ChannelSpec::default());
    f.serve("real-model", &[a, b]);
    f.state.routing().cfg.retry_times = 0;
    pool_backfill(&f, a, "ordered", false, false);
    let (_, keys) = user_keys(&f, "provider-failover-owner");
    assert_eq!(call(&f, &keys[0].1, "real-model").await, StatusCode::OK);
    assert_eq!(first.count(), 1);
    assert_eq!(second.count(), 1);
    assert_eq!(call(&f, &keys[1].1, "real-model").await, StatusCode::OK);
    assert_eq!(first.count(), 1);
    assert_eq!(second.count(), 2);
}

#[tokio::test]
async fn affinity_switched_while_queued_prevents_sending_the_old_key() {
    let f = fixture();
    let m = start_mock(Reply::Ok).await;
    let (a, b) = two_keys(&f, &m);
    let provider = pool_backfill(&f, a, "ordered", true, false);
    let (user, keys) = user_keys(&f, "queued-owner");
    let (old, next) = {
        let c = f.state.conn();
        c.execute("UPDATE channels SET max_concurrent=1 WHERE id=?1", [a.0])
            .unwrap();
        let first = provider_pool::source_credential(&c, a.0).unwrap().unwrap();
        let second = provider_pool::source_credential(&c, b.0).unwrap().unwrap();
        (
            provider_pool::ensure_affinity_for_selection(&c, user, provider, first.id).unwrap(),
            second.id,
        )
    };
    let held = f
        .state
        .channel_gate(a.0, 1)
        .unwrap()
        .try_acquire_owned()
        .unwrap();
    let state = f.state.clone();
    let key = keys[0].1.clone();
    let request = tokio::spawn(async move {
        app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(CHAT)
                    .header("authorization", format!("Bearer {key}"))
                    .body(axum::body::Body::from(
                        json!({"model":"real-model","messages":[],"max_tokens":16}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let count = f
                .state
                .conn()
                .query_row("SELECT COUNT(*) FROM requests", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap();
            if count == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(provider_pool::switch_affinity(&f.state.conn(), user, provider, &old, next).unwrap());
    drop(held);
    assert_eq!(request.await.unwrap().status(), StatusCode::CONFLICT);
    assert_eq!(m.count(), 0);
    assert_eq!(
        f.state
            .conn()
            .query_row("SELECT COUNT(*) FROM billing_ledger", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
