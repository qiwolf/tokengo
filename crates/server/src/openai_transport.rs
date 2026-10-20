//! Optional proxy for dedicated OpenAI clients; the generic provider client is unchanged.
pub fn configure(builder: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder, &'static str> {
    let value = match std::env::var("TOKENGO_OPENAI_PROXY") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err("Invalid OpenAI proxy configuration"),
    };
    with_proxy(builder, value.as_deref())
}
fn with_proxy(
    builder: reqwest::ClientBuilder,
    value: Option<&str>,
) -> Result<reqwest::ClientBuilder, &'static str> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(builder);
    };
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid OpenAI proxy configuration")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.port() == Some(0)
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Invalid OpenAI proxy configuration");
    }
    let proxy = reqwest::Proxy::all(value).map_err(|_| "Invalid OpenAI proxy configuration")?;
    Ok(builder.no_proxy().proxy(proxy))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dedicated_proxy_reaches_only_the_client_configured_for_it() {
        use axum::{extract::State, http::Uri, routing::any, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let hits = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .fallback(any(
                |State(hits): State<Arc<AtomicUsize>>, uri: Uri| async move {
                    assert_eq!(uri.host(), Some("oauth-test.invalid"));
                    hits.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({"via":"dedicated"}))
                },
            ))
            .with_state(hits.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let direct = reqwest::Client::builder().no_proxy().build().unwrap();
        let dedicated = with_proxy(reqwest::Client::builder(), Some(&proxy))
            .unwrap()
            .build()
            .unwrap();
        let value: serde_json::Value = dedicated
            .get("http://oauth-test.invalid/check")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(value["via"], "dedicated");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(500),
                direct.get("http://oauth-test.invalid/check").send()
            )
            .await
            .is_err()
                || hits.load(Ordering::SeqCst) == 1
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        task.abort();
    }
    #[test]
    fn invalid_proxy_errors_never_echo_credentials() {
        for value in [
            "not a URL SECRET",
            "ftp://user:SECRET@example.com",
            "http://user:SECRET@example.com/?token=SECRET",
        ] {
            let error = with_proxy(reqwest::Client::builder(), Some(value))
                .err()
                .unwrap();
            assert!(!error.contains("SECRET"));
        }
    }
}
