//! Evidence-bound reasoning discovery. A boolean never invents effort levels.
use crate::db::{self, DbError};
use crate::protocol::Protocol;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningCapabilities {
    pub status: String,
    pub levels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thinking_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_wire: Option<String>,
    pub default_effort: Option<String>,
    pub upstream_default: Option<String>,
    pub configured_default: bool,
    pub manual_levels: Option<Vec<String>>,
    pub source: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_identity: Option<Value>,
}
impl Default for ReasoningCapabilities {
    fn default() -> Self {
        Self::unknown("上游未声明具体推理档位")
    }
}
impl ReasoningCapabilities {
    fn unknown(reason: &str) -> Self {
        Self {
            status: "unknown".into(),
            levels: vec![],
            thinking_modes: vec![],
            effort_wire: None,
            default_effort: None,
            upstream_default: None,
            configured_default: false,
            manual_levels: None,
            source: "unknown".into(),
            reason: reason.into(),
            runtime_identity: None,
        }
    }
}
pub fn valid_level(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}
fn levels(v: &Value, objects: bool) -> Option<Vec<String>> {
    let rows = v.as_array()?;
    if rows.is_empty() || rows.len() > 16 {
        return None;
    }
    let mut out = Vec::new();
    for row in rows {
        let s = if objects {
            row.get("effort")?.as_str()?
        } else {
            row.as_str()?
        };
        if !valid_level(s) || out.iter().any(|old| old == s) {
            return None;
        }
        out.push(s.to_owned());
    }
    Some(out)
}
/// Parse explicitly named per-value capability flags, never a generic boolean.
/// A thinking-off type may be supported even when active thinking is not.
fn named_supported_values(value: &Value, thinking: bool) -> Option<(Vec<String>, Option<bool>)> {
    let object = value.as_object()?;
    if object.len() > 17 { return None; }
    let supported = match object.get("supported") {
        Some(value) => Some(value.as_bool()?),
        None => None,
    };
    let mut values = Vec::new();
    for (name, row) in object {
        if name == "supported" { continue; }
        if !valid_level(name) || (!thinking && matches!(name.as_str(), "disabled" | "enabled" | "adaptive" | "between_tools")) {
            return None;
        }
        let supported_value = row.as_object()?.get("supported")?.as_bool()?;
        if supported_value { values.push(name.clone()); }
    }
    if supported == Some(false) && values.iter().any(|name| !thinking || name != "disabled") {
        return None;
    }
    Some((values, supported))
}

/// Real SIWC shape and explicitly named compatible metadata fields only.
pub fn from_metadata(v: &Value) -> ReasoningCapabilities {
    let candidates = [
        (v.get("supported_reasoning_levels"), true),
        (v.get("reasoning_effort_levels"), false),
        (v.pointer("/capabilities/reasoning_effort"), false),
        (v.pointer("/capabilities/reasoning/effort_levels"), false),
        (v.pointer("/reasoning/effort_levels"), false),
        (v.pointer("/effort/supported_levels"), false),
    ];
    let mut declared: Option<Vec<String>> = None;
    let mut effort_wire: Option<String> = None;
    for (value, objects) in candidates {
        if let Some(value) = value {
            let Some(current) = levels(value, objects).filter(|rows| !rows.iter().any(|name| matches!(name.as_str(), "disabled" | "enabled" | "adaptive" | "between_tools"))) else {
                return ReasoningCapabilities::unknown("上游推理档位声明格式无效，未采用部分结果");
            };
            if declared.as_ref().is_some_and(|old| {
                let mut a = old.clone();
                let mut b = current.clone();
                a.sort();
                b.sort();
                a != b
            }) {
                return ReasoningCapabilities::unknown("上游推理档位声明相互矛盾");
            }
            declared = Some(current);
        }
    }
    if v.pointer("/effort/supported_levels").is_some() {
        effort_wire = Some("reasoning_effort".into());
    }
    let mut explicitly_no_effort = false;
    if let Some(value) = v.pointer("/capabilities/effort") {
        let Some((current, supported)) = named_supported_values(value, false) else {
            return ReasoningCapabilities::unknown("上游命名推理强度声明格式无效或相互矛盾");
        };
        if effort_wire.is_some() {
            return ReasoningCapabilities::unknown("上游推理参数位置声明相互矛盾");
        }
        effort_wire = Some("output_config.effort".into());
        explicitly_no_effort = supported == Some(false);
        let named_rows = value.as_object().is_some_and(|rows| rows.keys().any(|key| key != "supported"));
        if !current.is_empty() || explicitly_no_effort || named_rows {
            if declared.as_ref().is_some_and(|old| {
                let mut a = old.clone();
                let mut b = current.clone();
                a.sort(); b.sort(); a != b
            }) {
                return ReasoningCapabilities::unknown("上游推理档位声明相互矛盾");
            }
            declared = Some(current);
        }
    }
    let mut thinking_modes = Vec::new();
    let mut declared_thinking = false;
    if let Some(value) = v.pointer("/capabilities/thinking") {
        let Some(object) = value.as_object() else {
            return ReasoningCapabilities::unknown("上游思考模式声明格式无效");
        };
        let supported = match object.get("supported") {
            Some(value) => match value.as_bool() {
                Some(value) => Some(value),
                None => return ReasoningCapabilities::unknown("上游思考模式声明格式无效"),
            },
            None => None,
        };
        if let Some(types) = object.get("types") {
            let Some((modes, _)) = named_supported_values(types, true) else {
                return ReasoningCapabilities::unknown("上游思考模式声明格式无效或相互矛盾");
            };
            if supported == Some(false) && modes.iter().any(|mode| mode != "disabled") {
                return ReasoningCapabilities::unknown("上游思考模式声明相互矛盾");
            }
            thinking_modes = modes;
            declared_thinking = true;
        }
    }
    let unsupported = v.get("reasoning") == Some(&Value::Bool(false))
        || v.pointer("/capabilities/reasoning") == Some(&Value::Bool(false));
    if unsupported && (declared.as_ref().is_some_and(|rows| !rows.is_empty())
        || thinking_modes.iter().any(|mode| mode != "disabled")) {
        return ReasoningCapabilities::unknown("上游推理能力声明相互矛盾");
    }
    let Some(levels) = declared else {
        let mut c = ReasoningCapabilities::unknown(if declared_thinking {
            "上游明确声明思考模式，但未声明推理强度档位；强度能力仍未知"
        } else { "上游未声明具体推理档位" });
        c.thinking_modes = thinking_modes;
        c.effort_wire = effort_wire;
        if unsupported && !declared_thinking {
            c.status = "unsupported".into();
            c.source = "upstream".into();
            c.reason = "上游明确声明不支持推理".into();
        } else if declared_thinking || c.effort_wire.is_some() {
            c.source = "upstream".into();
        }
        return c;
    };
    let mut default: Option<String> = None;
    for value in [
        v.get("default_reasoning_level"),
        v.pointer("/capabilities/reasoning/default_effort"),
        v.pointer("/effort/default_level"),
    ].into_iter().flatten() {
        if value.is_null() { continue; }
        let Some(value) = value.as_str().filter(|s| levels.iter().any(|level| level == s)) else {
            return ReasoningCapabilities::unknown("上游默认推理档位不在其声明列表中");
        };
        if default.as_deref().is_some_and(|old| old != value) {
            return ReasoningCapabilities::unknown("上游默认推理档位声明相互矛盾");
        }
        default = Some(value.to_owned());
    }
    ReasoningCapabilities {
        status: if explicitly_no_effort { "unsupported" } else if levels.is_empty() { "unknown" } else { "supported" }.into(),
        levels,
        thinking_modes,
        effort_wire,
        upstream_default: default.clone(),
        default_effort: default,
        configured_default: false,
        manual_levels: None,
        source: "upstream".into(),
        reason: "上游模型目录明确声明；思考模式与推理强度分别记录；未执行付费推理探测".into(),
        runtime_identity: None,
    }
}

/// Not returned in DTOs: binds metadata to credential/account, endpoint, headers,
/// and protocol identity. Does not change when a mapping is added or toggled.
pub(crate) fn fingerprint(c: &Connection, id: i64) -> db::Result<String> {
    let mut digest = Sha256::new();
    for sql in [
        "SELECT base_url FROM channels WHERE id=?1",
        "SELECT nonce,cipher FROM channel_secrets WHERE channel_id=?1",
        "SELECT name,value FROM channel_headers WHERE channel_id=?1 ORDER BY ordinal,id",
        "SELECT account_id FROM oauth_channel_bindings WHERE channel_id=?1",
    ] {
        digest.update(sql.as_bytes());
        let mut q = c.prepare(sql)?;
        let n = q.column_count();
        let mut rows = q.query([id])?;
        while let Some(row) = rows.next()? {
            for i in 0..n {
                let value = row.get_ref(i)?;
                let bytes = match value {
                    rusqlite::types::ValueRef::Null => vec![0],
                    rusqlite::types::ValueRef::Integer(n) => n.to_le_bytes().to_vec(),
                    rusqlite::types::ValueRef::Real(n) => n.to_le_bytes().to_vec(),
                    rusqlite::types::ValueRef::Text(s) | rusqlite::types::ValueRef::Blob(s) => {
                        s.to_vec()
                    }
                };
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(bytes);
            }
        }
    }
    let protocol = crate::protocol_config::load(c, id)?;
    digest.update(
        serde_json::to_vec(&protocol)
            .map_err(|_| DbError::Other("Invalid source protocol identity".into()))?,
    );
    Ok(hex::encode(digest.finalize()))
}
pub fn store_catalog(
    c: &Connection,
    id: i64,
    models: &[crate::upstream::UpstreamModel],
    now: i64,
) -> db::Result<()> {
    let identity = fingerprint(c, id)?;
    let runtime = runtime_profile(c, id)?;
    c.execute(
        "DELETE FROM channel_reasoning_catalog WHERE channel_id=?1",
        [id],
    )?;
    for m in models {
        let reasoning = if runtime { runtime_unknown() } else { m.reasoning.clone() };
        let json = serde_json::to_string(&reasoning)
            .map_err(|_| DbError::Other("Invalid reasoning metadata".into()))?;
        c.execute("INSERT INTO channel_reasoning_catalog(channel_id,upstream_model,source_identity,capability_json,fetched_at_ms) VALUES(?1,?2,?3,?4,?5)",params![id,m.id,identity,json,now])?;
    }
    Ok(())
}
pub fn for_model(c: &Connection, id: i64, upstream: &str) -> db::Result<ReasoningCapabilities> {
    for_model_with_identity(c, id, upstream, &fingerprint(c, id)?)
}
pub(crate) fn for_model_with_identity(
    c: &Connection,
    id: i64,
    upstream: &str,
    current_identity: &str,
) -> db::Result<ReasoningCapabilities> {
    if runtime_profile(c, id)? {
        let mut caps=runtime_unknown();
        caps.runtime_identity=Some(crate::protocol_config::load(c,id)?.runtime_description(upstream));
        return Ok(caps);
    }
    let row:Option<(String,String)>=c.query_row("SELECT source_identity,capability_json FROM channel_reasoning_catalog WHERE channel_id=?1 AND upstream_model=?2",params![id,upstream],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((identity, json)) = row else {
        return Ok(ReasoningCapabilities::default());
    };
    if identity != current_identity {
        return Ok(ReasoningCapabilities::unknown(
            "来源凭据、地址或协议已变化，请重新读取模型目录",
        ));
    }
    let mut caps: ReasoningCapabilities = serde_json::from_str(&json)
        .map_err(|_| DbError::Other("Invalid reasoning capability cache".into()))?;
    // A provider compatibility rule augments a valid same-identity declaration;
    // it never manufactures support from an empty, unsupported or invalid cache.
    let compatible = match compatibility_provider(c, id)? {
        Some(CompatibilityProvider::DeepSeek) => true,
        Some(CompatibilityProvider::OpenAi) => upstream == "gpt-6-luna",
        None => false,
    } || source_disable_adapter(c,id,upstream)?;
    if caps.status == "supported" && !caps.levels.is_empty()
        && !caps.levels.iter().any(|level| level == "none") && compatible
    {
        caps.levels.insert(0, "none".into());
        caps.source = "upstream_with_compatibility".into();
        caps.reason = "上游模型目录声明的档位，加上官方来源已验证的关闭推理兼容值；保留服务商默认，未对当前账号执行推理探测".into();
    }
    Ok(caps)
}

fn runtime_profile(c: &Connection, id: i64) -> db::Result<bool> {
    Ok(matches!(crate::protocol_config::load(c, id)?.parameter_profile.as_deref(), Some("ollama" | "vllm")))
}
fn runtime_unknown() -> ReasoningCapabilities {
    ReasoningCapabilities::unknown("运行时模型目录声明未验证；模板、解析器与运行时身份不证明推理档位，请独立确认")
}
/// Source-aware DTO boundary: a runtime's own model metadata cannot grant capabilities.
pub fn constrain_runtime_catalog(c: &Connection, id: i64, models: &mut [crate::upstream::UpstreamModel]) -> db::Result<()> {
    if runtime_profile(c, id)? {
        let config=crate::protocol_config::load(c,id)?;
        for model in models {
            model.reasoning = runtime_unknown();
            model.reasoning.runtime_identity=Some(config.runtime_description(&model.id));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum CompatibilityProvider { OpenAi, DeepSeek }

fn compatibility_provider(c: &Connection, id: i64) -> db::Result<Option<CompatibilityProvider>> {
    let raw: Option<String> = c.query_row("SELECT base_url FROM channels WHERE id=?1", [id], |row| row.get(0)).optional()?;
    let Some(url) = raw.and_then(|raw| url::Url::parse(&raw).ok()) else { return Ok(None); };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
        || url.port().is_some() || url.query().is_some() || url.fragment().is_some()
    { return Ok(None); }
    Ok(match (url.host_str(), url.path().trim_end_matches('/')) {
        (Some("api.openai.com"), "/v1") => Some(CompatibilityProvider::OpenAi),
        (Some("api.deepseek.com"), "" | "/v1" | "/anthropic/v1") => Some(CompatibilityProvider::DeepSeek),
        _ => None,
    })
}

/// A source-specific, documented off-switch mapping may supplement a list of
/// active effort levels. Absence of a validator error alone is not proof of off.
fn source_disable_adapter(c:&Connection,id:i64,upstream:&str)->db::Result<bool> {
    let Some(channel)=db::channels::get(c,tokengo_shared::ChannelId(id))? else{return Ok(false);};
    let config=crate::protocol_config::load(c,id)?;
    let protocol=config.native.first().copied().unwrap_or(Protocol::Chat);
    let context=crate::parameter_contracts::Context {base_url:&channel.base_url,model:upstream,protocol,
        api_version:None,is_oauth:false,profile_override:config.parameter_profile.as_deref()};
    let mut body=match protocol {Protocol::Chat=>serde_json::json!({"reasoning_effort":"none"}),
        Protocol::Responses=>serde_json::json!({"reasoning":{"effort":"none"}}),Protocol::Messages=>serde_json::json!({"output_config":{"effort":"none"}})};
    Ok(crate::parameter_contracts_china::adapt(&context,&mut body).is_ok_and(|changed|changed)
        && crate::parameter_contracts::validate(&context,&body).is_ok()
        && crate::parameter_contracts_china::validate(&context,&body).is_ok())
}

/// Validate explicit off intent against this mapping's effective capabilities.
/// Current converters preserve none; restoring it also supports older callers.
/// Known exclusions fail closed; unknown contracts retain the explicit request.
pub(crate) fn restore_disable_intent(
    c: &Connection, id: i64, model: &str, from: Protocol, to: Protocol, original: &Value, body: &mut Value,
) -> Result<bool, String> {
    let disabled = match from {
        Protocol::Responses => original.pointer("/reasoning/effort"),
        Protocol::Messages => original.pointer("/output_config/effort"),
        Protocol::Chat => None, // Chat->Responses already preserves its effort.
    }.and_then(Value::as_str) == Some("none");
    if from == to || !disabled { return Ok(false); }
    let caps = for_mapping(c, id, model)
        .map_err(|_| "Cannot read model reasoning configuration".to_owned())?;
    if caps.status == "unsupported"
        || (caps.status == "supported" && !caps.levels.iter().any(|level| level == "none"))
    {
        return Err("Requested reasoning none is not declared for this model on this connection".into());
    }
    if caps.status != "supported" { return Ok(false); }
    if to == Protocol::Chat {
        body["reasoning_effort"] = serde_json::json!("none");
    } else if to == Protocol::Responses {
        if body.get("reasoning").is_none() { body["reasoning"] = serde_json::json!({}); }
        body["reasoning"].as_object_mut().ok_or("Native reasoning configuration must be an object")?
            .insert("effort".into(), serde_json::json!("none"));
    } else {
        if body.get("output_config").is_none() { body["output_config"] = serde_json::json!({}); }
        body["output_config"].as_object_mut().ok_or("Native reasoning configuration must be an object")?
            .insert("effort".into(), serde_json::json!("none"));
    }
    Ok(true)
}

/// DeepSeek Chat/Messages use thinking.type as the off switch; effort selects
/// only the active-thinking effort. Validate before mutation, preserving fields.
fn deepseek_disable(protocol: Protocol, body: &mut Value) -> Result<bool, String> {
    let none = match protocol {
        Protocol::Chat => body.get("reasoning_effort"),
        Protocol::Responses => body.pointer("/reasoning/effort"),
        Protocol::Messages => body.pointer("/output_config/effort"),
    }.and_then(Value::as_str) == Some("none");
    if !none { return Ok(false); }
    if let Some(thinking) = body.get("thinking") {
        let object = thinking.as_object().ok_or("DeepSeek thinking must be an object")?;
        if object.len() != 1 || object.get("type").and_then(Value::as_str) != Some("disabled") {
            return Err("Reasoning none conflicts with active or unsupported DeepSeek thinking controls".into());
        }
    }
    if protocol == Protocol::Responses { return Ok(false); }
    if protocol == Protocol::Chat {
        body.as_object_mut().ok_or("Expected request object")?.remove("reasoning_effort");
    } else {
        let output = body["output_config"].as_object_mut().ok_or("Native reasoning configuration must be an object")?;
        output.remove("effort");
        if output.is_empty() { body.as_object_mut().unwrap().remove("output_config"); }
    }
    body["thinking"] = serde_json::json!({"type":"disabled"});
    Ok(true)
}

fn mapping(c: &Connection, id: i64, model: &str) -> db::Result<crate::db::channels::ModelMapping> {
    let channel = db::channels::get(c, tokengo_shared::ChannelId(id))?
        .ok_or_else(|| DbError::Other("连接不存在".into()))?;
    channel
        .models
        .into_iter()
        .find(|m| tokengo_shared::same_model_name(&m.model, model))
        .ok_or_else(|| DbError::Other("模型未配置在此连接".into()))
}
type Settings = (String, String, Option<String>, Option<String>);
fn settings(c: &Connection, id: i64, model: &str) -> db::Result<Option<Settings>> {
    Ok(c.query_row("SELECT source_identity,upstream_model,manual_levels,default_effort FROM channel_reasoning_settings WHERE channel_id=?1 AND model_name=?2",params![id,tokengo_shared::normalize_model_name(model)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?)
}
pub fn for_mapping(c: &Connection, id: i64, model: &str) -> db::Result<ReasoningCapabilities> {
    let m = mapping(c, id, model)?;
    for_loaded_mapping(c, id, &m)
}
/// The caller already loaded this mapping in the current connection snapshot.
pub(crate) fn for_loaded_mapping(c: &Connection, id: i64, m: &db::channels::ModelMapping) -> db::Result<ReasoningCapabilities> {
    let model = &m.model;
    let mut result = for_model(c, id, m.upstream.as_deref().unwrap_or(&m.model))?;
    if let Some((identity, upstream, manual, default)) = settings(c, id, model)? {
        if identity != fingerprint(c, id)? || upstream != m.upstream.as_deref().unwrap_or(&m.model)
        {
            let mut unknown =
                ReasoningCapabilities::unknown("来源身份或上游模型映射已变化，请重新确认推理配置");
            unknown.configured_default = true;
            return Ok(unknown);
        }
        result.configured_default = true;
        result.default_effort = default;
        if let Some(manual) = manual {
            let levels: Vec<String> = serde_json::from_str(&manual)
                .map_err(|_| DbError::Other("Invalid manual reasoning configuration".into()))?;
            result.levels = levels.clone();
            result.manual_levels = Some(levels);
            result.status = "supported".into();
            result.source = "user_confirmed".into();
            result.reason = "管理员明确确认的档位；不是自动探测结果".into();
        }
        if result
            .default_effort
            .as_ref()
            .is_some_and(|value| !result.levels.contains(value))
        {
            let mut unknown =
                ReasoningCapabilities::unknown("模型目录已变更，所选默认档位需重新确认");
            unknown.configured_default = true;
            return Ok(unknown);
        }
    }
    Ok(result)
}
pub fn configure(
    c: &Connection,
    id: i64,
    model: &str,
    manual: Option<&[String]>,
    default: Option<&str>,
) -> db::Result<ReasoningCapabilities> {
    let m = mapping(c, id, model)?;
    let detected = for_model(c, id, m.upstream.as_deref().unwrap_or(&m.model))?;
    if let Some(items) = manual {
        if levels(&serde_json::json!(items), false).is_none() {
            return Err(DbError::Other(
                "手工档位须为1–16个不重复的小写参数值，每项不超过32字节".into(),
            ));
        }
    }
    let available = manual.unwrap_or(&detected.levels);
    if default.is_some_and(|value| !valid_level(value) || !available.iter().any(|v| v == value)) {
        return Err(DbError::Other(
            "默认档位必须在当前已声明或手工确认的档位中".into(),
        ));
    }
    let json = manual
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| DbError::Other("Invalid manual reasoning configuration".into()))?;
    c.execute("INSERT INTO channel_reasoning_settings(channel_id,model_name,upstream_model,source_identity,manual_levels,default_effort,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(channel_id,model_name) DO UPDATE SET upstream_model=excluded.upstream_model,source_identity=excluded.source_identity,manual_levels=excluded.manual_levels,default_effort=excluded.default_effort,updated_at_ms=excluded.updated_at_ms",params![id,tokengo_shared::normalize_model_name(model),m.upstream.as_deref().unwrap_or(&m.model),fingerprint(c,id)?,json,default,db::now_ms()])?;
    c.execute(
        "UPDATE channels SET binding_generation=binding_generation+1 WHERE id=?1",
        [id],
    )?;
    for_mapping(c, id, model)
}
/// Applies only defaults for this mapping; explicit client effort always wins.
/// Returns whether source defaults are superseded or the native wire body changed.
pub fn apply_defaults_and_validate(
    c: &Connection,
    id: i64,
    model: &str,
    _upstream: &str,
    protocol: Protocol,
    body: &mut Value,
) -> Result<bool, String> {
    let caps = for_mapping(c, id, model)
        .map_err(|_| "Cannot read model reasoning configuration".to_owned())?;
    let deepseek = compatibility_provider(c, id)
        .map_err(|_| "Cannot read source reasoning identity")? == Some(CompatibilityProvider::DeepSeek);
    let key = match protocol {
        Protocol::Chat => "reasoning_effort",
        Protocol::Responses => "reasoning",
        Protocol::Messages => "output_config",
    };
    let existing = if protocol == Protocol::Chat {
        body.get(key)
    } else {
        body.get(key).and_then(|v| v.get("effort"))
    };
    if let Some(value) = existing {
        let value = value
            .as_str()
            .filter(|s| valid_level(s))
            .ok_or("Reasoning effort must be a bounded native parameter value")?;
        // DeepSeek documents native effort aliases beyond its catalog's recommended
        // levels. Preserve accepted client payloads rather than narrowing them after
        // adding catalog discovery. A manual list remains an explicit restriction.
        let native_alias = if deepseek && caps.manual_levels.is_none() {
            match value {
                "minimal" => Some("low"),
                "medium" | "xhigh" => Some("high"),
                "ultra" => Some("max"),
                _ => None,
            }.is_some_and(|canonical| caps.levels.iter().any(|level| level == canonical))
        } else { false };
        if caps.status == "supported" && !caps.levels.iter().any(|level| level == value) && !native_alias {
            return Err(
                "Requested reasoning effort is not declared for this model on this connection"
                    .into(),
            );
        }
        if caps.status == "unsupported" {
            return Err("This model explicitly declares no reasoning support".into());
        }
    } else if caps.configured_default
        && !(deepseek && protocol != Protocol::Responses && body.get("thinking").is_some())
    {
        if caps.status == "unknown"
            && settings(c, id, model)
                .map_err(|_| "Cannot read reasoning settings")?
                .is_some_and(|row| row.3.is_some())
        {
            return Err(
                "Configured reasoning default needs reconfirmation after source or catalog change"
                    .into(),
            );
        }
        if let Some(default) = caps.default_effort.as_ref() {
            if protocol == Protocol::Chat {
                body[key] = serde_json::json!(default);
            } else {
                if body.get(key).is_none() {
                    body[key] = serde_json::json!({});
                }
                let object = body[key]
                    .as_object_mut()
                    .ok_or("Native reasoning configuration must be an object")?;
                object.insert("effort".into(), serde_json::json!(default));
            }
        }
    }
    let wire_changed = deepseek && deepseek_disable(protocol, body)?;
    Ok(caps.configured_default || wire_changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn siwc_levels_and_default_are_read_without_descriptions_or_name_guesses() {
        let c = from_metadata(
            &json!({"slug":"arbitrary","supported_reasoning_levels":[{"effort":"low","description":"private text"},{"effort":"xhigh"},{"effort":"max"},{"effort":"ultra"}],"default_reasoning_level":"low"}),
        );
        assert_eq!(c.levels, vec!["low", "xhigh", "max", "ultra"]);
        assert_eq!(c.default_effort.as_deref(), Some("low"));
        assert!(!serde_json::to_string(&c).unwrap().contains("private"));
    }
    #[test]
    fn booleans_absence_and_names_do_not_invent_levels() {
        for v in [
            json!({"slug":"gpt-low-high-ultra"}),
            json!({"reasoning":true}),
            json!({"capabilities":{"reasoning":true}}),
        ] {
            let c = from_metadata(&v);
            assert_eq!(c.status, "unknown");
            assert!(c.levels.is_empty());
        }
        assert_eq!(
            from_metadata(&json!({"reasoning":false})).status,
            "unsupported"
        );
    }
    #[test]
    fn malformed_conflicting_or_unknown_defaults_fail_closed() {
        for v in [
            json!({"supported_reasoning_levels":[{"effort":"high"},{"effort":"high"}]}),
            json!({"capabilities":{"reasoning_effort":["high",4]}}),
            json!({"capabilities":{"reasoning_effort":["high"]},"reasoning_effort_levels":["low"]}),
            json!({"capabilities":{"reasoning_effort":["high"]},"default_reasoning_level":"low"}),
            json!({"reasoning":false,"capabilities":{"reasoning_effort":["high"]}}),
        ] {
            assert_eq!(from_metadata(&v).status, "unknown");
        }
    }
    #[test]
    fn cache_is_scoped_to_exact_source_and_upstream_but_not_mapping_changes() {
        let c = db::open_in_memory().unwrap();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "Source".into(),
                base_url: "http://example.test".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        let models = crate::upstream::parse_model_catalog(
            r#"{"models":[{"slug":"exact","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}],"default_reasoning_level":"low"}]}"#,
        );
        store_catalog(&c, id.0, &models, 1).unwrap();
        assert_eq!(
            for_model(&c, id.0, "exact").unwrap().levels,
            vec!["low", "high"]
        );
        assert_eq!(for_model(&c, id.0, "another").unwrap().status, "unknown");
        assert_eq!(for_model(&c, id.0, "Exact").unwrap().status, "unknown");
        crate::protocol_config::save(&c, id.0, &Default::default()).unwrap();
        assert_eq!(for_model(&c, id.0, "exact").unwrap().status, "supported");
        db::channels::set_models(
            &c,
            id,
            &[db::channels::ModelMapping::new(
                "public",
                Some("exact".into()),
                false,
            )],
        )
        .unwrap();
        assert_eq!(for_mapping(&c, id.0, "public").unwrap().status, "supported");
        c.execute(
            "UPDATE channels SET base_url='http://other.test' WHERE id=?1",
            [id.0],
        )
        .unwrap();
        assert_eq!(for_mapping(&c, id.0, "public").unwrap().status, "unknown");
    }
    #[test]
    fn defaults_are_per_logical_mapping_and_explicit_native_effort_wins() {
        let c = db::open_in_memory().unwrap();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "Source".into(),
                base_url: "http://example.test".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        db::channels::set_models(
            &c,
            id,
            &[
                db::channels::ModelMapping::new("canonical", Some("exact".into()), false),
                db::channels::ModelMapping::new("legacy-low", Some("exact".into()), false),
            ],
        )
        .unwrap();
        configure(
            &c,
            id.0,
            "canonical",
            Some(&["low".into(), "high".into()]),
            Some("high"),
        )
        .unwrap();
        let mut body = json!({"reasoning":{"effort":"low","summary":"none"}});
        assert!(apply_defaults_and_validate(
            &c,
            id.0,
            "canonical",
            "exact",
            Protocol::Responses,
            &mut body
        )
        .unwrap());
        assert_eq!(body["reasoning"]["effort"], "low");
        let mut body = json!({});
        apply_defaults_and_validate(&c, id.0, "canonical", "exact", Protocol::Chat, &mut body)
            .unwrap();
        assert_eq!(body["reasoning_effort"], "high");
        assert!(
            !for_mapping(&c, id.0, "legacy-low")
                .unwrap()
                .configured_default
        );
        assert!(configure(&c, id.0, "canonical", None, Some("max")).is_err());
        configure(&c, id.0, "canonical", None, None).unwrap();
        let mut body = json!({});
        assert!(apply_defaults_and_validate(
            &c,
            id.0,
            "canonical",
            "exact",
            Protocol::Responses,
            &mut body
        )
        .unwrap());
        assert!(body.get("reasoning").is_none());
    }
    #[test]
    fn remapping_alias_to_another_upstream_never_reuses_manual_claims() {
        let c = db::open_in_memory().unwrap();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "Source".into(),
                base_url: "http://example.test".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        db::channels::set_models(
            &c,
            id,
            &[db::channels::ModelMapping::new(
                "alias",
                Some("first".into()),
                false,
            )],
        )
        .unwrap();
        configure(&c, id.0, "alias", Some(&["high".into()]), Some("high")).unwrap();
        db::channels::set_models(
            &c,
            id,
            &[db::channels::ModelMapping::new(
                "alias",
                Some("second".into()),
                false,
            )],
        )
        .unwrap();
        let stale = for_mapping(&c, id.0, "alias").unwrap();
        assert_eq!(stale.status, "unknown");
        assert!(stale.levels.is_empty());
        assert!(stale.default_effort.is_none());
        let mut body = json!({});
        assert!(apply_defaults_and_validate(
            &c,
            id.0,
            "alias",
            "second",
            Protocol::Chat,
            &mut body
        )
        .is_err());
        assert!(body.get("reasoning_effort").is_none());
    }
    #[test]
    fn catalog_removing_a_configured_default_never_restores_legacy_defaults() {
        let c = db::open_in_memory().unwrap();
        let id = db::channels::create(
            &c,
            &db::channels::NewChannel {
                name: "Source".into(),
                base_url: "http://example.test".into(),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap();
        db::channels::set_models(
            &c,
            id,
            &[db::channels::ModelMapping::new(
                "canonical",
                Some("exact".into()),
                false,
            )],
        )
        .unwrap();
        let before = crate::upstream::parse_model_catalog(
            r#"{"data":[{"id":"exact","capabilities":{"reasoning_effort":["low","high"]}}]}"#,
        );
        store_catalog(&c, id.0, &before, 1).unwrap();
        configure(&c, id.0, "canonical", None, Some("high")).unwrap();
        let after = crate::upstream::parse_model_catalog(
            r#"{"data":[{"id":"exact","capabilities":{"reasoning_effort":["low"]}}]}"#,
        );
        store_catalog(&c, id.0, &after, 2).unwrap();
        assert!(
            for_mapping(&c, id.0, "canonical")
                .unwrap()
                .configured_default
        );
        let mut body = json!({});
        assert!(apply_defaults_and_validate(
            &c,
            id.0,
            "canonical",
            "exact",
            Protocol::Responses,
            &mut body
        )
        .is_err());
        assert!(body.get("reasoning").is_none());
        configure(&c, id.0, "canonical", None, None).unwrap();
        assert!(apply_defaults_and_validate(
            &c,
            id.0,
            "canonical",
            "exact",
            Protocol::Responses,
            &mut body
        )
        .unwrap());
    }

    fn portable_source(url: &str, upstream: &str, mut metadata: Value) -> (Connection, i64) {
        let c = db::open_in_memory().unwrap();
        let id = db::channels::create(&c, &db::channels::NewChannel {
            name: "Synthetic provider".into(), base_url: url.into(), enabled: true,
            unwrap_envelope: false, max_concurrent: 0,
        }).unwrap();
        db::channels::set_models(&c, id, &[db::channels::ModelMapping::new("public", Some(upstream.into()), false)]).unwrap();
        metadata["id"] = json!(upstream);
        let models = crate::upstream::parse_model_catalog(&json!({"data":[metadata]}).to_string());
        store_catalog(&c, id.0, &models, 1).unwrap();
        (c, id.0)
    }

    fn deepseek_metadata() -> Value {
        json!({"effort":{"supported_levels":["low","high","max"],"default_level":"high"}})
    }

    #[test]
    fn deepseek_effort_metadata_is_explicit_and_conflicts_fail_closed() {
        let caps = from_metadata(&deepseek_metadata());
        assert_eq!(caps.levels, vec!["low", "high", "max"]);
        assert_eq!(caps.default_effort.as_deref(), Some("high"));
        assert!(!caps.levels.contains(&"none".into()), "raw upstream declaration must remain truthful");
        for invalid in [
            json!({"effort":{"supported_levels":["low",4]}}),
            json!({"effort":{"supported_levels":["high","high"]}}),
            json!({"effort":{"supported_levels":["high"],"default_level":"low"}}),
            json!({"effort":{"supported_levels":["high"]},"reasoning":false}),
            json!({"effort":{"supported_levels":["high"]},"reasoning_effort_levels":["low"]}),
            json!({"effort":{"supported_levels":["low","high"],"default_level":"high"},"default_reasoning_level":"low"}),
        ] {
            assert_eq!(from_metadata(&invalid).status, "unknown", "{invalid}");
        }
    }

    #[test]
    fn portable_none_augments_only_exact_official_provider_contracts() {
        for url in ["https://api.deepseek.com", "https://api.deepseek.com/v1/", "https://api.deepseek.com/anthropic/v1"] {
            let (c, id) = portable_source(url, "arbitrary-upstream", deepseek_metadata());
            let before: String = c.query_row("SELECT capability_json FROM channel_reasoning_catalog WHERE channel_id=?1", [id], |row| row.get(0)).unwrap();
            let caps = for_mapping(&c, id, "public").unwrap();
            assert_eq!(caps.levels, vec!["none", "low", "high", "max"]);
            assert_eq!(caps.source, "upstream_with_compatibility");
            assert_eq!(caps.default_effort.as_deref(), Some("high"));
            assert!(!caps.configured_default);
            assert!(caps.manual_levels.is_none());
            let after: String = c.query_row("SELECT capability_json FROM channel_reasoning_catalog WHERE channel_id=?1", [id], |row| row.get(0)).unwrap();
            assert_eq!(before, after);
        }
        let (c, id) = portable_source("https://api.openai.com/v1", "gpt-6-luna", deepseek_metadata());
        assert!(for_model(&c, id, "gpt-6-luna").unwrap().levels.contains(&"none".into()));
        let (c, id) = portable_source("https://api.openai.com/v1", "another-gpt", deepseek_metadata());
        assert!(!for_model(&c, id, "another-gpt").unwrap().levels.contains(&"none".into()));
    }

    #[test]
    fn provider_names_and_lookalike_endpoints_do_not_grant_none() {
        for url in [
            "http://api.deepseek.com", "https://api.deepseek.com.attacker.test/v1",
            "https://api.deepseek.com:8443/v1", "https://api.deepseek.com/proxy/v1",
            "https://api.deepseek.com/v1?other=1", "https://name@api.deepseek.com/v1",
            "https://api.openai.com/proxy/v1", "https://proxy.example.test/v1",
        ] {
            let (c, id) = portable_source(url, "gpt-6-luna", deepseek_metadata());
            assert!(!for_model(&c, id, "gpt-6-luna").unwrap().levels.contains(&"none".into()), "{url}");
            let mut body = json!({"reasoning_effort":"none"});
            configure(&c, id, "public", Some(&["none".into(), "high".into()]), None).unwrap();
            apply_defaults_and_validate(&c, id, "public", "gpt-6-luna", Protocol::Chat, &mut body).unwrap();
            assert_eq!(body, json!({"reasoning_effort":"none"}));
        }
    }

    #[test]
    fn none_compatibility_never_revives_unknown_unsupported_or_stale_catalogs() {
        for metadata in [json!({}), json!({"reasoning":true}), json!({"reasoning":false}), json!({"effort":{"supported_levels":[]}})] {
            let (c, id) = portable_source("https://api.deepseek.com/v1", "exact", metadata);
            assert!(!for_model(&c, id, "exact").unwrap().levels.contains(&"none".into()));
        }
        let (c, id) = portable_source("https://api.deepseek.com/v1", "exact", deepseek_metadata());
        assert_eq!(for_model(&c, id, "missing").unwrap().status, "unknown");
        c.execute("UPDATE channels SET base_url='https://api.deepseek.com/anthropic/v1' WHERE id=?1", [id]).unwrap();
        assert_eq!(for_model(&c, id, "exact").unwrap().status, "unknown", "identity changes still require catalog refresh");
    }

    #[test]
    fn none_manual_exclusion_and_explicit_default_remain_authoritative() {
        let (c, id) = portable_source("https://api.deepseek.com/v1", "exact", deepseek_metadata());
        configure(&c, id, "public", Some(&["high".into()]), Some("high")).unwrap();
        let caps = for_mapping(&c, id, "public").unwrap();
        assert_eq!(caps.levels, vec!["high"]);
        assert_eq!(caps.source, "user_confirmed");
        let mut body = json!({"reasoning_effort":"none"});
        assert!(apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Chat, &mut body).is_err());
        configure(&c, id, "public", None, Some("none")).unwrap();
        let mut body = json!({});
        assert!(apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Chat, &mut body).unwrap());
        assert_eq!(body, json!({"thinking":{"type":"disabled"}}));
        let mut explicit = json!({"thinking":{"type":"enabled"}});
        apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Chat, &mut explicit).unwrap();
        assert_eq!(explicit, json!({"thinking":{"type":"enabled"}}), "native explicit client intent wins over configured default");
        configure(&c, id, "public", None, Some("high")).unwrap();
        let mut disabled = json!({"thinking":{"type":"disabled"}});
        apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Chat, &mut disabled).unwrap();
        assert_eq!(disabled, json!({"thinking":{"type":"disabled"}}));
    }

    #[test]
    fn deepseek_none_uses_native_wire_toggle_and_reports_mutation_without_defaults() {
        let (c, id) = portable_source("https://api.deepseek.com/v1", "exact", deepseek_metadata());
        let mut chat = json!({"reasoning_effort":"none","extra":"preserved"});
        assert!(apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Chat, &mut chat).unwrap());
        assert_eq!(chat, json!({"thinking":{"type":"disabled"},"extra":"preserved"}));
        let mut messages = json!({"output_config":{"effort":"none","format":{"type":"text"}},"extra":"preserved"});
        assert!(apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Messages, &mut messages).unwrap());
        assert_eq!(messages, json!({"output_config":{"format":{"type":"text"}},"thinking":{"type":"disabled"},"extra":"preserved"}));
        let mut responses = json!({"reasoning":{"effort":"none","summary":"none"}});
        assert!(!apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Responses, &mut responses).unwrap());
        assert_eq!(responses, json!({"reasoning":{"effort":"none","summary":"none"}}));
        let mut untouched = json!({"messages":[]});
        assert!(!apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Chat, &mut untouched).unwrap());
        assert_eq!(untouched, json!({"messages":[]}));
    }

    #[test]
    fn deepseek_none_rejects_conflicting_or_unknown_thinking_without_mutation() {
        let (c, id) = portable_source("https://api.deepseek.com/v1", "exact", deepseek_metadata());
        for thinking in [json!({"type":"enabled"}), json!({"type":"disabled","budget_tokens":10}), json!({"type":"adaptive"}), json!(true), Value::Null] {
            for protocol in [Protocol::Chat, Protocol::Messages, Protocol::Responses] {
                let mut body = match protocol {
                    Protocol::Chat => json!({"reasoning_effort":"none","thinking":thinking.clone()}),
                    Protocol::Messages => json!({"output_config":{"effort":"none"},"thinking":thinking.clone()}),
                    Protocol::Responses => json!({"reasoning":{"effort":"none"},"thinking":thinking.clone()}),
                };
                let before = body.clone();
                assert!(apply_defaults_and_validate(&c, id, "public", "exact", protocol, &mut body).is_err());
                assert_eq!(before, body);
            }
        }
    }

    #[test]
    fn deepseek_catalog_discovery_preserves_documented_native_client_aliases() {
        let (c, id) = portable_source("https://api.deepseek.com/v1", "exact", deepseek_metadata());
        for alias in ["minimal", "medium", "xhigh", "ultra"] {
            for protocol in [Protocol::Chat, Protocol::Responses, Protocol::Messages] {
                let mut body = match protocol {
                    Protocol::Chat => json!({"reasoning_effort":alias}),
                    Protocol::Responses => json!({"reasoning":{"effort":alias}}),
                    Protocol::Messages => json!({"output_config":{"effort":alias}}),
                };
                let before = body.clone();
                apply_defaults_and_validate(&c, id, "public", "exact", protocol, &mut body).unwrap();
                assert_eq!(body, before, "native aliases are preserved, never rewritten");
            }
        }
        configure(&c, id, "public", Some(&["high".into()]), None).unwrap();
        assert!(apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Chat, &mut json!({"reasoning_effort":"medium"})).is_err());
    }

    #[test]
    fn messages_none_conversion_does_not_reenable_deepseek_reasoning() {
        let original = json!({"model":"public","max_tokens":32,"messages":[{"role":"user","content":"hello"}],"output_config":{"effort":"none"}});
        let (c, id) = portable_source("https://api.deepseek.com/v1", "exact", deepseek_metadata());
        configure(&c, id, "public", None, Some("high")).unwrap();
        for to in [Protocol::Chat, Protocol::Responses] {
            let mut converted = crate::protocol::convert_request(Protocol::Messages, to, &original).unwrap();
            assert!(restore_disable_intent(&c, id, "public", Protocol::Messages, to, &original, &mut converted).unwrap());
            apply_defaults_and_validate(&c, id, "public", "exact", to, &mut converted).unwrap();
            if to == Protocol::Chat {
                assert_eq!(converted["thinking"], json!({"type":"disabled"}));
                assert!(converted.get("reasoning_effort").is_none());
            } else {
                assert_eq!(converted["reasoning"]["effort"], "none");
            }
        }
    }

    #[test]
    fn responses_none_conversion_restores_declared_deepseek_off_intent() {
        let original = json!({"model":"public","input":"hello","reasoning":{"effort":"none"}});
        let (c, id) = portable_source("https://api.deepseek.com/v1", "exact", deepseek_metadata());
        let mut converted = crate::protocol::convert_request(Protocol::Responses, Protocol::Chat, &original).unwrap();
        assert_eq!(converted["reasoning_effort"], "none", "the generic converter preserves explicit off intent");
        assert!(restore_disable_intent(&c, id, "public", Protocol::Responses, Protocol::Chat, &original, &mut converted).unwrap());
        assert!(apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Chat, &mut converted).unwrap());
        assert_eq!(converted["thinking"], json!({"type":"disabled"}));
        let mut messages = json!({"output_config":{"format":{"type":"text"}}});
        assert!(restore_disable_intent(&c, id, "public", Protocol::Responses, Protocol::Messages, &original, &mut messages).unwrap());
        apply_defaults_and_validate(&c, id, "public", "exact", Protocol::Messages, &mut messages).unwrap();
        assert_eq!(messages["thinking"], json!({"type":"disabled"}));
        assert_eq!(messages["output_config"], json!({"format":{"type":"text"}}));
        let (other, oid) = portable_source("https://proxy.example.test/v1", "exact", deepseek_metadata());
        let mut body = json!({});
        assert!(restore_disable_intent(&other, oid, "public", Protocol::Responses, Protocol::Chat, &original, &mut body).is_err(), "known third-party declaration excludes none");
        assert_eq!(body, json!({}));
        assert!(!restore_disable_intent(&c, id, "public", Protocol::Chat, Protocol::Chat, &json!({"reasoning_effort":"none"}), &mut body).unwrap());
        assert_eq!(body, json!({}));
    }


    #[test]
    fn cross_protocol_none_preserves_openai_and_explicit_third_party_capabilities() {
        for (url, upstream, metadata) in [
            ("https://api.openai.com/v1", "gpt-6-luna", deepseek_metadata()),
            ("https://proxy.example.test/v1", "synthetic-model", json!({"reasoning_effort_levels":["none","high"],"default_reasoning_level":"high"})),
        ] {
            let (c, id) = portable_source(url, upstream, metadata);
            configure(&c, id, "public", None, Some("high")).unwrap();
            for (from, to, original) in [
                (Protocol::Messages, Protocol::Responses, json!({"model":"public","max_tokens":32,"messages":[{"role":"user","content":"hello"}],"output_config":{"effort":"none"}})),
                (Protocol::Responses, Protocol::Chat, json!({"model":"public","max_output_tokens":32,"input":"hello","reasoning":{"effort":"none"}})),
            ] {
                let mut converted = crate::protocol::convert_request(from, to, &original).unwrap();
                assert!(restore_disable_intent(&c, id, "public", from, to, &original, &mut converted).unwrap());
                apply_defaults_and_validate(&c, id, "public", upstream, to, &mut converted).unwrap();
                let effort = if to == Protocol::Chat { converted.get("reasoning_effort") } else { converted.pointer("/reasoning/effort") };
                assert_eq!(effort.and_then(Value::as_str), Some("none"), "{url}");
                assert!(converted.get("thinking").is_none(), "DeepSeek wire rules never apply to another source");
            }
            let mut messages = json!({"output_config":{"format":{"type":"text"}}});
            assert!(restore_disable_intent(&c, id, "public", Protocol::Responses, Protocol::Messages,
                &json!({"reasoning":{"effort":"none"}}), &mut messages).unwrap());
            assert_eq!(messages["output_config"], json!({"effort":"none","format":{"type":"text"}}));
        }
    }

    #[test]
    fn cross_protocol_none_rejects_known_exclusions_and_preserves_unknown_legacy_contract() {
        let original = json!({"model":"public","input":"hello","reasoning":{"effort":"none"}});
        for (metadata, manual) in [
            (json!({"reasoning":false}), false),
            (json!({"reasoning_effort_levels":["high"]}), false),
            (json!({"reasoning_effort_levels":["none","high"]}), true),
        ] {
            let (c, id) = portable_source("https://proxy.example.test/v1", "exact", metadata);
            if manual { configure(&c, id, "public", Some(&["high".into()]), Some("high")).unwrap(); }
            for to in [Protocol::Chat, Protocol::Messages] {
                let mut converted = json!({"model":"exact"});
                let before = converted.clone();
                assert!(restore_disable_intent(&c, id, "public", Protocol::Responses, to, &original, &mut converted).is_err());
                assert_eq!(converted, before, "rejection cannot replace none with a configured active effort");
            }
            let mut converted = json!({"model":"exact"});
            assert!(restore_disable_intent(&c, id, "public", Protocol::Messages, Protocol::Responses,
                &json!({"output_config":{"effort":"none"}}), &mut converted).is_err());
        }
        let (c, id) = portable_source("https://proxy.example.test/v1", "exact", json!({}));
        let mut converted = crate::protocol::convert_request(Protocol::Responses, Protocol::Chat, &original).unwrap();
        let before = converted.clone();
        assert!(!restore_disable_intent(&c, id, "public", Protocol::Responses, Protocol::Chat, &original, &mut converted).unwrap());
        assert_eq!(converted, before);
        assert_eq!(converted["reasoning_effort"], "none", "unknown capability is not permission to delete explicit off intent; the source may reject it");
    }

}

#[cfg(test)]
mod named_metadata_tests {
    use super::*;
    use serde_json::json;

    fn claude_metadata() -> Value {
        json!({"capabilities":{
            "effort":{"supported":true,"low":{"supported":true},"high":{"supported":true},"max":{"supported":false}},
            "thinking":{"supported":true,"types":{"adaptive":{"supported":true},"between_tools":{"supported":true},"disabled":{"supported":false},"enabled":{"supported":false}}}
        },"default_reasoning_level":"high"})
    }

    #[test]
    fn claude_named_boolean_support_keeps_modes_separate_from_strengths() {
        let caps = from_metadata(&claude_metadata());
        assert_eq!(caps.status, "supported");
        assert_eq!(caps.levels.len(), 2);
        assert!(caps.levels.contains(&"low".into()));
        assert!(caps.levels.contains(&"high".into()));
        assert!(!caps.levels.contains(&"max".into()));
        assert_eq!(caps.thinking_modes.len(), 2);
        assert!(caps.thinking_modes.contains(&"adaptive".into()));
        assert!(caps.thinking_modes.contains(&"between_tools".into()));
        assert!(!caps.thinking_modes.contains(&"disabled".into()));
        assert!(caps.thinking_modes.iter().all(|mode| !caps.levels.contains(mode)));
        assert_eq!(caps.effort_wire.as_deref(), Some("output_config.effort"));
        assert_eq!(caps.default_effort.as_deref(), Some("high"));
    }

    #[test]
    fn modes_without_effort_do_not_claim_or_deny_strength_levels() {
        for v in [
            json!({"capabilities":{"thinking":{"supported":true,"types":{"adaptive":{"supported":true},"disabled":{"supported":false}}}}}),
            json!({"capabilities":{"thinking":{"supported":false,"types":{"disabled":{"supported":true},"enabled":{"supported":false}}}}}),
        ] {
            let caps = from_metadata(&v);
            assert_eq!(caps.status, "unknown");
            assert!(caps.levels.is_empty());
            assert_eq!(caps.thinking_modes.len(), 1);
            assert!(caps.reason.contains("强度"));
            assert!(caps.effort_wire.is_none());
        }
        let caps = from_metadata(&json!({"capabilities":{"effort":{"supported":true},"thinking":{"supported":true}}}));
        assert_eq!(caps.status, "unknown");
        assert!(caps.levels.is_empty());
    }

    #[test]
    fn named_metadata_invalid_types_and_conflicts_clear_partial_results() {
        for v in [
            json!({"capabilities":{"effort":{"high":{"supported":"true"}}}}),
            json!({"capabilities":{"effort":{"high":true}}}),
            json!({"capabilities":{"effort":{"supported":false,"high":{"supported":true}}}}),
            json!({"capabilities":{"effort":{"adaptive":{"supported":true}}}}),
            json!({"capabilities":{"effort":{"high":{"supported":true}},"thinking":{"types":{"disabled":{"supported":1}}}}}),
            json!({"capabilities":{"thinking":{"supported":false,"types":{"adaptive":{"supported":true}}}}}),
            json!({"capabilities":{"effort":{"high":{"supported":true}}},"reasoning_effort_levels":["low"]}),
            json!({"capabilities":{"effort":{"high":{"supported":false}}},"reasoning_effort_levels":["high"]}),
            json!({"capabilities":{"effort":{"high":{"supported":true}}},"effort":{"supported_levels":["high"]}}),
            json!({"reasoning":false,"capabilities":{"effort":{"high":{"supported":true}}}}),
            json!({"reasoning_effort_levels":["disabled","high"]}),
            json!({"reasoning_effort_levels":["between_tools"]}),
        ] {
            let caps = from_metadata(&v);
            assert_eq!(caps.status, "unknown", "{v}");
            assert!(caps.levels.is_empty(), "{v}");
            assert!(caps.thinking_modes.is_empty(), "{v}");
            assert!(caps.effort_wire.is_none(), "{v}");
        }
    }

    #[test]
    fn false_and_omitted_flags_never_invent_supported_values() {
        let caps = from_metadata(&json!({"capabilities":{"effort":{"supported":false,"high":{"supported":false}}}}));
        assert_eq!(caps.status, "unsupported");
        assert!(caps.levels.is_empty());
        let caps = from_metadata(&json!({"capabilities":{"effort":{"high":{"supported":false}}}}));
        assert_eq!(caps.status, "unknown");
        assert!(caps.levels.is_empty());
        assert!(from_metadata(&json!({"capabilities":{"thinking":{"types":{"adaptive":{}}}}})).thinking_modes.is_empty());
    }

    #[test]
    fn origin_wire_is_only_assigned_for_documented_explicit_metadata_shapes() {
        let caps = from_metadata(&json!({"effort":{"supported_levels":["low","high"],"default_level":"high"}}));
        assert_eq!(caps.effort_wire.as_deref(), Some("reasoning_effort"));
        assert_eq!(caps.upstream_default.as_deref(), Some("high"));
        for v in [
            json!({"supported_reasoning_levels":[{"effort":"high"}]}),
            json!({"reasoning_effort_levels":["high"]}),
            json!({"reasoning":true}),
        ] {
            assert!(from_metadata(&v).effort_wire.is_none());
        }
    }

    #[test]
    fn old_cache_deserializes_and_empty_new_fields_remain_omitted() {
        let old = json!({"status":"supported","levels":["high"],"default_effort":"high","upstream_default":"high","configured_default":false,"manual_levels":null,"source":"upstream","reason":"old cache"});
        let caps: ReasoningCapabilities = serde_json::from_value(old.clone()).unwrap();
        assert!(caps.thinking_modes.is_empty());
        assert!(caps.effort_wire.is_none());
        assert_eq!(serde_json::to_value(caps).unwrap(), old);
        let caps = from_metadata(&claude_metadata());
        assert_eq!(serde_json::from_value::<ReasoningCapabilities>(serde_json::to_value(&caps).unwrap()).unwrap(), caps);
    }

    #[test]
    fn manually_confirmed_levels_do_not_erase_source_modes_or_origin() {
        let c = db::open_in_memory().unwrap();
        let id = db::channels::create(&c, &db::channels::NewChannel {
            name:"Named metadata source".into(),base_url:"https://api.anthropic.com/v1".into(),
            enabled:true,unwrap_envelope:false,max_concurrent:0,
        }).unwrap();
        db::channels::set_models(&c,id,&[db::channels::ModelMapping::new("public",Some("exact-model".into()),false)]).unwrap();
        let mut metadata = claude_metadata();
        metadata["id"] = json!("exact-model");
        let models = crate::upstream::parse_model_catalog(&json!({"data":[metadata]}).to_string());
        store_catalog(&c,id.0,&models,1).unwrap();
        configure(&c,id.0,"public",Some(&["low".into()]),None).unwrap();
        let caps = for_mapping(&c,id.0,"public").unwrap();
        assert_eq!(caps.levels, vec!["low"]);
        assert_eq!(caps.thinking_modes.len(),2);
        assert_eq!(caps.effort_wire.as_deref(),Some("output_config.effort"));
    }
}
