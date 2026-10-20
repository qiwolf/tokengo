//! Credential catalog discovery must use only the selected key and endpoint.
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use reqwest::Client;
use serde_json::{json, Value};
use tokengo_server::{app, db, AppState, Config, SharedState};
struct Server {
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    Server {
        base,
        task: tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    }
}
struct Fixture {
    state: SharedState,
    server: Server,
    token: String,
}
impl Fixture {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("provider-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            ..Config::default()
        })
        .unwrap();
        db::sessions::set_panel_password(&state.conn(), "catalog-password-123").unwrap();
        let token = db::sessions::create_session(&state.conn(), None).unwrap();
        let server = serve(app(state.clone())).await;
        Self {
            state,
            server,
            token,
        }
    }
    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let response = Client::new()
            .post(format!("{}{path}", self.server.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        (response.status(), response.json().await.unwrap())
    }
    async fn provider(&self, base: &str) -> i64 {
        let (status, v) = self
            .post(
                "/api/providers",
                json!({"name":format!("supplier-{}",uuid::Uuid::new_v4()),"base_url":base}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        v["provider"]["id"].as_i64().unwrap()
    }
    async fn key(&self, provider: i64, key: &str) -> i64 {
        let (status,v)=self.post(&format!("/api/providers/{provider}/credentials"),json!({"label":key,"key":key,"models":[{"model":"retained","upstream":"unchanged"}]})).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        v["credential"]["id"].as_i64().unwrap()
    }
    fn snapshot(&self) -> Value {
        let c = self.state.conn();
        let mut all = Vec::new();
        // Catalog reads must not save draft keys, source mappings, requests or ledger.
        for table in [
            "channels",
            "channel_models",
            "provider_credentials",
            "provider_connections",
            "source_credentials",
            "requests",
            "billing_ledger",
            "billing_budgets",
            "channel_catalog",
            "channel_secrets",
            "channel_headers",
        ] {
            let exists: bool = c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            if !exists {
                continue;
            }
            let mut statement = c
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let n = statement.column_count();
            let rows = statement
                .query_map([], |r| {
                    Ok((0..n)
                        .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                        .collect::<Vec<_>>())
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            all.push((table.to_owned(), rows));
        }
        json!(all)
    }
}
async fn keyed_catalog(headers: HeaderMap) -> Json<Value> {
    let auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("anonymous");
    let name = match auth {
        "Bearer key-a" => "only-A",
        "Bearer key-b" => "only-B",
        "anonymous" => "public-model",
        _ => "unexpected-auth",
    };
    Json(json!({"data":[{"id":name}]}))
}
#[tokio::test]
async fn saved_and_draft_keys_discover_independent_catalogs_without_configuration_writes() {
    let upstream = serve(Router::new().route("/v1/models", get(keyed_catalog))).await;
    let f = Fixture::new().await;
    let p = f.provider(&format!("{}/v1", upstream.base)).await;
    let a = f.key(p, "key-a").await;
    let b = f.key(p, "key-b").await;
    let before = f.snapshot();
    for (body, expected) in [
        (json!({"credential_id":a}), "only-A"),
        (json!({"credential_id":b}), "only-B"),
        (json!({"key":"key-a"}), "only-A"),
        (json!({"kind":"anonymous"}), "public-model"),
    ] {
        let (status, v) = f
            .post(&format!("/api/providers/{p}/models/discover"), body)
            .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["models"], json!([expected]));
        assert!(!v.to_string().contains("key-a"));
        assert!(!v.to_string().contains("key-b"));
    }
    assert_eq!(before, f.snapshot());
    let other = f.provider(&format!("{}/v1", upstream.base)).await;
    assert_eq!(
        f.post(
            &format!("/api/providers/{other}/models/discover"),
            json!({"credential_id":a})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn local_draft_endpoints_preserve_port_and_invalid_urls_are_rejected() {
    async fn catalog(State(name): State<&'static str>) -> Json<Value> {
        Json(json!({"data":[{"id":name}]}))
    }
    let a = serve(
        Router::new()
            .route("/models", get(catalog))
            .with_state("port-A"),
    )
    .await;
    let b = serve(
        Router::new()
            .route("/models", get(catalog))
            .with_state("port-B"),
    )
    .await;
    let f = Fixture::new().await;
    let (_, v) = f
        .post(
            "/api/providers",
            json!({"name":"local supplier","endpoint_mode":"local"}),
        )
        .await;
    let p = v["provider"]["id"].as_i64().unwrap();
    let before = f.snapshot();
    for (url, name) in [(&a.base, "port-A"), (&b.base, "port-B")] {
        let (status, v) = f
            .post(
                &format!("/api/providers/{p}/models/discover"),
                json!({"kind":"anonymous","base_url":url}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["models"], json!([name]));
    }
    for url in [
        "file:///tmp/test",
        "http://user:password@127.0.0.1",
        "http://127.0.0.1?token=private",
        "http://127.0.0.1/#x",
    ] {
        assert_eq!(
            f.post(
                &format!("/api/providers/{p}/models/discover"),
                json!({"key":"secret","base_url":url})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(before, f.snapshot());
}
#[tokio::test]
async fn errors_and_catalog_secret_echoes_are_not_exposed() {
    async fn bad(headers: HeaderMap) -> (StatusCode, String) {
        (
            StatusCode::UNAUTHORIZED,
            format!("upstream private body: {:?}", headers.get("authorization")),
        )
    }
    async fn echo(headers: HeaderMap) -> Json<Value> {
        Json(json!({"data":[{"id":headers.get("authorization").unwrap().to_str().unwrap()}]}))
    }
    let upstream = serve(
        Router::new()
            .route("/bad/models", get(bad))
            .route("/echo/models", get(echo)),
    )
    .await;
    let f = Fixture::new().await;
    for route in ["bad", "echo"] {
        let p = f.provider(&format!("{}/{route}", upstream.base)).await;
        let before = f.snapshot();
        let (status, v) = f
            .post(
                &format!("/api/providers/{p}/models/discover"),
                json!({"key":"do-not-echo"}),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{v}");
        assert!(!v.to_string().contains("do-not-echo"));
        assert!(!v.to_string().contains("upstream private body"));
        assert_eq!(before, f.snapshot());
    }
}
#[tokio::test]
async fn stored_custom_headers_are_used_and_catalog_redirects_are_not_followed() {
    async fn custom(headers: HeaderMap) -> Json<Value> {
        Json(
            json!({"data":[{"id":if headers.get("x-custom-auth").is_some_and(|v|v=="custom-private"){"custom-model"}else{"wrong"}}]}),
        )
    }
    let destination = serve(Router::new().route(
        "/models",
        get(|| async {
            panic!("catalog redirect must not be followed");
            #[allow(unreachable_code)]
            ""
        }),
    ))
    .await;
    let location = format!("{}/models", destination.base);
    let upstream = serve(Router::new().route("/custom/models", get(custom)).route(
        "/redirect/models",
        get(move || {
            let location = location.clone();
            async move { (StatusCode::FOUND, [("location", location)]) }
        }),
    ))
    .await;
    let f = Fixture::new().await;
    let p = f.provider(&format!("{}/custom", upstream.base)).await;
    let k = f.key(p, "key-a").await;
    let source = db::provider_pool::credentials(&f.state.conn(), p)
        .unwrap()
        .into_iter()
        .find(|c| c.id == k)
        .unwrap()
        .source_ids[0];
    db::channels::set_headers(
        &f.state.conn(),
        &f.state.secrets,
        tokengo_shared::ChannelId(source),
        &[("x-custom-auth".into(), "custom-private".into())],
    )
    .unwrap();
    let (status, v) = f
        .post(
            &format!("/api/providers/{p}/models/discover"),
            json!({"credential_id":k}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["models"], json!(["custom-model"]));
    let p = f.provider(&format!("{}/redirect", upstream.base)).await;
    let (status, v) = f
        .post(
            &format!("/api/providers/{p}/models/discover"),
            json!({"key":"key-a"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(v.to_string().contains("302"));
}
#[tokio::test]
async fn discovery_requires_admin_and_bound_oauth_account() {
    let f = Fixture::new().await;
    let p = f.provider("https://example.test/v1").await;
    let response = Client::new()
        .post(format!(
            "{}/api/providers/{p}/models/discover",
            f.server.base
        ))
        .json(&json!({"key":"key-a"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = Client::new()
        .post(format!(
            "{}/api/providers/{p}/models/discover",
            f.server.base
        ))
        .header("Cookie", format!("tokengo_session={}", f.token))
        .json(&json!({"key":"key-a"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "cookie mutations require same-origin CSRF protection"
    );
    let (status, v) = f
        .post(
            &format!("/api/providers/{p}/models/discover"),
            json!({"kind":"oauth"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(v.to_string().contains("OAuth"));
}

#[tokio::test]
async fn a_rotated_key_cannot_return_a_stale_catalog() {
    #[derive(Clone)]
    struct Barrier { entered: std::sync::Arc<tokio::sync::Notify>, release: std::sync::Arc<tokio::sync::Notify> }
    async fn held(State(barrier):State<Barrier>)->Json<Value>{
        barrier.entered.notify_one();barrier.release.notified().await;
        Json(json!({"data":[{"id":"old-key-model"}]}))
    }
    let barrier=Barrier{entered:Default::default(),release:Default::default()};
    let upstream=serve(Router::new().route("/v1/models",get(held)).with_state(barrier.clone())).await;
    let f=Fixture::new().await;let p=f.provider(&format!("{}/v1",upstream.base)).await;let k=f.key(p,"key-a").await;
    let request=Client::new().post(format!("{}/api/providers/{p}/models/discover",f.server.base)).bearer_auth(&f.token).json(&json!({"credential_id":k}));
    let task=tokio::spawn(async move{let r=request.send().await.unwrap();(r.status(),r.json::<Value>().await.unwrap())});
    tokio::time::timeout(std::time::Duration::from_secs(3),barrier.entered.notified()).await.unwrap();
    db::provider_pool::rotate_secret(&f.state.conn(),&f.state.secrets,k,"new-key").unwrap();
    barrier.release.notify_one();let (status,v)=task.await.unwrap();
    assert_eq!(status,StatusCode::CONFLICT,"{v}");assert!(!v.to_string().contains("old-key-model"));
}

#[tokio::test]
async fn explicit_reveal_is_root_only_csrf_protected_uncached_and_not_in_lists() {
    let f=Fixture::new().await;let p=f.provider("http://127.0.0.1:9/v1").await;let key=f.key(p,"saved-api-secret").await;
    let path=format!("{}/api/providers/{p}/credentials/{key}/reveal",f.server.base);
    let r=Client::new().post(&path).bearer_auth(&f.token).send().await.unwrap();
    assert_eq!(r.status(),StatusCode::OK);assert_eq!(r.headers()["cache-control"],"no-store");assert_eq!(r.headers()["pragma"],"no-cache");
    assert_eq!(r.json::<Value>().await.unwrap(),json!({"key":"saved-api-secret"}));
    let list=Client::new().get(format!("{}/api/providers/{p}/credentials",f.server.base)).bearer_auth(&f.token).send().await.unwrap().json::<Value>().await.unwrap();
    assert!(list["credentials"][0].get("key").is_none());
    assert!(list["credentials"][0].get("secret").is_none());
    let r=Client::new().post(&path).send().await.unwrap();assert_eq!(r.status(),StatusCode::UNAUTHORIZED);assert_eq!(r.headers()["cache-control"],"no-store");
    let r=Client::new().post(&path).header("Cookie",format!("tokengo_session={}",f.token)).send().await.unwrap();assert_eq!(r.status(),StatusCode::FORBIDDEN);
    let r=Client::new().post(&path).header("Cookie",format!("tokengo_session={}",f.token)).header("x-tokengo-panel","1").header("origin",&f.server.base).send().await.unwrap();assert_eq!(r.status(),StatusCode::OK);
    let ordinary={let c=f.state.conn();let user=db::identity::create_user(&c,"ordinary","user",&Default::default()).unwrap();let account=db::accounts::create(&c,"ordinary","ordinary",user.id,"test-only-hash").unwrap();let credential=db::accounts::by_id(&c,account.id).unwrap().unwrap();db::accounts::create_session(&c,&credential).unwrap()};
    let r=Client::new().post(&path).header("Cookie",format!("tokengo_account_session={ordinary}")).header("x-tokengo-panel","1").header("origin",&f.server.base).send().await.unwrap();assert_eq!(r.status(),StatusCode::FORBIDDEN);
    let other=f.provider("http://127.0.0.1:8/v1").await;
    assert_eq!(f.post(&format!("/api/providers/{other}/credentials/{key}/reveal"),json!({})).await.0,StatusCode::NOT_FOUND);
    assert_eq!(f.post(&format!("/api/providers/{p}/credentials/999999/reveal"),json!({})).await.0,StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reveal_requires_consistent_api_secrets_and_never_returns_oauth_tokens() {
    let f=Fixture::new().await;let p=f.provider("http://127.0.0.1:9/v1").await;let key=f.key(p,"same-secret").await;
    let source={let c=f.state.conn();let source=db::channels::create(&c,&db::channels::NewChannel{name:"second-source".into(),base_url:"http://127.0.0.1:9/v1".into(),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();db::channels::set_secret(&c,&f.state.secrets,source,"same-secret").unwrap();db::provider_channels::set_membership(&c,source.0,Some(p)).unwrap();assert_eq!(db::provider_pool::adopt_source(&c,&f.state.secrets,source.0,p).unwrap(),key);source};
    let path=format!("/api/providers/{p}/credentials/{key}/reveal");
    let before=f.snapshot();let (status,value)=f.post(&path,json!({})).await;assert_eq!(status,StatusCode::OK);assert_eq!(value["key"],"same-secret");assert_eq!(before,f.snapshot());
    db::channels::set_secret(&f.state.conn(),&f.state.secrets,source,"inconsistent-secret").unwrap();
    let (status,value)=f.post(&path,json!({})).await;assert_eq!(status,StatusCode::CONFLICT);assert!(!value.to_string().contains("inconsistent-secret"));
    db::channels::clear_secret(&f.state.conn(),source).unwrap();assert_eq!(f.post(&path,json!({})).await.0,StatusCode::CONFLICT);
    {let c=f.state.conn();for s in db::provider_pool::credentials(&c,p).unwrap()[0].source_ids.clone(){db::channels::clear_secret(&c,tokengo_shared::ChannelId(s)).unwrap();}}
    assert_eq!(f.post(&path,json!({})).await.0,StatusCode::NOT_FOUND);
    f.state.conn().execute("UPDATE provider_credentials SET kind='oauth' WHERE id=?1",[key]).unwrap();
    let (status,value)=f.post(&path,json!({})).await;assert_eq!(status,StatusCode::BAD_REQUEST);assert!(!value.to_string().contains("same-secret"));assert!(value.get("key").is_none());
    f.state.conn().execute("UPDATE provider_credentials SET kind='anonymous' WHERE id=?1",[key]).unwrap();assert_eq!(f.post(&path,json!({})).await.0,StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn local_key_without_models_is_unroutable_until_models_are_configured() {
    async fn generation()->Json<Value>{Json(json!({"id":"isolated","object":"chat.completion","model":"actual-upstream","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}))}
    let upstream=serve(Router::new().route("/v1/chat/completions",axum::routing::post(generation))).await;
    let f=Fixture::new().await;db::sessions::set_gateway_key(&f.state.conn(),&f.state.secrets,"test-gateway").unwrap();
    let (_,value)=f.post("/api/providers",json!({"name":"local model supplier","endpoint_mode":"local"})).await;let p=value["provider"]["id"].as_i64().unwrap();
    // An existing port must not donate its models to the newly saved key.
    let (status,value)=f.post(&format!("/api/providers/{p}/credentials"),json!({"label":"other-port","base_url":"http://127.0.0.1:9/v1","key":"different-port-key","models":[{"model":"other-model"}]})).await;assert_eq!(status,StatusCode::OK,"{value}");
    let (status,value)=f.post(&format!("/api/providers/{p}/credentials"),json!({"label":"empty-new-key","base_url":format!("{}/v1",upstream.base),"key":"new-local-key","models":[]})).await;assert_eq!(status,StatusCode::OK,"{value}");assert_eq!(value["credential"]["models"],json!([]));let key=value["credential"]["id"].as_i64().unwrap();
    let source=value["source_id"].as_i64().unwrap();assert!(db::channels::get(&f.state.conn(),tokengo_shared::ChannelId(source)).unwrap().unwrap().models.is_empty());
    let response=Client::new().post(format!("{}/v1/chat/completions",f.server.base)).bearer_auth("test-gateway").json(&json!({"model":"new-local-model","messages":[{"role":"user","content":"hi"}]})).send().await.unwrap();assert!(!response.status().is_success());
    let listed=Client::new().get(format!("{}/v1/models",f.server.base)).bearer_auth("test-gateway").send().await.unwrap().json::<Value>().await.unwrap();assert!(!listed["data"].as_array().unwrap().iter().any(|m|m["id"]=="new-local-model"));
    let response=Client::new().patch(format!("{}/api/providers/{p}/credentials/{key}",f.server.base)).bearer_auth(&f.token).json(&json!({"models":[{"model":"new-local-model","upstream":"actual-upstream","enabled":true}]})).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);
    let response=Client::new().post(format!("{}/v1/chat/completions",f.server.base)).bearer_auth("test-gateway").json(&json!({"model":"new-local-model","messages":[{"role":"user","content":"hi"}]})).send().await.unwrap();let status=response.status();let body=response.text().await.unwrap();assert_eq!(status,StatusCode::OK,"{body}");
}

#[tokio::test]
async fn online_key_can_disable_legacy_model_inheritance_until_models_are_configured() {
    async fn generation()->Json<Value>{Json(json!({"id":"isolated","object":"chat.completion","model":"actual-upstream","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}))}
    let upstream=serve(Router::new().route("/v1/chat/completions",axum::routing::post(generation))).await;
    let f=Fixture::new().await;db::sessions::set_gateway_key(&f.state.conn(),&f.state.secrets,"test-gateway").unwrap();
    let (_,value)=f.post("/api/providers",json!({"name":"online model supplier","base_url":format!("{}/v1",upstream.base)})).await;let p=value["provider"]["id"].as_i64().unwrap();
    // An existing key must not donate its models to this explicit standalone key.
    let (status,value)=f.post(&format!("/api/providers/{p}/credentials"),json!({"label":"other-port","key":"different-port-key","models":[{"model":"other-model"}]})).await;assert_eq!(status,StatusCode::OK,"{value}");
    let (status,value)=f.post(&format!("/api/providers/{p}/credentials"),json!({"label":"empty-new-key","key":"new-online-key","inherit_models":false})).await;assert_eq!(status,StatusCode::OK,"{value}");assert_eq!(value["credential"]["models"],json!([]));let key=value["credential"]["id"].as_i64().unwrap();
    let source=value["source_id"].as_i64().unwrap();assert!(db::channels::get(&f.state.conn(),tokengo_shared::ChannelId(source)).unwrap().unwrap().models.is_empty());
    let (status,legacy)=f.post(&format!("/api/providers/{p}/credentials"),json!({"label":"legacy-default","key":"legacy-default-key"})).await;assert_eq!(status,StatusCode::OK,"{legacy}");assert!(legacy["credential"]["models"].as_array().unwrap().iter().any(|m|m["model"]=="other-model"));
    assert!(db::channels::get(&f.state.conn(),tokengo_shared::ChannelId(source)).unwrap().unwrap().models.is_empty());
    let response=Client::new().post(format!("{}/v1/chat/completions",f.server.base)).bearer_auth("test-gateway").json(&json!({"model":"new-local-model","messages":[{"role":"user","content":"hi"}]})).send().await.unwrap();assert!(!response.status().is_success());
    let listed=Client::new().get(format!("{}/v1/models",f.server.base)).bearer_auth("test-gateway").send().await.unwrap().json::<Value>().await.unwrap();assert!(!listed["data"].as_array().unwrap().iter().any(|m|m["id"]=="new-local-model"));
    let response=Client::new().patch(format!("{}/api/providers/{p}/credentials/{key}",f.server.base)).bearer_auth(&f.token).json(&json!({"models":[{"model":"new-local-model","upstream":"actual-upstream","enabled":true}]})).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);
    let response=Client::new().post(format!("{}/v1/chat/completions",f.server.base)).bearer_auth("test-gateway").json(&json!({"model":"new-local-model","messages":[{"role":"user","content":"hi"}]})).send().await.unwrap();let status=response.status();let body=response.text().await.unwrap();assert_eq!(status,StatusCode::OK,"{body}");
}
