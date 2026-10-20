//! Real loopback HTTP for explicit provider grouping; fixed test credentials only.
use axum::{routing::post, Json, Router};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokengo_server::{app, db, AppState, Config, SharedState};
struct Fixture {
    state: SharedState,
    cfg: Config,
    base: String,
    token: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move { axum::serve(l, router).await.unwrap() }),
    )
}
impl Fixture {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("provider-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            ..Config::default()
        };
        let state = AppState::init(cfg.clone()).unwrap();
        db::sessions::set_panel_password(&state.conn(), "test-password-1234").unwrap();
        let token = db::sessions::create_session(&state.conn(), None).unwrap();
        db::sessions::set_gateway_key(&state.conn(), &state.secrets, "provider-test-key").unwrap();
        let mut c = state.routing().cfg.clone();
        c.retry_times = 0;
        c.probe_enabled = false;
        db::routing::save_config(&state.conn(), &c).unwrap();
        state.routing().cfg = c;
        let (base, task) = serve(app(state.clone())).await;
        Self {
            state,
            cfg,
            base,
            token,
            task,
        }
    }
    async fn req(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let r = reqwest::Client::new()
            .request(method.parse().unwrap(), format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn channel(&self, base: &str) -> i64 {
        let(s,v)=self.req("POST","/api/channels",json!({"name":"connection","base_url":base,"key":"fixed-upstream-secret","models":[{"model":"m","upstream":"exact-M","vision":true,"drop_fields":"unused"},{"model":"other","upstream":"other-exact"}]})).await;
        assert_eq!(s, 200, "{v}");
        v["id"].as_i64().unwrap()
    }
    async fn provider(&self, name: &str) -> i64 {
        let (s, v) = self
            .req("POST", "/api/providers", json!({"name":name}))
            .await;
        assert_eq!(s, 200, "{v}");
        v["provider"]["id"].as_i64().unwrap()
    }
    async fn assign(&self, c: i64, p: Option<i64>) {
        let (s, v) = self
            .req(
                "PUT",
                &format!("/api/channels/{c}/provider"),
                json!({"provider_id":p}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }
    async fn models(&self) -> Value {
        reqwest::Client::new()
            .get(format!("{}/v1/models", self.base))
            .bearer_auth("provider-test-key")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn chat(&self, m: &str) -> StatusCode {
        reqwest::Client::new()
            .post(format!("{}/v1/chat/completions", self.base))
            .bearer_auth("provider-test-key")
            .json(&json!({"model":m,"messages":[{"role":"user","content":"hello"}],"max_tokens":8}))
            .send()
            .await
            .unwrap()
            .status()
    }
}
#[tokio::test]
async fn provider_membership_atomic_model_switch_and_restart_preserve_identity() {
    let f = Fixture::new().await;
    let a = f.channel("http://127.0.0.1:1").await;
    let b = f.channel("http://127.0.0.1:2").await;
    let p = f.provider("Explicit provider").await;
    f.assign(a, Some(p)).await;
    f.assign(b, Some(p)).await;
    let active = tokengo_shared::routing::RouteState {
        active: Some(tokengo_shared::ChannelId(a)),
        ..Default::default()
    };
    f.state.routing().restore("m", active.clone());
    db::routing::save_state(&f.state.conn(), "m", &active).unwrap();
    let before = db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(a))
        .unwrap()
        .unwrap();
    let (s, _) = f
        .req(
            "PUT",
            &format!("/api/providers/{p}/models/enabled"),
            json!({"model":"M","enabled":false,"connection_ids":[a,b]}),
        )
        .await;
    assert_eq!(s, 200);
    assert!(
        f.state
            .routing()
            .all_states()
            .get("m")
            .unwrap()
            .active
            .is_none(),
        "normalized model switch must reset existing route state"
    );
    let (s, v) = f.req("GET", "/api/model-routes", json!(null)).await;
    assert_eq!(s, 200);
    let m = v["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "m")
        .unwrap();
    assert_eq!(m["declared_by"], json!([a, b]));
    assert_eq!(m["routed"], false);
    let row = db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(a))
        .unwrap()
        .unwrap();
    assert_eq!(row.models[0].upstream, before.models[0].upstream);
    assert_eq!(row.models[0].drop_fields, before.models[0].drop_fields);
    assert_eq!(row.models.len(), 2);
    let fresh = AppState::init(f.cfg.clone()).unwrap();
    assert!(!db::model_routes::routes_for(&fresh.conn(), "m")
        .unwrap()
        .unwrap()
        .routed());
    assert!(db::model_routes::routes_for(&fresh.conn(), "other")
        .unwrap()
        .unwrap()
        .routed());
    let (s, _) = f
        .req(
            "PUT",
            &format!("/api/providers/{p}/models/enabled"),
            json!({"model":"m","enabled":true,"connection_ids":[a,99999]}),
        )
        .await;
    assert_eq!(s, 400);
    assert!(!db::model_routes::routes_for(&f.state.conn(), "m")
        .unwrap()
        .unwrap()
        .routed());
    let (_, v) = f.req("GET", "/api/providers", json!(null)).await;
    assert_eq!(
        v["providers"][0]["models"][0]["enabled_connection_ids"],
        json!([])
    );
    assert!(!v.to_string().contains("fixed-upstream-secret"));
}
#[tokio::test]
async fn enabled_model_parent_and_visibility_filter_real_forward_and_public_list() {
    let(up,t)=serve(Router::new().route("/chat/completions",post(|Json(v):Json<Value>|async move{assert!(v["model"]=="exact-M"||v["model"]=="other-exact");Json(json!({"choices":[{"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))}))).await;
    let f = Fixture::new().await;
    let a = f.channel(&up).await;
    let p = f.provider("provider").await;
    f.assign(a, Some(p)).await;
    assert_eq!(f.chat("m").await, 200);
    let (s, _) = f
        .req(
            "PUT",
            &format!("/api/channels/{a}/models/enabled"),
            json!({"model":"m","enabled":false}),
        )
        .await;
    assert_eq!(s, 200);
    assert!(!f.models().await["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["id"] == "m"));
    assert_ne!(f.chat("m").await, 200);
    assert_eq!(f.chat("other").await, 200);
    f.req(
        "PUT",
        &format!("/api/channels/{a}/models/enabled"),
        json!({"model":"m","enabled":true}),
    )
    .await;
    let (s, _) = f
        .req(
            "PUT",
            "/api/model-visibility?model=m",
            json!({"visible":false}),
        )
        .await;
    assert_eq!(s, 200);
    assert!(!f.models().await["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["id"] == "m"));
    assert_eq!(f.chat("m").await, 200);
    assert!(!db::model_routes::auto_candidates(
        &f.state.conn(),
        tokengo_shared::ModelKind::Chat,
        false,
        db::now_ms()
    )
    .unwrap()
    .iter()
    .any(|x| x == "m"));
    let (s, _) = f
        .req(
            "PUT",
            &format!("/api/providers/{p}"),
            json!({"name":"provider","enabled":false}),
        )
        .await;
    assert_eq!(s, 200);
    assert!(f.models().await["data"].as_array().unwrap().is_empty());
    assert_ne!(f.chat("other").await, 200);
    f.req(
        "PUT",
        &format!("/api/providers/{p}"),
        json!({"name":"provider","enabled":true}),
    )
    .await;
    assert_eq!(f.chat("other").await, 200);
    t.abort();
}
#[tokio::test]
async fn legacy_editor_omission_does_not_reenable_and_explicit_mapping_switch_preserves_generation()
{
    let f = Fixture::new().await;
    let a = f.channel("http://127.0.0.1:1").await;
    let gen = || {
        f.state
            .conn()
            .query_row(
                "SELECT binding_generation FROM channels WHERE id=?1",
                [a],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    };
    let before = gen();
    let (s, _) = f
        .req(
            "PUT",
            &format!("/api/channels/{a}/models/enabled"),
            json!({"model":"m","enabled":false}),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(gen(), before);
    let(s,v)=f.req("PUT",&format!("/api/channels/{a}"),json!({"name":"edited connection","base_url":"http://127.0.0.1:1","models":[{"model":"m","upstream":"exact-M","vision":true,"drop_fields":"unused"},{"model":"other","upstream":"other-exact"}]})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["models"][0]["enabled"], false);
    assert_eq!(gen(), before);
    let(s,v)=f.req("PUT",&format!("/api/channels/{a}"),json!({"name":"edited connection","base_url":"http://127.0.0.1:1","models":[{"model":"m","upstream":"exact-M","vision":true,"drop_fields":"unused","enabled":true},{"model":"other","upstream":"other-exact"}]})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["models"][0]["enabled"], true);
}
#[tokio::test]
async fn grouping_is_explicit_unique_validated_root_and_csrf_only() {
    let f = Fixture::new().await;
    let a = f.channel("http://127.0.0.1:1").await;
    let p = f.provider("one").await;
    let q = f.provider("two").await;
    let (_, v) = f.req("GET", "/api/providers", json!(null)).await;
    assert_eq!(v["ungrouped_connection_ids"], json!([a]));
    f.assign(a, Some(p)).await;
    f.assign(a, Some(q)).await;
    let (_, v) = f.req("GET", "/api/providers", json!(null)).await;
    let one = v["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == p)
        .unwrap();
    assert_eq!(one["connection_ids"], json!([]));
    let (s, _) = f
        .req("POST", "/api/providers", json!({"name":"  TWO "}))
        .await;
    assert_eq!(s, 400);
    for ids in [json!([]), json!([a, a]), json!([a])] {
        let (s, _) = f
            .req(
                "PUT",
                &format!("/api/providers/{p}/models/enabled"),
                json!({"model":"m","enabled":false,"connection_ids":ids}),
            )
            .await;
        assert_eq!(s, 400);
    }
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{}/api/providers", f.base))
            .bearer_auth("provider-test-key")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let response = reqwest::Client::new()
        .post(format!("{}/api/account/login", f.base))
        .header("host", "localhost")
        .header("X-Tokengo-Panel", "1")
        .json(&json!({"username":"admin","password":"test-password-1234"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let response = reqwest::Client::new()
        .post(format!("{}/api/providers", f.base))
        .header("host", "localhost")
        .header("cookie", &cookie)
        .json(&json!({"name":"csrf denied"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = reqwest::Client::new()
        .post(format!("{}/api/providers", f.base))
        .header("host", "localhost")
        .header("cookie", &cookie)
        .header("X-Tokengo-Panel", "1")
        .json(&json!({"name":"csrf accepted"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    f.assign(a, None).await;
    let (_, v) = f.req("GET", "/api/channels", json!(null)).await;
    assert!(v["channels"][0]["provider_id"].is_null());
}
#[test]
fn migration_33_preserves_existing_connections_models_and_secrets() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);").unwrap();
    for (v, sql) in db::schema::MIGRATIONS.iter().filter(|(v, _)| *v <= 32) {
        c.execute_batch(sql).unwrap();
        c.execute("INSERT INTO schema_migrations(version) VALUES(?1)", [v])
            .unwrap();
    }
    c.execute("INSERT INTO channels(id,name,base_url,max_concurrent) VALUES(71,'legacy','https://unit.invalid/v1',3)",[]).unwrap();
    c.execute("INSERT INTO channel_models(channel_id,model_name,upstream_model,vision,drop_fields) VALUES(71,'m','Exact-UP',1,'reasoning_effort')",[]).unwrap();
    c.execute(
        "INSERT INTO channel_secrets(channel_id,nonce,cipher) VALUES(71,X'010203',X'040506')",
        [],
    )
    .unwrap();
    let original: Vec<u8> = c
        .query_row(
            "SELECT cipher FROM channel_secrets WHERE channel_id=71",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let gen: i64 = c
        .query_row(
            "SELECT binding_generation FROM channels WHERE id=71",
            [],
            |r| r.get(0),
        )
        .unwrap();
    db::migrate(&c).unwrap();
    db::migrate(&c).unwrap();
    let row = db::channels::get(&c, tokengo_shared::ChannelId(71))
        .unwrap()
        .unwrap();
    assert!(row.provider_id.is_none());
    assert_eq!(row.max_concurrent, 3);
    assert!(row.models[0].enabled);
    assert_eq!(row.models[0].upstream.as_deref(), Some("Exact-UP"));
    assert!(row.models[0].vision);
    assert_eq!(row.models[0].drop_fields, vec!["reasoning_effort"]);
    assert_eq!(
        c.query_row(
            "SELECT cipher FROM channel_secrets WHERE channel_id=71",
            [],
            |r| r.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        original
    );
    assert_eq!(
        c.query_row(
            "SELECT binding_generation FROM channels WHERE id=71",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        gen
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM provider_channels", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[tokio::test]
async fn metadata_request_cannot_assign_old_identity_capabilities_after_secret_change() {
    use std::sync::Arc;
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let handler_started = started.clone();
    let handler_release = release.clone();
    let (up,t)=serve(Router::new().route("/models",axum::routing::get(move||{let started=handler_started.clone();let release=handler_release.clone();async move{started.notify_one();release.notified().await;Json(json!({"data":[{"id":"exact-M","supported_reasoning_levels":[{"effort":"high"}]}]}))}}))).await;
    let f = Fixture::new().await;
    let id = f.channel(&up).await;
    let url = format!("{}/api/channels/{id}/models", f.base);
    let token = f.token.clone();
    let task = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
        .await
        .unwrap();
    db::channels::set_secret(
        &f.state.conn(),
        &f.state.secrets,
        tokengo_shared::ChannelId(id),
        "new-fixed-upstream-secret",
    )
    .unwrap();
    release.notify_one();
    let response = task.await.unwrap();
    assert_eq!(response.status(), 400);
    let value: Value = response.json().await.unwrap();
    assert!(value["error"]["message"]
        .as_str()
        .unwrap()
        .contains("configuration changed"));
    assert!(!value.to_string().contains("new-fixed-upstream-secret"));
    assert!(
        db::catalog::list(&f.state.conn(), tokengo_shared::ChannelId(id))
            .unwrap()
            .is_empty()
    );
    t.abort();
}
#[tokio::test]
async fn pulled_reasoning_metadata_maps_exact_model_and_configures_only_explicit_defaults() {
    let(up,t)=serve(Router::new().route("/models",axum::routing::get(||async{Json(json!({"data":[{"id":"exact-M","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"},{"effort":"xhigh"}],"default_reasoning_level":"low"},{"id":"other-exact","reasoning":true}]}))}))).await;
    let f = Fixture::new().await;
    let id = f.channel(&up).await;
    let (s, v) = f
        .req("POST", &format!("/api/channels/{id}/models"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = f.req("GET", "/api/channels", json!(null)).await;
    assert_eq!(s, 200);
    let models = v["channels"][0]["models"].as_array().unwrap();
    assert_eq!(
        models[0]["reasoning"]["levels"],
        json!(["low", "high", "xhigh"])
    );
    assert_eq!(models[1]["reasoning"]["status"], "unknown");
    let (s, v) = f
        .req(
            "PUT",
            &format!("/api/channels/{id}/models/reasoning"),
            json!({"model":"m","manual_levels":null,"default_effort":"xhigh"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["reasoning"]["default_effort"], "xhigh");
    let (s, _) = f
        .req(
            "PUT",
            &format!("/api/channels/{id}/models/reasoning"),
            json!({"model":"m","manual_levels":null,"default_effort":"ultra"}),
        )
        .await;
    assert_eq!(s, 400);
    let (s, v) = f
        .req(
            "PUT",
            &format!("/api/channels/{id}/models/reasoning"),
            json!({"model":"m","manual_levels":null,"default_effort":null}),
        )
        .await;
    assert_eq!(s, 200);
    assert!(v["reasoning"]["configured_default"].as_bool().unwrap());
    assert!(v["reasoning"]["default_effort"].is_null());
    let (s, v) = f
        .req("GET", &format!("/api/channels/{id}/catalog"), json!(null))
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        v["info"][0]["reasoning"]["levels"],
        json!(["low", "high", "xhigh"])
    );
    t.abort();
}
#[tokio::test]
async fn reasoning_defaults_reach_real_chat_and_explicit_invalid_null_and_other_alias_remain_distinct(
) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let capture = seen.clone();
    let(up,t)=serve(Router::new().route("/models",axum::routing::get(||async{Json(json!({"data":[{"id":"exact-M","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}]}]}))})).route("/chat/completions",post(move|Json(v):Json<Value>|{let seen=capture.clone();async move{seen.lock().unwrap().push(v);Json(json!({"choices":[{"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))}}))).await;
    let f = Fixture::new().await;
    let id = f.channel(&up).await;
    assert_eq!(
        f.req("POST", &format!("/api/channels/{id}/models"), json!({}))
            .await
            .0,
        200
    );
    assert_eq!(
        f.req(
            "PUT",
            &format!("/api/channels/{id}/models/reasoning"),
            json!({"model":"m","manual_levels":null,"default_effort":"high"})
        )
        .await
        .0,
        200
    );
    assert_eq!(f.chat("m").await, 200);
    let send = |effort: &str| {
        let url = format!("{}/v1/chat/completions", f.base);
        let effort = effort.to_string();
        async move {
            reqwest::Client::new().post(url).bearer_auth("provider-test-key").json(&json!({"model":"m","messages":[{"role":"user","content":"hello"}],"max_tokens":8,"reasoning_effort":effort})).send().await.unwrap().status()
        }
    };
    assert_eq!(send("low").await, 200);
    assert_eq!(send("max").await, 400);
    assert_eq!(seen.lock().unwrap().len(), 2);
    assert_eq!(
        f.req(
            "PUT",
            &format!("/api/channels/{id}/models/reasoning"),
            json!({"model":"m","manual_levels":null,"default_effort":null})
        )
        .await
        .0,
        200
    );
    assert_eq!(f.chat("m").await, 200);
    assert_eq!(f.chat("other").await, 200);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[0]["reasoning_effort"], "high");
    assert_eq!(seen[1]["reasoning_effort"], "low");
    assert!(seen[2].get("reasoning_effort").is_none());
    assert!(seen[3].get("reasoning_effort").is_none());
    t.abort();
}
#[tokio::test]
async fn shared_provider_never_broadens_existing_connection_acl() {
    let f = Fixture::new().await;
    let a = f.channel("http://127.0.0.1:1").await;
    let b = f.channel("http://127.0.0.1:2").await;
    let p = f.provider("same explicit provider").await;
    f.assign(a, Some(p)).await;
    f.assign(b, Some(p)).await;
    let key = {
        let c = f.state.conn();
        let policy = db::identity::Policy {
            allowed_channels: Some(vec![a]),
            ..Default::default()
        };
        let u = db::identity::create_user(&c, "provider-acl-user", "user", &policy).unwrap();
        db::identity::create_key(&c, u.id, "only-original-a", &Default::default(), None, None)
            .unwrap()
            .1
    };
    assert_eq!(
        f.req(
            "PUT",
            &format!("/api/channels/{a}/models/enabled"),
            json!({"model":"m","enabled":false})
        )
        .await
        .0,
        200
    );
    let client = reqwest::Client::new();
    let models: Value = client
        .get(format!("{}/v1/models", f.base))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!models["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["id"] == "m"));
    assert!(models["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["id"] == "other"));
    let response = client
        .post(format!("{}/v1/chat/completions", f.base))
        .bearer_auth(&key)
        .json(&json!({"model":"m","messages":[{"role":"user","content":"hello"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let count: i64 = f
        .state
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM attempts WHERE channel_id=?1",
            [b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn provider_delete_requires_empty_group_and_preserves_connections_and_history() {
    let f = Fixture::new().await;
    let p = f.provider("delete only empty").await;
    let a = f.channel("http://127.0.0.1:1").await;
    f.assign(a, Some(p)).await;
    let snapshot = protected_connection_snapshot(&f.state.conn());
    let before = db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(a))
        .unwrap()
        .unwrap();
    let (s, v) = f
        .req("DELETE", &format!("/api/providers/{p}"), json!(null))
        .await;
    assert_eq!(s, 409, "{v}");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("请先移动/删除连接"));
    assert_eq!(protected_connection_snapshot(&f.state.conn()), snapshot);
    assert_eq!(
        db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(a))
            .unwrap()
            .unwrap(),
        before
    );
    assert!(db::provider_channels::exists(&f.state.conn(), p).unwrap());
    f.assign(a, None).await;
    let (s, v) = f
        .req("DELETE", &format!("/api/providers/{p}"), json!(null))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["deleted"], true);
    assert!(
        db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(a))
            .unwrap()
            .is_some()
    );
    assert_eq!(
        f.req("DELETE", &format!("/api/providers/{p}"), json!(null))
            .await
            .0,
        404
    );
}

#[tokio::test]
async fn provider_delete_requires_root_and_browser_csrf() {
    let f = Fixture::new().await;
    let p = f.provider("protected delete").await;
    let c = reqwest::Client::new();
    let path = format!("{}/api/providers/{p}", f.base);
    assert_eq!(c.delete(&path).send().await.unwrap().status(), 401);
    assert_eq!(
        c.delete(&path)
            .bearer_auth("provider-test-key")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let login = c
        .post(format!("{}/api/account/login", f.base))
        .header("host", "localhost")
        .header("X-Tokengo-Panel", "1")
        .json(&json!({"username":"admin","password":"test-password-1234"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert_eq!(
        c.delete(&path)
            .header("host", "localhost")
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert!(db::provider_channels::exists(&f.state.conn(), p).unwrap());
    assert_eq!(
        c.delete(&path)
            .header("host", "localhost")
            .header("cookie", cookie)
            .header("X-Tokengo-Panel", "1")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn connection_delete_rejects_dependencies_and_preserves_history() {
    let f = Fixture::new().await;
    let a = f.channel("http://127.0.0.1:1").await;
    let b = f.channel("http://127.0.0.1:2").await;
    let before = db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(a))
        .unwrap()
        .unwrap();
    let assert_blocked = |v: &Value| {
        assert!(
            v["error"]["message"].as_str().unwrap().contains("停用"),
            "{v}"
        )
    };
    {
        let c = f.state.conn();
        let mut user = db::identity::user(&c, 1).unwrap().unwrap();
        user.policy.allowed_channels = Some(vec![a]);
        db::identity::update_user(&c, &user).unwrap();
    }
    let (s, v) = f
        .req("DELETE", &format!("/api/channels/{a}"), json!(null))
        .await;
    assert_eq!(s, 409, "{v}");
    assert_blocked(&v);
    assert_eq!(
        db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(a))
            .unwrap()
            .unwrap(),
        before
    );
    {
        let c = f.state.conn();
        let mut u = db::identity::user(&c, 1).unwrap().unwrap();
        u.policy.allowed_channels = None;
        db::identity::update_user(&c, &u).unwrap();
        db::route_policy::save(
            &c,
            "m",
            &db::route_policy::RoutePolicy {
                channel_ids: Some(vec![a]),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let (s, v) = f
        .req("DELETE", &format!("/api/channels/{a}"), json!(null))
        .await;
    assert_eq!(s, 409, "{v}");
    assert_blocked(&v);
    db::route_policy::delete(&f.state.conn(), "m").unwrap();
    {
        let c = f.state.conn();
        c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,debit_micro,status,created_ms) VALUES('retained-history',1,1,991,?1,'m',0,0,0,0,'success',1)",[a]).unwrap();
    }
    let (s, v) = f
        .req("DELETE", &format!("/api/channels/{a}"), json!(null))
        .await;
    assert_eq!(s, 409, "{v}");
    assert_blocked(&v);
    assert_eq!(
        f.state
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM billing_ledger WHERE attempt_id='retained-history'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        f.req("DELETE", &format!("/api/channels/{b}"), json!(null))
            .await
            .0,
        200
    );
    assert_eq!(
        f.req("DELETE", &format!("/api/channels/{b}"), json!(null))
            .await
            .0,
        404
    );
}

fn protected_connection_snapshot(c: &rusqlite::Connection) -> Vec<(String, Vec<Vec<String>>)> {
    [
        "channels",
        "channel_models",
        "channel_secrets",
        "channel_headers",
        "provider_connections",
        "users",
        "gateway_keys",
        "oauth_accounts",
        "oauth_channel_bindings",
        "requests",
        "attempts",
        "billing_ledger",
        "settings",
        "protocol_sessions",
    ]
    .into_iter()
    .map(|table| {
        let mut st = c
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let count = st.column_count();
        let values = st
            .query_map([], |row| {
                Ok((0..count)
                    .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                    .collect::<Vec<_>>())
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        (table.to_string(), values)
    })
    .collect()
}

#[tokio::test]
async fn connection_delete_blocks_oauth_key_acl_session_and_invalid_pins_without_mutation() {
    let f = Fixture::new().await;
    for reason in [
        "oauth",
        "key_acl",
        "pin",
        "invalid_pin",
        "request",
        "attempt",
    ] {
        let a = f.channel("http://127.0.0.1:1").await;
        {
            let c = f.state.conn();
            match reason {
                "oauth" => {
                    c.execute("INSERT INTO oauth_accounts(label,issuer,subject,client_id,status,scopes_json,expires_at,earliest_refresh_at) VALUES('test','issuer',?1,?1,'revoked','[]',0,0)",[format!("test-{a}")]).unwrap();
                    let account = c.last_insert_rowid();
                    c.execute(
                        "INSERT INTO oauth_channel_bindings(channel_id,account_id) VALUES(?1,?2)",
                        rusqlite::params![a, account],
                    )
                    .unwrap();
                }
                "key_acl" => {
                    let mut k = db::identity::keys(&c, 1).unwrap().remove(0);
                    k.policy.allowed_channels = Some(vec![a]);
                    db::identity::update_key(&c, &k).unwrap();
                }
                "pin" => {
                    c.execute("INSERT INTO protocol_sessions(key,pin_json,last_used_ms) VALUES(?1,?2,1)",rusqlite::params![format!("pin-{a}"),json!({"channel":a,"generation":1,"upstream_model":"exact-M","protocol":"chat"}).to_string()]).unwrap();
                }
                "invalid_pin" => {
                    c.execute("INSERT INTO protocol_sessions(key,pin_json,last_used_ms) VALUES(?1,'invalid json',1)",[format!("pin-{a}")]).unwrap();
                }
                "request" => {
                    c.execute("INSERT INTO requests(ts_ms,endpoint,model,status,channel_id) VALUES(1,'/v1/chat/completions','m','running',?1)",[a]).unwrap();
                }
                "attempt" => {
                    let request =
                        db::requests::start(&c, "/v1/chat/completions", "m", None, false).unwrap();
                    db::requests::add_attempt_args(
                        &c,
                        &db::requests::AttemptArgs {
                            request_id: request,
                            ordinal: 1,
                            channel_id: a,
                            outcome: "success".into(),
                            http_status: Some(200),
                            duration_ms: 1,
                            error: None,
                            first_byte_ms: None,
                            model_name: "m".into(),
                            upstream_model: "exact-M".into(),
                        },
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
        }
        let before = protected_connection_snapshot(&f.state.conn());
        let (s, v) = f
            .req("DELETE", &format!("/api/channels/{a}"), json!(null))
            .await;
        assert_eq!(s, 409, "{reason}: {v}");
        assert_eq!(v["error"]["type"], "connection_in_use");
        let category = match reason {
            "oauth" => "OAuth绑定",
            "key_acl" => "API Key来源授权",
            "pin" => "原生会话绑定",
            "invalid_pin" => "记录无效",
            "request" => "请求历史/进行中请求",
            "attempt" => "来源尝试历史",
            _ => unreachable!(),
        };
        assert!(
            v["error"]["message"].as_str().unwrap().contains(category),
            "{reason}: {v}"
        );
        assert_eq!(
            protected_connection_snapshot(&f.state.conn()),
            before,
            "{reason}"
        );
        // A corrupt pin blocks every removal until explicitly repaired; restore this test record.
        if reason == "invalid_pin" {
            f.state
                .conn()
                .execute(
                    "DELETE FROM protocol_sessions WHERE key=?1",
                    [format!("pin-{a}")],
                )
                .unwrap();
        }
    }
}
