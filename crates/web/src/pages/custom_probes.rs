//! Operator-owned probes use explicit fields and existing credential/model choices.
use super::ModuleDialog;
use crate::{Ctx, admin_api as api, model::Channel};
use leptos::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn initial() -> Value {
    json!({"name":"","source_id":0,"model":"","kind":"chat","input":"Reply OK","max_tokens":"8","expected_status":"200","contains":"","expected_dimensions":"","timeout_secs":"10","max_latency_secs":"","interval_secs":"300","enabled":false,"search_count":"1","need_content":false,"localization":null,"ori_question":null})
}
fn read_draft(row: &Value) -> Value {
    let mut d = row.clone();
    for k in [
        "max_tokens",
        "expected_status",
        "expected_dimensions",
        "interval_secs",
    ] {
        d[k] = json!(row[k].as_u64().map(|n| n.to_string()).unwrap_or_default());
    }
    for (key, target) in [
        ("timeout_ms", "timeout_secs"),
        ("max_latency_ms", "max_latency_secs"),
    ] {
        d[target] = json!(
            row[key]
                .as_u64()
                .map(|n| (n as f64 / 1000.0).to_string())
                .unwrap_or_default()
        );
    }
    d["search_count"] = json!(row["search_count"].as_u64().unwrap_or(1).to_string());
    d["need_content"] = json!(row["need_content"] == true);
    d["contains"] = json!(row["contains"].as_str().unwrap_or(""));
    d
}
fn number(d: &Value, key: &str, label: &str, min: u64, max: u64) -> Result<u64, String> {
    let n = d[key]
        .as_str()
        .unwrap_or("")
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("{label}请输入整数"))?;
    if !(min..=max).contains(&n) {
        return Err(format!("{label}应为 {min} 至 {max}"));
    }
    Ok(n)
}
fn seconds(d: &Value, key: &str, label: &str, optional: bool) -> Result<Option<u64>, String> {
    let s = d[key].as_str().unwrap_or("").trim();
    if optional && s.is_empty() {
        return Ok(None);
    }
    let n = s
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= if optional { 0.001 } else { 1.0 } && *n <= 60.0)
        .ok_or_else(|| {
            format!(
                "{label}应为 {} 至 60 秒",
                if optional { "0.001" } else { "1" }
            )
        })?;
    Ok(Some((n * 1000.0).round() as u64))
}
fn payload(d: &Value, channels: &[Channel]) -> Result<Value, String> {
    let name = d["name"].as_str().unwrap_or("").trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("探针名称须为 1 至 80 字且不含控制字符".into());
    }
    let id = d["source_id"].as_i64().unwrap_or(0);
    let repair = d["version"].is_u64() && d["enabled"] == false;
    let c = channels.iter().find(|c| c.id == id);
    if c.is_none() && !(d["version"].is_u64() && d["enabled"] == false && id > 0) {
        return Err("请选择有效的上游 Key / API 地址".into());
    }
    let model = d["model"].as_str().unwrap_or("");
    if model.is_empty()
        || (!repair && c.is_some_and(|c| !c.models.iter().any(|m| m.model == model)))
    {
        return Err("请选择该接入已配置的模型".into());
    }
    let kind = d["kind"].as_str().unwrap_or("");
    if ![
        "chat",
        "responses",
        "messages",
        "embeddings",
        "search",
        "fetch",
    ]
    .contains(&kind)
    {
        return Err("请选择请求类型".into());
    }
    if !repair
        && !["embeddings", "search", "fetch"].contains(&kind)
        && c.is_some_and(|c| !native_kinds(c).iter().any(|k| k == kind))
    {
        return Err("所选接入未声明支持此原生请求类型".into());
    }
    let input = d["input"].as_str().unwrap_or("");
    if kind == "search" {
        if input.trim().is_empty() || input.chars().count() > 128 {
            return Err("查询词须为 1 至 128 字".into());
        }
    } else if kind == "fetch" {
        validate_url(input)?;
    } else if input.trim().is_empty() || input.len() > 4096 {
        return Err("测试文本须为 1 至 4096 字节".into());
    }
    let contains = d["contains"].as_str().unwrap_or("").trim();
    if kind != "embeddings" && contains.len() > 512 {
        return Err("匹配文字不能超过 512 字节".into());
    }
    let dim = if kind != "embeddings"
        || d["expected_dimensions"]
            .as_str()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        None
    } else {
        Some(number(d, "expected_dimensions", "向量维度", 1, 65536)?)
    };
    let mut b = json!({"name":name,"source_id":id,"model":model,"kind":kind,"input":input,"max_tokens":number(d,"max_tokens","最大输出 Token",1,256)?,"expected_status":number(d,"expected_status","HTTP 状态",200,599)?,"contains":if contains.is_empty()||kind=="embeddings"{None}else{Some(contains)},"expected_dimensions":if kind=="embeddings"{dim}else{None},"timeout_ms":seconds(d,"timeout_secs","等待超时",false)?.unwrap(),"max_latency_ms":seconds(d,"max_latency_secs","耗时上限",true)?,"interval_secs":number(d,"interval_secs","执行间隔（秒）",60,86400)?,"enabled":d["enabled"]==true});
    b["search_count"] = json!(if kind == "search" {
        number(d, "search_count", "搜索结果数", 1, 50)?
    } else {
        d["search_count"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| (1..=50).contains(n))
            .unwrap_or(1)
    });
    b["need_content"] = json!(d["need_content"] == true);
    b["localization"] = d["localization"].clone();
    b["ori_question"] = d["ori_question"].clone();
    if let Some(v) = d["version"].as_u64() {
        b["version"] = json!(v);
    }
    Ok(b)
}
fn validate_url(input: &str) -> Result<(), String> {
    let authority = input
        .split_once("://")
        .filter(|(scheme, _)| ["http", "https"].contains(scheme))
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""));
    if input.chars().count() > 8192
        || input.chars().any(|c| c.is_control() || c.is_whitespace())
        || authority.is_none_or(|s| s.is_empty() || s.contains('@') || s.contains('\\'))
    {
        return Err("请输入完整的 HTTP(S) 网页 URL，不能包含账号密码".into());
    }
    Ok(())
}
fn effective_channel(c: &Channel, meta: &Value) -> Channel {
    let mut c = c.clone();
    if meta["native_kinds"].is_array() {
        c.protocols = Some(json!({"native":meta["native_kinds"]}));
    }
    c
}
fn available_kinds(c: &Channel, known: Option<&str>, meta: &Value) -> Vec<String> {
    let mut list = kind_options(&effective_channel(c, meta), known);
    if meta["credential_kind"]=="oauth" {list.retain(|k|k=="responses");return list;}
    for key in ["search", "fetch"] {
        if meta[key] == true {
            list.push(key.into());
        }
    }
    list
}
fn source_choices(
    c: &Channel,
    name: &str,
    kinds: &BTreeMap<String, String>,
    meta: &Value,
) -> Vec<String> {
    let known = meta["models"]
        .as_array()
        .and_then(|a| a.iter().find(|m| m["name"].as_str() == Some(name)))
        .and_then(|m| m["kind"].as_str())
        .or_else(|| kinds.get(name).map(String::as_str));
    available_kinds(c, known, meta)
}
fn type_label(meta: &Value) -> String {
    match meta["credential_kind"].as_str() {
        Some("oauth") => format!(
            "OAuth 授权 · {}",
            meta["account_label"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("已绑定账号")
        ),
        Some("anonymous") => "免认证接入".into(),
        Some("api_key") => "API Key".into(),
        _ => "凭据类型待确认".into(),
    }
}
fn kind_label(kind: &str) -> &'static str {
    match kind {
        "chat" => "Chat 对话",
        "responses" => "Responses 对话",
        "messages" => "Messages 对话",
        "embeddings" => "Embeddings 向量",
        "search" => "独立搜索",
        "fetch" => "网页提取",
        _ => "请选择请求类型",
    }
}
fn native_kinds(c: &Channel) -> Vec<String> {
    let kinds: Vec<String> = c
        .protocols
        .as_ref()
        .and_then(|v| v["native"].as_array())
        .map(|v| {
            v.iter()
                .filter_map(Value::as_str)
                .filter(|s| ["chat", "responses", "messages"].contains(s))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    kinds
}
fn kind_options(c: &Channel, known: Option<&str>) -> Vec<String> {
    match known {
        Some("embeddings") => vec!["embeddings".into()],
        Some("chat" | "unknown" | "") | None => native_kinds(c),
        _ => vec![],
    }
}
fn cost_text(result: &Value) -> String {
    if !result.is_object() {
        return String::new();
    }
    let b = &result["billing"];
    if !b.is_object() {
        return "成本待确认".into();
    }
    let usage = if b["billing_unit"] == "request" {
        b["request_units"]
            .as_u64()
            .map(|n| format!("{n} 次"))
            .unwrap_or_else(|| "请求用量未知".into())
    } else {
        match (b["input_tokens"].as_u64(), b["output_tokens"].as_u64()) {
            (Some(i), Some(o)) => format!("输入 {i} / 输出 {o} Token"),
            _ => "Token 用量未知".into(),
        }
    };
    let cost = b["observed_cost_micro"]
        .as_u64()
        .map(|n| format!("{}.{:06} USD",n/1_000_000,n%1_000_000))
        .unwrap_or_else(|| {
            if b["cost_status"] == "subscription" {
                "订阅成本未知".into()
            } else {
                "成本未知".into()
            }
        });
    let note = b["note"]
        .as_str()
        .filter(|n| !n.is_empty())
        .map(|n| format!(" · {n}"))
        .unwrap_or_default();
    format!("{usage} · {cost}{note}")
}
fn result_when(r: &Value) -> String {
    let Some(ms) = r["at_ms"].as_f64() else {
        return String::new();
    };
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
        return date
            .to_iso_string()
            .as_string()
            .map(|s| format!("{} UTC", s.replace('T', " ").get(..19).unwrap_or(&s)))
            .unwrap_or_default();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = ms;
        String::new()
    }
}
fn result_text(r: &Value) -> String {
    if !r.is_object() {
        return "尚未执行".into();
    }
    format!(
        "{} · HTTP {} · {} ms · {}",
        if r["ok"] == true {
            "通过"
        } else {
            "未通过"
        },
        r["status"]
            .as_u64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "无响应".into()),
        r["latency_ms"].as_u64().unwrap_or(0),
        r["note"].as_str().unwrap_or("")
    )
}
async fn load() -> Result<(Vec<Value>, Vec<i64>, BTreeMap<i64, Value>), crate::api::ApiErr> {
    let v = api::request("GET", "/api/custom-probes", None).await?;
    if !v["probes"].is_array() {
        return Err(crate::api::ApiErr {
            status: 0,
            message: "探针列表返回异常，请刷新确认服务端版本".into(),
        });
    }
    Ok((
        api::rows(&v, "probes"),
        v["eligible_source_ids"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default(),
        api::rows(&v, "targets")
            .into_iter()
            .filter_map(|r| r["source_id"].as_i64().map(|id| (id, r)))
            .collect(),
    ))
}
#[component]
fn ProbeField(
    draft: RwSignal<Value>,
    field: &'static str,
    label: &'static str,
    #[prop(default = "text")] kind: &'static str,
    #[prop(default = "")] hint: &'static str,
) -> impl IntoView {
    view! {<label>{label}<input type=kind step=if field.ends_with("_secs")&&field!="interval_secs"{"0.001"}else{"1"} prop:value=move||draft.get()[field].as_str().unwrap_or("").to_string() on:input=move|e|draft.update(|d|d[field]=json!(event_target_value(&e)))/>{(!hint.is_empty()).then(||view!{<span class="muted small">{hint}</span>})}</label>}
}
#[component]
pub fn CustomProbes(ctx: Ctx) -> impl IntoView {
    let probes = RwSignal::new(Vec::<Value>::new());
    let eligible = RwSignal::new(Vec::<i64>::new());
    let targets = RwSignal::new(BTreeMap::<i64, Value>::new());
    let model_kinds = RwSignal::new(BTreeMap::<String, String>::new());
    let channels = RwSignal::new(Vec::<Channel>::new());
    let labels = RwSignal::new(BTreeMap::<i64, String>::new());
    let busy = RwSignal::new(true);
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let open = RwSignal::new(false);
    let deleting = RwSignal::new(false);
    let selected = RwSignal::new(None::<Value>);
    let draft = RwSignal::new(initial());
    let provider = RwSignal::new(String::new());
    leptos::task::spawn_local(async move {
        match (load().await, crate::api::channels().await) {
            (Ok(p), Ok(c)) => {
                let mut l = BTreeMap::new();
                let mut ids: Vec<_> = c.iter().filter_map(|c| c.provider_id).collect();
                ids.sort();
                ids.dedup();
                for id in ids {
                    match api::request("GET", &format!("/api/providers/{id}/credentials"), None)
                        .await
                    {
                        Ok(v) => {
                            for (i, r) in api::rows(&v, "credentials").iter().enumerate() {
                                if let Some(ids) = r["source_ids"].as_array() {
                                    for id in ids.iter().filter_map(Value::as_i64) {
                                        l.insert(id, crate::provider_api::credential_label(r, i));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            ctx.note_err(&e);
                            error.set("部分 Key 备注读取失败，请刷新后再配置探针。".into());
                        }
                    }
                }
                match crate::api::model_routes().await {
                    Ok(v) => {
                        model_kinds.set(v.models.into_iter().map(|r| (r.name, r.kind)).collect())
                    }
                    Err(e) => {
                        ctx.note_err(&e);
                        error.set(format!(
                            "读取模型类型失败，请刷新页面再新增探针：{}",
                            e.message
                        ));
                    }
                }
                labels.set(l);
                channels.set(c);
                probes.set(p.0);
                eligible.set(p.1);
                targets.set(p.2);
            }
            (Err(e), _) | (_, Err(e)) => {
                ctx.note_err(&e);
                error.set(e.message);
            }
        }
        busy.set(false);
    });
    let edit = move |row: Option<Value>| {
        if busy.get_untracked() {
            return;
        }
        let d = row.as_ref().map(read_draft).unwrap_or_else(initial);
        let sid = d["source_id"].as_i64().unwrap_or(0);
        provider.set(
            channels
                .get_untracked()
                .iter()
                .find(|c| c.id == sid)
                .map(|c| {
                    c.provider_id
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "ungrouped".into())
                })
                .unwrap_or_default(),
        );
        selected.set(row);
        draft.set(d);
        error.set(String::new());
        notice.set(String::new());
        open.set(true);
    };
    let save = move |_| {
        if busy.get_untracked() {
            return;
        }
        let current = draft.get_untracked();
        if current["version"].is_null() || current["enabled"] == true {
            let kinds = model_kinds.get_untracked();
            let valid = channels
                .get_untracked()
                .iter()
                .find(|c| Some(c.id) == current["source_id"].as_i64())
                .is_some_and(|c| {
                    source_choices(
                        c,
                        current["model"].as_str().unwrap_or(""),
                        &kinds,
                        targets.get_untracked().get(&c.id).unwrap_or(&Value::Null),
                    )
                    .iter()
                    .any(|k| Some(k.as_str()) == current["kind"].as_str())
                });
            if !valid {
                error.set("所选接入、模型与请求类型不匹配，请选择该目标支持的协议或工具。".into());
                return;
            }
        }
        let b = match payload(
            &current,
            &channels
                .get_untracked()
                .iter()
                .map(|c| {
                    effective_channel(
                        c,
                        targets.get_untracked().get(&c.id).unwrap_or(&Value::Null),
                    )
                })
                .collect::<Vec<_>>(),
        ) {
            Ok(b) => b,
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let row = selected.get_untracked();
        let path = row
            .as_ref()
            .and_then(|r| r["id"].as_str())
            .map(|id| format!("/api/custom-probes/{id}"))
            .unwrap_or_else(|| "/api/custom-probes".into());
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request(if row.is_some() { "PUT" } else { "POST" }, &path, Some(b)).await {
                Ok(v) => {
                    if v["probe"].is_object() {
                        selected.set(Some(v["probe"].clone()));
                        draft.set(read_draft(&v["probe"]));
                    }
                    match load().await {
                        Ok(rows) => {
                            probes.set(rows.0);
                            eligible.set(rows.1);
                            targets.set(rows.2);
                            open.set(false);
                            notice.set("探针已保存并重新读取；保存本身不会执行请求。".into());
                        }
                        Err(e) => {
                            ctx.note_err(&e);
                            error.set(format!(
                                "保存已提交，但重新读取失败：{}。请刷新确认，勿重复新增。",
                                e.message
                            ));
                        }
                    }
                }
                Err(e) => {
                    ctx.note_err(&e);
                    if e.status == 409 {
                        if let Ok(p) = load().await {
                            probes.set(p.0);
                            eligible.set(p.1);
                            targets.set(p.2);
                        }
                    }
                    error.set(if e.status == 409 {
                        format!("配置已被其他操作更新，请取消后重新编辑。{}", e.message)
                    } else {
                        e.message
                    });
                }
            }
            busy.set(false);
        });
    };
    let action = move |row: Value, operation: &'static str| {
        if busy.get_untracked() {
            return;
        }
        let id = row["id"].as_str().unwrap_or("").to_string();
        let mut toggle_draft = read_draft(&row);
        toggle_draft["enabled"] = json!(row["enabled"] != true);
        let mut b = match payload(
            &toggle_draft,
            &channels
                .get_untracked()
                .iter()
                .map(|c| {
                    effective_channel(
                        c,
                        targets.get_untracked().get(&c.id).unwrap_or(&Value::Null),
                    )
                })
                .collect::<Vec<_>>(),
        ) {
            Ok(b) => b,
            Err(e) => {
                if operation == "toggle" {
                    error.set(e);
                    return;
                }
                json!({})
            }
        };
        b["enabled"] = json!(row["enabled"] != true);
        let (method, path, body) = match operation {
            "run" => ("POST", format!("/api/custom-probes/{id}/run"), None),
            "delete" => (
                "DELETE",
                format!("/api/custom-probes/{id}"),
                Some(json!({"version":row["version"]})),
            ),
            _ => ("PUT", format!("/api/custom-probes/{id}"), Some(b)),
        };
        busy.set(true);
        error.set(String::new());
        notice.set(String::new());
        leptos::task::spawn_local(async move {
            match api::request(method, &path, body).await {
                Ok(v) => {
                    notice.set(if operation == "run" {
                        format!(
                            "{} · {}",
                            result_text(&v["result"]),
                            cost_text(&v["result"])
                        )
                    } else if operation == "delete" {
                        "探针已删除。".into()
                    } else {
                        "启停状态已保存。".into()
                    });
                    if operation == "delete" {
                        deleting.set(false);
                    }
                    match load().await {
                        Ok(p) => {
                            probes.set(p.0);
                            eligible.set(p.1);
                            targets.set(p.2);
                        }
                        Err(e) => {
                            ctx.note_err(&e);
                            error.set(format!("操作已提交，但重新读取失败：{}", e.message));
                        }
                    }
                }
                Err(e) => {
                    ctx.note_err(&e);
                    if e.status == 409 {
                        if let Ok(p) = load().await {
                            probes.set(p.0);
                            eligible.set(p.1);
                            targets.set(p.2);
                        }
                    }
                    error.set(if e.status == 409 {
                        format!("配置或运行状态发生变化，请刷新后重试。{}", e.message)
                    } else {
                        e.message
                    });
                }
            }
            busy.set(false);
        });
    };
    view! {<section class="custom-probes card"><style>{include_str!("../../custom-probes.css")}</style><div class="custom-probe-head"><div><h2>"自定义探针"</h2><p class="muted small">"按指定凭据、模型和请求类型检查上游。系统诊断不扣用户额度，仍消耗上游资源；实际成本未知时会明确标注。"</p></div><div class="custom-probe-actions"><button class="ghost" disabled=move||busy.get() on:click=move |_|{if busy.get_untracked(){return;}busy.set(true);leptos::task::spawn_local(async move{match load().await{Ok(p)=>{probes.set(p.0);eligible.set(p.1);targets.set(p.2);error.set(String::new());},Err(e)=>{ctx.note_err(&e);error.set(e.message);}}busy.set(false);});}>"刷新"</button><button class="primary" disabled=move||busy.get() on:click=move |_|edit(None)>"新增探针"</button></div></div>
    <p role="status" class="small">{move||notice.get()}</p><Show when=move||!error.get().is_empty()&&!open.get()&&!deleting.get()><p role="alert" class="error">{move||error.get()}</p></Show>
    <div class="custom-probe-table-wrap"><table class="custom-probe-table"><thead><tr><th>"名称 / 定时状态"</th><th>"供应方 · Key · 模型"</th><th>"最近结果"</th><th>"执行间隔"</th><th>"操作"</th></tr></thead><tbody>{move||probes.get().into_iter().map(|row|{let e=row.clone();let t=row.clone();let r=row.clone();let d=row.clone();let sid=row["source_id"].as_i64().unwrap_or(0);let c=channels.get().into_iter().find(|c|c.id==sid);let target=c.as_ref().map(|c|format!("{} · {}",c.provider_name.as_deref().unwrap_or("未分组接入"),labels.get().get(&sid).cloned().unwrap_or_else(||c.base_url.clone()))).unwrap_or_else(||"原接入已移除，请编辑修复".into());let meta=targets.get().get(&sid).cloned().unwrap_or(Value::Null);let credential_type=type_label(&meta);let enabled=row["enabled"]==true;view!{<tr><td><strong>{api::text(&row,"name")}</strong><span class="muted small">{if enabled{"定时已启用"}else{"定时已停用"}}</span></td><td><span>{target}</span><span class="muted small">{credential_type}</span><span class="muted small">{format!("{} · {}",api::text(&row,"model"),kind_label(row["kind"].as_str().unwrap_or("")))}</span></td><td class="custom-probe-result"><span>{result_text(&row["last_result"])}</span><span class="muted small">{result_when(&row["last_result"])}</span><span class="muted small">{cost_text(&row["last_result"])}</span></td><td>{format!("{} 秒",row["interval_secs"].as_u64().unwrap_or(0))}</td><td><div class="custom-probe-actions"><button class="ghost" disabled=move||busy.get() on:click=move |_|edit(Some(e.clone()))>"编辑"</button><button class="ghost" disabled=move||busy.get() on:click=move |_|action(r.clone(),"run")>"执行一次"</button><button class="ghost" disabled=move||busy.get() on:click=move |_|action(t.clone(),"toggle")>{if enabled{"停用"}else{"启用"}}</button><button class="danger" disabled=move||busy.get() on:click=move |_|{selected.set(Some(d.clone()));error.set(String::new());deleting.set(true);}>"删除"</button></div></td></tr>}}).collect_view()}</tbody></table></div><Show when=move||probes.get().is_empty()&&!busy.get()><p class="muted">"暂无探针。点击“新增探针”选择需要检查的上游。"</p></Show>
    <ModuleDialog open=open title="配置自定义探针".into() busy=busy><div class="custom-probe-form"><Show when=move||!error.get().is_empty()><p role="alert" class="error">{move||error.get()}</p></Show><fieldset disabled=move||busy.get()><legend>"检查目标"</legend><p class="muted small">"请求类型与所选凭据的真实能力一致。OAuth 使用已绑定账号；搜索和网页提取使用专用工具接口。"</p><div class="custom-probe-fields"><ProbeField draft=draft field="name" label="探针名称"/>
    <label>"供应方"<select prop:value=move||provider.get() on:change=move|e|{provider.set(event_target_value(&e));draft.update(|d|{d["source_id"]=json!(0);d["model"]=json!("");});}><option value="">"请选择供应方"</option>{move||{let mut map=BTreeMap::new();for c in channels.get().into_iter().filter(|c|eligible.get().contains(&c.id)||draft.get()["source_id"].as_i64()==Some(c.id)){map.insert(c.provider_id.map(|i|i.to_string()).unwrap_or_else(||"ungrouped".into()),c.provider_name.unwrap_or_else(||"未分组接入".into()));}map.into_iter().map(|(id,name)|view!{<option value=id>{name}</option>}).collect_view()}}</select></label>
    <label>"凭据 / API 地址"<select prop:value=move||draft.get()["source_id"].as_i64().unwrap_or(0).to_string() on:change=move|e|draft.update(|d|{let id=event_target_value(&e).parse::<i64>().unwrap_or(0);d["source_id"]=json!(id);d["model"]=json!("");d["kind"]=json!("");})><option value="0">"请选择接入"</option>{move||{let id=draft.get()["source_id"].as_i64().unwrap_or(0);(id>0&&!channels.get().iter().any(|c|c.id==id)).then(||view!{<option value=id.to_string()>"原接入已移除，请重新选择"</option>})}}{move||channels.get().into_iter().filter(|c|eligible.get().contains(&c.id)||draft.get()["source_id"].as_i64()==Some(c.id)).filter(|c|c.provider_id.map(|i|i.to_string()).unwrap_or_else(||"ungrouped".into())==provider.get()).map(|c|{let meta=targets.get().get(&c.id).cloned().unwrap_or(Value::Null);let label=format!("{} · {} · {}",type_label(&meta),meta["credential_label"].as_str().map(str::to_string).or_else(||labels.get().get(&c.id).cloned()).unwrap_or_else(||"上游凭据".into()),c.base_url);view!{<option value=c.id.to_string()>{label}</option>}}).collect_view()}</select></label>
    <label>{move||if matches!(draft.get()["kind"].as_str(),Some("search"|"fetch")){"本地模型（计费归属）"}else{"已配置模型"}}<select prop:value=move||draft.get()["model"].as_str().unwrap_or("").to_string() on:change=move|e|draft.update(|d|{let name=event_target_value(&e);d["model"]=json!(name.clone());let kinds=model_kinds.get_untracked();d["kind"]=json!(channels.get_untracked().iter().find(|c|Some(c.id)==d["source_id"].as_i64()).and_then(|c|source_choices(c,&name,&kinds,targets.get_untracked().get(&c.id).unwrap_or(&Value::Null)).first().cloned()).unwrap_or_default());})><option value="">"请选择模型"</option>{move||{let current=draft.get()["model"].as_str().unwrap_or("").to_string();let id=draft.get()["source_id"].as_i64().unwrap_or(0);(!current.is_empty()&&!channels.get().iter().any(|c|c.id==id&&c.models.iter().any(|m|m.model==current))).then(||view!{<option value=current.clone()>{format!("{}（原映射已移除）",current)}</option>})}}{move||channels.get().into_iter().find(|c|Some(c.id)==draft.get()["source_id"].as_i64()).map(|c|c.models.into_iter().filter(|m|m.enabled||draft.get()["model"].as_str()==Some(m.model.as_str())).map(|m|view!{<option value=m.model.clone()>{m.model.clone()}</option>}).collect_view())}</select></label>
    <label>"请求类型"<select prop:value=move||draft.get()["kind"].as_str().unwrap_or("chat").to_string() on:change=move|e|draft.update(|d|d["kind"]=json!(event_target_value(&e)))>{move||channels.get().into_iter().find(|c|Some(c.id)==draft.get()["source_id"].as_i64()).map(|c|source_choices(&c,draft.get()["model"].as_str().unwrap_or(""),&model_kinds.get(),targets.get().get(&c.id).unwrap_or(&Value::Null)).into_iter().map(|kind|{let label=kind_label(&kind);view!{<option value=kind>{label}</option>}}).collect_view())}</select></label><Show when=move||{let d=draft.get();!matches!(d["kind"].as_str(),Some("search"|"fetch"|"embeddings"))&&targets.get().get(&d["source_id"].as_i64().unwrap_or(0)).is_none_or(|t|t["credential_kind"]!="oauth")}><ProbeField draft=draft field="max_tokens" label="最大输出 Token" kind="number" hint="1–256；仅 API Key 对话请求使用"/></Show></div>
    <p class="custom-probe-target-note muted small">{move||draft.get()["source_id"].as_i64().and_then(|id|targets.get().get(&id).cloned()).map(|m|format!("{}{}",m["availability_note"].as_str().unwrap_or(""),m["cost_note"].as_str().map(|s|format!(" · {s}")).unwrap_or_default())).unwrap_or_default()}</p>
    <Show when=move||draft.get()["source_id"].as_i64().and_then(|id|targets.get().get(&id).cloned()).is_some_and(|m|m["credential_kind"]=="oauth")><p class="muted small">"OAuth 使用订阅额度；不发送输出 Token 上限参数，使用超时和响应大小保护。"</p></Show>
    <Show when=move||draft.get()["kind"]=="search"><div class="custom-probe-fields"><ProbeField draft=draft field="input" label="查询词" hint="1–128 字"/><ProbeField draft=draft field="search_count" label="搜索结果数" kind="number" hint="1–50 条"/><label class="custom-probe-check"><input type="checkbox" prop:checked=move||draft.get()["need_content"]==true on:change=move|e|draft.update(|d|d["need_content"]=json!(event_target_checked(&e)))/>"同时返回网页正文"</label></div></Show>
    <Show when=move||draft.get()["kind"]=="fetch"><ProbeField draft=draft field="input" label="网页 URL" kind="url" hint="完整 HTTP(S) 地址，不可包含账号密码"/></Show>
    <Show when=move||!matches!(draft.get()["kind"].as_str(),Some("search"|"fetch"))><label>"测试文本"<textarea rows="2" prop:value=move||draft.get()["input"].as_str().unwrap_or("").to_string() on:input=move|e|draft.update(|d|d["input"]=json!(event_target_value(&e)))></textarea></label></Show></fieldset>
    <fieldset disabled=move||busy.get()><legend>"通过条件与执行周期"</legend><div class="custom-probe-fields"><ProbeField draft=draft field="expected_status" label="预期 HTTP 状态" kind="number"/><Show when=move||draft.get()["kind"]!="embeddings"><ProbeField draft=draft field="contains" label="响应应包含文字（可选）" hint="匹配回答、搜索结果或网页正文；错误时匹配服务商错误说明"/></Show><Show when=move||draft.get()["kind"]=="embeddings"><ProbeField draft=draft field="expected_dimensions" label="预期向量维度（可选）" kind="number"/></Show><ProbeField draft=draft field="timeout_secs" label="等待超时（秒）" kind="number" hint="1–60 秒"/><ProbeField draft=draft field="max_latency_secs" label="耗时上限（秒，可选）" kind="number" hint="留空只检查超时；填写 0.001–60 秒"/><ProbeField draft=draft field="interval_secs" label="定时执行间隔（秒）" kind="number" hint="60–86400 秒"/><label class="custom-probe-check"><input type="checkbox" prop:checked=move||draft.get()["enabled"]==true on:change=move|e|draft.update(|d|d["enabled"]=json!(event_target_checked(&e)))/>"启用定时执行"</label></div></fieldset><div class="custom-probe-footer"><span class="muted small">"保存配置不会立即执行；可在列表点击“执行一次”。"</span><button class="ghost" disabled=move||busy.get() on:click=move |_|{open.set(false);draft.set(initial());error.set(String::new());}>"取消"</button><button class="primary" disabled=move||busy.get() on:click=save>{move||if busy.get(){"保存中…"}else{"保存探针"}}</button></div></div></ModuleDialog>
    <ModuleDialog open=deleting title="删除探针".into() busy=busy><p>{move||format!("确定删除“{}”？定时执行将停止。",selected.get().as_ref().and_then(|r|r["name"].as_str()).unwrap_or("此探针"))}</p><Show when=move||!error.get().is_empty()><p role="alert" class="error">{move||error.get()}</p></Show><div class="custom-probe-actions"><button class="ghost" disabled=move||busy.get() on:click=move |_|deleting.set(false)>"取消"</button><button class="danger" disabled=move||busy.get() on:click=move |_|{if let Some(r)=selected.get_untracked(){action(r,"delete");}}>"确认删除"</button></div></ModuleDialog></section>}
}
#[cfg(test)]
mod tests {
    use super::*;
    fn channel() -> Channel {
        Channel {
            id: 7,
            protocols: Some(json!({"native":["chat"]})),
            models: vec![crate::model::ModelPair {
                model: "local".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }
    fn valid() -> Value {
        let mut d = initial();
        d["name"] = json!("检查");
        d["source_id"] = json!(7);
        d["model"] = json!("local");
        d
    }
    #[test]
    fn defaults_are_disabled_and_optional_conditions_are_null() {
        let b = payload(&valid(), &[channel()]).unwrap();
        assert_eq!(b["enabled"], false);
        assert_eq!(b["max_tokens"], 8);
        assert_eq!(b["timeout_ms"], 10000);
        assert!(b["contains"].is_null());
        assert!(b["max_latency_ms"].is_null());
        assert!(b["version"].is_null());
    }
    #[test]
    fn read_and_save_preserve_version_and_millisecond_precision() {
        let mut b = payload(&valid(), &[channel()]).unwrap();
        b["version"] = json!(4);
        b["timeout_ms"] = json!(1250);
        b["max_latency_ms"] = json!(1500);
        let copy = payload(&read_draft(&b), &[channel()]).unwrap();
        assert_eq!(copy, b);
    }
    #[test]
    fn invalid_numbers_targets_and_empty_inputs_are_rejected() {
        for (k, v) in [
            ("max_tokens", json!("0")),
            ("expected_status", json!("199")),
            ("interval_secs", json!("0")),
            ("timeout_secs", json!("NaN")),
            ("max_latency_secs", json!("0")),
            ("name", json!(" ")),
            ("input", json!("")),
            ("source_id", json!(999)),
            ("model", json!("missing")),
        ] {
            let mut d = valid();
            d[k] = v;
            assert!(payload(&d, &[channel()]).is_err(), "{k}");
        }
    }
    #[test]
    fn embedding_and_native_protocol_fields_follow_selected_type() {
        assert_eq!(
            kind_options(&channel(), Some("embeddings")),
            vec!["embeddings"]
        );
        assert_eq!(kind_options(&channel(), Some("chat")), vec!["chat"]);
        assert!(kind_options(&channel(), Some("image")).is_empty());
        let mut d = valid();
        d["kind"] = json!("embeddings");
        d["expected_dimensions"] = json!("3");

        d["contains"] = json!("stale text");
        let b = payload(&d, &[channel()]).unwrap();
        assert_eq!(b["expected_dimensions"], 3);
        assert!(b["contains"].is_null());
        d["expected_dimensions"] = json!("65537");
        assert!(payload(&d, &[channel()]).is_err());
        d["kind"] = json!("chat");
        assert!(payload(&d, &[channel()]).unwrap()["expected_dimensions"].is_null());
        d["kind"] = json!("messages");
        assert!(payload(&d, &[channel()]).is_err());
    }
    #[test]
    fn disabled_existing_orphan_can_be_saved_but_never_enabled() {
        let mut d = valid();
        d["version"] = json!(3);
        assert!(payload(&d, &[]).is_ok());
        let mut changed = channel();
        changed.models.clear();
        changed.protocols = None;
        assert!(payload(&d, &[changed]).is_ok());
        d["enabled"] = json!(true);
        assert!(payload(&d, &[]).is_err());
    }
    #[test]
    fn tool_controls_create_typed_payloads_without_generation_fields() {
        let mut d = valid();
        d["kind"] = json!("search");
        d["input"] = json!("检索测试");
        d["search_count"] = json!("3");
        d["need_content"] = json!(true);
        let b = payload(&d, &[channel()]).unwrap();
        assert_eq!(b["search_count"], 3);
        assert_eq!(b["need_content"], true);
        assert_eq!(b["model"], "local");
        d["search_count"] = json!("0");
        assert!(payload(&d, &[channel()]).is_err());
        d["search_count"] = json!("1");
        d["input"] = json!("中".repeat(129));
        assert!(payload(&d, &[channel()]).is_err());
        d["kind"] = json!("fetch");
        d["input"] = json!("https://example.com/page");
        assert!(payload(&d, &[channel()]).is_ok());
        for url in [
            "file:///tmp/a",
            "https://user:password@example.com",
            "http://",
            "https://example.com/ space",
        ] {
            d["input"] = json!(url);
            assert!(payload(&d, &[channel()]).is_err());
        }
    }
    #[test]
    fn oauth_and_tool_types_follow_target_metadata_instead_of_apikey_defaults() {
        let c = channel();
        let meta = json!({"credential_kind":"oauth","account_label":"订阅账户","native_kinds":["responses"],"search":false,"fetch":false});
        assert_eq!(available_kinds(&c, Some("chat"), &meta), vec!["responses"]);
        assert!(available_kinds(&c,Some("embeddings"),&meta).is_empty());
        assert!(type_label(&meta).contains("OAuth"));
        assert!(type_label(&meta).contains("订阅账户"));
        let mut d = valid();
        d["kind"] = json!("responses");
        assert!(payload(&d, &[effective_channel(&c, &meta)]).is_ok());
        let tool =
            json!({"credential_kind":"api_key","native_kinds":[],"search":true,"fetch":true});
        assert_eq!(
            available_kinds(&c, Some("chat"), &tool),
            vec!["search", "fetch"]
        );
        assert!(!type_label(&Value::Null).contains("API Key"));
    }
    #[test]
    fn old_saved_probes_gain_safe_tool_defaults_and_keep_optional_values() {
        let mut original = payload(&valid(), &[channel()]).unwrap();
        for k in [
            "search_count",
            "need_content",
            "localization",
            "ori_question",
        ] {
            original.as_object_mut().unwrap().remove(k);
        }
        let d = read_draft(&original);
        let b = payload(&d, &[channel()]).unwrap();
        assert_eq!(b["search_count"], 1);
        assert_eq!(b["need_content"], false);
        assert!(b["localization"].is_null());
        assert!(b["ori_question"].is_null());
    }
    #[test]
    fn result_cost_never_treats_unknown_or_subscription_as_free() {
        let unknown = json!({"billing":{"billing_unit":"request","request_units":null,"observed_cost_micro":null,"debit_micro":0,"cost_status":"unknown"}});
        assert!(cost_text(&unknown).contains("成本未知"));
        assert!(!cost_text(&unknown).contains("0.000000"));
        let sub = json!({"billing":{"billing_unit":"token","input_tokens":3,"output_tokens":4,"observed_cost_micro":null,"cost_status":"subscription"}});
        assert!(cost_text(&sub).contains("订阅成本未知"));
        assert!(cost_text(&sub).contains("输入 3 / 输出 4"));
        let known = json!({"billing":{"billing_unit":"request","request_units":1,"observed_cost_micro":123,"cost_status":"known"}});
        assert!(cost_text(&known).contains("1 次"));
        assert!(cost_text(&known).contains("0.000123 USD"));
    }
}
