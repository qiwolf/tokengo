use axum::http::StatusCode;
use serde_json::{json, Value};
use std::sync::Arc;
use tokengo_server::{
    app,
    db::{self, gateway_keys, sessions, settings},
    state::AppState,
    Config,
};

fn config() -> Config {
    let dir = std::env::temp_dir().join(format!("tokengo-multikey-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    Config {
        db_path: dir.join("test.db"),
        master_key_path: dir.join("master.key").to_string_lossy().into(),
        ..Config::default()
    }
}
async fn request(
    state: Arc<AppState>,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut req = client
        .request(method.parse().unwrap(), format!("http://{address}{path}"))
        .json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let res = req.send().await.unwrap();
    let status = res.status();
    let value = res.json().await.unwrap_or(Value::Null);
    server.abort();
    (status, value)
}

#[tokio::test]
async fn named_keys_routes_revocation_restart_and_settings_compatibility() {
    let cfg = config();
    let state = AppState::init(cfg.clone()).unwrap();
    let token = sessions::create_session(&state.conn(), None).unwrap();
    for (method, path) in [
        ("GET", "/api/gateway-keys"),
        ("POST", "/api/gateway-keys"),
        ("GET", "/api/gateway-keys/1/reveal"),
        ("DELETE", "/api/gateway-keys/1"),
    ] {
        assert_eq!(
            request(state.clone(), method, path, None, json!({"name":"a"}))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                state.clone(),
                method,
                path,
                Some("not-a-session"),
                json!({"name":"a"})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    for name in ["", "   ", &"a".repeat(81), "bad\nname"] {
        assert_eq!(
            request(
                state.clone(),
                "POST",
                "/api/gateway-keys",
                Some(&token),
                json!({"name":name})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut keys = Vec::new();
    for name in ["first", "second"] {
        let (status, row) = request(
            state.clone(),
            "POST",
            "/api/gateway-keys",
            Some(&token),
            json!({"name":name}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(row["name"], name);
        assert!(row.get("gateway_key").is_none());
        assert!(row["hint"].is_string());
        let id = row["id"].as_i64().unwrap();
        let (status, reveal) = request(
            state.clone(),
            "GET",
            &format!("/api/gateway-keys/{id}/reveal"),
            Some(&token),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        keys.push((id, reveal["gateway_key"].as_str().unwrap().to_owned()));
    }
    assert_ne!(keys[0].1, keys[1].1);
    let (_, list) = request(
        state.clone(),
        "GET",
        "/api/gateway-keys",
        Some(&token),
        json!({}),
    )
    .await;
    assert_eq!(list["keys"].as_array().unwrap().len(), 2);
    for (_, key) in &keys {
        assert!(!list.to_string().contains(key));
        assert!(!list.to_string().contains("key_hash"));
        assert!(state.check_gateway_key(key));
    }
    // Compatibility generate/set rotates its own row only.
    request(
        state.clone(),
        "POST",
        "/api/settings/generate-gateway-key",
        Some(&token),
        json!({}),
    )
    .await;
    for (_, key) in &keys {
        assert!(state.check_gateway_key(key));
    }
    assert_eq!(
        request(
            state.clone(),
            "PUT",
            "/api/settings",
            Some(&token),
            json!({"gateway_key":"compatibility-custom-key"})
        )
        .await
        .0,
        StatusCode::OK
    );
    for (_, key) in &keys {
        assert!(state.check_gateway_key(key));
    }
    let (_, r) = request(
        state.clone(),
        "GET",
        "/api/settings/gateway-key",
        Some(&token),
        json!({}),
    )
    .await;
    let compat = r["gateway_key"].as_str().unwrap().to_owned();
    let default = gateway_keys::default_id(&state.conn()).unwrap().unwrap();
    assert!(state.check_gateway_key(&compat));
    assert_eq!(
        request(
            state.clone(),
            "DELETE",
            &format!("/api/gateway-keys/{default}"),
            Some(&token),
            json!({})
        )
        .await
        .1,
        json!({"deleted":true})
    );
    assert_eq!(
        request(
            state.clone(),
            "GET",
            "/api/settings/gateway-key",
            Some(&token),
            json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Stale old settings must never reactivate a deleted credential.
    settings::set(
        &state.conn(),
        settings::GATEWAY_KEY_HASH,
        &sessions::hash_password(&compat).unwrap(),
    )
    .unwrap();
    assert!(!state.check_gateway_key(&compat));
    let (id, key) = &keys[0];
    assert_eq!(
        request(state.clone(), "GET", "/v1/models", Some(key), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    request(
        state.clone(),
        "DELETE",
        &format!("/api/gateway-keys/{id}"),
        Some(&token),
        json!({}),
    )
    .await;
    for (method, path, body) in [
        ("GET", "/v1/models", json!({})),
        (
            "POST",
            "/v1/chat/completions",
            json!({"model":"x","messages":[]}),
        ),
        ("POST", "/v1/embeddings", json!({"model":"x","input":"x"})),
        (
            "POST",
            "/v1/rerank",
            json!({"model":"x","query":"x","documents":["x"]}),
        ),
    ] {
        assert_eq!(
            request(state.clone(), method, path, Some(key), body)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        request(
            state.clone(),
            "GET",
            "/v1/models",
            Some(&keys[1].1),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    drop(state);
    let restarted = AppState::init(cfg.clone()).unwrap();
    assert!(!restarted.check_gateway_key(key));
    assert!(!restarted.check_gateway_key(&compat));
    assert!(restarted.check_gateway_key(&keys[1].1));
    drop(restarted);
    std::fs::remove_dir_all(cfg.db_path.parent().unwrap()).unwrap();
}

#[test]
fn legacy_migration_preserves_hash_and_ciphertext_once_and_never_resurrects() {
    for hash_only in [false, true] {
        let cfg = config();
        let conn = rusqlite::Connection::open(&cfg.db_path).unwrap();
        conn.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY)")
            .unwrap();
        for (version, sql) in db::schema::MIGRATIONS.iter().filter(|(v, _)| *v < 21) {
            conn.execute_batch(sql).unwrap();
            conn.execute("INSERT INTO schema_migrations VALUES(?1)", [version])
                .unwrap();
        }
        let sb = tokengo_server::crypto::SecretBox::load_or_init(std::path::Path::new(
            &cfg.master_key_path,
        ))
        .unwrap();
        let key = "EXAMPLE-TEST-ONLY-KEY-A001";
        settings::set(
            &conn,
            settings::GATEWAY_KEY_HASH,
            &sessions::hash_password(key).unwrap(),
        )
        .unwrap();
        if !hash_only {
            let (n, c) = sb.seal(key).unwrap();
            settings::set(
                &conn,
                settings::GATEWAY_KEY_SEALED,
                &format!("{}:{}", hex::encode(n), hex::encode(c)),
            )
            .unwrap();
        }
        drop(conn);
        let state = AppState::init(cfg.clone()).unwrap();
        let rows = gateway_keys::list(&state.conn()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Legacy");
        assert!(state.check_gateway_key(key));
        assert_eq!(
            gateway_keys::reveal(&state.conn(), &state.secrets, rows[0].id)
                .unwrap()
                .as_deref(),
            if hash_only { None } else { Some(key) }
        );
        db::migrate(&state.conn()).unwrap();
        assert_eq!(gateway_keys::list(&state.conn()).unwrap().len(), 1);
        {
            let conn = state.conn();
            gateway_keys::delete(&conn, rows[0].id).unwrap();
            state.forget_gateway_key();
        }
        drop(state);
        let restarted = AppState::init(cfg.clone()).unwrap();
        assert!(!restarted.check_gateway_key(key));
        assert!(gateway_keys::list(&restarted.conn()).unwrap().is_empty());
        drop(restarted);
        std::fs::remove_dir_all(cfg.db_path.parent().unwrap()).unwrap();
    }
}

#[test]
fn concurrent_validation_and_deletion_cannot_refill_a_revoked_cache() {
    let cfg = config();
    let state = AppState::init(cfg.clone()).unwrap();
    let key = gateway_keys::generate();
    let row = gateway_keys::create(&state.conn(), &state.secrets, "race", &key, false).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let state = state.clone();
        let key = key.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..20 {
                state.check_gateway_key(&key);
            }
        }));
    }
    barrier.wait();
    {
        let conn = state.conn();
        gateway_keys::delete(&conn, row.id).unwrap();
        state.forget_gateway_key();
    }
    for worker in workers {
        worker.join().unwrap();
    }
    for _ in 0..100 {
        assert!(!state.check_gateway_key(&key));
    }
    drop(state);
    std::fs::remove_dir_all(cfg.db_path.parent().unwrap()).unwrap();
}
