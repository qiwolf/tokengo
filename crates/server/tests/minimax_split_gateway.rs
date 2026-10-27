//! Synthetic loopback manufacturer-shaped evidence, not a live MiniMax account test.
use axum::{extract::State,response::{IntoResponse,Response},routing::post,Json,Router};
use serde_json::{json,Value};
use std::sync::{Arc,Mutex};
use tokengo_server::{app,db,protocol::Protocol,protocol_config::Capabilities,AppState,Config};
struct Http {url:String,task:tokio::task::JoinHandle<()>}
impl Drop for Http {fn drop(&mut self){self.task.abort();}}
async fn serve(router:Router)->Http {let l=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}",l.local_addr().unwrap());Http{url,task:tokio::spawn(async move{axum::serve(l,router).await.unwrap()})}}
#[derive(Clone)]struct Source {seen:Arc<Mutex<Vec<Value>>>,fault:Option<&'static str>}
fn neutral()->Value {json!({"input_sensitive":false,"output_sensitive":false,"input_sensitive_type":0,"output_sensitive_type":0,"base_resp":{"status_code":0,"status_msg":""}})}
async fn upstream(State(s):State<Source>,Json(body):Json<Value>)->Response {
    s.seen.lock().unwrap().push(body.clone());
    if body["messages"].as_array().unwrap().iter().any(|m|m["role"]=="developer")
        || body["messages"].as_array().unwrap().iter().filter(|m|m["role"]=="system").count()>1 {
        return (axum::http::StatusCode::BAD_REQUEST, "unsupported control shape").into_response();
    }
    if matches!(s.fault,Some("masked_sensitive"|"masked_body")) {
        let masked=if s.fault==Some("masked_body") {json!({"error":{"type":"filtered"},"input_sensitive":true})}else{json!({"choices":[],"output_sensitive":true,"usage":{"prompt_tokens":7,"completion_tokens":0}})};
        return if body["stream"]==true {( [("content-type","text/event-stream")],format!("data: {masked}\n\ndata: [DONE]\n\n") ).into_response()}else{Json(masked).into_response()};
    }
    let continuation=body["messages"].as_array().unwrap().iter().any(|m|m["role"]=="tool");
    let delta=if continuation {json!({"content":"answer <think>literal</think>"})}else{json!({"tool_calls":[{"index":0,"id":"call_w09","type":"function","function":{"name":"weather","arguments":"{}"}}]})};
    if body["stream"]==true {
        let mut out=String::new();
        for (i,(d,finish)) in [(json!({"reasoning_content":"raw 思考"}),Value::Null),(delta,Value::Null),(json!({}),json!(if continuation{"stop"}else{"tool_calls"}))].into_iter().enumerate() {
            let mut frame=json!({"id":"c_w09","object":"chat.completion.chunk","created":1,"model":"MiniMax-M3","choices":[{"index":0,"delta":d,"finish_reason":finish}]});
            if i==2 {frame["usage"]=json!({"prompt_tokens":7,"completion_tokens":9,"total_tokens":16,"completion_tokens_details":{"reasoning_tokens":4}});for(k,v)in neutral().as_object().unwrap(){frame[k]=v.clone();}if matches!(s.fault,Some("sensitive"|"late_sensitive")){frame["output_sensitive"]=json!(true);}}
            out.push_str(&format!("data: {frame}\n\n"));
        }
        if s.fault!=Some("truncated"){out.push_str("data: [DONE]\n\n");}
        if s.fault==Some("late_sensitive") {
            let split=out.find("\n\n").unwrap()+2;let tail=out.split_off(split);
            let stream=futures_util::stream::unfold((0,out,tail),|(n,head,tail)|async move{
                match n {
                    0=>Some((Ok::<_,std::io::Error>(bytes::Bytes::from(head.clone())),(1,head,tail))),
                    1=>{tokio::time::sleep(std::time::Duration::from_millis(40)).await;Some((Ok(bytes::Bytes::from(tail.clone())),(2,head,tail)))},
                    _=>None,
                }
            });
            return ([("content-type","text/event-stream")],axum::body::Body::from_stream(stream)).into_response();
        }
        return ([("content-type","text/event-stream")],out).into_response();
    }
    let mut message=if continuation{json!({"role":"assistant","content":"answer <think>literal</think>","reasoning_content":"raw 思考"})}else{json!({"role":"assistant","content":null,"reasoning_content":"raw 思考","tool_calls":[{"id":"call_w09","type":"function","function":{"name":"weather","arguments":"{}"}}]})};
    if s.fault==Some("opaque"){message["reasoning_details"]=json!([{"type":"encrypted","data":"opaque"}]);}
    let mut out=json!({"id":"c_w09","object":"chat.completion","created":1,"model":"MiniMax-M3","choices":[{"index":0,"message":message,"finish_reason":if continuation{"stop"}else{"tool_calls"}}],"usage":{"prompt_tokens":7,"completion_tokens":9,"total_tokens":16,"completion_tokens_details":{"reasoning_tokens":4}}});
    for(k,v)in neutral().as_object().unwrap(){out[k]=v.clone();}
    if s.fault==Some("sensitive"){out["input_sensitive"]=json!(true);}
    if s.fault==Some("provider_error"){out["base_resp"]=json!({"status_code":1008,"status_msg":"synthetic"});}
    Json(out).into_response()
}
async fn setup(fault:Option<&'static str>)->(Http,Http,Arc<AppState>,String,Arc<Mutex<Vec<Value>>>,std::path::PathBuf) {
    let seen=Arc::new(Mutex::new(vec![]));let source=serve(Router::new().route("/v1/chat/completions",post(upstream)).with_state(Source{seen:seen.clone(),fault})).await;
    let dir=std::env::temp_dir().join(format!("minimax-split-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();
    let state=AppState::init(Config{db_path:dir.join("db"),master_key_path:dir.join("master").to_string_lossy().into(),..Default::default()}).unwrap();state.routing().cfg.retry_times=0;state.routing().cfg.probe_enabled=false;
    let key={let c=state.conn();let(_,key)=db::identity::create_key(&c,1,"synthetic",&Default::default(),None,None).unwrap();let ch=db::channels::create(&c,&db::channels::NewChannel{name:"MiniMax synthetic".into(),base_url:format!("{}/v1",source.url),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();db::channels::set_models(&c,ch,&[db::channels::ModelMapping::new("logical",Some("MiniMax-M3".into()),false)]).unwrap();tokengo_server::protocol_config::save(&c,ch.0,&Capabilities{parameter_profile:Some("minimax".into()),native:vec![Protocol::Chat],allow_conversion:true,chat_reasoning_replay:true,max_input_tokens:Some(8192),max_output_tokens:Some(1024),..Default::default()}).unwrap();db::billing::create_price(&c,&db::billing::Price{id:0,channel_id:ch.0,model:"logical".into(),input_micro_per_million:1_000_000,output_micro_per_million:1_000_000,max_input_tokens:8192,max_output_tokens:1024},"synthetic W09 accounting").unwrap();key};
    let gateway=serve(app(state.clone())).await;(source,gateway,state,key,seen,dir)
}
fn terminal(text:&str)->Value {text.lines().filter_map(|l|l.strip_prefix("data: ")).filter_map(|s|serde_json::from_str::<Value>(s).ok()).find(|v|v["type"]=="response.completed").unwrap()["response"].clone()}
#[tokio::test]
async fn w09_http_and_sse_tools_replay_complete_raw_history_and_neutral_metadata() {
    for stream in [false,true] { for strict in [None,Some(true),Some(false)] {
        let(_source,gateway,state,key,seen,dir)=setup(None).await;let client=reqwest::Client::new();
        let mut request=json!({"model":"logical","instructions":"first instruction","input":[{"role":"system","content":"second instruction"},{"role":"developer","content":"third instruction"},{"role":"user","content":"weather?"}],"stream":stream,"max_output_tokens":41,"tools":[{"type":"function","name":"weather","parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}]});
        if let Some(strict)=strict {request["tools"][0]["strict"]=json!(strict);}
        let response=client.post(format!("{}/v1/responses",gateway.url)).bearer_auth(&key).json(&request).send().await.unwrap();let status=response.status();let text=response.text().await.unwrap();assert_eq!(status,200,"{text}");
        let first=if stream{terminal(&text)}else{serde_json::from_str::<Value>(&text).unwrap()};
        assert_eq!(first["usage"]["input_tokens"],7);assert_eq!(first["usage"]["output_tokens"],9);assert_eq!(first["usage"]["output_tokens_details"]["reasoning_tokens"],4);
        assert_eq!(first["output"][0]["content"][0]["text"],"raw 思考");assert_eq!(first["output"][1]["call_id"],"call_w09");assert_eq!(first["tokengo_provider_metadata"]["fields"]["input_sensitive"],false);
        let mut history=first["output"].as_array().unwrap().clone();history.push(json!({"type":"function_call_output","call_id":"call_w09","output":"sunny"}));
        let response=client.post(format!("{}/v1/responses",gateway.url)).bearer_auth(&key).json(&json!({"model":"logical","input":history,"max_output_tokens":41})).send().await.unwrap();let status=response.status();let text=response.text().await.unwrap();assert_eq!(status,200,"{text}");let next:Value=serde_json::from_str(&text).unwrap();assert_eq!(next["output"][1]["content"][0]["text"],"answer <think>literal</think>");
        let sent=seen.lock().unwrap();assert_eq!(sent.len(),2);assert_eq!(sent[0]["tools"][0]["function"]["strict"],strict.unwrap_or(true),"preserve strict intent even when source enforcement is unverified");assert_eq!(sent[0]["messages"][0],json!({"role":"system","content":"first instruction\n\nsecond instruction\n\nthird instruction"}));for body in sent.iter(){assert_eq!(body["reasoning_split"],true);assert_eq!(body["max_tokens"],41);}assert_eq!(sent[1]["messages"][0]["reasoning_content"],"raw 思考");assert_eq!(sent[1]["messages"][0]["tool_calls"][0]["id"],"call_w09");assert_eq!(sent[1]["messages"][1]["tool_call_id"],"call_w09");drop(sent);
        assert_accounting(&state,2,Some((7,9)),&[if stream {"ok"} else {"success"},"success"]);
        state.drain_writes();drop(gateway);drop(state);std::fs::remove_dir_all(dir).ok();
    }}
}
#[tokio::test]
async fn w09_explicit_nosplit_and_signed_messages_fail_before_transport_native_stays_native() {
    let(_source,gateway,state,key,seen,dir)=setup(None).await;let client=reqwest::Client::new();
    for(path,body)in [("responses",json!({"model":"logical","input":"hi","reasoning_split":false})),("responses",json!({"model":"logical","input":[{"role":"developer","content":"keep","id":"state"}]})),("responses",json!({"model":"logical","input":[{"role":"user","content":"hi"},{"role":"system","content":"late"}]})),("messages",json!({"model":"logical","max_tokens":30,"thinking":{"type":"adaptive"},"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"raw","signature":"signed"}]}]}))] {
        let response=client.post(format!("{}/v1/{path}",gateway.url)).bearer_auth(&key).json(&body).send().await.unwrap();assert_eq!(response.status(),400,"{}",response.text().await.unwrap());assert_eq!(seen.lock().unwrap().len(),0);
    }
    let response=client.post(format!("{}/v1/chat/completions",gateway.url)).bearer_auth(&key).json(&json!({"model":"logical","messages":[{"role":"user","content":"hi"}],"reasoning_split":false,"max_tokens":41,"tools":[{"type":"function","function":{"name":"weather","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}}],"vendor_future":{"keep":"yes"}})).send().await.unwrap();assert_eq!(response.status(),200);let out:Value=response.json().await.unwrap();assert_eq!(out["base_resp"]["status_code"],0);assert!(out.get("tokengo_provider_metadata").is_none());let sent=seen.lock().unwrap();assert_eq!(sent[0]["reasoning_split"],false);assert_eq!(sent[0]["tools"][0]["function"]["strict"],true);assert_eq!(sent[0]["vendor_future"]["keep"],"yes");drop(sent);
    state.drain_writes();drop(gateway);drop(state);std::fs::remove_dir_all(dir).ok();
}
#[tokio::test]
async fn w09_semantic_provider_faults_never_become_successful_responses() {
    for fault in ["sensitive","provider_error","opaque"] {
        let(_source,gateway,state,key,_seen,dir)=setup(Some(fault)).await;
        let response=reqwest::Client::new().post(format!("{}/v1/responses",gateway.url)).bearer_auth(&key).json(&json!({"model":"logical","input":"hi","max_output_tokens":41})).send().await.unwrap();let status=response.status();let text=response.text().await.unwrap();assert!(!status.is_success(),"{fault}: {text}");assert!(!text.contains("response.completed"));
        state.drain_writes();drop(gateway);drop(state);std::fs::remove_dir_all(dir).ok();
    }
}
async fn fallback(state:&Arc<AppState>)->(Http,Arc<Mutex<Vec<Value>>>) {
    let seen=Arc::new(Mutex::new(vec![]));let source=serve(Router::new().route("/v1/chat/completions",post(upstream)).with_state(Source{seen:seen.clone(),fault:None})).await;
    let c=state.conn();let first:i64=c.query_row("SELECT id FROM channels ORDER BY id LIMIT 1",[],|r|r.get(0)).unwrap();
    let ch=db::channels::create(&c,&db::channels::NewChannel{name:"fallback synthetic".into(),base_url:format!("{}/v1",source.url),enabled:true,unwrap_envelope:false,max_concurrent:0}).unwrap();
    db::channels::set_models(&c,ch,&[db::channels::ModelMapping::new("logical",Some("MiniMax-M3".into()),false)]).unwrap();
    tokengo_server::protocol_config::save(&c,ch.0,&Capabilities{parameter_profile:Some("minimax".into()),native:vec![Protocol::Chat],allow_conversion:true,chat_reasoning_replay:true,max_input_tokens:Some(8192),max_output_tokens:Some(1024),..Default::default()}).unwrap();db::billing::create_price(&c,&db::billing::Price{id:0,channel_id:ch.0,model:"logical".into(),input_micro_per_million:1_000_000,output_micro_per_million:1_000_000,max_input_tokens:8192,max_output_tokens:1024},"synthetic W09 accounting").unwrap();
    db::route_policy::save(&c,"logical",&db::route_policy::RoutePolicy{channel_ids:Some(vec![first,ch.0]),retry_times:Some(2),probe_enabled:Some(false),include_in_auto:Some(false),..Default::default()}).unwrap();
    (source,seen)
}
#[tokio::test]
async fn w09_policy_semantics_never_retry_or_switch_source_before_or_after_commit() {
    for (fault,stream) in [("sensitive",false),("sensitive",true),("masked_sensitive",false),("masked_sensitive",true),("masked_body",false),("masked_body",true),("late_sensitive",true)] {
        let(_source,gateway,state,key,seen,dir)=setup(Some(fault)).await;state.routing().cfg.retry_times=2;
        let(_fallback,other)=fallback(&state).await;
        let response=reqwest::Client::new().post(format!("{}/v1/responses",gateway.url)).bearer_auth(&key).json(&json!({"model":"logical","input":"hi","stream":stream,"max_output_tokens":41})).send().await.unwrap();
        let status=response.status();let text=response.text().await.unwrap();
        if fault=="late_sensitive" {assert_eq!(status,200,"{text}");assert!(text.contains("event: error"),"{text}");}else{assert_eq!(status,400,"{text}");}
        assert!(!text.contains("response.completed"),"{text}");assert_eq!(seen.lock().unwrap().len(),1,"semantic flag must not retry source");assert_eq!(other.lock().unwrap().len(),0,"semantic flag must not select another supplier");
        state.drain_writes();let c=state.conn();let mut stmt=c.prepare("SELECT outcome FROM attempts ORDER BY id").unwrap();let outcomes:Vec<String>=stmt.query_map([],|r|r.get(0)).unwrap().map(Result::unwrap).collect();assert_eq!(outcomes,vec!["protocol_fault".to_string()]);assert!(db::policy::rows(&c,"logical").unwrap().iter().all(|r|r.failures==0));drop(stmt);drop(c);
        let usage=if fault=="masked_body"{None}else if fault=="masked_sensitive"{Some((7,0))}else{Some((7,9))};
        assert_accounting(&state,1,usage,&["protocol_fault"]);
        state.drain_writes();drop(gateway);drop(state);std::fs::remove_dir_all(dir).ok();
    }
}
/// Reserve and settle update one unique ledger row per attempt (not one audit
/// event). Unknown source usage may legitimately consume the priced hold.
fn assert_accounting(state:&Arc<AppState>,count:i64,known:Option<(i64,i64)>,outcomes:&[&str]) {
    state.drain_writes();let c=state.conn();
    let requests:i64=c.query_row("SELECT COUNT(*) FROM requests",[],|r|r.get(0)).unwrap();assert_eq!(requests,count);
    let attempts:i64=c.query_row("SELECT COUNT(*) FROM attempts",[],|r|r.get(0)).unwrap();assert_eq!(attempts,count);
    let ledgers:i64=c.query_row("SELECT COUNT(*) FROM billing_ledger",[],|r|r.get(0)).unwrap();assert_eq!(ledgers,count,"one reservation row per actual attempt, not duplicate holds");
    let duplicates:i64=c.query_row("SELECT COUNT(*) FROM (SELECT request_id,COUNT(*) AS n FROM billing_ledger GROUP BY request_id HAVING n<>1)",[],|r|r.get(0)).unwrap();assert_eq!(duplicates,0);
    let mut stmt=c.prepare("SELECT l.input_tokens,l.output_tokens,l.debit_micro,l.reserved_micro,l.status,l.outcome,l.settled_ms,r.input_tokens,r.output_tokens,r.input_tokens_known,r.output_tokens_known FROM billing_ledger l JOIN requests r ON r.id=l.request_id ORDER BY l.id").unwrap();
    let rows:Vec<(Option<i64>,Option<i64>,Option<i64>,i64,String,String,Option<i64>,i64,i64,bool,bool)>=stmt.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?))).unwrap().map(Result::unwrap).collect();assert_eq!(rows.len()as i64,count);
    assert_eq!(outcomes.len() as i64,count);
    for (row,outcome) in rows.into_iter().zip(outcomes) {
        assert_ne!(row.4,"reserved");assert_eq!(row.5,*outcome);assert!(row.6.is_some());assert!(row.2.is_some_and(|debit|debit>=0&&debit<=row.3));
        if let Some((input,output))=known {assert_eq!((row.0,row.1),(Some(input),Some(output)));assert_eq!((row.7,row.8,row.9,row.10),(input,output,true,true));assert_eq!(row.2,Some(input+output),"reported reasoning subset must not add to the source total output twice");}
        else {assert_eq!((row.0,row.1),(None,None));assert!(!row.9&&!row.10,"missing usage stays unknown, not reported zero");assert_eq!(row.2,Some(row.3),"unknown usage consumes the single priced hold under the existing settlement rule");}
    }
    drop(stmt);
    let pending:i64=c.query_row("SELECT COUNT(*) FROM billing_budgets WHERE reserved_micro<>0",[],|r|r.get(0)).unwrap();assert_eq!(pending,0,"all holds released exactly once at terminal settlement");
    let mismatched:i64=c.query_row("SELECT COUNT(*) FROM billing_budgets b WHERE b.spent_micro<>COALESCE((SELECT SUM(l.debit_micro) FROM billing_allocations a JOIN billing_ledger l ON l.id=a.reservation_id WHERE a.scope=b.scope AND a.scope_id=b.scope_id),0)",[],|r|r.get(0)).unwrap();assert_eq!(mismatched,0,"budget debit equals the unique settled allocations, without a second debit");
}
