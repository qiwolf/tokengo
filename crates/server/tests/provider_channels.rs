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
    let b = f.channel("http://127.0.0.1:1").await;
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
async fn manual_reasoning_append_and_default_save_atomically_with_legacy_replace() {
    let (up, task) = serve(Router::new().route("/models", axum::routing::get(|| async {
        Json(json!({"data":[{"id":"exact-M","reasoning_effort_levels":["low","high"],"default_reasoning_level":"low"}]}))
    }))).await;
    let f = Fixture::new().await;
    let id = f.channel(&up).await;
    let (status, body) = f.req("POST", &format!("/api/channels/{id}/models"), json!({})).await;
    assert_eq!(status, 200, "{body}");
    let path = format!("/api/channels/{id}/models/reasoning");
    let (status, body) = f.req("PUT", &path,
        json!({"model":"m","manual_mode":"append","manual_levels":["custom"],"default_effort":"custom"})).await;
    assert_eq!(status, 200, "{body}");
    let reasoning = body["reasoning"].clone();
    assert_eq!(reasoning["levels"], json!(["low","high","custom"]));
    assert_eq!(reasoning["upstream_levels"], json!(["low","high"]));
    assert_eq!(reasoning["verified_levels"], json!([]));
    assert_eq!(reasoning["compatibility_levels"], json!([]));
    assert_eq!(reasoning["manual_levels"], json!(["custom"]));
    assert_eq!(reasoning["manual_mode"], "append");
    assert_eq!(reasoning["default_effort"], "custom");
    assert_eq!(reasoning["source"], "user_confirmed");
    let before_generation: i64 = f.state.conn().query_row(
        "SELECT binding_generation FROM channels WHERE id=?1", [id], |r|r.get(0)).unwrap();
    // A new manual list plus an invalid default must not persist either field.
    let (status, _) = f.req("PUT", &path,
        json!({"model":"m","manual_mode":"replace","manual_levels":["another"],"default_effort":"custom"})).await;
    assert_eq!(status, 400);
    let (status, body) = f.req("GET", "/api/channels", json!(null)).await;
    assert_eq!(status, 200);
    let model = body["channels"].as_array().unwrap().iter().find(|row| row["id"] == id).unwrap()["models"]
        .as_array().unwrap().iter().find(|row| row["model"] == "m").unwrap().clone();
    assert_eq!(model["reasoning"], reasoning);
    assert_eq!(f.state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1", [id], |r|r.get::<_,i64>(0)).unwrap(), before_generation);
    // Old clients omit manual_mode and continue replacing, never appending.
    let (status, body) = f.req("PUT", &path,
        json!({"model":"m","manual_levels":["high"],"default_effort":"high"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reasoning"]["levels"], json!(["high"]));
    assert_eq!(body["reasoning"]["manual_mode"], "replace");
    assert_eq!(body["reasoning"]["upstream_levels"], json!(["low","high"]));
    assert_eq!(body["reasoning"]["default_effort"], "high");
    task.abort();
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
    let b = f.channel("http://127.0.0.1:1").await;
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
async fn provider_delete_cascades_config_preserves_history_and_other_provider() {
    let f=Fixture::new().await;
    let p=f.provider("delete with keys").await;
    let q=f.provider("keep").await;
    let a=f.channel("http://127.0.0.1:1").await;
    let b=f.channel("http://127.0.0.1:2").await;
    f.assign(a,Some(p)).await;f.assign(b,Some(q)).await;
    {
        let c=f.state.conn();
        let mut u=db::identity::user(&c,1).unwrap().unwrap();u.policy.allowed_channels=Some(vec![a,b]);db::identity::update_user(&c,&u).unwrap();
        c.execute("INSERT INTO requests(ts_ms,endpoint,model,status,channel_id) VALUES(1,'/v1/chat/completions','m','success',?1)",[a]).unwrap();
        c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,debit_micro,status,created_ms,price_snapshot) VALUES('retained-history',1,1,991,?1,'m',0,0,0,7,'success',1,'snapshot')",[a]).unwrap();
        c.execute("INSERT INTO user_virtual_models(user_id,name,normalized_name,enabled,mode,timezone,default_positions_json,version,created_ms,updated_ms) VALUES(1,'private','private',1,'scheduled','UTC','[1,2]',1,1,1)",[]).unwrap();let vm=c.last_insert_rowid();
        c.execute("INSERT INTO user_virtual_targets VALUES(?1,1,?2,'m',1,0),(?1,2,?3,'m',1,0)",rusqlite::params![vm,p,q]).unwrap();
        c.execute("INSERT INTO user_virtual_windows VALUES(?1,1,127,0,1440,'[2,1]')",[vm]).unwrap();
    }
    let before=db::channels::get(&f.state.conn(),tokengo_shared::ChannelId(b)).unwrap().unwrap();
    let(s,v)=f.req("DELETE",&format!("/api/providers/{p}"),json!(null)).await;assert_eq!(s,200,"{v}");assert_eq!(v["history_preserved"],true);
    let c=f.state.conn();
    assert!(!db::provider_channels::exists(&c,p).unwrap());assert!(db::channels::get(&c,tokengo_shared::ChannelId(a)).unwrap().is_none());
    assert_eq!(db::channels::get(&c,tokengo_shared::ChannelId(b)).unwrap().unwrap(),before);
    assert_eq!(db::identity::user(&c,1).unwrap().unwrap().policy.allowed_channels,Some(vec![b]));
    assert_eq!(c.query_row("SELECT price_snapshot FROM billing_ledger WHERE attempt_id='retained-history'",[],|r|r.get::<_,String>(0)).unwrap(),"snapshot");
    assert_eq!(c.query_row("SELECT channel_id FROM requests",[],|r|r.get::<_,i64>(0)).unwrap(),a);
    assert_eq!(c.query_row("SELECT default_positions_json FROM user_virtual_models",[],|r|r.get::<_,String>(0)).unwrap(),"[1]");
    assert_eq!(c.query_row("SELECT target_positions_json FROM user_virtual_windows",[],|r|r.get::<_,String>(0)).unwrap(),"[1]");
    assert_eq!(c.query_row("SELECT provider_id FROM user_virtual_targets",[],|r|r.get::<_,i64>(0)).unwrap(),q);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    drop(c);
    assert_eq!(f.req("DELETE",&format!("/api/providers/{p}"),json!(null)).await.0,404);
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
async fn connection_delete_cleans_explicit_grants_without_broadening_and_retains_ledger() {
    let f=Fixture::new().await;let a=f.channel("http://127.0.0.1:1").await;
    {
        let c=f.state.conn();let mut u=db::identity::user(&c,1).unwrap().unwrap();u.policy.allowed_channels=Some(vec![a]);u.policy.allowed_models=Some(vec!["m".into()]);db::identity::update_user(&c,&u).unwrap();
        let mut key=db::identity::keys(&c,1).unwrap().remove(0);key.policy.allowed_channels=Some(vec![a]);db::identity::update_key(&c,&key).unwrap();
        db::route_policy::save(&c,"m",&db::route_policy::RoutePolicy{channel_ids:Some(vec![a]),..Default::default()}).unwrap();
        c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,debit_micro,status,created_ms) VALUES('retained-history',1,1,991,?1,'m',0,0,0,0,'success',1)",[a]).unwrap();
    }
    let(s,v)=f.req("DELETE",&format!("/api/channels/{a}"),json!(null)).await;assert_eq!(s,200,"{v}");
    let c=f.state.conn();let u=db::identity::user(&c,1).unwrap().unwrap();assert_eq!(u.policy.allowed_channels,Some(vec![]));assert_eq!(u.policy.allowed_models,Some(vec![]));
    assert_eq!(db::identity::keys(&c,1).unwrap()[0].policy.allowed_channels,Some(vec![]));
    assert_eq!(db::route_policy::load(&c,"m").unwrap().channel_ids,Some(vec![]));
    assert_eq!(c.query_row("SELECT channel_id FROM billing_ledger",[],|r|r.get::<_,i64>(0)).unwrap(),a);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}

fn protected_connection_snapshot(c: &rusqlite::Connection) -> Vec<(String, Vec<Vec<String>>)> {
    [
        "channels",
        "channel_models",
        "channel_secrets",
        "channel_headers",
        "provider_connections",
        "provider_credentials",
        "credential_affinity",
        "billing_prices",
        "billing_request_prices",
        "billing_budgets",
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
async fn deletion_busy_and_invalid_config_roll_back_every_change() {
    for reason in ["running","reserved","invalid_pin"] {
        let f=Fixture::new().await;let p=f.provider("blocked").await;let a=f.channel("http://127.0.0.1:1").await;f.assign(a,Some(p)).await;
        {
            let c=f.state.conn();match reason {
                "running"=>{c.execute("INSERT INTO requests(ts_ms,endpoint,model,status,channel_id) VALUES(1,'/v1/chat/completions','m','running',?1)",[a]).unwrap();},
                "reserved"=>{c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,status,created_ms) VALUES('busy',1,1,991,?1,'m',0,0,5,'reserved',1)",[a]).unwrap();},
                _=>{c.execute("INSERT INTO protocol_sessions VALUES('bad','invalid json',1)",[]).unwrap();}
            }
        }
        let before=protected_connection_snapshot(&f.state.conn());
        for path in [format!("/api/providers/{p}"),format!("/api/channels/{a}")] {
            let(s,v)=f.req("DELETE",&path,json!(null)).await;assert_eq!(s,409,"{reason}: {v}");assert_eq!(protected_connection_snapshot(&f.state.conn()),before);
        }
        assert_eq!(db::provider_pool::credentials(&f.state.conn(),p).unwrap().len(),1);
    }
}

#[tokio::test]
async fn deleting_credential_carrier_keeps_shared_key_and_remaining_mapping() {
    let f=Fixture::new().await;let p=f.provider("shared key").await;
    let a=f.channel("http://127.0.0.1:1").await;let b=f.channel("http://127.0.0.1:1").await;
    f.assign(a,Some(p)).await;f.assign(b,Some(p)).await;
    let key=db::provider_pool::credentials(&f.state.conn(),p).unwrap();assert_eq!(key.len(),1);assert_eq!(key[0].source_ids,vec![a,b]);let key=key[0].id;
    let(s,v)=f.req("DELETE",&format!("/api/channels/{a}"),json!(null)).await;assert_eq!(s,200,"{v}");
    let c=f.state.conn();let keys=db::provider_pool::credentials(&c,p).unwrap();assert_eq!(keys.len(),1);assert_eq!(keys[0].id,key);assert_eq!(keys[0].source_ids,vec![b]);
    assert_eq!(db::channels::secret(&c,&f.state.secrets,tokengo_shared::ChannelId(b)).unwrap().as_deref(),Some("fixed-upstream-secret"));
    assert_eq!(c.query_row("SELECT secret_source_id FROM provider_credentials WHERE id=?1",[key],|r|r.get::<_,i64>(0)).unwrap(),b);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    drop(c);
    let(s,v)=f.req("DELETE",&format!("/api/channels/{b}"),json!(null)).await;assert_eq!(s,200,"{v}");assert!(db::provider_pool::credentials(&f.state.conn(),p).unwrap().is_empty());
}

#[tokio::test]
async fn deleting_scheduled_only_target_disables_alias_without_falling_through_to_default() {
    use tokengo_server::db::user_virtual_routes as vr;
    use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
    let calls=Arc::new(AtomicUsize::new(0));let observed=calls.clone();
    let(up,task)=serve(Router::new().route("/chat/completions",post(move||{let n=observed.clone();async move {n.fetch_add(1,Ordering::SeqCst);Json(json!({"choices":[{"message":{"role":"assistant","content":"B must not receive this"},"finish_reason":"stop"}]}))}}))).await;
    let f=Fixture::new().await;let p=f.provider("A window").await;let q=f.provider("B default").await;
    let a=f.channel("http://127.0.0.1:1").await;let b=f.channel(&up).await;f.assign(a,Some(p)).await;f.assign(b,Some(q)).await;
    let alias={let c=f.state.conn();vr::create(&c,1,&vr::Config{name:"private-scheduled".into(),enabled:true,mode:"scheduled".into(),timezone:"UTC".into(),targets:vec![vr::Target{provider_id:p,logical_model:"m".into(),switch_on_429:true,switch_on_400:false},vr::Target{provider_id:q,logical_model:"m".into(),switch_on_429:true,switch_on_400:false}],windows:vec![vr::Window{days_mask:127,start_minute:0,end_minute:1440,target_positions:vec![1]}],default_positions:vec![2]}).unwrap()};
    assert_eq!(vr::resolve(&f.state.conn(),1,"private-scheduled",0).unwrap().unwrap().targets[0].provider_id,p);
    let(s,v)=f.req("DELETE",&format!("/api/providers/{p}"),json!(null)).await;assert_eq!(s,200,"{v}");
    {
        let c=f.state.conn();let saved=vr::list(&c,1).unwrap();assert_eq!(saved.len(),1);assert!(!saved[0].config.enabled);assert_eq!(saved[0].config.targets[0].provider_id,q);assert_eq!(saved[0].config.default_positions,vec![1]);assert!(saved[0].config.windows[0].target_positions.is_empty());
        assert_eq!(vr::resolve(&c,1,"private-scheduled",0).unwrap_err().code,"virtual_route_forbidden");
        let mut invalid=saved[0].config.clone();invalid.enabled=true;assert!(vr::replace(&c,1,alias.id,saved[0].version,&invalid).is_err(),"must repair empty window before re-enabling");
    }
    assert_eq!(f.chat("private-scheduled").await,StatusCode::FORBIDDEN);
    assert_eq!(calls.load(Ordering::SeqCst),0,"deleting A must not route the former A window to default B");
    // Removing the remaining target also keeps the disabled alias readable for repair.
    let(s,v)=f.req("DELETE",&format!("/api/providers/{q}"),json!(null)).await;assert_eq!(s,200,"{v}");
    let saved=vr::list(&f.state.conn(),1).unwrap();assert!(saved[0].config.targets.is_empty());assert!(!saved[0].config.enabled);
    task.abort();
}

fn managed_header_body(f:&Fixture,id:i64,headers:Value)->Value {
    let c=f.state.conn();let row=db::channels::get(&c,tokengo_shared::ChannelId(id)).unwrap().unwrap();
    json!({"name":row.name,"base_url":row.base_url,"enabled":row.enabled,"max_concurrent":row.max_concurrent,"unwrap_envelope":row.unwrap_envelope,"headers":headers,"models":row.models.iter().map(|m|json!({"model":m.model,"upstream":m.upstream,"enabled":m.enabled,"vision":m.vision,"drop_fields":m.drop_fields_text()})).collect::<Vec<_>>()})
}
fn header_state(f:&Fixture,id:i64)->Vec<(String,String)> {db::channels::headers(&f.state.conn(),&f.state.secrets,tokengo_shared::ChannelId(id)).unwrap()}

#[tokio::test]
async fn managed_non_auth_headers_sync_all_key_sources_and_preserve_models_and_history() {
    let f=Fixture::new().await;let p=f.provider("header managed").await;
    let a=f.channel("http://127.0.0.1:1").await;let b=f.channel("http://127.0.0.1:1").await;f.assign(a,Some(p)).await;f.assign(b,Some(p)).await;
    let key=db::provider_pool::credentials(&f.state.conn(),p).unwrap().remove(0);
    let before_hash:String=f.state.conn().query_row("SELECT identity_hash FROM provider_credentials WHERE id=?1",[key.id],|r|r.get(0)).unwrap();
    let before_models={let c=f.state.conn();[a,b].map(|id|db::channels::get(&c,tokengo_shared::ChannelId(id)).unwrap().unwrap().models)};
    {let c=f.state.conn();c.execute("INSERT INTO requests(ts_ms,endpoint,model,status,channel_id) VALUES(1,'chat','m','ok',?1)",[a]).unwrap();}
    {let c=f.state.conn();
        c.execute("INSERT INTO billing_prices(channel_id,model,input_micro_per_million,output_micro_per_million,max_input_tokens,max_output_tokens,created_ms) VALUES(?1,'m',11,22,128,64,1)",[a]).unwrap();
        c.execute("INSERT INTO billing_request_prices(channel_id,model,micro_per_call,created_ms) VALUES(?1,'m',17,1)",[a]).unwrap();
        c.execute("INSERT INTO billing_budgets(scope,scope_id,limit_micro,spent_micro) VALUES('user',1,1000,7) ON CONFLICT(scope,scope_id) DO UPDATE SET limit_micro=1000,spent_micro=7",[]).unwrap();
        c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,debit_micro,status,created_ms,settled_ms) VALUES('header-history',1,1,(SELECT MAX(id) FROM requests),?1,'m',1,1,0,7,'settled',1,1)",[a]).unwrap();
    }
    let before_history:i64=f.state.conn().query_row("SELECT COUNT(*) FROM requests WHERE channel_id=?1 AND status='ok'",[a],|r|r.get(0)).unwrap();
    let immutable=|snapshot:Vec<(String,Vec<Vec<String>>)>|snapshot.into_iter().filter(|(table,_)|matches!(table.as_str(),"requests"|"attempts"|"billing_ledger"|"billing_prices"|"billing_request_prices"|"billing_budgets"|"users"|"gateway_keys"|"oauth_accounts"|"oauth_channel_bindings")).collect::<Vec<_>>();
    let before_immutable=immutable(protected_connection_snapshot(&f.state.conn()));
    let body=managed_header_body(&f,a,json!([{"name":"X-Trace-Label","value":"tenant-test","action":"replace"}]));
    let (status,response)=f.req("PUT",&format!("/api/channels/{a}"),body).await;assert_eq!(status,200,"{response}");
    assert!(!response.to_string().contains("tenant-test"),"management responses must mask header values");
    for (i,id) in [a,b].into_iter().enumerate(){assert_eq!(header_state(&f,id),vec![("X-Trace-Label".into(),"tenant-test".into())]);let c=f.state.conn();let row=db::channels::get(&c,tokengo_shared::ChannelId(id)).unwrap().unwrap();assert_eq!(row.models,before_models[i]);assert_eq!(db::channels::secret(&c,&f.state.secrets,tokengo_shared::ChannelId(id)).unwrap().as_deref(),Some("fixed-upstream-secret"));}
    let keys=db::provider_pool::credentials(&f.state.conn(),p).unwrap();assert_eq!(keys.len(),1);assert_eq!(keys[0].id,key.id);assert_eq!(keys[0].source_ids,vec![a,b]);assert_eq!(keys[0].revision,key.revision+1);
    let changed_hash:String=f.state.conn().query_row("SELECT identity_hash FROM provider_credentials WHERE id=?1",[key.id],|r|r.get(0)).unwrap();assert_ne!(before_hash,changed_hash);
    assert_eq!(f.state.conn().query_row("SELECT COUNT(*) FROM requests WHERE channel_id=?1 AND status='ok'",[a],|r|r.get::<_,i64>(0)).unwrap(),before_history);
    assert_eq!(immutable(protected_connection_snapshot(&f.state.conn())),before_immutable);
    let body=managed_header_body(&f,b,json!([]));let(status,response)=f.req("PUT",&format!("/api/channels/{b}"),body).await;assert_eq!(status,200,"{response}");
    assert!(header_state(&f,a).is_empty());assert!(header_state(&f,b).is_empty());
    assert_eq!(f.state.conn().query_row("SELECT identity_hash FROM provider_credentials WHERE id=?1",[key.id],|r|r.get::<_,String>(0)).unwrap(),before_hash);
}

#[tokio::test]
async fn managed_non_auth_headers_keep_authentication_and_reject_auth_bypass_atomically() {
    let f=Fixture::new().await;let p=f.provider("auth headers").await;
    let(status,v)=f.req("POST","/api/channels",json!({"name":"auth source","base_url":"http://127.0.0.1:1","key":"fixed-key","headers":[{"name":"Authorization","value":"Bearer fixed-key"}],"models":[{"model":"m"}]})).await;assert_eq!(status,200,"{v}");let a=v["id"].as_i64().unwrap();f.assign(a,Some(p)).await;
    let body=managed_header_body(&f,a,json!([{"name":"Authorization","action":"keep","original_name":"Authorization"},{"name":"X-Project-Id","value":"project-a"}]));let(status,v)=f.req("PUT",&format!("/api/channels/{a}"),body).await;assert_eq!(status,200,"{v}");
    for headers in [json!([]),json!([{"name":"Authorization","value":"Bearer replacement"}]),json!([{"name":"Authorization","action":"keep"},{"name":"X-Api-Key","value":"bypass"}]),json!([{"name":"Authorization","action":"keep"},{"name":"Cookie","value":"session=bypass"}])]{
        let before=protected_connection_snapshot(&f.state.conn());let body=managed_header_body(&f,a,headers);let(status,v)=f.req("PUT",&format!("/api/channels/{a}"),body).await;assert_eq!(status,409,"{v}");assert_eq!(v["error"]["type"],"provider_pool_managed");assert_eq!(protected_connection_snapshot(&f.state.conn()),before);
    }
    for field in ["key","base_url"] {let mut body=managed_header_body(&f,a,json!([{"name":"Authorization","action":"keep"},{"name":"X-Project-Id","action":"keep"}]));body[field]=json!(if field=="key"{"changed-secret"}else{"http://127.0.0.1:2"});let before=protected_connection_snapshot(&f.state.conn());assert_eq!(f.req("PUT",&format!("/api/channels/{a}"),body).await.0,409);assert_eq!(protected_connection_snapshot(&f.state.conn()),before);}
}

#[tokio::test]
async fn managed_non_auth_headers_duplicate_identity_rolls_back_entire_key() {
    let f=Fixture::new().await;let p=f.provider("identity conflict").await;let mut ids=vec![];
    for value in ["one","two"] {let(status,v)=f.req("POST","/api/channels",json!({"name":value,"base_url":"http://127.0.0.1:1","key":"same-key","headers":[{"name":"X-Project-Id","value":value}],"models":[{"model":"m"}]})).await;assert_eq!(status,200,"{v}");let id=v["id"].as_i64().unwrap();f.assign(id,Some(p)).await;ids.push(id);}
    assert_eq!(db::provider_pool::credentials(&f.state.conn(),p).unwrap().len(),2);
    let before=protected_connection_snapshot(&f.state.conn());let body=managed_header_body(&f,ids[0],json!([{"name":"X-Project-Id","value":"two"}]));let(status,v)=f.req("PUT",&format!("/api/channels/{}",ids[0]),body).await;assert_eq!(status,409,"{v}");assert_eq!(protected_connection_snapshot(&f.state.conn()),before);
}

#[tokio::test]
async fn managed_non_auth_headers_preserve_anonymous_and_oauth_authentication_boundaries() {
    let f=Fixture::new().await;
    for issuer in [None,Some("https://auth.openai.com"),Some("https://fixture.invalid")] {
        let oauth=issuer.is_some();
        let p=f.provider(match issuer{None=>"anonymous headers",Some("https://auth.openai.com")=>"oauth headers",_=>"unsupported oauth headers"}).await;
        let(status,v)=f.req("POST","/api/channels",json!({"name":"no secret","base_url":"http://127.0.0.1:1","models":[{"model":"m"}]})).await;assert_eq!(status,200,"{v}");let id=v["id"].as_i64().unwrap();
        if oauth {let c=f.state.conn();c.execute("INSERT INTO oauth_accounts(label,issuer,subject,client_id,status,scopes_json,expires_at,earliest_refresh_at) VALUES('fixture',?1,'subject','client','active','[]',9999999999999,9999999999999)",[issuer.unwrap()]).unwrap();let account=c.last_insert_rowid();c.execute("INSERT INTO oauth_channel_bindings(channel_id,account_id) VALUES(?1,?2)",rusqlite::params![id,account]).unwrap();}
        f.assign(id,Some(p)).await;
        let before=protected_connection_snapshot(&f.state.conn());let body=managed_header_body(&f,id,json!([{"name":"X-Trace-Label","value":"safe-metadata"}]));let(status,v)=f.req("PUT",&format!("/api/channels/{id}"),body).await;
        if issuer==Some("https://auth.openai.com") {assert_eq!(status,200,"{v}");assert_eq!(db::provider_pool::credentials(&f.state.conn(),p).unwrap()[0].kind,"oauth");assert_eq!(f.state.conn().query_row("SELECT COUNT(*) FROM oauth_channel_bindings WHERE channel_id=?1",[id],|r|r.get::<_,i64>(0)).unwrap(),1);}
        else {assert_eq!(status,409,"{v}");assert_eq!(protected_connection_snapshot(&f.state.conn()),before);assert_eq!(db::provider_pool::credentials(&f.state.conn(),p).unwrap()[0].kind,if oauth{"oauth"}else{"anonymous"});}
    }
}

#[tokio::test]
async fn managed_non_auth_headers_reject_cross_provider_credential_ownership() {
    let f=Fixture::new().await;let p=f.provider("original headers owner").await;let q=f.provider("different owner").await;
    let a=f.channel("http://127.0.0.1:1").await;let b=f.channel("http://127.0.0.1:1").await;f.assign(a,Some(p)).await;f.assign(b,Some(p)).await;
    f.state.conn().execute("UPDATE provider_connections SET provider_id=?2 WHERE channel_id=?1",rusqlite::params![b,q]).unwrap();
    let before=protected_connection_snapshot(&f.state.conn());
    for id in [a,b] {let body=managed_header_body(&f,id,json!([{"name":"X-Trace-Label","value":"must-not-write"}]));let(status,v)=f.req("PUT",&format!("/api/channels/{id}"),body).await;assert_eq!(status,409,"{v}");assert_eq!(protected_connection_snapshot(&f.state.conn()),before);}
}

/// Capture every persisted column other than the explicitly mutable display label.
fn presentation_identity(c: &rusqlite::Connection, table: &str, id: i64) -> Vec<String> {
    let mut st=c.prepare(&format!("SELECT * FROM {table} WHERE id=?1")).unwrap();
    let columns=st.column_names().iter().map(|s|s.to_string()).collect::<Vec<_>>();
    st.query_row([id],|r|Ok(columns.iter().enumerate().filter(|(_,n)|n.as_str()!="name").map(|(i,n)|format!("{n}:{:?}",r.get_ref(i).unwrap())).collect())).unwrap()
}
#[tokio::test]
async fn presentation_rename_preserves_all_runtime_identity_and_api_names() {
    let f=Fixture::new().await;
    let channel=f.channel("http://127.0.0.1:1").await;
    let provider=f.provider("cloud").await;
    f.assign(channel,Some(provider)).await;
    let (before_channel,before_provider,before_models,before_sources)={
        let c=f.state.conn();
        (presentation_identity(&c,"channels",channel),presentation_identity(&c,"provider_channels",provider),db::model_routes::list(&c).unwrap(),db::provider_channels::source_json(&c,"m").unwrap())
    };
    let mut before_public=f.models().await;
    for row in before_public["data"].as_array_mut().unwrap() { row.as_object_mut().unwrap().remove("created"); }
    for (path,label) in [(format!("/api/channels/{channel}/presentation"),"云端 · 对话 · M"),(format!("/api/providers/{provider}/presentation"),"供应商 · 云端")] {
        let (s,v)=f.req("PUT",&path,json!({"display_name":label})).await;
        assert_eq!(s,200,"{v}");
    }
    let (status,body)=f.req("PUT","/api/model-presentation?model=m",json!({"display_name":"  对话 · M  "})).await;
    assert_eq!(status,200,"{body}");assert_eq!(body["model"],"m");assert_eq!(body["display_name"],"对话 · M");
    {
        let c=f.state.conn();
        assert_eq!(presentation_identity(&c,"channels",channel),before_channel);
        assert_eq!(presentation_identity(&c,"provider_channels",provider),before_provider);
        assert_eq!(db::model_routes::list(&c).unwrap(),before_models);
        let mut before=before_sources.clone();let mut after=db::provider_channels::source_json(&c,"m").unwrap();
        for row in before.iter_mut().chain(after.iter_mut()) { row.as_object_mut().unwrap().remove("connection_name");row.as_object_mut().unwrap().remove("provider_name"); }
        assert_eq!(before,after);
        assert_eq!(db::model_access::display_name(&c,"m").unwrap(),"对话 · M");
        assert!(!db::model_access::internal_only(&c,"m").unwrap());
    }
    let mut after_public=f.models().await;
    for row in after_public["data"].as_array_mut().unwrap() { row.as_object_mut().unwrap().remove("created"); }
    assert_eq!(after_public,before_public);
    let (_,routes)=f.req("GET","/api/model-routes",Value::Null).await;
    let row=routes["models"].as_array().unwrap().iter().find(|m|m["name"]=="m").unwrap();
    assert_eq!(row["display_name"],"对话 · M");
    let (_,providers)=f.req("GET","/api/providers",Value::Null).await;
    let row=providers["providers"].as_array().unwrap().iter().find(|p|p["id"]==provider).unwrap();
    let m=row["models"].as_array().unwrap().iter().find(|m|m["name"]=="m").unwrap();
    assert_eq!(m["display_name"],"对话 · M");assert!(m["kind_label"].is_string());
    assert_eq!(f.req("PUT","/api/model-presentation?model=m",json!({"display_name":" "})).await.0,200);
    assert_eq!(db::model_access::display_name(&f.state.conn(),"m").unwrap(),"");
}
#[tokio::test]
async fn presentation_endpoints_reject_invalid_missing_unprivileged_and_csrf_requests() {
    let f=Fixture::new().await;
    let channel=f.channel("http://127.0.0.1:1").await;let p=f.provider("one").await;let q=f.provider("two").await;
    assert_eq!(f.req("PUT",&format!("/api/providers/{p}/presentation"),json!({"display_name":" TWO "})).await.0,400);
    for path in ["/api/model-presentation?model=m".into(),format!("/api/channels/{channel}/presentation"),format!("/api/providers/{q}/presentation")] {
        for label in ["bad\nname".into(),"x".repeat(129)] {
            assert_eq!(f.req("PUT",&path,json!({"display_name":label})).await.0,400);
        }
        let r=reqwest::Client::new().put(format!("{}{path}",f.base)).bearer_auth("provider-test-key").json(&json!({"display_name":"denied"})).send().await.unwrap();
        assert_eq!(r.status(),401);
        let r=reqwest::Client::new().put(format!("{}{path}",f.base)).header("cookie",format!("tokengo_session={}",f.token)).json(&json!({"display_name":"csrf denied"})).send().await.unwrap();
        assert_eq!(r.status(),403);
    }
    for path in ["/api/model-presentation?model=missing","/api/channels/999999/presentation","/api/providers/999999/presentation"] {
        assert_eq!(f.req("PUT",path,json!({"display_name":"missing"})).await.0,404);
    }
    // Maintenance tokens do not acquire newly introduced panel capabilities implicitly.
    let secret={let c=f.state.conn();let root=db::accounts::root(&c).unwrap().unwrap();db::operations_tokens::create(&c,"presentation-denied",1,root.account.id).unwrap().secret};
    for path in ["/api/model-presentation?model=m".into(),format!("/api/channels/{channel}/presentation"),format!("/api/providers/{q}/presentation")] {
        let response=reqwest::Client::new().put(format!("{}{path}",f.base)).bearer_auth(&secret).json(&json!({"display_name":"denied"})).send().await.unwrap();
        assert_eq!(response.status(),403);
    }
}

#[tokio::test]
async fn presentation_changes_roll_back_if_audit_storage_fails() {
    let f=Fixture::new().await;
    let channel=f.channel("http://127.0.0.1:1").await;
    let provider=f.provider("original-provider").await;
    f.state.conn().execute_batch("CREATE TRIGGER reject_presentation_audit BEFORE INSERT ON audit_events WHEN NEW.action IN ('presentation.update','model_presentation.update') BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;").unwrap();
    for path in ["/api/model-presentation?model=m".into(),format!("/api/channels/{channel}/presentation"),format!("/api/providers/{provider}/presentation")] {
        let body=if path.starts_with("/api/model-presentation") {json!({"display_name":"must rollback","purpose":"audio"})}else{json!({"display_name":"must rollback"})};
        assert_eq!(f.req("PUT",&path,body).await.0,500);
    }
    let c=f.state.conn();
    assert_eq!(db::model_access::display_name(&c,"m").unwrap(),"");
    assert_eq!(db::model_access::catalog_purpose(&c,"m").unwrap(),"");
    assert_eq!(c.query_row("SELECT name FROM channels WHERE id=?1",[channel],|r|r.get::<_,String>(0)).unwrap(),"connection");
    assert_eq!(c.query_row("SELECT name FROM provider_channels WHERE id=?1",[provider],|r|r.get::<_,String>(0)).unwrap(),"original-provider");
}

#[tokio::test]
async fn catalog_purpose_is_display_only_and_optional_updates_preserve_or_clear_it() {
    let f=Fixture::new().await;
    let channel=f.channel("http://127.0.0.1:1").await;
    let provider=f.provider("purpose-provider").await;f.assign(channel,Some(provider)).await;
    db::kinds::set(&f.state.conn(),"m",tokengo_shared::ModelKind::Chat).unwrap();
    db::model_access::set_internal_only(&f.state.conn(),"m",true).unwrap();
    let before_routes=db::model_routes::list(&f.state.conn()).unwrap();
    let before_kinds=db::kinds::all(&f.state.conn()).unwrap();
    let settings=|| db::settings::all(&f.state.conn()).unwrap().into_iter().filter(|(k,_)|!k.starts_with("model_catalog_purpose_v1:")&&!k.starts_with("model_display_name_v1:")).collect::<Vec<_>>();
    let before_settings=settings();
    for (purpose,label) in [("audio","音频"),("tools","工具")] {
        let (s,body)=f.req("PUT","/api/model-presentation?model=m",json!({"display_name":"展示名","purpose":purpose})).await;
        assert_eq!(s,200,"{body}");assert_eq!(body["catalog_purpose"],purpose);assert_eq!(body["catalog_purpose_label"],label);
        assert_eq!(db::model_routes::list(&f.state.conn()).unwrap(),before_routes);
        assert_eq!(db::kinds::all(&f.state.conn()).unwrap(),before_kinds);
        assert_eq!(settings(),before_settings);
        let (_,routes)=f.req("GET","/api/model-routes",Value::Null).await;
        let m=routes["models"].as_array().unwrap().iter().find(|m|m["name"]=="m").unwrap();
        assert_eq!(m["kind"],"chat");assert_eq!(m["catalog_purpose"],purpose);assert_eq!(m["catalog_purpose_label"],label);
        let (_,providers)=f.req("GET","/api/providers",Value::Null).await;
        let p=providers["providers"].as_array().unwrap().iter().find(|p|p["id"]==provider).unwrap();
        let m=p["models"].as_array().unwrap().iter().find(|m|m["name"]=="m").unwrap();
        assert_eq!(m["kind"],"chat");assert_eq!(m["catalog_purpose"],purpose);
    }
    assert_eq!(f.req("PUT","/api/model-presentation?model=m",json!({"display_name":"改名保留用途"})).await.0,200);
    assert_eq!(db::model_access::catalog_purpose(&f.state.conn(),"m").unwrap(),"tools");
    for invalid in ["speech","TOOLS"," audio ","unknown"] {
        assert_eq!(f.req("PUT","/api/model-presentation?model=m",json!({"display_name":"不应保存","purpose":invalid})).await.0,400);
        assert_eq!(db::model_access::display_name(&f.state.conn(),"m").unwrap(),"改名保留用途");
        assert_eq!(db::model_access::catalog_purpose(&f.state.conn(),"m").unwrap(),"tools");
    }
    let (s,body)=f.req("PUT","/api/model-presentation?model=m",json!({"display_name":"展示名","purpose":""})).await;
    assert_eq!(s,200);assert_eq!(body["catalog_purpose"],"");assert_eq!(body["catalog_purpose_label"],"");
    assert_eq!(db::model_routes::list(&f.state.conn()).unwrap(),before_routes);
    assert_eq!(db::kinds::all(&f.state.conn()).unwrap(),before_kinds);assert_eq!(settings(),before_settings);
}

#[tokio::test]
async fn new_unknown_model_catalog_purpose_round_trips_without_changing_runtime_contract() {
    let f = Fixture::new().await;
    let name = "gpt-6.1-terra";
    let display_name = "GPT 6.1 Terra · 工作模型";
    let (status, created) = f.req("POST", "/api/channels", json!({
        "name":"new subscription model", "base_url":"http://127.0.0.1:1",
        "key":"fixed-terra-test-secret", "models":[{"model":name,"upstream":name}]
    })).await;
    assert_eq!(status, 200, "{created}");
    let channel = created["id"].as_i64().unwrap();
    let provider = f.provider("Terra purpose fixture").await;
    f.assign(channel, Some(provider)).await;
    {
        let c = f.state.conn();
        tokengo_server::protocol_config::save(&c, channel, &tokengo_server::protocol_config::Capabilities {
            native: vec![tokengo_server::protocol::Protocol::Responses],
            allow_conversion: true,
            ..Default::default()
        }).unwrap();
        db::route_policy::save(&c, name, &db::route_policy::RoutePolicy {
            channel_ids: Some(vec![channel]), retry_times: Some(0), include_in_auto: Some(false),
            ..Default::default()
        }).unwrap();
        let mut user = db::identity::user(&c, 1).unwrap().unwrap();
        user.policy.allowed_models = Some(vec![name.into()]);
        user.policy.allowed_channels = Some(vec![channel]);
        db::identity::update_user(&c, &user).unwrap();
        let mut key = db::identity::keys(&c, 1).unwrap().remove(0);
        key.policy.allowed_models = Some(vec![name.into()]);
        key.policy.allowed_channels = Some(vec![channel]);
        db::identity::update_key(&c, &key).unwrap();
        db::model_access::set_display_name(&c, name, display_name).unwrap();
        assert!(db::kinds::get(&c, name).unwrap().is_none(), "new model has no learned or manual kind");
        assert_eq!(db::model_access::catalog_purpose(&c, name).unwrap(), "");
        let route = db::model_routes::routes_for(&c, name).unwrap().unwrap();
        assert_eq!(route.kind, tokengo_shared::ModelKind::Unknown);
        assert_eq!(route.kind_source, tokengo_shared::KindSource::Unknown);
    }
    let protected = || {
        protected_connection_snapshot(&f.state.conn()).into_iter()
            .filter(|(table, _)| table != "settings").collect::<Vec<_>>()
    };
    let settings = || db::settings::all(&f.state.conn()).unwrap().into_iter()
        .filter(|(key, _)| !key.starts_with("model_catalog_purpose_v1:")).collect::<Vec<_>>();
    let before_protected = protected();
    let before_settings = settings();
    let before_routes = db::model_routes::list(&f.state.conn()).unwrap();
    let before_kinds = db::kinds::all(&f.state.conn()).unwrap();
    let mut before_public = f.models().await;
    for model in before_public["data"].as_array_mut().unwrap() { model.as_object_mut().unwrap().remove("created"); }
    for purpose in ["chat", ""] {
        let (status, saved) = f.req("PUT", "/api/model-presentation?model=gpt-6.1-terra",
            json!({"display_name":display_name,"purpose":purpose})).await;
        assert_eq!(status, 200, "{saved}");
        assert_eq!(saved["model"], name);
        assert_eq!(saved["display_name"], display_name);
        assert_eq!(saved["catalog_purpose"], purpose);
        assert_eq!(saved["catalog_purpose_label"], if purpose == "chat" { "对话" } else { "" });
        let (status, routes) = f.req("GET", "/api/model-routes", Value::Null).await;
        assert_eq!(status, 200, "{routes}");
        let model = routes["models"].as_array().unwrap().iter().find(|model| model["name"] == name).unwrap();
        assert_eq!(model["name"], name);
        assert_eq!(model["display_name"], display_name);
        assert_eq!(model["kind"], "");
        assert_eq!(model["kind_label"], "还没定");
        assert_eq!(model["catalog_purpose"], purpose);
        assert_eq!(model["catalog_purpose_label"], if purpose == "chat" { "对话" } else { "" });
        let (status, providers) = f.req("GET", "/api/providers", Value::Null).await;
        assert_eq!(status, 200, "{providers}");
        let row = providers["providers"].as_array().unwrap().iter().find(|row| row["id"] == provider).unwrap();
        let model = row["models"].as_array().unwrap().iter().find(|model| model["name"] == name).unwrap();
        assert_eq!(model["display_name"], display_name);
        assert_eq!(model["kind"], "");
        assert_eq!(model["kind_label"], "还没定");
        assert_eq!(model["catalog_purpose"], purpose);
        assert_eq!(model["catalog_purpose_label"], if purpose == "chat" { "对话" } else { "" });
        let c = f.state.conn();
        assert!(db::kinds::get(&c, name).unwrap().is_none());
        assert_eq!(db::kinds::all(&c).unwrap(), before_kinds);
        assert_eq!(db::model_routes::list(&c).unwrap(), before_routes);
        assert_eq!(db::model_access::display_name(&c, name).unwrap(), display_name);
        drop(c);
        assert_eq!(protected(), before_protected, "presentation cannot change models, keys, source bindings or authorization");
        assert_eq!(settings(), before_settings, "route allowlist and protocol declarations remain unchanged");
        let mut public = f.models().await;
        for model in public["data"].as_array_mut().unwrap() { model.as_object_mut().unwrap().remove("created"); }
        assert_eq!(public, before_public, "API model identifiers and executable capabilities do not follow directory classification");
    }
}
