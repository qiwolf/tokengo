//! Full HTTP gateway regression with synthetic OAuth credentials and source.
use super::*;
use axum::{extract::State, http::HeaderMap, routing::post, Router};
use serde_json::json;
use std::sync::Mutex;
struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(router: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server { url, task }
}
#[derive(Default)]
struct Source {
    seen: Mutex<Vec<(HeaderMap, Value)>>,
}
async fn source(
    State(source): State<Arc<Source>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    source.seen.lock().unwrap().push((headers, body.clone()));
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    let cap = body.get("max_output_tokens").and_then(Value::as_i64);
    let limited = cap.is_some();
    let status = if limited { "incomplete" } else { "completed" };
    let output_tokens = cap.unwrap_or(3);
    let item = json!({"id":"msg_synthetic","type":"message","status":status,"phase":"final_answer","role":"assistant","content":[{"type":"output_text","text":"OAuth reply","annotations":[]}]});
    let bytes = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":if limited {"response.incomplete"} else {"response.completed"},"response":{"id":"resp_synthetic","object":"response","status":status,"completed_at":if limited {Value::Null} else {json!(1)},"text":{"format":body.get("text").and_then(|v|v.get("format")).cloned().unwrap_or(json!({"type":"text"})),"verbosity":"medium"},"incomplete_details":if limited {json!({"reason":"max_output_tokens"})} else {Value::Null},"output":if limited {json!([item])} else {json!([])},"usage":{"input_tokens":7,"output_tokens":output_tokens}}})
    );
    let stream = futures_util::stream::unfold(Some(bytes), |chunk| async move {
        match chunk {
            Some(chunk) => Some((Ok::<_, std::convert::Infallible>(Bytes::from(chunk)), None)),
            None => {
                std::future::pending::<()>().await;
                None
            }
        }
    });
    (
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}
struct Fixture {
    state: SharedState,
    gateway: Server,
    _source: Server,
    seen: Arc<Source>,
    key: String,
    channel: i64,
    dir: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
impl Fixture {
    async fn new() -> Self {
        let seen = Arc::new(Source::default());
        let upstream = serve(
            Router::new()
                .route("/v1/responses", post(source))
                .with_state(seen.clone()),
        )
        .await;
        let dir =
            std::env::temp_dir().join(format!("tokengo-oauth-forward-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::AppState::init(crate::Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into(),
            ..Default::default()
        })
        .unwrap();
        state.routing().cfg.retry_times = 0;
        state.routing().cfg.probe_enabled = false;
        let (_, key) = crate::db::identity::create_key(
            &state.conn(),
            1,
            "oauth-client",
            &Default::default(),
            None,
            None,
        )
        .unwrap();
        let channel = {
            let c = state.conn();
            let id = channels::create(
                &c,
                &channels::NewChannel {
                    name: "official OAuth".into(),
                    base_url: crate::oauth::API_BASE.into(),
                    enabled: true,
                    unwrap_envelope: false,
                    max_concurrent: 1,
                },
            )
            .unwrap();
            channels::set_models(
                &c,
                id,
                &[channels::ModelMapping::new(
                    "gpt-test",
                    Some("source-model".into()),
                    false,
                )],
            )
            .unwrap();
            id.0
        };
        let tokens = json!({"access_token":"SYNTHETIC-OAUTH-ACCESS","refresh_token":"SYNTHETIC-REFRESH","id_token":"e30.e30.c3ludGhldGlj","scopes":["chatgpt.tokens.use.direct","offline_access","resource.invoke"]});
        let (n, cipher) = state.secrets.seal(&tokens.to_string()).unwrap();
        state.conn().execute("INSERT INTO oauth_accounts(id,label,issuer,subject,client_id,status,scopes_json,expires_at,earliest_refresh_at,nonce,cipher) VALUES(1,'synthetic','https://auth.openai.com','subject','oaiapp_test','active','[]',?1,0,?2,?3)",rusqlite::params![now_ms()+3_600_000,n,cipher]).unwrap();
        crate::oauth::bind(
            &state,
            channel,
            &crate::oauth::Binding {
                account_id: 1,
                reasoning_effort: Some("low".into()),
                service_tier: Some("priority".into()),
                max_input_tokens: Some(131072),
                max_output_tokens: Some(32768),
            },
        )
        .unwrap();
        state.oauth.test_api_base(&format!("{}/v1", upstream.url));
        let gateway = serve(crate::app(state.clone())).await;
        Self {
            state,
            gateway,
            _source: upstream,
            seen,
            key,
            channel,
            dir,
        }
    }
    async fn send(&self, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/v1/{path}", self.gateway.url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
}
#[tokio::test]
async fn oauth_nonstream_conversion_native_stream_billing_and_explicit_limit_rejection() {
    let f = Fixture::new().await;
    for (path, body) in [
        (
            "responses",
            json!({"model":"gpt-test","input":"hello","stream":false}),
        ),
        (
            "chat/completions",
            json!({"model":"gpt-test","messages":[{"role":"user","content":"hello"}],"stream":false}),
        ),
    ] {
        let response = f.send(path, body).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("application/json"));
        let value: Value = response.json().await.unwrap();
        if path == "responses" {
            assert_eq!(value["output"][0]["content"][0]["text"], "OAuth reply");
        } else {
            assert_eq!(value["choices"][0]["message"]["content"], "OAuth reply");
        }
    }
    let response = f
        .send(
            "responses",
            json!({"model":"gpt-test","input":"hello","stream":true}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("response.completed"));
    let seen = f.seen.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    for (headers, body) in seen.iter() {
        assert_eq!(headers["authorization"], "Bearer SYNTHETIC-OAUTH-ACCESS");
        assert!(!headers.values().any(|v| v.to_str().ok() == Some(&f.key)));
        assert_eq!(body["model"], "source-model");
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["service_tier"], "priority");
    }
    drop(seen);
    f.state.drain_writes();
    crate::billing_writer::replay(&f.state.conn(), &f.state.cfg.db_path).unwrap();
    let count:i64=f.state.conn().query_row("SELECT count(*) FROM billing_ledger WHERE channel_id=?1 AND price_id IS NULL AND debit_micro IS NULL AND input_tokens=7 AND output_tokens=3 AND input_bound>0 AND output_bound=32768 AND status='unpriced'",[f.channel],|r|r.get(0)).unwrap();
    assert_eq!(count, 3);
    for field in ["temperature"] {
        let mut body = json!({"model":"gpt-test","input":"hello"});
        body[field] = json!(32);
        assert_eq!(
            f.send("responses", body).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(f.seen.seen.lock().unwrap().len(), 3);
    f.state
        .conn()
        .execute(
            "UPDATE oauth_accounts SET status='revoked',nonce=NULL,cipher=NULL WHERE id=1",
            [],
        )
        .unwrap();
    assert_eq!(
        f.send("responses", json!({"model":"gpt-test","input":"hello"}))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(f.seen.seen.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn oauth_client_native_dials_override_connection_defaults_without_model_suffix_inference() {
    let f=Fixture::new().await;
    let response=f.send("responses",json!({"model":"gpt-test","input":"hello","reasoning":{"effort":"high"},"service_tier":"default"})).await;
    assert_eq!(response.status(),StatusCode::OK);
    let _=response.bytes().await.unwrap();
    let seen=f.seen.seen.lock().unwrap();
    assert_eq!(seen.len(),1);
    assert_eq!(seen[0].1["model"],"source-model");
    assert_eq!(seen[0].1["reasoning"]["effort"],"high");
    assert_eq!(seen[0].1["service_tier"],"default");
}

#[tokio::test]
async fn oauth_mapping_defaults_and_provider_default_supersede_legacy_binding_without_affecting_explicit_client() {
    let f=Fixture::new().await;
    let levels=vec!["low".to_owned(),"high".to_owned()];
    {let c=f.state.conn();crate::reasoning_capabilities::configure(&c,f.channel,"gpt-test",Some(&levels),Some("high")).unwrap();}
    let response=f.send("responses",json!({"model":"gpt-test","input":"hello"})).await;assert_eq!(response.status(),StatusCode::OK);let _=response.bytes().await.unwrap();
    let response=f.send("responses",json!({"model":"gpt-test","input":"hello","reasoning":{"effort":"low"}})).await;assert_eq!(response.status(),StatusCode::OK);let _=response.bytes().await.unwrap();
    {let c=f.state.conn();crate::reasoning_capabilities::configure(&c,f.channel,"gpt-test",Some(&levels),None).unwrap();}
    let response=f.send("responses",json!({"model":"gpt-test","input":"hello"})).await;assert_eq!(response.status(),StatusCode::OK);let _=response.bytes().await.unwrap();
    let seen=f.seen.seen.lock().unwrap();assert_eq!(seen.len(),3);assert_eq!(seen[0].1["reasoning"]["effort"],"high");assert_eq!(seen[1].1["reasoning"]["effort"],"low");assert!(seen[2].1.get("reasoning").is_none(),"explicit provider default must not resurrect old low binding");
}

#[tokio::test]
async fn oauth_chat_schema_reaches_source_and_invalid_output_caps_do_not() {
    let f = Fixture::new().await;
    let schema = json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false});
    let body = json!({"model":"gpt-test","messages":[{"role":"user","content":"Return JSON"}],"response_format":{"type":"json_schema","json_schema":{"name":"extract","strict":true,"schema":schema}}});
    let response = f.send("chat/completions", body.clone()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let reply:Value = response.json().await.unwrap();
    assert_eq!(reply["choices"][0]["message"]["content"], "OAuth reply");
    {
        let seen = f.seen.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1["text"]["format"], json!({"type":"json_schema","name":"extract","strict":true,"schema":schema}));
        assert!(seen[0].1.get("response_format").is_none());
    }
    for field in ["max_tokens", "max_completion_tokens"] {
        let mut capped = body.clone();
        capped[field] = json!(15);
        assert_eq!(f.send("chat/completions", capped).await.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(f.seen.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn oauth_caps_preserve_truncation_usage_and_subscription_ledger() {
    let f = Fixture::new().await;
    for (path, field) in [("responses", "max_output_tokens"), ("chat/completions", "max_tokens"), ("chat/completions", "max_completion_tokens")] {
        for cap in [16, 32] {
            let mut body = if path == "responses" {json!({"model":"gpt-test","input":"Count numbers"})} else {json!({"model":"gpt-test","messages":[{"role":"user","content":"Count numbers"}]})};
            body[field] = json!(cap);
            let response = f.send(path, body).await;
            assert_eq!(response.status(), StatusCode::OK);
            let reply:Value = response.json().await.unwrap();
            if path == "responses" {
                assert_eq!(reply["status"], "incomplete");
                assert_eq!(reply["incomplete_details"]["reason"], "max_output_tokens");
                assert_eq!(reply["usage"]["input_tokens"], 7);
                assert_eq!(reply["usage"]["output_tokens"], cap);
            } else {
                assert_eq!(reply["choices"][0]["finish_reason"], "length");
                assert_eq!(reply["usage"]["prompt_tokens"], 7);
                assert_eq!(reply["usage"]["completion_tokens"], cap);
            }
            let seen = f.seen.seen.lock().unwrap();
            assert_eq!(seen.last().unwrap().1["max_output_tokens"], cap);
            assert!(seen.last().unwrap().1.get("max_tokens").is_none());
            assert!(seen.last().unwrap().1.get("max_completion_tokens").is_none());
        }
    }
    f.state.drain_writes();
    crate::billing_writer::replay(&f.state.conn(), &f.state.cfg.db_path).unwrap();
    let c = f.state.conn();
    let count:i64=c.query_row("SELECT count(*) FROM billing_ledger WHERE channel_id=?1 AND price_id IS NULL AND debit_micro IS NULL AND input_tokens=7 AND output_tokens=output_bound AND output_bound IN (16,32) AND status='unpriced' AND settled_ms IS NOT NULL",[f.channel],|r|r.get(0)).unwrap();
    assert_eq!(count, 6);
    let reserved:i64=c.query_row("SELECT coalesce(sum(reserved_micro),0) FROM billing_budgets",[],|r|r.get(0)).unwrap();
    assert_eq!(reserved, 0);
}

#[tokio::test]
async fn oauth_caps_reject_below_minimum_above_model_ceiling_and_bad_types_before_source() {
    let f = Fixture::new().await;
    for cap in [json!(0), json!(15), json!(32769), json!(-1), json!(16.5), json!("32"), Value::Null] {
        let reply = f.send("responses", json!({"model":"gpt-test","input":"hello","max_output_tokens":cap})).await;
        assert_eq!(reply.status(), StatusCode::BAD_REQUEST, "{cap}");
    }
    assert!(f.seen.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn oauth_cap_stream_keeps_length_usage_and_priced_settlement() {
    let f = Fixture::new().await;
    crate::db::billing::create_price(&f.state.conn(),&crate::db::billing::Price{id:0,channel_id:f.channel,model:"gpt-test".into(),input_micro_per_million:0,output_micro_per_million:1_000_000,max_input_tokens:16384,max_output_tokens:32768},"cap fixture").unwrap();
    let response=f.send("chat/completions",json!({"model":"gpt-test","messages":[{"role":"user","content":"Count numbers"}],"max_completion_tokens":16,"stream":true,"stream_options":{"include_usage":true}})).await;
    assert_eq!(response.status(),StatusCode::OK);
    let wire=response.text().await.unwrap();
    let events:Vec<Value>=wire.lines().filter_map(|s|s.strip_prefix("data: ")).filter_map(|s|serde_json::from_str(s).ok()).collect();
    assert!(events.iter().any(|e|e["choices"][0]["finish_reason"]=="length"));
    assert!(events.iter().any(|e|e["usage"]["completion_tokens"]==16));
    f.state.drain_writes();
    crate::billing_writer::replay(&f.state.conn(),&f.state.cfg.db_path).unwrap();
    let c=f.state.conn();
    let row:(i64,i64,i64,i64)=c.query_row("SELECT output_bound,output_tokens,debit_micro,overrun FROM billing_ledger WHERE channel_id=?1",[f.channel],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(row,(16,16,16,0));
    assert_eq!(c.query_row("SELECT coalesce(sum(reserved_micro),0) FROM billing_budgets",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}

#[tokio::test]
async fn oauth_explicit_cap_uses_cap_for_tpm_admission_instead_of_full_ceiling() {
    let f=Fixture::new().await;
    let auth=f.state.authenticate_context(f.key.clone()).await.unwrap();
    let mut key=crate::db::identity::key(&f.state.conn(),auth.key_id).unwrap().unwrap();
    key.policy.tpm=Some(1000);
    crate::db::identity::update_key(&f.state.conn(),&key).unwrap();
    let response=f.send("responses",json!({"model":"gpt-test","input":"Count numbers","max_output_tokens":32})).await;
    assert_eq!(response.status(),StatusCode::OK);
    let _=response.bytes().await.unwrap();
    assert_eq!(f.send("responses",json!({"model":"gpt-test","input":"Count numbers"})).await.status(),StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(f.seen.seen.lock().unwrap().len(),1);
}

#[tokio::test]
async fn oauth_responses_accepts_output_cap_aliases_without_weakening_conflict_or_ceiling_checks() {
    let f=Fixture::new().await;
    for limits in [json!({"max_tokens":32}),json!({"max_completion_tokens":32}),json!({"max_tokens":32,"max_output_tokens":32,"max_completion_tokens":32})] {
        let mut body=json!({"model":"gpt-test","input":"Count numbers"});
        body.as_object_mut().unwrap().extend(limits.as_object().unwrap().clone());
        let response=f.send("responses",body).await;
        assert_eq!(response.status(),StatusCode::OK);
        let reply:Value=response.json().await.unwrap();
        assert_eq!(reply["usage"]["output_tokens"],32);
        let seen=f.seen.seen.lock().unwrap();
        let body=&seen.last().unwrap().1;
        assert_eq!(body["max_output_tokens"],32);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("max_completion_tokens").is_none());
    }
    for limits in [json!({"max_tokens":15}),json!({"max_completion_tokens":32769}),json!({"max_tokens":16,"max_output_tokens":32}),json!({"max_tokens":16,"max_completion_tokens":32}),json!({"max_tokens":"32"}),json!({"max_tokens":null})] {
        let mut body=json!({"model":"gpt-test","input":"Count numbers"});
        body.as_object_mut().unwrap().extend(limits.as_object().unwrap().clone());
        assert_eq!(f.send("responses",body).await.status(),StatusCode::BAD_REQUEST);
    }
    assert_eq!(f.seen.seen.lock().unwrap().len(),3);
}
