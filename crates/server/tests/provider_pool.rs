//! Real loopback HTTP for explicit provider grouping; fixed test credentials only.
use axum::Router;
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokengo_server::{app, db, AppState, Config, SharedState};
struct Fixture {
    state: SharedState,
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
        let dir = std::env::temp_dir().join(format!("provider-pool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            ..Config::default()
        };
        let state = AppState::init(cfg.clone()).unwrap();
        db::sessions::set_panel_password(&state.conn(), "test-password-1234").unwrap();
        let token = db::sessions::create_session(&state.conn(), None).unwrap();
        db::sessions::set_gateway_key(&state.conn(), &state.secrets, "provider-pool-key").unwrap();
        let mut c = state.routing().cfg.clone();
        c.retry_times = 0;
        c.probe_enabled = false;
        db::routing::save_config(&state.conn(), &c).unwrap();
        state.routing().cfg = c;
        let (base, task) = serve(app(state.clone())).await;
        Self {
            state,
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

}

#[tokio::test]
async fn migration_splits_endpoints_preserves_source_identity_and_is_idempotent() {
    let f = Fixture::new().await;
    let provider = f.provider("Supplier").await;
    let a = f.channel("https://EXAMPLE.test:443/v1/").await;
    let b = f.channel("https://example.test/v1").await;
    let d = f.channel("https://example.test/other").await;
    {
        let c = f.state.conn();
        for id in [a, b, d] {
            db::provider_channels::set_membership(&c, id, Some(provider)).unwrap();
        }
        c.execute(
            "UPDATE provider_channels SET enabled=0 WHERE id=?1",
            [provider],
        )
        .unwrap();
    }
    fn snapshots(c: &rusqlite::Connection) -> Vec<(i64, String, i64, i64, i64, i64)> {
        let mut q=c.prepare("SELECT id,base_url,enabled,priority,binding_generation,fault_revision FROM channels ORDER BY id").unwrap();
        q.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }
    let before = snapshots(&f.state.conn());
    {
        let c = f.state.conn();
        db::provider_pool::backfill(&c, &f.state.secrets).unwrap();
        assert_eq!(snapshots(&c), before);
    }
    let list = db::provider_channels::list(&f.state.conn()).unwrap();
    assert_eq!(list["providers"].as_array().unwrap().len(), 2);
    assert!(list["providers"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["enabled"] == false && p["canonical_state"] == "canonical"));
    assert_eq!(
        db::provider_channels::membership(&f.state.conn(), a)
            .unwrap()
            .unwrap()
            .0,
        provider
    );
    assert_eq!(
        db::provider_channels::membership(&f.state.conn(), b)
            .unwrap()
            .unwrap()
            .0,
        provider
    );
    assert_ne!(
        db::provider_channels::membership(&f.state.conn(), d)
            .unwrap()
            .unwrap()
            .0,
        provider
    );
    let ca = db::provider_pool::source_credential(&f.state.conn(), a)
        .unwrap()
        .unwrap();
    let cb = db::provider_pool::source_credential(&f.state.conn(), b)
        .unwrap()
        .unwrap();
    assert_eq!(ca.id, cb.id);
    assert!(!ca.enabled);
    db::provider_pool::backfill(&f.state.conn(), &f.state.secrets).unwrap();
    assert_eq!(snapshots(&f.state.conn()), before);
    assert_eq!(db::provider_channels::list(&f.state.conn()).unwrap(), list);
}

#[tokio::test]
async fn missing_secrets_and_different_auth_headers_never_collapse_credentials() {
    let f = Fixture::new().await;
    let p = f.provider("Headers").await;
    let a = f.channel("https://example.test/v1").await;
    let b = f.channel("https://example.test/v1").await;
    let x = f.channel("https://example.test/v1").await;
    let y = f.channel("https://example.test/v1").await;
    {
        let c = f.state.conn();
        for id in [a, b, x, y] {
            db::provider_channels::set_membership(&c, id, Some(p)).unwrap();
        }
        db::channels::set_headers(
            &c,
            &f.state.secrets,
            tokengo_shared::ChannelId(b),
            &[("x-project".into(), "other-project".into())],
        )
        .unwrap();
        for id in [x, y] {
            db::channels::clear_secret(&c, tokengo_shared::ChannelId(id)).unwrap();
        }
        db::provider_pool::backfill(&c, &f.state.secrets).unwrap();
    }
    let list = db::provider_pool::credentials(&f.state.conn(), p).unwrap();
    assert_eq!(list.len(), 4);
    assert_eq!(list.iter().filter(|k| k.kind == "anonymous").count(), 2);
    let text = serde_json::to_string(&list).unwrap();
    assert!(!text.contains("fixed-upstream-secret"));
    assert!(!text.contains("other-project"));
    assert!(!text.contains("identity_hash"));
}

#[tokio::test]
async fn credential_crud_preserves_sources_and_rotates_generation_only_for_identity_changes() {
    let f = Fixture::new().await;
    let (s,v)=f.req("POST","/api/providers",json!({"name":"Pool","base_url":"https://example.test/v1","key_strategy":"concurrent","switch_on_429":true,"switch_on_400":false})).await;
    assert_eq!(s, 200, "{v}");
    let p = v["provider"]["id"].as_i64().unwrap();
    let path = format!("/api/providers/{p}/credentials");
    let (s,v)=f.req("POST",&path,json!({"label":"First","key":"first-key","models":[{"model":"a","upstream":"real-a","vision":true,"drop_fields":"unused"},{"model":"old-hidden","enabled":false}]})).await;
    assert_eq!(s, 200, "{v}");
    let k = v["credential"]["id"].as_i64().unwrap();
    let source = v["source_id"].as_i64().unwrap();
    let generation = f
        .state
        .conn()
        .query_row(
            "SELECT binding_generation FROM channels WHERE id=?1",
            [source],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    let (s, v) = f
        .req(
            "POST",
            &path,
            json!({"label":"Duplicate","key":"first-key","models":[{"model":"a"}]}),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(db::channels::list(&f.state.conn()).unwrap().len(), 1);
    let patch = format!("{path}/{k}");
    let (s,v)=f.req("PATCH",&patch,json!({"label":"Renamed","models":[{"model":"a","enabled":false},{"model":"b","upstream":""}]})).await;
    assert_eq!(s, 200, "{v}");
    let models = db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(source))
        .unwrap()
        .unwrap()
        .models;
    assert_eq!(models.len(), 3);
    let a = models.iter().find(|m| m.model == "a").unwrap();
    assert!(!a.enabled && a.vision);
    assert_eq!(a.upstream.as_deref(), Some("real-a"));
    assert_eq!(a.drop_fields, vec!["unused"]);
    assert!(models.iter().any(|m| m.model == "old-hidden" && !m.enabled));
    let pre_rotate = f
        .state
        .conn()
        .query_row(
            "SELECT binding_generation FROM channels WHERE id=?1",
            [source],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert!(pre_rotate > generation);
    let (s, v) = f.req("PATCH", &patch, json!({"key":"rotated-key"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["credential"]["revision"], 2);
    assert_eq!(
        db::provider_pool::load_source_secret(&f.state.conn(), &f.state.secrets, source)
            .unwrap()
            .as_deref(),
        Some("rotated-key")
    );
    assert!(
        f.state
            .conn()
            .query_row(
                "SELECT binding_generation FROM channels WHERE id=?1",
                [source],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
            > pre_rotate
    );
    let (s, v) = f.req("DELETE", &patch, json!(null)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["credential"]["enabled"], false);
    assert!(
        db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(source))
            .unwrap()
            .is_some()
    );
    assert!(
        !db::provider_pool::source_credential(&f.state.conn(), source)
            .unwrap()
            .unwrap()
            .enabled
    );
    let (s, v) = f
        .req(
            "PUT",
            &format!("/api/channels/{source}"),
            json!({"name":"bad","base_url":"https://other.test/v1","models":[]}),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["type"], "provider_pool_managed");
}

#[tokio::test]
async fn planning_does_not_set_affinity_and_cas_cannot_overwrite_a_newer_choice() {
    let f = Fixture::new().await;
    let(s,v)=f.req("POST","/api/providers",json!({"name":"Affinity","base_url":"https://example.test/v1","key_strategy":"concurrent"})).await;
    assert_eq!(s, 200, "{v}");
    let p = v["provider"]["id"].as_i64().unwrap();
    let mut sources = vec![];
    let mut keys = vec![];
    for i in 0..2 {
        let(s,v)=f.req("POST",&format!("/api/providers/{p}/credentials"),json!({"label":format!("Key-{i}"),"key":format!("key-{i}"),"models":[{"model":"m"}]})).await;
        assert_eq!(s, 200, "{v}");
        sources.push(v["source_id"].as_i64().unwrap());
        keys.push(v["credential"]["id"].as_i64().unwrap());
    }
    let c = f.state.conn();
    let mut loads = std::collections::HashMap::new();
    loads.insert(keys[0], 3usize);
    loads.insert(keys[1], 0usize);
    let sorted = db::provider_pool::order_for_user_with_load(&c, 1, p, &sources, &loads).unwrap();
    assert_eq!(sorted[0], sources[1]);
    assert!(db::provider_pool::affinity_token(&c, 1, p)
        .unwrap()
        .is_none());
    let token = db::provider_pool::ensure_affinity_for_selection(&c, 1, p, keys[0]).unwrap();
    assert_eq!(
        db::provider_pool::order_for_user_with_load(&c, 1, p, &sources, &loads).unwrap()[0],
        sources[0]
    );
    let _ = db::provider_pool::order_for_user(&c, 1, p, &[sources[1]]).unwrap();
    assert_eq!(
        db::provider_pool::affinity_token(&c, 1, p)
            .unwrap()
            .unwrap(),
        token
    );
    assert!(db::provider_pool::switch_affinity(&c, 1, p, &token, keys[1]).unwrap());
    assert!(!db::provider_pool::switch_affinity(&c, 1, p, &token, keys[0]).unwrap());
    assert_eq!(
        db::provider_pool::affinity_token(&c, 1, p)
            .unwrap()
            .unwrap()
            .credential_id,
        keys[1]
    );
    let old = db::provider_pool::affinity_token(&c, 1, p)
        .unwrap()
        .unwrap();
    c.execute(
        "UPDATE provider_credentials SET enabled=0 WHERE id=?1",
        [keys[1]],
    )
    .unwrap();
    let repaired = db::provider_pool::ensure_affinity_for_selection(&c, 1, p, keys[0]).unwrap();
    assert_eq!(repaired.credential_id, keys[0]);
    assert_eq!(repaired.generation, old.generation + 1);
    assert!(!db::provider_pool::switch_affinity(&c, 1, p, &old, keys[0]).unwrap());
}

#[test]
fn canonical_urls_preserve_base_paths_and_reject_secret_or_query_urls() {
    assert_eq!(
        db::provider_pool::canonical_url(" HTTPS://EXAMPLE.test:443/v1/// ").unwrap(),
        "https://example.test/v1"
    );
    assert_ne!(
        db::provider_pool::canonical_url("https://x.test/v1").unwrap(),
        db::provider_pool::canonical_url("https://x.test/api/v1").unwrap()
    );
    for bad in [
        "https://user:pass@x.test/v1",
        "https://x.test/v1?key=secret",
        "https://x.test/v1#secret",
        "file:///tmp/key",
    ] {
        assert!(db::provider_pool::canonical_url(bad).is_err());
    }
}

#[tokio::test]
async fn first_oauth_credential_has_responses_and_verified_bounds_without_a_template() {
    let f = Fixture::new().await;
    let account = {
        let c = f.state.conn();
        c.execute("INSERT INTO oauth_accounts(label,issuer,subject,client_id,status,scopes_json,expires_at,earliest_refresh_at,nonce,cipher) VALUES('test','issuer','subject','client','active','[]',9999999999999,9999999999999,X'01',X'02')",[]).unwrap();
        c.last_insert_rowid()
    };
    let (s, v) = f
        .req(
            "POST",
            "/api/providers",
            json!({"name":"OAuth","base_url":tokengo_server::oauth::API_BASE}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let p = v["provider"]["id"].as_i64().unwrap();
    let path = format!("/api/providers/{p}/credentials");
    let(s,v)=f.req("POST",&path,json!({"label":"OAuth","kind":"oauth","oauth_account_id":account,"models":[{"model":"gpt"}]})).await;
    assert_eq!(s, 400, "{v}");
    assert!(db::channels::list(&f.state.conn()).unwrap().is_empty());
    let(s,v)=f.req("POST",&path,json!({"label":"OAuth","kind":"oauth","oauth_account_id":account,"max_input_tokens":16000,"max_output_tokens":4000,"models":[{"model":"gpt"}]})).await;
    assert_eq!(s, 200, "{v}");
    let source = v["source_id"].as_i64().unwrap();
    let c = f.state.conn();
    let protocols = tokengo_server::protocol_config::load(&c, source).unwrap();
    assert_eq!(
        protocols.native,
        vec![tokengo_server::protocol::Protocol::Responses]
    );
    assert!(protocols.allow_conversion);
    assert_eq!(
        tokengo_server::oauth::context_bounds(&c, source).unwrap(),
        Some((16000, 4000))
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM billing_ledger", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "metadata/configuration is not inference acceptance"
    );
}

#[tokio::test]
async fn disabling_the_only_credential_removes_provider_routable_model_counts() {
    let f = Fixture::new().await;
    let (s, v) = f
        .req(
            "POST",
            "/api/providers",
            json!({"name":"Count","base_url":"https://example.test/v1"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let p = v["provider"]["id"].as_i64().unwrap();
    let (s, v) = f
        .req(
            "POST",
            &format!("/api/providers/{p}/credentials"),
            json!({"label":"Only","key":"only-key","models":[{"model":"count-model"}]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let key = v["credential"]["id"].as_i64().unwrap();
    assert_eq!(
        db::provider_channels::one(&f.state.conn(), p).unwrap()["models"][0]
            ["routable_connection_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.req(
            "DELETE",
            &format!("/api/providers/{p}/credentials/{key}"),
            json!(null)
        )
        .await
        .0,
        200
    );
    let provider = db::provider_channels::one(&f.state.conn(), p).unwrap();
    assert!(provider["models"][0]["routable_connection_ids"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        provider["models"][0]["declared_connection_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn different_url_assignment_is_conflict_and_atomic() {
    let f = Fixture::new().await;
    let a = f.channel("https://example.test/v1").await;
    let b = f.channel("https://other.test/v1").await;
    let p = f.provider("single URL").await;
    f.assign(a, Some(p)).await;
    let before = db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(b))
        .unwrap()
        .unwrap();
    let (s, v) = f
        .req(
            "PUT",
            &format!("/api/channels/{b}/provider"),
            json!({"provider_id":p}),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["type"], "provider_url_conflict");
    assert_eq!(
        db::channels::get(&f.state.conn(), tokengo_shared::ChannelId(b))
            .unwrap()
            .unwrap(),
        before
    );
    assert!(db::provider_pool::source_credential(&f.state.conn(), b)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn tools_credentials_require_explicit_scope_and_preserve_independent_capabilities() {
    let f = Fixture::new().await;
    let (s, v) = f
        .req(
            "POST",
            "/api/providers",
            json!({"name":"search pool","base_url":"https://search.example.test/v1"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let p = v["provider"]["id"].as_i64().unwrap();
    let path = format!("/api/providers/{p}/credentials");
    let(s,v)=f.req("POST",&path,json!({"label":"bad","key":"tool-key","tools":{"search":true,"account_scope":"","qps":5}})).await;
    assert_eq!(s, 400, "{v}");
    assert!(db::channels::list(&f.state.conn()).unwrap().is_empty());
    for i in 1..=2 {
        let(s,v)=f.req("POST",&path,json!({"label":format!("key{i}"),"key":format!("tool-key-{i}"),"tools":{"search":true,"fetch":false,"account_scope":format!("account-{i}"),"qps":5}})).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(
            v["credential"]["tools"][0]["capabilities"]["account_scope"],
            format!("account-{i}")
        );
    }
    let (s, v) = f
        .req("POST", &path, json!({"label":"not tools","key":"ordinary"}))
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(db::channels::list(&f.state.conn()).unwrap().len(), 2);
    let c = f.state.conn();
    assert_eq!(
        c.query_row(
            "SELECT adapter_kind FROM provider_channels WHERE id=?1",
            [p],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "tools"
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM billing_ledger", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn moving_a_carrier_then_rotating_never_changes_the_old_pool_secret() {
    let f = Fixture::new().await;
    let a = f.channel("https://example.test/v1").await;
    let b = f.channel("https://example.test/v1").await;
    let p = f.provider("original").await;
    f.assign(a, Some(p)).await;
    f.assign(b, Some(p)).await;
    let other = f.provider("moved").await;
    f.assign(a, Some(other)).await;
    let credential = db::provider_pool::source_credential(&f.state.conn(), a)
        .unwrap()
        .unwrap()
        .id;
    let (s, v) = f
        .req(
            "PATCH",
            &format!("/api/providers/{other}/credentials/{credential}"),
            json!({"key":"new-isolated-key"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let c = f.state.conn();
    assert_eq!(
        db::provider_pool::load_source_secret(&c, &f.state.secrets, b)
            .unwrap()
            .as_deref(),
        Some("fixed-upstream-secret")
    );
    assert_eq!(
        db::provider_pool::load_source_secret(&c, &f.state.secrets, a)
            .unwrap()
            .as_deref(),
        Some("new-isolated-key")
    );
}

#[tokio::test]
async fn local_endpoints_preserve_identity_restart_and_isolate_key_rotation() {
    let f=Fixture::new().await;
    let (s,v)=f.req("POST","/api/providers",json!({"name":"My local host","endpoint_mode":"local"})).await;
    assert_eq!(s,200,"{v}");let p=v["provider"]["id"].as_i64().unwrap();
    let path=format!("/api/providers/{p}/credentials");
    let mut keys=Vec::new();let mut sources=Vec::new();
    for (port,model) in [(8101,"local-chat"),(8102,"local-vector")] {
        let (s,v)=f.req("POST",&path,json!({"label":model,"kind":"api_key","key":"same-secret","base_url":format!("http://127.0.0.1:{port}/v1"),"models":[{"model":model}]})).await;
        assert_eq!(s,200,"{v}"); keys.push(v["credential"]["id"].as_i64().unwrap());sources.push(v["source_id"].as_i64().unwrap());
    }
    assert_ne!(keys[0],keys[1]);
    let before=db::provider_channels::one(&f.state.conn(),p).unwrap();
    for _ in 0..2 {db::provider_pool::backfill(&f.state.conn(),&f.state.secrets).unwrap();}
    assert_eq!(before,db::provider_channels::one(&f.state.conn(),p).unwrap());
    assert_eq!(before["base_urls"].as_array().unwrap().len(),2);
    let (s,v)=f.req("PUT",&format!("/api/providers/{p}"),json!({"name":"Renamed local","enabled":true,"endpoint_mode":"local"})).await;assert_eq!(s,200,"{v}");
    let (s,v)=f.req("PUT",&format!("/api/providers/{p}"),json!({"name":"Unsafe rewrite","base_url":"http://127.0.0.1:9999/v1"})).await;assert_eq!(s,400,"{v}");
    let (s,v)=f.req("PUT",&format!("/api/providers/{p}"),json!({"name":"Unsafe single","endpoint_mode":"single"})).await;assert_eq!(s,400,"{v}");
    let (s,v)=f.req("PATCH",&format!("{path}/{}",keys[0]),json!({"base_url":"http://127.0.0.1:8102/v1"})).await;assert_eq!(s,400,"{v}");
    assert_eq!(db::channels::get(&f.state.conn(),tokengo_shared::ChannelId(sources[0])).unwrap().unwrap().base_url,"http://127.0.0.1:8101/v1");
    let before_generation=f.state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[sources[0]],|r|r.get::<_,i64>(0)).unwrap();
    let (s,v)=f.req("PATCH",&format!("{path}/{}",keys[0]),json!({"base_url":"http://127.0.0.1:8201/v1"})).await;assert_eq!(s,200,"{v}");
    assert_eq!(f.state.conn().query_row("SELECT binding_generation FROM channels WHERE id=?1",[sources[0]],|r|r.get::<_,i64>(0)).unwrap(),before_generation+1);
    let (s,v)=f.req("PATCH",&format!("{path}/{}",keys[0]),json!({"key":"rotated"})).await;assert_eq!(s,200,"{v}");
    let c=f.state.conn();
    let a=db::channels::get(&c,tokengo_shared::ChannelId(sources[0])).unwrap().unwrap();
    let b=db::channels::get(&c,tokengo_shared::ChannelId(sources[1])).unwrap().unwrap();
    assert_eq!(a.base_url,"http://127.0.0.1:8201/v1");assert_eq!(b.base_url,"http://127.0.0.1:8102/v1");
    assert_eq!(a.models.len(),1);assert_eq!(b.models.len(),1);assert_eq!(b.models[0].model,"local-vector");
    assert_eq!(db::provider_pool::load_source_secret(&c,&f.state.secrets,b.id.0).unwrap().as_deref(),Some("same-secret"));
    assert_eq!(db::provider_pool::load_source_secret(&c,&f.state.secrets,a.id.0).unwrap().as_deref(),Some("rotated"));
    let generation=|id|c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get::<_,i64>(0)).unwrap();
    assert!(generation(a.id.0)>generation(b.id.0));
    assert_eq!(db::provider_channels::membership(&c,a.id.0).unwrap().unwrap().0,p);
}

#[tokio::test]
async fn local_endpoint_validation_and_single_supplier_guard() {
    let f=Fixture::new().await;
    let (s,v)=f.req("POST","/api/providers",json!({"name":"Local","endpoint_mode":"local"})).await;assert_eq!(s,200,"{v}");let p=v["provider"]["id"].as_i64().unwrap();
    let path=format!("/api/providers/{p}/credentials");
    for body in [json!({"label":"missing-url","kind":"anonymous","models":[{"model":"m"}]}),json!({"label":"invalid","kind":"anonymous","base_url":"http://user:pass@127.0.0.1:8001/v1","models":[{"model":"m"}]}),json!({"label":"oauth","kind":"oauth","base_url":"http://127.0.0.1:8001/v1","models":[{"model":"m"}]})] {
        let(s,v)=f.req("POST",&path,body).await;assert_eq!(s,400,"{v}");
    }
    let a=f.channel("http://127.0.0.1:8101/v1").await;let b=f.channel("http://127.0.0.1:8102/v1").await;
    f.assign(a,Some(p)).await;f.assign(b,Some(p)).await;
    let single=f.provider("Single").await;
    f.assign(a,Some(single)).await;
    let(s,v)=f.req("PUT",&format!("/api/channels/{b}/provider"),json!({"provider_id":single})).await;assert_eq!(s,409,"{v}");
    assert_eq!(db::provider_channels::membership(&f.state.conn(),b).unwrap().unwrap().0,p);
}

#[tokio::test]
async fn local_two_ports_route_only_the_declared_model_and_upstream_mapping() {
    async fn first(Json(v):axum::Json<Value>)->axum::Json<Value>{assert_eq!(v["model"],"port-one-model");axum::Json(json!({"id":"a","object":"chat.completion","model":"one","choices":[{"index":0,"message":{"role":"assistant","content":"port-one"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}))}
    async fn second(Json(v):axum::Json<Value>)->axum::Json<Value>{assert_eq!(v["model"],"port-two-model");axum::Json(json!({"id":"b","object":"chat.completion","model":"two","choices":[{"index":0,"message":{"role":"assistant","content":"port-two"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}))}
    use axum::Json;
    let (one,t1)=serve(Router::new().route("/v1/chat/completions",axum::routing::post(first))).await;
    let (two,t2)=serve(Router::new().route("/v1/chat/completions",axum::routing::post(second))).await;
    let f=Fixture::new().await;
    let(_,v)=f.req("POST","/api/providers",json!({"name":"Local routing","endpoint_mode":"local"})).await;let p=v["provider"]["id"].as_i64().unwrap();
    for (base,public,upstream) in [(one,"one","port-one-model"),(two,"two","port-two-model")] {
        let(s,v)=f.req("POST",&format!("/api/providers/{p}/credentials"),json!({"label":public,"kind":"anonymous","base_url":format!("{base}/v1"),"models":[{"model":public,"upstream":upstream}]})).await;assert_eq!(s,200,"{v}");
    }
    for (model,content) in [("one","port-one"),("two","port-two")] {
        let response=reqwest::Client::new().post(format!("{}/v1/chat/completions",f.base)).bearer_auth("provider-pool-key").json(&json!({"model":model,"messages":[{"role":"user","content":"hi"}]})).send().await.unwrap();
        let status=response.status();let body:Value=response.json().await.unwrap();assert_eq!(status,200,"{body}");assert_eq!(body["choices"][0]["message"]["content"],content);
    }
    t1.abort();t2.abort();
}

#[tokio::test]
async fn schema_37_defaults_existing_suppliers_to_single_without_rewriting_sources() {
    let f=Fixture::new().await;
    let source=f.channel("http://127.0.0.1:8101/v1").await;
    let p=f.provider("Existing supplier").await;f.assign(source,Some(p)).await;
    {
        let c=f.state.conn();
        let before=db::channels::get(&c,tokengo_shared::ChannelId(source)).unwrap().unwrap();
        let credentials=serde_json::to_value(db::provider_pool::credentials(&c,p).unwrap()).unwrap();
        c.execute_batch("ALTER TABLE provider_channels DROP COLUMN endpoint_mode; DELETE FROM schema_migrations WHERE version=37;").unwrap();
        db::migrate(&c).unwrap();db::migrate(&c).unwrap();
        assert!(!db::provider_pool::local(&c,p).unwrap());
        db::provider_pool::backfill(&c,&f.state.secrets).unwrap();
        let after=db::channels::get(&c,tokengo_shared::ChannelId(source)).unwrap().unwrap();
        assert_eq!(before.base_url,after.base_url);assert_eq!(before.id,after.id);
        assert_eq!(credentials,serde_json::to_value(db::provider_pool::credentials(&c,p).unwrap()).unwrap());
    }
    let (s,v)=f.req("PUT",&format!("/api/providers/{p}"),json!({"name":"Now local","endpoint_mode":"local"})).await;
    assert_eq!(s,200,"{v}");assert!(v["provider"]["base_url"].is_null());assert_eq!(v["provider"]["base_urls"],json!(["http://127.0.0.1:8101/v1"]));
    let credentials_before=serde_json::to_value(db::provider_pool::credentials(&f.state.conn(),p).unwrap()).unwrap();
    db::provider_pool::backfill(&f.state.conn(),&f.state.secrets).unwrap();
    assert_eq!(credentials_before,serde_json::to_value(db::provider_pool::credentials(&f.state.conn(),p).unwrap()).unwrap());
    let(s,v)=f.req("PUT",&format!("/api/providers/{p}"),json!({"name":"Back to single","endpoint_mode":"single","base_url":"http://127.0.0.1:8103/v1"})).await;assert_eq!(s,200,"{v}");
    assert_eq!(db::channels::get(&f.state.conn(),tokengo_shared::ChannelId(source)).unwrap().unwrap().base_url,"http://127.0.0.1:8103/v1");
}
