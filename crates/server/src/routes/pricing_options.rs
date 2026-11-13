//! Model-bound pricing metadata. Reads configured source limits; never edits model configuration.
use crate::db::{self, channels};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use tokengo_shared::{same_model_name, ChannelId, ModelKind};

#[derive(Debug, Clone, Serialize)]
pub(super) struct PricingOption {
    pub channel_id:i64,
    pub model:String,
    pub upstream:String,
    pub task_type:String,
    pub task_source:String,
    pub pricing_kind:String,
    pub allowed_pricing_kinds:Vec<&'static str>,
    pub recommended_unit:Option<&'static str>,
    pub allowed_units:Vec<&'static str>,
    pub max_input_tokens:Option<i64>,
    pub max_output_tokens:Option<i64>,
    pub max_audio_seconds:Option<i64>,
    pub input_bound_source:Option<&'static str>,
    pub output_bound_source:Option<&'static str>,
    pub automatic_settlement:bool,
}
fn subtype(raw:&str)->Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "asr"|"transcription"|"audio_transcription"|"speech_recognition"=>Some("asr"),
        "tts"|"speech"|"audio_speech"|"speech_synthesis"=>Some("tts"),
        _=>None,
    }
}
// Audio subtypes are not represented separately by ModelKind. Explicit identifier
// markers supply a UI recommendation only, never a provider price or capability claim.
fn audio_identifier(raw:&str)->Option<&'static str> {
    let lower=raw.to_ascii_lowercase();
    let segments=lower.split(|c:char|!c.is_ascii_alphanumeric()).collect::<Vec<_>>();
    if segments.contains(&"asr")||segments.contains(&"whisper")||lower.contains("sensevoice") {Some("asr")}
    else if segments.contains(&"tts") {Some("tts")} else {None}
}
fn task(c:&Connection,channel:ChannelId,mapping:&channels::ModelMapping)->db::Result<(String,String)> {
    let upstream=crate::model_registry::upstream(mapping);
    let declared=db::kinds::all(c)?.into_iter().find(|k|same_model_name(&k.name,&mapping.model)&&k.kind.is_set());
    let purpose=db::model_access::catalog_purpose(c,&mapping.model)?;
    let entries=db::catalog::routing_entries(c,channel,&[upstream])?;
    let claims=entries.iter().filter(|e|same_model_name(&e.model,upstream)).filter_map(|e|e.kind.as_deref()).collect::<Vec<_>>();
    // Configured presentation purpose is the administrator's current model type.
    // Older executable kinds may retain Chat for transport compatibility; that
    // must not turn an explicitly configured ASR/audio price back into tokens.
    // This precedence is billing-only and never changes the routing kind.
    if purpose=="tools" {return Ok(("tools".into(),"model_purpose".into()));}
    let mut kind=if purpose.is_empty(){declared.as_ref().map(|k|k.kind).unwrap_or(ModelKind::Unknown)}else{ModelKind::parse(&purpose)};
    let mut origin=if !purpose.is_empty(){"model_purpose"}else if declared.is_some(){"model_kind"}else{"unknown"};
    if !kind.is_set() {
        let mut kinds=Vec::new();for raw in &claims {let k=if subtype(raw).is_some(){ModelKind::Audio}else{ModelKind::parse(raw)};if k.is_set()&&!kinds.contains(&k){kinds.push(k);}}
        if kinds.len()==1{kind=kinds[0];origin="upstream_catalog";}
    }
    if kind==ModelKind::Audio {
        let mut types=claims.iter().filter_map(|v|subtype(v)).collect::<Vec<_>>();types.sort_unstable();types.dedup();
        if types.len()==1 {return Ok((types[0].into(),"upstream_catalog".into()));}
        if types.is_empty() {if let Some(t)=audio_identifier(upstream).or_else(||audio_identifier(&mapping.model)) {return Ok((t.into(),"model_identifier".into()));}}
    }
    Ok((if kind.is_set(){kind.as_str()}else{"unknown"}.into(),origin.into()))
}
pub(super) fn for_mapping(c:&Connection,channel:ChannelId,mapping:&channels::ModelMapping)->db::Result<PricingOption> {
    let (task_type,task_source)=task(c,channel,mapping)?;
    let (pricing_kind,allowed_pricing_kinds,recommended_unit,allowed_units,automatic_settlement)=match task_type.as_str(){
        "chat"|"embeddings"|"rerank"=>("token",vec!["token","request"],None,vec![],true),
        "asr"=>("unit",vec!["unit"],Some("second"),vec!["second","minute","hour"],true),
        "tts"=>("unit",vec!["unit"],None,vec!["character","byte"],false),
        "image"=>("unit",vec!["unit"],Some("image"),vec!["image"],false),
        "video"=>("unit",vec!["unit"],None,vec!["video","second","minute","hour"],false),
        "audio"=>("unit",vec!["unit"],None,vec!["second","minute","hour","character","byte"],false),
        "tools"=>("request",vec!["request"],None,vec![],true),
        _=>("unknown",vec!["token","unit","request"],None,vec!["second","minute","hour","character","byte","image","video"],false),
    };
    // OAuth binding is the effective source bound for OAuth traffic. Source-wide
    // protocol limits must not silently override that independent account config.
    let oauth:Option<(Option<i64>,Option<i64>)>=c.query_row("SELECT max_input_tokens,max_output_tokens FROM oauth_channel_bindings WHERE channel_id=?1",[channel.0],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (mut input,mut output,mut input_source,mut output_source)=if let Some((a,b))=oauth {
        (a.filter(|v|*v>0),b.filter(|v|*v>0),Some("oauth_binding"),Some("oauth_binding"))
    }else{
        let caps=crate::protocol_config::load(c,channel.0)?;
        (caps.max_input_tokens,caps.max_output_tokens,Some("source_protocols"),Some("source_protocols"))
    };
    // A model-specific context restriction may be narrower than a shared source
    // allowance; never let a price snapshot accidentally widen it.
    let manual=crate::model_registry::context_override(c,channel,mapping)?.filter(|v|*v>0);
    let entries=db::catalog::routing_entries(c,channel,&[crate::model_registry::upstream(mapping)])?;
    let mapped=manual.or_else(||crate::capability_detector::mapped_context(&entries,mapping));
    if let Some(value)=mapped {
        if input.is_none_or(|old|value<old) {
            input=Some(value);input_source=Some(if manual.is_some(){"model_context_override"}else{"upstream_catalog"});
        }
    }
    if matches!(task_type.as_str(),"embeddings"|"rerank") {output=Some(0);output_source=Some("non_generation_model");}
    if input.is_none(){input_source=None;}if output.is_none(){output_source=None;}
    Ok(PricingOption{channel_id:channel.0,model:tokengo_shared::normalize_model_name(&mapping.model),upstream:crate::model_registry::upstream(mapping).into(),task_type,task_source,pricing_kind:pricing_kind.into(),allowed_pricing_kinds,recommended_unit,allowed_units,max_input_tokens:input,max_output_tokens:output,max_audio_seconds:None,input_bound_source:input_source,output_bound_source:output_source,automatic_settlement})
}
pub(super) fn list(c:&Connection)->db::Result<Vec<PricingOption>> {
    let mut out=Vec::new();
    for channel in channels::list(c)? {
        let linked:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM source_credentials WHERE source_id=?1)",[channel.id.0],|r|r.get(0))?;
        let credential=db::provider_pool::source_credential(c,channel.id.0)?;
        if !channel.enabled||!channel.provider_enabled||(linked&&credential.as_ref().is_none_or(|v|!v.enabled)){continue;}
        for mapping in channel.models.iter().filter(|m|m.enabled) {
            if db::model_routes::is_auto(&mapping.model)||!db::model_routes::enabled(c,&mapping.model)? {continue;}
            out.push(for_mapping(c,channel.id,mapping)?);
        }
    }
    Ok(out)
}
/// Legacy API callers may price an as-yet-unmapped name. Keep that compatibility,
/// but known model/source pairs always use their current configuration and type.
pub(super) fn for_pair(c:&Connection,channel:i64,model:&str)->db::Result<Option<PricingOption>> {
    if let Some(row)=channels::get(c,ChannelId(channel))? {
        let linked:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM source_credentials WHERE source_id=?1)",[channel],|r|r.get(0))?;
        let credential=db::provider_pool::source_credential(c,channel)?;
        if !row.enabled||!row.provider_enabled||(linked&&credential.as_ref().is_none_or(|v|!v.enabled)) {
            return Err(db::DbError::Other("来源渠道或上游凭据已失效，请刷新模型配置。".into()));
        }
        if let Some(mapping)=row.models.iter().find(|m|same_model_name(&m.model,model)){
            if !mapping.enabled||!db::model_routes::enabled(c,model)? {return Err(db::DbError::Other("模型已停用，请刷新模型配置。".into()));}
            return for_mapping(c,row.id,mapping).map(Some);
        }
    }
    Ok(None)
}
pub(super) fn require_kind(option:Option<&PricingOption>,kind:&str)->db::Result<()> {
    if option.is_some_and(|v|!v.allowed_pricing_kinds.contains(&kind)){return Err(db::DbError::Other("所选模型类型不适用此计价方式，请按模型类型重新选择。".into()));}Ok(())
}
