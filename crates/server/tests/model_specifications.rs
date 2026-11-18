use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tokengo_shared::ChannelId;
use tower::ServiceExt;

struct Fixture {
    state: SharedState,
    admin: String,
    dir: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("tokengo-specifications-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("master").to_string_lossy().into_owned(),
            ..Default::default()
        }).unwrap();
        let admin = db::sessions::create_session(&state.conn(), None).unwrap();
        Self { state, admin, dir }
    }

    fn channel(&self, provider_name: &str, logical: &str, upstream: &str) -> (ChannelId, i64) {
        let conn = self.state.conn();
        conn.execute(
            "INSERT INTO provider_channels(name,created_ms) VALUES(?1,?2)",
            rusqlite::params![provider_name, db::now_ms()],
        ).unwrap();
        let provider = conn.last_insert_rowid();
        let channel = db::channels::create(&conn, &db::channels::NewChannel {
            name: format!("{provider_name} line"),
            base_url: "https://source.invalid/v1".into(),
            enabled: true,
            unwrap_envelope: false,
            max_concurrent: 0,
        }).unwrap();
        db::provider_channels::set_membership(&conn, channel.0, Some(provider)).unwrap();
        db::channels::set_models(
            &conn,
            channel,
            &[db::channels::ModelMapping::new(logical, Some(upstream.into()), false)],
        ).unwrap();
        (channel, provider)
    }

    fn client_key(&self, channels: Option<Vec<i64>>) -> String {
        let conn = self.state.conn();
        let username = format!("spec-client-{}", uuid::Uuid::new_v4());
        let user = db::identity::create_user(&conn, &username, "user", &Default::default()).unwrap();
        let policy = db::identity::Policy { allowed_channels: channels, ..Default::default() };
        db::identity::create_key(&conn, user.id, "spec discovery", &policy, None, None).unwrap().1
    }

    async fn call(&self, method: &str, path: &str, token: Option<&str>, payload: Option<Value>)
        -> (StatusCode, Value)
    {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let body = if let Some(payload) = payload {
            request = request.header("content-type", "application/json");
            Body::from(payload.to_string())
        } else {
            Body::empty()
        };
        let response = app(self.state.clone()).oneshot(request.body(body).unwrap()).await.unwrap();
        let status = response.status();
        let raw = to_bytes(response.into_body(), 2 << 20).await.unwrap();
        (status, serde_json::from_slice(&raw).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).into_owned())))
    }

    async fn put(&self, row: Value) -> (StatusCode, Value) {
        self.call("PUT", "/api/model-specifications", Some(&self.admin), Some(row)).await
    }

    async fn model(&self, token: &str, id: &str) -> Value {
        let (status, body) = self.call("GET", "/v1/models", Some(token), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["data"].as_array().unwrap().iter()
            .find(|row| row["id"] == id).unwrap().clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn chat(provider: Option<i64>, context: i64) -> Value {
    json!({
        "model_id": "vendor/ExactChat",
        "provider_id": provider,
        "kind": "chat",
        "context_window": context,
        "input_usage_reference": 34_000,
        "output_usage_reference": 7_000,
        "source_url": "https://vendor.invalid/models",
        "metrics": {"image_count": {"value": 4, "unit": "image"}},
    })
}

#[tokio::test]
async fn exact_specs_are_managed_by_admin_and_visible_only_on_eligible_routes() {
    let f = Fixture::new();
    let (first, provider) = f.channel("first", "public-alias", "vendor/ExactChat");
    f.channel("second", "public-alias", "vendor/ExactChat");
    let client = f.client_key(None);
    let scoped = f.client_key(Some(vec![first.0]));

    let (status, _) = f.call("GET", "/api/model-specifications", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = f.call(
        "PUT", "/api/model-specifications", Some(&client), Some(chat(None, 128_000))
    ).await;
    assert!(matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN));
    assert_eq!(f.put(chat(None, 128_000)).await.0, StatusCode::OK);
    assert_eq!(f.put(chat(Some(provider), 64_000)).await.0, StatusCode::OK);

    let (status, result) = f.call(
        "POST",
        "/api/model-specifications/resolve",
        Some(&f.admin),
        Some(json!({"provider_id":provider,"model_ids":["vendor/ExactChat","vendor/exactchat","vendor/ExactChat"]})),
    ).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let rows = result["specifications"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["model_id"], "vendor/ExactChat");
    assert_eq!(rows[0]["context_window"], 64_000);
    assert_eq!(rows[0]["source_url"], "https://vendor.invalid/models");

    let model = f.model(&scoped, "public-alias").await;
    assert_eq!(model["specifications"]["context_window"], 64_000, "{model}");
    assert_eq!(model["specifications"]["input_usage_reference"], 34_000);
    assert!(model.get("context_window").is_none());
    assert!(!model.to_string().contains("vendor"));

    let model = f.model(&client, "public-alias").await;
    assert!(model["specifications"].get("context_window").is_none(), "{model}");
    assert_eq!(model["specifications"]["input_usage_reference"], 34_000);
    assert_eq!(model["specifications"]["output_usage_reference"], 7_000);
    assert_eq!(model["specifications"]["metrics"]["image_count"], json!({"value":4,"unit":"image"}));

    let (status, result) = f.call(
        "DELETE",
        &format!("/api/model-specifications?model_id=vendor%2FExactChat&provider_id={provider}"),
        Some(&f.admin),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["removed"], true);
    assert_eq!(f.model(&scoped, "public-alias").await["specifications"]["context_window"], 128_000);

    f.channel("unmaintained", "public-alias", "different-exact-model");
    assert!(f.model(&client, "public-alias").await.get("specifications").is_none());
    assert_eq!(f.model(&scoped, "public-alias").await["specifications"]["context_window"], 128_000);
}

fn pricing_option<'a>(response: &'a Value, model: &str) -> &'a Value {
    let normalized = tokengo_shared::normalize_model_name(model);
    response["options"].as_array().unwrap().iter()
        .find(|row| row["model"].as_str() == Some(normalized.as_str()))
        .unwrap_or_else(|| panic!("missing pricing option for {model}: {response}"))
}

#[tokio::test]
async fn pricing_reuses_usage_references_without_treating_context_as_a_reference() {
    let f = Fixture::new();
    let (channel, provider) = f.channel("first", "public-alias", "vendor/ExactChat");
    f.channel("vectors", "vector-alias", "vendor/ExactVector");
    assert_eq!(f.put(chat(None, 128_000)).await.0, StatusCode::OK);
    assert_eq!(f.put(json!({
        "model_id":"vendor/ExactVector",
        "kind":"embeddings",
        "context_window":8192,
        "metrics":{"embedding_dimensions":{"value":1024,"unit":"dimension"}},
    })).await.0, StatusCode::OK);
    let mut invalid = chat(None, 128_000);
    invalid["output_usage_reference"] = 0.into();
    assert_eq!(f.put(invalid).await.0, StatusCode::BAD_REQUEST);

    let configured = tokengo_server::protocol_config::Capabilities {
        max_input_tokens: Some(100_000),
        max_output_tokens: Some(16_000),
        ..Default::default()
    };
    tokengo_server::protocol_config::save(&f.state.conn(), channel.0, &configured).unwrap();
    let (status, options) = f.call("GET", "/api/pricing/options", Some(&f.admin), None).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let chat_option = pricing_option(&options, "public-alias");
    assert_eq!(chat_option["input_usage_reference"], 34_000);
    assert_eq!(chat_option["output_usage_reference"], 7_000);
    assert_eq!(chat_option["input_bound_source"], "model_specification");
    let embedding_option = pricing_option(&options, "vector-alias");
    assert_eq!(embedding_option["task_type"], "embeddings");
    assert!(embedding_option["max_input_tokens"].is_null());
    assert_eq!(embedding_option["output_usage_reference"], 0);

    let (status, price) = f.call("POST", "/api/pricing", Some(&f.admin), Some(json!({
        "channel_id":channel.0,"model":"public-alias",
        "input_micro_per_million":10_000,"output_micro_per_million":20_000,
        "max_input_tokens":12_000
    }))).await;
    assert_eq!(status, StatusCode::OK, "{price}");
    assert_eq!(price["price"]["max_input_tokens"], 12_000);
    assert_eq!(price["price"]["max_output_tokens"], 7_000);

    let mut updated = chat(Some(provider), 64_000);
    updated["max_input_tokens"] = 60_000.into();
    updated["input_usage_reference"] = 44_000.into();
    updated["output_usage_reference"] = 9_000.into();
    let (status, specification) = f.put(updated).await;
    assert_eq!(status, StatusCode::OK, "{specification}");
    assert_eq!(specification["specification"]["context_window"], 64_000);
    assert_eq!(specification["specification"]["max_input_tokens"], 60_000);
    let client = f.client_key(None);
    let public = f.model(&client, "public-alias").await;
    assert_eq!(public["specifications"]["max_input_tokens"], 60_000);
    assert_eq!(public["specifications"]["input_usage_reference"], 44_000);

    let (status, options) = f.call("GET", "/api/pricing/options", Some(&f.admin), None).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let row = pricing_option(&options, "public-alias");
    assert_eq!(row["input_usage_reference"], 44_000);
    assert_eq!(row["output_usage_reference"], 9_000);
    assert_eq!(row["input_bound_source"], "channel_model");
    let rates = json!({
        "channel_id":channel.0, "model":"public-alias",
        "input_micro_per_million":11_000, "output_micro_per_million":22_000
    });
    let path = format!("/api/pricing/{}", price["price"]["id"]);
    let mut conflicting = rates.clone();
    conflicting["max_input_tokens"] = 1_000.into();
    assert_eq!(f.call("PUT", &path, Some(&f.admin), Some(conflicting)).await.0, StatusCode::BAD_REQUEST);
    let (status, revised) = f.call("PUT", &path, Some(&f.admin), Some(rates)).await;
    assert_eq!(status, StatusCode::OK, "{revised}");
    assert_eq!(revised["price"]["max_input_tokens"], 44_000);
    assert_eq!(revised["price"]["max_output_tokens"], 9_000);

    let conn = f.state.conn();
    let history = db::billing::list_prices(&conn).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].max_input_tokens, 12_000);
    assert_eq!(history[1].max_output_tokens, 7_000);
    let configured = tokengo_server::protocol_config::load(&conn, channel.0).unwrap();
    assert_eq!(configured.max_input_tokens, Some(100_000));
    assert_eq!(configured.max_output_tokens, Some(16_000));
}

#[tokio::test]
async fn partial_override_keeps_source_specific_references_and_lower_defaults() {
    let f = Fixture::new();
    let (legacy, provider) = f.channel("first", "mapped-chat", "vendor/ExactChat");
    let (fresh, _) = f.channel("second", "mapped-chat", "vendor/ExactChat");
    {
        let conn = f.state.conn();
        db::provider_channels::set_membership(&conn, fresh.0, Some(provider)).unwrap();
        conn.execute(
            "INSERT INTO billing_prices(channel_id,model,input_micro_per_million,output_micro_per_million,
                max_input_tokens,max_output_tokens,created_ms,currency)
             VALUES(?1,'mapped chat',123,456,12000,3000,1,'USD')",
            [legacy.0],
        ).unwrap();
        let cny = db::billing::create_price(&conn, &db::billing::Price {
            id: 0,
            channel_id: fresh.0,
            model: "mapped-chat".into(),
            currency: "CNY".into(),
            input_micro_per_million: 987,
            output_micro_per_million: 654,
            max_input_tokens: 88_000,
            max_output_tokens: 17_000,
        }, "fixture").unwrap();
        db::billing::change_price_state(&conn, "token", cny.id, None, "fixture").unwrap();
    }

    assert_eq!(f.put(chat(None, 128_000)).await.0, StatusCode::OK);
    assert_eq!(f.put(json!({
        "model_id":"vendor/ExactChat", "provider_id":provider,
        "kind":"chat", "max_input_tokens":100_000
    })).await.0, StatusCode::OK);

    let (status, options) = f.call("GET", "/api/pricing/options", Some(&f.admin), None).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let by_source = |channel: ChannelId| {
        options["options"].as_array().unwrap().iter().find(|row| row["channel_id"] == channel.0).unwrap()
    };
    let historical = by_source(legacy);
    assert_eq!(historical["input_usage_reference"], 12_000);
    assert_eq!(historical["output_usage_reference"], 3_000);
    assert_eq!(historical["input_bound_source"], "existing_price");
    let inherited = by_source(fresh);
    assert_eq!(inherited["input_usage_reference"], 34_000);
    assert_eq!(inherited["output_usage_reference"], 7_000);
    assert_eq!(inherited["input_bound_source"], "model_specification");

    let (status, edit) = f.call(
        "GET", &format!("/api/model-specifications?model_id=vendor%2FExactChat&provider_id={provider}"),
        Some(&f.admin), None,
    ).await;
    assert_eq!(status, StatusCode::OK, "{edit}");
    assert!(edit["specifications"][0].get("input_usage_reference").is_none());
    let client = f.client_key(None);
    let published = f.model(&client, "mapped-chat").await;
    assert_eq!(published["specifications"]["max_input_tokens"], 100_000);
    assert_eq!(published["specifications"]["input_usage_reference"], 34_000);

    let (status, cny) = f.call("POST", "/api/pricing", Some(&f.admin), Some(json!({
        "channel_id":legacy.0, "model":"mapped-chat",
        "input_micro_per_million":100, "output_micro_per_million":200
    }))).await;
    assert_eq!(status, StatusCode::OK, "{cny}");
    assert_eq!(cny["price"]["currency"], "CNY");
    assert_eq!(cny["price"]["input_micro_per_million"], 100);
    assert_eq!(cny["price"]["max_input_tokens"], 12_000);
    let history = db::billing::list_prices(&f.state.conn()).unwrap();
    let usd = history.iter().find(|row| row.currency == "USD").unwrap();
    assert_eq!(usd.input_micro_per_million, 123);
    assert_eq!(usd.max_input_tokens, 12_000);
}

#[tokio::test]
async fn unit_rate_versions_copy_channel_quantities_for_the_selected_unit() {
    let f = Fixture::new();
    let (channel, provider) = f.channel("audio provider", "speech-to-text", "vendor/ExactAsr");
    let mut spec = json!({
        "model_id":"vendor/ExactAsr", "provider_id":provider, "kind":"asr",
        "quantity_usage_references":{"second":64_000, "hour":75_000}
    });
    assert_eq!(f.put(spec.clone()).await.0, StatusCode::OK);
    let (status, options) = f.call("GET", "/api/pricing/options", Some(&f.admin), None).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let option = pricing_option(&options, "speech-to-text");
    assert_eq!(option["quantity_usage_references"]["minute"], 64_000);
    assert_eq!(option["quantity_usage_references"]["hour"], 75_000);
    assert_eq!(option["quantity_reference_sources"]["minute"], "channel_model");

    let rates = json!({
        "channel_id":channel.0, "model":"speech-to-text", "unit":"minute", "micro_per_unit":25_000
    });
    let (status, original) = f.call("POST", "/api/unit-pricing", Some(&f.admin), Some(rates.clone())).await;
    assert_eq!(status, StatusCode::OK, "{original}");
    assert_eq!(original["unit_price"]["max_quantity_milli"], 64_000);
    let path = format!("/api/unit-pricing/{}", original["unit_price"]["id"]);

    spec["quantity_usage_references"]["second"] = 80_000.into();
    assert_eq!(f.put(spec).await.0, StatusCode::OK);
    let mut invalid = rates.clone();
    invalid["max_quantity_milli"] = 64_000.into();
    assert_eq!(f.call("PUT", &path, Some(&f.admin), Some(invalid)).await.0, StatusCode::BAD_REQUEST);
    let (status, revised) = f.call("PUT", &path, Some(&f.admin), Some(rates)).await;
    assert_eq!(status, StatusCode::OK, "{revised}");
    assert_eq!(revised["unit_price"]["max_quantity_milli"], 80_000);
    let history = db::billing::list_unit_prices(&f.state.conn()).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].max_quantity_milli, 64_000);
}

#[tokio::test]
async fn exact_relay_defaults_take_precedence_without_matching_similar_urls_or_variants() {
    let f = Fixture::new();
    let (channel, provider) = f.channel("test relay", "public-alias", "fixture-chat-fast");
    let mut global = chat(None, 100_000);
    global["model_id"] = "fixture-chat".into();
    assert_eq!(f.put(global).await.0, StatusCode::OK);
    {
        let conn = f.state.conn();
        conn.execute_batch(
            r#"INSERT INTO model_specification_aliases VALUES
                 ('fixture-chat-fast', 'fixture-chat'), ('fixture-chat-ultrafast', 'fixture-chat');
             INSERT INTO model_specification_relay_defaults VALUES
                 ('command_code', 'fixture-chat-fast', 'https://api.commandcode.ai/provider/v1/models',
                  '{"kind":"chat","context_window":64000,"origin":"relay_official"}');"#,
        ).unwrap();
        conn.execute("UPDATE channels SET base_url='https://api.commandcode.ai/provider/v1' WHERE id=?1", [channel.0]).unwrap();
        conn.execute(
            "INSERT INTO registry_context(model_id,context_window,reason) VALUES('fixture-nominal',8192,'fixture')",
            [],
        ).unwrap();
        conn.execute("UPDATE registry_context_status SET fetched_at=?1 WHERE id=1", [db::now_ms()]).unwrap();
    }

    let mut scoped_canonical = chat(Some(provider), 48_000);
    scoped_canonical["model_id"] = "fixture-chat".into();
    assert_eq!(f.put(scoped_canonical).await.0, StatusCode::OK);

    let request = json!({"provider_id": provider,
        "model_ids": ["fixture-chat-fast", "fixture-chat-ultrafast", "fixture-nominal"]});
    let (status, body) = f.call(
        "POST", "/api/model-specifications/resolve", Some(&f.admin), Some(request.clone())
    ).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["specifications"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["model_id"], "fixture-chat-fast");
    assert_eq!(rows[0]["context_window"], 64_000);
    assert!(rows[0].get("output_usage_reference").is_none());
    assert_eq!(rows[1]["model_id"], "fixture-chat-ultrafast");
    assert_eq!(rows[1]["context_window"], 100_000);

    let client = f.client_key(None);
    let model = f.model(&client, "public-alias").await;
    assert_eq!(model["specifications"]["origin"], "relay_official", "{model}");
    assert!(model["specifications"].get("output_usage_reference").is_none());

    let mut wrong_endpoint = request;
    wrong_endpoint["base_url"] = "https://api.commandcode.ai.example.invalid/provider/v1".into();
    let (status, body) = f.call(
        "POST", "/api/model-specifications/resolve", Some(&f.admin), Some(wrong_endpoint)
    ).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["specifications"][0]["context_window"], 100_000);

    let mut override_row = chat(Some(provider), 32_000);
    override_row["model_id"] = "fixture-chat-fast".into();
    assert_eq!(f.put(override_row).await.0, StatusCode::OK);
    assert_eq!(f.model(&client, "public-alias").await["specifications"]["context_window"], 32_000);
}
