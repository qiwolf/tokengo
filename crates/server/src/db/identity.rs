//! User and virtual-key identity. Authentication always rechecks live ancestry.
use super::{now_ms, DbError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokengo_shared::{normalize_model_name, same_model_name};

fn yes() -> bool {
    true
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_channels: Option<Vec<i64>>,
    /// None preserves legacy per-key route grants for users; keys inherit.
    #[serde(default)]
    pub allowed_services: Option<Vec<i64>>,
    #[serde(default = "yes")]
    pub allow_paid: bool,
    /// Explicit management capability; legacy credentials never receive this implicitly.
    #[serde(default)]
    pub admin_access: bool,
    #[serde(default)]
    pub rpm: Option<u32>,
    #[serde(default)]
    pub tpm: Option<u64>,
    #[serde(default)]
    pub max_concurrent: Option<u32>,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            allowed_models: None,
            allowed_channels: None,
            allowed_services: None,
            allow_paid: true,
            admin_access: false,
            rpm: None,
            tpm: None,
            max_concurrent: None,
        }
    }
}
impl Policy {
    pub fn validate(&self) -> Result<()> {
        if let Some(models) = &self.allowed_models {
            if models.len() > 256 {
                return Err(DbError::Other("Too many allowed models".into()));
            }
            let mut seen = std::collections::HashSet::new();
            for model in models {
                let name = normalize_model_name(model);
                if name.is_empty()
                    || model.chars().count() > 256
                    || model.chars().any(char::is_control)
                    || !seen.insert(name)
                {
                    return Err(DbError::Other("Invalid or duplicate allowed model".into()));
                }
            }
        }
        if let Some(ids) = &self.allowed_channels {
            if ids.len() > 256
                || ids
                    .iter()
                    .enumerate()
                    .any(|(i, id)| *id <= 0 || ids[..i].contains(id))
            {
                return Err(DbError::Other(
                    "Invalid or duplicate allowed channel".into(),
                ));
            }
        }
        if let Some(ids) = &self.allowed_services {
            if ids.len() > 256 || ids.iter().enumerate().any(|(i,id)| *id <= 0 || ids[..i].contains(id)) {
                return Err(DbError::Other("Invalid or duplicate allowed service".into()));
            }
        }
        if self.rpm == Some(0)
            || self.tpm == Some(0)
            || self.max_concurrent == Some(0)
            || self.tpm.is_some_and(|v| v > i64::MAX as u64)
        {
            return Err(DbError::Other(
                "Policy limits must be positive and fit storage".into(),
            ));
        }
        Ok(())
    }
    pub fn intersection(&self, other: &Self) -> Self {
        let allowed_models = match (&self.allowed_models, &other.allowed_models) {
            (None, x) | (x, None) => x.clone(),
            (Some(a), Some(b)) => Some(
                a.iter()
                    .filter(|name| b.iter().any(|n| same_model_name(name, n)))
                    .cloned()
                    .collect(),
            ),
        };
        let allowed_channels = match (&self.allowed_channels, &other.allowed_channels) {
            (None, x) | (x, None) => x.clone(),
            (Some(a), Some(b)) => Some(a.iter().filter(|id| b.contains(id)).copied().collect()),
        };
        let allowed_services = match (&self.allowed_services, &other.allowed_services) {
            (None,x)|(x,None) => x.clone(),
            (Some(a),Some(b)) => Some(a.iter().filter(|id| b.contains(id)).copied().collect()),
        };
        fn minimum<T: Ord + Copy>(a: Option<T>, b: Option<T>) -> Option<T> {
            match (a, b) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        }
        Self {
            allowed_models,
            allowed_channels,
            allowed_services,
            allow_paid: self.allow_paid && other.allow_paid,
            admin_access: self.admin_access && other.admin_access,
            rpm: minimum(self.rpm, other.rpm),
            tpm: minimum(self.tpm, other.tpm),
            max_concurrent: minimum(self.max_concurrent, other.max_concurrent),
        }
    }
    /// An explicitly requested child policy must not claim broader access than its parent.
    pub fn no_broader_than(&self, parent: &Self) -> bool {
        let models = match (&self.allowed_models, &parent.allowed_models) {
            (None, Some(_)) => false,
            (Some(a), Some(b)) => a.iter().all(|n| b.iter().any(|p| same_model_name(n, p))),
            _ => true,
        };
        let channels = match (&self.allowed_channels, &parent.allowed_channels) {
            (None, Some(_)) => false,
            (Some(a), Some(b)) => a.iter().all(|n| b.contains(n)),
            _ => true,
        };
        // Service null on a key inherits; unlike models, it does not mean
        // every service. The owner boundary is checked separately at issuance.
        let services = match (&self.allowed_services, &parent.allowed_services) {
            (Some(a),Some(b)) => a.iter().all(|id|b.contains(id)),
            (Some(_),None) => true,
            (None,Some(_)) => true,
            (None,None) => true,
        };
        fn capped<T: Ord>(child: Option<T>, parent: Option<T>) -> bool {
            match (child, parent) {
                (None, Some(_)) => false,
                (Some(a), Some(b)) => a <= b,
                _ => true,
            }
        }
        models
            && channels
            && services
            && (!self.allow_paid || parent.allow_paid)
            && (!self.admin_access || parent.admin_access)
            && capped(self.rpm, parent.rpm)
            && capped(self.tpm, parent.tpm)
            && capped(self.max_concurrent, parent.max_concurrent)
    }
    fn permits_model(&self, name: &str) -> bool {
        self.allowed_models
            .as_ref()
            .is_none_or(|models| models.iter().any(|m| same_model_name(m, name)))
    }
    fn permits_channel(&self, id: i64, paid: bool) -> bool {
        (!paid || self.allow_paid)
            && self
                .allowed_channels
                .as_ref()
                .is_none_or(|ids| ids.contains(&id))
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct AuthContext {
    pub user_id: i64,
    pub key_id: i64,
    pub role: String,
    pub user_policy: Policy,
    pub key_policy: Policy,
    /// Closest parent first; never includes this key. Used by shared limits and budgets.
    pub ancestor_key_ids: Vec<i64>,
    /// Earliest expiry across this credential and every ancestor.
    pub expires_at: Option<i64>,
    /// SHA-256 of the canonical effective identity policy snapshot, never pricing.
    pub policy_version: String,
}
impl AuthContext {
    pub fn permits_model(&self, name: &str) -> bool {
        self.user_policy.permits_model(name) && self.key_policy.permits_model(name)
    }
    pub fn permits_channel(&self, id: i64, paid: bool) -> bool {
        self.user_policy.permits_channel(id, paid) && self.key_policy.permits_channel(id, paid)
    }
    pub fn effective_policy(&self) -> Policy {
        self.user_policy.intersection(&self.key_policy)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub enabled: bool,
    pub policy: Policy,
    pub created_at_ms: i64,
}
#[derive(Clone, Debug, Serialize)]
pub struct VKey {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub hint: String,
    pub enabled: bool,
    pub revoked_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub policy: Policy,
    pub parent_key_id: Option<i64>,
    pub created_at_ms: i64,
}
fn decode(raw: &str) -> Result<Policy> {
    let p: Policy = serde_json::from_str(raw)
        .map_err(|_| DbError::Other("Invalid persisted identity policy".into()))?;
    p.validate()?;
    Ok(p)
}
pub fn validate_name(name: &str) -> Result<&str> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(DbError::Other(
            "Name must contain 1..80 characters without controls".into(),
        ));
    }
    Ok(trimmed)
}
pub fn validate_role(role: &str) -> Result<()> {
    if !["admin", "user"].contains(&role) {
        return Err(DbError::Other("Role must be admin or user".into()));
    }
    Ok(())
}
pub fn user(c: &Connection, id: i64) -> Result<Option<User>> {
    if super::lifecycle::archived_user(c,id)? { return Ok(None); }
    let row = c
        .query_row(
            "SELECT id,name,role,enabled,policy_json,created_at_ms FROM users WHERE id=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;
    row.map(|(id, name, role, enabled, policy, created_at_ms)| {
        Ok(User {
            id,
            name,
            role,
            enabled,
            policy: decode(&policy)?,
            created_at_ms,
        })
    })
    .transpose()
}
pub fn users(c: &Connection) -> Result<Vec<User>> {
    let mut q = c.prepare("SELECT id FROM users WHERE NOT EXISTS(SELECT 1 FROM settings WHERE key='deleted_user:'||users.id) ORDER BY id")?;
    let ids = q
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| user(c, id)?.ok_or_else(|| DbError::Other("User disappeared".into())))
        .collect()
}
pub fn create_user(c: &Connection, name: &str, role: &str, policy: &Policy) -> Result<User> {
    let name = validate_name(name)?;
    validate_role(role)?;
    policy.validate()?;
    super::passthrough::validate_service_grants(c,None,policy.allowed_services.as_deref())?;
    c.execute(
        "INSERT INTO users(name,role,policy_json,created_at_ms) VALUES(?1,?2,?3,?4)",
        params![name, role, serde_json::to_string(policy).unwrap(), now_ms()],
    )?;
    user(c, c.last_insert_rowid())?.ok_or_else(|| DbError::Other("User disappeared".into()))
}
pub fn update_user(c: &Connection, row: &User) -> Result<()> {
    validate_name(&row.name)?;
    validate_role(&row.role)?;
    row.policy.validate()?;
    let previous=user(c,row.id)?;
    super::passthrough::validate_service_grants(c,previous.as_ref().and_then(|u|u.policy.allowed_services.as_deref()),row.policy.allowed_services.as_deref())?;
    c.execute(
        "UPDATE users SET name=?2,role=?3,enabled=?4,policy_json=?5 WHERE id=?1",
        params![
            row.id,
            row.name.trim(),
            row.role,
            row.enabled,
            serde_json::to_string(&row.policy).unwrap()
        ],
    )?;
    Ok(())
}
pub fn key(c: &Connection, id: i64) -> Result<Option<VKey>> {
    let row=c.query_row("SELECT id,user_id,name,hint,enabled,revoked_at,expires_at,policy_json,parent_key_id,created_at_ms FROM gateway_keys WHERE id=?1",[id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,bool>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,Option<i64>>(6)?,r.get::<_,String>(7)?,r.get::<_,Option<i64>>(8)?,r.get::<_,i64>(9)?))).optional()?;
    row.map(
        |(
            id,
            user_id,
            name,
            hint,
            enabled,
            revoked_at,
            expires_at,
            policy,
            parent_key_id,
            created_at_ms,
        )| {
            Ok(VKey {
                id,
                user_id,
                name,
                hint,
                enabled,
                revoked_at,
                expires_at,
                policy: decode(&policy)?,
                parent_key_id,
                created_at_ms,
            })
        },
    )
    .transpose()
}
pub fn keys(c: &Connection, user_id: i64) -> Result<Vec<VKey>> {
    let mut q = c.prepare("SELECT g.id FROM gateway_keys g WHERE g.user_id=?1 AND NOT EXISTS(SELECT 1 FROM settings s WHERE s.key='deleted_key:'||CAST(g.id AS TEXT)) ORDER BY g.id")?;
    let ids = q
        .query_map([user_id], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| key(c, id)?.ok_or_else(|| DbError::Other("Key disappeared".into())))
        .collect()
}
/// An owner key may manage itself and descendants, never its parent or siblings.
/// Walk metadata rather than active contexts so revoked descendants remain manageable.
pub fn key_in_subtree(c: &Connection, key_id: i64, ancestor_id: i64, user_id: i64) -> Result<bool> {
    let mut cursor = Some(key_id);
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = cursor {
        if seen.len() >= 32 || !seen.insert(id) {
            return Ok(false);
        }
        let Some(row) = key(c, id)? else {
            return Ok(false);
        };
        if row.user_id != user_id {
            return Ok(false);
        }
        if id == ancestor_id {
            return Ok(true);
        }
        cursor = row.parent_key_id;
    }
    Ok(false)
}

fn policy_version(auth: &AuthContext) -> Result<String> {
    use sha2::{Digest, Sha256};
    fn normalized(policy: &Policy) -> Policy {
        let mut policy = policy.clone();
        if let Some(models) = &mut policy.allowed_models {
            *models = models
                .iter()
                .map(|model| normalize_model_name(model))
                .collect();
            models.sort();
        }
        if let Some(services) = &mut policy.allowed_services { services.sort_unstable(); }
        if let Some(channels) = &mut policy.allowed_channels {
            channels.sort_unstable();
        }
        policy
    }
    // BTreeMap canonicalizes object keys recursively; allowlists are sets and are
    // normalized/sorted, while ancestry intentionally retains its closest-first order.
    fn canonical(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: std::collections::BTreeMap<_, _> = map.into_iter().collect();
                serde_json::Value::Object(
                    sorted.into_iter().map(|(k, v)| (k, canonical(v))).collect(),
                )
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(canonical).collect())
            }
            other => other,
        }
    }
    let snapshot = canonical(serde_json::json!({
        "user_id": auth.user_id, "key_id": auth.key_id, "role": auth.role,
        "user_policy": normalized(&auth.user_policy), "key_policy": normalized(&auth.key_policy),
        "ancestor_key_ids": auth.ancestor_key_ids, "expires_at": auth.expires_at,
    }));
    let encoded = serde_json::to_vec(&snapshot)
        .map_err(|_| DbError::Other("Cannot encode identity policy version".into()))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

pub fn context(c: &Connection, key_id: i64) -> Result<Option<AuthContext>> {
    let Some(own) = key(c, key_id)? else {
        return Ok(None);
    };
    let Some(owner) = user(c, own.user_id)? else {
        return Ok(None);
    };
    if !owner.enabled {
        return Ok(None);
    }
    // Intersection starts at the all-capabilities identity element, while newly
    // omitted policies deliberately default admin_access to false.
    let mut policy = Policy {
        admin_access: true,
        ..Policy::default()
    };
    let mut ancestors = Vec::new();
    let mut expires_at: Option<i64> = None;
    let mut current = Some(own);
    let mut seen = std::collections::HashSet::new();
    while let Some(row) = current {
        if seen.len() >= 32
            || !seen.insert(row.id)
            || row.user_id != owner.id
            || !row.enabled
            || row.revoked_at.is_some()
            || row.expires_at.is_some_and(|t| t <= now_ms())
        {
            return Ok(None);
        }
        if row.id != key_id {
            ancestors.push(row.id);
        }
        if let Some(expiry) = row.expires_at {
            expires_at = Some(expires_at.map_or(expiry, |current| current.min(expiry)));
        }
        policy = policy.intersection(&row.policy);
        current = if let Some(parent) = row.parent_key_id {
            match key(c, parent)? {
                Some(row) => Some(row),
                None => return Ok(None),
            }
        } else {
            None
        };
    }
    let mut auth = AuthContext {
        user_id: owner.id,
        key_id,
        role: owner.role,
        user_policy: owner.policy,
        key_policy: policy,
        ancestor_key_ids: ancestors,
        expires_at,
        policy_version: String::new(),
    };
    auth.policy_version = policy_version(&auth)?;
    Ok(Some(auth))
}
/// No secret/ciphertext is returned by metadata APIs. New secret is returned once here.
pub fn create_key(
    c: &Connection,
    user_id: i64,
    name: &str,
    policy: &Policy,
    expires_at: Option<i64>,
    parent_key_id: Option<i64>,
) -> Result<(VKey, String)> {
    let name = validate_name(name)?;
    policy.validate()?;
    super::passthrough::validate_service_grants(c,None,policy.allowed_services.as_deref())?;
    let owner = user(c, user_id)?.ok_or_else(|| DbError::Other("User does not exist".into()))?;
    if !owner.enabled {
        return Err(DbError::Other("User is disabled".into()));
    }
    if expires_at.is_some_and(|t| t <= now_ms()) {
        return Err(DbError::Other("Expiry must be in the future".into()));
    }
    if let Some(parent) = parent_key_id {
        let auth =
            context(c, parent)?.ok_or_else(|| DbError::Other("Parent key is inactive".into()))?;
        if auth.user_id != user_id || auth.ancestor_key_ids.len() >= 31 {
            return Err(DbError::Other("Invalid parent key".into()));
        }
        if !policy.no_broader_than(&auth.effective_policy()) {
            return Err(DbError::Other(
                "Child policy cannot broaden its parent".into(),
            ));
        }
        for ancestor in std::iter::once(parent).chain(auth.ancestor_key_ids) {
            if key(c, ancestor)?
                .and_then(|k| k.expires_at)
                .is_some_and(|limit| expires_at.is_none_or(|t| t > limit))
            {
                return Err(DbError::Other(
                    "Child expiry cannot exceed parent expiry".into(),
                ));
            }
        }
    }
    let secret = super::gateway_keys::generate();
    let hash = super::gateway_keys::digest(&secret);
    let selector = secret.split_once('.').unwrap().0;
    c.execute("INSERT INTO gateway_keys(name,hint,created_at_ms,key_hash,selector,user_id,policy_json,expires_at,parent_key_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![name,super::sessions::mask_key(&secret),now_ms(),hash,selector,user_id,serde_json::to_string(policy).unwrap(),expires_at,parent_key_id])?;
    let row =
        key(c, c.last_insert_rowid())?.ok_or_else(|| DbError::Other("Key disappeared".into()))?;
    Ok((row, secret))
}
/// Production creation stores encrypted recovery material in the same transaction.
/// The hash-only helper remains available for legacy callers and test fixtures.
pub fn create_key_recoverable(
    c:&Connection,sb:&crate::crypto::SecretBox,user_id:i64,name:&str,policy:&Policy,
    expires_at:Option<i64>,parent_key_id:Option<i64>,
)->Result<(VKey,String)> {
    let create=|c:&Connection| {
        let (row,secret)=create_key(c,user_id,name,policy,expires_at,parent_key_id)?;
        super::gateway_keys::retain_secret(c,sb,row.id,&secret)?;
        Ok((row,secret))
    };
    if c.is_autocommit() {super::in_tx(c,create)}else{create(c)}
}
/// Only an explicit owner pause can be undone through self-service.
pub fn self_paused(c: &Connection, key: &VKey) -> Result<bool> {
    Ok(super::settings::get(c,&format!("self_paused_key:{}",key.id))?.as_deref()==Some(key.user_id.to_string().as_str()))
}
pub fn clear_self_pause(c: &Connection, id: i64) -> Result<()> {
    c.execute("DELETE FROM settings WHERE key=?1",[format!("self_paused_key:{id}")])?;
    Ok(())
}
pub fn update_key(c: &Connection, row: &VKey) -> Result<()> {
    if key_deleted(c,row.id)? { return Err(DbError::Other("Deleted keys cannot be modified or restored".into())); }
    row.policy.validate()?;
    let previous=key(c,row.id)?;
    super::passthrough::validate_service_grants(c,previous.as_ref().and_then(|k|k.policy.allowed_services.as_deref()),row.policy.allowed_services.as_deref())?;
    validate_name(&row.name)?;
    if row.enabled || row.revoked_at.is_some() { clear_self_pause(c,row.id)?; }
    c.execute("UPDATE gateway_keys SET name=?2,enabled=?3,revoked_at=?4,expires_at=?5,policy_json=?6 WHERE id=?1",params![row.id,row.name,row.enabled,row.revoked_at,row.expires_at,serde_json::to_string(&row.policy).unwrap()])?;
    Ok(())
}
/// List-only tombstone: retain the credential row, parent chain and accounting IDs.
/// Deliberately separate from `key()` so historical and authentication reads keep
/// their physical record. Disabled keys are permanently revoked before hiding.
pub fn key_deleted(c: &Connection, id: i64) -> Result<bool> {
    Ok(super::settings::get(c,&format!("deleted_key:{id}"))?.is_some())
}
/// Call inside the caller's audit transaction; false means already deleted.
pub fn delete_key(c: &Connection, id: i64) -> Result<bool> {
    let remove=|c:&Connection| {
        let row=key(c,id)?.ok_or_else(||DbError::Other("Key not found".into()))?;
        if key_deleted(c,id)? { return Ok(false); }
        if row.enabled && row.revoked_at.is_none() { return Err(DbError::Other("请先停用或撤销密钥，再从列表删除".into())); }
        // Revoking the retained ancestor permanently blocks its descendants too.
        revoke(c,id)?;
        super::settings::set(c,&format!("deleted_key:{id}"),&now_ms().to_string())?;
        Ok(true)
    };
    if c.is_autocommit() {super::in_tx(c,remove)}else{remove(c)}
}
pub fn revoke(c: &Connection, id: i64) -> Result<bool> {
    clear_self_pause(c,id)?;
    Ok(c.execute(
        "UPDATE gateway_keys SET enabled=0,revoked_at=COALESCE(revoked_at,?2) WHERE id=?1",
        params![id, now_ms()],
    )? > 0)
}
pub fn audit(
    c: &Connection,
    actor: Option<&AuthContext>,
    action: &str,
    target_type: &str,
    target_id: i64,
    metadata: serde_json::Value,
) -> Result<()> {
    c.execute("INSERT INTO audit_events(ts_ms,actor_user_id,actor_key_id,action,target_type,target_id,metadata_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![now_ms(),actor.map(|a|a.user_id),actor.map(|a|a.key_id),action,target_type,target_id,metadata.to_string()])?;
    Ok(())
}
