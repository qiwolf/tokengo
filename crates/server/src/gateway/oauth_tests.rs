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
    assert!(body.get("max_output_tokens").is_none());
    let item = json!({"id":"msg_synthetic","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"OAuth reply","annotations":[]}]});
    let bytes = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":{"id":"resp_synthetic","object":"response","status":"completed","output":[],"usage":{"input_tokens":7,"output_tokens":3}}})
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
    for field in ["max_output_tokens", "temperature"] {
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
