//! Chat reasoning alias must reach a native Responses source without losing
//! explicit client intent or weakening authorization and local validation.
use axum::{body::Body, extract::State, response::{IntoResponse, Response}, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{app, db, AppState, Config, SharedState};
use tokengo_server::protocol::Protocol;
use tokengo_shared::ChannelId;

type Seen = Arc<Mutex<Vec<Value>>>;
struct Http { url: String, task: tokio::task::JoinHandle<()> }
impl Drop for Http { fn drop(&mut self) { self.task.abort(); } }
async fn serve(router: Router) -> Http {
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url=format!("http://{}",listener.local_addr().unwrap());
    Http { url, task:tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); }) }
}
async fn native_responses(State(seen):State<Seen>,Json(body):Json<Value>)->Response {
    seen.lock().unwrap().push(body.clone());
    let response=json!({"id":"resp_alias","object":"response","status":"completed","model":"native-luna","output":[{"id":"msg_alias","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"alias answer","annotations":[]}]}],"usage":{"input_tokens":7,"output_tokens":3,"total_tokens":10}});
    if body["stream"]==true {
        let events=[
            json!({"type":"response.created","response":{"id":"resp_alias","object":"response","status":"in_progress"}}),
            json!({"type":"response.output_text.delta","item_id":"msg_alias","output_index":0,"content_index":0,"delta":"alias answer"}),
            json!({"type":"response.completed","response":response}),
        ];
        let wire=events.iter().map(|event|format!("event: {}\ndata: {}\n\n",event["type"].as_str().unwrap(),event)).collect::<String>();
        return ([("content-type","text/event-stream")],Body::from(wire)).into_response();
    }
    Json(response).into_response()
}
struct Fixture {
    state: SharedState, gateway: Http, _source: Http, seen: Seen,
    key: String, key_id: i64, user_id: i64, channel: ChannelId, dir: std::path::PathBuf,
}
impl Drop for Fixture { fn drop(&mut self) { std::fs::remove_dir_all(&self.dir).ok(); } }
impl Fixture {
    async fn new()->Self {
        let seen=Arc::new(Mutex::new(Vec::new()));
        let source=serve(Router::new().route("/v1/responses",post(native_responses)).with_state(seen.clone())).await;
        let dir=std::env::temp_dir().join(format!("chat-reasoning-alias-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config { listen:"127.0.0.1:0".into(),db_path:dir.join("test.db"),master_key_path:dir.join("master").to_string_lossy().into(),web_dir:None,log_dir:None }).unwrap();
        let (key,key_id,user_id,channel)={
            let c=state.conn();
            let user=db::identity::create_user(&c,"alias owner","user",&Default::default()).unwrap();
            let (row,key)=db::identity::create_key(&c,user.id,"alias key",&Default::default(),None,None).unwrap();
            let channel=db::channels::create(&c,&db::channels::NewChannel { name:"native responses".into(),base_url:format!("{}/v1",source.url),enabled:true,unwrap_envelope:false,max_concurrent:0 }).unwrap();
            db::channels::set_models(&c,channel,&[db::channels::ModelMapping::new("gpt-6-luna",Some("native-luna".into()),false)]).unwrap();
            tokengo_server::protocol_config::save(&c,channel.0,&tokengo_server::protocol_config::Capabilities { native:vec![Protocol::Responses],allow_conversion:true,..Default::default() }).unwrap();
            tokengo_server::reasoning_capabilities::configure(&c,channel.0,"gpt-6-luna",Some(&["low".into(),"medium".into(),"high".into()]),Some("medium")).unwrap();
            db::billing::create_price(&c,&db::billing::Price { id:0,channel_id:channel.0,model:"gpt-6-luna".into(),input_micro_per_million:1,output_micro_per_million:1,max_input_tokens:8192,max_output_tokens:64 },"alias-test").unwrap();
            db::route_policy::save(&c,"gpt-6-luna",&db::route_policy::RoutePolicy { retry_times:Some(0),probe_enabled:Some(false),include_in_auto:Some(false),..Default::default() }).unwrap();
            (key,row.id,user.id,channel)
        };
        let gateway=serve(app(state.clone())).await;
        Self { state,gateway,_source:source,seen,key,key_id,user_id,channel,dir }
    }
    async fn call(&self,extra:Value,stream:bool)->(u16,String) {
        let mut body=json!({"model":"gpt-6-luna","messages":[{"role":"user","content":"hello"}],"max_tokens":16,"stream":stream});
        for (key,value) in extra.as_object().unwrap() { body[key]=value.clone(); }
        let response=reqwest::Client::new().post(format!("{}/v1/chat/completions",self.gateway.url)).bearer_auth(&self.key).json(&body).send().await.unwrap();
        (response.status().as_u16(),response.text().await.unwrap())
    }
}

#[tokio::test]
async fn chat_reasoning_alias_keeps_live_key_user_and_model_permissions() {
    let f=Fixture::new().await;
    let request=json!({"reasoning":{"effort":"low"}});
    assert_eq!(f.call(request.clone(),false).await.0,200);
    f.seen.lock().unwrap().clear();
    {
        let c=f.state.conn();let mut user=db::identity::user(&c,f.user_id).unwrap().unwrap();
        user.policy.allowed_models=Some(vec![]);db::identity::update_user(&c,&user).unwrap();
    }
    assert_eq!(f.call(request.clone(),false).await.0,403);
    assert!(f.seen.lock().unwrap().is_empty());
    {
        let c=f.state.conn();let mut user=db::identity::user(&c,f.user_id).unwrap().unwrap();
        user.policy.allowed_models=None;user.enabled=false;db::identity::update_user(&c,&user).unwrap();
    }
    assert_eq!(f.call(request.clone(),false).await.0,401);
    assert!(f.seen.lock().unwrap().is_empty());
    {
        let c=f.state.conn();let mut user=db::identity::user(&c,f.user_id).unwrap().unwrap();user.enabled=true;db::identity::update_user(&c,&user).unwrap();
        c.execute("UPDATE gateway_keys SET enabled=0 WHERE id=?1",[f.key_id]).unwrap();
    }
    assert_eq!(f.call(request.clone(),false).await.0,401);
    assert!(f.seen.lock().unwrap().is_empty());
    {
        let c=f.state.conn();c.execute("UPDATE gateway_keys SET enabled=1 WHERE id=?1",[f.key_id]).unwrap();
        c.execute("UPDATE channels SET enabled=0 WHERE id=?1",[f.channel.0]).unwrap();
    }
    let (status,_) = f.call(request,false).await;assert_ne!(status,200);
    assert!(f.seen.lock().unwrap().is_empty());
}
