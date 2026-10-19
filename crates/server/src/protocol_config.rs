//! Explicit source protocol capabilities. Stored apart from the legacy Channel wire format.
use crate::{db, protocol::Protocol};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub native: Vec<Protocol>,
    #[serde(default)]
    pub allow_conversion: bool,
    /// Native Chat input_audio only; output audio and media conversion stay disabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub native_input_audio: bool,
}
fn is_false(v: &bool) -> bool {
    !*v
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            native: vec![Protocol::Chat],
            allow_conversion: false,
            native_input_audio: false,
        }
    }
}
impl Capabilities {
    pub fn validate(&self) -> Result<(), String> {
        if self.native_input_audio && !self.native.contains(&Protocol::Chat) {
            return Err("native_input_audio requires native chat".into());
        }
        if self.native.is_empty() || self.native.len() > 3 {
            return Err("native must declare one to three protocols".into());
        }
        for (i, p) in self.native.iter().enumerate() {
            if self.native[..i].contains(p) {
                return Err("Duplicate native protocol".into());
            }
        }
        Ok(())
    }
    pub fn select(&self, client: Protocol) -> Option<Protocol> {
        if self.native.contains(&client) {
            Some(client)
        } else if self.allow_conversion {
            self.native.first().copied()
        } else {
            None
        }
    }
}
pub fn load(c: &rusqlite::Connection, id: i64) -> db::Result<Capabilities> {
    match db::settings::get(c, &format!("channel_protocols:{id}"))? {
        None => Ok(Capabilities::default()),
        Some(v) => {
            let caps: Capabilities = serde_json::from_str(&v)
                .map_err(|_| db::DbError::Other("Invalid source protocol configuration".into()))?;
            caps.validate().map_err(db::DbError::Other)?;
            Ok(caps)
        }
    }
}
pub fn save(c: &rusqlite::Connection, id: i64, caps: &Capabilities) -> db::Result<()> {
    caps.validate().map_err(db::DbError::Other)?;
    let key = format!("channel_protocols:{id}");
    let value = serde_json::to_string(caps).map_err(|e| db::DbError::Other(e.to_string()))?;
    if db::settings::get(c, &key)?.as_deref() != Some(&value) {
        c.execute(
            "UPDATE channels SET binding_generation=binding_generation+1 WHERE id=?1",
            [id],
        )?;
    }
    db::settings::set(c, &key, &value)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_never_infer_responses_from_source_name() {
        let c = db::open_in_memory().unwrap();
        let caps = load(&c, 19).unwrap();
        assert_eq!(caps.select(Protocol::Chat), Some(Protocol::Chat));
        assert_eq!(caps.select(Protocol::Responses), None);
        let chosen = Capabilities {
            native: vec![Protocol::Messages],
            allow_conversion: true,
            native_input_audio: false,
        };
        save(&c, 19, &chosen).unwrap();
        assert_eq!(
            load(&c, 19).unwrap().select(Protocol::Responses),
            Some(Protocol::Messages)
        );
    }
    #[test]
    fn malformed_capabilities_fail_closed() {
        assert!(Capabilities {
            native: vec![],
            allow_conversion: true,
            native_input_audio: false
        }
        .validate()
        .is_err());
        assert!(Capabilities {
            native: vec![Protocol::Chat, Protocol::Chat],
            allow_conversion: true,
            native_input_audio: false
        }
        .validate()
        .is_err());
        let c = db::open_in_memory().unwrap();
        db::settings::set(&c, "channel_protocols:1", "{}").unwrap();
        assert!(load(&c, 1).is_err());
    }
}

/// Diagnostics choose a declared native wire protocol rather than inferring it from model kind.
pub fn probe_endpoint(
    c: &rusqlite::Connection,
    id: i64,
    requested: crate::gateway::EndpointKind,
) -> db::Result<crate::gateway::EndpointKind> {
    use crate::gateway::EndpointKind;
    let Some(client) = requested.protocol() else {
        return Ok(requested);
    };
    let caps = load(c, id)?;
    let native = if caps.native.contains(&client) {
        client
    } else {
        caps.native[0]
    };
    Ok(match native {
        Protocol::Chat => EndpointKind::Chat,
        Protocol::Responses => EndpointKind::Responses,
        Protocol::Messages => EndpointKind::Messages,
    })
}
