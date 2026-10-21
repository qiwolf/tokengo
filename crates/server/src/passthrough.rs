//! Raw HTTP transport, intentionally separate from all JSON/model protocol code.
//! Reference: BerriAI/litellm 045fb7ba84803e3265462c7988ecfaab35351860.
use crate::{
    db::{
        self,
        passthrough::{self as store, RuntimeRoute},
        requests,
    },
    routes::{proxy::GatewayKey, ApiError},
    SharedState,
};
use axum::{
    body::Body,
    extract::{FromRequestParts, Request, State},
    http::{header, HeaderMap, Method, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::{stream, StreamExt};
use std::{
    collections::HashSet,
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};

pub(crate) fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("raw HTTP client")
    })
}

/// Runs outside normal compression and URI tracing: raw queries may contain business data.
pub async fn dispatch(State(state): State<SharedState>, request: Request, next: Next) -> Response {
    let route = store::match_route(&state.conn(), &state.secrets, request.uri().path());
    match route {
        Ok(Some(route)) => match forward(state, request, route).await {
            Ok(r) => r,
            Err(e) => e.into_response(),
        },
        Ok(None) => next.run(request).await,
        Err(_) => ApiError::internal("Pass-through configuration unavailable").into_response(),
    }
}

fn filtered(headers: &HeaderMap, inbound: bool) -> HeaderMap {
    let mut removed: HashSet<String> = [
        "connection",
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "content-length",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for v in headers.get_all(header::CONNECTION) {
        if let Ok(v) = v.to_str() {
            for n in v.split(',') {
                removed.insert(n.trim().to_ascii_lowercase());
            }
        }
    }
    if inbound {
        for n in ["host", "authorization", "x-api-key", "api-key", "cookie"] {
            removed.insert(n.into());
        }
    }
    let mut out = HeaderMap::new();
    for (n, v) in headers {
        if !removed.contains(n.as_str()) {
            out.append(n.clone(), v.clone());
        }
    }
    out
}

/// Reject URL normalizations rather than silently change the path/query sent upstream.
fn target(route: &store::Route, uri: &Uri) -> Result<(reqwest::Url, String), ApiError> {
    let suffix = if route.strip_prefix {
        uri.path()
            .strip_prefix(&route.prefix)
            .ok_or_else(|| ApiError::bad_request("Route path mismatch"))?
    } else {
        uri.path()
    };
    let base = route.upstream.trim_end_matches('/');
    let path = if route.strip_prefix {
        format!("/{suffix}")
    } else {
        suffix.to_owned()
    };
    let raw = format!(
        "{base}{path}{}",
        uri.query().map(|q| format!("?{q}")).unwrap_or_default()
    );
    let url =
        reqwest::Url::parse(&raw).map_err(|_| ApiError::bad_request("Invalid upstream path"))?;
    let base_url = reqwest::Url::parse(base)
        .map_err(|_| ApiError::internal("Invalid upstream configuration"))?;
    let expected = format!("{}{path}", base_url.path().trim_end_matches('/'));
    if url.path() != expected || url.query() != uri.query() || url.origin() != base_url.origin() {
        return Err(ApiError::bad_request(
            "Ambiguous path cannot be transparently forwarded",
        ));
    }
    Ok((url, expected))
}

async fn forward(
    state: SharedState,
    request: Request,
    route: RuntimeRoute,
) -> Result<Response, ApiError> {
    let (mut parts, body) = request.into_parts();
    if parts.method == Method::CONNECT || parts.headers.contains_key(header::UPGRADE) {
        return Err(ApiError::bad_request(
            "HTTP tunnels and upgrades are not supported",
        ));
    }
    // Native MT Photos clients only expose api-key. It is a LOCAL VKey here,
    // with the same conflict rules, and is never forwarded as backend identity.
    let values: Vec<_> = parts.headers.get_all("api-key").iter().cloned().collect();
    if values.len() > 1 {
        return Err(ApiError::bad_request(
            "Repeated api-key authentication header",
        ));
    }
    if let Some(value) = values.first() {
        let raw = value
            .to_str()
            .map_err(|_| ApiError::unauthorized("Invalid api-key"))?
            .trim();
        if raw.is_empty() {
            return Err(ApiError::unauthorized("Empty api-key"));
        }
        if parts.headers.get_all("x-api-key").iter().count() > 1
            || parts
                .headers
                .get("x-api-key")
                .is_some_and(|v| v.as_bytes() != raw.as_bytes())
        {
            return Err(ApiError::bad_request("Conflicting authentication headers"));
        }
        parts.headers.insert(
            "x-api-key",
            header::HeaderValue::from_str(raw)
                .map_err(|_| ApiError::unauthorized("Invalid api-key"))?,
        );
    }
    let key = GatewayKey::from_request_parts(&mut parts, &state).await?;
    if !route.route.enabled {
        return Err(ApiError::not_found("Pass-through route disabled"));
    }
    let (url, path) = target(&route.route, &parts.uri)?;
    if route
        .route
        .allowed_paths
        .as_ref()
        .is_some_and(|paths| !paths.contains(&path))
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "route_path_denied",
            "Path is not granted by this route",
        ));
    }
    if !store::authorize(&state.conn(), &route, &key.auth)? {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "route_denied",
            "Virtual key is not granted this route",
        ));
    }
    let limit = route.route.max_body_bytes;
    if parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n > limit)
    {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "Request exceeds route upload limit",
        ));
    }
    let started = db::now_ms();
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(route.route.timeout_secs.into());
    let admission = crate::admission::acquire_request_only(state.clone(), &key.auth)?;
    let gate = state
        .channel_gate(route.route.channel_id, route.route.max_concurrent)
        .expect("bounded route gate");
    let channel = tokio::time::timeout_at(deadline, gate.acquire_owned())
        .await
        .map_err(|_| timeout())?
        .map_err(|_| ApiError::internal("Route concurrency gate unavailable"))?;
    // Recheck auth and revision after queueing, before any billable attempt or request bytes.
    {
        let c = state.conn();
        let fresh = store::match_route(&c, &state.secrets, parts.uri.path())?
            .ok_or_else(|| ApiError::not_found("Route no longer available"))?;
        if fresh.route.id != route.route.id || fresh.route.revision != route.route.revision {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "route_changed",
                "Route changed while queued; retry explicitly",
            ));
        }
        if !store::authorize(&c, &fresh, &key.auth)? {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "route_denied",
                "Route authorization changed",
            ));
        }
        let current = db::identity::context(&c, key.auth.key_id)?
            .ok_or_else(|| ApiError::unauthorized("Key no longer active"))?;
        if current.effective_policy().tpm.is_some() {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "opaque_tpm_unsupported",
                "Pass-through cannot measure tokens",
            ));
        }
    }
    let mut headers = filtered(&parts.headers, true);
    for (name, value) in &route.headers {
        headers.insert(
            header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ApiError::internal("Invalid configured header"))?,
            header::HeaderValue::from_str(value)
                .map_err(|_| ApiError::internal("Invalid configured header"))?,
        );
    }
    let request_id = {
        let c = state.conn();
        let id = requests::start(
            &c,
            &format!("passthrough:{}", route.route.id),
            &route.price.model,
            None,
            false,
        )?;
        c.execute(
            "UPDATE requests SET user_id=?2,key_id=?3,auth_snapshot=?4 WHERE id=?1",
            rusqlite::params![
                id,
                key.auth.user_id,
                key.auth.key_id,
                serde_json::to_string(&key.auth)
                    .map_err(|_| ApiError::internal("Identity snapshot failed"))?
            ],
        )
        .map_err(db::DbError::from)?;
        id
    };
    let mut guard = Guard {
        state: state.clone(),
        reservation: None,
        request_id,
        channel_id: route.route.channel_id,
        model: route.price.model.clone(),
        started,
        http_status: None,
        dispatched: false,
        done: false,
        _admission: admission,
        _channel: channel,
    };
    guard.reservation = Some(
        db::billing::reserve_request(
            &state.conn(),
            key.auth.user_id,
            key.auth.key_id,
            request_id,
            &uuid::Uuid::new_v4().to_string(),
            &route.price,
        )
        .map_err(|e| {
            if e.to_string().contains("budget") {
                ApiError::new(
                    StatusCode::PAYMENT_REQUIRED,
                    "budget_exhausted",
                    "Pass-through budget unavailable",
                )
            } else {
                ApiError::internal("Billing reservation unavailable")
            }
        })?,
    );
    state.conn().execute("UPDATE billing_ledger SET source_snapshot=?2 WHERE id=?1",rusqlite::params![guard.reservation,serde_json::json!({"kind":"passthrough","route_id":route.route.id,"revision":route.route.revision,"prefix":route.route.prefix,"upstream":route.route.upstream}).to_string()]).map_err(db::DbError::from)?;
    let upload_exceeded = Arc::new(AtomicBool::new(false));
    let upload_failed = upload_exceeded.clone();
    let upload = stream::try_unfold(
        (body.into_data_stream(), 0u64),
        move |(mut stream, used)| {
            let upload_failed = upload_failed.clone();
            async move {
                match stream.next().await {
                    None => Ok(None),
                    Some(Err(_)) => Err(io::Error::other("Request upload interrupted")),
                    Some(Ok(bytes)) => {
                        let next = used.saturating_add(bytes.len() as u64);
                        if next > limit {
                            upload_failed.store(true, Ordering::Release);
                            return Err(io::Error::other("Route upload limit exceeded"));
                        }
                        Ok(Some((bytes, (stream, next))))
                    }
                }
            }
        },
    );
    guard.dispatched = true;
    let response = match tokio::time::timeout_at(
        deadline,
        client()
            .request(parts.method.clone(), url)
            .headers(headers)
            .body(reqwest::Body::wrap_stream(upload))
            .send(),
    )
    .await
    {
        Err(_) => {
            guard.finish(None, "upstream_timeout");
            return Err(timeout());
        }
        Ok(Err(e)) => {
            guard.finish(
                if e.is_connect() { Some(0) } else { None },
                "upstream_error",
            );
            return Err(if upload_exceeded.load(Ordering::Acquire) {
                ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "body_too_large",
                    "Request exceeds route upload limit",
                )
            } else {
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "upstream_error",
                    "Pass-through upstream transport failed",
                )
            });
        }
        Ok(Ok(response)) => response,
    };
    let empty = response.content_length() == Some(0);
    let status = response.status();
    guard.http_status = Some(status.as_u16());
    let headers = filtered(response.headers(), false);
    let stream = response.bytes_stream();
    // Hyper need not poll body-less responses. Finish those at headers.
    let body = if parts.method == Method::HEAD
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
        || status == StatusCode::RESET_CONTENT
        || empty
    {
        guard.finish(Some(1), "completed");
        Body::empty()
    } else {
        // Independent, bounded pump enforces the total deadline even while a slow
        // downstream stops polling Body. It holds at most one queued HTTP chunk.
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let failed = Arc::new(AtomicBool::new(false));
        let writer_failed = failed.clone();
        tokio::spawn(async move {
            let mut stream = Box::pin(stream);
            loop {
                let item = tokio::select! {
                    biased;
                    _=sender.closed()=>{guard.finish(None,"canceled");break;}
                    _=tokio::time::sleep_until(deadline)=>{writer_failed.store(true,Ordering::Release);guard.finish(None,"upstream_timeout");break;}
                    item=stream.next()=>item,
                };
                match item {
                    Some(Ok(bytes)) => {
                        match tokio::time::timeout_at(deadline, sender.send(bytes)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => {
                                guard.finish(None, "canceled");
                                break;
                            }
                            Err(_) => {
                                writer_failed.store(true, Ordering::Release);
                                guard.finish(None, "upstream_timeout");
                                break;
                            }
                        }
                    }
                    Some(Err(_)) => {
                        writer_failed.store(true, Ordering::Release);
                        guard.finish(None, "upstream_read_error");
                        break;
                    }
                    None => {
                        guard.finish(Some(1), "completed");
                        break;
                    }
                }
            }
        });
        Body::from_stream(stream::unfold(
            (receiver, failed),
            |(mut receiver, failed)| async move {
                match receiver.recv().await {
                    Some(bytes) => Some((Ok::<_, io::Error>(bytes), (receiver, failed))),
                    None if failed.swap(false, Ordering::AcqRel) => Some((
                        Err(io::Error::other(
                            "Upstream stream interrupted or deadline exceeded",
                        )),
                        (receiver, failed),
                    )),
                    None => None,
                }
            },
        ))
    };
    let mut out = Response::new(body);
    *out.status_mut() = status;
    *out.headers_mut() = headers;
    Ok(out)
}
fn timeout() -> ApiError {
    ApiError::new(
        StatusCode::GATEWAY_TIMEOUT,
        "upstream_timeout",
        "Pass-through deadline exceeded",
    )
}

struct Guard {
    state: SharedState,
    reservation: Option<i64>,
    request_id: i64,
    channel_id: i64,
    model: String,
    started: i64,
    http_status: Option<u16>,
    dispatched: bool,
    done: bool,
    _admission: crate::admission::AdmissionPermit,
    _channel: crate::state::ChannelPermit,
}
impl Guard {
    fn finish(&mut self, units: Option<i64>, outcome: &str) {
        if self.done {
            return;
        }
        self.done = true;
        if let Some(id) = self.reservation.take() {
            if crate::billing_writer::settle_request(&self.state, id, units, outcome).is_err() {
                tracing::error!(
                    reservation_id = id,
                    "Pass-through settlement pending durable reconciliation"
                );
            }
        }
        let status = if units == Some(1) {
            if self.http_status.is_some_and(|s| s >= 500) {
                requests::Status::Failed
            } else if self.http_status.is_some_and(|s| s >= 400) {
                requests::Status::ClientError
            } else {
                requests::Status::Ok
            }
        } else if outcome == "canceled" {
            requests::Status::Canceled
        } else {
            requests::Status::Failed
        };
        let elapsed = (db::now_ms() - self.started).max(0);
        if self.dispatched {
            self.state
                .enqueue_write(crate::state::WriteJob::Attempt(requests::AttemptArgs {
                    model_name: self.model.clone(),
                    upstream_model: self.model.clone(),
                    first_byte_ms: None,
                    request_id: self.request_id,
                    ordinal: 1,
                    channel_id: self.channel_id,
                    outcome: outcome.into(),
                    http_status: self.http_status,
                    duration_ms: elapsed,
                    error: None,
                }));
        }
        self.state
            .enqueue_write(crate::state::WriteJob::Finish(requests::FinishArgs {
                input_tokens_known: false,
                output_tokens_known: false,
                usage_source: "unreported".into(),
                request_id: self.request_id,
                status,
                http_status: self.http_status,
                channel_id: Some(self.channel_id),
                latency_ms: elapsed,
                first_byte_ms: None,
                input_tokens: 0,
                output_tokens: 0,
                streamed: true,
                error: None,
                served_model: Some(self.model.clone()),
                error_code: if units == Some(1) {
                    None
                } else {
                    Some(outcome.into())
                },
            }));
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if !self.done {
            self.finish(if self.dispatched { None } else { Some(0) }, "canceled")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_fields_are_filtered_case_insensitively() {
        let mut h = HeaderMap::new();
        h.insert("connection", "X-Private, TE".parse().unwrap());
        h.insert("x-private", "secret".parse().unwrap());
        h.append("set-cookie", "a=1".parse().unwrap());
        h.append("set-cookie", "b=2".parse().unwrap());
        h.insert("content-encoding", "gzip".parse().unwrap());
        let out = filtered(&h, false);
        assert!(!out.contains_key("x-private"));
        assert!(!out.contains_key("connection"));
        assert_eq!(out.get_all("set-cookie").iter().count(), 2);
        assert_eq!(out["content-encoding"], "gzip");
    }
}
