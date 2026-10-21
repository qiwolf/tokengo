//! Opaque HTTP routes. Credentials are encrypted independently of LLM sources.
use super::{
    DbError, Result,
    billing::RequestPrice,
    identity::{self, AuthContext},
};
use crate::crypto::SecretBox;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}
fn timeout() -> u32 {
    60
}
fn concurrent() -> u32 {
    16
}
fn body_limit() -> u64 {
    20 * 1024 * 1024
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderInput {
    pub name: String,
    pub value: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteInput {
    pub name: String,
    pub prefix: String,
    pub upstream: String,
    #[serde(default = "yes")]
    pub strip_prefix: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub price_micro_usd_per_request: i64,
    #[serde(default = "timeout")]
    pub timeout_secs: u32,
    #[serde(default = "concurrent")]
    pub max_concurrent: u32,
    #[serde(default = "body_limit")]
    pub max_body_bytes: u64,
    #[serde(default)]
    pub headers: Vec<HeaderInput>,
    #[serde(default)]
    pub allowed_key_ids: Vec<i64>,
    #[serde(default)]
    pub allowed_paths: Option<Vec<String>>,
    #[serde(default)]
    pub revision: Option<i64>,
}
#[derive(Clone, Debug, Serialize)]
pub struct HeaderSummary {
    pub name: String,
    pub has_value: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct Route {
    pub id: i64,
    pub revision: i64,
    pub channel_id: i64,
    pub name: String,
    pub prefix: String,
    pub upstream: String,
    pub strip_prefix: bool,
    pub enabled: bool,
    pub price_micro_usd_per_request: i64,
    pub timeout_secs: u32,
    pub max_concurrent: u32,
    pub max_body_bytes: u64,
    pub headers: Vec<HeaderSummary>,
    pub allowed_key_ids: Vec<i64>,
    pub allowed_paths: Option<Vec<String>>,
}
// Deliberately no Debug/Serialize: decrypted credentials must not enter logs or API JSON.
pub struct RuntimeRoute {
    pub route: Route,
    pub headers: Vec<(String, String)>,
    pub price: RequestPrice,
}
fn bad(s: &str) -> DbError {
    DbError::Other(s.into())
}
pub fn validate(v: &RouteInput) -> Result<()> {
    if v.name.trim().is_empty() || v.name.len() > 128 || v.name.chars().any(char::is_control) {
        return Err(bad("invalid route name"));
    }
    let p = &v.prefix;
    if !p.is_ascii()
        || !p.starts_with('/')
        || !p.ends_with('/')
        || p.len() < 3
        || p.len() > 256
        || p.contains(['%', '?', '#', '\\'])
        || p.chars().any(|c| c.is_control() || c.is_whitespace())
        || p.split('/').any(|s| s == "." || s == "..")
        || p.contains("//")
    {
        return Err(bad("prefix must be a literal absolute path ending in /"));
    }
    let first = p.split('/').nth(1).unwrap_or("");
    if [
        "api",
        "v1",
        "ui",
        "health",
        "healthz",
        "livez",
        "readyz",
        "metrics",
        "status",
        "assets",
        "oauth",
        "auth",
        "login",
        "logout",
        "favicon.ico",
    ]
    .contains(&first)
    {
        return Err(bad("prefix overlaps a reserved namespace"));
    }
    let u = reqwest::Url::parse(&v.upstream).map_err(|_| bad("invalid upstream URL"))?;
    if !["http", "https"].contains(&u.scheme())
        || u.host_str().is_none()
        || v.upstream
            .split_once("://")
            .is_some_and(|(_, rest)| rest.split('/').next().unwrap_or("").contains('@'))
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || v.upstream.contains('\\')
        || v.upstream
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        || !v.upstream.contains("://")
        || v.upstream
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('/'))
            .is_some_and(|(_, path)| {
                path.to_ascii_lowercase()
                    .replace("%2e", ".")
                    .split('/')
                    .any(|s| s == "." || s == "..")
            })
    {
        return Err(bad(
            "upstream must be an HTTP(S) URL without userinfo, query or fragment",
        ));
    }
    if v.price_micro_usd_per_request < 0
        || !(1..=3600).contains(&v.timeout_secs)
        || !(1..=1024).contains(&v.max_concurrent)
        || !(1..=100 * 1024 * 1024).contains(&v.max_body_bytes)
    {
        return Err(bad(
            "invalid route price, timeout, concurrency or body limit",
        ));
    }
    if v.headers.len() > 64 || v.allowed_key_ids.len() > 256 {
        return Err(bad("too many headers or key grants"));
    }
    let mut seen = std::collections::HashSet::new();
    for h in &v.headers {
        if h.name.len() > 256 {
            return Err(bad("invalid upstream header name"));
        }
        let n = axum::http::HeaderName::from_bytes(h.name.as_bytes())
            .map_err(|_| bad("invalid upstream header name"))?;
        if [
            "host",
            "content-length",
            "transfer-encoding",
            "connection",
            "proxy-authorization",
            "proxy-connection",
            "proxy-authenticate",
            "keep-alive",
            "te",
            "trailer",
            "upgrade",
        ]
        .contains(&n.as_str())
            || !seen.insert(n.to_string())
        {
            return Err(bad("duplicate or reserved upstream header"));
        }
        if h.value
            .as_ref()
            .is_some_and(|s| s.len() > 16 * 1024 || axum::http::HeaderValue::from_str(s).is_err())
        {
            return Err(bad("invalid upstream header value"));
        }
    }
    let mut keys = std::collections::HashSet::new();
    if v.allowed_key_ids
        .iter()
        .any(|id| *id <= 0 || !keys.insert(*id))
    {
        return Err(bad("key grants must be distinct positive IDs"));
    }
    if let Some(paths) = &v.allowed_paths {
        let mut seen = std::collections::HashSet::new();
        if paths.len() > 256
            || paths.iter().any(|p| {
                !p.is_ascii()
                    || !p.starts_with('/')
                    || p.len() > 1024
                    || p.contains(['%', '?', '#', '\\'])
                    || p.contains("//")
                    || p.chars().any(|c| c.is_whitespace() || c.is_control())
                    || p.split('/').any(|s| s == "." || s == "..")
                    || !seen.insert(p)
            })
        {
            return Err(bad(
                "allowed_paths must contain distinct literal absolute paths",
            ));
        }
    }
    Ok(())
}
const SELECT: &str = "SELECT r.id,r.revision,r.channel_id,r.name,r.prefix,r.upstream,r.strip_prefix,r.enabled,p.micro_per_call,r.timeout_secs,r.max_concurrent,r.max_body_bytes,r.allowed_paths_json FROM passthrough_routes r JOIN billing_request_prices p ON p.id=r.price_id";
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Route> {
    let paths: Option<String> = r.get(12)?;
    Ok(Route {
        id: r.get(0)?,
        revision: r.get(1)?,
        channel_id: r.get(2)?,
        name: r.get(3)?,
        prefix: r.get(4)?,
        upstream: r.get(5)?,
        strip_prefix: r.get(6)?,
        enabled: r.get(7)?,
        price_micro_usd_per_request: r.get(8)?,
        timeout_secs: r.get(9)?,
        max_concurrent: r.get(10)?,
        max_body_bytes: r.get::<_, i64>(11)? as u64,
        headers: vec![],
        allowed_key_ids: vec![],
        allowed_paths: paths
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    12,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
    })
}
fn children(c: &Connection, r: &mut Route) -> Result<()> {
    r.headers = c
        .prepare("SELECT name FROM passthrough_headers WHERE route_id=?1 ORDER BY name")?
        .query_map([r.id], |q| {
            Ok(HeaderSummary {
                name: q.get(0)?,
                has_value: true,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    r.allowed_key_ids = c
        .prepare("SELECT key_id FROM passthrough_acl WHERE route_id=?1 ORDER BY key_id")?
        .query_map([r.id], |q| q.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(())
}
pub fn get(c: &Connection, id: i64) -> Result<Option<Route>> {
    let mut r = c
        .query_row(&format!("{SELECT} WHERE r.id=?1"), [id], row)
        .optional()?;
    if let Some(r) = &mut r {
        children(c, r)?
    }
    Ok(r)
}
pub fn list(c: &Connection) -> Result<Vec<Route>> {
    let mut rows = c
        .prepare(&format!("{SELECT} ORDER BY r.prefix"))?
        .query_map([], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for r in &mut rows {
        children(c, r)?
    }
    Ok(rows)
}
fn save_children(c: &Connection, sb: &SecretBox, id: i64, v: &RouteInput) -> Result<()> {
    for key in &v.allowed_key_ids {
        if identity::context(c, *key)?.is_none() {
            return Err(bad("granted key is unknown or inactive"));
        }
    }
    let mut old = std::collections::HashMap::new();
    {
        let mut q =
            c.prepare("SELECT name,nonce,cipher FROM passthrough_headers WHERE route_id=?1")?;
        for r in q.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })? {
            let (n, a, b) = r?;
            old.insert(n, (a, b));
        }
    }
    c.execute("DELETE FROM passthrough_headers WHERE route_id=?1", [id])?;
    for h in &v.headers {
        let name = h.name.to_ascii_lowercase();
        let (nonce, cipher) = match &h.value {
            Some(s) => sb.seal(s).map_err(|_| bad("header encryption failed"))?,
            None => old
                .remove(&name)
                .ok_or_else(|| bad("new upstream header requires a value"))?,
        };
        c.execute(
            "INSERT INTO passthrough_headers(route_id,name,nonce,cipher) VALUES(?1,?2,?3,?4)",
            params![id, name, nonce, cipher],
        )?;
    }
    c.execute("DELETE FROM passthrough_acl WHERE route_id=?1", [id])?;
    for key in &v.allowed_key_ids {
        c.execute(
            "INSERT INTO passthrough_acl VALUES(?1,?2)",
            params![id, key],
        )?;
    }
    Ok(())
}
fn price(c: &Connection, channel: i64, model: &str, micro: i64) -> Result<i64> {
    c.execute("INSERT INTO billing_request_prices(channel_id,model,micro_per_call,created_ms) VALUES(?1,?2,?3,?4)",params![channel,model,micro,super::now_ms()])?;
    let id = c.last_insert_rowid();
    c.execute("INSERT INTO billing_audit(event,actor,object,detail,created_ms) VALUES('request_price_created','root_panel',?1,?2,?3)",params![id.to_string(),serde_json::json!({"id":id,"channel_id":channel,"model":model,"micro_per_call":micro}).to_string(),super::now_ms()])?;
    Ok(id)
}
fn audit_metadata(c: &Connection, revision: i64) -> Result<serde_json::Value> {
    let root_id = super::accounts::root(c)?.map(|a| a.account.id);
    Ok(
        serde_json::json!({"revision":revision,"actor_type":"root_panel","actor_account_id":root_id}),
    )
}
pub fn create(c: &Connection, sb: &SecretBox, v: &RouteInput) -> Result<Route> {
    validate(v)?;
    super::in_tx(c, |c| {
        c.execute("INSERT INTO channels(name,base_url,enabled,max_concurrent,transport_kind) VALUES(?1,?2,?3,?4,'passthrough')",params![v.name,v.upstream,v.enabled,v.max_concurrent])?;
        let channel = c.last_insert_rowid();
        // Route identity is reserved inside the same transaction, before versioned pricing is written.
        let id:i64=c.query_row("SELECT MAX(COALESCE((SELECT MAX(id) FROM passthrough_routes),0),COALESCE((SELECT seq FROM sqlite_sequence WHERE name='passthrough_routes'),0))+1",[],|r|r.get(0))?;
        let pid = price(
            c,
            channel,
            &format!("passthrough:{id}"),
            v.price_micro_usd_per_request,
        )?;
        c.execute("INSERT INTO passthrough_routes(id,channel_id,name,prefix,upstream,strip_prefix,enabled,price_id,timeout_secs,max_concurrent,max_body_bytes,allowed_paths_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![id,channel,v.name.trim(),v.prefix,v.upstream,v.strip_prefix,v.enabled,pid,v.timeout_secs,v.max_concurrent,v.max_body_bytes as i64,v.allowed_paths.as_ref().map(|p|serde_json::to_string(p).unwrap())])?;
        save_children(c, sb, id, v)?;
        identity::audit(
            c,
            None,
            "passthrough.created",
            "passthrough_route",
            id,
            audit_metadata(c, 1)?,
        )?;
        get(c, id)?.ok_or_else(|| bad("route creation failed"))
    })
}
pub fn update(
    c: &Connection,
    sb: &SecretBox,
    id: i64,
    revision: i64,
    v: &RouteInput,
) -> Result<Route> {
    validate(v)?;
    super::in_tx(c, |c| {
        let old = get(c, id)?.ok_or_else(|| bad("route not found"))?;
        if old.revision != revision {
            return Err(bad("route revision conflict"));
        }
        let pid = if old.price_micro_usd_per_request != v.price_micro_usd_per_request {
            price(
                c,
                old.channel_id,
                &format!("passthrough:{id}"),
                v.price_micro_usd_per_request,
            )?
        } else {
            c.query_row(
                "SELECT price_id FROM passthrough_routes WHERE id=?1",
                [id],
                |r| r.get(0),
            )?
        };
        c.execute("UPDATE passthrough_routes SET revision=revision+1,name=?2,prefix=?3,upstream=?4,strip_prefix=?5,enabled=?6,price_id=?7,timeout_secs=?8,max_concurrent=?9,max_body_bytes=?10,allowed_paths_json=?11 WHERE id=?1",params![id,v.name.trim(),v.prefix,v.upstream,v.strip_prefix,v.enabled,pid,v.timeout_secs,v.max_concurrent,v.max_body_bytes as i64,v.allowed_paths.as_ref().map(|p|serde_json::to_string(p).unwrap())])?;
        c.execute("UPDATE channels SET name=?2,base_url=?3,enabled=?4,max_concurrent=?5 WHERE id=?1 AND transport_kind='passthrough'",params![old.channel_id,v.name.trim(),v.upstream,v.enabled,v.max_concurrent])?;
        save_children(c, sb, id, v)?;
        identity::audit(
            c,
            None,
            "passthrough.updated",
            "passthrough_route",
            id,
            audit_metadata(c, revision + 1)?,
        )?;
        get(c, id)?.ok_or_else(|| bad("route update failed"))
    })
}
pub fn delete(c: &Connection, id: i64, revision: i64) -> Result<()> {
    super::in_tx(c, |c| {
        let old = get(c, id)?.ok_or_else(|| bad("route not found"))?;
        if old.revision != revision {
            return Err(bad("route revision conflict"));
        }
        c.execute("DELETE FROM passthrough_routes WHERE id=?1", [id])?;
        c.execute(
            "DELETE FROM channels WHERE id=?1 AND transport_kind='passthrough'",
            [old.channel_id],
        )?;
        identity::audit(
            c,
            None,
            "passthrough.deleted",
            "passthrough_route",
            id,
            audit_metadata(c, revision)?,
        )?;
        Ok(())
    })
}
pub fn match_route(c: &Connection, sb: &SecretBox, path: &str) -> Result<Option<RuntimeRoute>> {
    let Some(mut r)=c.query_row(&format!("{SELECT} WHERE substr(?1,1,length(r.prefix))=r.prefix ORDER BY length(r.prefix) DESC LIMIT 1"),[path],row).optional()? else{return Ok(None)};
    children(c, &mut r)?;
    let mut headers = vec![];
    let mut q = c.prepare(
        "SELECT name,nonce,cipher FROM passthrough_headers WHERE route_id=?1 ORDER BY name",
    )?;
    for h in q.query_map([r.id], |q| {
        Ok((
            q.get::<_, String>(0)?,
            q.get::<_, Vec<u8>>(1)?,
            q.get::<_, Vec<u8>>(2)?,
        ))
    })? {
        let (n, a, b) = h?;
        headers.push((
            n,
            sb.open(&a, &b)
                .map_err(|_| bad("header decryption failed"))?,
        ));
    }
    let price=c.query_row("SELECT p.id,p.channel_id,p.model,p.micro_per_call FROM billing_request_prices p JOIN passthrough_routes r ON r.price_id=p.id WHERE r.id=?1",[r.id],|r|Ok(RequestPrice{id:r.get(0)?,channel_id:r.get(1)?,model:r.get(2)?,micro_per_call:r.get(3)?}))?;
    Ok(Some(RuntimeRoute {
        route: r,
        headers,
        price,
    }))
}
pub fn authorize(c: &Connection, r: &RuntimeRoute, auth: &AuthContext) -> Result<bool> {
    let Some(current) = identity::context(c, auth.key_id)? else {
        return Ok(false);
    };
    Ok(current.user_id == auth.user_id
        && r.route.enabled
        && current.permits_channel(r.route.channel_id, r.price.micro_per_call > 0)
        && r.route
            .allowed_key_ids
            .iter()
            .any(|k| *k == current.key_id || current.ancestor_key_ids.contains(k)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(prefix: &str) -> RouteInput {
        serde_json::from_value(serde_json::json!({"name":"Face","prefix":prefix,"upstream":"http://127.0.0.1:8066","headers":[{"name":"api-key","value":"secret-test-marker"}],"enabled":true})).unwrap()
    }
    fn setup() -> (Connection, SecretBox) {
        (
            super::super::open_in_memory().unwrap(),
            SecretBox::from_hex(&"ab".repeat(32)).unwrap(),
        )
    }
    #[test]
    fn validates_reserved_names_headers_and_paths() {
        for prefix in [
            "/",
            "/api/",
            "/v1/",
            "/ui/",
            "/face",
            "/face/../",
            "/f%61ce/",
        ] {
            assert!(validate(&input(prefix)).is_err(), "{prefix}")
        }
        let mut v = input("/face/");
        for header in [
            "connection",
            "proxy-connection",
            "transfer-encoding",
            "te",
            "trailer",
            "upgrade",
            "keep-alive",
            "host",
            "content-length",
            "proxy-authorization",
            "proxy-authenticate",
        ] {
            v.headers[0].name = header.into();
            assert!(validate(&v).is_err(), "{header}");
        }
        v.headers[0].name = "api-key".into();
        for upstream in [
            "http://user:secret@localhost/",
            "http://@localhost/",
            "http://localhost/a/../b",
            "http://localhost/%2E%2e/b",
            "http://localhost/?token=secret",
            "http://localhost/#secret",
            "http://localhost/with space",
        ] {
            v.upstream = upstream.into();
            assert!(validate(&v).is_err(), "{upstream}");
        }
        v.upstream = "http://127.0.0.1:8066".into();
        v.headers.clear();
        v.allowed_paths = Some(vec!["/restart/../represent".into()]);
        assert!(validate(&v).is_err());
        v.allowed_paths = Some(vec!["/represent".into()]);
        assert!(validate(&v).is_ok());
    }
    #[test]
    fn credentials_encrypted_no_echo_kept_and_removed_atomically() {
        let (c, sb) = setup();
        let mut v = input("/face/");
        let r = create(&c, &sb, &v).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("secret-test-marker"));
        let cipher: Vec<u8> = c
            .query_row("SELECT cipher FROM passthrough_headers", [], |r| r.get(0))
            .unwrap();
        assert!(!String::from_utf8_lossy(&cipher).contains("secret-test-marker"));
        v.headers[0].value = None;
        let r = update(&c, &sb, r.id, 1, &v).unwrap();
        let runtime = match_route(&c, &sb, "/face/represent").unwrap().unwrap();
        assert_eq!(runtime.headers[0].1, "secret-test-marker");
        assert!(update(&c, &sb, r.id, 1, &v).is_err());
        v.headers.clear();
        update(&c, &sb, r.id, 2, &v).unwrap();
        assert!(
            match_route(&c, &sb, "/face/represent")
                .unwrap()
                .unwrap()
                .headers
                .is_empty()
        );
    }
    #[test]
    fn longest_disabled_route_does_not_fall_back_and_internal_channels_are_hidden() {
        let (c, sb) = setup();
        create(&c, &sb, &input("/face/")).unwrap();
        let mut v = input("/face/private/");
        v.enabled = false;
        let r = create(&c, &sb, &v).unwrap();
        assert_eq!(
            match_route(&c, &sb, "/face/private/a")
                .unwrap()
                .unwrap()
                .route
                .id,
            r.id
        );
        assert!(!r.enabled);
        assert!(super::super::channels::list(&c).unwrap().is_empty());
        assert!(
            super::super::channels::get(&c, tokengo_shared::ChannelId(r.channel_id))
                .unwrap()
                .is_none()
        );
        assert!(
            super::super::channels::delete(&c, tokengo_shared::ChannelId(r.channel_id)).is_err()
        );
        super::super::provider_pool::backfill(&c, &sb).unwrap();
        let n: i64 = c
            .query_row("SELECT count(*) FROM provider_channels", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
    #[test]
    fn grants_default_deny_descendants_recheck_revocation() {
        let (c, sb) = setup();
        let policy = identity::Policy::default();
        let (parent, _) = identity::create_key(&c, 1, "parent", &policy, None, None).unwrap();
        let (child, _) =
            identity::create_key(&c, 1, "child", &policy, None, Some(parent.id)).unwrap();
        let auth = identity::context(&c, child.id).unwrap().unwrap();
        let mut v = input("/face/");
        let r = create(&c, &sb, &v).unwrap();
        assert!(
            !authorize(
                &c,
                &match_route(&c, &sb, "/face/a").unwrap().unwrap(),
                &auth
            )
            .unwrap()
        );
        v.allowed_key_ids = vec![parent.id];
        update(&c, &sb, r.id, 1, &v).unwrap();
        assert!(
            authorize(
                &c,
                &match_route(&c, &sb, "/face/a").unwrap().unwrap(),
                &auth
            )
            .unwrap()
        );
        c.execute("UPDATE gateway_keys SET enabled=0 WHERE id=?1", [parent.id])
            .unwrap();
        assert!(
            !authorize(
                &c,
                &match_route(&c, &sb, "/face/a").unwrap().unwrap(),
                &auth
            )
            .unwrap()
        );
    }
    #[test]
    fn prices_immutable_ids_not_reused_and_invalid_grants_rollback() {
        let (c, sb) = setup();
        let mut v = input("/face/");
        let r = create(&c, &sb, &v).unwrap();
        v.price_micro_usd_per_request = 50;
        let updated = update(&c, &sb, r.id, 1, &v).unwrap();
        let n: i64 = c
            .query_row("SELECT count(*) FROM billing_request_prices", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 2);
        delete(&c, r.id, updated.revision).unwrap();
        let next = create(&c, &sb, &v).unwrap();
        assert!(next.id > r.id);
        let count = list(&c).unwrap().len();
        v.prefix = "/clip/".into();
        v.allowed_key_ids = vec![999999];
        assert!(create(&c, &sb, &v).is_err());
        assert_eq!(list(&c).unwrap().len(), count);
    }
    #[test]
    fn internal_sources_reject_llm_setters_pricing_and_machine_writes() {
        let (c, sb) = setup();
        let route = create(&c, &sb, &input("/face/")).unwrap();
        let id = tokengo_shared::ChannelId(route.channel_id);
        assert!(super::super::channels::set_secret(&c, &sb, id, "no").is_err());
        assert!(super::super::channels::clear_secret(&c, id).is_err());
        assert!(super::super::channels::set_headers(&c, &sb, id, &[]).is_err());
        assert!(super::super::channels::set_models(&c, id, &[]).is_err());
        assert!(
            super::super::billing::create_request_price(
                &c,
                &RequestPrice {
                    id: 0,
                    channel_id: id.0,
                    model: format!("passthrough:{}", route.id),
                    micro_per_call: 0
                },
                "test"
            )
            .is_err()
        );
        super::super::accounts::sync_root_hash(&c, "test-hash").unwrap();
        let root = super::super::accounts::root(&c).unwrap().unwrap();
        let ops =
            super::super::operations_tokens::create(&c, "agent", 90, root.account.id).unwrap();
        crate::operations_guard::install(&c).unwrap();
        c.execute(
            "UPDATE temp.operations_request_context SET token_id=?1 WHERE singleton=1",
            [ops.token.id],
        )
        .unwrap();
        assert!(
            c.execute(
                "UPDATE passthrough_routes SET enabled=0 WHERE id=?1",
                [route.id]
            )
            .is_err()
        );
        assert!(
            c.execute(
                "DELETE FROM passthrough_headers WHERE route_id=?1",
                [route.id]
            )
            .is_err()
        );
        assert!(
            c.execute(
                "INSERT INTO passthrough_acl(route_id,key_id) VALUES(?1,99)",
                [route.id]
            )
            .is_err()
        );
        assert!(
            c.execute("UPDATE channels SET enabled=0 WHERE id=?1", [id.0])
                .is_err()
        );
    }
}
