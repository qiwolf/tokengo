//! Canonical provider endpoints and credential identities; immutable legacy source IDs survive.
use super::{channels, now_ms, DbError, Result};
use crate::crypto::SecretBox;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tokengo_shared::ChannelId;
fn bad(s: impl Into<String>) -> DbError {
    DbError::Other(s.into())
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CredentialRef {
    pub id: i64,
    pub provider_id: i64,
    pub enabled: bool,
    pub priority: i64,
    pub max_concurrent: u32,
    pub key_strategy: String,
    pub switch_on_429: bool,
    pub switch_on_400: bool,
    pub revision: i64,
    pub provider_version: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AffinityToken {
    pub credential_id: i64,
    pub generation: i64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Credential {
    pub id: i64,
    pub provider_id: i64,
    pub label: String,
    pub kind: String,
    pub enabled: bool,
    pub priority: i64,
    pub max_concurrent: u32,
    pub source_ids: Vec<i64>,
    pub has_secret: bool,
    pub oauth_account_id: Option<i64>,
    pub revision: i64,
    pub models: Vec<CredentialModel>,
    pub tools: Vec<CredentialTools>,
}
#[derive(Clone, Debug, Serialize)]
pub struct CredentialTools {
    pub source_id: i64,
    pub capabilities: crate::search_tools::ToolCapabilities,
}
#[derive(Clone, Debug, Serialize)]
pub struct CredentialModel {
    pub model: String,
    pub upstream: Option<String>,
    pub enabled: bool,
    pub vision: bool,
    pub drop_fields: String,
    pub conflict: bool,
}
pub fn canonical_url(raw: &str) -> Result<String> {
    let value = raw.trim();
    let u = reqwest::Url::parse(value).map_err(|_| bad("API URL必须是完整HTTP(S)地址"))?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || value.contains('\\')
    {
        return Err(bad("API URL不得含凭据、查询参数或片段"));
    }
    Ok(u.to_string().trim_end_matches('/').to_owned())
}
pub fn validate_strategy(s: &str) -> Result<()> {
    if matches!(s, "ordered" | "concurrent") {
        Ok(())
    } else {
        Err(bad("Key策略只能是ordered或concurrent"))
    }
}
fn auth_identity(
    c: &Connection,
    sb: &SecretBox,
    row: &channels::ChannelRow,
) -> Result<(String, String, Option<i64>)> {
    let account: Option<i64> = c
        .query_row(
            "SELECT account_id FROM oauth_channel_bindings WHERE channel_id=?1",
            [row.id.0],
            |r| r.get(0),
        )
        .optional()?;
    let secret = channels::secret(c, sb, row.id)?;
    let mut headers = channels::headers(c, sb, row.id)?;
    headers.sort();
    let kind = if account.is_some() {
        "oauth"
    } else if secret.is_some() || !headers.is_empty() {
        "api_key"
    } else {
        "anonymous"
    };
    let raw = serde_json::to_vec(&(
        kind,
        account,
        &secret,
        &headers,
        if kind == "anonymous" {
            Some(row.id.0)
        } else {
            None
        },
    ))
    .map_err(|_| bad("Credential identity invalid"))?;
    Ok((kind.into(), hex::encode(Sha256::digest(raw)), account))
}
fn adapter(c: &Connection, id: i64) -> Result<String> {
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings WHERE channel_id=?1)",
        [id],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok("oauth".into());
    }
    let tools = crate::search_tools::load(c, id)?;
    Ok(if tools.search || tools.fetch {
        "tools"
    } else {
        "api"
    }
    .into())
}
fn unique_name(c: &Connection, name: &str, source: i64) -> Result<String> {
    let mut value = name.chars().take(112).collect::<String>();
    let mut n = 0;
    while c.query_row(
        "SELECT EXISTS(SELECT 1 FROM provider_channels WHERE name=?1 COLLATE NOCASE)",
        [&value],
        |r| r.get::<_, bool>(0),
    )? {
        n += 1;
        value = format!(
            "{} · #{source}-{n}",
            name.chars().take(100).collect::<String>()
        );
    }
    Ok(value)
}
/// Master-controlled, idempotent backfill. No source URL/secret/priority/generation is rewritten.
pub fn backfill(c: &Connection, sb: &SecretBox) -> Result<()> {
    super::in_tx(c, |c| {
        let rows = channels::list(c)?;
        let mut old = Vec::<(i64, String, bool)>::new();
        {
            let mut q = c.prepare("SELECT id,name,enabled FROM provider_channels ORDER BY id")?;
            for row in q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
                old.push(row?);
            }
        }
        for (pid, name, enabled) in old {
            let members = rows
                .iter()
                .filter(|r| r.provider_id == Some(pid))
                .collect::<Vec<_>>();
            let mut groups: Vec<(String, String, Vec<&channels::ChannelRow>)> = vec![];
            for row in members {
                let url = canonical_url(&row.base_url).ok();
                let ad = adapter(c, row.id.0).unwrap_or_else(|_| "attention".into());
                let key = url
                    .clone()
                    .unwrap_or_else(|| format!("invalid:{}", row.id.0));
                if let Some(g) = groups.iter_mut().find(|g| g.0 == key && g.1 == ad) {
                    g.2.push(row);
                } else {
                    groups.push((key, ad, vec![row]));
                }
            }
            for (idx, (url, ad, members)) in groups.into_iter().enumerate() {
                let canonical = !url.starts_with("invalid:") && ad != "attention";
                let target = if idx == 0 {
                    pid
                } else {
                    let suffix = reqwest::Url::parse(&url)
                        .ok()
                        .map(|u| format!("{}{}", u.host_str().unwrap_or("入口"), u.path()))
                        .unwrap_or_else(|| format!("待确认#{}", members[0].id.0));
                    let derived =
                        unique_name(c, &format!("{name}·{suffix}·{ad}"), members[0].id.0)?;
                    c.execute(
                        "INSERT INTO provider_channels(name,enabled,created_ms) VALUES(?1,?2,?3)",
                        params![derived, enabled, now_ms()],
                    )?;
                    c.last_insert_rowid()
                };
                c.execute("UPDATE provider_channels SET base_url=?2,adapter_kind=?3,canonical_state=?4 WHERE id=?1",params![target,if canonical{Some(&url)}else{None},ad,if canonical{"canonical"}else{"attention"}])?;
                for row in members {
                    c.execute(
                        "UPDATE provider_connections SET provider_id=?2 WHERE channel_id=?1",
                        params![row.id.0, target],
                    )?;
                    ensure_source(c, sb, row, target)?;
                }
            }
        }
        // Ungrouped sources receive explicit imported endpoints, never a guessed vendor brand.
        let mut imported: HashMap<(String, String), i64> = HashMap::new();
        for row in rows.iter().filter(|r| r.provider_id.is_none()) {
            let url = canonical_url(&row.base_url).ok();
            let ad = adapter(c, row.id.0).unwrap_or_else(|_| "attention".into());
            let key = (
                url.clone()
                    .unwrap_or_else(|| format!("invalid:{}", row.id.0)),
                ad.clone(),
            );
            let pid = if let Some(id) = imported.get(&key) {
                *id
            } else {
                let suffix = reqwest::Url::parse(&key.0)
                    .ok()
                    .map(|u| format!("{}{}", u.host_str().unwrap_or("入口"), u.path()))
                    .unwrap_or_else(|| format!("待确认#{}", row.id.0));
                let name = unique_name(c, &format!("已导入入口·{suffix}·{ad}"), row.id.0)?;
                c.execute("INSERT INTO provider_channels(name,enabled,created_ms,base_url,adapter_kind,canonical_state) VALUES(?1,1,?2,?3,?4,?5)",params![name,now_ms(),url,ad,if key.0.starts_with("invalid:")||key.1=="attention"{"attention"}else{"canonical"}])?;
                let id = c.last_insert_rowid();
                imported.insert(key.clone(), id);
                id
            };
            c.execute(
                "INSERT INTO provider_connections(channel_id,provider_id) VALUES(?1,?2)",
                params![row.id.0, pid],
            )?;
            ensure_source(c, sb, row, pid)?;
        }
        for provider in imported.values() {
            c.execute("UPDATE provider_channels SET enabled=EXISTS(SELECT 1 FROM provider_connections p JOIN channels s ON s.id=p.channel_id WHERE p.provider_id=?1 AND s.enabled=1) WHERE id=?1",[provider])?;
        }
        Ok(())
    })
}
fn ensure_source(
    c: &Connection,
    sb: &SecretBox,
    row: &channels::ChannelRow,
    provider: i64,
) -> Result<i64> {
    let current:Option<(i64,i64)>=c.query_row("SELECT p.id,p.provider_id FROM source_credentials x JOIN provider_credentials p ON p.id=x.credential_id WHERE x.source_id=?1",[row.id.0],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((id, p)) = current {
        if p == provider {
            return Ok(id);
        }
    }
    let identity = auth_identity(c, sb, row);
    let readable = identity.is_ok();
    let (kind, hash, account) =
        identity.unwrap_or_else(|_| ("api_key".into(), format!("unreadable:{}", row.id.0), None));
    let matching:Option<i64>=c.query_row("SELECT id FROM provider_credentials WHERE provider_id=?1 AND identity_hash=?2 ORDER BY id LIMIT 1",params![provider,hash],|r|r.get(0)).optional()?;
    let credential = if let Some(id) = matching {
        id
    } else {
        c.execute("INSERT INTO provider_credentials(provider_id,kind,label,enabled,priority,max_concurrent,secret_source_id,oauth_account_id,identity_hash,created_at_ms) VALUES(?1,?2,?3,?4,?5,0,?6,?7,?8,?9)",params![provider,kind,if readable{format!("{} · Key",row.name)}else{format!("凭据待确认 #{}",row.id.0)},readable,row.priority,if kind=="oauth"{None}else{Some(row.id.0)},account,hash,now_ms()])?;
        c.last_insert_rowid()
    };
    // Reparenting metadata alone keeps the same physical identity and native generation.
    c.execute("INSERT INTO source_credentials(source_id,credential_id) VALUES(?1,?2) ON CONFLICT(source_id) DO UPDATE SET credential_id=excluded.credential_id",params![row.id.0,credential])?;
    Ok(credential)
}
pub fn adopt_source(c: &Connection, sb: &SecretBox, source: i64, provider: i64) -> Result<i64> {
    let row = channels::get(c, ChannelId(source))?.ok_or_else(|| bad("连接不存在"))?;
    let (url, state, provider_adapter): (Option<String>, String, String) = c.query_row(
        "SELECT base_url,canonical_state,adapter_kind FROM provider_channels WHERE id=?1",
        [provider],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let normalized = canonical_url(&row.base_url)?;
    let ad = adapter(c, source)?;
    if state == "canonical" && url.as_deref() != Some(normalized.as_str()) {
        return Err(bad("渠道只允许一个API URL，请创建另一个渠道"));
    }
    let has_credentials: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM provider_credentials WHERE provider_id=?1)",
        [provider],
        |r| r.get(0),
    )?;
    if has_credentials && provider_adapter != ad {
        return Err(bad("认证或工具协议语义不同，请创建另一个渠道"));
    }
    if url.is_none() {
        c.execute("UPDATE provider_channels SET base_url=?2,canonical_state='canonical',adapter_kind=?3 WHERE id=?1",params![provider,normalized,ad])?;
    }
    ensure_source(c, sb, &row, provider)
}
pub fn source_credential(c: &Connection, source: i64) -> Result<Option<CredentialRef>> {
    Ok(c.query_row("SELECT k.id,k.provider_id,k.enabled<>0 AND p.enabled<>0 AND p.canonical_state='canonical' AND (k.kind<>'oauth' OR EXISTS(SELECT 1 FROM oauth_accounts a WHERE a.id=k.oauth_account_id AND a.status='active')),k.priority,k.max_concurrent,p.key_strategy,p.switch_on_429,p.switch_on_400,k.revision,p.config_version FROM source_credentials x JOIN provider_credentials k ON k.id=x.credential_id JOIN provider_channels p ON p.id=k.provider_id JOIN provider_connections g ON g.channel_id=x.source_id AND g.provider_id=k.provider_id WHERE x.source_id=?1",[source],|r|Ok(CredentialRef{id:r.get(0)?,provider_id:r.get(1)?,enabled:r.get(2)?,priority:r.get(3)?,max_concurrent:r.get(4)?,key_strategy:r.get(5)?,switch_on_429:r.get(6)?,switch_on_400:r.get(7)?,revision:r.get(8)?,provider_version:r.get(9)?})).optional()?)
}
pub fn load_source_secret(c: &Connection, sb: &SecretBox, source: i64) -> Result<Option<String>> {
    // Each legacy deployment retains its own ciphertext. A metadata carrier may move
    // to another provider later; it must never become another source's secret.
    channels::secret(c, sb, ChannelId(source))
}

pub fn credentials(c: &Connection, provider: i64) -> Result<Vec<Credential>> {
    let mut q=c.prepare("SELECT id,provider_id,label,kind,enabled,priority,max_concurrent,oauth_account_id,revision,secret_source_id FROM provider_credentials WHERE provider_id=?1 ORDER BY priority,id")?;
    let data = q
        .query_map([provider], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, bool>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, u32>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, Option<i64>>(9)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    data.into_iter().map(|(id,provider_id,label,kind,enabled,priority,max_concurrent,oauth_account_id,revision,carrier)|{
  let mut q=c.prepare("SELECT source_id FROM source_credentials WHERE credential_id=?1 ORDER BY source_id")?;let source_ids=q.query_map([id],|r|r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
  let has_secret=if let Some(source)=carrier{c.query_row("SELECT EXISTS(SELECT 1 FROM channel_secrets WHERE channel_id=?1)",[source],|r|r.get(0))?}else{false};
  let mut models:Vec<CredentialModel>=vec![];
  for source in &source_ids {if let Some(source)=channels::get(c,ChannelId(*source))? {for m in source.models {
   if let Some(old)=models.iter_mut().find(|old|tokengo_shared::same_model_name(&old.model,&m.model)){if old.upstream!=m.upstream||old.enabled!=m.enabled||old.vision!=m.vision||old.drop_fields!=m.drop_fields_text(){old.conflict=true;}}
   else {models.push(CredentialModel{model:m.model.clone(),upstream:m.upstream.clone(),enabled:m.enabled,vision:m.vision,drop_fields:m.drop_fields_text(),conflict:false});}
  }}}
  let tools=source_ids.iter().map(|source_id|Ok(CredentialTools{source_id:*source_id,capabilities:crate::search_tools::load(c,*source_id)?})).collect::<Result<Vec<_>>>()?;
  Ok(Credential{id,provider_id,label,kind,enabled,priority,max_concurrent,source_ids,has_secret,oauth_account_id,revision,models,tools})
 }).collect()
}
pub fn affinity_token(c: &Connection, user: i64, provider: i64) -> Result<Option<AffinityToken>> {
    Ok(c.query_row("SELECT credential_id,generation FROM credential_affinity WHERE user_id=?1 AND provider_id=?2",params![user,provider],|r|Ok(AffinityToken{credential_id:r.get(0)?,generation:r.get(1)?})).optional()?)
}
pub fn switch_affinity(
    c: &Connection,
    user: i64,
    provider: i64,
    expected: &AffinityToken,
    next: i64,
) -> Result<bool> {
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM provider_credentials k JOIN provider_channels p ON p.id=k.provider_id WHERE k.id=?1 AND k.provider_id=?2 AND k.enabled=1 AND p.enabled=1)",params![next,provider],|r|r.get(0))?;
    if !valid {
        return Ok(false);
    }
    Ok(c.execute("UPDATE credential_affinity SET credential_id=?3,generation=generation+1,updated_at_ms=?4 WHERE user_id=?1 AND provider_id=?2 AND credential_id=?5 AND generation=?6",params![user,provider,next,now_ms(),expected.credential_id,expected.generation])?==1)
}
pub fn order_for_user(
    c: &Connection,
    user: i64,
    provider: i64,
    sources: &[i64],
) -> Result<Vec<i64>> {
    order_for_user_with_load(c, user, provider, sources, &HashMap::new())
}
pub fn order_for_user_with_load(
    c: &Connection,
    user: i64,
    provider: i64,
    sources: &[i64],
    loads: &HashMap<i64, usize>,
) -> Result<Vec<i64>> {
    let mut pool = Vec::new();
    for (ordinal, source) in sources.iter().enumerate() {
        if let Some(k) = source_credential(c, *source)? {
            if k.provider_id == provider && k.enabled {
                pool.push((*source, k, ordinal));
            }
        }
    }
    let mut ids = pool
        .iter()
        .map(|(_, k, _)| (k.priority, k.id))
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let existing = affinity_token(c, user, provider)?;
    let valid_existing = existing
        .as_ref()
        .filter(|p| ids.iter().any(|(_, id)| *id == p.credential_id));
    let selected = if let Some(p) = valid_existing {
        p.credential_id
    } else {
        let concurrent = pool[0].1.key_strategy == "concurrent";
        let selected = if concurrent {
            let offset = user.unsigned_abs() as usize % ids.len();
            ids.iter()
                .enumerate()
                .min_by_key(|(idx, (_, id))| {
                    (
                        loads.get(id).copied().unwrap_or(0),
                        (*idx + ids.len() - offset) % ids.len(),
                    )
                })
                .unwrap()
                .1
                 .1
        } else {
            ids[0].1
        };
        selected
    };
    pool.sort_by_key(|(_, k, ordinal)| (k.id != selected, k.priority, k.id, *ordinal));
    Ok(pool.into_iter().map(|(source, _, _)| source).collect())
}

pub fn ensure_affinity_for_selection(
    c: &Connection,
    user: i64,
    provider: i64,
    selected: i64,
) -> Result<AffinityToken> {
    if user <= 0 {
        return Err(bad("User identity required for credential affinity"));
    }
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM provider_credentials k JOIN provider_channels p ON p.id=k.provider_id WHERE k.id=?1 AND k.provider_id=?2 AND k.enabled=1 AND p.enabled=1)",params![selected,provider],|r|r.get(0))?;
    if !valid {
        return Err(bad("Credential unavailable"));
    }
    c.execute("INSERT OR IGNORE INTO credential_affinity(user_id,provider_id,credential_id,updated_at_ms) VALUES(?1,?2,?3,?4)",params![user,provider,selected,now_ms()])?;
    // Repair only a globally unavailable pin, never one absent from the current model subset.
    c.execute("UPDATE credential_affinity SET credential_id=?3,generation=generation+1,updated_at_ms=?4 WHERE user_id=?1 AND provider_id=?2 AND NOT EXISTS(SELECT 1 FROM provider_credentials k JOIN provider_channels p ON p.id=k.provider_id WHERE k.id=credential_affinity.credential_id AND k.provider_id=?2 AND k.enabled=1 AND p.enabled=1 AND p.canonical_state='canonical' AND (k.kind<>'oauth' OR EXISTS(SELECT 1 FROM oauth_accounts a WHERE a.id=k.oauth_account_id AND a.status='active')))",params![user,provider,selected,now_ms()])?;
    affinity_token(c, user, provider)?.ok_or_else(|| bad("Affinity could not be established"))
}

/// Keep secret and literal authentication headers coherent across all deployments of one Key.
pub fn rotate_secret(c: &Connection, sb: &SecretBox, credential: i64, key: &str) -> Result<()> {
    let row = c.query_row(
        "SELECT provider_id,kind,identity_hash FROM provider_credentials WHERE id=?1",
        [credential],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    )?;
    if row.1 != "api_key" {
        return Err(bad("此凭据须通过专用授权流程轮换"));
    }
    let mut q = c.prepare(
        "SELECT source_id FROM source_credentials WHERE credential_id=?1 ORDER BY source_id",
    )?;
    let sources = q
        .query_map([credential], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for source in &sources {
        let id = ChannelId(*source);
        let mut headers = channels::headers(c, sb, id)?;
        let auth = headers
            .iter()
            .enumerate()
            .filter(|(_, (name, _))| {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "authorization" | "x-api-key" | "x-goog-api-key" | "api-key"
                )
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if auth.len() > 1 {
            return Err(bad("存在多个认证头，请先明确认证方式再轮换"));
        }
        if let Some(i) = auth.first() {
            let old = &headers[*i].1;
            let value = if old.to_ascii_lowercase().starts_with("bearer ") {
                format!("Bearer {key}")
            } else {
                key.to_owned()
            };
            if old != &value {
                headers[*i].1 = value;
                channels::set_headers(c, sb, id, &headers)?;
            }
        }
        channels::set_secret(c, sb, id, key)?;
    }
    let source = *sources.first().ok_or_else(|| bad("凭据没有来源部署"))?;
    let source = channels::get(c, ChannelId(source))?.ok_or_else(|| bad("来源不存在"))?;
    let (_, hash, _) = auth_identity(c, sb, &source)?;
    let duplicate:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM provider_credentials WHERE provider_id=?1 AND id<>?2 AND identity_hash=?3)",params![row.0,credential,hash],|r|r.get(0))?;
    if duplicate {
        return Err(bad("此渠道已存在相同凭据，请使用已有Key"));
    }
    if hash != row.2 {
        c.execute(
            "UPDATE provider_credentials SET identity_hash=?2,revision=revision+1 WHERE id=?1",
            params![credential, hash],
        )?;
    }
    Ok(())
}
