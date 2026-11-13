// Included by billing.rs. Lifecycle overlays retain immutable prices and ledger snapshots.
#[derive(Clone,Copy,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct PriceLifecycle { enabled:bool, deleted:bool }
fn pricing_table(kind:&str)->Result<&'static str> {match kind {"token"=>Ok("billing_prices"),"unit"=>Ok("billing_unit_prices"),"request"=>Ok("billing_request_prices"),_=>Err(bad("invalid pricing kind"))}}
fn lifecycle_key(kind:&str,id:i64)->Result<String> {pricing_table(kind)?;if id<=0{return Err(bad("unknown_price: price not found"));}Ok(format!("pricing_lifecycle_v1:{kind}:{id}"))}
fn price_lifecycle(c:&Connection,kind:&str,id:i64)->Result<PriceLifecycle> {
 let key=lifecycle_key(kind,id)?;
 match super::settings::get(c,&key)? {None=>Ok(PriceLifecycle{enabled:true,deleted:false}),Some(raw)=>serde_json::from_str(&raw).map_err(|_|bad("invalid stored price lifecycle"))}
}
fn price_version_enabled(c:&Connection,kind:&str,id:i64)->Result<bool> {let s=price_lifecycle(c,kind,id)?;Ok(s.enabled&&!s.deleted)}
fn require_price_version_enabled(c:&Connection,kind:&str,id:i64)->Result<()> {
 if !price_version_enabled(c,kind,id)? {return Err(bad("config_required: price is disabled or deleted; configure active pricing before a new reservation"));}Ok(())
}
fn price_identity(c:&Connection,kind:&str,id:i64)->Result<(i64,String,String)> {
 let table=pricing_table(kind)?;
 c.query_row(&format!("SELECT channel_id,model,currency FROM {table} WHERE id=?1"),[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.ok_or_else(||bad("unknown_price: price not found"))
}
fn latest_price_id(c:&Connection,kind:&str,identity:&(i64,String,String))->Result<i64> {
 let table=pricing_table(kind)?;
 Ok(c.query_row(&format!("SELECT id FROM {table} WHERE channel_id=?1 AND model=?2 AND currency=?3 ORDER BY id DESC LIMIT 1"),params![identity.0,identity.1,identity.2],|r|r.get(0))?)
}
/// Validate the price selected for replacement. Call while holding the control-plane DB lock.
pub fn require_current_price(c:&Connection,kind:&str,id:i64,channel:i64,model:&str,currency:&str)->Result<()> {
 let identity=price_identity(c,kind,id)?;
 if identity.0!=channel||identity.1!=tokengo_shared::normalize_model_name(model)||identity.2!=currency {return Err(bad("price identity is fixed; create a price for another model, source or currency"));}
 if currency!="CNY" {return Err(bad("historical currency prices are read-only"));}
 if latest_price_id(c,kind,&identity)?!=id||price_lifecycle(c,kind,id)?.deleted {return Err(bad("price_conflict: price was replaced or deleted; refresh before editing"));}
 Ok(())
}
pub fn enrich_price(c:&Connection,kind:&str,mut value:serde_json::Value)->Result<serde_json::Value> {
 let id=value["id"].as_i64().ok_or_else(||bad("invalid price row"))?;let identity=price_identity(c,kind,id)?;let state=price_lifecycle(c,kind,id)?;let latest=latest_price_id(c,kind,&identity)?==id;
 value["enabled"]=state.enabled.into();value["deleted"]=state.deleted.into();value["is_latest"]=latest.into();value["effective"]=(latest&&state.enabled&&!state.deleted&&identity.2=="CNY").into();Ok(value)
}
pub fn price_lifecycle_rows(c:&Connection,kind:&str,include_deleted:bool)->Result<Vec<serde_json::Value>> {
 let rows=match kind {
  "token"=>list_prices(c)?.into_iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>,_>>(),
  "unit"=>list_unit_prices(c)?.into_iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>,_>>(),
  "request"=>list_request_prices(c)?.into_iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>,_>>(),
  _=>return Err(bad("invalid pricing kind")),
 }.map_err(|_|bad("price serialization failed"))?;
 rows.into_iter().map(|row|enrich_price(c,kind,row)).filter_map(|row|match row {Ok(row) if !include_deleted&&row["deleted"]==true=>None,other=>Some(other)}).collect()
}
pub fn change_price_state(c:&Connection,kind:&str,id:i64,enabled:Option<bool>,actor:&str)->Result<serde_json::Value> {
 let c=tx(c)?;let identity=price_identity(&c,kind,id)?;let old=price_lifecycle(&c,kind,id)?;
 let state=if let Some(enabled)=enabled {
  require_current_price(&c,kind,id,identity.0,&identity.1,&identity.2)?;
  PriceLifecycle{enabled,deleted:false}
 } else {PriceLifecycle{enabled:false,deleted:true}};
 super::settings::set(&c,&lifecycle_key(kind,id)?,&serde_json::to_string(&state).map_err(|_|bad("price lifecycle serialization failed"))?)?;
 audit(&c,if enabled.is_some(){"price_state_changed"}else{"price_archived"},actor,&format!("{kind}:{id}"),&serde_json::json!({"before":{"enabled":old.enabled,"deleted":old.deleted},"after":{"enabled":state.enabled,"deleted":state.deleted}}).to_string())?;
 let raw=match kind {"token"=>c.query_row("SELECT id,channel_id,model,input_micro_per_million,output_micro_per_million,max_input_tokens,max_output_tokens,currency FROM billing_prices WHERE id=?1",[id],price_row).map(serde_json::to_value)?,"unit"=>c.query_row(&format!("{UNIT_PRICE_SELECT} WHERE id=?1"),[id],unit_price_row).map(serde_json::to_value)?,"request"=>c.query_row("SELECT id,channel_id,model,micro_per_call,currency FROM billing_request_prices WHERE id=?1",[id],request_price_row).map(serde_json::to_value)?,_=>unreachable!()}.map_err(|_|bad("price serialization failed"))?;
 let result=enrich_price(&c,kind,raw)?;c.commit()?;Ok(result)
}
