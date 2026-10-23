use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use tokengo_server::{
    app,
    db::{self, user_virtual_routes as vr},
    AppState, Config, SharedState,
};
use tower::ServiceExt;
fn cfg(name: &str, p: i64) -> vr::Config {
    vr::Config {
        name: name.into(),
        enabled: true,
        mode: "ordered".into(),
        timezone: "Asia/Shanghai".into(),
        targets: vec![vr::Target {
            provider_id: p,
            logical_model: "real-model".into(),
            switch_on_429: true,
            switch_on_400: false,
        }],
        windows: vec![],
        default_positions: vec![],
    }
}
struct Fixture {
    state: SharedState,
    u: i64,
    other: i64,
    provider: i64,
    source: i64,
    key: i64,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("virtual-routes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into(),
            ..Config::default()
        })
        .unwrap();
        let (u, other, provider, source, key) = {
            let c = state.conn();
            let u = db::identity::create_user(&c, "Alice", "user", &Default::default())
                .unwrap()
                .id;
            let other = db::identity::create_user(&c, "Bob", "user", &Default::default())
                .unwrap()
                .id;
            let source = db::channels::create(
                &c,
                &db::channels::NewChannel {
                    name: "provider source".into(),
                    base_url: "https://provider.example/v1".into(),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 2,
                },
            )
            .unwrap()
            .0;
            db::channels::set_models(
                &c,
                tokengo_shared::ChannelId(source),
                &[db::channels::ModelMapping::new("real-model", None, false)],
            )
            .unwrap();
            c.execute("INSERT INTO provider_channels(name,enabled,created_ms) VALUES('test provider',1,0)",[]).unwrap();
            let provider = c.last_insert_rowid();
            db::provider_channels::set_membership(&c, source, Some(provider)).unwrap();
            db::provider_pool::adopt_source(&c, &state.secrets, source, provider).unwrap();
            let (key, _) =
                db::identity::create_key(&c, u, "client", &Default::default(), None, None).unwrap();
            (u, other, provider, source, key.id)
        };
        Self {
            state,
            u,
            other,
            provider,
            source,
            key,
        }
    }
    async fn req(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        csrf: bool,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut r = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json");
        if let Some(v) = cookie {
            r = r.header("cookie", v);
        }
        if csrf {
            r = r.header("X-Tokengo-Panel", "1");
        }
        let out = app(self.state.clone())
            .oneshot(r.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = out.status();
        let bytes = axum::body::to_bytes(out.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    fn cookie(&self, user: i64, name: &str) -> String {
        let c = self.state.conn();
        let hash = db::sessions::hash_password("virtual-route-test-password").unwrap();
        let account = db::accounts::create(&c, name, name, user, &hash).unwrap();
        let credential = db::accounts::by_id(&c, account.id).unwrap().unwrap();
        let token = db::accounts::create_session(&c, &credential).unwrap();
        format!("tokengo_account_session={token}")
    }
}
#[test]
fn private_names_owner_acl_version_and_soft_delete_are_enforced() {
    let f = Fixture::new();
    let c = f.state.conn();
    let a = vr::create(&c, f.u, &cfg("my-private", f.provider)).unwrap();
    let b = vr::create(&c, f.other, &cfg("my-private", f.provider)).unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(
        vr::resolve(&c, f.u, "my-private", 0).unwrap().unwrap().id,
        a.id
    );
    assert_eq!(
        vr::replace(&c, f.other, a.id, 1, &cfg("another", f.provider))
            .unwrap_err()
            .code,
        "virtual_route_not_found"
    );
    let mut changed = cfg("my-private", f.provider);
    changed.targets[0].switch_on_400 = true;
    let updated = vr::replace(&c, f.u, a.id, 1, &changed).unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(
        vr::replace(&c, f.u, a.id, 1, &changed).unwrap_err().code,
        "virtual_route_changed"
    );
    assert_eq!(
        vr::validate_current(&c, f.u, a.id, 1, 1, 0)
            .unwrap_err()
            .code,
        "virtual_route_changed"
    );
    vr::disable(&c, f.u, a.id, 2).unwrap();
    assert_eq!(vr::list(&c, f.u).unwrap().len(), 1);
    assert!(vr::resolve(&c, f.u, "my-private", 0).is_err());
}
#[test]
fn aliases_never_authorize_real_models_or_sources() {
    let f = Fixture::new();
    let c = f.state.conn();
    let a = vr::create(&c, f.u, &cfg("mine", f.provider)).unwrap();
    let auth = db::identity::context(&c, f.key).unwrap().unwrap();
    assert_eq!(vr::alias_names(&c, &auth, 0).unwrap(), vec!["mine"]);
    let mut key = db::identity::key(&c, f.key).unwrap().unwrap();
    key.policy.allowed_models = Some(vec!["mine".into()]);
    db::identity::update_key(&c, &key).unwrap();
    assert!(vr::alias_names(&c, &auth, 0).unwrap().is_empty());
    let mut u = db::identity::user(&c, f.u).unwrap().unwrap();
    u.policy.allowed_channels = Some(vec![]);
    db::identity::update_user(&c, &u).unwrap();
    assert_eq!(
        vr::validate_current(&c, f.u, a.id, 1, 1, 0)
            .unwrap_err()
            .code,
        "virtual_target_forbidden"
    );
    assert!(vr::create(&c, f.u, &cfg("denied", f.provider)).is_err());
    assert!(db::channels::get(&c, tokengo_shared::ChannelId(f.source))
        .unwrap()
        .is_some());
}
#[test]
fn time_windows_are_half_open_weekly_and_timezone_explicit() {
    let mut v = cfg("schedule", 1);
    v.mode = "scheduled".into();
    v.timezone = "UTC".into();
    v.windows = vec![vr::Window {
        days_mask: 1,
        start_minute: 60,
        end_minute: 120,
        target_positions: vec![1],
    }];
    let monday = 4 * 86_400_000;
    assert!(vr::time_positions(&v, monday + 59 * 60_000).is_err());
    assert_eq!(
        vr::time_positions(&v, monday + 60 * 60_000).unwrap(),
        vec![1]
    );
    assert!(vr::time_positions(&v, monday + 120 * 60_000).is_err());
    v.timezone = "Asia/Shanghai".into();
    assert_eq!(
        vr::time_positions(&v, monday - 7 * 3_600_000).unwrap(),
        vec![1]
    );
    v.default_positions = vec![1];
    assert_eq!(vr::time_positions(&v, 0).unwrap(), vec![1]);
    v.windows.push(v.windows[0].clone());
    assert!(vr::validate_shape(&v).is_err());
    v.windows.pop();
    v.windows[0].start_minute = 1400;
    v.windows[0].end_minute = 60;
    assert!(vr::validate_shape(&v).is_err());
    v.timezone = "Europe/London".into();
    assert!(vr::validate_shape(&v).is_err());
}
#[test]
fn recursion_auto_global_names_and_malformed_shapes_are_rejected() {
    let f = Fixture::new();
    let c = f.state.conn();
    for n in ["auto", "real-model"] {
        assert!(vr::create(&c, f.u, &cfg(n, f.provider)).is_err());
    }
    vr::create(&c, f.u, &cfg("first-private", f.provider)).unwrap();
    let mut v = cfg("second-private", f.provider);
    v.targets[0].logical_model = "first-private".into();
    assert!(vr::create(&c, f.u, &v).is_err());
    v.targets[0].logical_model = "auto".into();
    assert!(vr::create(&c, f.u, &v).is_err());
    v = cfg("empty", f.provider);
    v.targets.clear();
    assert!(vr::validate_shape(&v).is_err());
}
#[tokio::test]
async fn browser_api_is_owner_scoped_csrf_typed_and_versioned() {
    let f = Fixture::new();
    let cookie = f.cookie(f.u, "alice");
    let other = f.cookie(f.other, "bob");
    let body = serde_json::to_value(cfg("mine", f.provider)).unwrap();
    assert_eq!(
        f.req(
            "GET",
            "/api/account/virtual-models",
            None,
            false,
            Value::Null
        )
        .await
        .0,
        401
    );
    assert_eq!(
        f.req(
            "POST",
            "/api/account/virtual-models",
            Some(&cookie),
            false,
            body.clone()
        )
        .await
        .0,
        403
    );
    let (s, v) = f
        .req(
            "POST",
            "/api/account/virtual-models",
            Some(&cookie),
            true,
            body.clone(),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let id = v["virtual_model"]["id"].as_i64().unwrap();
    let path = format!("/api/account/virtual-models/{id}");
    let mut edit = body.clone();
    edit["version"] = json!(1);
    assert_eq!(
        f.req("PATCH", &path, Some(&other), true, edit.clone())
            .await
            .0,
        404
    );
    let (s, v) = f
        .req("PATCH", &path, Some(&cookie), true, edit.clone())
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        f.req("PATCH", &path, Some(&cookie), true, edit).await.0,
        409
    );
    assert_eq!(
        f.req("DELETE", &path, Some(&other), true, json!({"version":2}))
            .await
            .0,
        404
    );
    assert_eq!(
        f.req("DELETE", &path, Some(&cookie), true, json!({"version":2}))
            .await
            .0,
        200
    );
    let (s, v) = f
        .req(
            "GET",
            "/api/account/virtual-models",
            Some(&cookie),
            false,
            Value::Null,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["virtual_models"][0]["enabled"], false);
    assert!(!v.to_string().contains("https://provider.example"));
    let mut spoof = body;
    spoof["user_id"] = json!(f.other);
    assert_eq!(
        f.req(
            "POST",
            "/api/account/virtual-models",
            Some(&cookie),
            true,
            spoof
        )
        .await
        .0,
        400
    );
}

#[test]
fn live_time_credential_and_user_revocation_stop_new_attempts() {
    let f = Fixture::new();
    let c = f.state.conn();
    let mut v = cfg("clock-route", f.provider);
    v.mode = "scheduled".into();
    v.timezone = "UTC".into();
    v.windows = vec![vr::Window {
        days_mask: 8,
        start_minute: 0,
        end_minute: 1,
        target_positions: vec![1],
    }];
    let route = vr::create(&c, f.u, &v).unwrap();
    assert!(vr::validate_current(&c, f.u, route.id, 1, 1, 0).is_ok());
    assert_eq!(
        vr::validate_current(&c, f.u, route.id, 1, 1, 60_000)
            .unwrap_err()
            .code,
        "route_time_unavailable"
    );
    let auth = db::identity::context(&c, f.key).unwrap().unwrap();
    c.execute(
        "UPDATE provider_credentials SET enabled=0 WHERE provider_id=?1",
        [f.provider],
    )
    .unwrap();
    assert!(vr::alias_names(&c, &auth, 0).unwrap().is_empty());
    assert_eq!(
        vr::validate_current(&c, f.u, route.id, 1, 1, 0)
            .unwrap_err()
            .code,
        "virtual_target_forbidden"
    );
    let mut user = db::identity::user(&c, f.u).unwrap().unwrap();
    user.enabled = false;
    db::identity::update_user(&c, &user).unwrap();
    assert!(vr::resolve(&c, f.u, "clock-route", 0).is_err());
    assert!(vr::alias_names(&c, &auth, 0).is_err());
}
#[test]
fn same_version_concurrent_edits_have_one_winner() {
    let f = Fixture::new();
    let route = vr::create(&f.state.conn(), f.u, &cfg("cas", f.provider)).unwrap();
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut joins = Vec::new();
    for name in ["cas-first", "cas-second"] {
        let state = f.state.clone();
        let gate = gate.clone();
        let user = f.u;
        let provider = f.provider;
        let id = route.id;
        joins.push(std::thread::spawn(move || {
            gate.wait();
            vr::replace(&state.conn(), user, id, 1, &cfg(name, provider))
                .map(|r| r.version)
                .map_err(|e| e.code)
        }));
    }
    let results = joins
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err("virtual_route_changed")))
            .count(),
        1
    );
}
#[test]
fn later_standard_name_collision_fails_closed_and_unknown_kind_cannot_mix() {
    let f = Fixture::new();
    let c = f.state.conn();
    vr::create(&c, f.u, &cfg("private-first", f.provider)).unwrap();
    db::channels::set_models(
        &c,
        tokengo_shared::ChannelId(f.source),
        &[
            db::channels::ModelMapping::new("real-model", None, false),
            db::channels::ModelMapping::new("private-first", None, false),
            db::channels::ModelMapping::new("another-real", None, false),
        ],
    )
    .unwrap();
    assert_eq!(
        vr::resolve(&c, f.u, "private-first", 0).unwrap_err().code,
        "virtual_route_changed"
    );
    let auth = db::identity::context(&c, f.key).unwrap().unwrap();
    assert!(vr::alias_names(&c, &auth, 0).unwrap().is_empty());

    let mut v = cfg("mixed", f.provider);
    v.targets.push(v.targets[0].clone());
    v.targets[1].logical_model = "another-real".into();
    assert!(vr::create(&c, f.u, &v).is_err());
}

#[test]
fn explicit_order_and_window_default_order_are_preserved() {
    let f = Fixture::new();
    let c = f.state.conn();
    let source = db::channels::create(
        &c,
        &db::channels::NewChannel {
            name: "second".into(),
            base_url: "https://second.example/v1".into(),
            enabled: true,
            unwrap_envelope: false,
            max_concurrent: 1,
        },
    )
    .unwrap();
    db::channels::set_models(
        &c,
        source,
        &[db::channels::ModelMapping::new("real-model", None, false)],
    )
    .unwrap();
    c.execute(
        "INSERT INTO provider_channels(name,enabled,created_ms) VALUES('second provider',1,0)",
        [],
    )
    .unwrap();
    let second = c.last_insert_rowid();
    db::provider_channels::set_membership(&c, source.0, Some(second)).unwrap();
    db::provider_pool::adopt_source(&c, &f.state.secrets, source.0, second).unwrap();
    let mut config = cfg("chain", f.provider);
    let mut target = config.targets[0].clone();
    target.provider_id = second;
    target.switch_on_400 = true;
    config.targets.push(target);
    vr::create(&c, f.u, &config).unwrap();
    let p = vr::resolve(&c, f.u, "chain", 0).unwrap().unwrap();
    assert_eq!(
        p.targets.iter().map(|t| t.provider_id).collect::<Vec<_>>(),
        vec![f.provider, second]
    );
    assert!(!p.targets[0].switch_on_400);
    assert!(p.targets[1].switch_on_400);
    config.name = "scheduled-chain".into();
    config.mode = "scheduled".into();
    config.default_positions = vec![2, 1];
    vr::create(&c, f.u, &config).unwrap();
    assert_eq!(
        vr::resolve(&c, f.u, "scheduled-chain", 0)
            .unwrap()
            .unwrap()
            .targets
            .iter()
            .map(|t| t.provider_id)
            .collect::<Vec<_>>(),
        vec![second, f.provider]
    );
}

#[test]
fn paid_permission_follows_real_target_price_not_the_alias() {
    let f = Fixture::new();
    let c = f.state.conn();
    let mut user = db::identity::user(&c, f.u).unwrap().unwrap();
    user.policy.allow_paid = false;
    db::identity::update_user(&c, &user).unwrap();
    assert_eq!(
        vr::create(&c, f.u, &cfg("free-only", f.provider))
            .unwrap_err()
            .code,
        "virtual_target_forbidden"
    );
    let mut price = db::billing::Price {
        id: 0,
        channel_id: f.source,
        model: "real-model".into(),
        input_micro_per_million: 0,
        output_micro_per_million: 0,
        max_input_tokens: 32,
        max_output_tokens: 16,
    };
    db::billing::create_price(&c, &price, "root").unwrap();
    vr::create(&c, f.u, &cfg("free-only", f.provider)).unwrap();
    let auth = db::identity::context(&c, f.key).unwrap().unwrap();
    assert_eq!(vr::alias_names(&c, &auth, 0).unwrap(), vec!["free-only"]);
    price.input_micro_per_million = 1000;
    db::billing::create_price(&c, &price, "root").unwrap();
    assert!(vr::alias_names(&c, &auth, 0).unwrap().is_empty());
}


fn unrelated_corrupt_private_row(c: &rusqlite::Connection, user: i64, template: i64) {
    c.execute(
        "INSERT INTO user_virtual_models(user_id,name,normalized_name,enabled,mode,timezone,default_positions_json,version,created_ms,updated_ms) SELECT ?1,'unrelated-broken','unrelated-broken',enabled,mode,timezone,'{invalid-json',version,created_ms,updated_ms FROM user_virtual_models WHERE id=?2",
        rusqlite::params![user,template],
    ).unwrap();
}

#[test]
fn private_request_lookup_reads_only_selected_owned_route_and_rechecks_authorization() {
    let f = Fixture::new();
    let c = f.state.conn();
    let route = vr::create(&c, f.u, &cfg("point-private", f.provider)).unwrap();
    let foreign = vr::create(&c, f.other, &cfg("foreign-private", f.provider)).unwrap();
    unrelated_corrupt_private_row(&c, f.u, route.id);
    assert!(vr::list(&c, f.u).is_err(), "fixture proves a directory hydration would fail");
    assert_eq!(vr::resolve(&c, f.u, " POINT-PRIVATE ", 0).unwrap().unwrap().id, route.id);
    assert!(vr::resolve(&c, f.u, "real-model", 0).unwrap().is_none(), "ordinary endpoints must not hydrate private siblings");
    assert!(vr::resolve(&c, f.u, "foreign-private", 0).unwrap().is_none());
    assert!(vr::get(&c, f.u, foreign.id).unwrap().is_none());
    assert_eq!(vr::get(&c, f.u, route.id).unwrap().unwrap().id, route.id);
    vr::validate_current(&c, f.u, route.id, route.version, 1, 0).unwrap();
    let mut user = db::identity::user(&c, f.u).unwrap().unwrap();
    user.policy.allowed_models = Some(vec![]);
    db::identity::update_user(&c, &user).unwrap();
    assert_eq!(vr::validate_current(&c, f.u, route.id, route.version, 1, 0).unwrap_err().code, "virtual_target_forbidden", "point lookup must not cache away live revocation");
    user.enabled = false;
    db::identity::update_user(&c, &user).unwrap();
    assert_eq!(vr::resolve(&c, f.u, "point-private", 0).unwrap_err().code, "virtual_route_forbidden");
}

#[test]
fn private_point_reads_preserve_time_windows_and_recycle_bin_without_sibling_hydration() {
    let f = Fixture::new();
    let c = f.state.conn();
    let mut config = cfg("scheduled-point", f.provider);
    config.mode = "scheduled".into();
    config.timezone = "UTC".into();
    config.windows = vec![vr::Window {days_mask:127,start_minute:0,end_minute:1,target_positions:vec![1]}];
    let route = vr::create(&c, f.u, &config).unwrap();
    unrelated_corrupt_private_row(&c, f.u, route.id);
    assert_eq!(vr::resolve(&c, f.u, "scheduled-point", 0).unwrap().unwrap().targets.len(), 1);
    assert_eq!(vr::validate_current(&c, f.u, route.id, route.version, 1, 60_000).unwrap_err().code, "route_time_unavailable");
    let deleted = vr::trash(&c, f.u, route.id, route.version).unwrap();
    assert!(vr::resolve(&c, f.u, "scheduled-point", 0).unwrap().is_none());
    assert!(vr::get(&c, f.u, route.id).unwrap().is_none());
    assert_eq!(vr::validate_current(&c, f.u, route.id, route.version, 1, 0).unwrap_err().code, "virtual_route_forbidden");
    let restored = vr::restore(&c, f.u, route.id, deleted.version).unwrap();
    assert!(!restored.config.enabled);
    assert_eq!(vr::resolve(&c, f.u, "scheduled-point", 0).unwrap_err().code, "virtual_route_forbidden");
}

#[test]
fn private_name_and_id_queries_use_existing_point_indexes() {
    let f = Fixture::new();
    let c = f.state.conn();
    let route = vr::create(&c, f.u, &cfg("indexed-point", f.provider)).unwrap();
    for (query,params) in [
        ("EXPLAIN QUERY PLAN SELECT id FROM user_virtual_models WHERE user_id=?1 AND normalized_name=?2", vec![rusqlite::types::Value::Integer(f.u),rusqlite::types::Value::Text("indexed-point".into())]),
        ("EXPLAIN QUERY PLAN SELECT id FROM user_virtual_models WHERE id=?1 AND user_id=?2", vec![rusqlite::types::Value::Integer(route.id),rusqlite::types::Value::Integer(f.u)]),
    ] {
        let mut statement = c.prepare(query).unwrap();
        let plan = statement.query_map(rusqlite::params_from_iter(params), |row| row.get::<_,String>(3)).unwrap().collect::<Result<Vec<_>,_>>().unwrap().join(" ");
        assert!(plan.contains("SEARCH") && !plan.contains("SCAN"), "private point read must not scan a user directory: {plan}");
    }
}


#[test]
fn legacy_private_name_index_resolves_without_hydrating_siblings_and_keeps_live_guards() {
    let f = Fixture::new();
    let c = f.state.conn();
    let route = vr::create(&c, f.u, &cfg("Memory-BGE-M3", f.provider)).unwrap();
    c.execute("UPDATE user_virtual_models SET normalized_name='legacy-private-index' WHERE id=?1", [route.id]).unwrap();
    unrelated_corrupt_private_row(&c, f.u, route.id);
    assert!(vr::list(&c,f.u).is_err(), "legacy fallback must not parse unrelated bad JSON");
    assert_eq!(vr::resolve(&c,f.u," memory-bge-m3 ",0).unwrap().unwrap().id,route.id);
    assert!(vr::resolve(&c,f.other,"memory-bge-m3",0).unwrap().is_none());
    vr::validate_current(&c,f.u,route.id,route.version,1,0).unwrap();
    assert_eq!(vr::create(&c,f.u,&cfg("memory-bge-m3",f.provider)).unwrap_err().code,"virtual_route_conflict", "legacy metadata still owns its active alias");
    let mut user=db::identity::user(&c,f.u).unwrap().unwrap();
    user.policy.allowed_models=Some(vec![]);
    db::identity::update_user(&c,&user).unwrap();
    assert_eq!(vr::validate_current(&c,f.u,route.id,route.version,1,0).unwrap_err().code,"virtual_target_forbidden");
    user.policy.allowed_models=None;
    db::identity::update_user(&c,&user).unwrap();
    let deleted=vr::trash(&c,f.u,route.id,route.version).unwrap();
    assert!(vr::resolve(&c,f.u,"memory-bge-m3",0).unwrap().is_none(),"metadata fallback must exclude the recycle bin");
    let replacement=vr::create(&c,f.u,&cfg("memory-bge-m3",f.provider)).unwrap();
    assert_ne!(replacement.id,route.id);
    assert_eq!(vr::resolve(&c,f.u,"memory-bge-m3",0).unwrap().unwrap().id,replacement.id);
    assert_eq!(vr::restore(&c,f.u,route.id,deleted.version).unwrap_err().code,"virtual_route_conflict");
}

#[test]
fn private_index_hit_must_match_real_name_before_it_can_own_an_alias() {
    let f=Fixture::new();let c=f.state.conn();
    let route=vr::create(&c,f.u,&cfg("private-original",f.provider)).unwrap();
    c.execute("UPDATE user_virtual_models SET normalized_name='different-alias' WHERE id=?1",[route.id]).unwrap();
    assert!(vr::resolve(&c,f.u,"different-alias",0).unwrap().is_none(),"stale index metadata must never route an unrelated requested name");
    assert_eq!(vr::resolve(&c,f.u,"private-original",0).unwrap().unwrap().id,route.id);
    let second=vr::create(&c,f.u,&cfg("private-second",f.provider)).unwrap();
    assert_eq!(vr::replace(&c,f.u,second.id,second.version,&cfg("PRIVATE-ORIGINAL",f.provider)).unwrap_err().code,"virtual_route_conflict");
}


#[test]
fn earlier_legacy_private_name_keeps_precedence_over_a_later_canonical_index() {
    let f=Fixture::new();let c=f.state.conn();
    let old=vr::create(&c,f.u,&cfg("memory-bge-m3",f.provider)).unwrap();
    c.execute("UPDATE user_virtual_models SET normalized_name='legacy-index-name' WHERE id=?1",[old.id]).unwrap();
    // Reproduce a legacy/canonical duplicate without using the guarded write API.
    c.execute("INSERT INTO user_virtual_models(user_id,name,normalized_name,enabled,mode,timezone,default_positions_json,version,created_ms,updated_ms) SELECT user_id,name,?1,enabled,mode,timezone,default_positions_json,version,created_ms,updated_ms FROM user_virtual_models WHERE id=?2",rusqlite::params![tokengo_shared::normalize_model_name("memory-bge-m3"),old.id]).unwrap();
    let later=c.last_insert_rowid();
    c.execute("INSERT INTO user_virtual_targets SELECT ?1,position,provider_id,logical_model,switch_on_429,switch_on_400 FROM user_virtual_targets WHERE virtual_model_id=?2",rusqlite::params![later,old.id]).unwrap();
    unrelated_corrupt_private_row(&c,f.u,old.id);
    assert_eq!(vr::resolve(&c,f.u,"memory-bge-m3",0).unwrap().unwrap().id,old.id,"indexed canonical candidate must not replace earlier legacy first-match semantics");
    let mut user=db::identity::user(&c,f.u).unwrap().unwrap();
    user.policy.allowed_models=Some(vec![]);db::identity::update_user(&c,&user).unwrap();
    assert_eq!(vr::validate_current(&c,f.u,old.id,old.version,1,0).unwrap_err().code,"virtual_target_forbidden");
    user.policy.allowed_models=None;db::identity::update_user(&c,&user).unwrap();
    vr::trash(&c,f.u,old.id,old.version).unwrap();
    assert_eq!(vr::resolve(&c,f.u,"memory-bge-m3",0).unwrap().unwrap().id,later,"trashed legacy rows no longer own their old name");
}
