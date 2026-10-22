//! Model publication is independent of model availability and provider routing.
use super::{Connection, DbError, Result};
use tokengo_shared::normalize_model_name;
fn key(name: &str) -> Result<String> {
    let name = normalize_model_name(name);
    if name.is_empty() {
        return Err(DbError::Other("Model name cannot be empty".into()));
    }
    Ok(format!("internal_model_access_v1:{name}"))
}
pub fn internal_only(c: &Connection, name: &str) -> Result<bool> {
    Ok(super::settings::get(c, &key(name)?)?.is_some_and(|v| v == "true"))
}
pub fn set_internal_only(c: &Connection, name: &str, value: bool) -> Result<()> {
    super::settings::set(c, &key(name)?, if value { "true" } else { "false" })
}
