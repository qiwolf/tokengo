//! Multipart ASR transport with independently measured audio-duration billing.
use super::{client_ip, proxy::GatewayKey, ApiError};
use crate::{db::{self, channels, billing, requests}, SharedState};
use axum::{body::{Body, Bytes}, extract::{DefaultBodyLimit, Multipart, State}, http::{header, HeaderMap, StatusCode}, response::{IntoResponse, Response}, routing::post, Router};
use rusqlite::params;
use std::time::Duration;
use tokengo_shared::Channel;
const BODY_LIMIT:usize=64*1024*1024;
const RESPONSE_LIMIT:usize=16*1024*1024;
const DEADLINE:Duration=Duration::from_secs(3600);
pub fn router()->Router<SharedState>{Router::new().route("/v1/audio/transcriptions",post(transcribe).layer(DefaultBodyLimit::max(BODY_LIMIT)))}
struct Part { name:String, filename:Option<String>, content_type:Option<String>, bytes:Bytes }
async fn upload(mut multipart:Multipart)->Result<(String,Vec<Part>,Option<i64>),ApiError> {
    let mut parts=Vec::new();let mut model=None;let mut file=None;let mut total=0usize;
    while let Some(field)=multipart.next_field().await.map_err(|_|ApiError::bad_request("Malformed audio multipart upload"))? {
        if parts.len()>=64{return Err(ApiError::bad_request("Too many audio fields"));}
        let name=field.name().filter(|v|!v.is_empty()).ok_or_else(||ApiError::bad_request("Unnamed audio field"))?.to_owned();
        if !name.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-'|b'['|b']')) {return Err(ApiError::bad_request("Invalid audio field name"));}
        let filename=field.file_name().map(|name|{
            let extension=name.rsplit('.').next().filter(|s|s.len()<=8&&s.bytes().all(|b|b.is_ascii_alphanumeric()));
            extension.map(|ext|format!("audio-upload.{ext}")).unwrap_or_else(||"audio-upload".into())
        });
        let content_type=field.content_type().filter(|s|!s.chars().any(char::is_control)).map(str::to_string);
        let bytes=field.bytes().await.map_err(|_|ApiError::bad_request("Incomplete audio upload"))?;
        total=total.saturating_add(bytes.len());if total>BODY_LIMIT{return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE,"body_too_large","Audio upload too large"));}
        if name=="model" {
            if model.is_some()||filename.is_some(){return Err(ApiError::bad_request("Exactly one text model field is required"));}
            let name=std::str::from_utf8(&bytes).map_err(|_|ApiError::bad_request("Invalid model name"))?.trim().to_string();
            if name.is_empty()||name.len()>256{return Err(ApiError::bad_request("Invalid model name"));}model=Some(name);
        }
        if name=="file" {if file.is_some()||bytes.is_empty(){return Err(ApiError::bad_request("Exactly one nonempty audio file is required"));}file=Some(bytes.clone());}
        parts.push(Part{name,filename,content_type,bytes});
    }
    let model=model.ok_or_else(||ApiError::bad_request("Audio model is required"))?;
    let file=file.ok_or_else(||ApiError::bad_request("Audio file is required"))?;
    let duration=crate::audio_meter::input_duration_ms(&file).await;
    Ok((model,parts,duration))
}
fn encode(parts:&[Part],model:&str)->(String,Vec<u8>){
    let boundary=format!("tokengo-{}",uuid::Uuid::new_v4());let mut body=Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"",part.name).as_bytes());
        if let Some(filename)=&part.filename{body.extend_from_slice(format!("; filename=\"{filename}\"").as_bytes());}
        body.extend_from_slice(b"\r\n");if let Some(mime)=&part.content_type{body.extend_from_slice(format!("Content-Type: {mime}\r\n").as_bytes());}body.extend_from_slice(b"\r\n");
        body.extend_from_slice(if part.name=="model"{model.as_bytes()}else{part.bytes.as_ref()});body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());(format!("multipart/form-data; boundary={boundary}"),body)
}
struct Source { channel:Channel, secret:Option<String>, credential:Option<db::provider_pool::CredentialRef>, price:Option<billing::UnitPrice> }
fn source(state:&SharedState,auth:&db::identity::AuthContext,model:&str)->Result<Source,ApiError>{
    let c=state.conn();
    if !auth.permits_model(model){return Err(ApiError::new(StatusCode::FORBIDDEN,"model_denied","Audio model is not permitted"));}
    for mut channel in db::model_routes::channels_for(&c,model)? {
        if !channel.enabled||channel.base_url.trim().is_empty()||db::faults::blocked(&c,channel.id.0,model,db::now_ms())? {continue;}
        let credential=db::provider_pool::source_credential(&c,channel.id.0)?;
        let oauth:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_channel_bindings WHERE channel_id=?1)",[channel.id.0],|r|r.get(0)).map_err(db::DbError::from)?;
        if credential.as_ref().is_some_and(|k|!k.enabled)||oauth{continue;}
        let price=billing::unit_price(&c,model,channel.id.0)?;
        if price.as_ref().is_some_and(|p|!matches!(p.unit.as_str(),"second"|"minute"|"hour")){continue;}
        if !auth.permits_channel(channel.id.0,price.as_ref().is_none_or(|p|p.micro_per_unit>0)){continue;}
        channel.headers=channels::headers(&c,&state.secrets,channel.id)?;
        let secret=db::provider_pool::load_source_secret(&c,&state.secrets,channel.id.0)?;
        return Ok(Source{channel,secret,credential,price});
    }
    Err(ApiError::not_found("No active authorized ASR source with duration pricing is configured"))
}
fn billing_error(error:db::DbError)->ApiError {match error {db::DbError::Other(message)=>ApiError::new(StatusCode::PAYMENT_REQUIRED,"billing_unavailable",message),e=>e.into()}}
async fn transcribe(key:GatewayKey,State(state):State<SharedState>,headers:HeaderMap,multipart:Multipart)->Result<Response,ApiError>{
    let (model,parts,measured)=upload(multipart).await?;
    let _admission=crate::admission::acquire_request_only(state.clone(),&key.auth)?;
    let selected=source(&state,&key.auth,&model)?;
    let gate=state.channel_gate(selected.channel.id.0,selected.channel.max_concurrent).ok_or_else(||ApiError::internal("Audio channel gate unavailable"))?;
    let _channel=tokio::time::timeout(Duration::from_secs(60),gate.acquire_owned()).await.map_err(|_|ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"source_busy","Audio source busy"))?.map_err(|_|ApiError::internal("Audio channel gate unavailable"))?;
    let _credential=if let Some(ref credential)=selected.credential{let gate=state.credential_gate(credential.id,credential.max_concurrent).ok_or_else(||ApiError::internal("Audio credential gate unavailable"))?;Some(tokio::time::timeout(Duration::from_secs(60),gate.acquire_owned()).await.map_err(|_|ApiError::new(StatusCode::SERVICE_UNAVAILABLE,"source_busy","Audio credential busy"))?.map_err(|_|ApiError::internal("Audio credential gate unavailable"))?)}else{None};
    // Queueing must not preserve a revoked key, changed model mapping, or stale price.
    let fresh_auth=db::identity::context(&state.conn(),key.auth.key_id)?.ok_or_else(||ApiError::unauthorized("Key no longer active"))?;
    if fresh_auth.effective_policy().tpm.is_some(){return Err(ApiError::new(StatusCode::FORBIDDEN,"audio_tpm_unsupported","ASR cannot measure token quota usage"));}
    let fresh=source(&state,&fresh_auth,&model)?;
    if fresh.channel.id!=selected.channel.id||fresh.channel.base_url!=selected.channel.base_url||fresh.channel.model_alias!=selected.channel.model_alias||fresh.channel.max_concurrent!=selected.channel.max_concurrent||fresh.credential!=selected.credential||fresh.price!=selected.price{return Err(ApiError::new(StatusCode::CONFLICT,"source_changed","ASR source changed while queued; retry explicitly"));}
    let source=fresh;
    let bound=source.price.as_ref().map(|p|p.max_quantity_milli).unwrap_or_else(||measured.unwrap_or(3_600_000));
    if source.price.as_ref().is_some_and(|p|measured.is_some_and(|n|n>p.max_quantity_milli)){return Err(ApiError::bad_request("Audio duration exceeds the configured price bound"));}
    let started=db::now_ms();
    let request_id={let c=state.conn();let id=requests::start(&c,"/v1/audio/transcriptions",&model,client_ip(&headers).as_deref(),false)?;c.execute("UPDATE requests SET user_id=?2,key_id=?3,auth_snapshot=?4,request_path='/v1/audio/transcriptions' WHERE id=?1",params![id,key.auth.user_id,key.auth.key_id,serde_json::to_string(&fresh_auth).map_err(|_|ApiError::internal("Audio identity snapshot failed"))?]).map_err(db::DbError::from)?;id};
    let mut guard=Guard{state:state.clone(),reservation:None,request_id,channel:source.channel.id.0,model:model.clone(),upstream_model:source.channel.model_alias.clone().unwrap_or_else(||model.clone()),started,http_status:None,dispatched:false,done:false};
    let attempt=uuid::Uuid::new_v4().to_string();
    guard.reservation=Some(if let Some(ref price)=source.price{billing::reserve_units(&state.conn(),key.auth.user_id,key.auth.key_id,request_id,&attempt,price,bound)}else{billing::reserve_units_unpriced(&state.conn(),key.auth.user_id,key.auth.key_id,request_id,&attempt,source.channel.id.0,&model,bound)}.map_err(billing_error)?);
    state.conn().execute("UPDATE billing_ledger SET source_snapshot=?2 WHERE id=?1",params![guard.reservation,serde_json::json!({"kind":"audio_transcription","channel_name":source.channel.name,"upstream_model":guard.upstream_model,"input_audio_ms":measured}).to_string()]).map_err(db::DbError::from)?;
    let (mime,body)=encode(&parts,&guard.upstream_model);
    let client=crate::egress::source_client(&state,&source.channel,crate::egress::Purpose::Inference).map_err(|_|ApiError::internal("Audio outbound policy unavailable"))?;
    let mut request=client.post(crate::upstream::join_url(&source.channel.base_url,"audio/transcriptions")).timeout(DEADLINE).header(header::CONTENT_TYPE,mime).body(body);
    if !source.channel.headers.iter().any(|(n,_)|n.eq_ignore_ascii_case("authorization")||n.eq_ignore_ascii_case("x-api-key")){if let Some(secret)=source.secret{let mut value=header::HeaderValue::from_str(&format!("Bearer {secret}")).map_err(|_|ApiError::internal("Invalid upstream audio credential"))?;value.set_sensitive(true);request=request.header(header::AUTHORIZATION,value);}}
    for(name,value)in &source.channel.headers{if matches!(name.to_ascii_lowercase().as_str(),"host"|"content-length"|"content-type"|"cookie"){continue;}let mut value=header::HeaderValue::from_str(value).map_err(|_|ApiError::internal("Invalid upstream audio header"))?;value.set_sensitive(true);request=request.header(name.as_str(),value);}
    guard.dispatched=true;
    let mut response=match request.send().await{Ok(response)=>response,Err(_)=>{guard.finish(None,"upstream_error");return Err(ApiError::new(StatusCode::BAD_GATEWAY,"upstream_error","Audio upstream request failed"));}};
    let status=response.status();guard.http_status=Some(status.as_u16());
    let response_type=response.headers().get(header::CONTENT_TYPE).cloned();let trace=response.headers().get("x-siliconcloud-trace-id").cloned();
    let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|ApiError::new(StatusCode::BAD_GATEWAY,"upstream_error","Audio upstream body interrupted"))?{if chunk.len()>RESPONSE_LIMIT.saturating_sub(bytes.len()){return Err(ApiError::new(StatusCode::BAD_GATEWAY,"upstream_response_too_large","Audio response too large"));}bytes.extend_from_slice(&chunk);}
    if status.is_success(){
        let reported=crate::audio_meter::reported_duration_ms(&bytes);
        let provenance=serde_json::json!({"kind":"audio_transcription","channel_name":source.channel.name,"upstream_model":guard.upstream_model,"input_audio_ms":measured,"reported_audio_ms":reported,"quantity_milli":reported.or(measured),"measured_quantity_source":if reported.is_some(){"upstream_reported"}else if measured.is_some(){"media_duration"}else{"unknown"}});
        if state.conn().execute("UPDATE billing_ledger SET source_snapshot=?2 WHERE id=?1",params![guard.reservation,provenance.to_string()]).is_err(){tracing::warn!(reservation_id=guard.reservation,"ASR measurement provenance could not be saved");}
        guard.finish(reported.or(measured),"success");
    }else{guard.finish(None,"upstream_error");}
    let mut result=(status,Body::from(bytes)).into_response();if let Some(mime)=response_type{result.headers_mut().insert(header::CONTENT_TYPE,mime);}if let Some(trace)=trace{result.headers_mut().insert("x-siliconcloud-trace-id",trace);}Ok(result)
}
struct Guard {state:SharedState,reservation:Option<i64>,request_id:i64,channel:i64,model:String,upstream_model:String,started:i64,http_status:Option<u16>,dispatched:bool,done:bool}
impl Guard {
    fn finish(&mut self,quantity:Option<i64>,outcome:&str){if self.done{return;}self.done=true;if let Some(id)=self.reservation.take(){if crate::billing_writer::settle_units(&self.state,id,quantity,outcome).is_err(){tracing::error!(reservation_id=id,"ASR settlement pending durable reconciliation");}}
        let status=if outcome=="success"{requests::Status::Ok}else if outcome=="canceled"{requests::Status::Canceled}else if self.http_status.is_some_and(|s|(400..500).contains(&s)){requests::Status::ClientError}else{requests::Status::Failed};
        if self.dispatched{self.state.enqueue_write(crate::state::WriteJob::Attempt(requests::AttemptArgs{request_id:self.request_id,ordinal:1,channel_id:self.channel,model_name:self.model.clone(),upstream_model:self.upstream_model.clone(),outcome:outcome.into(),http_status:self.http_status,duration_ms:(db::now_ms()-self.started).max(0),first_byte_ms:None,error:None}));}
        self.state.enqueue_write(crate::state::WriteJob::Finish(requests::FinishArgs{request_id:self.request_id,status,http_status:self.http_status,channel_id:Some(self.channel),latency_ms:(db::now_ms()-self.started).max(0),first_byte_ms:None,input_tokens:0,output_tokens:0,input_tokens_known:false,output_tokens_known:false,usage_source:if quantity.is_some(){"audio_duration"}else{"unreported"}.into(),streamed:false,error:None,served_model:Some(self.model.clone()),error_code:if outcome=="success"{None}else{Some(outcome.into())}}));
    }
}
impl Drop for Guard{fn drop(&mut self){if !self.done{self.finish(if self.dispatched{None}else{Some(0)},if self.dispatched{"canceled"}else{"not_sent"});}}}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes,http::Request};
    use std::sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}};
    use tower::ServiceExt;
    fn wav()->Bytes {
        let n=40000u32;let mut b=Vec::new();b.extend_from_slice(b"RIFF");b.extend_from_slice(&(36+n).to_le_bytes());b.extend_from_slice(b"WAVEfmt ");b.extend_from_slice(&16u32.to_le_bytes());b.extend_from_slice(&1u16.to_le_bytes());b.extend_from_slice(&1u16.to_le_bytes());b.extend_from_slice(&16000u32.to_le_bytes());b.extend_from_slice(&32000u32.to_le_bytes());b.extend_from_slice(&2u16.to_le_bytes());b.extend_from_slice(&16u16.to_le_bytes());b.extend_from_slice(b"data");b.extend_from_slice(&n.to_le_bytes());b.resize(44+n as usize,0);Bytes::from(b)
    }
    struct Fixture {state:SharedState,key:String,channel:i64,hits:Arc<AtomicUsize>,bodies:Arc<Mutex<Vec<Vec<u8>>>>,server:tokio::task::JoinHandle<()>,dir:std::path::PathBuf}
    impl Drop for Fixture{fn drop(&mut self){self.server.abort();let _=std::fs::remove_dir_all(&self.dir);}}
    impl Fixture {
        async fn new(reported:bool)->Self{
            let hits=Arc::new(AtomicUsize::new(0));let bodies=Arc::new(Mutex::new(Vec::new()));let h=hits.clone();let captured=bodies.clone();
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
            let upstream=Router::new().route("/v1/audio/transcriptions",post(move|headers:HeaderMap,body:Bytes|{let h=h.clone();let captured=captured.clone();async move{h.fetch_add(1,Ordering::SeqCst);assert_eq!(headers.get(header::AUTHORIZATION).unwrap(),"Bearer upstream-fixture");captured.lock().unwrap().push(body.to_vec());axum::Json(if reported{serde_json::json!({"text":"fixture transcript","duration":1.5,"usage":{"total_tokens":999999}})}else{serde_json::json!({"text":"fixture transcript"})})}}));
            let server=tokio::spawn(async move{axum::serve(listener,upstream).await.unwrap();});
            let dir=std::env::temp_dir().join(format!("tokengo-asr-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
            let state=crate::AppState::init(crate::Config{listen:"127.0.0.1:0".into(),db_path:dir.join("test.db"),master_key_path:dir.join("master").to_string_lossy().into_owned(),web_dir:None,log_dir:None}).unwrap();
            let (key,channel)={let c=state.conn();let user=db::identity::create_user(&c,"ASR fixture","user",&Default::default()).unwrap();let(_,key)=db::identity::create_key(&c,user.id,"ASR fixture",&Default::default(),None,None).unwrap();let channel=channels::create(&c,&channels::NewChannel{name:"ASR fixture".into(),base_url:format!("http://{addr}/v1"),enabled:true,unwrap_envelope:false,max_concurrent:2}).unwrap();channels::set_secret(&c,&state.secrets,channel,"upstream-fixture").unwrap();channels::set_models(&c,channel,&[channels::ModelMapping::new("local-asr",Some("FunAudioLLM/SenseVoiceSmall".into()),false)]).unwrap();billing::create_unit_price(&c,&billing::UnitPrice{id:0,channel_id:channel.0,model:"local-asr".into(),currency:"CNY".into(),unit:"minute".into(),micro_per_unit:120000,max_quantity_milli:600000},"fixture").unwrap();(key,channel.0)};
            Self{state,key,channel,hits,bodies,server,dir}
        }
        async fn request(&self,parts:Vec<Part>,model:&str,authenticated:bool)->(StatusCode,Vec<u8>){let(mime,body)=encode(&parts,model);let mut r=Request::builder().method("POST").uri("/v1/audio/transcriptions").header(header::CONTENT_TYPE,mime);if authenticated{r=r.header(header::AUTHORIZATION,format!("Bearer {}",self.key));}let response=crate::app(self.state.clone()).oneshot(r.body(Body::from(body)).unwrap()).await.unwrap();let status=response.status();(status,to_bytes(response.into_body(),RESPONSE_LIMIT).await.unwrap().to_vec())}
        fn parts()->Vec<Part>{vec![Part{name:"model".into(),filename:None,content_type:None,bytes:Bytes::from_static(b"local-asr")},Part{name:"file".into(),filename:Some("fixture.wav".into()),content_type:Some("audio/wav".into()),bytes:wav()},Part{name:"duration".into(),filename:None,content_type:None,bytes:Bytes::from_static(b"999999")}]}
    }
    #[tokio::test] async fn asr_measures_audio_and_maps_model_without_forwarding_client_key(){let f=Fixture::new(false).await;let(status,body)=f.request(Fixture::parts(),"local-asr",true).await;assert_eq!(status,StatusCode::OK,"{}",String::from_utf8_lossy(&body));assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["text"],"fixture transcript");assert_eq!(f.hits.load(Ordering::SeqCst),1);let captured=f.bodies.lock().unwrap();assert!(captured[0].windows(b"FunAudioLLM/SenseVoiceSmall".len()).any(|v|v==b"FunAudioLLM/SenseVoiceSmall"));drop(captured);let c=f.state.conn();let row=billing::usage(&c,&billing::UsageQuery{channel_id:Some(f.channel),..Default::default()}).unwrap().pop().unwrap();assert_eq!(row.currency,"CNY");assert_eq!(row.quantity_milli,Some(1250));assert_eq!(row.observed_cost_micro,Some(2500));assert_eq!(row.debit_micro,Some(2500));assert_eq!(row.input_tokens,None);assert_eq!(row.output_tokens,None);assert_eq!(row.source_snapshot["measured_quantity_source"],"media_duration");assert_eq!(row.source_snapshot["quantity_milli"],1250);}
    #[tokio::test] async fn asr_reported_audio_seconds_override_container_not_token_count(){let f=Fixture::new(true).await;let(status,_)=f.request(Fixture::parts(),"local-asr",true).await;assert_eq!(status,StatusCode::OK);let c=f.state.conn();let row=billing::usage(&c,&billing::UsageQuery{channel_id:Some(f.channel),..Default::default()}).unwrap().pop().unwrap();assert_eq!(row.quantity_milli,Some(1500));assert_eq!(row.observed_cost_micro,Some(3000));assert_eq!(row.debit_micro,Some(3000));assert!(!row.overrun);let frozen:i64=c.query_row("SELECT coalesce(max(frozen),0) FROM billing_budgets WHERE currency='CNY'",[],|r|r.get(0)).unwrap();let held:i64=c.query_row("SELECT coalesce(sum(reserved_micro),0) FROM billing_budgets WHERE currency='CNY'",[],|r|r.get(0)).unwrap();assert_eq!(frozen,0);assert_eq!(held,0);assert_eq!(row.input_tokens,None);assert_eq!(row.source_snapshot["measured_quantity_source"],"upstream_reported");assert_eq!(row.source_snapshot["reported_audio_ms"],1500);}
    #[tokio::test] async fn asr_disabled_parent_without_credential_cannot_receive_calls(){let f=Fixture::new(false).await;{let c=f.state.conn();c.execute("INSERT INTO provider_channels(name,enabled,created_ms) VALUES('disabled ASR parent',0,0)",[]).unwrap();let parent=c.last_insert_rowid();db::provider_channels::set_membership(&c,f.channel,Some(parent)).unwrap();assert!(db::provider_pool::source_credential(&c,f.channel).unwrap().is_none());}assert_eq!(f.request(Fixture::parts(),"local-asr",true).await.0,StatusCode::NOT_FOUND);assert_eq!(f.hits.load(Ordering::SeqCst),0);}
    #[tokio::test] async fn asr_key_model_scope_cannot_be_bypassed(){let mut f=Fixture::new(false).await;let key={let c=f.state.conn();let owner=db::identity::create_user(&c,"Other ASR owner","user",&Default::default()).unwrap();let policy=db::identity::Policy{allowed_models:Some(vec!["other-model".into()]),..Default::default()};db::identity::create_key(&c,owner.id,"restricted ASR",&policy,None,None).unwrap().1};f.key=key;assert_eq!(f.request(Fixture::parts(),"local-asr",true).await.0,StatusCode::FORBIDDEN);assert_eq!(f.hits.load(Ordering::SeqCst),0);}
    #[tokio::test] async fn asr_malformed_upload_and_inactive_model_never_reach_upstream(){let f=Fixture::new(false).await;let mut parts=Fixture::parts();parts.push(Part{name:"model".into(),filename:None,content_type:None,bytes:Bytes::from_static(b"local-asr")});assert_eq!(f.request(parts,"local-asr",true).await.0,StatusCode::BAD_REQUEST);assert_eq!(f.request(Fixture::parts(),"local-asr",false).await.0,StatusCode::UNAUTHORIZED);db::model_routes::set_enabled(&f.state.conn(),"local-asr",false).unwrap();assert_eq!(f.request(Fixture::parts(),"local-asr",true).await.0,StatusCode::NOT_FOUND);assert_eq!(f.hits.load(Ordering::SeqCst),0);}
}
