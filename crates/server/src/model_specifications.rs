//! Administrator-maintained model metadata used for discovery and price defaults.
use crate::{db, model_registry};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Metric {
    pub value: i64,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct PublicSpecification {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_usage_reference: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_usage_reference: Option<i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, Metric>,
    pub origin: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Specification {
    pub model_id: String,
    pub provider_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(flatten)]
    pub public: PublicSpecification,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SpecificationInput {
    pub model_id: String,
    pub provider_id: Option<i64>,
    pub source_url: Option<String>,
    pub kind: String,
    pub context_window: Option<i64>,
    pub input_usage_reference: Option<i64>,
    pub output_usage_reference: Option<i64>,
    #[serde(default)]
    pub metrics: BTreeMap<String, Metric>,
    // A read-back row may round-trip through PUT. The server assigns origin.
    #[serde(default, rename = "origin")]
    pub _origin: Option<String>,
}

impl SpecificationInput {
    pub fn validate(self) -> Result<Specification, &'static str> {
        validate_model_id(&self.model_id)?;
        if self.provider_id.is_some_and(|id| id <= 0) {
            return Err("供应商编号无效");
        }
        if !matches!(
            self.kind.as_str(),
            "unknown" | "chat" | "embeddings" | "rerank" | "image" | "video"
                | "audio" | "asr" | "tts" | "tools"
        ) {
            return Err("模型类型无效");
        }
        if self.context_window.is_some_and(|value| value <= 0)
            || self.input_usage_reference.is_some_and(|value| value <= 0)
        {
            return Err("上下文窗口和输入用量参考值须为正整数");
        }
        let non_generation = matches!(self.kind.as_str(), "embeddings" | "rerank");
        if self.output_usage_reference.is_some_and(|value| {
            if non_generation { value != 0 } else { value <= 0 }
        }) {
            return Err("向量和重排的输出参考值须为 0，其他类型须为正整数");
        }
        if self.metrics.len() > 32 {
            return Err("单个模型最多可维护 32 个扩展指标");
        }
        for (key, metric) in &self.metrics {
            if key.is_empty()
                || key.len() > 64
                || !key.bytes().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'_')
            {
                return Err("指标名称须由小写字母、数字或下划线组成，最长 64 个字符");
            }
            if metric.value <= 0
                || !matches!(
                    metric.unit.as_str(),
                    "token" | "dimension" | "document" | "second" | "millisecond"
                        | "character" | "byte" | "image" | "video" | "pixel"
                        | "frame" | "frame_per_second" | "count"
                )
            {
                return Err("扩展指标须使用有效单位和正整数");
            }
        }
        let source_url = self.source_url.as_deref().map(str::trim).filter(|url| !url.is_empty());
        if let Some(raw) = source_url {
            let url = url::Url::parse(raw).map_err(|_| "来源须为有效的 HTTP(S) 地址")?;
            if raw.len() > 2048
                || !matches!(url.scheme(), "http" | "https")
                || !url.has_host()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err("来源须为有效的 HTTP(S) 地址且不含登录凭据");
            }
        }
        Ok(Specification {
            model_id: self.model_id,
            provider_id: self.provider_id,
            source_url: source_url.map(str::to_owned),
            public: PublicSpecification {
                kind: self.kind,
                context_window: self.context_window,
                input_usage_reference: self.input_usage_reference,
                output_usage_reference: self.output_usage_reference,
                metrics: self.metrics,
                origin: "maintained".into(),
            },
        })
    }
}

pub(crate) fn validate_model_id(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 512
        || value.trim() != value
        || value.chars().any(char::is_control)
        || db::model_routes::is_auto(value)
    {
        return Err("请填写有效的精确上游模型 ID");
    }
    Ok(())
}

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Specification> {
    let details: String = row.get(3)?;
    let public = serde_json::from_str(&details).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(Specification {
        model_id: row.get(0)?,
        provider_id: row.get(1)?,
        source_url: row.get(2)?,
        public,
    })
}

pub(crate) fn list(conn: &Connection, provider_id: Option<i64>) -> db::Result<Vec<Specification>> {
    let mut statement = conn.prepare(
        "SELECT model_id, provider_id, source_url, specification
         FROM model_specifications WHERE ?1 IS NULL OR provider_id = ?1
         ORDER BY model_id, provider_id",
    )?;
    let rows = statement
        .query_map([provider_id], from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(crate) fn save(conn: &Connection, row: &Specification) -> db::Result<()> {
    let details = serde_json::to_string(&row.public)
        .map_err(|error| db::DbError::Other(error.to_string()))?;
    conn.execute(
        "INSERT INTO model_specifications (model_id, provider_id, source_url, specification)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT DO UPDATE SET source_url = excluded.source_url, specification = excluded.specification",
        params![row.model_id, row.provider_id, row.source_url, details],
    )?;
    Ok(())
}

pub(crate) fn delete(conn: &Connection, id: &str, provider_id: Option<i64>) -> db::Result<bool> {
    Ok(conn.execute(
        "DELETE FROM model_specifications WHERE model_id = ?1 AND provider_id IS ?2",
        params![id, provider_id],
    )? != 0)
}

fn maintained(
    conn: &Connection,
    id: &str,
    provider_id: Option<i64>,
) -> db::Result<Option<Specification>> {
    let row = if let Some(provider) = provider_id {
        conn.prepare_cached(
            "SELECT model_id, provider_id, source_url, specification
             FROM model_specifications WHERE model_id = ?1 AND provider_id = ?2",
        )?.query_row(params![id, provider], from_row).optional()?
    } else {
        conn.prepare_cached(
            "SELECT model_id, provider_id, source_url, specification
             FROM model_specifications WHERE model_id = ?1 AND provider_id IS NULL",
        )?.query_row([id], from_row).optional()?
    };
    Ok(row)
}

fn relay_key(raw: &str) -> Option<&'static str> {
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match (url.host_str(), url.path().trim_end_matches('/')) {
        (Some("opencode.ai"), "/zen/go/v1"
            | "/inference/go/openai/v1"
            | "/inference/go/anthropic/v1"
            | "/inference/go/google/v1beta") => Some("opencode_go"),
        (Some("opencode.ai"), "/zen/v1"
            | "/inference/openai/v1"
            | "/inference/anthropic/v1"
            | "/inference/google/v1beta") => Some("opencode_pay"),
        (Some("api.commandcode.ai"), "/provider/v1") => Some("command_code"),
        _ => None,
    }
}

fn relay_default(
    conn: &Connection,
    id: &str,
    base_url: Option<&str>,
) -> db::Result<Option<Specification>> {
    let Some(relay) = base_url.and_then(relay_key) else {
        return Ok(None);
    };
    Ok(conn.query_row(
        "SELECT model_id, NULL, source_url, specification
         FROM model_specification_relay_defaults WHERE relay = ?1 AND model_id = ?2",
        params![relay, id],
        from_row,
    ).optional()?)
}

pub(crate) fn resolve(
    conn: &Connection,
    id: &str,
    provider_id: Option<i64>,
    base_url: Option<&str>,
) -> db::Result<Option<Specification>> {
    let canonical: Option<String> = conn.prepare_cached(
        "SELECT model_id FROM model_specification_aliases WHERE alias_id = ?1",
    )?.query_row([id], |row| row.get(0)).optional()?;
    let ids = [Some(id), canonical.as_deref()];
    // Scoped exceptions affect only the selected upstream ID. Aliases share defaults.
    if provider_id.is_some() {
        if let Some(row) = maintained(conn, id, provider_id)? {
            return Ok(Some(row));
        }
    }
    // A relay exception belongs to its exact requested ID, including version/speed variants.
    if let Some(row) = relay_default(conn, id, base_url)? {
        return Ok(Some(row));
    }
    for candidate in ids.into_iter().flatten() {
        if let Some(mut row) = maintained(conn, candidate, None)? {
            row.model_id = id.to_owned();
            return Ok(Some(row));
        }
    }
    Ok(None)
}

/// Explicit registry associations may bridge a source's upstream ID to a canonical ID.
pub(crate) fn for_mapping(
    conn: &Connection,
    channel: &db::channels::ChannelRow,
    mapping: &db::channels::ModelMapping,
) -> db::Result<Option<PublicSpecification>> {
    let id = model_registry::upstream(mapping);
    let base_url = Some(channel.base_url.as_str());
    if let Some(row) = resolve(conn, id, channel.provider_id, base_url)? {
        return Ok(Some(row.public));
    }
    let Some(canonical) = model_registry::associated(conn, channel.id, mapping)? else {
        return Ok(None);
    };
    // A registry association makes the official default discoverable for a
    // renamed model. Scoped exceptions still apply only to the actual upstream ID.
    Ok(resolve(conn, &canonical, None, None)?.map(|row| row.public))
}

impl PublicSpecification {
    /// Client metadata must apply to every eligible source behind its logical ID.
    pub fn retain_agreed(&mut self, other: Self) {
        if self.context_window != other.context_window {
            self.context_window = None;
        }
        if self.input_usage_reference != other.input_usage_reference {
            self.input_usage_reference = None;
        }
        if self.output_usage_reference != other.output_usage_reference {
            self.output_usage_reference = None;
        }
        if self.kind != other.kind {
            self.kind = "unknown".into();
        }
        if self.origin != other.origin {
            self.origin = "multiple_sources".into();
        }
        self.metrics.retain(|key, metric| other.metrics.get(key) == Some(metric));
    }

    pub fn has_metadata(&self) -> bool {
        self.kind != "unknown"
            || self.context_window.is_some()
            || self.input_usage_reference.is_some()
            || self.output_usage_reference.is_some()
            || !self.metrics.is_empty()
    }
}
