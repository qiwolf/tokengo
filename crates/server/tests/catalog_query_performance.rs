//! Opt-in configuration-cardinality benchmark. Synthetic identities only, no KDF,
//! no upstream traffic, no real credentials. Run in release mode on build-test.
use axum::{body::Body,http::{Request,StatusCode}};
use serde_json::{json,Value};
use std::{collections::BTreeSet,path::PathBuf,time::Instant};
use tokengo_server::{app,db,AppState,Config,SharedState};
use tower::ServiceExt;

struct Fixture {
    state:SharedState,dir:PathBuf,token:String,users:i64,keys:i64,channels:i64,
    active_users:BTreeSet<i64>,active_keys:BTreeSet<i64>,active_channels:BTreeSet<i64>,
}
impl Drop for Fixture {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.dir);}}
fn user_active(id:i64)->bool {id%11!=0&&id%13!=0}
fn key_own_active(id:i64)->bool {id%17!=0&&id%19!=0&&id%23!=0&&id%29!=0}
impl Fixture {
    fn new(scale:i64)->Self {
        let(users,keys,channels)=(56*scale,133*scale,35*scale);
        let providers=9*scale;
        let dir=std::env::temp_dir().join(format!("catalog-cardinality-{scale}-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config{db_path:dir.join("fixture.db"),master_key_path:dir.join("fixture.key").to_string_lossy().into(),..Default::default()}).unwrap();
        let token;
        {
            let c=state.conn();
            // Session fixture bypasses password hashing only; actual HTTP session
            // authentication and directory authorization are exercised below.
            db::sessions::set_panel_hash(&c,"synthetic-not-a-password-login-hash").unwrap();
            token=db::sessions::create_session(&c,None).unwrap();
            c.execute_batch("BEGIN").unwrap();
            let policy=json!({"allowed_models":["fixture model"],"allowed_channels":[1,2],"allow_paid":true,"admin_access":false,"rpm":120,"tpm":30000,"max_concurrent":4}).to_string();
            c.execute("UPDATE users SET name='fixture-user-1',policy_json=?1 WHERE id=1",[&policy]).unwrap();
            for id in 2..=users {
                c.execute("INSERT INTO users(id,name,role,enabled,policy_json,created_at_ms) VALUES(?1,?2,'user',1,?3,0)",rusqlite::params![id,format!("fixture-user-{id}"),policy]).unwrap();
            }
            for id in 1..=users {
                db::accounts::create(&c,&format!("fixture-login-{id}"),&format!("Fixture user {id}"),id,"synthetic-unused-password-hash").unwrap();
                c.execute("INSERT INTO billing_budgets(scope,scope_id,limit_micro) VALUES('user',?1,1000000)",[id]).unwrap();
            }
            let now=db::now_ms();
            for id in 1..=keys {
                let user=(id-1)%users+1;
                let parent=if id>users {Some(user)}else{None};
                c.execute("INSERT INTO gateway_keys(id,name,hint,created_at_ms,key_hash,selector,user_id,enabled,policy_json,parent_key_id) VALUES(?1,?2,'synthetic',0,?3,?4,?5,1,?6,?7)",rusqlite::params![id,format!("fixture-key-{id}"),format!("synthetic-hash-{id}"),format!("fixture-selector-{id}"),user,policy,parent]).unwrap();
                c.execute("INSERT INTO billing_budgets(scope,scope_id,limit_micro) VALUES('key',?1,500000)",[id]).unwrap();
                if id%17==0 {c.execute("UPDATE gateway_keys SET enabled=0 WHERE id=?1",[id]).unwrap();}
                if id%19==0||id%29==0 {c.execute("UPDATE gateway_keys SET revoked_at=?2 WHERE id=?1",rusqlite::params![id,now-3600000]).unwrap();}
                if id%23==0 {c.execute("UPDATE gateway_keys SET expires_at=?2 WHERE id=?1",rusqlite::params![id,now-3600000]).unwrap();}
                if id%29==0 {db::settings::set(&c,&format!("deleted_key:{id}"),"synthetic-deleted").unwrap();}
            }
            for id in 2..=users {
                if id%13==0 {db::lifecycle::archive_user(&c,id,1).unwrap();}
                else if id%11==0 {c.execute("UPDATE users SET enabled=0 WHERE id=?1",[id]).unwrap();}
            }
            for id in 1..=providers {
                c.execute("INSERT INTO provider_channels(id,name,created_ms,enabled) VALUES(?1,?2,0,?3)",rusqlite::params![id,format!("fixture-provider-{id}"),id%5!=0]).unwrap();
            }
            for id in 1..=channels {
                c.execute("INSERT INTO channels(id,name,base_url,enabled,priority) VALUES(?1,?2,'http://fixture.invalid/v1',?3,?1)",rusqlite::params![id,format!("fixture-source-{id}"),id%7!=0]).unwrap();
                db::provider_channels::set_membership(&c,id,Some((id-1)%providers+1)).unwrap();
            }
            // Names differ between snapshots, retaining the latest historical label.
            for id in 1..=5*scale {
                for version in ["old","latest"] {
                    c.execute("INSERT INTO billing_ledger(attempt_id,user_id,key_id,request_id,channel_id,model,input_bound,output_bound,reserved_micro,created_ms,status,source_snapshot) VALUES(?1,1,1,0,?2,'fixture model',0,0,0,0,'settled',?3)",rusqlite::params![format!("history-{id}-{version}"),10000+id,json!({"channel_name":format!("removed-{id}-{version}")}).to_string()]).unwrap();
                }
            }
            c.execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
            assert_eq!(c.query_row("SELECT COUNT(*) FROM users",[],|r|r.get::<_,i64>(0)).unwrap(),users);
            assert_eq!(c.query_row("SELECT COUNT(*) FROM gateway_keys",[],|r|r.get::<_,i64>(0)).unwrap(),keys);
            assert_eq!(c.query_row("SELECT COUNT(*) FROM channels",[],|r|r.get::<_,i64>(0)).unwrap(),channels);
        }
        let active_users=(1..=users).filter(|id|user_active(*id)).collect();
        let active_keys=(1..=keys).filter(|id| {
            let user=(*id-1)%users+1;
            user_active(user)&&key_own_active(*id)&&(*id<=users||key_own_active(user))
        }).collect();
        let active_channels=(1..=channels).filter(|id|*id%7!=0&&((id-1)%providers+1)%5!=0).collect();
        Self{state,dir,token,users,keys,channels,active_users,active_keys,active_channels}
    }
    async fn get3(&self,path:&str)->(Value,[u128;3]) {
        let mut last=Value::Null;let mut timings=[0;3];
        for time in &mut timings {
            let request=Request::builder().uri(path).header("authorization",format!("Bearer {}",self.token)).body(Body::empty()).unwrap();
            let start=Instant::now();
            let response=app(self.state.clone()).oneshot(request).await.unwrap();
            let status=response.status();let body=axum::body::to_bytes(response.into_body(),16<<20).await.unwrap();
            last=serde_json::from_slice(&body).unwrap();*time=start.elapsed().as_micros();
            assert_eq!(status,StatusCode::OK,"{path}: {last}");
        }
        (last,timings)
    }
}
fn timed<T>(mut f:impl FnMut()->T)->(T,[u128;3]) {
    let mut last=None;let mut times=[0;3];
    for time in &mut times {let start=Instant::now();last=Some(f());*time=start.elapsed().as_micros();}
    (last.unwrap(),times)
}
fn report(scale:i64,name:&str,times:[u128;3],count:usize,failures:&mut Vec<String>) {
    let max=*times.iter().max().unwrap();
    eprintln!("CATALOG_PERF scale={scale} query={name} n=3 cardinality={count} samples_us={times:?} max_us={max} current_scale_gate_us=300000 over_300ms={}",max>300000);
    if scale==1&&max>300000 {failures.push(format!("{name}: max {max}us > 300000us"));}
}
fn active_ids(catalog:&Value,kind:&str)->BTreeSet<i64> {
    catalog[kind].as_array().unwrap().iter().filter(|row|row["state"]=="active").map(|row|row["id"].as_i64().unwrap()).collect()
}
fn assert_catalog(f:&Fixture,catalog:&Value,history:bool,scale:i64) {
    assert_eq!(catalog["users"].as_array().unwrap().len(),f.users as usize);
    assert_eq!(catalog["keys"].as_array().unwrap().len(),f.keys as usize);
    assert_eq!(catalog["channels"].as_array().unwrap().len(),(f.channels+if history{5*scale}else{0}) as usize);
    assert_eq!(active_ids(catalog,"users"),f.active_users);
    assert_eq!(active_ids(catalog,"keys"),f.active_keys);
    assert_eq!(active_ids(catalog,"channels"),f.active_channels);
    if history {
        let deleted=catalog["channels"].as_array().unwrap().iter().find(|row|row["id"]==10001).unwrap();
        assert_eq!(deleted["name"],"removed-1-latest");assert_eq!(deleted["state"],"historical");
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="configuration-cardinality release benchmark; run isolated on build-test"]
async fn catalog_directories_current_and_tenfold_scale() {
    let mut failures=Vec::new();
    for scale in [1,10] {
        let f=Fixture::new(scale);
        eprintln!("CATALOG_FIXTURE scale={scale} users={} keys={} channels={} active_users={} active_keys={} active_channels={} historical_channels={} synthetic_no_kdf=true",f.users,f.keys,f.channels,f.active_users.len(),f.active_keys.len(),f.active_channels.len(),5*scale);
        {
            let c=f.state.conn();
            let(catalog,times)=timed(||db::billing_management::current_objects(&c).unwrap());
            report(scale,"current_objects",times,(f.users+f.keys+f.channels) as usize,&mut failures);assert_catalog(&f,&catalog,false,scale);
            let(catalog,times)=timed(||db::billing_management::objects(&c).unwrap());
            report(scale,"objects",times,(f.users+f.keys+f.channels+5*scale) as usize,&mut failures);assert_catalog(&f,&catalog,true,scale);
            for state in ["active","historical","all"] {
                let(rows,times)=timed(||db::billing_management::budgets(&c,None,None,Some(state)).unwrap());
                report(scale,&format!("budgets_{state}"),times,rows.len(),&mut failures);
                let expected=(1..=f.keys).map(|id|("key".to_string(),id)).chain((1..=f.users).map(|id|("user".to_string(),id))).filter(|(scope,id)| {
                    let active=if scope=="key"{f.active_keys.contains(id)}else{f.active_users.contains(id)};
                    state=="all"||active==(state=="active")
                }).take(1000).collect::<Vec<_>>();
                let actual=rows.iter().map(|row|(row["scope"].as_str().unwrap().to_string(),row["scope_id"].as_i64().unwrap())).collect::<Vec<_>>();
                assert_eq!(actual,expected);
            }
            let(users,times)=timed(||db::identity::users(&c).unwrap());
            report(scale,"identity_users",times,users.len(),&mut failures);
            assert_eq!(users.iter().map(|u|u.id).collect::<BTreeSet<_>>(),(1..=f.users).filter(|id|id%13!=0).collect());
            let(keys,times)=timed(||(1..=f.users).flat_map(|user|db::identity::keys(&c,user).unwrap()).collect::<Vec<_>>());
            report(scale,"identity_keys_all_owners_batch",times,keys.len(),&mut failures);
            assert_eq!(keys.iter().map(|key|key.id).collect::<BTreeSet<_>>(),(1..=f.keys).filter(|id|id%29!=0).collect());
            let(contexts,times)=timed(||(1..=f.keys).filter_map(|id|db::identity::context(&c,id).unwrap().map(|ctx|(id,ctx))).collect::<Vec<_>>());
            report(scale,"key_policy_context_all_keys_batch",times,contexts.len(),&mut failures);
            assert_eq!(contexts.iter().map(|(id,_)|*id).collect::<BTreeSet<_>>(),f.active_keys);
        }
        for endpoint in ["/api/managed-users","/api/gateway-keys","/api/users/1/keys"] {
            let(body,times)=f.get3(endpoint).await;
            let kind=if endpoint=="/api/managed-users"{"users"}else{"keys"};
            let rows=body[kind].as_array().unwrap();
            report(scale,endpoint,times,rows.len(),&mut failures);
            let actual=rows.iter().map(|row|row["id"].as_i64().unwrap()).collect::<BTreeSet<_>>();
            let expected=if kind=="users" {(1..=f.users).filter(|id|id%13!=0).collect()}
                else if endpoint=="/api/gateway-keys" {(1..=f.keys).filter(|id|id%29!=0).collect()}
                else {(1..=f.keys).filter(|id|(id-1)%f.users+1==1&&id%29!=0).collect()};
            assert_eq!(actual,expected);
        }
    }
    assert!(failures.is_empty(),"current-cardinality 300ms gate failed: {failures:?}");
    eprintln!("CATALOG_GATE current_scale_passed=true tenfold_policy=record_only threshold_unchanged_us=300000");
}
