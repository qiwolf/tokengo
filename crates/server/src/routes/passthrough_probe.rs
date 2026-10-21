//! Explicit manual HTTP diagnostics; never consume a user budget.
use crate::{
    db,
    routes::{admin::PanelAuth, ApiError},
    SharedState,
};
use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use futures_util::StreamExt;
use serde::Deserialize;

pub fn router() -> Router<SharedState> {
    Router::new().route("/api/passthrough-routes/{id}/probe", post(run))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
    method: String,
    expected_status: u16,
}
async fn run(
    _auth: PanelAuth,
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    body: Result<Json<Input>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Json(input) = body.map_err(|_| ApiError::bad_request("Invalid HTTP diagnostic request"))?;
    if !["GET", "POST", "HEAD"].contains(&input.method.as_str())
        || !(200..=599).contains(&input.expected_status)
        || !input.path.starts_with('/')
        || input.path.contains(['?', '#', '\\', '%'])
        || input.path.contains("//")
        || input.path.split('/').any(|p| p == "." || p == "..")
        || input
            .path
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        || input.path.len() > 1024
    {
        return Err(ApiError::bad_request(
            "Probe requires a literal upstream path, GET/POST/HEAD, and expected HTTP status",
        ));
    }
    let route = {
        let c = state.conn();
        let r =
            db::passthrough::get(&c, id)?.ok_or_else(|| ApiError::not_found("Route not found"))?;
        db::passthrough::match_route(&c, &state.secrets, &r.prefix)?
            .ok_or_else(|| ApiError::not_found("Route not found"))?
    };
    if !route.route.enabled {
        return Err(ApiError::bad_request("Enable route before probing"));
    }
    let raw = format!(
        "{}{}",
        route.route.upstream.trim_end_matches('/'),
        input.path
    );
    let url =
        reqwest::Url::parse(&raw).map_err(|_| ApiError::bad_request("Invalid diagnostic URL"))?;
    let base = reqwest::Url::parse(&route.route.upstream)
        .map_err(|_| ApiError::internal("Invalid route URL"))?;
    let full_path = format!("{}{}", base.path().trim_end_matches('/'), input.path);
    if url.path() != full_path
        || url.origin() != base.origin()
        || route
            .route
            .allowed_paths
            .as_ref()
            .is_some_and(|paths| !paths.contains(&full_path))
    {
        return Err(ApiError::bad_request(
            "Probe path must be granted by the route",
        ));
    }
    let started = tokio::time::Instant::now();
    let deadline =
        started + std::time::Duration::from_secs(u64::from(route.route.timeout_secs.min(60)));
    let gate = state
        .channel_gate(route.route.channel_id, route.route.max_concurrent)
        .ok_or_else(|| ApiError::internal("Route gate unavailable"))?;
    let _permit = tokio::time::timeout_at(deadline, gate.acquire_owned())
        .await
        .map_err(|_| ApiError::bad_request("Probe queue deadline exceeded"))?
        .map_err(|_| ApiError::internal("Route gate unavailable"))?;
    // A queued diagnostic must not send stale credentials after edit/revoke.
    {
        let c = state.conn();
        let current =
            db::passthrough::get(&c, id)?.ok_or_else(|| ApiError::not_found("Route removed"))?;
        if !current.enabled || current.revision != route.route.revision {
            return Err(ApiError::bad_request("Route changed while queued"));
        }
    }
    let mut builder = crate::passthrough::client().request(
        reqwest::Method::from_bytes(input.method.as_bytes())
            .map_err(|_| ApiError::bad_request("Invalid method"))?,
        url,
    );
    for (name, value) in &route.headers {
        builder = builder.header(name, value);
    }
    let reservation = db::billing::reserve_diagnostic(
        &state.conn(),
        &uuid::Uuid::new_v4().to_string(),
        route.route.channel_id,
        &route.price.model,
        true,
        false,
        &serde_json::json!({"kind":"admin_diagnostic","subscription":false,"endpoint":"passthrough_probe"}),
        &serde_json::json!({"route_id":id,"revision":route.route.revision,"path":full_path,"method":input.method}),
    )?;
    let mut guard = Diagnostic {
        state: state.clone(),
        id: reservation,
        done: false,
    };
    let result = tokio::time::timeout_at(deadline, async move {
        let response = builder.send().await.map_err(|_| ())?;
        let status = response.status().as_u16();
        let mut stream = response.bytes_stream();
        let mut bytes = 0usize;
        while let Some(chunk) = stream.next().await {
            bytes = bytes.saturating_add(chunk.map_err(|_| ())?.len());
            if bytes > 128 * 1024 {
                return Err(());
            }
        }
        Ok::<_, ()>(status)
    })
    .await;
    let status = match result {
        Ok(Ok(status)) => {
            guard.finish(Some(1), "probe_completed");
            Some(status)
        }
        _ => {
            guard.finish(None, "probe_failed");
            None
        }
    };
    Ok(Json(
        serde_json::json!({"healthy":status==Some(input.expected_status),"http_status":status,"latency_ms":started.elapsed().as_millis() as u64,"inference_verified":false}),
    ))
}
struct Diagnostic {
    state: SharedState,
    id: i64,
    done: bool,
}
impl Diagnostic {
    fn finish(&mut self, units: Option<i64>, outcome: &str) {
        if !self.done {
            self.done = true;
            if crate::billing_writer::settle_diagnostic(
                &self.state,
                self.id,
                None,
                None,
                units,
                true,
                outcome,
            )
            .is_err()
            {
                tracing::error!(
                    reservation_id = self.id,
                    "HTTP diagnostic settlement pending durable reconciliation"
                );
            }
        }
    }
}
impl Drop for Diagnostic {
    fn drop(&mut self) {
        self.finish(None, "probe_canceled");
    }
}
