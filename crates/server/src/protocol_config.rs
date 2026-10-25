//! Explicit source protocol capabilities. Stored apart from the legacy Channel wire format.
use crate::{db, protocol::Protocol};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Explicit provider contract for compatible proxies/self-hosted endpoints.
    /// None uses only verified endpoint recognition and preserves old fingerprints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter_profile: Option<String>,
    pub native: Vec<Protocol>,
    #[serde(default)]
    pub allow_conversion: bool,
    /// Native Chat input_audio only; output audio and media conversion stay disabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub native_input_audio: bool,
    /// Explicit ability to replay converted assistant reasoning_content history.
    /// Native Chat passthrough and returned reasoning fields do not require it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub chat_reasoning_replay: bool,
    /// Verified independent input/output ceilings, not a token price or a promise
    /// that both ceilings fit the provider's joint context window simultaneously.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<i64>,
}
fn is_false(v: &bool) -> bool {
    !*v
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            parameter_profile: None,
            native: vec![Protocol::Chat],
            allow_conversion: false,
            native_input_audio: false,
            chat_reasoning_replay: false,
            max_input_tokens: None,
            max_output_tokens: None,
        }
    }
}
impl Capabilities {
    pub fn validate(&self) -> Result<(), String> {
        if self.parameter_profile.as_deref().is_some_and(|profile| !crate::parameter_contracts::profile_supported(profile)) {
            return Err("Unknown parameter_profile; select a documented provider contract".into());
        }
        match (self.max_input_tokens,self.max_output_tokens) {
            (None,None)=>{},
            (Some(input),Some(output)) if input>0 && output>0=>{},
            _=>return Err("Source context bounds require positive max_input_tokens and max_output_tokens together".into()),
        }
        if self.chat_reasoning_replay && !self.native.contains(&Protocol::Chat) {
            return Err("chat_reasoning_replay requires native chat".into());
        }
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
    pub fn context_bounds(&self) -> Option<(i64, i64)> {
        self.max_input_tokens.zip(self.max_output_tokens)
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
            parameter_profile: None,
            native: vec![Protocol::Messages],
            allow_conversion: true,
            native_input_audio: false,
            chat_reasoning_replay: false,
            max_input_tokens: None,
            max_output_tokens: None,
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
            parameter_profile: None,
            native: vec![],
            allow_conversion: true,
            native_input_audio: false,
            chat_reasoning_replay: false,
            max_input_tokens: None,
            max_output_tokens: None,
        }
        .validate()
        .is_err());
        assert!(Capabilities {
            parameter_profile: None,
            native: vec![Protocol::Chat, Protocol::Chat],
            allow_conversion: true,
            native_input_audio: false,
            chat_reasoning_replay: false,
            max_input_tokens: None,
            max_output_tokens: None,
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

/// OAuth and standalone paid tools must use the authenticated, metered executor.
pub fn background_probe_allowed(c: &rusqlite::Connection, id: i64) -> db::Result<bool> {
    let oauth: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings WHERE channel_id=?1)",
        [id],
        |r| r.get(0),
    )?;
    let tools = crate::search_tools::load(c, id)?;
    Ok(!oauth && !tools.search && !tools.fetch)
}
#[cfg(test)]
mod context_capability_tests {
    use super::*;
    #[test]
    fn context_pairs_and_chat_reasoning_replay_require_valid_explicit_declarations() {
        let default = Capabilities::default();
        let raw = serde_json::to_value(&default).unwrap();
        assert!(raw.get("chat_reasoning_replay").is_none());
        assert!(raw.get("max_input_tokens").is_none());
        assert_eq!(
            serde_json::from_value::<Capabilities>(raw)
                .unwrap()
                .context_bounds(),
            None
        );
        for (input, output) in [
            (Some(1), None),
            (None, Some(1)),
            (Some(0), Some(1)),
            (Some(1), Some(-1)),
        ] {
            assert!(Capabilities {
                max_input_tokens: input,
                max_output_tokens: output,
                ..Default::default()
            }
            .validate()
            .is_err());
        }
        assert!(Capabilities {
            native: vec![Protocol::Responses],
            chat_reasoning_replay: true,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(Capabilities {
            max_input_tokens: Some(1048576),
            max_output_tokens: Some(393216),
            ..Default::default()
        }
        .validate()
        .is_ok());
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;
    #[test]
    fn reasoning_and_context_capability_changes_advance_source_generation() {
        let c = db::open_in_memory().unwrap();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "test".into(),
                base_url: "https://example.invalid".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 1,
            },
        )
        .unwrap()
        .0;
        let mut cap = Capabilities::default();
        save(&c, id, &cap).unwrap();
        let generation = |c: &rusqlite::Connection| {
            c.query_row(
                "SELECT binding_generation FROM channels WHERE id=?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        };
        let original = generation(&c);
        cap.chat_reasoning_replay = true;
        save(&c, id, &cap).unwrap();
        assert_eq!(generation(&c), original + 1);
        cap.max_input_tokens = Some(1048576);
        cap.max_output_tokens = Some(393216);
        save(&c, id, &cap).unwrap();
        assert_eq!(generation(&c), original + 2);
        save(&c, id, &cap).unwrap();
        assert_eq!(generation(&c), original + 2);
        assert_eq!(
            load(&c, id).unwrap().context_bounds(),
            Some((1048576, 393216))
        );
    }
}

#[cfg(test)]
mod parameter_profile_tests {
    use super::*;
    #[test]
    fn old_sources_keep_the_exact_serialized_protocol_identity() {
        assert_eq!(serde_json::to_string(&Capabilities::default()).unwrap(), r#"{"native":["chat"],"allow_conversion":false}"#);
        let legacy: Capabilities = serde_json::from_str(r#"{"native":["chat"],"allow_conversion":false}"#).unwrap();
        assert!(legacy.parameter_profile.is_none());
    }
    #[test]
    fn compatible_proxy_profile_is_explicit_and_bounded() {
        let mut caps = Capabilities::default();
        caps.parameter_profile = Some("dashscope".into());
        assert!(caps.validate().is_ok());
        caps.parameter_profile = Some("drop_everything".into());
        assert!(caps.validate().is_err());
    }
}
