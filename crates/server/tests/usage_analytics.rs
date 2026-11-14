//! Separate full-range reports preserve filtering/auth while cursor pages stay light.
use axum::{body::{Body,to_bytes},http::{Request,StatusCode}};
use serde_json::Value;
use tokengo_server::{app,db,AppState,Config,SharedState};
use tower::ServiceExt;
struct Fixture{state:SharedState,dir:std::path::PathBuf,admin:String,cookie:String,key:String,user:i64,key_id:i64}
impl Drop for Fixture{fn drop(&mut self){let _=std::fs::remove_dir_all(&self.dir);}}
impl Fixture{
 fn new()->Self{
  let dir=std::env::temp_dir().join(format!("usage-analytics-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
  let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("key").to_string_lossy().into(),..Default::default()}).unwrap();
  let(admin,cookie,key,user,key_id)={let c=state.conn();let admin=db::sessions::create_session(&c,None).unwrap();let user=db::identity::create_user(&c,"usage owner","user",&Default::default()).unwrap().id;let foreign=db::identity::create_user(&c,"foreign","user",&Default::default()).unwrap().id;
   let (own,key)=db::identity::create_key(&c,user,"own",&Default::default(),None,None).unwrap();let (other,_)=db::identity::create_key(&c,user,"other key",&Default::default(),None,None).unwrap();let (foreign_key,_)=db::identity::create_key(&c,foreign,"foreign key",&Default::default(),None,None).unwrap();
   let account=db::accounts::create(&c,"usage-owner","Usage owner",user,"synthetic-stored-hash").unwrap().id;let cookie=db::accounts::create_session(&c,&db::accounts::by_id(&c,account).unwrap().unwrap()).unwrap();
   c.execute_batch("INSERT INTO channels(id,name,base_url,enabled) VALUES(1,'active source','http://synthetic.invalid',1),(2,'disabled source','http://synthetic.invalid',0);").unwrap();
   for(attempt,owner,key_id,channel,model,ms,request,input,output,debit)in[
    ("one",user,own.id,1,"alpha",10000,10,Some(10),1,Some(3)),("retry",user,own.id,1,"alpha",11000,10,None,2,None),
    ("historical",user,own.id,2,"beta",15000,11,Some(100),5,Some(9)),("other-key",user,other.id,1,"alpha",16000,12,Some(7),1,Some(4)),
    ("foreign",foreign,foreign_key.id,1,"foreign",17000,13,Some(999),999,Some(999)),("before",user,own.id,1,"alpha",9999,9,Some(500),5,Some(50)),("end",user,own.id,1,"alpha",20000,14,Some(600),6,Some(60))] {
    c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,input_tokens,output_tokens,debit_micro,currency) VALUES(?1,?2,?3,?4,?5,?6,0,0,0,?7,'settled',?8,?9,?10,'CNY')",rusqlite::params![attempt,owner,key_id,request,channel,model,ms,input,output,debit]).unwrap();
   }(admin,cookie,key,user,own.id)
  };Self{state,dir,admin,cookie,key,user,key_id}
 }
 async fn get(&self,path:&str,bearer:Option<&str>,cookie:Option<&str>)->(StatusCode,Value){let mut r=Request::builder().uri(path);if let Some(t)=bearer{r=r.header("authorization",format!("Bearer {t}"));}if let Some(t)=cookie{r=r.header("cookie",format!("tokengo_account_session={t}"));}let r=app(self.state.clone()).oneshot(r.body(Body::empty()).unwrap()).await.unwrap();let status=r.status();let bytes=to_bytes(r.into_body(),4<<20).await.unwrap();(status,serde_json::from_slice(&bytes).unwrap())}
}
fn assert_full_report(v:&Value){assert_eq!(v["summary"]["request_count"],1);assert_eq!(v["summary"]["ledger_count"],2);assert_eq!(v["summary"]["total_tokens"],13);assert_eq!(v["summary"]["unknown_input_entries"],1);assert_eq!(v["summary"]["unknown_debit_entries"],1);assert_eq!(v["summary"]["known_debit_cny_micro"],3);assert_eq!(v["analytics"]["buckets"][0]["total_tokens"],13);assert_eq!(v["analytics"]["models"][0]["ledger_count"],2);assert_eq!(v["analytics"]["model_series"][0]["buckets"][0]["ledger_count"],2);assert!(v.get("entries").is_none());}
#[tokio::test]
async fn report_filters_owner_source_model_time_and_ignores_cursor_without_changing_pages(){
 let f=Fixture::new();let filter=format!("user_id={}&key_id={}&channel_id=1&source_group_id=channel:1&model=alpha&start_at=10&end_at=20&state=active",f.user,f.key_id);
 let(status,page)=f.get(&format!("/api/usage?{filter}&limit=1"),Some(&f.admin),None).await;assert_eq!(status,StatusCode::OK,"{page}");assert_eq!(page["entries"].as_array().unwrap().len(),1);assert!(page.get("summary").is_none()&&page.get("analytics").is_none());let cursor=page["next_before_id"].as_i64().unwrap();
 let(status,v)=f.get(&format!("/api/usage/analytics?{filter}&limit=1&before_id={cursor}"),Some(&f.admin),None).await;assert_eq!(status,StatusCode::OK,"{v}");assert_full_report(&v);
 let(_,all)=f.get(&format!("/api/usage/analytics?{filter}"),Some(&f.admin),None).await;assert_eq!(v,all);
 let(_,historical)=f.get(&format!("/api/usage/analytics?user_id={}&state=historical&start_at=10&end_at=20",f.user),Some(&f.admin),None).await;assert_eq!(historical["summary"]["ledger_count"],1);assert_eq!(historical["analytics"]["models"][0]["model"],"beta");
 for query in ["start_at=20&end_at=10","start_at=-1","source_group_id=channel:invalid","currency=INVALID"]{assert_eq!(f.get(&format!("/api/usage/analytics?{query}"),Some(&f.admin),None).await.0,StatusCode::BAD_REQUEST);}
}
#[tokio::test]
async fn account_and_gateway_reports_force_authenticated_ownership_and_recheck_revocation(){
 let f=Fixture::new();let filter="user_id=999999&key_id=999999&source_group_id=channel:1&model=alpha&start_at=10&end_at=20&limit=1&before_id=1";
 let(status,v)=f.get(&format!("/v1/usage/analytics?{filter}"),Some(&f.key),None).await;assert_eq!(status,StatusCode::OK,"{v}");assert_full_report(&v);
 let(status,page)=f.get(&format!("/v1/usage?{filter}"),Some(&f.key),None).await;assert_eq!(status,StatusCode::OK);assert!(page.get("summary").is_none()&&page.get("analytics").is_none());assert!(page.get("budgets").is_some());
 let filter=format!("user_id=999999&key_id={}&source_group_id=channel:1&model=alpha&start_at=10&end_at=20&limit=1&before_id=1",f.key_id);
 let(status,v)=f.get(&format!("/api/account/usage/analytics?{filter}"),None,Some(&f.cookie)).await;assert_eq!(status,StatusCode::OK,"{v}");assert_full_report(&v);
 let(status,portal)=f.get("/api/account/portal?start_at=10&end_at=20&limit=1",None,Some(&f.cookie)).await;assert_eq!(status,StatusCode::OK);assert_eq!(portal["usage"]["summary"]["ledger_count"],4);assert!(portal["usage"].get("analytics").is_none());
 for path in ["/api/usage/analytics","/api/account/usage/analytics","/v1/usage/analytics"]{assert_eq!(f.get(path,None,None).await.0,StatusCode::UNAUTHORIZED);}
 assert_ne!(f.get("/api/usage/analytics",None,Some(&f.cookie)).await.0,StatusCode::OK);
 f.state.conn().execute("UPDATE users SET enabled=0 WHERE id=?1",[f.user]).unwrap();
 assert_eq!(f.get("/api/account/usage/analytics",None,Some(&f.cookie)).await.0,StatusCode::UNAUTHORIZED);assert_eq!(f.get("/v1/usage/analytics",Some(&f.key),None).await.0,StatusCode::UNAUTHORIZED);
}
