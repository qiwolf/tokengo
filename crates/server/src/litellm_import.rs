//! Strict, root-only import of a documented JSON subset; never deserialize into Debug types.
use crate::{
    db::{
        self, channels,
        route_policy::{self, RoutePolicy},
        settings,
    },
    protocol::Protocol,
    protocol_config::{self, Capabilities},
    routes::{admin::PanelAuth, ApiError},
    SharedState,
};
use axum::{extract::State, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use tokengo_shared::{normalize_model_name, ChannelId, ModelKind};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/import/litellm/preview", post(preview))
        .route("/api/import/litellm/apply", post(apply))
}
#[derive(Serialize)]
struct Issue {
    path: String,
    code: &'static str,
}
struct Deployment {
    id: String,
    logical: String,
    upstream: String,
    base: String,
    secret: String,
    headers: Vec<(String, String)>,
    protocol: Protocol,
    kind: ModelKind,
}
struct Plan {
    key: String,
    deployments: Vec<Deployment>,
    routes: BTreeMap<String, Vec<usize>>,
    issues: Vec<Issue>,
}
fn issue(issues: &mut Vec<Issue>, path: impl Into<String>, code: &'static str) {
    issues.push(Issue {
        path: path.into(),
        code,
    });
}
fn unknown(v: &Value, allowed: &[&str], path: &str, issues: &mut Vec<Issue>) {
    if let Some(o) = v.as_object() {
        for (i, k) in o.keys().enumerate() {
            if !allowed.contains(&k.as_str()) {
                issue(issues, format!("{path}.field[{i}]"), "unsupported_field");
            }
        }
    } else {
        issue(issues, path, "object_required");
    }
}
fn text(v: &Value) -> Option<&str> {
    v.as_str()
        .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
}
fn env(v: &str) -> bool {
    v.starts_with("os.environ/") || v.starts_with("ENV/") || v.contains("${")
}
fn plan(v: &Value) -> Plan {
    let mut issues = vec![];
    unknown(
        v,
        &[
            "model_list",
            "litellm_settings",
            "router_settings",
            "import_options",
        ],
        "$",
        &mut issues,
    );
    let opts = &v["import_options"];
    unknown(
        opts,
        &["name", "routing_mode"],
        "import_options",
        &mut issues,
    );
    let name = text(&opts["name"]).filter(|s| {
        s.len() <= 80
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    });
    if name.is_none() {
        issue(
            &mut issues,
            "import_options.name",
            "stable_import_name_required",
        );
    }
    if opts["routing_mode"] != "ordered_fallback" {
        issue(
            &mut issues,
            "import_options.routing_mode",
            "explicit_ordered_fallback_acknowledgement_required",
        );
    }
    for section in ["litellm_settings", "router_settings"] {
        if let Some(s) = v.get(section) {
            unknown(s, &["fallbacks"], section, &mut issues);
        }
    }
    let mut deployments = vec![];
    let mut ids = BTreeSet::new();
    let Some(list) = v["model_list"].as_array() else {
        issue(&mut issues, "model_list", "nonempty_array_required");
        return Plan {
            key: String::new(),
            deployments,
            routes: BTreeMap::new(),
            issues,
        };
    };
    if list.is_empty() || list.len() > 256 {
        issue(
            &mut issues,
            "model_list",
            "deployment_count_1_to_256_required",
        );
    }
    for (i, d) in list.iter().take(256).enumerate() {
        let path = format!("model_list[{i}]");
        let start = issues.len();
        unknown(
            d,
            &["model_name", "litellm_params", "model_info"],
            &path,
            &mut issues,
        );
        let logical = text(&d["model_name"]).map(str::trim).filter(|s| {
            s.len() <= 256 && !normalize_model_name(s).is_empty() && !db::model_routes::is_auto(s)
        });
        if logical.is_none() {
            issue(
                &mut issues,
                format!("{path}.model_name"),
                "invalid_logical_model",
            );
        }
        let p = &d["litellm_params"];
        unknown(
            p,
            &["model", "api_base", "api_key", "http_headers"],
            &format!("{path}.litellm_params"),
            &mut issues,
        );
        let info = &d["model_info"];
        unknown(
            info,
            &["id", "mode"],
            &format!("{path}.model_info"),
            &mut issues,
        );
        let id = text(&info["id"]).filter(|s| s.len() <= 128);
        if id.is_none() || !ids.insert(id.unwrap_or("").to_string()) {
            issue(
                &mut issues,
                format!("{path}.model_info.id"),
                "unique_stable_deployment_id_required",
            );
        }
        let kind = match info["mode"].as_str() {
            Some("chat") => Some(ModelKind::Chat),
            Some("embedding") => Some(ModelKind::Embeddings),
            Some("rerank") => Some(ModelKind::Rerank),
            _ => None,
        };
        if kind.is_none() {
            issue(
                &mut issues,
                format!("{path}.model_info.mode"),
                "explicit_supported_mode_required",
            );
        }
        let model = text(&p["model"]).and_then(|s| s.split_once('/'));
        let (protocol, upstream) = match model {
            Some(("openai", m)) if !m.trim().is_empty() => (Some(Protocol::Chat), m),
            Some(("anthropic", m)) if !m.trim().is_empty() && kind == Some(ModelKind::Chat) => {
                (Some(Protocol::Messages), m)
            }
            // LiteLLM ollama uses its native /api protocol, which is not OpenAI /v1.
            Some(("ollama" | "ollama_chat", _)) => {
                issue(
                    &mut issues,
                    format!("{path}.litellm_params.model"),
                    "ollama_native_adapter_not_supported_use_explicit_openai_compatible_deployment",
                );
                (None, "")
            }
            _ => {
                issue(
                    &mut issues,
                    format!("{path}.litellm_params.model"),
                    "explicit_supported_provider_prefix_required",
                );
                (None, "")
            }
        };
        let base = text(&p["api_base"]);
        let valid_base = base
            .and_then(|s| reqwest::Url::parse(s).ok())
            .is_some_and(|u| {
                matches!(u.scheme(), "http" | "https")
                    && u.host_str().is_some()
                    && u.username().is_empty()
                    && u.password().is_none()
                    && u.query().is_none()
                    && u.fragment().is_none()
            });
        if !valid_base || base.is_some_and(env) {
            issue(
                &mut issues,
                format!("{path}.litellm_params.api_base"),
                "explicit_http_base_without_embedded_credentials_required",
            );
        }
        let secret = text(&p["api_key"]);
        if secret.is_none() || secret.is_some_and(env) {
            issue(
                &mut issues,
                format!("{path}.litellm_params.api_key"),
                "resolved_api_key_required",
            );
        }
        let mut headers = vec![];
        let mut header_names = BTreeSet::new();
        if let Some(h) = p.get("http_headers") {
            if let Some(map) = h.as_object() {
                for (n, value) in map {
                    let val = text(value);
                    if !header_names.insert(n.to_ascii_lowercase())
                        || axum::http::HeaderName::from_bytes(n.as_bytes()).is_err()
                        || val.is_none()
                        || val.is_some_and(env)
                        || val.is_some_and(|s| axum::http::HeaderValue::from_str(s).is_err())
                        || [
                            "host",
                            "content-length",
                            "transfer-encoding",
                            "connection",
                            "te",
                            "trailer",
                            "upgrade",
                            "keep-alive",
                            "proxy-authorization",
                            "proxy-authenticate",
                        ]
                        .iter()
                        .any(|x| n.eq_ignore_ascii_case(x))
                    {
                        issue(
                            &mut issues,
                            format!("{path}.litellm_params.http_headers"),
                            "invalid_or_unresolved_outbound_header",
                        );
                    } else {
                        headers.push((n.clone(), val.unwrap().to_string()));
                    }
                }
            } else {
                issue(
                    &mut issues,
                    format!("{path}.litellm_params.http_headers"),
                    "object_required",
                );
            }
        }
        if issues.len() == start {
            deployments.push(Deployment {
                id: id.unwrap().into(),
                logical: logical.unwrap().into(),
                upstream: upstream.into(),
                base: {
                    let b = base.unwrap().trim_end_matches('/');
                    if protocol == Some(Protocol::Messages) && !b.ends_with("/v1") {
                        format!("{b}/v1")
                    } else {
                        b.to_string()
                    }
                },
                secret: secret.unwrap().into(),
                headers,
                protocol: protocol.unwrap(),
                kind: kind.unwrap(),
            });
        }
    }
    let mut names: BTreeMap<String, (String, ModelKind, Protocol)> = BTreeMap::new();
    for d in &deployments {
        let norm = normalize_model_name(&d.logical);
        if let Some((name, k, p)) = names.get(&norm) {
            if name != &d.logical || k != &d.kind || p != &d.protocol {
                issue(
                    &mut issues,
                    "model_list",
                    "logical_name_collision_or_incompatible_deployments",
                );
            }
        } else {
            names.insert(norm, (d.logical.clone(), d.kind, d.protocol));
        }
    }
    let routes = expand_routes(v, &deployments, &mut issues);
    Plan {
        routes,
        key: format!(
            "litellm_import:v1:{}",
            hex::encode(Sha256::digest(name.unwrap_or("").as_bytes()))
        ),
        deployments,
        issues,
    }
}

fn expand_routes(
    v: &Value,
    ds: &[Deployment],
    issues: &mut Vec<Issue>,
) -> BTreeMap<String, Vec<usize>> {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, d) in ds.iter().enumerate() {
        groups.entry(d.logical.clone()).or_default().push(i);
    }
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let r = v["router_settings"].get("fallbacks");
    let l = v["litellm_settings"].get("fallbacks");
    if r.is_some() && l.is_some() {
        issue(issues, "fallbacks", "ambiguous_settings_locations");
    }
    if let Some(raw) = r.or(l) {
        if let Some(rows) = raw.as_array() {
            if rows.len() > 256 {
                issue(issues, "fallbacks", "too_many_entries");
            }
            for (i, row) in rows.iter().take(256).enumerate() {
                let path = format!("fallbacks[{i}]");
                let Some(map) = row.as_object().filter(|m| m.len() == 1) else {
                    issue(issues, &path, "single_model_mapping_required");
                    continue;
                };
                let (from, to) = map.iter().next().unwrap();
                if !groups.contains_key(from) || edges.contains_key(from) {
                    issue(issues, &path, "unknown_or_duplicate_primary_model");
                    continue;
                }
                let Some(targets) = to.as_array().filter(|t| !t.is_empty() && t.len() <= 256)
                else {
                    issue(issues, &path, "nonempty_target_array_required");
                    continue;
                };
                let mut selected = vec![];
                for target in targets {
                    let Some(t) = target.as_str().filter(|t| groups.contains_key(*t)) else {
                        issue(issues, &path, "unknown_fallback_target");
                        continue;
                    };
                    if t == from || selected.iter().any(|x| x == t) {
                        issue(issues, &path, "self_or_duplicate_fallback_target");
                        continue;
                    }
                    let first = &ds[groups[from][0]];
                    let next = &ds[groups[t][0]];
                    if first.kind != next.kind || first.protocol != next.protocol {
                        issue(issues, &path, "fallback_kind_or_native_protocol_mismatch");
                        continue;
                    }
                    selected.push(t.to_string());
                }
                edges.insert(from.clone(), selected);
            }
        } else {
            issue(issues, "fallbacks", "array_required");
        }
    }
    fn visit(
        name: &str,
        groups: &BTreeMap<String, Vec<usize>>,
        edges: &BTreeMap<String, Vec<String>>,
        stack: &mut BTreeSet<String>,
        out: &mut Vec<usize>,
    ) -> bool {
        if stack.contains(name) {
            return false;
        }
        if out.contains(&groups[name][0]) {
            return true;
        }
        stack.insert(name.to_string());
        for id in &groups[name] {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        if let Some(next) = edges.get(name) {
            for n in next {
                if !visit(n, groups, edges, stack, out) {
                    return false;
                }
            }
        }
        stack.remove(name);
        true
    }
    let mut routes = BTreeMap::new();
    for name in groups.keys() {
        let mut out = vec![];
        if !visit(name, &groups, &edges, &mut BTreeSet::new(), &mut out) {
            issue(issues, "fallbacks", "cycle_detected");
        }
        routes.insert(name.clone(), out);
    }
    routes
}

#[derive(Serialize, Deserialize, Default)]
struct Provenance {
    deployments: BTreeMap<String, i64>,
    policies: BTreeMap<String, RoutePolicy>,
    aliases: BTreeMap<String, Vec<String>>,
}
fn load(c: &rusqlite::Connection, key: &str) -> db::Result<Provenance> {
    match settings::get(c, key)? {
        Some(v) => serde_json::from_str(&v)
            .map_err(|_| db::DbError::Other("Invalid import provenance".into())),
        None => Ok(Provenance::default()),
    }
}
fn inspect(c: &rusqlite::Connection, p: &mut Plan) -> Provenance {
    let old = match load(c, &p.key) {
        Ok(v) => v,
        Err(_) => {
            issue(&mut p.issues, "database", "invalid_provenance");
            return Provenance::default();
        }
    };
    let existing = match channels::list(c) {
        Ok(v) => v,
        Err(_) => {
            issue(&mut p.issues, "database", "read_failed");
            return old;
        }
    };
    for (deployment_index, d) in p.deployments.iter().enumerate() {
        if let Some(id) = old.deployments.get(&d.id) {
            if let Some(source) = existing.iter().find(|r| r.id.0 == *id) {
                let prior = old.aliases.get(&d.id).cloned().unwrap_or_default();
                for (alias, indices) in &p.routes {
                    if indices.contains(&deployment_index)
                        && source.models.iter().any(|m| {
                            !prior.contains(&m.model)
                                && normalize_model_name(&m.model) == normalize_model_name(alias)
                        })
                    {
                        issue(&mut p.issues, "database", "manual_alias_collision");
                    }
                }
            }
            if !existing.iter().any(|r| r.id.0 == *id) {
                issue(&mut p.issues, "database", "managed_source_missing");
            }
        }
        for source in &existing {
            for m in &source.models {
                if normalize_model_name(&m.model) == normalize_model_name(&d.logical)
                    && !old.deployments.values().any(|id| *id == source.id.0)
                {
                    issue(
                        &mut p.issues,
                        "database",
                        "logical_model_conflicts_with_unmanaged_source",
                    );
                }
            }
        }
        let existing_policy = route_policy::load(c, &d.logical);
        match existing_policy {
            Ok(pol)
                if old
                    .policies
                    .get(&d.logical)
                    .map_or(pol != RoutePolicy::default(), |old| old != &pol) =>
            {
                issue(
                    &mut p.issues,
                    "database",
                    "logical_policy_changed_outside_import",
                )
            }
            Err(_) => issue(&mut p.issues, "database", "invalid_existing_policy"),
            _ => {}
        }
    }
    old
}
fn report(p: &Plan, old: &Provenance, applied: bool) -> Value {
    let retained = old
        .deployments
        .keys()
        .filter(|id| !p.deployments.iter().any(|d| &d.id == *id))
        .count();
    json!({"supported":p.issues.is_empty(),"applied":applied,"deployment_count":p.deployments.len(),"retained_removed_deployments":retained,"issues":p.issues,"semantics":{"routing":"ordered_fallback","retry_times":0,"probe_enabled":false,"include_in_auto":false,"protocol_conversion":false},"warnings":["Only this strict JSON subset is imported; YAML, environment resolution, arbitrary parameters and conditional fallbacks are unsupported.","Managed source configuration is updated by stable deployment ID; removed deployments and manual aliases are retained."]})
}
async fn preview(
    _root: PanelAuth,
    State(state): State<SharedState>,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(v) = body.map_err(|_| ApiError::bad_request("Invalid import JSON"))?;
    let mut p = plan(&v);
    let old = inspect(&state.conn(), &mut p);
    Ok(Json(report(&p, &old, false)))
}
async fn apply(
    _root: PanelAuth,
    State(state): State<SharedState>,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(v) = body.map_err(|_| ApiError::bad_request("Invalid import JSON"))?;
    let mut p = plan(&v);
    let conn = state.conn();
    let mut old = inspect(&conn, &mut p);
    if !p.issues.is_empty() {
        return Ok(Json(report(&p, &old, false)));
    }
    let outcome = db::in_tx(&conn, |c| {
        for (deployment_index, d) in p.deployments.iter().enumerate() {
            let id = if let Some(id) = old.deployments.get(&d.id) {
                let id = ChannelId(*id);
                let row = channels::get(c, id)?
                    .ok_or_else(|| db::DbError::Other("Missing source".into()))?;
                channels::update(
                    c,
                    id,
                    &row.name,
                    &d.base,
                    row.enabled,
                    row.unwrap_envelope,
                    row.max_concurrent,
                )?;
                id
            } else {
                let id = channels::create(
                    c,
                    &channels::NewChannel {
                        name: format!(
                            "LiteLLM {} source {}",
                            &p.key[p.key.len() - 8..],
                            old.deployments.len() + 1
                        ),
                        base_url: d.base.clone(),
                        enabled: true,
                        unwrap_envelope: false,
                        max_concurrent: 0,
                    },
                )?;
                old.deployments.insert(d.id.clone(), id.0);
                id
            };
            channels::set_secret(c, &state.secrets, id, &d.secret)?;
            channels::set_headers(c, &state.secrets, id, &d.headers)?;
            let mut models = channels::get(c, id)?.unwrap().models;
            let prior = old.aliases.get(&d.id).cloned().unwrap_or_default();
            models.retain(|m| !prior.contains(&m.model));
            let aliases: Vec<String> = p
                .routes
                .iter()
                .filter(|(_, indices)| indices.contains(&deployment_index))
                .map(|(name, _)| name.clone())
                .collect();
            for alias in &aliases {
                if models
                    .iter()
                    .any(|m| normalize_model_name(&m.model) == normalize_model_name(alias))
                {
                    return Err(db::DbError::Other("Manual alias collision".into()));
                }
                models.push(channels::ModelMapping::new(
                    alias,
                    Some(d.upstream.clone()),
                    false,
                ));
            }
            channels::set_models(c, id, &models)?;
            old.aliases.insert(d.id.clone(), aliases);
            protocol_config::save(
                c,
                id.0,
                &Capabilities {
                    parameter_profile: None,
                    runtime_identity: None,
                    native: vec![d.protocol],
                    allow_conversion: false,
                    native_input_audio: false,
                    chat_reasoning_replay: false,
                    max_input_tokens: None,
                    max_output_tokens: None,
                },
            )?;
            db::kinds::set(c, &d.logical, d.kind)?;
        }
        for (name, indices) in &p.routes {
            let ids: Vec<i64> = indices
                .iter()
                .map(|i| old.deployments[&p.deployments[*i].id])
                .collect();
            let pol = RoutePolicy {
                channel_ids: Some(ids),
                retry_times: Some(0),
                probe_enabled: Some(false),
                include_in_auto: Some(false),
                ..Default::default()
            };
            route_policy::save(c, &name, &pol)?;
            let reset = Default::default();
            db::routing::save_state(c, &name, &reset)?;
            old.policies.insert(name.clone(), pol);
        }
        settings::set(
            c,
            &p.key,
            &serde_json::to_string(&old)
                .map_err(|_| db::DbError::Other("Encoding failed".into()))?,
        )?;
        db::identity::audit(
            c,
            None,
            "litellm_import.apply",
            "import",
            0,
            json!({"deployment_count":p.deployments.len()}),
        )?;
        Ok(())
    });
    if outcome.is_err() {
        return Err(ApiError::internal("Import failed; no changes applied"));
    }
    drop(conn);
    for d in &p.deployments {
        state.routing().restore(&d.logical, Default::default());
    }
    Ok(Json(report(&p, &old, true)))
}
