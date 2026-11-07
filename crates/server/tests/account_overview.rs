//! Owner APIs deliberately exercise real HTTP authentication and SQL scoping.
use axum::{body::Body,http::{Request,StatusCode}};
use serde_json::{json,Value};
use tokengo_server::{app,db,AppState,Config,SharedState};
use tower::ServiceExt;
struct Fixture {state:SharedState,owners:[i64;2],keys:[i64;2],cookies:[String;2],legacy:String}
impl Fixture {
    fn new()->Self {
        let dir=std::env::temp_dir().join(format!("tokengo-owner-overview-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into_owned(),..Config::default()}).unwrap();
        let c=state.conn();let mut owners=vec![];let mut keys=vec![];let mut cookies=vec![];
        for name in ["alice","bravo"] {
            let user=db::identity::create_user(&c,name,"user",&Default::default()).unwrap();
            let key=db::identity::create_key(&c,user.id,"same personal name",&Default::default(),None,None).unwrap().0;
            let account=db::accounts::create(&c,name,name,user.id,"test-only-stored-password-hash").unwrap();
            let credential=db::accounts::by_id(&c,account.id).unwrap().unwrap();
            let session=db::accounts::create_session(&c,&credential).unwrap();
            owners.push(user.id);keys.push(key.id);cookies.push(format!("tokengo_account_session={session}"));
        }
        let legacy=db::sessions::create_session(&c,None).unwrap();drop(c);
        Self{state,owners:owners.try_into().unwrap(),keys:keys.try_into().unwrap(),cookies:cookies.try_into().unwrap(),legacy}
    }
    async fn get(&self,path:&str,cookie:Option<&str>,bearer:Option<&str>)->(StatusCode,Value) {
        let mut request=Request::builder().uri(path);
        if let Some(cookie)=cookie {request=request.header("cookie",cookie)}
        if let Some(bearer)=bearer {request=request.header("authorization",format!("Bearer {bearer}"))}
        let response=app(self.state.clone()).oneshot(request.body(Body::empty()).unwrap()).await.unwrap();let status=response.status();
        let bytes=axum::body::to_bytes(response.into_body(),1<<20).await.unwrap();(status,serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    fn seed(&self,owner:Option<i64>,key:Option<i64>,ts:i64,endpoint:&str,model:&str,input:i64)->i64 {
        let c=self.state.conn();
        c.execute("INSERT INTO requests(user_id,key_id,ts_ms,endpoint,model,status,input_tokens,output_tokens,input_tokens_known,output_tokens_known,usage_source,client_ip,request_path,upstream_path,error,user_agent,served_model) VALUES(?1,?2,?3,?4,?5,'ok',?6,2,1,1,'reported','client-only','/photos/clip','/PRIVATE_BACKEND','PRIVATE_RAW_ERROR','PRIVATE_USER_AGENT','actual-logical-model')",rusqlite::params![owner,key,ts,endpoint,model,input]).unwrap();let id=c.last_insert_rowid();
        c.execute("INSERT INTO attempts(request_id,ordinal,channel_id,outcome,http_status,duration_ms,error,model_name,upstream_model) VALUES(?1,0,999,'success',200,13,'PRIVATE_ATTEMPT_ERROR','PRIVATE_SOURCE_MODEL','PRIVATE_ATTEMPT_MODEL')",[id]).unwrap();id
    }
}
#[tokio::test]
async fn owner_logs_details_and_stats_never_reveal_other_owners_or_admin_fields() {
    let f=Fixture::new();let now=db::now_ms();
    let mine=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"/v1/chat/completions","my-requested-model",3);
    let foreign=f.seed(Some(f.owners[1]),Some(f.keys[1]),now,"/v1/chat/completions","FOREIGN_MODEL",900);
    f.seed(None,Some(f.keys[0]),now,"/v1/embeddings","LEGACY_UNOWNED",9000);
    let passthrough=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"passthrough:91","PRIVATE_TRANSPORT_MODEL",5);
    let (status,logs)=f.get("/api/account/overview/logs",Some(&f.cookies[0]),None).await;
    assert_eq!(status,StatusCode::OK);assert_eq!(logs["logs"].as_array().unwrap().len(),2);
    let raw=logs.to_string();for marker in ["FOREIGN","LEGACY_UNOWNED","PRIVATE","passthrough:91","channel_id","channel_name","upstream_path","user_agent","auth_snapshot","error"] {assert!(!raw.contains(marker),"unexpected {marker}")}
    assert_eq!(logs["logs"][0]["id"],passthrough);assert_eq!(logs["logs"][0]["model"],"HTTP 透传");assert_eq!(logs["logs"][0]["endpoint"],"passthrough");assert_eq!(logs["logs"][0]["served_model"],"HTTP 透传");
    assert_eq!(logs["logs"][0]["key_display_name"],"same personal name");
    let (status,detail)=f.get(&format!("/api/account/overview/requests/{mine}"),Some(&f.cookies[0]),None).await;
    assert_eq!(status,StatusCode::OK);assert_eq!(detail["request"]["served_model"],"actual-logical-model");assert_eq!(detail["attempts"][0],json!({"ordinal":0,"outcome":"success","http_status":200,"duration_ms":13}));assert!(!detail.to_string().contains("PRIVATE"));
    let foreign_response=f.get(&format!("/api/account/overview/requests/{foreign}"),Some(&f.cookies[0]),None).await;
    let missing_response=f.get("/api/account/overview/requests/999999",Some(&f.cookies[0]),None).await;
    assert_eq!(foreign_response,missing_response);assert_eq!(foreign_response.0,StatusCode::NOT_FOUND);
    let (status,stats)=f.get("/api/account/overview/stats?hours=1",Some(&f.cookies[0]),None).await;
    assert_eq!(status,StatusCode::OK);let stats=&stats["stats"];assert_eq!(stats["total"],2);assert_eq!(stats["input_tokens"],8);assert_eq!(stats["output_tokens"],4);assert_eq!(stats["by_channel"],json!([]));assert!(!stats.to_string().contains("PRIVATE"));assert!(!stats.to_string().contains("FOREIGN"));
    for suffix in ["logs?user_id=2","logs?key_id=2","stats?user_id=2","stats?key_id=2"] {assert_eq!(f.get(&format!("/api/account/overview/{suffix}"),Some(&f.cookies[0]),None).await.0,StatusCode::BAD_REQUEST)}
    for path in ["/api/stats","/api/logs","/api/channels","/api/requests/1"] {assert!(!f.get(path,Some(&f.cookies[0]),None).await.0.is_success(),"{path}")}
    assert_eq!(f.get("/api/stats",None,Some(&f.legacy)).await.0,StatusCode::OK);
}
#[tokio::test]
async fn pagination_filters_before_limit_and_revoked_keys_keep_history_but_disabled_users_cannot_read() {
    let f=Fixture::new();let now=db::now_ms();
    let first=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"/v1/embeddings","same model",11);
    for _ in 0..6 {f.seed(Some(f.owners[1]),Some(f.keys[1]),now,"/v1/rerank","other model",500);}
    let second=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"/v1/rerank","same model",13);
    let c=f.state.conn();db::identity::revoke(&c,f.keys[0]).unwrap();drop(c);
    let (status,page)=f.get("/api/account/overview/logs?limit=1",Some(&f.cookies[0]),None).await;assert_eq!(status,StatusCode::OK);assert_eq!(page["logs"][0]["id"],second);assert_eq!(page["next_before_id"],second);
    let (_,page2)=f.get(&format!("/api/account/overview/logs?limit=1&before_id={second}"),Some(&f.cookies[0]),None).await;assert_eq!(page2["logs"][0]["id"],first);assert!(page2["next_before_id"].is_null());
    let (_,stats)=f.get("/api/account/overview/stats",Some(&f.cookies[0]),None).await;assert_eq!(stats["stats"]["total"],2);assert_eq!(stats["stats"]["input_tokens"],24);
    assert_eq!(f.get(&format!("/api/account/overview/requests/{first}"),Some(&f.cookies[0]),None).await.0,StatusCode::OK);
    let c=f.state.conn();c.execute("UPDATE users SET enabled=0 WHERE id=?1",[f.owners[0]]).unwrap();drop(c);
    for path in ["/api/account/overview/logs","/api/account/overview/stats",&format!("/api/account/overview/requests/{first}")] {assert_eq!(f.get(path,Some(&f.cookies[0]),None).await.0,StatusCode::UNAUTHORIZED)}
    assert_eq!(f.get("/api/account/overview/logs",None,Some(&f.legacy)).await.0,StatusCode::UNAUTHORIZED);
}
#[test]
fn owner_statistics_window_top50_and_nullable_usage_are_filtered_before_aggregation() {
    let f=Fixture::new();let since=db::now_ms()-1000;
    f.seed(Some(f.owners[0]),Some(f.keys[0]),since-1,"/v1/chat/completions","OUTSIDE_WINDOW",10000);
    for n in 0..55 {f.seed(Some(f.owners[0]),Some(f.keys[0]),since,"/v1/embeddings",&format!("model-{n:02}"),1);}
    for _ in 0..60 {f.seed(Some(f.owners[1]),Some(f.keys[1]),since,"/v1/chat/completions","FOREIGN_POPULAR",10000);}
    let c=f.state.conn();c.execute("UPDATE requests SET served_model=model WHERE user_id=?1",[f.owners[0]]).unwrap();c.execute("UPDATE requests SET input_tokens_known=0,usage_source='legacy' WHERE user_id=?1 AND model='model-01'",[f.owners[0]]).unwrap();
    let stats=db::account_overview::stats_since(&c,f.owners[0],since).unwrap();assert_eq!(stats.total,55);assert_eq!(stats.input_tokens,55);assert_eq!(stats.by_model.len(),50);assert_eq!(stats.by_model[0].model,"model-54");assert_eq!(stats.by_model.last().unwrap().model,"model-05");assert_eq!(stats.usage.unknown_requests,1);assert_eq!(stats.usage.legacy_requests,1);assert_eq!(stats.by_kind.iter().find(|r|r.kind=="embeddings").unwrap().requests,55);assert_eq!(stats.by_client[0].requests,55);
    let empty=db::account_overview::stats_since(&c,999999,since).unwrap();assert_eq!(empty.total,0);assert_eq!(empty.by_kind.len(),3);assert!(empty.by_client.is_empty());
}
#[test]
fn mixed_usage_certainty_and_terminal_statuses_remain_explicit() {
    let f=Fixture::new();let since=db::now_ms()-1000;
    let a=f.seed(Some(f.owners[0]),Some(f.keys[0]),since,"/v1/chat/completions","mixed",3);
    let b=f.seed(Some(f.owners[0]),Some(f.keys[0]),since,"/v1/chat/completions","mixed",5);
    let d=f.seed(Some(f.owners[0]),Some(f.keys[0]),since,"/v1/chat/completions","mixed",7);
    let c=f.state.conn();
    c.execute("UPDATE requests SET input_tokens_known=0 WHERE id=?1",[a]).unwrap();
    c.execute("UPDATE requests SET output_tokens_known=0,status='failed' WHERE id=?1",[b]).unwrap();
    c.execute("UPDATE requests SET input_tokens_known=0,output_tokens_known=0,usage_source='',status='client_error' WHERE id=?1",[d]).unwrap();
    c.execute("INSERT INTO requests(user_id,key_id,ts_ms,endpoint,model,status) VALUES(?1,?2,?3,'/v1/chat/completions','mixed','canceled')",rusqlite::params![f.owners[0],f.keys[0],since]).unwrap();
    let stats=db::account_overview::stats_since(&c,f.owners[0],since).unwrap();
    assert_eq!((stats.total,stats.ok,stats.failed,stats.client_errors,stats.canceled),(4,1,1,1,1));
    assert_eq!((stats.input_tokens,stats.output_tokens),(15,6));
    assert_eq!((stats.usage.unknown_requests,stats.usage.unknown_input_requests,stats.usage.unknown_output_requests,stats.usage.legacy_requests),(4,3,3,1));
    assert_eq!(stats.by_kind[0].usage,stats.usage);
    let actual=stats.by_model.iter().find(|r|r.model=="actual-logical-model").unwrap();
    assert_eq!((actual.usage.unknown_requests,actual.usage.unknown_input_requests,actual.usage.unknown_output_requests,actual.usage.legacy_requests),(3,2,2,0));
    assert_eq!(stats.by_model.iter().find(|r|r.model=="未调用上游").unwrap().usage.legacy_requests,1);
    assert_eq!(stats.by_client.iter().find(|r|r.client_ip.is_none()).unwrap().usage.legacy_requests,1);
}
#[tokio::test]
async fn time_order_cursors_are_owner_scoped_and_foreign_equals_missing() {
    let f=Fixture::new();let now=db::now_ms();
    let newer=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"/v1/chat/completions","a",1);
    let older=f.seed(Some(f.owners[0]),Some(f.keys[0]),now-1000,"/v1/chat/completions","b",1);
    let foreign=f.seed(Some(f.owners[1]),Some(f.keys[1]),now+1000,"/v1/chat/completions","foreign",1);
    let (_,page)=f.get("/api/account/overview/logs?limit=1",Some(&f.cookies[0]),None).await;
    assert_eq!(page["logs"][0]["id"],newer);
    let (_,page)=f.get(&format!("/api/account/overview/logs?limit=1&before_id={newer}"),Some(&f.cookies[0]),None).await;
    assert_eq!(page["logs"][0]["id"],older);
    assert_eq!(f.get(&format!("/api/account/overview/logs?before_id={foreign}"),Some(&f.cookies[0]),None).await,f.get("/api/account/overview/logs?before_id=999999",Some(&f.cookies[0]),None).await);
}
#[test]
fn schema44_adds_only_the_owner_index_and_preserves_schema43_requests() {
    let c=rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL DEFAULT(datetime('now')));").unwrap();
    for (version,sql) in db::schema::MIGRATIONS.iter().filter(|(version,_)|*version<=43) {c.execute_batch(sql).unwrap();c.execute("INSERT INTO schema_migrations(version) VALUES(?1)",[version]).unwrap();}
    c.execute("INSERT INTO requests(user_id,key_id,ts_ms,endpoint,model,status,request_path,upstream_path) VALUES(1,1,100,'passthrough:91','internal','ok','/photos/clip','/clip')",[]).unwrap();
    let snapshot=|c:&rusqlite::Connection|c.query_row("SELECT id,user_id,key_id,ts_ms,endpoint,model,status,request_path,upstream_path FROM requests",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,Option<String>>(8)?))).unwrap();
    let before=snapshot(&c);let old_history=c.prepare("SELECT version,applied_at FROM schema_migrations ORDER BY version").unwrap().query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    // Verify the original v44 step on its own, then the current additive upgrade.
    let (_,sql)=db::schema::MIGRATIONS.iter().find(|(version,_)|*version==44).unwrap();
    c.execute_batch(sql).unwrap();c.execute("INSERT INTO schema_migrations(version) VALUES(44)",[]).unwrap();
    assert_eq!(snapshot(&c),before);
    let history=c.prepare("SELECT version,applied_at FROM schema_migrations ORDER BY version").unwrap().query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();assert_eq!(&history[..old_history.len()],&old_history);assert_eq!(history.last().unwrap().0,44);assert_eq!(history.len(),old_history.len()+1);
    let definition:String=c.query_row("SELECT sql FROM sqlite_master WHERE name='requests_owner_recent'",[],|r|r.get(0)).unwrap();assert_eq!(definition,"CREATE INDEX requests_owner_recent ON requests(user_id,ts_ms DESC,id DESC)");
    db::migrate(&c).unwrap();assert_eq!(snapshot(&c),before);
    assert_eq!(c.query_row("SELECT MAX(version) FROM schema_migrations",[],|r|r.get::<_,i64>(0)).unwrap(),45);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM schema_migrations WHERE version<=43",[],|r|r.get::<_,i64>(0)).unwrap(),old_history.len() as i64);
}
#[test]
fn model_statistics_follow_actual_logical_models_with_neutral_auto_and_passthrough_labels() {
    let f=Fixture::new();let now=db::now_ms();
    let a=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"/v1/chat/completions","virtual-entry",3);
    let b=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"/v1/chat/completions","virtual-entry",5);
    let unsent=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"/v1/chat/completions","auto",0);
    let unknown=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"/v1/chat/completions","AUTO",0);
    let fallback=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"/v1/chat/completions","ordinary-model",7);
    let passthrough=f.seed(Some(f.owners[0]),Some(f.keys[0]),now,"passthrough:91","PRIVATE_ROUTE_MODEL",11);
    let c=f.state.conn();
    for (id,served,channel) in [(a,"actual-a",Some(1i64)),(b,"actual-b",Some(1)),(unsent,"",None),(unknown," AuTo ",Some(1)),(fallback,"   ",Some(1)),(passthrough,"PRIVATE_BACKEND_MODEL",Some(1))] {
        c.execute("UPDATE requests SET served_model=?2,channel_id=?3 WHERE id=?1",rusqlite::params![id,served,channel]).unwrap();
    }
    let stats=db::account_overview::stats_since(&c,f.owners[0],now-1).unwrap();
    assert_eq!(stats.total,6);
    let group=|name:&str|stats.by_model.iter().find(|g|g.model==name).unwrap();
    assert_eq!((group("actual-a").requests,group("actual-a").input_tokens),(1,3));
    assert_eq!((group("actual-b").requests,group("actual-b").input_tokens),(1,5));
    assert_eq!(group("未调用上游").requests,1);assert_eq!(group("实际模型未记录").requests,1);
    assert_eq!(group("ordinary-model").input_tokens,7);assert_eq!(group("HTTP 透传").input_tokens,11);
    assert!(!stats.by_model.iter().any(|g|g.model=="virtual-entry"||g.model.to_lowercase().trim()=="auto"||g.model.contains("PRIVATE")));
}
