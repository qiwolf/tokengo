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

/// Presentation metadata never participates in model identity or authorization.
fn display_key(name: &str) -> Result<String> {
    let canonical = normalize_model_name(name);
    if canonical.is_empty() { return Err(DbError::Other("Model name cannot be empty".into())); }
    Ok(format!("model_display_name_v1:{canonical}"))
}
pub fn presentation_name(raw: &str) -> std::result::Result<String, &'static str> {
    if raw.chars().any(char::is_control) || raw.trim().chars().count() > 128 {
        return Err("显示名称最多128个字符，不能包含控制字符");
    }
    Ok(raw.trim().into())
}
pub fn display_name(c: &Connection, name: &str) -> Result<String> {
    Ok(super::settings::get(c, &display_key(name)?)?.unwrap_or_default())
}
pub fn set_display_name(c: &Connection, name: &str, value: &str) -> Result<()> {
    let value = presentation_name(value).map_err(|e| DbError::Other(e.into()))?;
    let key = display_key(name)?;
    if value.is_empty() { super::settings::delete(c, &key) } else { super::settings::set(c, &key, &value) }
}
/// Catalog grouping is deliberately independent of executable model kind.
fn purpose_key(name: &str) -> Result<String> {
    let canonical=normalize_model_name(name);
    if canonical.is_empty() { return Err(DbError::Other("Model name cannot be empty".into())); }
    Ok(format!("model_catalog_purpose_v1:{canonical}"))
}
pub fn presentation_purpose(raw:&str)->std::result::Result<&str,&'static str> {
    match raw {
        ""|"chat"|"embeddings"|"rerank"|"image"|"video"|"audio"|"tools"=>Ok(raw),
        _=>Err("展示用途必须为chat、embeddings、rerank、image、video、audio、tools或空字符串"),
    }
}
pub fn catalog_purpose(c:&Connection,name:&str)->Result<String> {
    Ok(super::settings::get(c,&purpose_key(name)?)?.unwrap_or_default())
}
pub fn catalog_purpose_label(purpose:&str)->&'static str {
    match purpose {
        ""=>"",
        "tools"=>"工具",
        _=>tokengo_shared::ModelKind::parse(purpose).label(),
    }
}
pub fn set_catalog_purpose(c:&Connection,name:&str,value:&str)->Result<()> {
    let value=presentation_purpose(value).map_err(|e|DbError::Other(e.into()))?;
    let key=purpose_key(name)?;
    if value.is_empty() {super::settings::delete(c,&key)}else{super::settings::set(c,&key,value)}
}
#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn purpose_normalizes_model_key_without_touching_executable_kind() {
        let c=super::super::open_in_memory().unwrap();
        super::super::kinds::set(&c,"BGE-M3",tokengo_shared::ModelKind::Embeddings).unwrap();
        let before=super::super::kinds::all(&c).unwrap();
        for purpose in ["chat","embeddings","rerank","image","video","audio","tools"] {
            set_catalog_purpose(&c," BGE-M3 ",purpose).unwrap();
            assert_eq!(catalog_purpose(&c,"bge-m3").unwrap(),purpose);
            assert!(!catalog_purpose_label(purpose).is_empty());
        }
        assert!(set_catalog_purpose(&c,"bge-m3","invalid").is_err());
        assert_eq!(catalog_purpose(&c,"bge-m3").unwrap(),"tools");
        set_catalog_purpose(&c,"BGE-M3","").unwrap();
        assert_eq!(catalog_purpose(&c,"bge-m3").unwrap(),"");
        assert_eq!(super::super::kinds::all(&c).unwrap(),before);
    }
    #[test]
    fn display_name_uses_canonical_identity_and_clear_does_not_touch_access() {
        let c=super::super::open_in_memory().unwrap();
        set_internal_only(&c," BGE-M3 ",true).unwrap();
        set_display_name(&c," BGE-M3 ","  云端 · 向量 · BGE-M3  ").unwrap();
        assert_eq!(display_name(&c,"bge-m3").unwrap(),"云端 · 向量 · BGE-M3");
        assert!(internal_only(&c,"bge-m3").unwrap());
        for invalid in ["x\ny".to_string(), "x".repeat(129)] {
            assert!(set_display_name(&c,"bge-m3",&invalid).is_err());
            assert_eq!(display_name(&c,"bge-m3").unwrap(),"云端 · 向量 · BGE-M3");
        }
        set_display_name(&c,"bge-m3","   ").unwrap();
        assert_eq!(display_name(&c,"bge-m3").unwrap(),"");
        assert!(internal_only(&c,"bge-m3").unwrap());
    }
}
