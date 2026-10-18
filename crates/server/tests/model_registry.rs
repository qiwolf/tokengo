use axum::{routing::get, Router};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokengo_server::{
    db::{
        self,
        channels::{self, ModelMapping, NewChannel},
        model_routes,
    },
    model_registry, AppState, Config, SharedState,
};
use tower::ServiceExt;

const FIXTURE: &str = r#"{
 "vendor":{"models":{
   "real":{"id":"real","limit":{"context":8192},"canonical_model_id":"lab/real-v1"},
   "unique":{"id":"unique","limit":{"context":4096}},
   "missing":{"id":"missing","limit":{"output":2048}}
 }},
 "other":{"models":{"real":{"id":"real","limit":{"context":16384}}}}
}"#;
fn fixture() -> (SharedState, std::path::PathBuf) {
    let dir = std::path::PathBuf::from("/tmp")
        .join(format!("registry-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = AppState::init(Config {
        db_path: dir.join("test.db"),
        master_key_path: dir.join("key").to_string_lossy().into_owned(),
        ..Default::default()
    })
    .unwrap();
    let conn = state.conn();
    let id = channels::create(
        &conn,
        &NewChannel {
            name: "local".into(),
            base_url: "http://127.0.0.1:1".into(),
            enabled: true,
            unwrap_envelope: false,
            max_concurrent: 0,
        },
    )
    .unwrap();
    channels::set_models(
        &conn,
        id,
        &[
            ModelMapping::new("public", Some("vendor/real".into()), false),
            ModelMapping::new("ambiguous", Some("real".into()), false),
            ModelMapping::new("unique", None, false),
            ModelMapping::new("v4.1flash", None, false),
            ModelMapping::new("anthropic/claude-v4.1flash", None, false),
            ModelMapping::new("alias-must-not-match", Some("absent".into()), false),
        ],
    )
    .unwrap();
    drop(conn);
    (state, dir)
}
async fn server(
    status: u16,
    body: String,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let router = Router::new().route(
        "/registry",
        get(move |headers: axum::http::HeaderMap| {
            let count = seen.clone();
            let body = body.clone();
            async move {
                assert_eq!(headers["accept"], "application/json");
                assert!(!headers.contains_key("authorization"));
                count.fetch_add(1, Ordering::SeqCst);
                (axum::http::StatusCode::from_u16(status).unwrap(), body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/registry", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, count, task)
}
#[test]
fn exact_ids_conflicts_and_invalid_payloads() {
    let entries = model_registry::parse(FIXTURE.as_bytes()).unwrap();
    assert_eq!(entries["vendor/real"].tokens, Some(8192));
    assert_eq!(entries["lab/real-v1"].tokens, Some(8192));
    assert_eq!(entries["unique"].tokens, Some(4096));
    assert_eq!(entries["missing"].tokens, None);
    assert_eq!(entries["real"].tokens, None);
    assert!(entries["real"].reason.contains("歧义"));
    for name in [
        "REAL",
        "vendor_real",
        "v4.1flash",
        "anthropic/claude-v4.1flash",
    ] {
        assert!(!entries.contains_key(name));
    }
    for body in [
        "{}",
        "[]",
        "<html>blocked</html>",
        r#"{"data":[{"id":"x","context_length":8192}]}"#,
    ] {
        assert!(model_registry::parse(body.as_bytes()).is_err());
    }
    assert!(model_registry::parse(&vec![b' '; 12 * 1024 * 1024 + 1]).is_err());
}
#[tokio::test]
async fn cache_restart_aliases_nominal_only_and_upstream_precedence() {
    let (url, count, server) = server(200, FIXTURE.into()).await;
    let (state, dir) = fixture();
    model_registry::refresh(&state, &[&url]).await.unwrap();
    model_registry::refresh(&state, &[&url]).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    {
        let conn = state.conn();
        let public = model_routes::routes_for(&conn, "public").unwrap().unwrap();
        assert_eq!(public.context_window, None);
        assert_eq!(public.nominal_context_window, Some(8192));
        assert_eq!(public.nominal_context_source.as_deref(), Some("models.dev"));
        for alias in [
            "v4.1flash",
            "anthropic/claude-v4.1flash",
            "alias-must-not-match",
        ] {
            let route = model_routes::routes_for(&conn, alias).unwrap().unwrap();
            assert!(route.nominal_context_window.is_none());
            assert!(route.context_reason.contains("没有精确 models.dev ID"));
        }
        assert!(model_routes::routes_for(&conn, "ambiguous")
            .unwrap()
            .unwrap()
            .context_reason
            .contains("歧义"));
        for table in ["requests", "attempts", "routing_state"] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        db::sessions::set_gateway_key(&conn, &state.secrets, "test-key").unwrap();
    }
    let response = tokengo_server::app(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri("/v1/models")
                .header("authorization", "Bearer test-key")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for item in body["data"].as_array().unwrap() {
        assert!(
            item.get("context_window").is_none(),
            "nominal registry metadata must not become safe channel context: {item}"
        );
    }
    let cfg = state.cfg.clone();
    drop(state);
    let state = AppState::init(cfg).unwrap();
    model_registry::refresh(&state, &[&url]).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    {
        let conn = state.conn();
        assert_eq!(
            model_routes::routes_for(&conn, "public")
                .unwrap()
                .unwrap()
                .nominal_context_window,
            Some(8192)
        );
        let id = channels::list(&conn).unwrap()[0].id;
        db::catalog::replace(
            &conn,
            id,
            &tokengo_server::upstream::parse_model_catalog(
                r#"{"data":[{"id":"vendor/real","context_length":1024}]}"#,
            ),
            db::now_ms(),
        )
        .unwrap();
        let route = model_routes::routes_for(&conn, "public").unwrap().unwrap();
        assert_eq!(route.context_window, Some(1024));
        assert_eq!(route.context_source.as_deref(), Some("upstream"));
        assert_eq!(route.nominal_context_window, None);
        // One unknown usable target keeps both safe and nominal aggregates unknown.
        let id = channels::create(
            &conn,
            &NewChannel {
                name: "second".into(),
                base_url: "http://127.0.0.1:1".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        channels::set_models(
            &conn,
            id,
            &[ModelMapping::new("public", Some("unknown".into()), false)],
        )
        .unwrap();
        let route = model_routes::routes_for(&conn, "public").unwrap().unwrap();
        assert_eq!(route.context_window, None);
        assert_eq!(route.nominal_context_window, None);
        channels::set_enabled(&conn, id, false).unwrap();
        assert_eq!(
            model_routes::routes_for(&conn, "public")
                .unwrap()
                .unwrap()
                .context_window,
            Some(1024)
        );
    }
    server.abort();
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn alternate_failure_backoff_stale_expiry_and_download_bounds() {
    let (bad, bad_count, bad_server) = server(403, "denied".into()).await;
    let (good, good_count, good_server) = server(200, FIXTURE.into()).await;
    let (state, dir) = fixture();
    model_registry::refresh(&state, &[&bad, &good])
        .await
        .unwrap();
    assert_eq!(bad_count.load(Ordering::SeqCst), 1);
    assert_eq!(good_count.load(Ordering::SeqCst), 1);
    state
        .conn()
        .execute("UPDATE registry_context_status SET next_at=0", [])
        .unwrap();
    model_registry::refresh(&state, &[&bad, &bad, &good])
        .await
        .unwrap();
    model_registry::refresh(&state, &[&bad, &bad, &good])
        .await
        .unwrap();
    assert_eq!(
        bad_count.load(Ordering::SeqCst),
        3,
        "maximum two attempts per scheduled refresh"
    );
    assert_eq!(good_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        model_routes::routes_for(&state.conn(), "public")
            .unwrap()
            .unwrap()
            .nominal_context_window,
        Some(8192),
        "failed refresh preserves good cache"
    );
    let cfg = state.cfg.clone();
    drop(state);
    let state = AppState::init(cfg).unwrap();
    model_registry::refresh(&state, &[&bad]).await.unwrap();
    assert_eq!(
        bad_count.load(Ordering::SeqCst),
        3,
        "backoff persists on restart"
    );
    state
        .conn()
        .execute(
            "UPDATE registry_context_status SET fetched_at=1,next_at=0",
            [],
        )
        .unwrap();
    assert!(model_routes::routes_for(&state.conn(), "public")
        .unwrap()
        .unwrap()
        .context_reason
        .contains("过期"));
    let (huge, huge_count, huge_server) = server(200, " ".repeat(12 * 1024 * 1024 + 1)).await;
    model_registry::refresh(&state, &[&huge]).await.unwrap();
    assert_eq!(huge_count.load(Ordering::SeqCst), 1);
    let error: String = state
        .conn()
        .query_row("SELECT error FROM registry_context_status", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(error.contains("byte limit"));
    for task in [bad_server, good_server, huge_server] {
        task.abort();
    }
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Explicit opt-in only: never makes CI/test runs depend on the public Internet.
#[tokio::test]
#[ignore]
async fn real_registry_fetch() {
    let (state, dir) = fixture();
    model_registry::tick(&state).await.unwrap();
    let conn = state.conn();
    let (at, origin, error): (i64, String, String) = conn
        .query_row(
            "SELECT fetched_at,origin,error FROM registry_context_status",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!(at > 0, "{error}");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM registry_context", [], |r| r.get(0))
        .unwrap();
    assert!(count > 0);
    let context = model_registry::mapped(
        &conn,
        tokengo_shared::ChannelId(1),
        &ModelMapping::new("alias", Some("deepseek/deepseek-flash".into()), false),
    )
    .unwrap();
    assert!(context.tokens.is_some(), "{context:?}");
    println!(
        "real registry: origin={origin}, identifiers={count}, deepseek/deepseek-flash={:?}",
        context.tokens
    );
    for alias in ["v4.1flash", "anthropic/claude-v4.1flash"] {
        let context =
            model_registry::mapped(&conn, tokengo_shared::ChannelId(1), &ModelMapping::new(alias, None, false)).unwrap();
        assert_eq!(context.tokens, None);
        println!("{alias}: {}", context.reason);
    }
    drop(conn);
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}

async fn registry_request(state: &SharedState, token: Option<&str>, method: &str, id: i64, body: serde_json::Value) -> (u16, serde_json::Value) {
    let uri = if method == "GET" {
        format!("/api/channels/{id}/registry?model=v4.1flash&upstream=v4.1flash&q=vendor")
    } else { format!("/api/channels/{id}/registry") };
    let mut request = axum::http::Request::builder().method(method).uri(uri).header("content-type","application/json");
    if let Some(token) = token { request = request.header("authorization",format!("Bearer {token}")); }
    let response = tokengo_server::app(state.clone()).oneshot(request.body(axum::body::Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1024*1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn explicit_associations_crud_auth_isolation_restart_and_mapping_changes() {
    use tokengo_shared::ChannelId;
    use serde_json::json;
    let (url, _, server) = server(200, FIXTURE.into()).await;
    let (state, dir) = fixture();
    model_registry::refresh(&state, &[&url]).await.unwrap();
    let token = db::sessions::create_session(&state.conn(), None).unwrap();
    let body = |identity: Option<&str>| json!({"model":"v4.1flash","upstream":"v4.1flash","identity":identity});
    for method in ["GET","PUT"] {
        assert_eq!(registry_request(&state,None,method,1,body(Some("vendor/real"))).await.0,401);
        assert_eq!(registry_request(&state,Some("invalid"),method,1,body(Some("vendor/real"))).await.0,401);
    }
    let (code, result) = registry_request(&state,Some(&token),"GET",1,json!(null)).await;
    assert_eq!(code,200);
    assert!(result["identity"].is_null());
    assert_eq!(result["entries"][0]["id"],"vendor/real");
    for identity in ["real","missing","not-found","VENDOR/real", "", " vendor/real"] {
        assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(Some(identity))).await.0,400,"{identity}");
    }
    assert_eq!(registry_request(&state,Some(&token),"PUT",999,body(Some("vendor/real"))).await.0,404);
    assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(Some("vendor/real"))).await.0,200);
    let (_, result) = registry_request(&state,Some(&token),"GET",1,json!(null)).await;
    assert_eq!(result["identity"],"vendor/real");
    assert_eq!(result["tokens"],8192);
    {
        let conn=state.conn();
        let channel=channels::get(&conn,ChannelId(1)).unwrap().unwrap();
        let mapping=channel.models.iter().find(|m|m.model=="v4.1flash").unwrap();
        assert_eq!(mapping.upstream,None);
        assert_eq!(model_registry::upstream(mapping),"v4.1flash");
        assert_eq!(model_registry::mapped(&conn,ChannelId(1),mapping).unwrap().tokens,Some(8192));
        let second=channels::create(&conn,&NewChannel {name:"second".into(),base_url:"http://127.0.0.1:1".into(),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
        channels::set_models(&conn,second,&[mapping.clone()]).unwrap();
        assert_eq!(model_registry::mapped(&conn,second,mapping).unwrap().tokens,None);
        let another=ModelMapping::new("other-public",Some("v4.1flash".into()),false);
        assert_eq!(model_registry::mapped(&conn,ChannelId(1),&another).unwrap().tokens,None);
        channels::set_models(&conn,ChannelId(1),&channel.models).unwrap();
        assert_eq!(model_registry::associated(&conn,ChannelId(1),mapping).unwrap().as_deref(),Some("vendor/real"));
    }
    let config=state.cfg.clone(); drop(state);
    let state=AppState::init(config).unwrap();
    let (_, result)=registry_request(&state,Some(&token),"GET",1,json!(null)).await;
    assert_eq!(result["identity"],"vendor/real");
    assert_eq!(result["tokens"],8192);
    assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(Some("unique"))).await.0,200);
    assert_eq!(registry_request(&state,Some(&token),"GET",1,json!(null)).await.1["tokens"],4096);
    assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(None)).await.0,200);
    assert!(registry_request(&state,Some(&token),"GET",1,json!(null)).await.1["identity"].is_null());
    assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(Some("vendor/real"))).await.0,200);
    {
        let conn=state.conn();
        let mut channel=channels::get(&conn,ChannelId(1)).unwrap().unwrap();
        channel.models.iter_mut().find(|m|m.model=="v4.1flash").unwrap().upstream=Some("changed".into());
        channels::set_models(&conn,ChannelId(1),&channel.models).unwrap();
    }
    assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(Some("vendor/real"))).await.0,400);
    {
        let conn=state.conn();
        let mut channel=channels::get(&conn,ChannelId(1)).unwrap().unwrap();
        let mapping=channel.models.iter_mut().find(|m|m.model=="v4.1flash").unwrap();
        assert_eq!(model_registry::associated(&conn,ChannelId(1),mapping).unwrap(),None);
        mapping.upstream=None;
        channels::set_models(&conn,ChannelId(1),&channel.models).unwrap();
    }
    assert!(registry_request(&state,Some(&token),"GET",1,json!(null)).await.1["identity"].is_null(),"old upstream must not resurrect association");
    // Deleted/conflicting/expired cached identities cannot silently retain nominal capacity.
    assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(Some("vendor/real"))).await.0,200);
    state.conn().execute("UPDATE registry_context SET context_window=NULL,reason='冲突' WHERE model_id='vendor/real'",[]).unwrap();
    let (_,result)=registry_request(&state,Some(&token),"GET",1,json!(null)).await;
    assert_eq!(result["identity"],"vendor/real"); assert!(result["tokens"].is_null());
    assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(Some("vendor/real"))).await.0,400);
    state.conn().execute("UPDATE registry_context_status SET fetched_at=1 WHERE id=1",[]).unwrap();
    assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(Some("unique"))).await.0,400);
    assert_eq!(registry_request(&state,Some(&token),"PUT",1,body(None)).await.0,200);
    for table in ["requests","attempts","routing_state"] {
        assert_eq!(state.conn().query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }
    drop(state); server.abort(); std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn user_confirmed_capacity_alias_auto_minimum_restart_and_invalidation() {
    use tokengo_shared::ChannelId;
    use serde_json::json;
    let (state,dir)=fixture();
    let token=db::sessions::create_session(&state.conn(),None).unwrap();
    {
        let conn=state.conn();
        let mut channel=channels::get(&conn,ChannelId(1)).unwrap().unwrap();
        channel.models=vec![ModelMapping::new("v4.1flash",None,false),ModelMapping::new("alias",Some("v4.1flash".into()),false),ModelMapping::new("anthropic/claude-v4.1flash",None,false)];
        channels::set_models(&conn,ChannelId(1),&channel.models).unwrap();
        db::sessions::set_gateway_key(&conn,&state.secrets,"key").unwrap();
    }
    let put=|auth:Option<String>,tokens:serde_json::Value| {
        let state=state.clone();
        async move {
            let mut req=axum::http::Request::builder().method("PUT").uri("/api/channels/1/context").header("content-type","application/json");
            if let Some(t)=auth {req=req.header("authorization",format!("Bearer {t}"));}
            let response=tokengo_server::app(state).oneshot(req.body(axum::body::Body::from(json!({"model":"v4.1flash","upstream":"v4.1flash","tokens":tokens}).to_string())).unwrap()).await.unwrap();
            response.status().as_u16()
        }
    };
    assert_eq!(put(None,json!(524288)).await,401);
    for n in [json!(0),json!(-1),json!(1.5),json!("524288")] {assert_eq!(put(Some(token.clone()),n).await,400);}
    assert_eq!(put(Some(token.clone()),json!(524288)).await,200);
    for name in ["v4.1flash","alias"] {
        let r=model_routes::routes_for(&state.conn(),name).unwrap().unwrap();
        assert_eq!(r.context_window,Some(524288));assert_eq!(r.context_source.as_deref(),Some("user_confirmed"));
    }
    assert_eq!(model_routes::routes_for(&state.conn(),"anthropic/claude-v4.1flash").unwrap().unwrap().context_window,None);
    assert_eq!(put(Some(token.clone()),serde_json::Value::Null).await,200);
    assert_eq!(model_routes::routes_for(&state.conn(),"alias").unwrap().unwrap().context_window,None);
    assert_eq!(put(Some(token.clone()),json!(524288)).await,200);
    {
        let conn=state.conn();
        conn.execute("INSERT INTO channel_context_overrides VALUES(1,'anthropic/claude-v4.1flash',524288)",[]).unwrap();
        db::catalog::replace(&conn,ChannelId(1),&tokengo_server::upstream::parse_model_catalog(r#"{"data":[{"id":"v4.1flash","context_length":1048576}]}"#),db::now_ms()).unwrap();
        assert_eq!(model_routes::routes_for(&conn,"alias").unwrap().unwrap().context_window,Some(524288));
    }
    let cfg=state.cfg.clone();drop(state);
    let state=AppState::init(cfg).unwrap();
    assert_eq!(model_routes::routes_for(&state.conn(),"alias").unwrap().unwrap().context_window,Some(524288));
    let list=|state:SharedState|async move {
        let r=tokengo_server::app(state).oneshot(axum::http::Request::builder().uri("/v1/models").header("authorization","Bearer key").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(r.status(),200);
        serde_json::from_slice::<serde_json::Value>(&axum::body::to_bytes(r.into_body(),1024*1024).await.unwrap()).unwrap()
    };
    let result=list(state.clone()).await;
    for name in ["auto","alias","v4.1flash","anthropic/claude-v4.1flash"] {
        let item=result["data"].as_array().unwrap().iter().find(|m|m["id"]==name).unwrap();
        assert_eq!(item["context_window"],524288,"{item}");assert_eq!(item["context_source"],"user_confirmed");
    }
    {
        let conn=state.conn();
        let second=channels::create(&conn,&NewChannel{name:"second".into(),base_url:"http://127.0.0.1:1".into(),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
        channels::set_models(&conn,second,&[ModelMapping::new("alias",Some("v4.1flash".into()),false)]).unwrap();
        assert_eq!(model_routes::routes_for(&conn,"alias").unwrap().unwrap().context_window,None);
        conn.execute("INSERT INTO channel_context_overrides VALUES(?1,'v4.1flash',262144)",[second.0]).unwrap();
        assert_eq!(model_routes::routes_for(&conn,"alias").unwrap().unwrap().context_window,Some(262144));
    }
    let result=list(state.clone()).await;
    assert_eq!(result["data"].as_array().unwrap().iter().find(|m|m["id"]=="auto").unwrap()["context_window"],262144);
    {
        let conn=state.conn();let mut channel=channels::get(&conn,ChannelId(1)).unwrap().unwrap();
        // Remove all references to the old target: switching back must not revive stale manual capacity.
        for m in &mut channel.models {if m.model=="v4.1flash" || m.model=="alias" {m.upstream=Some("changed".into());}}
        channels::set_models(&conn,ChannelId(1),&channel.models).unwrap();
        assert_eq!(model_registry::context_override(&conn,ChannelId(1),&ModelMapping::new("v4.1flash",None,false)).unwrap(),None);
        assert_eq!(model_registry::context_override(&conn,ChannelId(1),&channel.models[0]).unwrap(),None);
    }
    drop(state); std::fs::remove_dir_all(dir).unwrap();
}
