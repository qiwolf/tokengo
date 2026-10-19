//! OAuth administration is exclusively a root panel session, never a general vkey.
use axum::{body::Body, http::Request};
use serde_json::{json, Value};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tower::ServiceExt;
async fn call(
    state: &SharedState,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> (u16, Value) {
    let mut r = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");
    if let Some(k) = key {
        r = r.header("Authorization", format!("Bearer {k}"));
    }
    let response = app(state.clone())
        .oneshot(r.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let code = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (code, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn oauth_routes_require_panel_root_not_admin_vkey_and_publish_no_credentials() {
    let dir = std::env::temp_dir().join(format!("oauth-admin-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = AppState::init(Config {
        db_path: dir.join("db"),
        master_key_path: dir.join("master").to_string_lossy().into(),
        ..Default::default()
    })
    .unwrap();
    let (panel, vkey) = {
        let c = state.conn();
        let panel = db::sessions::create_session(&c, None).unwrap();
        let policy = db::identity::Policy {
            admin_access: true,
            ..Default::default()
        };
        let user = db::identity::create_user(&c, "syntheticadmin", "admin", &policy).unwrap();
        let (_, key) =
            db::identity::create_key(&c, user.id, "syntheticadmin", &policy, None, None).unwrap();
        (panel, key)
    };
    for (method, path, body) in [
        ("GET", "/api/oauth/accounts", json!({})),
        ("GET", "/api/oauth/accounts/1/models", json!({})),
        (
            "POST",
            "/api/oauth/start",
            json!({"label":"test","redirect_uri":"http://127.0.0.1:1455/auth/callback"}),
        ),
        (
            "POST",
            "/api/oauth/complete",
            json!({"attempt_id":"fake","state":"fake"}),
        ),
        ("PUT", "/api/oauth/channels/1", json!({"account_id":1})),
        ("GET", "/api/oauth/channels/1", json!({})),
        ("DELETE", "/api/oauth/channels/1", json!({})),
        ("POST", "/api/oauth/accounts/1/revoke", json!({})),
    ] {
        for credential in [None, Some(vkey.as_str())] {
            assert_eq!(
                call(&state, method, path, credential, body.clone()).await.0,
                401
            );
        }
    }
    let (code, accounts) = call(
        &state,
        "GET",
        "/api/oauth/accounts",
        Some(&panel),
        json!({}),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(accounts, json!({"accounts":[]}));
    let (code, start) = call(
        &state,
        "POST",
        "/api/oauth/start",
        Some(&panel),
        json!({"label":"test","redirect_uri":"http://127.0.0.1:1455/auth/callback"}),
    )
    .await;
    assert_eq!(code, 200);
    assert!(start["authorization_url"]
        .as_str()
        .unwrap()
        .starts_with("https://auth.openai.com/api/accounts/authorize?"));
    for name in [
        "access_token",
        "refresh_token",
        "id_token",
        "code_verifier",
        "nonce",
    ] {
        assert!(start.get(name).is_none());
    }
    let url = reqwest::Url::parse(start["authorization_url"].as_str().unwrap()).unwrap();
    let statevalue = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .to_string();
    let (code, reply) = call(
        &state,
        "POST",
        "/api/oauth/complete",
        Some(&panel),
        json!({"attempt_id":start["attempt_id"],"state":statevalue,"error":"access_denied"}),
    )
    .await;
    assert_eq!(code, 400);
    assert_eq!(reply["error"]["message"], "oauth_authorization_denied");
    std::fs::remove_dir_all(dir).ok();
}
