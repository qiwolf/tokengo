//! Real TCP gateway/upstream regression for endpoint-scoped rerank meters.
use super::*;

async fn meter_case(path: &str, response: &'static str, priced: bool, expected: (Option<i64>, Option<i64>)) {
    let f = fixture();
    f.with_key();
    assert!(f.state.check_gateway_key(KEY));
    let upstream = start_mock(Reply::Status(200, response)).await;
    let id = f.add_channel("rerank-meter", &upstream.base, ChannelSpec::default());
    f.serve("mock-chat", &[id]);
    if priced {
        use tokengo_server::db::billing::{self, Price};
        billing::create_price(&f.state.conn(), &Price {
            id: 0, channel_id: id.0, model: "mock-chat".into(),
            input_micro_per_million: 1_000_000, output_micro_per_million: 1_000_000,
            max_input_tokens: 4096, max_output_tokens: 128,
        }, "rerank-meter-test").unwrap();
        billing::set_budget(&f.state.conn(), "user", 1, Some(100_000), "rerank-meter-test").unwrap();
    }
    f.state.spawn_writer();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = f.state.clone();
    let server = tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap(); });
    let request = if path == "/v1/rerank" {
        json!({"model":"mock-chat","query":"x","documents":["x"]})
    } else if path == "/v1/embeddings" {
        json!({"model":"mock-chat","input":"x"})
    } else {
        json!({"model":"mock-chat","messages":[{"role":"user","content":"x"}],"max_tokens":8})
    };
    let reply = reqwest::Client::new().post(format!("http://{addr}{path}"))
        .bearer_auth(KEY).json(&request).send().await.unwrap();
    assert_eq!(reply.status(), StatusCode::OK, "{response}");
    let delivered: Value = reply.json().await.unwrap();
    let original: Value = serde_json::from_str(response).unwrap();
    for field in ["usage", "tokens", "response", "message"] {
        assert_eq!(delivered.get(field), original.get(field), "meter extraction must not rewrite upstream {field}");
    }
    if path == "/v1/rerank" {
        // Existing Jina normalization adds meta groups and copies top-level usage
        // into them. Preserve that contract instead of requiring byte-identical meta.
        for field in ["input_tokens", "output_tokens"] {
            let pointer = format!("/meta/tokens/{field}");
            let normalized = original.get("usage").and_then(|u| u.get(field))
                .or_else(|| original.pointer(&pointer));
            assert_eq!(delivered.pointer(&pointer), normalized);
        }
        assert_eq!(delivered["results"][0]["index"], 0);
        assert_eq!(delivered["results"][0]["document"]["text"], "x");
    }
    assert_eq!(upstream.count(), 1);
    assert_eq!(upstream.seen()[0].path, path);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            f.state.drain_writes();
            let unsettled: i64 = f.state.conn().query_row(
                "SELECT count(*) FROM billing_ledger WHERE settled_ms IS NULL", [], |r| r.get(0),
            ).unwrap();
            if f.state.pending_writes() == 0 && unsettled == 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("usage and durable settlement must finish");
    let conn = f.state.conn();
    let logs = tokengo_server::db::requests::recent(&conn, 2).unwrap();
    assert_eq!(logs.len(), 1);
    let row = &logs[0];
    assert_eq!(row.status, "ok");
    assert_eq!((row.input_tokens, row.output_tokens), (expected.0.unwrap_or(0), expected.1.unwrap_or(0)), "{response}");
    assert_eq!((row.input_tokens_known, row.output_tokens_known), (expected.0.is_some(), expected.1.is_some()), "{response}");
    assert_eq!(row.usage_source, if expected.0.is_some() || expected.1.is_some() { "upstream" } else { "unreported" });
    // Existing settlement contracts structurally default nongeneration output to zero;
    // this is independent of whether diagnostics received an upstream output meter.
    let ledger_expected = (expected.0, expected.1.or_else(|| (path != CHAT).then_some(0)));
    let (input, output, certainty, status, debit, reserved, observed, price_id, snapshot):
        (Option<i64>, Option<i64>, String, String, Option<i64>, i64, Option<i64>, Option<i64>, Option<String>) = conn.query_row(
        "SELECT input_tokens,output_tokens,usage_certainty,status,debit_micro,reserved_micro,observed_cost_micro,price_id,price_snapshot FROM billing_ledger", [],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?)),
    ).unwrap();
    assert_eq!((input, output), ledger_expected, "retain endpoint-specific settlement semantics: {response}");
    assert_eq!(conn.query_row("SELECT count(*) FROM billing_ledger", [], |r|r.get::<_,i64>(0)).unwrap(), 1);
    if priced {
        assert_eq!(status, "success");
        let complete = ledger_expected.0.zip(ledger_expected.1);
        assert_eq!(certainty, if complete.is_some() { "reported" } else { "estimated" });
        assert_eq!(observed, complete.map(|(a,b)|a+b));
        let price_id = price_id.expect("priced request must retain its price identity");
        let snapshot: Value = serde_json::from_str(snapshot.as_deref().expect("priced snapshot must exist")).unwrap();
        assert_eq!(snapshot, json!({"id":price_id,"channel_id":id.0,"model":"mock chat",
            "input_micro_per_million":1_000_000,"output_micro_per_million":1_000_000,
            "max_input_tokens":4096,"max_output_tokens":128}));
        let expected_debit = complete.map(|(a,b)|a+b).unwrap_or(reserved).min(reserved);
        assert_eq!(debit, Some(expected_debit));
        let budget = tokengo_server::db::billing::budgets(&conn, Some("user"), Some(1)).unwrap().remove(0);
        assert_eq!((budget.spent_micro, budget.reserved_micro), (expected_debit, 0));
    } else {
        assert_eq!((status.as_str(), certainty.as_str(), debit), ("unpriced", "unpriced", None));
        assert_eq!((price_id, observed), (None, None));
        assert_eq!(snapshot, None);
    }
    drop(conn);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn rerank_meter_tokens_and_meta_reach_diagnostics_and_priced_or_unpriced_ledger() {
    for priced in [false, true] {
        for response in [
            r#"{"results":[{"index":0,"relevance_score":1}],"tokens":{"input_tokens":13,"output_tokens":2}}"#,
            r#"{"results":[{"index":0,"relevance_score":1}],"meta":{"tokens":{"input_tokens":13,"output_tokens":2}}}"#,
        ] {
            meter_case("/v1/rerank", response, priced, (Some(13), Some(2))).await;
        }
    }
}

#[tokio::test]
async fn rerank_meter_usage_zero_and_per_field_precedence() {
    for (response, expected) in [
        (r#"{"results":[{"index":0,"relevance_score":1}],"usage":{"input_tokens":0,"output_tokens":0},"tokens":{"input_tokens":13,"output_tokens":2}}"#, (Some(0),Some(0))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"usage":{"input_tokens":0},"tokens":{"input_tokens":13,"output_tokens":2}}"#, (Some(0),Some(2))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"usage":{"output_tokens":0},"meta":{"tokens":{"input_tokens":13,"output_tokens":2}}}"#, (Some(13),Some(0))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"tokens":{"input_tokens":13},"meta":{"tokens":{"input_tokens":99,"output_tokens":2}}}"#, (Some(13),Some(2))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"usage":{"total_tokens":9}}"#, (Some(9),Some(0))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"usage":{"total_tokens":9},"tokens":{"input_tokens":13}}"#, (Some(9),Some(0))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"tokens":{"input_tokens":13}}"#, (Some(13),Some(0))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"tokens":{"input_tokens":-1,"output_tokens":"2"},"meta":{"tokens":{"input_tokens":13,"output_tokens":2}}}"#, (Some(13),Some(2))),
    ] { meter_case("/v1/rerank", response, true, expected).await; }
}

#[tokio::test]
async fn rerank_meter_invalid_or_absent_values_stay_unknown() {
    for response in [
        r#"{"results":[{"index":0,"relevance_score":1}]}"#,
        r#"{"results":[{"index":0,"relevance_score":1}],"tokens":{"input_tokens":-1,"output_tokens":"2"}}"#,
        r#"{"results":[{"index":0,"relevance_score":1}],"meta":{"tokens":{"input_tokens":"13","output_tokens":-2}}}"#,
        r#"{"results":[{"index":0,"relevance_score":1}],"tokens":{"input_tokens":18446744073709551615,"output_tokens":1.5}}"#,
    ] { meter_case("/v1/rerank", response, true, (None,None)).await; }
}

#[tokio::test]
async fn rerank_meter_cache_overflow_cannot_be_replaced_by_smaller_token_fallback() {
    for response in [
        r#"{"results":[{"index":0,"relevance_score":1}],"usage":{"input_tokens":9223372036854775807,"cache_read_input_tokens":1,"output_tokens":0},"tokens":{"input_tokens":13}}"#,
        r#"{"results":[{"index":0,"relevance_score":1}],"usage":{"input_tokens":9223372036854775807,"cache_creation_input_tokens":1,"output_tokens":0},"meta":{"tokens":{"input_tokens":13}}}"#,
    ] {
        meter_case("/v1/rerank", response, true, (None, Some(0))).await;
    }
}

#[tokio::test]
async fn rerank_meter_preserves_wrapped_usage_and_anthropic_cache_counts() {
    for (response, expected) in [
        (r#"{"results":[{"index":0,"relevance_score":1}],"response":{"usage":{"input_tokens":3,"output_tokens":0,"cache_read_input_tokens":5,"cache_creation_input_tokens":7}}}"#, (Some(15),Some(0))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"message":{"usage":{"input_tokens":3,"output_tokens":0}}}"#, (Some(3),Some(0))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"response":{"tokens":{"input_tokens":13,"output_tokens":2}}}"#, (Some(13),Some(2))),
        (r#"{"results":[{"index":0,"relevance_score":1}],"message":{"meta":{"tokens":{"input_tokens":13,"output_tokens":2}}}}"#, (Some(13),Some(2))),
    ] { meter_case("/v1/rerank", response, true, expected).await; }
}

#[tokio::test]
async fn rerank_meter_does_not_treat_other_endpoint_tokens_as_usage() {
    meter_case(CHAT, r#"{"choices":[{"message":{"content":"ok"}}],"tokens":{"input_tokens":13,"output_tokens":2},"meta":{"tokens":{"input_tokens":99,"output_tokens":99}}}"#, false, (None,None)).await;
    meter_case("/v1/embeddings", r#"{"data":[{"embedding":[1,2]}],"tokens":{"input_tokens":13,"output_tokens":2}}"#, false, (None,None)).await;
    meter_case(CHAT, r#"{"choices":[{"message":{"content":"ok"}}],"usage":{"input_tokens":3,"output_tokens":0,"cache_read_input_tokens":5,"cache_creation_input_tokens":7},"tokens":{"input_tokens":99,"output_tokens":99}}"#, true, (Some(15),Some(0))).await;
}

#[tokio::test]
async fn rerank_meter_rejected_results_do_not_gain_token_fallback_usage() {
    for response in [
        r#"{"results":[{"index":"invalid","relevance_score":1}],"tokens":{"input_tokens":13,"output_tokens":2}}"#,
        r#"{"results":[{"index":0,"relevance_score":1}],"usage":{"input_tokens":-1},"tokens":{"input_tokens":13,"output_tokens":2}}"#,
    ] {
        let f = fixture(); f.with_key();
        f.state.routing().cfg.retry_times = 0;
        let upstream = start_mock(Reply::Status(200, response)).await;
        let id = f.add_channel("invalid-rerank-meter", &upstream.base, ChannelSpec::default());
        f.serve("mock-chat", &[id]);
        let (status, _, _) = post(&f, "/v1/rerank", Some(KEY),
            json!({"model":"mock-chat","query":"x","documents":["x"]})).await;
        assert!(!status.is_success());
        f.state.drain_writes();
        let logs = tokengo_server::db::requests::recent(&f.state.conn(), 1).unwrap();
        assert_eq!((logs[0].input_tokens_known, logs[0].output_tokens_known), (false, false));
        assert_eq!(logs[0].usage_source, "unreported");
        assert_eq!(upstream.count(), 1);
    }
}
