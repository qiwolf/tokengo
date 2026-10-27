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
    /// Unverified runtime identity evidence; never a capability or authentication grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_identity: Option<crate::runtime_identity::RuntimeIdentity>,
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
            runtime_identity: None,
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
        if let Some(identity) = &self.runtime_identity {
            identity.validate(self.parameter_profile.as_deref())?;
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
    pub fn runtime_description(&self, model: &str) -> serde_json::Value {
        self.runtime_identity.as_ref().map(|identity|identity.describe(model))
            .unwrap_or_else(||serde_json::json!({"status":"unknown","capabilities":"unknown","origin_verified":false}))
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
    if caps.runtime_identity.is_some() {
        let oauth: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings WHERE channel_id=?1)", [id], |row|row.get(0))?;
        if oauth { return Err(db::DbError::Other("Runtime identity cannot describe an independently authenticated OAuth source".into())); }
    }
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
            runtime_identity: None,
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
            runtime_identity: None,
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
            runtime_identity: None,
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

#[cfg(test)]
mod w08_runtime_tests {
    use super::*;
    use serde_json::json;
    fn declaration(template: &str) -> serde_json::Value {
        json!({"native":["chat"],"parameter_profile":"ollama","runtime_identity":{
            "runtime":"ollama","evidence":"operator_declared","version":"0.13.3",
            "models":[{"model":"qwen3:8b","template_sha256":template}]
        }})
    }
    #[test]
    fn w08_offline_identity_is_bounded_unverified_and_invalidates_cache() {
        let c = db::open_in_memory().unwrap();
        let id = db::channels::create(&c,&db::channels::NewChannel {
            name:"Offline".into(),base_url:"http://unreachable.invalid/v1".into(),enabled:true,
            unwrap_envelope:false,max_concurrent:0,
        }).unwrap().0;
        let caps: Capabilities = serde_json::from_value(declaration(&"a".repeat(64))).unwrap();
        save(&c,id,&caps).unwrap();
        let before=crate::reasoning_capabilities::fingerprint(&c,id).unwrap();
        let generation=|c:&rusqlite::Connection|c.query_row("SELECT binding_generation FROM channels WHERE id=?1",[id],|r|r.get::<_,i64>(0)).unwrap();
        let before_generation=generation(&c);
        save(&c,id,&caps).unwrap();
        assert_eq!(generation(&c),before_generation);
        assert_eq!(caps.runtime_description("qwen3:8b")["status"],"unverified");
        assert_eq!(caps.runtime_description("other")["status"],"unknown");
        let models=crate::upstream::parse_model_catalog(r#"{"data":[{"id":"qwen3:8b","reasoning_effort_levels":["low","high"]}]}"#);
        crate::reasoning_capabilities::store_catalog(&c,id,&models,1).unwrap();
        assert_eq!(crate::reasoning_capabilities::for_model(&c,id,"qwen3:8b").unwrap().status,"unknown", "runtime catalog self-claims cannot grant levels");
        db::channels::set_models(&c,tokengo_shared::ChannelId(id),&[
            db::channels::ModelMapping::new("runtime-alias",Some("qwen3:8b".into()),false)
        ]).unwrap();
        let manual_levels=vec!["low".to_owned(),"high".to_owned()];
        crate::reasoning_capabilities::configure(&c,id,"runtime-alias",Some(&manual_levels),Some("high")).unwrap();
        let confirmed=crate::reasoning_capabilities::for_mapping(&c,id,"runtime-alias").unwrap();
        assert_eq!(confirmed.status,"supported");
        assert_eq!(confirmed.source,"user_confirmed");
        assert_eq!(confirmed.manual_levels,Some(manual_levels.clone()));
        assert_eq!(confirmed.default_effort.as_deref(),Some("high"));
        let saved_manual=|c:&rusqlite::Connection|c.query_row(
            "SELECT source_identity,upstream_model,manual_levels,default_effort FROM channel_reasoning_settings WHERE channel_id=?1 AND model_name=?2",
            rusqlite::params![id,tokengo_shared::normalize_model_name("runtime-alias")],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?))
        ).unwrap();
        let original_manual=saved_manual(&c);
        let before_generation=generation(&c);
        let different: Capabilities=serde_json::from_value(declaration(&"b".repeat(64))).unwrap();
        save(&c,id,&different).unwrap();
        assert_eq!(generation(&c),before_generation+1);
        assert_ne!(before,crate::reasoning_capabilities::fingerprint(&c,id).unwrap());
        assert_eq!(crate::reasoning_capabilities::for_model(&c,id,"qwen3:8b").unwrap().status,"unknown");
        let stale_manual=crate::reasoning_capabilities::for_mapping(&c,id,"runtime-alias").unwrap();
        assert_eq!(stale_manual.status,"unknown");
        assert!(stale_manual.manual_levels.is_none());
        assert!(stale_manual.default_effort.is_none());
        assert_eq!(saved_manual(&c),original_manual,"identity invalidation must not delete or rewrite old operator settings");
        assert!(!load(&c,id).unwrap().chat_reasoning_replay);
        for field in ["template_sha256","reasoning_parser_sha256","generation_config_sha256"] {
            let mut bad=declaration(&"a".repeat(64)); bad["runtime_identity"]["models"][0][field]=json!("https://private.test/?api_key=secret");
            assert!(serde_json::from_value::<Capabilities>(bad).map(|c|c.validate().is_err()).unwrap_or(true));
        }
        let mut spoof=declaration(&"a".repeat(64)); spoof["runtime_identity"]["evidence"]=json!("verified");
        assert!(serde_json::from_value::<Capabilities>(spoof).is_err());
        for profile in ["vllm","openai","native"] {
            let mut conflict=declaration(&"a".repeat(64));conflict["parameter_profile"]=json!(profile);
            assert!(serde_json::from_value::<Capabilities>(conflict).unwrap().validate().is_err());
        }
        let mut excessive=declaration(&"a".repeat(64));
        excessive["runtime_identity"]["models"]=json!((0..129).map(|i|json!({"model":format!("model-{i}")})).collect::<Vec<_>>());
        assert!(serde_json::from_value::<Capabilities>(excessive).unwrap().validate().is_err());
        let mut conflict=declaration(&"a".repeat(64));conflict["parameter_profile"]=json!("openai");
        assert!(serde_json::from_value::<Capabilities>(conflict).unwrap().validate().is_err());
    }
}
