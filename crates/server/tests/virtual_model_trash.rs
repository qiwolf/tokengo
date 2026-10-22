//! Recycle-bin API acceptance: authentication, CAS, name reuse, and preserved history.
use axum::{body::Body,http::{Request,StatusCode}};
use serde_json::{json,Value};
use tokengo_server::{app,db,AppState,Config,SharedState};
use tokengo_shared::ModelKind;
use tower::ServiceExt;
struct Fixture {state:SharedState,dir:std::path::PathBuf,root:String,owner:String,other:String,key:String,provider:i64}
impl Drop for Fixture {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.dir);}}
impl Fixture {
    fn new()->Self {
        let dir=std::env::temp_dir().join(format!("virtual-trash-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into_owned(),..Config::default()}).unwrap();
        let(root,owner,other,key,provider)={let c=state.conn();let hash=db::sessions::hash_password("trash-fixture-only-password").unwrap();db::accounts::sync_root_hash(&c,&hash).unwrap();let account=db::accounts::root(&c).unwrap().unwrap();let root=format!("tokengo_account_session={}",db::accounts::create_session(&c,&account).unwrap());
            let mut cookies=Vec::new();let mut key=String::new();for name in ["trash-owner","trash-other"] {let user=db::identity::create_user(&c,name,"user",&Default::default()).unwrap();let account=db::accounts::create(&c,name,name,user.id,&hash).unwrap();let account=db::accounts::by_id(&c,account.id).unwrap().unwrap();cookies.push(format!("tokengo_account_session={}",db::accounts::create_session(&c,&account).unwrap()));if name=="trash-owner"{key=db::identity::create_key(&c,user.id,"caller",&Default::default(),None,None).unwrap().1;}}
            let source=db::channels::create(&c,&db::channels::NewChannel{name:"trash-source".into(),base_url:"http://127.0.0.1:9/v1".into(),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();db::channels::set_secret(&c,&state.secrets,source,"synthetic-upstream").unwrap();db::channels::set_models(&c,source,&[db::channels::ModelMapping::new("trash-bottom",None,false)]).unwrap();db::kinds::set(&c,"trash-bottom",ModelKind::Chat).unwrap();db::provider_pool::backfill(&c,&state.secrets).unwrap();let provider=db::provider_pool::source_credential(&c,source.0).unwrap().unwrap().provider_id;
            (root,cookies.remove(0),cookies.remove(0),key,provider)};
        Self{state,dir,root,owner,other,key,provider}
    }
    fn config(&self,name:&str)->Value{json!({"name":name,"enabled":true,"mode":"ordered","timezone":"UTC","targets":[{"provider_id":self.provider,"logical_model":"trash-bottom","switch_on_429":true,"switch_on_400":false}],"windows":[],"default_positions":[]})}
    fn base(shared:bool)->&'static str{if shared{"/api/shared-virtual-models"}else{"/api/account/virtual-models"}}
    fn cookie(&self,shared:bool)->&str{if shared{&self.root}else{&self.owner}}
    async fn request(&self,method:&str,path:&str,cookie:Option<&str>,csrf:bool,body:Value)->(StatusCode,Value){
        let mut req=Request::builder().method(method).uri(path).header("host","localhost").header("content-type","application/json");if let Some(cookie)=cookie{req=req.header("cookie",cookie);}if csrf{req=req.header("X-Tokengo-Panel","1");}
        let response=app(self.state.clone()).oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();let status=response.status();let bytes=axum::body::to_bytes(response.into_body(),1<<20).await.unwrap();(status,serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    async fn create(&self,shared:bool,name:&str)->Value{let(status,body)=self.request("POST",Self::base(shared),Some(self.cookie(shared)),true,self.config(name)).await;assert_eq!(status,StatusCode::OK,"{body}");body["virtual_model"].clone()}
    async fn mutate(&self,shared:bool,id:i64,action:&str,version:Value)->(StatusCode,Value){self.request("POST",&format!("{}/{id}/{action}",Self::base(shared)),Some(self.cookie(shared)),true,json!({"version":version})).await}
    async fn list(&self,shared:bool,trash:bool)->Value{let path=format!("{}{}",Self::base(shared),if trash{"?trash=true"}else{""});let(status,body)=self.request("GET",&path,Some(self.cookie(shared)),false,Value::Null).await;assert_eq!(status,StatusCode::OK,"{body}");body["virtual_models"].clone()}
    async fn cannot_call(&self,name:&str){let req=Request::builder().method("POST").uri("/v1/chat/completions").header("content-type","application/json").header("authorization",format!("Bearer {}",self.key)).body(Body::from(json!({"model":name,"messages":[{"role":"user","content":"test deleted alias"}],"max_tokens":8}).to_string())).unwrap();let response=app(self.state.clone()).oneshot(req).await.unwrap();assert!(!response.status().is_success());let c=self.state.conn();let attempts:i64=c.query_row("SELECT count(*) FROM attempts",[],|r|r.get(0)).unwrap();assert_eq!(attempts,0,"trashed or restored-disabled aliases cannot reach upstream");}
}

#[tokio::test]
async fn private_and_shared_trash_restore_preserve_history_and_restore_disabled(){
    for shared in [false,true]{let f=Fixture::new();let saved=f.create(shared,"recyclable-entry").await;let id=saved["id"].as_i64().unwrap();assert!(saved["deleted_at_ms"].is_null());
        let(status,body)=f.mutate(shared,id,"trash",saved["version"].clone()).await;assert_eq!(status,StatusCode::OK,"{body}");let deleted=body["virtual_model"].clone();assert_eq!(deleted["enabled"],false);assert!(deleted["deleted_at_ms"].as_i64().unwrap()>0);assert_eq!(deleted["name"],"recyclable-entry");assert_eq!(deleted["version"],2);
        assert_eq!(f.list(shared,false).await.as_array().unwrap().len(),0);let trash=f.list(shared,true).await;assert_eq!(trash.as_array().unwrap().len(),1);assert_eq!(trash[0]["id"],id);f.cannot_call("recyclable-entry").await;
        let(status,body)=f.mutate(shared,id,"restore",deleted["version"].clone()).await;assert_eq!(status,StatusCode::OK,"{body}");let restored=&body["virtual_model"];assert_eq!(restored["enabled"],false);assert!(restored["deleted_at_ms"].is_null());assert_eq!(restored["version"],3);assert_eq!(f.list(shared,false).await.as_array().unwrap().len(),1);assert_eq!(f.list(shared,true).await.as_array().unwrap().len(),0);f.cannot_call("recyclable-entry").await;
        let c=f.state.conn();let action=if shared{"shared_virtual_model"}else{"virtual_model"};for suffix in ["create","trash","restore"]{let count:i64=c.query_row("SELECT count(*) FROM audit_events WHERE action=?1 AND target_id=?2",rusqlite::params![format!("{action}.{suffix}"),id],|r|r.get(0)).unwrap();assert_eq!(count,1);}
    }
}

#[tokio::test]
async fn names_are_reusable_and_restore_conflicts_never_overwrite_new_entry(){
    for shared in [false,true]{let f=Fixture::new();let saved=f.create(shared,"reused-name").await;let old=saved["id"].as_i64().unwrap();let(status,deleted)=f.mutate(shared,old,"trash",saved["version"].clone()).await;assert_eq!(status,StatusCode::OK,"{deleted}");let fresh=f.create(shared,"reused-name").await;assert_ne!(fresh["id"],old);
        let(status,body)=f.mutate(shared,old,"restore",deleted["virtual_model"]["version"].clone()).await;assert_eq!(status,StatusCode::CONFLICT,"{body}");let active=f.list(shared,false).await;assert_eq!(active.as_array().unwrap().len(),1);assert_eq!(active[0]["id"],fresh["id"]);assert_eq!(active[0]["enabled"],true);assert_eq!(f.list(shared,true).await[0]["id"],old);
    }
}

#[tokio::test]
async fn trash_and_restore_require_csrf_owner_root_and_current_version(){
    for shared in [false,true]{let f=Fixture::new();let saved=f.create(shared,"guarded-entry").await;let id=saved["id"].as_i64().unwrap();let path=format!("{}/{id}/trash",Fixture::base(shared));
        assert_eq!(f.request("POST",&path,Some(f.cookie(shared)),false,json!({"version":1})).await.0,StatusCode::FORBIDDEN);
        assert!(matches!(f.request("POST",&path,None,true,json!({"version":1})).await.0,StatusCode::UNAUTHORIZED|StatusCode::FORBIDDEN));
        let wrong=if shared{&f.owner}else{&f.other};assert_eq!(f.request("POST",&path,Some(wrong),true,json!({"version":1})).await.0,if shared{StatusCode::FORBIDDEN}else{StatusCode::NOT_FOUND});
        assert_eq!(f.mutate(shared,id,"trash",json!(99)).await.0,StatusCode::CONFLICT);assert_eq!(f.list(shared,false).await.as_array().unwrap().len(),1);
        let(status,deleted)=f.mutate(shared,id,"trash",json!(1)).await;assert_eq!(status,StatusCode::OK,"{deleted}");assert_eq!(f.mutate(shared,id,"restore",json!(1)).await.0,StatusCode::CONFLICT);
        let restore=format!("{}/{id}/restore",Fixture::base(shared));assert_eq!(f.request("POST",&restore,Some(wrong),true,json!({"version":2})).await.0,if shared{StatusCode::FORBIDDEN}else{StatusCode::NOT_FOUND});assert_eq!(f.request("POST",&restore,Some(f.cookie(shared)),false,json!({"version":2})).await.0,StatusCode::FORBIDDEN);
        assert_eq!(f.mutate(shared,id,"restore",json!(2)).await.0,StatusCode::OK);
    }
}

#[tokio::test]
async fn legacy_delete_only_disables_and_trashed_rows_reject_edit_and_disable(){
    for shared in [false,true]{let f=Fixture::new();let saved=f.create(shared,"legacy-entry").await;let id=saved["id"].as_i64().unwrap();let path=format!("{}/{id}",Fixture::base(shared));let(status,body)=f.request("DELETE",&path,Some(f.cookie(shared)),true,json!({"version":1})).await;assert_eq!(status,StatusCode::OK,"{body}");assert!(body["virtual_model"]["deleted_at_ms"].is_null());assert_eq!(f.list(shared,false).await.as_array().unwrap().len(),1);assert_eq!(f.list(shared,true).await.as_array().unwrap().len(),0);
        assert_eq!(f.mutate(shared,id,"trash",json!(2)).await.0,StatusCode::OK);assert_eq!(f.request("DELETE",&path,Some(f.cookie(shared)),true,json!({"version":3})).await.0,StatusCode::NOT_FOUND);let mut edit=f.config("legacy-entry");edit["version"]=json!(3);assert_eq!(f.request("PATCH",&path,Some(f.cookie(shared)),true,edit).await.0,StatusCode::NOT_FOUND);
    }
}
