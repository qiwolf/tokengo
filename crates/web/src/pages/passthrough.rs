//! Root-owned opaque HTTP routes; upstream credentials never come back from the API.
use crate::{Ctx, api};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Deserialize)]
struct Route {
    id: i64,
    revision: i64,
    channel_id: i64,
    name: String,
    prefix: String,
    upstream: String,
    strip_prefix: bool,
    enabled: bool,
    price_micro_usd_per_request: i64,
    timeout_secs: u64,
    max_concurrent: u64,
    max_body_bytes: u64,
    allowed_key_ids: Vec<i64>,
    allowed_paths: Option<Vec<String>>,
    headers: Vec<Header>,
}
#[derive(Clone, Deserialize)]
struct Header {
    name: String,
    has_value: bool,
}
#[derive(Clone)]
struct HeaderDraft {
    id: u64,
    name: String,
    value: String,
    has_value: bool,
}
#[derive(Serialize)]
struct HeaderWrite {
    name: String,
    value: Option<String>,
}

fn initial() -> Value {
    json!({"name":"","prefix":"","upstream":"","strip_prefix":true,"enabled":false,"price_micro_usd_per_request":"0","timeout_secs":"60","max_concurrent":"16","max_body_bytes":"20971520","allowed_key_ids":"","allow_all_paths":false,"allowed_paths":""})
}
fn draft_for(r: &Route) -> Value {
    json!({"id":r.id,"revision":r.revision,"name":r.name,"prefix":r.prefix,"upstream":r.upstream,"strip_prefix":r.strip_prefix,"enabled":r.enabled,"price_micro_usd_per_request":r.price_micro_usd_per_request.to_string(),"timeout_secs":r.timeout_secs.to_string(),"max_concurrent":r.max_concurrent.to_string(),"max_body_bytes":r.max_body_bytes.to_string(),"allowed_key_ids":r.allowed_key_ids.iter().map(i64::to_string).collect::<Vec<_>>().join(", "),"allow_all_paths":r.allowed_paths.is_none(),"allowed_paths":r.allowed_paths.clone().unwrap_or_default().join("\n")})
}
fn key_ids(raw: &str) -> Result<Vec<i64>, String> {
    let mut ids = Vec::new();
    for s in raw
        .split(|c: char| c == ',' || c == '，' || c.is_whitespace())
        .filter(|s| !s.is_empty())
    {
        let id = s
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("授权 VKey ID 必须是正整数，以逗号或空格分隔")?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}
fn payload(d: &Value, headers: &[HeaderDraft]) -> Result<Value, String> {
    let mut out = json!({});
    for (field, label) in [
        ("name", "名称"),
        ("prefix", "路径前缀"),
        ("upstream", "上游地址"),
    ] {
        let v = d[field].as_str().unwrap_or("").trim();
        if v.is_empty() {
            return Err(format!("请填写{label}"));
        }
        out[field] = json!(v);
    }
    if !out["prefix"].as_str().unwrap().starts_with('/') {
        return Err("路径前缀须以 / 开头，如 /face/".into());
    }
    for (field, label, min) in [
        ("price_micro_usd_per_request", "每次价格", 0),
        ("timeout_secs", "超时秒数", 1),
        ("max_concurrent", "并发上限", 1),
        ("max_body_bytes", "请求体大小上限", 1),
    ] {
        let n = d[field]
            .as_str()
            .unwrap_or("")
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= min)
            .ok_or_else(|| format!("{label}须为不小于 {min} 的整数"))?;
        out[field] = json!(n);
    }
    out["allowed_key_ids"] = json!(key_ids(d["allowed_key_ids"].as_str().unwrap_or(""))?);
    out["allowed_paths"] = if d["allow_all_paths"] == true {
        Value::Null
    } else {
        let mut paths = Vec::<String>::new();
        for path in d["allowed_paths"]
            .as_str()
            .unwrap_or("")
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !path.starts_with('/') || path.contains(['?', '#']) {
                return Err("允许的上游路径须以 / 开头，不含查询或片段；每行一个完整路径".into());
            }
            if !paths.iter().any(|p| p == path) {
                paths.push(path.into());
            }
        }
        json!(paths)
    };
    out["strip_prefix"] = json!(d["strip_prefix"] == true);
    out["enabled"] = json!(d["enabled"] == true);
    let mut names = Vec::<String>::new();
    let mut writes = Vec::new();
    for h in headers {
        let name = h.name.trim();
        if name.is_empty() {
            return Err("请填写上游 Header 名称，或移除空行".into());
        }
        if names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            return Err("上游 Header 名称不能重复".into());
        }
        if !h.has_value && h.value.is_empty() {
            return Err("新增 Header 请填写值".into());
        }
        names.push(name.to_string());
        writes.push(HeaderWrite {
            name: name.into(),
            value: if h.has_value && h.value.is_empty() {
                None
            } else {
                Some(h.value.clone())
            },
        });
    }
    out["headers"] = serde_json::to_value(writes).map_err(|_| "Header 数据格式错误")?;
    if d["revision"].is_i64() {
        out["revision"] = d["revision"].clone();
    }
    Ok(out)
}

fn probe_payload(path: &str, method: &str, status: &str) -> Result<Value, String> {
    let path = path.trim();
    let status = status
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|n| (200..=599).contains(n));
    if !path.starts_with('/') || path.contains(['?', '#']) || status.is_none() {
        return Err("请填写以 / 开头的上游路径和 200 至 599 的预期状态码".into());
    }
    if !["GET", "POST", "HEAD"].contains(&method) {
        return Err("检查仅支持 GET、POST 或 HEAD".into());
    }
    Ok(json!({"path":path,"method":method,"expected_status":status.unwrap()}))
}

#[component]
fn Field(
    draft: RwSignal<Value>,
    field: &'static str,
    label: &'static str,
    #[prop(default = "text")] kind: &'static str,
    #[prop(default = "")] hint: &'static str,
) -> impl IntoView {
    view! {<label class="account-field"><span>{label}</span><input type=kind required=field!="allowed_key_ids" aria-describedby="passthrough-form-error" prop:value=move||draft.get()[field].as_str().unwrap_or("").to_string() on:input=move|ev|draft.update(|d|d[field]=json!(event_target_value(&ev)))/><span class="muted small">{hint}</span></label>}
}

#[component]
pub fn PassthroughRoutes(ctx: Ctx) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Route>::new());
    let loaded = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let open = RwSignal::new(false);
    let deleting = RwSignal::new(false);
    let probing = RwSignal::new(false);
    let probe_path = RwSignal::new("/check".to_string());
    let probe_method = RwSignal::new("POST".to_string());
    let probe_status = RwSignal::new("200".to_string());
    let probe_result = RwSignal::new(String::new());
    let selected = RwSignal::new(None::<Route>);
    let draft = RwSignal::new(initial());
    let headers = RwSignal::new(Vec::<HeaderDraft>::new());
    let next_header = RwSignal::new(1_u64);
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    Effect::new(move |_| {
        if !open.get() {
            headers.set(Vec::new());
            draft.set(initial());
        }
    });
    let load = Callback::new(move |()| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            match api::typed::<Vec<Route>>("GET", "/api/passthrough-routes", None).await {
                Ok(result) => {
                    rows.set(result);
                    loaded.set(true);
                    error.set(String::new());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    });
    load.run(());
    let save = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let d = draft.get_untracked();
        let body = match payload(&d, &headers.get_untracked()) {
            Ok(v) => v,
            Err(e) => {
                error.set(e);
                return;
            }
        };
        let id = d["id"].as_i64();
        let path = id
            .map(|id| format!("/api/passthrough-routes/{id}"))
            .unwrap_or_else(|| "/api/passthrough-routes".into());
        error.set(String::new());
        busy.set(true);
        leptos::task::spawn_local(async move {
            match api::typed::<Route>(if id.is_some() { "PUT" } else { "POST" }, &path, Some(body))
                .await
            {
                Ok(_) => {
                    open.set(false);
                    notice.set("透传路由已保存。".into());
                    load.run(());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(if e.status == 409 {
                        "配置已被其他操作修改。请关闭编辑并刷新列表后重试。".into()
                    } else {
                        e.message
                    });
                    busy.set(false);
                }
            }
        });
    };
    let remove = move |_| {
        if busy.get_untracked() {
            return;
        }
        let Some(r) = selected.get_untracked() else {
            return;
        };
        busy.set(true);
        error.set(String::new());
        leptos::task::spawn_local(async move {
            match api::call(
                "DELETE",
                &format!("/api/passthrough-routes/{}?revision={}", r.id, r.revision),
                None,
            )
            .await
            {
                Ok(_) => {
                    deleting.set(false);
                    selected.set(None);
                    notice.set("透传路由已删除，历史账目保留。".into());
                    load.run(());
                }
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                    busy.set(false);
                }
            }
        });
    };
    let probe = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let Some(route) = selected.get_untracked() else {
            return;
        };
        let body = match probe_payload(
            &probe_path.get_untracked(),
            &probe_method.get_untracked(),
            &probe_status.get_untracked(),
        ) {
            Ok(body) => body,
            Err(message) => {
                error.set(message);
                return;
            }
        };
        busy.set(true);
        error.set(String::new());
        probe_result.set(String::new());
        leptos::task::spawn_local(async move {
            match api::call(
                "POST",
                &format!("/api/passthrough-routes/{}/probe", route.id),
                Some(body),
            )
            .await
            {
                Ok(value) => probe_result.set(format!(
                    "{}；HTTP {}，耗时 {} ms。仅代表本次接口检查，不代表模型推理已通过。",
                    if value["healthy"] == true {
                        "检查通过"
                    } else {
                        "检查未通过"
                    },
                    value["http_status"],
                    value["latency_ms"]
                )),
                Err(e) => {
                    ctx.note_err(&e);
                    error.set(e.message);
                }
            }
            busy.set(false);
        });
    };
    view! {
        <section class="card passthrough-routes">
            <header class="card-head"><h2>"服务列表"</h2><p class="muted small">"按路径前缀访问 HTTP 服务，适用于人脸检测、CLIP 等自定义接口。请求与响应正文保持原样；与模型分开列出，可在用户与 API Key 的授权中分配。"</p></header>
            <div class="card-body">
                <div class="toolbar"><button type="button" class="primary" disabled=move||busy.get() on:click=move |_|{draft.set(initial());headers.set(Vec::new());error.set(String::new());open.set(true);}>"新建透传服务"</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|load.run(())>"刷新服务"</button></div>
                <Show when=move||busy.get()><p class="muted small" role="status">"正在处理透传路由…"</p></Show>
                <Show when=move||!error.get().is_empty()&&!open.get()&&!deleting.get()&&!probing.get()><p class="banner bad" role="alert">{move||error.get()}</p></Show>
                <Show when=move||!notice.get().is_empty()><p role="status" class="muted small">{move||notice.get()}</p></Show>
                <Show when=move||loaded.get()&&rows.get().is_empty()><p class="muted">"尚无透传服务。点击“新建透传服务”配置上游；新服务默认停用；启用后请在用户或 API Key 中分配权限。"</p></Show>
                <For each=move||rows.get() key=|r|(r.id,r.revision) children=move|r|{
                    let edit=r.clone();let delete=r.clone();let check=r.clone();
                    view!{<div class="panel"><h3>{r.name.clone()}</h3><p class="muted small">{format!("路由 ID：{} · 计费接入 ID：{}",r.id,r.channel_id)}</p><p><code>{r.prefix.clone()}</code>" → "<code>{r.upstream.clone()}</code></p><p class="muted small">{format!("{} · {}前缀 · {} microUSD / 次 · {} 秒 · {} 个授权 VKey",if r.enabled{"已启用"}else{"已停用"},if r.strip_prefix{"剥离"}else{"保留"},r.price_micro_usd_per_request,r.timeout_secs,r.allowed_key_ids.len())}</p><p class="muted small">{format!("授权 VKey ID：{}",if r.allowed_key_ids.is_empty(){"无（可由用户服务授权授予）".into()}else{r.allowed_key_ids.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")})}</p><p class="muted small">{format!("允许的上游路径：{}",r.allowed_paths.as_ref().map(|p|if p.is_empty(){"无（全部拒绝）".into()}else{p.join(", ")}).unwrap_or_else(||"全部子路径".into()))}</p><div class="toolbar"><button type="button" class="ghost" disabled=move||busy.get() aria-label=format!("编辑透传路由 {}",r.name) on:click=move |_|{draft.set(draft_for(&edit));headers.set(edit.headers.iter().enumerate().map(|(i,h)|HeaderDraft{id:i as u64+1,name:h.name.clone(),value:String::new(),has_value:h.has_value}).collect());next_header.set(edit.headers.len() as u64+1);error.set(String::new());open.set(true);}>"编辑"</button><button type="button" class="ghost" disabled=move||busy.get() aria-label=format!("检查透传路由 {}",r.name) on:click=move |_|{selected.set(Some(check.clone()));probe_path.set("/check".into());probe_method.set("POST".into());probe_status.set("200".into());error.set(String::new());probe_result.set(String::new());probing.set(true);}>"检查"</button><button type="button" class="ghost danger" disabled=move||busy.get() aria-label=format!("删除透传路由 {}",r.name) on:click=move |_|{selected.set(Some(delete.clone()));error.set(String::new());deleting.set(true);}>"删除"</button></div></div>}
                }/>
            </div>
            <super::ModuleDialog open=open busy=busy title="配置透传路由".to_string()>
                <form class="account-form passthrough-form" on:submit=save>
                    <fieldset disabled=move||busy.get()><legend>"地址与匹配规则"</legend>
                        <div class="custom-probe-fields"><Field draft=draft field="name" label="名称"/><Field draft=draft field="prefix" label="路径前缀" hint="如 /face/；最长前缀优先，不能覆盖网关管理路径"/><Field draft=draft field="upstream" label="上游地址" kind="url" hint="完整 HTTP(S) 地址，不含账号、查询或片段"/>
                        <label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["strip_prefix"]==true on:change=move|ev|draft.update(|d|d["strip_prefix"]=json!(event_target_checked(&ev)))/>"剥离路径前缀"</label><label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["enabled"]==true on:change=move|ev|draft.update(|d|d["enabled"]=json!(event_target_checked(&ev)))/>"启用此路由"</label></div>
                    </fieldset>
                    <fieldset disabled=move||busy.get()><legend>"授权、计量与限制"</legend><div class="custom-probe-fields">
                        <Field draft=draft field="allowed_key_ids" label="旧模式：允许调用的 VKey ID" hint="可在用户授权中直接分配此服务；此项保留逐 Key 白名单。逗号或空格分隔，填写 VKey 的数字 ID，不是密钥明文。"/>
                        <label class="account-check"><input type="checkbox" prop:checked=move||draft.get()["allow_all_paths"]==true on:change=move|ev|draft.update(|d|d["allow_all_paths"]=json!(event_target_checked(&ev)))/>"允许前缀下全部上游路径（包括维护接口）"</label>
                        <Show when=move||draft.get()["allow_all_paths"]!=true><label class="account-field"><span>"允许的上游路径（每行一个）"</span><textarea rows="4" aria-describedby="passthrough-path-hint passthrough-form-error" prop:value=move||draft.get()["allowed_paths"].as_str().unwrap_or("").to_string() on:input=move|ev|draft.update(|d|d["allowed_paths"]=json!(event_target_value(&ev)))/><span id="passthrough-path-hint" class="muted small">"填写最终上游的精确路径，不含查询。如 /represent 或 /clip/img；留空拒绝全部路径。建议只开放业务接口，避免开放 /restart 等维护接口。"</span></label></Show>
                        <Field draft=draft field="price_micro_usd_per_request" label="每次价格（microUSD）" kind="number" hint="0 表示免费；1 USD = 1,000,000 microUSD"/><Field draft=draft field="timeout_secs" label="总超时（秒）" kind="number"/><Field draft=draft field="max_concurrent" label="同时转发上限" kind="number"/><Field draft=draft field="max_body_bytes" label="请求体大小上限（字节）" kind="number" hint="默认 20971520 字节，即 20 MiB"/>
                    </div><p class="muted small">"已有渠道授权和付费开关仍生效；仅填写 VKey ID 不会扩大原权限。建议为照片服务创建独立密钥，禁用 LLM 模型授权，单独设置预算、RPM 与并发。不自动重试。已转发到上游的请求按次计量，包括上游返回的 4xx / 5xx；中断且结果未知时保守扣除预留金额，用量记为未知。VKey 的预算、RPM 和并发限制仍有效；带 TPM 限制的密钥暂不支持透传。"</p></fieldset>
                    <fieldset disabled=move||busy.get()><legend>"上游 Header"</legend><p class="muted small">"客户端 Authorization、Proxy-Authorization、x-api-key 和 api-key 会被剥离。后端凭据只存服务端，不会回显；已有值留空保留，移除该行会删除。"</p>
                        <For each=move||headers.get() key=|h|h.id children=move|h|{let id=h.id;let existing=h.has_value;view!{<div class="panel"><label class="account-field"><span>"Header 名称"</span><input readonly=existing aria-describedby="passthrough-form-error" prop:value=move||headers.get().iter().find(|h|h.id==id).map(|h|h.name.clone()).unwrap_or_default() on:input=move|ev|headers.update(|hs|{if let Some(h)=hs.iter_mut().find(|h|h.id==id){h.name=event_target_value(&ev);}})/></label><label class="account-field"><span>{if existing{"Header 值（已保存，留空保留）"}else{"Header 值"}}</span><input type="password" autocomplete="new-password" aria-describedby="passthrough-form-error" prop:value=move||headers.get().iter().find(|h|h.id==id).map(|h|h.value.clone()).unwrap_or_default() on:input=move|ev|headers.update(|hs|{if let Some(h)=hs.iter_mut().find(|h|h.id==id){h.value=event_target_value(&ev);}})/></label><button type="button" class="ghost danger" aria-label=format!("移除 Header {}",h.name) on:click=move |_|headers.update(|hs|hs.retain(|h|h.id!=id))>"移除此 Header"</button></div>}}/>
                        <button type="button" class="ghost" on:click=move |_|{let id=next_header.get_untracked();next_header.set(id+1);headers.update(|hs|hs.push(HeaderDraft{id,name:String::new(),value:String::new(),has_value:false}));}>"添加 Header"</button>
                    </fieldset>
                    <p id="passthrough-form-error" role="alert" class="banner bad" hidden=move||error.get().is_empty()>{move||error.get()}</p>
                    <div class="toolbar"><button type="submit" class="primary" disabled=move||busy.get()>{move||if busy.get(){"保存中…"}else{"保存透传路由"}}</button><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|open.set(false)>"取消"</button></div>
                </form>
            </super::ModuleDialog>
            <super::ModuleDialog open=probing busy=busy title="手动检查透传上游".to_string()>
                <p class="muted small">"使用该路由保存的上游凭据发出一次无正文 HTTP 请求。检查计入系统诊断，不扣用户额度；选择安全的检查接口，避免重启等有副作用的路径。此检查不代表模型推理验收。"</p>
                <form class="account-form" on:submit=probe><fieldset disabled=move||busy.get()><legend>"检查条件"</legend><label class="account-field"><span>"上游路径"</span><input required prop:value=probe_path on:input=move|ev|probe_path.set(event_target_value(&ev))/></label><label class="account-field"><span>"HTTP 方法"</span><select prop:value=probe_method on:change=move|ev|probe_method.set(event_target_value(&ev))><option value="POST">"POST"</option><option value="GET">"GET"</option><option value="HEAD">"HEAD"</option></select></label><label class="account-field"><span>"预期 HTTP 状态"</span><input required type="number" min="200" max="599" prop:value=probe_status on:input=move|ev|probe_status.set(event_target_value(&ev))/></label><button type="submit" class="primary">"执行一次检查"</button></fieldset></form><Show when=move||!probe_result.get().is_empty()><p role="status">{move||probe_result.get()}</p></Show><Show when=move||!error.get().is_empty()><p role="alert" class="banner bad">{move||error.get()}</p></Show>
            </super::ModuleDialog>
            <super::ModuleDialog open=deleting busy=busy title="删除透传路由".to_string()><p>{move||selected.get().map(|r|format!("确定删除透传路由“{}”（{}）？后续请求将无法通过此路由访问上游；历史账目保留。",r.name,r.prefix)).unwrap_or_default()}</p><Show when=move||!error.get().is_empty()><p class="banner bad" role="alert">{move||error.get()}</p></Show><div class="toolbar"><button type="button" class="ghost" disabled=move||busy.get() on:click=move |_|deleting.set(false)>"取消"</button><button type="button" class="danger" disabled=move||busy.get() on:click=remove>"确认删除"</button></div></super::ModuleDialog>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_rejects_informational_status_and_unsupported_methods() {
        assert!(probe_payload("/check", "POST", "199").is_err());
        assert!(probe_payload("/check", "POST", "600").is_err());
        assert!(probe_payload("/check", "DELETE", "200").is_err());
        assert_eq!(
            probe_payload("/check", "POST", "200").unwrap(),
            json!({"path":"/check","method":"POST","expected_status":200})
        );
    }
    #[test]
    fn key_ids_are_explicit_and_invalid_entries_are_not_silently_dropped() {
        assert_eq!(key_ids("").unwrap(), Vec::<i64>::new());
        assert_eq!(key_ids("12, 13，12\n14").unwrap(), vec![12, 13, 14]);
        for raw in ["0", "-1", "2, typo", "9223372036854775808"] {
            assert!(key_ids(raw).is_err());
        }
    }
    #[test]
    fn path_grants_are_exact_explicit_and_default_to_deny() {
        let mut d = initial();
        d["name"] = json!("face");
        d["prefix"] = json!("/face/");
        d["upstream"] = json!("http://localhost:8066");
        assert_eq!(payload(&d, &[]).unwrap()["allowed_paths"], json!([]));
        d["allowed_paths"] = json!("/represent\n/clip/img\n/represent");
        assert_eq!(
            payload(&d, &[]).unwrap()["allowed_paths"],
            json!(["/represent", "/clip/img"])
        );
        d["allowed_paths"] = json!("/restart?token=bad");
        assert!(payload(&d, &[]).is_err());
        d["allow_all_paths"] = json!(true);
        assert!(payload(&d, &[]).unwrap()["allowed_paths"].is_null());
    }
    #[test]
    fn saved_secrets_are_preserved_without_reconstructing_a_value() {
        let mut d = initial();
        d["name"] = json!("face");
        d["prefix"] = json!("/face/");
        d["upstream"] = json!("http://localhost:8066");
        d["revision"] = json!(4);
        let h = HeaderDraft {
            id: 1,
            name: "api-key".into(),
            value: String::new(),
            has_value: true,
        };
        let body = payload(&d, &[h.clone()]).unwrap();
        assert_eq!(body["allowed_paths"], json!([]));
        assert!(body["headers"][0]["value"].is_null());
        assert_eq!(body["revision"], 4);
        assert_eq!(body["allowed_key_ids"], json!([]));
        let mut changed = h;
        changed.value = "synthetic-value".into();
        assert_eq!(
            payload(&d, &[changed]).unwrap()["headers"][0]["value"],
            "synthetic-value"
        );
        assert_eq!(payload(&d, &[]).unwrap()["headers"], json!([]));
    }
}
