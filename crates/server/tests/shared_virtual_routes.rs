//! Public shared-route API acceptance with real loopback HTTP and source-side observations.
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokengo_server::{
    app,
    db::{self, user_virtual_routes as vr},
    AppState, Config, SharedState,
};
use tokengo_shared::{ChannelId, ModelKind};
use tower::ServiceExt;

#[derive(Clone, Copy, Default)]
enum Behavior {
    #[default]
    Good,
    Status(u16),
    Dimension,
}
#[derive(Default)]
struct UpstreamState {
    behavior: Mutex<Behavior>,
    seen: Mutex<Vec<Value>>,
    hold: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}
struct Upstream {
    state: Arc<UpstreamState>,
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Upstream {
    async fn start() -> Self {
        let state = Arc::new(UpstreamState::default());
        let router = Router::new()
            .route("/v1/chat/completions", post(upstream))
            .route("/v1/messages", post(upstream))
            .route("/v1/responses", post(upstream))
            .route("/v1/tools/search", post(upstream))
            .route("/v1/tools/fetch", post(upstream))
            .route("/v1/embeddings", post(upstream))
            .route("/v1/rerank", post(upstream))
            .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { state, base, task }
    }
    fn set(&self, value: Behavior) {
        *self.state.behavior.lock().unwrap() = value;
    }
    fn clear(&self) {
        self.state.seen.lock().unwrap().clear();
    }
    fn count(&self) -> usize {
        self.state.seen.lock().unwrap().len()
    }
}
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |v, b| {
        (v ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
fn embedding(text: &str, behavior: Behavior) -> Vec<f64> {
    let mut seed = hash(text);
    let n = if matches!(behavior, Behavior::Dimension) {
        7
    } else {
        8
    };
    let values: Vec<f64> = (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed % 10001) as f64 - 5000.0) / 5000.0
        })
        .collect();
    let norm = values.iter().map(|v| v * v).sum::<f64>().sqrt();
    values.into_iter().map(|v| v / norm).collect()
}
async fn upstream(
    State(state): State<Arc<UpstreamState>>,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    state.seen.lock().unwrap().push(body.clone());
    let behavior = *state.behavior.lock().unwrap();
    let hold = state.hold.lock().unwrap().clone();
    if let Some((entered, release)) = hold {
        entered.notify_one();
        release.notified().await;
    }
    if let Behavior::Status(code) = behavior {
        return (
            StatusCode::from_u16(code).unwrap(),
            Json(json!({"error":{"message":"controlled failure"}})),
        )
            .into_response();
    }
    if uri.path().ends_with("/messages") {
        return Json(json!({"id":"msg_shared_fixture","type":"message","role":"assistant","model":body["model"],"content":[{"type":"text","text":"shared native message"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":1}})).into_response();
    }
    if uri.path().ends_with("/responses") {
        return Json(json!({"id":"resp_shared_fixture","object":"response","status":"completed","model":body["model"],"output":[{"id":"msg_shared_fixture","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"shared native response","annotations":[]}]}],"usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}})).into_response();
    }
    if uri.path().ends_with("tools/search") {
        return Json(json!({"results":[{"title":"Synthetic shared result","url":"https://example.com","snippet":"s","summary":"s","content":"evidence","rerank_score":2.5,"authority":0,"site_name":null,"site_icon":null,"date":null}]})).into_response();
    }
    if uri.path().ends_with("tools/fetch") {
        return Json(json!({"title":"Synthetic shared page","content":"shared evidence"})).into_response();
    }
    if uri.path().ends_with("chat/completions") {
        return Json(json!({"choices":[{"message":{"role":"assistant","content":"shared upstream response"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}})).into_response();
    }
    if uri.path().ends_with("embeddings") {
        let input: Vec<&str> = if let Some(s) = body["input"].as_str() {
            vec![s]
        } else {
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect()
        };
        let data: Vec<_> = input.iter().enumerate().map(|(i,text)| json!({
            "object":"embedding", "index":i,
            "embedding":embedding(text, behavior)
        })).collect();
        return Json(json!({"object":"list","model":body["model"],"data":data,"usage":{"prompt_tokens":3,"total_tokens":3}})).into_response();
    }
    let docs = body["documents"].as_array().unwrap();
    let query = body["query"].as_str().unwrap_or("");
    let mut rows: Vec<(usize, f64)> = docs
        .iter()
        .enumerate()
        .map(|(i, doc)| {
            let text = doc.as_str().or_else(|| doc["text"].as_str()).unwrap_or("");
            (
                i,
                (hash(&format!("{query}:{text}")) % 1_000_000) as f64 / 1_000_000.0,
            )
        })
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    let take = body["top_n"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(docs.len());
    let results: Vec<_> = rows.into_iter().take(take).map(|(i,score)| {
        let mut row = json!({"index":i,"relevance_score":score});
        if body["return_documents"].as_bool().unwrap_or(true) {
            row["document"] = if docs[i].is_string() {json!({"text":docs[i]})} else {docs[i].clone()};
        }
        row
    }).collect();
    Json(json!({"results":results,"usage":{"prompt_tokens":3,"total_tokens":3}})).into_response()
}
struct Fixture {
    state: SharedState,
    dir: std::path::PathBuf,
    user: i64,
    key_id: i64,
    key: String,
    root: String,
    config: vr::Config,
    sources: Vec<ChannelId>,
    upstreams: Vec<Upstream>,
    kind: ModelKind,
}
impl Drop for Fixture { fn drop(&mut self) { let _=std::fs::remove_dir_all(&self.dir); } }
impl Fixture {
    async fn new(kind: ModelKind) -> Self {
        let dir=std::env::temp_dir().join(format!("shared-virtual-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into_owned(),..Config::default()}).unwrap();
        let (user,key_id,key,root)={
            let c=state.conn();
            let user=db::identity::create_user(&c,"shared-caller","user",&Default::default()).unwrap().id;
            let (key,secret)=db::identity::create_key(&c,user,"caller-key",&Default::default(),None,None).unwrap();
            db::sessions::set_panel_password(&c,"shared-root-fixture-password").unwrap();
            let root_account=db::accounts::root(&c).unwrap().unwrap();
            let token=db::accounts::create_session(&c,&root_account).unwrap();
            let root=format!("tokengo_account_session={token}");
            (user,key.id,secret,root)
        };
        if matches!(kind,ModelKind::Embeddings|ModelKind::Rerank) {
            db::billing::set_budget(&state.conn(),"user",user,Some(1_000_000_000),"fixture-full-workload").unwrap();
        }
        let mut sources=vec![];let mut upstreams=vec![];let mut targets=vec![];
        for i in 0..2 {
            let upstream=Upstream::start().await;
            let c=state.conn();let name=format!("bottom-{i}");
            let source=db::channels::create(&c,&db::channels::NewChannel{name:name.clone(),base_url:upstream.base.clone(),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
            db::channels::set_secret(&c,&state.secrets,source,"synthetic-shared-upstream").unwrap();
            db::channels::set_models(&c,source,&[db::channels::ModelMapping::new(&name,None,false)]).unwrap();
            db::kinds::set(&c,&name,kind).unwrap();
            db::provider_pool::backfill(&c,&state.secrets).unwrap();
            let provider=db::provider_pool::source_credential(&c,source.0).unwrap().unwrap().provider_id;
            db::billing::create_price(&c,&db::billing::Price{id:0,channel_id:source.0,model:name.clone(),input_micro_per_million:1_000_000,output_micro_per_million:1_000_000,max_input_tokens:if matches!(kind,ModelKind::Embeddings|ModelKind::Rerank){8_000_000}else{50000},max_output_tokens:1024},"fixture").unwrap();
            targets.push(vr::Target{provider_id:provider,logical_model:name,switch_on_429:true,switch_on_400:false});
            sources.push(source);upstreams.push(upstream);
        }
        state.routing().cfg.retry_times=0;
        let config=vr::Config{name:"shared-model".into(),enabled:true,mode:"ordered".into(),timezone:"UTC".into(),targets,windows:vec![],default_positions:vec![]};
        Self{state,dir,user,key_id,key,root,config,sources,upstreams,kind}
    }
    async fn request(&self,method:&str,path:&str,bearer:Option<&str>,cookie:Option<&str>,body:Value)->(StatusCode,Value) {
        let mut request=Request::builder().method(method).uri(path).header("host","localhost").header("content-type","application/json").header("X-Tokengo-Panel","1");
        if let Some(token)=bearer {request=request.header("authorization",format!("Bearer {token}"));}
        if let Some(cookie)=cookie {request=request.header("cookie",cookie);}
        let response=app(self.state.clone()).oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap();
        let status=response.status();let bytes=axum::body::to_bytes(response.into_body(),1<<20).await.unwrap();
        (status,serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    async fn admin(&self,method:&str,path:&str,body:Value)->(StatusCode,Value) {self.request(method,path,None,Some(&self.root),body).await}
    fn body(&self,name:&str)->Value {match self.kind {
        ModelKind::Chat=>json!({"model":name,"messages":[{"role":"user","content":"shared model test"}],"max_tokens":8}),
        ModelKind::Embeddings=>json!({"model":name,"input":["hospital medicine","patient safety"]}),
        _=>json!({"model":name,"query":"hospital","documents":["clinical medicine","a train","patient safety"],"top_n":2,"return_documents":true}),
    }}
    async fn call_key(&self,name:&str,key:&str)->(StatusCode,Value) {
        let path=match self.kind {ModelKind::Chat=>"/v1/chat/completions",ModelKind::Embeddings=>"/v1/embeddings",_=>"/v1/rerank"};
        self.request("POST",path,Some(key),None,self.body(name)).await
    }
    async fn call(&self,name:&str)->(StatusCode,Value) {self.call_key(name,&self.key).await}
    fn clear(&self) {for upstream in &self.upstreams {upstream.clear();}}
    fn count(&self)->usize {self.upstreams.iter().map(Upstream::count).sum()}
    async fn set_internal(&self,name:&str) {
        let (status,body)=self.admin("PUT",&format!("/api/model-access?model={name}"),json!({"internal_only":true})).await;
        assert_eq!(status,StatusCode::OK,"set internal {name}: {body}");
    }
    async fn publish(&self)->Value {
        let (status,body)=self.admin("POST","/api/shared-virtual-models",serde_json::to_value(&self.config).unwrap()).await;
        assert_eq!(status,StatusCode::OK,"publish {:?}: {body}",self.kind);
        assert!(body["virtual_model"]["id"].as_i64().is_some());
        body["virtual_model"].clone()
    }
    fn allow_alias_only(&self) {
        let c=self.state.conn();let mut user=db::identity::user(&c,self.user).unwrap().unwrap();
        user.policy.allowed_models=Some(vec![self.config.name.clone()]);
        user.policy.allowed_channels=Some(self.sources.iter().map(|id|id.0).collect());
        db::identity::update_user(&c,&user).unwrap();
    }
    fn cookie(&self)->String {
        let c=self.state.conn();let hash=db::sessions::hash_password("shared-route-test-password").unwrap();
        let account=db::accounts::create(&c,"shared-caller","Shared caller",self.user,&hash).unwrap();
        let credential=db::accounts::by_id(&c,account.id).unwrap().unwrap();
        format!("tokengo_account_session={}",db::accounts::create_session(&c,&credential).unwrap())
    }
    async fn flush(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(10),async {
            loop {
                self.state.drain_writes();
                let reserved:i64=self.state.conn().query_row("SELECT count(*) FROM billing_ledger WHERE status='reserved'",[],|r|r.get(0)).unwrap();
                if self.state.pending_writes()==0&&reserved==0 {break;}
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("diagnostic and financial ledgers must settle");
    }
}

#[tokio::test]
async fn shared_route_root_crud_is_versioned_and_not_available_to_ordinary_users() {
    let f=Fixture::new(ModelKind::Chat).await;
    let cookie=f.cookie();
    for (method,path,body) in [("GET","/api/shared-virtual-models",Value::Null),("POST","/api/shared-virtual-models",serde_json::to_value(&f.config).unwrap()),("PUT","/api/model-access?model=bottom-0",json!({"internal_only":true}))] {
        let (status,body)=f.request(method,path,None,Some(&cookie),body).await;
        assert_eq!(status,StatusCode::FORBIDDEN,"ordinary browser {method} {path}: {body}");
    }
    assert_eq!(f.count(),0);
    let saved=f.publish().await;let id=saved["id"].as_i64().unwrap();
    let (status,list)=f.admin("GET","/api/shared-virtual-models",Value::Null).await;
    assert_eq!(status,StatusCode::OK,"{list}");
    assert_eq!(list["virtual_models"].as_array().unwrap().len(),1);
    assert!(list["options"].as_array().unwrap().len()>=2);
    let mut edit=serde_json::to_value(&f.config).unwrap();edit["version"]=saved["version"].clone();
    edit["targets"][0]["switch_on_400"]=json!(true);
    let path=format!("/api/shared-virtual-models/{id}");
    let (status,updated)=f.admin("PATCH",&path,edit.clone()).await;
    assert_eq!(status,StatusCode::OK,"{updated}");
    assert_eq!(f.admin("PATCH",&path,edit).await.0,StatusCode::CONFLICT);
    let version=updated["virtual_model"]["version"].clone();
    let (status,body)=f.admin("DELETE",&path,json!({"version":version})).await;
    assert_eq!(status,StatusCode::OK,"{body}");
    assert_eq!(body["virtual_model"]["enabled"],false);
    assert!(!f.call("shared-model").await.0.is_success());
}

#[tokio::test]
async fn shared_alias_only_acl_lists_and_calls_all_supported_kinds_with_caller_billing() {
    for kind in [ModelKind::Chat,ModelKind::Embeddings,ModelKind::Rerank] {
        let f=Fixture::new(kind).await;
        for name in ["bottom-0","bottom-1"] {f.set_internal(name).await;}
        f.publish().await;f.allow_alias_only();f.clear();
        let (status,models)=f.request("GET","/v1/models",Some(&f.key),None,Value::Null).await;
        assert_eq!(status,StatusCode::OK,"{models}");
        let ids:Vec<_>=models["data"].as_array().unwrap().iter().filter_map(|row|row["id"].as_str()).collect();
        assert!(ids.contains(&"shared-model"),"{models}");
        assert!(!ids.contains(&"bottom-0")&&!ids.contains(&"bottom-1"),"{models}");
        f.upstreams[0].set(Behavior::Status(503));
        let (status,body)=f.call("shared-model").await;
        assert_eq!(status,StatusCode::OK,"{kind:?}: {body}");
        assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),1);
        f.flush().await;
        let c=f.state.conn();
        let (user,key,model):(i64,i64,String)=c.query_row("SELECT user_id,key_id,model FROM requests WHERE endpoint!='' ORDER BY id DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!((user,key,model),(f.user,f.key_id,"shared-model".into()));
        let rows:Vec<(i64,i64,i64,Option<i64>)>={let mut q=c.prepare("SELECT user_id,key_id,channel_id,settled_ms FROM billing_ledger WHERE request_id>0 ORDER BY id").unwrap();let rows=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().collect::<Result<_,_>>().unwrap();rows};
        assert_eq!(rows.len(),2);
        for (i,(user,key,source,settled)) in rows.iter().enumerate() {assert_eq!((*user,*key,*source),(f.user,f.key_id,f.sources[i].0));assert!(settled.is_some());}
        if kind!=ModelKind::Chat {
            let mut q=c.prepare("SELECT user_id,key_id,auth_snapshot,settled_ms FROM billing_ledger WHERE request_id=0").unwrap();
            let probes:Vec<(i64,i64,String,Option<i64>)>=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().collect::<Result<_,_>>().unwrap();
            assert!(!probes.is_empty());
            for (user,key,raw,settled) in probes {assert_eq!((user,key),(0,0));assert!(settled.is_some());let auth:Value=serde_json::from_str(&raw).unwrap();assert_eq!(auth["kind"],"admin_diagnostic");}
        }
    }
}

#[tokio::test]
async fn internal_models_cannot_be_called_directly_or_through_auto_and_private_alias() {
    let f=Fixture::new(ModelKind::Chat).await;
    let mut private=f.config.clone();private.name="private-bypass".into();private.targets.truncate(1);
    vr::create(&f.state.conn(),f.user,&private).unwrap();
    for name in ["bottom-0","bottom-1"] {f.set_internal(name).await;}
    f.publish().await;f.clear();
    // The caller intentionally has allow-all model ACL here: internal-only is a separate boundary.
    for name in ["bottom-0","bottom-1","auto","private-bypass"] {
        let (status,body)=f.call(name).await;
        assert!(!status.is_success(),"{name} bypassed internal-only: {body}");
        if name.starts_with("bottom-") {assert_eq!(status,StatusCode::FORBIDDEN);}
    }
    assert_eq!(f.count(),0);
    assert_eq!(f.call("shared-model").await.0,StatusCode::OK);
}

#[tokio::test]
async fn shared_alias_preserves_key_ancestor_channel_and_paid_constraints() {
    for denial in ["key_alias","ancestor_alias","channel","paid"] {
        let f=Fixture::new(ModelKind::Chat).await;f.publish().await;f.allow_alias_only();
        let secret={
            let c=f.state.conn();
            match denial {
                "key_alias"=>{let mut key=db::identity::key(&c,f.key_id).unwrap().unwrap();key.policy.allowed_models=Some(vec!["another-model".into()]);db::identity::update_key(&c,&key).unwrap();f.key.clone()},
                "ancestor_alias"=>{
                    let (parent,_)=db::identity::create_key(&c,f.user,"parent",&Default::default(),None,None).unwrap();
                    let child_policy=db::identity::context(&c,parent.id).unwrap().unwrap().effective_policy();
                    let (child,secret)=db::identity::create_key(&c,f.user,"child",&child_policy,None,Some(parent.id)).unwrap();
                    assert!(db::identity::context(&c,child.id).unwrap().unwrap().permits_model("shared-model"),"child initially has the inherited alias grant");
                    let mut parent=db::identity::key(&c,parent.id).unwrap().unwrap();parent.policy.allowed_models=Some(vec!["another-model".into()]);db::identity::update_key(&c,&parent).unwrap();secret
                },
                _=>{let mut user=db::identity::user(&c,f.user).unwrap().unwrap();if denial=="channel" {user.policy.allowed_channels=Some(vec![]);}else{user.policy.allow_paid=false;}db::identity::update_user(&c,&user).unwrap();f.key.clone()},
            }
        };
        f.clear();let (status,body)=f.call_key("shared-model",&secret).await;
        assert!(!status.is_success(),"{denial}: {body}");assert_eq!(f.count(),0);
    }
}

#[tokio::test]
async fn shared_disable_and_global_bottom_disable_take_effect_without_deleting_routes() {
    for shared_disable in [true,false] {
        let f=Fixture::new(ModelKind::Chat).await;let saved=f.publish().await;f.allow_alias_only();
        if shared_disable {let path=format!("/api/shared-virtual-models/{}",saved["id"].as_i64().unwrap());assert_eq!(f.admin("DELETE",&path,json!({"version":saved["version"]})).await.0,StatusCode::OK);}
        else {let c=f.state.conn();for name in ["bottom-0","bottom-1"] {db::model_routes::set_enabled(&c,name,false).unwrap();}}
        f.clear();assert!(!f.call("shared-model").await.0.is_success());assert_eq!(f.count(),0);
    }
}

#[tokio::test]
async fn in_flight_shared_route_rechecks_caller_key_acl_budget_and_admin_version() {
    for change in ["user_disabled","key_revoked","channel_revoked","frozen","route_version","bottom_disabled"] {
        let f=Fixture::new(ModelKind::Chat).await;let saved=f.publish().await;f.allow_alias_only();f.clear();
        let entered=Arc::new(tokio::sync::Notify::new());let release=Arc::new(tokio::sync::Notify::new());
        *f.upstreams[0].state.hold.lock().unwrap()=Some((entered.clone(),release.clone()));
        f.upstreams[0].set(Behavior::Status(503));
        let mutate=async {
            tokio::time::timeout(std::time::Duration::from_secs(5),entered.notified()).await.unwrap();
            if change=="route_version" {
                let mut edit=serde_json::to_value(&f.config).unwrap();edit["version"]=saved["version"].clone();edit["targets"][0]["switch_on_400"]=json!(true);
                let path=format!("/api/shared-virtual-models/{}",saved["id"].as_i64().unwrap());
                let (status,body)=f.admin("PATCH",&path,edit).await;assert_eq!(status,StatusCode::OK,"{body}");
            }else {
                let c=f.state.conn();match change {
                    "user_disabled"=>{let mut user=db::identity::user(&c,f.user).unwrap().unwrap();user.enabled=false;db::identity::update_user(&c,&user).unwrap();},
                    "key_revoked"=>{db::identity::revoke(&c,f.key_id).unwrap();},
                    "channel_revoked"=>{let mut user=db::identity::user(&c,f.user).unwrap().unwrap();user.policy.allowed_channels=Some(vec![f.sources[0].0]);db::identity::update_user(&c,&user).unwrap();},
                    "frozen"=>{db::billing::set_budget(&c,"user",f.user,None,"test").unwrap();c.execute("UPDATE billing_budgets SET frozen=1 WHERE scope='user' AND scope_id=?1",[f.user]).unwrap();},
                    _=>{db::model_routes::set_enabled(&c,"bottom-1",false).unwrap();},
                }
            }
            release.notify_one();
        };
        let ((status,body),())=tokio::join!(f.call("shared-model"),mutate);
        assert!(!status.is_success(),"{change}: {body}");assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),0,"{change} must be rechecked before backup");
    }
}

#[tokio::test]
async fn incompatible_shared_vectors_are_rejected_before_publishing() {
    let f=Fixture::new(ModelKind::Embeddings).await;f.upstreams[1].set(Behavior::Dimension);
    let (status,body)=f.admin("POST","/api/shared-virtual-models",serde_json::to_value(&f.config).unwrap()).await;
    assert!(!status.is_success(),"{body}");
    let (status,list)=f.admin("GET","/api/shared-virtual-models",Value::Null).await;
    assert_eq!(status,StatusCode::OK,"{list}");assert!(list["virtual_models"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn admin_compatibility_probe_does_not_borrow_callers_budget() {
    for kind in [ModelKind::Embeddings,ModelKind::Rerank] {
        let f=Fixture::new(kind).await;f.allow_alias_only();
        db::billing::set_budget(&f.state.conn(),"user",f.user,Some(0),"test").unwrap();
        f.publish().await;assert!(f.count()>0);f.clear();
        let (status,body)=f.call("shared-model").await;assert!(!status.is_success(),"{body}");assert_eq!(f.count(),0);
        let c=f.state.conn();let budget=db::billing::budgets(&c,Some("user"),Some(f.user)).unwrap().remove(0);
        assert_eq!((budget.spent_micro,budget.reserved_micro),(0,0));
        assert_eq!(c.query_row("SELECT count(*) FROM billing_ledger WHERE request_id=0 AND user_id!=0",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }
}

#[tokio::test]
async fn root_can_list_shared_virtual_models_over_http() {
    let f=Fixture::new(ModelKind::Chat).await;
    let (status,body)=f.admin("GET","/api/shared-virtual-models",Value::Null).await;
    assert_eq!(status,StatusCode::OK,"{body}");
    assert!(body["virtual_models"].is_array());
    assert!(body["options"].is_array());
}

#[tokio::test]
async fn shared_chat_explicit_400_and_429_switches_still_control_fallback() {
    for (code, allow) in [(400, false), (400, true), (429, false), (429, true)] {
        let mut f = Fixture::new(ModelKind::Chat).await;
        f.config.targets[0].switch_on_400 = allow;
        f.config.targets[0].switch_on_429 = allow;
        f.publish().await;
        f.allow_alias_only();
        f.clear();
        f.upstreams[0].set(Behavior::Status(code));
        let (status, body) = f.call("shared-model").await;
        assert_eq!(status.is_success(), allow, "HTTP {code}, allow={allow}: {body}");
        assert_eq!(f.upstreams[0].count(), 1);
        assert_eq!(f.upstreams[1].count(), usize::from(allow));
    }
}


#[tokio::test]
async fn queued_single_target_shared_chat_rejects_changed_model_kind_before_outbound() {
    let mut f=Fixture::new(ModelKind::Chat).await;
    f.config.targets.truncate(1);
    db::channels::update(&f.state.conn(),f.sources[0],"queued source",&f.upstreams[0].base,true,false,1).unwrap();
    f.publish().await;f.allow_alias_only();f.clear();
    let gate=f.state.channel_gate(f.sources[0].0,1).unwrap();
    let permit=gate.try_acquire_owned().unwrap();
    let mutate=async {
        // A running request row is created only after the route/target snapshot loads.
        tokio::time::timeout(std::time::Duration::from_secs(5),async {
            loop {
                let running:i64=f.state.conn().query_row("SELECT count(*) FROM requests WHERE status='running'",[],|r|r.get(0)).unwrap();
                if running>0 {break;}
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("request must load its route and wait behind the held source gate");
        db::kinds::set(&f.state.conn(),"bottom-0",ModelKind::Embeddings).unwrap();
        drop(permit);
    };
    let ((status,body),())=tokio::join!(f.call("shared-model"),mutate);
    assert!(!status.is_success(),"queued kind mutation was ignored: {body}");
    assert_eq!(f.count(),0,"a queued Chat request must not reach a source whose model is now Embeddings");
}

#[tokio::test]
async fn stateless_messages_and_responses_use_shared_fallback_and_respect_internal_boundary() {
    for path in ["/v1/messages","/v1/responses"] {
        let f=Fixture::new(ModelKind::Chat).await;
        {
            let c=f.state.conn();
            for source in &f.sources {
                tokengo_server::protocol_config::save(&c,source.0,&tokengo_server::protocol_config::Capabilities {
                    native:vec![tokengo_server::protocol::Protocol::Chat,tokengo_server::protocol::Protocol::Messages,tokengo_server::protocol::Protocol::Responses],
                    ..Default::default()
                }).unwrap();
            }
        }
        for name in ["bottom-0","bottom-1"] {f.set_internal(name).await;}
        f.publish().await;f.allow_alias_only();f.clear();
        let body=if path.ends_with("messages") {json!({"model":"shared-model","messages":[{"role":"user","content":"stateless hello"}],"max_tokens":8})}
            else {json!({"model":"shared-model","input":"stateless hello","max_output_tokens":8,"store":false})};
        let (status,reply)=f.request("POST",path,Some(&f.key),None,body.clone()).await;
        assert_eq!(status,StatusCode::OK,"{path}: {reply}");
        assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),0);
        f.clear();f.upstreams[0].set(Behavior::Status(503));
        let (status,reply)=f.request("POST",path,Some(&f.key),None,body.clone()).await;
        assert_eq!(status,StatusCode::OK,"{path} fallback: {reply}");
        assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),1);
        {
            let c=f.state.conn();let mut user=db::identity::user(&c,f.user).unwrap().unwrap();user.policy.allowed_models=None;db::identity::update_user(&c,&user).unwrap();
        }
        f.clear();let mut direct=body;direct["model"]=json!("bottom-0");
        let (status,reply)=f.request("POST",path,Some(&f.key),None,direct).await;
        assert_eq!(status,StatusCode::FORBIDDEN,"{path} internal direct: {reply}");assert_eq!(f.count(),0);
    }
}

#[tokio::test]
async fn shared_standalone_tools_require_explicit_capabilities_and_preserve_request_billing() {
    for path in ["/v1/tools/search","/v1/tools/fetch"] {
        for declared in [true,false] {
            let mut f=Fixture::new(ModelKind::Unknown).await;
            // Unknown tool routes may chain providers of the same explicit logical model.
            f.config.targets[1].logical_model="bottom-0".into();
            {
                let c=f.state.conn();
                db::channels::set_models(&c,f.sources[1],&[db::channels::ModelMapping::new("bottom-0",None,false)]).unwrap();
                for source in &f.sources {
                    if declared {tokengo_server::search_tools::save(&c,source.0,&tokengo_server::search_tools::ToolCapabilities {
                        search:true,fetch:true,account_scope:format!("shared-tool-source-{}",source.0),qps:100,
                    }).unwrap();}
                    db::billing::create_request_price(&c,&db::billing::RequestPrice{id:0,channel_id:source.0,model:"bottom-0".into(),micro_per_call:7},"fixture").unwrap();
                }
            }
            f.set_internal("bottom-0").await;f.publish().await;f.allow_alias_only();f.clear();
            let body=if path.ends_with("search") {json!({"model":"shared-model","query":"medical evidence","count":1})}
                else {json!({"model":"shared-model","url":"https://example.com/evidence"})};
            let (status,reply)=f.request("POST",path,Some(&f.key),None,body.clone()).await;
            assert_eq!(status.is_success(),declared,"{path}, declared={declared}: {reply}");
            if !declared {assert_eq!(f.count(),0);continue;}
            assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),0);
            f.clear();f.upstreams[0].set(Behavior::Status(503));
            let (status,reply)=f.request("POST",path,Some(&f.key),None,body.clone()).await;
            assert_eq!(status,StatusCode::OK,"{path} fallback: {reply}");
            assert_eq!(f.upstreams[0].count(),1);assert_eq!(f.upstreams[1].count(),1);
            f.flush().await;
            {
                let c=f.state.conn();
                let (user,key,unit,units,debit):(i64,i64,String,i64,i64)=c.query_row("SELECT user_id,key_id,billing_unit,request_units,debit_micro FROM billing_ledger WHERE request_id>0 ORDER BY id DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
                assert_eq!((user,key,unit,units,debit),(f.user,f.key_id,"request".into(),1,7));
                let mut user=db::identity::user(&c,f.user).unwrap().unwrap();user.policy.allowed_models=None;db::identity::update_user(&c,&user).unwrap();
            }
            f.clear();let mut direct=body;direct["model"]=json!("bottom-0");
            let (status,reply)=f.request("POST",path,Some(&f.key),None,direct).await;
            assert_eq!(status,StatusCode::FORBIDDEN,"{path} internal direct: {reply}");assert_eq!(f.count(),0);
        }
    }
}

#[tokio::test]
async fn successful_certificate_timestamp_refresh_does_not_reject_queued_requests() {
    for kind in [ModelKind::Embeddings,ModelKind::Rerank] {
        let f=Fixture::new(kind).await;
        for (source,upstream) in f.sources.iter().zip(&f.upstreams){db::channels::update(&f.state.conn(),*source,"queued vector source",&upstream.base,true,false,1).unwrap();}
        f.publish().await;f.allow_alias_only();f.clear();
        {let c=f.state.conn();for(k,raw)in db::settings::all(&c).unwrap(){if k.starts_with("internal_vector_compat_v1:0:"){let mut v:Value=serde_json::from_str(&raw).unwrap();v["verified_at_ms"]=json!(db::now_ms()-1000);db::settings::set(&c,&k,&v.to_string()).unwrap();}}}
        let permits=f.sources.iter().map(|id|f.state.channel_gate(id.0,1).unwrap().try_acquire_owned().unwrap()).collect::<Vec<_>>();
        let refresh=async {
            tokio::time::timeout(std::time::Duration::from_secs(5),async{loop{let running:i64=f.state.conn().query_row("SELECT count(*) FROM requests WHERE status='running'",[],|r|r.get(0)).unwrap();if running>0{break;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}}).await.unwrap();
            {let c=f.state.conn();for(k,raw)in db::settings::all(&c).unwrap(){if k.starts_with("internal_vector_compat_v1:0:"){let mut v:Value=serde_json::from_str(&raw).unwrap();v["verified_at_ms"]=json!(db::now_ms());db::settings::set(&c,&k,&v.to_string()).unwrap();}}}
            drop(permits);
        };
        let ((status,body),())=tokio::join!(f.call("shared-model"),refresh);
        assert_eq!(status,StatusCode::OK,"a timestamp-only successful renewal must not reject the queued request: {body}");
        assert_eq!(f.count(),1);
    }
}

/// A call to one alias must not parse unrelated aliases, including recycle-bin
/// entries. This covers the generic planner for every declared model kind; HTTP
/// forwarding remains restricted to the endpoints the gateway already supports.
#[tokio::test]
async fn targeted_shared_lookup_ignores_unrelated_payloads_for_every_model_kind() {
    for kind in [ModelKind::Unknown,ModelKind::Chat,ModelKind::Embeddings,ModelKind::Rerank,ModelKind::Image,ModelKind::Video,ModelKind::Audio] {
        let mut f=Fixture::new(kind).await;
        f.config.targets.truncate(1);
        {
            let c=f.state.conn();
            for i in 0..32 {
                c.execute("INSERT INTO shared_virtual_models(name,normalized_name,enabled,config_json,version,actor_account_id,created_ms,updated_ms) VALUES(?1,?1,1,'invalid unrelated payload',1,1,1,1)",[format!("unrelated-{i}")]).unwrap();
            }
        }
        let saved=f.publish().await;
        let c=f.state.conn();
        let auth=db::identity::context(&c,f.key_id).unwrap().unwrap();
        let plan=db::shared_virtual_routes::resolve(&c,&auth,"SHARED-MODEL",db::now_ms()).unwrap().unwrap();
        assert_eq!(plan.id,saved["id"].as_i64().unwrap());
        assert_eq!(plan.targets.len(),1);
        assert!(db::shared_virtual_routes::validate_current(&c,&auth,plan.id,plan.version,1,db::now_ms()).unwrap().is_none());
    }
}

#[tokio::test]
async fn targeted_shared_lookup_rechecks_revocation_policy_source_and_version() {
    let mut f=Fixture::new(ModelKind::Chat).await;
    f.config.targets.truncate(1);
    let saved=f.publish().await;
    let c=f.state.conn();
    let auth=db::identity::context(&c,f.key_id).unwrap().unwrap();
    let id=saved["id"].as_i64().unwrap();
    let check=||db::shared_virtual_routes::validate_current(&c,&auth,id,1,1,db::now_ms());
    assert!(check().is_ok());
    c.execute("UPDATE gateway_keys SET enabled=0 WHERE id=?1",[f.key_id]).unwrap();
    assert_eq!(check().unwrap_err().code,"virtual_route_forbidden");
    c.execute("UPDATE gateway_keys SET enabled=1 WHERE id=?1",[f.key_id]).unwrap();
    c.execute("UPDATE users SET enabled=0 WHERE id=?1",[f.user]).unwrap();
    assert_eq!(check().unwrap_err().code,"virtual_route_forbidden");
    c.execute("UPDATE users SET enabled=1 WHERE id=?1",[f.user]).unwrap();
    c.execute("UPDATE channels SET enabled=0 WHERE id=?1",[f.sources[0].0]).unwrap();
    assert_eq!(check().unwrap_err().code,"virtual_target_forbidden");
    c.execute("UPDATE channels SET enabled=1 WHERE id=?1",[f.sources[0].0]).unwrap();
    c.execute("UPDATE shared_virtual_models SET version=2 WHERE id=?1",[id]).unwrap();
    assert_eq!(check().unwrap_err().code,"virtual_route_changed");
    c.execute("UPDATE shared_virtual_models SET version=1 WHERE id=?1",[id]).unwrap();
    let mut user=db::identity::user(&c,f.user).unwrap().unwrap();
    user.policy.allowed_models=Some(vec![]);
    db::identity::update_user(&c,&user).unwrap();
    assert_eq!(check().unwrap_err().code,"virtual_route_forbidden");
}

#[tokio::test]
async fn targeted_shared_lookup_preserves_legacy_index_names_and_first_active_precedence() {
    let mut f=Fixture::new(ModelKind::Chat).await;f.config.targets.truncate(1);
    let saved=f.publish().await;let id=saved["id"].as_i64().unwrap();
    let c=f.state.conn();let auth=db::identity::context(&c,f.key_id).unwrap().unwrap();
    // Historical imports stored lower-case display names instead of Rust-normalized names.
    c.execute("UPDATE shared_virtual_models SET normalized_name='shared-model' WHERE id=?1",[id]).unwrap();
    c.execute("INSERT INTO shared_virtual_models(name,normalized_name,enabled,config_json,version,actor_account_id,created_ms,updated_ms) SELECT name,?2,enabled,config_json,version,actor_account_id,created_ms,updated_ms FROM shared_virtual_models WHERE id=?1",rusqlite::params![id,tokengo_shared::normalize_model_name("shared-model")]).unwrap();
    c.execute("INSERT INTO shared_virtual_models(name,normalized_name,enabled,config_json,version,actor_account_id,created_ms,updated_ms) VALUES('broken','broken',1,'unrelated invalid',1,1,1,1)",[]).unwrap();
    assert!(db::shared_virtual_routes::name_exists(&c,"SHARED_MODEL").unwrap());
    assert_eq!(db::shared_virtual_routes::resolve(&c,&auth,"SHARED_MODEL",db::now_ms()).unwrap().unwrap().id,id,"keep the old first active route; never switch sources because an import had a legacy index");
    assert!(db::shared_virtual_routes::validate_current(&c,&auth,id,1,1,db::now_ms()).is_ok());
    c.execute("UPDATE gateway_keys SET enabled=0 WHERE id=?1",[f.key_id]).unwrap();
    assert_eq!(db::shared_virtual_routes::resolve(&c,&auth,"shared-model",db::now_ms()).unwrap_err().code,"virtual_route_forbidden");
}

#[tokio::test]
async fn targeted_shared_lookup_does_not_trust_an_index_pointing_to_another_name() {
    let mut f=Fixture::new(ModelKind::Chat).await;f.config.targets.truncate(1);
    let saved=f.publish().await;let id=saved["id"].as_i64().unwrap();
    let c=f.state.conn();let auth=db::identity::context(&c,f.key_id).unwrap().unwrap();
    c.execute("UPDATE shared_virtual_models SET normalized_name=?2 WHERE id=?1",rusqlite::params![id,tokengo_shared::normalize_model_name("another-name")]).unwrap();
    assert!(db::shared_virtual_routes::resolve(&c,&auth,"another-name",db::now_ms()).unwrap().is_none());
    assert_eq!(db::shared_virtual_routes::resolve(&c,&auth,"shared-model",db::now_ms()).unwrap().unwrap().id,id);
}
