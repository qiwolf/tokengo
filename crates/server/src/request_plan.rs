//! Atomic final request preparation. Metering and validation run on the exact
//! outgoing copy; failures never materialize defaults in the caller's request.
use serde_json::Value;

pub struct RequestPlan<T> {
    body: Value,
    result: T,
    fields: Vec<FieldDisposition>,
}

impl<T> RequestPlan<T> {
    pub fn prepare<E>(original: &Value, prepare: impl FnOnce(&mut Value) -> Result<T, E>) -> Result<Self, E> {
        let mut body = original.clone();
        let result = prepare(&mut body)?;
        Ok(Self { body, result, fields:Vec::new() })
    }

    pub fn into_parts(self) -> (Value, T) { (self.body, self.result) }
    pub fn record_budget_validation(&mut self,original:&Value,ctx:&crate::parameter_contracts::Context<'_>){
        self.fields.extend(control_records(original,&self.body,ctx,"final_output_budget_validation",true)
            .into_iter().filter(|record|matches!(record.field_path,"/max_tokens"|"/max_completion_tokens"|"/max_output_tokens"|"/thinking_budget"|"/thinking"|"/reasoning")));
    }
    pub fn field_dispositions(&self)-> &[FieldDisposition] { &self.fields }
    /// Bounded machine-readable execution report. This is evidence of the
    /// current preparation, never a global model-support declaration.
    pub fn disposition_report(&self)->Value { serde_json::json!({"revision":1,"fields":self.fields}) }
}


/// No values, source addresses, headers or caller-selected field names enter
/// this record. Paths and rule identifiers come from the executing stages.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct FieldDisposition {
    pub rule_id: &'static str,
    pub stage: &'static str,
    pub field_path: &'static str,
    pub disposition: &'static str,
    pub effect: &'static str,
    pub semantic_note: Option<&'static str>,
}

#[derive(Debug)]
pub struct PreparationError {
    pub message:String,
    fields:Vec<FieldDisposition>,
}
impl PreparationError {
    fn at(message:String,rule_id:&'static str,stage:&'static str,field_path:&'static str,disposition:&'static str)->Self {
        Self{message,fields:vec![FieldDisposition{rule_id,stage,field_path,disposition,effect:"rejected_before_send_and_reservation",semantic_note:None}]}
    }
    /// Excludes the original error string, which remains in the existing HTTP
    /// error path. Only fixed schema paths/classifications enter this report.
    pub fn disposition_report(&self)->Value {serde_json::json!({"revision":1,"fields":self.fields})}
}

const CONTROLS: &[(&str,&str)] = &[
    ("thinking","/thinking"),("reasoning_effort","/reasoning_effort"),
    ("reasoning","/reasoning"),("output_config","/output_config"),
    ("enable_thinking","/enable_thinking"),("thinking_budget","/thinking_budget"),
    ("max_tokens","/max_tokens"),("max_completion_tokens","/max_completion_tokens"),
    ("max_output_tokens","/max_output_tokens"),("tool_choice","/tool_choice"),
    ("parallel_tool_calls","/parallel_tool_calls"),("tools","/tools"),
    ("response_format","/response_format"),("stream","/stream"),
    ("tool_stream","/tool_stream"),("temperature","/temperature"),("top_p","/top_p"),
    ("top_k","/top_k"),("store","/store"),("previous_response_id","/previous_response_id"),
    ("chat_template_kwargs","/chat_template_kwargs"),
];

fn source_rule(ctx:&crate::parameter_contracts::Context<'_>)-> &'static str {
    match crate::parameter_contracts::provider(ctx) {
        "deepseek"=>"G07","dashscope"=>"G08","moonshot"=>"G09","zai"=>"G11/G13",
        "anthropic"=>"G14","minimax"=>"G15","siliconflow"=>"exact_hosted_source_contract",
        _=>"source_parameter_contract",
    }
}
// A passed whole-request validator is not evidence about every preserved field.
// Only these exact field validators establish local native acceptance; all other
// fields remain unknown here, even on a known model and successful request.
fn validated_field(ctx:&crate::parameter_contracts::Context<'_>,path:&str)->bool {
    !ctx.is_oauth && ctx.api_version.is_none()
        && crate::parameter_contracts::provider(ctx)=="dashscope"
        && ctx.protocol==crate::protocol::Protocol::Responses
        && path=="/max_output_tokens"
        && crate::parameter_contracts_china::documented_responses_cap(ctx)
}
fn control_records(before:&Value,after:&Value,ctx:&crate::parameter_contracts::Context<'_>,stage:&'static str,validated:bool)->Vec<FieldDisposition> {
    CONTROLS.iter().filter(|(key,_)|before.get(*key).is_some()||after.get(*key).is_some()).map(|(key,path)| {
        let changed=before.get(*key)!=after.get(*key);
        let provider_adapter=matches!(stage,"provider_parameters_before_defaults"|"provider_parameters_after_defaults");
        // The executing provider adapter implements only explicit off rewrites.
        // Diff selects the affected fixed path; it never determines semantics.
        let off_rewrite=changed && provider_adapter && matches!(*path,"/thinking"|"/reasoning_effort"|"/reasoning"|"/output_config"|"/enable_thinking");
        FieldDisposition{rule_id:source_rule(ctx),stage,field_path:path,
            disposition:if off_rewrite {"exact_transform"}
                else if validated&&validated_field(ctx,path) {"supported_native"}else{"unknown"},
            effect:if changed&&stage=="mapping_defaults" {"operator_mapping_default"}
                else if changed {"applied"}else if validated {"validated_without_change"}else{"unchanged"},
            semantic_note:if changed&&stage=="mapping_defaults" {Some("Operator-selected default inserted; final contract validation remains a separate stage")}
                else if off_rewrite {Some("Executing exact source adapter maps explicit none to the documented native off control")}
                else {None}}
    }).collect()
}

impl RequestPlan<()> {
    pub fn prepare_conversion(original:&Value,ctx:&crate::parameter_contracts::Context<'_>,from:crate::protocol::Protocol,
        convert:impl FnOnce(&Value)->Result<Value,String>)->Result<Self,String> {
        Self::prepare_conversion_detailed(original,ctx,from,convert).map_err(|error|error.message)
    }
    pub fn prepare_conversion_detailed(original:&Value,ctx:&crate::parameter_contracts::Context<'_>,from:crate::protocol::Protocol,
        convert:impl FnOnce(&Value)->Result<Value,String>)->Result<Self,PreparationError> {
        let body=convert(original).map_err(|message| {
            if from!=ctx.protocol && original.get("background")==Some(&Value::Bool(true)) {
                PreparationError::at(message,"stateful_executor","protocol_conversion","/background","executor_gated")
            }else{PreparationError::at(message,"protocol_conversion","protocol_conversion","/","unsupported")}
        })?;
        let changed_protocol=from!=ctx.protocol;
        let mut fields=control_records(original,&body,ctx,"protocol_conversion",false);
        if original.get("tools").is_some() {
            fields.push(FieldDisposition{rule_id:"G04",stage:"protocol_conversion",field_path:"/tools/*/strict",
                disposition:if changed_protocol {"exact_transform"}else{"unknown"},effect:if changed_protocol {"converted_with_schema_guard"}else{"native_passthrough"},
                semantic_note:Some("Omitted/true/false strict semantics resolved by the executing converter; native passthrough does not establish upstream enforcement")});
        }
        for note in crate::protocol::normalization_notes(from,ctx.protocol,original) {
            fields.push(FieldDisposition{rule_id:"protocol_normalization",stage:"protocol_conversion",field_path:"/",
                disposition:"documented_semantic_fold",effect:"converted_with_documented_normalization",semantic_note:Some(note)});
        }
        if crate::protocol::minimax_codec::policy(ctx,from).is_some()
            && (original.get("instructions").is_some() || original["input"].as_array().is_some_and(|items|
                items.iter().any(|item|matches!(item["role"].as_str(),Some("system"|"developer"))))) {
            fields.push(FieldDisposition{rule_id:"G16",stage:"protocol_conversion",field_path:"/input/*/role",
                disposition:"documented_semantic_fold",effect:"applied",
                semantic_note:Some("MiniMax source codec folds ordered control text into one system priority; independent developer/system priority is not retained")});
        }
        fields.push(FieldDisposition{rule_id:"protocol_transport",stage:"protocol_conversion",field_path:"/",
            disposition:"unknown",effect:if changed_protocol {"converted"}else{"native_passthrough"},semantic_note:None});
        Ok(Self{body,result:(),fields})
    }
}

impl RequestPlan<SourcePreparation> {
    /// Run the actual source pipeline atomically. Defaults are applied exactly
    /// once between the two existing provider-adaptation passes; validation is
    /// performed on this same copy before the gateway meters/reserves/sends it.
    pub fn prepare_source(
        original:&Value,ctx:&crate::parameter_contracts::Context<'_>,client:crate::protocol::Protocol,
        defaults:impl FnOnce(&mut Value)->Result<bool,String>,
    )->Result<Self,String> {
        Self::prepare_source_detailed(original,ctx,client,defaults).map_err(|error|error.message)
    }
    pub fn prepare_source_detailed(
        original:&Value,ctx:&crate::parameter_contracts::Context<'_>,client:crate::protocol::Protocol,
        defaults:impl FnOnce(&mut Value)->Result<bool,String>,
    )->Result<Self,PreparationError> {
        let mut body=original.clone();
        let mut fields=Vec::new();
        let role_changed=crate::message_roles::adapt(ctx,&mut body)
            .map_err(|message|PreparationError::at(message,"G05","source_roles","/messages/*/role","unsupported"))?;
        if original.get("messages").is_some() {
            fields.push(FieldDisposition{rule_id:"G05",stage:"source_roles",field_path:"/messages/*/role",
                disposition:if role_changed {"documented_semantic_fold"}else{"unknown"},
                effect:if role_changed {"applied"}else{"unchanged"},
                semantic_note:role_changed.then_some("Leading textual instruction roles fold to one provider priority; original distinct priority cannot be represented")});
        }
        let before=body.clone();
        let first=if ctx.is_oauth {false}else{crate::parameter_contracts_china::adapt(ctx,&mut body)
            .map_err(|message|PreparationError::at(message,source_rule(ctx),"provider_parameters_before_defaults","/","unsupported"))?};
        fields.extend(control_records(&before,&body,ctx,"provider_parameters_before_defaults",false));
        let before=body.clone();
        let default_changed=defaults(&mut body)
            .map_err(|message|PreparationError::at(message,"mapping_capability","mapping_defaults","/","unsupported"))?;
        fields.extend(control_records(&before,&body,ctx,"mapping_defaults",false));
        let before=body.clone();
        let second=if ctx.is_oauth {false}else{crate::parameter_contracts_china::adapt(ctx,&mut body)
            .map_err(|message|PreparationError::at(message,source_rule(ctx),"provider_parameters_after_defaults","/","unsupported"))?};
        fields.extend(control_records(&before,&body,ctx,"provider_parameters_after_defaults",false));
        crate::parameter_contracts::validate(ctx,&body)
            .and_then(|_|crate::parameter_contracts_china::validate(ctx,&body))
            .map_err(|message|PreparationError::at(message,source_rule(ctx),"final_source_validation","/","unsupported"))?;
        crate::parameter_budgets::validate_warmup(ctx,client,&body)
            .map_err(|message|PreparationError::at(message,"native_cache_warmup_executor","final_executor_validation","/max_tokens","executor_gated"))?;
        fields.extend(control_records(&body,&body,ctx,"final_source_validation",true));
        Ok(Self{body,result:SourcePreparation{changed:role_changed||first||second||default_changed,default_changed},fields})
    }
}
#[derive(Debug)]
pub struct SourcePreparation {pub changed:bool,pub default_changed:bool}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failure_discards_all_partial_transformations() {
        let original = json!({"model":"native", "input":"original"});
        let result = RequestPlan::prepare(&original, |body| {
            body["max_output_tokens"] = json!(32);
            body["model"] = json!("changed");
            Err::<(), _>("unsupported final shape")
        });
        assert!(result.is_err());
        assert_eq!(original, json!({"model":"native", "input":"original"}));
    }

    #[test]
    fn success_returns_the_same_final_body_that_was_metered_once() {
        let original = json!({"input":"unchanged"});
        let mut calls = 0;
        let plan = RequestPlan::prepare(&original, |body| {
            calls += 1;
            body["max_output_tokens"] = json!(32);
            Ok::<_, ()>(body["max_output_tokens"].as_i64().unwrap())
        }).unwrap();
        let (body, bound) = plan.into_parts();
        assert_eq!(calls,1);
        assert_eq!(bound,32);
        assert_eq!(body,json!({"input":"unchanged","max_output_tokens":32}));
        assert_eq!(original,json!({"input":"unchanged"}));
    }
    fn context()->crate::parameter_contracts::Context<'static>{crate::parameter_contracts::Context{
        base_url:"https://api.siliconflow.cn/v1",model:"Qwen/Qwen3.8-27B",protocol:crate::protocol::Protocol::Chat,
        api_version:None,is_oauth:false,profile_override:Some("siliconflow")}}
    #[test]
    fn source_pipeline_records_actual_roles_defaults_and_validation_without_values(){
        let original=json!({"messages":[{"role":"developer","content":"private prompt do not log"},{"role":"user","content":"hello"}],"max_tokens":32});
        let plan=RequestPlan::prepare_source(&original,&context(),crate::protocol::Protocol::Chat,|body|{body["reasoning_effort"]=json!("none");Ok(true)}).unwrap();
        assert!(plan.field_dispositions().iter().any(|r|r.stage=="source_roles"&&r.disposition=="documented_semantic_fold"));
        assert!(plan.field_dispositions().iter().any(|r|r.stage=="mapping_defaults"&&r.field_path=="/reasoning_effort"&&r.effect=="operator_mapping_default"));
        assert!(plan.field_dispositions().iter().any(|r|r.stage=="final_source_validation"));
        assert!(!plan.disposition_report().to_string().contains("private prompt"));
        let(body,result)=plan.into_parts();assert!(result.changed);assert!(result.default_changed);
        assert_eq!(body["messages"][0]["role"],"system");assert_eq!(original["messages"][0]["role"],"developer");
    }
    #[test]
    fn source_pipeline_failure_after_role_change_and_defaults_keeps_original_atomic(){
        let original=json!({"messages":[{"role":"developer","content":"private"}],"max_tokens":32});
        let result=RequestPlan::prepare_source(&original,&context(),crate::protocol::Protocol::Chat,|body|{body["reasoning_effort"]=json!("high");Err("mapping rejected".into())});
        assert!(result.is_err());assert_eq!(original,json!({"messages":[{"role":"developer","content":"private"}],"max_tokens":32}));
    }

    #[test]
    fn known_model_unknown_preserved_fields_never_become_supported_native(){
        let original=json!({"messages":[{"role":"user","content":"hello"}],"max_tokens":32,
            "chat_template_kwargs":{"vendor_future":"do not echo"},"top_k":99,"previous_response_id":"private-reference"});
        let plan=RequestPlan::prepare_source(&original,&context(),crate::protocol::Protocol::Chat,|_|Ok(false)).unwrap();
        for path in ["/chat_template_kwargs","/top_k","/previous_response_id"] {
            let records:Vec<_>=plan.field_dispositions().iter().filter(|r|r.field_path==path).collect();
            assert!(!records.is_empty());assert!(records.iter().all(|r|r.disposition=="unknown"));
        }
        let report=plan.disposition_report().to_string();assert!(!report.contains("private-reference"));assert!(!report.contains("do not echo"));
        assert!(plan.field_dispositions().iter().all(|r|matches!(r.disposition,"supported_native"|"exact_transform"|"documented_semantic_fold"|"unsupported"|"unknown"|"executor_gated")));
    }

    #[test]
    fn detailed_rejection_reports_are_typed_secret_free_and_atomic(){
        let original=json!({"messages":[{"role":"developer","content":"private body"}],"max_tokens":32});
        let error=RequestPlan::prepare_source_detailed(&original,&context(),crate::protocol::Protocol::Chat,|body|{
            body["reasoning_effort"]=json!("none");Err("private error from callback".into())
        }).err().unwrap();
        let report=error.disposition_report();assert_eq!(report["fields"][0]["disposition"],"unsupported");
        assert_eq!(report["fields"][0]["stage"],"mapping_defaults");
        assert!(!report.to_string().contains("private"));assert_eq!(original["messages"][0]["role"],"developer");
        let mut ctx=context();ctx.protocol=crate::protocol::Protocol::Chat;
        let background=json!({"model":"m","input":"private","background":true});
        let error=RequestPlan::prepare_conversion_detailed(&background,&ctx,crate::protocol::Protocol::Responses,|body|
            crate::protocol::convert_request(crate::protocol::Protocol::Responses,crate::protocol::Protocol::Chat,body)).err().unwrap();
        assert_eq!(error.disposition_report()["fields"][0]["disposition"],"executor_gated");
        assert_eq!(background["input"],"private");
    }

}
