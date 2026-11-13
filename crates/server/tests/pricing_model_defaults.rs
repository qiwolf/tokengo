//! Price configuration inherits current model parameters without editing those parameters.
use axum::{body::{Body,to_bytes},http::{Request,StatusCode}};
use serde_json::{json,Value};
use tokengo_server::{app,db,AppState,Config,SharedState};
use tower::ServiceExt;

struct Fixture{state:SharedState,admin:String,channel:i64,dir:std::path::PathBuf}
impl Drop for Fixture{fn drop(&mut self){let _=std::fs::remove_dir_all(&self.dir);}}
impl Fixture {
    fn new()->Self{
        let dir=std::env::temp_dir().join(format!("tokengo-model-price-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into_owned(),..Default::default()}).unwrap();
        let (admin,channel)={let c=state.conn();let admin=db::sessions::create_session(&c,None).unwrap();let channel=db::channels::create(&c,&db::channels::NewChannel{name:"price source".into(),base_url:"https://fixture.invalid/v1".into(),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
            let names=[("chat","physical-chat"),("embed","physical-embed"),("rank","physical-rank"),("voice","Qwen/Qwen3-ASR-1.7B"),("speak","physical-tts"),("picture","physical-image"),("movie","physical-video"),("tool","physical-tool"),("unrecorded","physical-unrecorded"),("audio","physical-audio")];
            db::channels::set_models(&c,channel,&names.iter().map(|(name,upstream)|db::channels::ModelMapping::new(*name,Some((*upstream).into()),false)).collect::<Vec<_>>()).unwrap();
            for (name,kind) in [("chat",tokengo_shared::ModelKind::Chat),("embed",tokengo_shared::ModelKind::Embeddings),("rank",tokengo_shared::ModelKind::Rerank),("voice",tokengo_shared::ModelKind::Audio),("speak",tokengo_shared::ModelKind::Audio),("picture",tokengo_shared::ModelKind::Image),("movie",tokengo_shared::ModelKind::Video),("audio",tokengo_shared::ModelKind::Audio)]{db::kinds::set(&c,name,kind).unwrap();}
            db::model_access::set_catalog_purpose(&c,"tool","tools").unwrap();
            c.execute("INSERT INTO channel_catalog(channel_id,model_id,kind_claim,context_window) VALUES(?1,'physical-tts','tts',NULL)",[channel.0]).unwrap();
            (admin,channel.0)};
        Self{state,admin,channel,dir}
    }
    async fn call(&self,path:&str,body:Option<Value>,admin:bool)->(StatusCode,Value){
        let mut request=Request::builder().uri(path).method(if body.is_some(){"POST"}else{"GET"}).header("content-type","application/json");if admin{request=request.header("authorization",format!("Bearer {}",self.admin));}
        let response=app(self.state.clone()).oneshot(request.body(body.map(|v|Body::from(v.to_string())).unwrap_or_else(Body::empty)).unwrap()).await.unwrap();let status=response.status();let bytes=to_bytes(response.into_body(),2<<20).await.unwrap();(status,serde_json::from_slice(&bytes).unwrap())
    }
    fn configure_bounds(&self,input:i64,output:i64){tokengo_server::protocol_config::save(&self.state.conn(),self.channel,&tokengo_server::protocol_config::Capabilities{max_input_tokens:Some(input),max_output_tokens:Some(output),..Default::default()}).unwrap();}
    fn price(&self,model:&str)->Value{json!({"channel_id":self.channel,"model":model,"input_micro_per_million":1000000,"output_micro_per_million":2000000})}
}
#[tokio::test]
async fn model_pricing_metadata_is_admin_only_type_aware_and_source_bound(){
    let f=Fixture::new();f.configure_bounds(128000,32000);
    assert_eq!(f.call("/api/pricing/options",None,false).await.0,StatusCode::UNAUTHORIZED);
    let (_,secret)=db::identity::create_key(&f.state.conn(),1,"metadata forbidden",&Default::default(),None,None).unwrap();
    let response=app(f.state.clone()).oneshot(Request::builder().uri("/api/pricing/options").header("authorization",format!("Bearer {secret}")).body(Body::empty()).unwrap()).await.unwrap();assert_eq!(response.status(),StatusCode::FORBIDDEN);
    let(status,v)=f.call("/api/pricing/options",None,true).await;assert_eq!(status,StatusCode::OK,"{v}");let rows=v["options"].as_array().unwrap();let get=|name:&str|rows.iter().find(|r|r["model"]==name).unwrap();
    assert_eq!(get("chat")["max_input_tokens"],128000);assert_eq!(get("chat")["max_output_tokens"],32000);assert_eq!(get("chat")["input_bound_source"],"source_protocols");
    for name in ["embed","rank"]{assert_eq!(get(name)["pricing_kind"],"token");assert_eq!(get(name)["max_output_tokens"],0);}
    assert_eq!(get("voice")["task_type"],"asr");assert_eq!(get("voice")["task_source"],"model_identifier");assert_eq!(get("voice")["allowed_units"],json!(["second","minute","hour"]));assert_eq!(get("voice")["automatic_settlement"],true);assert!(get("voice")["max_audio_seconds"].is_null());
    assert_eq!(get("speak")["task_type"],"tts");assert_eq!(get("speak")["task_source"],"upstream_catalog");assert!(get("speak")["recommended_unit"].is_null());assert_eq!(get("speak")["allowed_units"],json!(["character","byte"]));assert_eq!(get("speak")["automatic_settlement"],false);
    assert_eq!(get("picture")["pricing_kind"],"unit");assert_eq!(get("picture")["recommended_unit"],"image");assert_eq!(get("picture")["automatic_settlement"],false);
    assert_eq!(get("movie")["pricing_kind"],"unit");assert!(get("movie")["recommended_unit"].is_null());assert_eq!(get("movie")["allowed_units"],json!(["video","second","minute","hour"]));assert_eq!(get("movie")["automatic_settlement"],false);
    assert_eq!(get("tool")["pricing_kind"],"request");assert_eq!(get("unrecorded")["pricing_kind"],"unknown");assert_eq!(get("audio")["task_type"],"audio");assert!(get("audio")["recommended_unit"].is_null());
    assert!(!v.to_string().contains(&secret));
    db::model_routes::set_enabled(&f.state.conn(),"chat",false).unwrap();let(_,v)=f.call("/api/pricing/options",None,true).await;assert!(v["options"].as_array().unwrap().iter().all(|r|r["model"]!="chat"));
}
#[tokio::test]
async fn creating_token_price_uses_current_config_and_prompts_only_for_missing_bounds(){
    let f=Fixture::new();
    let(status,_)=f.call("/api/pricing",Some(f.price("chat")),true).await;assert_eq!(status,StatusCode::BAD_REQUEST);
    let mut manual=f.price("chat");manual["max_input_tokens"]=json!(1000);manual["max_output_tokens"]=json!(2000);let(status,v)=f.call("/api/pricing",Some(manual.clone()),true).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["price"]["max_input_tokens"],1000);
    f.configure_bounds(128000,32000);
    let raw_before=db::settings::get(&f.state.conn(),&format!("channel_protocols:{}",f.channel)).unwrap();
    let(status,v)=f.call("/api/pricing",Some(f.price("chat")),true).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["price"]["max_input_tokens"],128000);assert_eq!(v["price"]["max_output_tokens"],32000);
    // A stale client cannot change already-configured model parameters by posting
    // the old duplicate fields. Missing configuration still accepts explicit fallback.
    let(status,v)=f.call("/api/pricing",Some(manual),true).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["price"]["max_input_tokens"],128000);assert_eq!(v["price"]["max_output_tokens"],32000);assert_eq!(raw_before,db::settings::get(&f.state.conn(),&format!("channel_protocols:{}",f.channel)).unwrap());
    f.configure_bounds(256000,64000);let(status,v)=f.call("/api/pricing",Some(f.price("chat")),true).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["price"]["max_input_tokens"],256000);assert_eq!(v["price"]["max_output_tokens"],64000);
    for name in ["voice","speak","picture","movie","tool","audio"]{let(status,_)=f.call("/api/pricing",Some(f.price(name)),true).await;assert_eq!(status,StatusCode::BAD_REQUEST,"{name}");}
}
#[tokio::test]
async fn source_model_context_and_oauth_bounds_are_not_replaced_by_another_model_or_nominal_registry(){
    let f=Fixture::new();
    {let c=f.state.conn();c.execute("INSERT INTO channel_context_overrides(channel_id,upstream_model,context_window) VALUES(?1,'physical-chat',8192)",[f.channel]).unwrap();c.execute("INSERT INTO channel_catalog(channel_id,model_id,context_window) VALUES(?1,'physical-embed',4096)",[f.channel]).unwrap();}
    let(_,v)=f.call("/api/pricing/options",None,true).await;let rows=v["options"].as_array().unwrap();let chat=rows.iter().find(|r|r["model"]=="chat").unwrap();assert_eq!(chat["max_input_tokens"],8192);assert!(chat["max_output_tokens"].is_null());let embed=rows.iter().find(|r|r["model"]=="embed").unwrap();assert_eq!(embed["max_input_tokens"],4096);assert_eq!(embed["max_output_tokens"],0);let rank=rows.iter().find(|r|r["model"]=="rank").unwrap();assert!(rank["max_input_tokens"].is_null());
    let mut body=f.price("chat");body["max_output_tokens"]=json!(1024);let(status,v)=f.call("/api/pricing",Some(body),true).await;assert_eq!(status,StatusCode::OK,"{v}");assert_eq!(v["price"]["max_input_tokens"],8192);
    f.configure_bounds(128000,32000);let(_,v)=f.call("/api/pricing/options",None,true).await;assert_eq!(v["options"].as_array().unwrap().iter().find(|r|r["model"]=="chat").unwrap()["max_input_tokens"],8192);
    {let c=f.state.conn();c.execute("INSERT INTO oauth_accounts(label,issuer,subject,client_id,status,scopes_json,expires_at,earliest_refresh_at) VALUES('synthetic','https://auth.openai.com','synthetic','synthetic','active','[]',?1,0)",[db::now_ms()+60000]).unwrap();let account=c.last_insert_rowid();c.execute("INSERT INTO oauth_channel_bindings(channel_id,account_id,max_input_tokens,max_output_tokens) VALUES(?1,?2,4096,2048)",rusqlite::params![f.channel,account]).unwrap();}
    let(_,v)=f.call("/api/pricing/options",None,true).await;let chat=v["options"].as_array().unwrap().iter().find(|r|r["model"]=="chat").unwrap();assert_eq!(chat["max_input_tokens"],4096);assert_eq!(chat["max_output_tokens"],2048);assert_eq!(chat["input_bound_source"],"oauth_binding");
}
#[tokio::test]
async fn units_follow_model_type_and_cannot_create_accidental_token_prices(){
    let f=Fixture::new();
    for(name,unit,valid)in[("voice","minute",true),("voice","character",false),("speak","character",true),("speak","second",false),("picture","image",true),("movie","video",true),("movie","second",true),("movie","minute",true),("movie","hour",true),("movie","character",false),("chat","second",false)]{
        let body=json!({"channel_id":f.channel,"model":name,"unit":unit,"micro_per_unit":120000,"max_quantity_milli":60000});let(status,v)=f.call("/api/unit-pricing",Some(body),true).await;assert_eq!(status,if valid{StatusCode::OK}else{StatusCode::BAD_REQUEST},"{name}/{unit}: {v}");
    }
    let(status,_)=f.call("/api/request-pricing",Some(json!({"channel_id":f.channel,"model":"voice","micro_per_call":1})),true).await;assert_eq!(status,StatusCode::BAD_REQUEST);
    let(status,v)=f.call("/api/request-pricing",Some(json!({"channel_id":f.channel,"model":"tool","micro_per_call":1})),true).await;assert_eq!(status,StatusCode::OK,"{v}");
}

#[tokio::test]
async fn price_mutation_rechecks_source_and_model_activity_without_touching_old_versions(){
    let f=Fixture::new();f.configure_bounds(8192,2048);
    let(status,v)=f.call("/api/pricing",Some(f.price("chat")),true).await;assert_eq!(status,StatusCode::OK,"{v}");let original=v["price"].clone();
    db::model_routes::set_enabled(&f.state.conn(),"chat",false).unwrap();let(status,_)=f.call("/api/pricing",Some(f.price("chat")),true).await;assert_eq!(status,StatusCode::BAD_REQUEST);
    db::model_routes::set_enabled(&f.state.conn(),"chat",true).unwrap();db::channels::set_enabled(&f.state.conn(),tokengo_shared::ChannelId(f.channel),false).unwrap();let(status,_)=f.call("/api/pricing",Some(f.price("chat")),true).await;assert_eq!(status,StatusCode::BAD_REQUEST);
    let prices=db::billing::list_prices(&f.state.conn()).unwrap();assert_eq!(prices.len(),1);assert_eq!(serde_json::to_value(&prices[0]).unwrap(),original);
}
