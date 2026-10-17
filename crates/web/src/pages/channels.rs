//! 渠道管理：增删改查 + 自定义请求头 + 按渠道改模型名 + 「测一下」连通性 + 拉上游模型清单。
//!
//! 三条纪律都落在界面上：
//!
//! - **「测一下」不给假绿点**：真发一个请求，连不上/上游不收都如实报红，并写清原因；
//!   这条渠道没配模型名就直接说"探不了"，而不是拿虚拟组名 `auto` 去凑一个 400。
//! - **密钥留空 = 不改**：编辑一条渠道时密钥框留空表示沿用已存的；想清掉要**明确**点
//!   「清空密钥」。不然每次改个名字都得重输一遍密钥。
//! - **清单是上游说的，不是我们编的**：拉不到就只说实话（连不上/HTTP 404/格式没认出），
//!   绝不给一份看着挺像的假清单。勾选也**默认一个不勾**，要加哪几条自己挑。
//! - **「能不能看图」只认实测**：模型名里带 vl/vision 不代表能看图（deepseek 的 flash
//!   就能读图，有些带 vision 的名字反倒是纯文本）。所以每一行都有「测视觉」按钮：
//!   真发一张图问一个只有看图才答得出的问题（左上角什么颜色），答对了才勾上。
//!   勾了才有资格接带图的请求 —— 没勾的渠道，带图请求一条都不会发给它。

use std::collections::BTreeSet;

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{HeaderPair, Channel, ModelPair};
use crate::Ctx;

/// 上游清单一次最多显示这么多行（真有上游报上千个模型，全塞进 DOM 会卡）。
const SHOW_LIMIT: usize = 200;

/// 按关键字过滤上游报的模型名（大小写不敏感），最多取 `limit` 个。
pub(crate) fn filter_models(all: &[String], filter: &str, limit: usize) -> Vec<String> {
    let needle = filter.trim().to_lowercase();
    all.iter()
        .filter(|m| needle.is_empty() || m.to_lowercase().contains(&needle))
        .take(limit)
        .cloned()
        .collect()
}

/// 把勾选的模型名追加成「该渠道认的模型名」表单行，返回加了几个。
///
/// 空名字跳过、表里已有的**不重复加**（否则连点两次就一堆重复行）。
/// 加进来的行右边留空 = 同名发给上游；要改名人在右框里自己填。
pub(crate) fn add_picked_rows(rows: &mut Vec<ModelPair>, picked: &BTreeSet<String>) -> usize {
    let mut added = 0;
    for name in picked {
        let name = name.trim();
        if name.is_empty() || rows.iter().any(|m| m.model.trim() == name) {
            continue;
        }
        rows.push(ModelPair {
            model: name.to_string(),
            upstream: None,
            // 刚从上游清单里拉回来的名字，能力未知 —— **不许猜**，
            // 要标能看图得点这一行的「测视觉」实测。
            vision: false,
        });
        added += 1;
    }
    added
}

/// 编辑表单的状态。
#[derive(Debug, Clone, PartialEq)]
enum Form {
    /// 没在编辑
    Closed,
    /// 新建
    New,
    /// 改已存在的渠道
    Edit(i64),
}

#[component]
pub fn Channels(ctx: Ctx) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Channel>::new());
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let form = RwSignal::new(Form::Closed);
    let probe_kind = RwSignal::new("chat".to_string());
    let probe_msg = RwSignal::new(None::<(i64, String, bool)>);

    // 「拉取上游模型清单」的状态
    let pulled = RwSignal::new(Vec::<String>::new());
    let picked = RwSignal::new(BTreeSet::<String>::new());
    let pull_msg = RwSignal::new(None::<(String, bool)>);
    let pulling = RwSignal::new(false);
    let pull_filter = RwSignal::new(String::new());

    // 「测视觉」的状态：(这行的渠道 id, 给人看的结果, 好不好)
    let vt_msg = RwSignal::new(None::<(i64, String, bool)>);
    let vt_busy = RwSignal::new(None::<i64>);

    // 表单字段
    let f_name = RwSignal::new(String::new());
    let f_url = RwSignal::new(String::new());
    let f_enabled = RwSignal::new(true);
    let f_key = RwSignal::new(String::new());
    let f_key_note = RwSignal::new("");
    let f_headers = RwSignal::new(Vec::<HeaderPair>::new());
    let f_models = RwSignal::new(Vec::<ModelPair>::new());
    // 表单行的重绘开关：只有加/删行才 +1。**打字时不重绘**，否则输入框会丢焦点。
    let form_ver = RwSignal::new(0u32);

    // 重新进表单：清掉上一次拉的清单，免得看着像这条渠道的
    let reset_pull = move || {
        pulled.set(Vec::new());
        picked.set(BTreeSet::new());
        pull_msg.set(None);
        pull_filter.set(String::new());
    };

    let reload = move || {
        busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api::channels().await {
                Ok(v) => {
                    rows.set(v);
                    err.set(None);
                }
                Err(e) => {
                    ctx.note_err(&e);
                    err.set(Some(e.message));
                }
            }
            busy.set(false);
        });
    };
    reload();

    let open_new = move |_| {
        f_name.set(String::new());
        f_url.set(String::new());
        f_enabled.set(true);
        f_key.set(String::new());
        f_key_note.set("新建时要填密钥；留空表示这条渠道没有密钥。");
        f_headers.set(Vec::new());
        f_models.set(Vec::new());
        reset_pull();
        form_ver.update(|v| *v += 1);
        form.set(Form::New);
    };

    let open_edit = move |l: Channel| {
        f_name.set(l.name.clone());
        f_url.set(l.base_url.clone());
        f_enabled.set(l.enabled);
        f_key.set(String::new());
        f_key_note.set(if l.has_secret {
            "留空 = 沿用已存的密钥；要清掉请点「清空密钥」。"
        } else {
            "这条渠道现在没有密钥。"
        });
        f_headers.set(l.headers.clone());
        f_models.set(l.models.clone());
        reset_pull();
        form_ver.update(|v| *v += 1);
        form.set(Form::Edit(l.id));
    };

    let save = move |_| {
        let (Some(name), Some(url)) = (
            Some(f_name.get_untracked().trim().to_string()),
            Some(f_url.get_untracked().trim().to_string()),
        ) else {
            return;
        };
        if name.is_empty() || url.is_empty() {
            ctx.say("名称和 base_url 都得填");
            return;
        }
        let key = f_key.get_untracked();
        let id = match form.get_untracked() {
            Form::Edit(i) => Some(i),
            _ => None,
        };
        let headers = f_headers.get_untracked();
        let models = f_models
            .get_untracked()
            .into_iter()
            .filter(|m| !m.model.trim().is_empty())
            .collect::<Vec<_>>();
        let enabled = f_enabled.get_untracked();
        // 编辑时留空 = 不改（传 None）；新建时空 = 这条渠道没密钥（也传 None，后端一样）
        let key_arg = if key.trim().is_empty() { None } else { Some(key) };

        wasm_bindgen_futures::spawn_local(async move {
            match api::save_channel(id, &name, &url, enabled, key_arg.as_deref(), &headers, &models)
                .await
            {
                Ok(new_id) => {
                    match new_id {
                        // 新建：**表单不关**，直接切成"编辑这条新渠道" ——
                        // 这样能马上用它的 URL/密钥去拉上游模型清单，不用先关再点编辑
                        Some(i) => {
                            form.set(Form::Edit(i));
                            f_key.set(String::new());
                            f_key_note.set("留空 = 沿用已存的密钥；要清掉请点「清空密钥」。");
                            ctx.say("渠道已新建 —— 接着可以点「拉取上游模型清单」");
                        }
                        None => {
                            form.set(Form::Closed);
                            ctx.say("渠道已保存");
                        }
                    }
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("保存失败：{}", e.message));
                }
            }
        });
    };

    let clear_key = move |_| {
        let Form::Edit(id) = form.get_untracked() else { return };
        let name = f_name.get_untracked();
        let url = f_url.get_untracked();
        let enabled = f_enabled.get_untracked();
        let headers = f_headers.get_untracked();
        let models = f_models.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            // 传空串 = 明确清掉密钥
            match api::save_channel(Some(id), &name, &url, enabled, Some(""), &headers, &models).await {
                Ok(_) => {
                    f_key_note.set("已清空这条渠道的密钥。");
                    ctx.say("密钥已清空");
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("清空失败：{}", e.message));
                }
            }
        });
    };

    let remove = move |l: Channel| {
        let ok = web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("确定删掉渠道「{}」？这条渠道的历史账会留着。", l.name)).ok())
            .unwrap_or(false);
        if !ok {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match api::delete_channel(l.id).await {
                Ok(()) => {
                    ctx.say("渠道已删除");
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("删除失败：{}", e.message));
                }
            }
        });
    };

    let probe = move |l: Channel| {
        let kind = probe_kind.get_untracked();
        probe_msg.set(Some((l.id, "正在真发一个请求…".into(), true)));
        wasm_bindgen_futures::spawn_local(async move {
            match api::probe_channel(l.id, &kind).await {
                Ok(r) => {
                    let mut s = String::new();
                    if r.ok {
                        s.push_str("通了");
                    } else {
                        s.push_str("没通");
                    }
                    if let Some(m) = &r.model_sent {
                        s.push_str(&format!("；发给上游的模型名是 {m}"));
                    }
                    if let Some(code) = r.http_status {
                        s.push_str(&format!("；HTTP {code}"));
                    }
                    if let Some(ms) = r.latency_ms {
                        s.push_str(&format!("；{}", fmt::dur(ms)));
                    }
                    if !r.has_secret {
                        s.push_str("；注意：这条渠道没配密钥");
                    }
                    if let Some(n) = &r.note {
                        // 成功时 note 就是"通了"，前面已经写过一次，别重复
                        if !(r.ok && n == "通了") {
                            s.push_str(&format!("；{n}"));
                        }
                    }
                    probe_msg.set(Some((l.id, s, r.ok)));
                }
                Err(e) => {
                    ctx.note_err(&e);
                    probe_msg.set(Some((l.id, format!("测不了：{}", e.message), false)));
                }
            }
        });
    };

    // 「测视觉」：给这一行真发一张带图的小请求，看这条渠道认不认得出来。
    //
    // 为什么要真发：我们不像上游 FreeLLMAPI 那样有一份带能力标记的目录
    // （本项目的上游全是自己加的），而"名字里带 vl"这种推断经常错。
    // 测通了我们顺手把这行标上（服务端的 apply），界面上勾也随之亮起。
    let vision_test = move |(channel_id, model, upstream): (i64, String, String)| {
        if model.trim().is_empty() {
            ctx.say("先填左边的模型名（客户端打的那个），不然不知道该测哪一行");
            return;
        }
        vt_busy.set(Some(channel_id));
        vt_msg.set(Some((
            channel_id,
            format!("正在给「{model}」真发一张图（问它左上角是什么颜色）…"),
            true,
        )));
        wasm_bindgen_futures::spawn_local(async move {
            let up = upstream.trim().to_string();
            let up_arg = if up.is_empty() { None } else { Some(up.as_str()) };
            match api::vision_test(channel_id, &model, up_arg).await {
                Ok(r) => {
                    vt_busy.set(None);
                    let mut s = if r.ok { "能看图".to_string() } else { "没测出来".to_string() };
                    if let Some(m) = &r.model_sent {
                        s.push_str(&format!("；发给上游的是 {m}"));
                    }
                    if let Some(code) = r.http_status {
                        s.push_str(&format!("；HTTP {code}"));
                    }
                    if let Some(ms) = r.latency_ms {
                        s.push_str(&format!("；{}", fmt::dur(ms)));
                    }
                    if let Some(reply) = r.reply.as_deref().filter(|x| !x.trim().is_empty()) {
                        s.push_str(&format!("；它回的话「{reply}」"));
                    }
                    if r.applied {
                        s.push_str("；已顺手勾上「能看图」");
                        // 把表里这一行的勾也点亮（勾=服务端真落库了，不是本地糊弄）
                        f_models.update(|v| {
                            if let Some(x) = v
                                .iter_mut()
                                .find(|x| x.model.trim() == model.trim())
                            {
                                x.vision = true;
                            }
                        });
                    }
                    if let Some(n) = &r.note {
                        s.push_str(&format!("；{n}"));
                    }
                    vt_msg.set(Some((channel_id, s, r.ok)));
                }
                Err(e) => {
                    vt_busy.set(None);
                    ctx.note_err(&e);
                    vt_msg.set(Some((channel_id, format!("测不了：{}", e.message), false)));
                }
            }
        });
    };

    // 拉这条渠道的上游模型清单：真打一次 GET /v1/models。
    let pull = move |_| {
        let Form::Edit(id) = form.get_untracked() else {
            ctx.say("这条渠道还没保存：先点「保存」，再用它自己的 URL 和密钥去拉清单");
            return;
        };
        pulling.set(true);
        pull_msg.set(Some(("正在真打上游的 /v1/models…".into(), true)));
        wasm_bindgen_futures::spawn_local(async move {
            match api::pull_channel_models(id).await {
                Ok(r) => {
                    pulling.set(false);
                    let mut s = String::new();
                    s.push_str(if r.ok {
                        "拉到清单了"
                    } else {
                        "没拉到清单"
                    });
                    if r.ok {
                        s.push_str(&format!("：{} 个模型名", r.count));
                    }
                    if let Some(u) = &r.url {
                        s.push_str(&format!("；打的地址 {u}"));
                    }
                    if let Some(code) = r.http_status {
                        s.push_str(&format!("；HTTP {code}"));
                    }
                    if let Some(ms) = r.latency_ms {
                        s.push_str(&format!("；{}", fmt::dur(ms)));
                    }
                    if let Some(n) = &r.note {
                        // 成功时 note 是"拉到 N 个模型名"，前面写过了
                        if !(r.ok && n.starts_with("拉到")) {
                            s.push_str(&format!("；{n}"));
                        }
                    }
                    pull_msg.set(Some((s, r.ok)));
                    // 默认**一个都不勾**：真要哪个自己挑，免得一把加进来几十条
                    picked.set(BTreeSet::new());
                    pull_filter.set(String::new());
                    pulled.set(r.models);
                }
                Err(e) => {
                    pulling.set(false);
                    ctx.note_err(&e);
                    pull_msg.set(Some((format!("拉不了：{}", e.message), false)));
                }
            }
        });
    };

    // 勾选「当前显示的那些」
    let select_shown = move |_| {
        let names = filter_models(&pulled.get_untracked(), &pull_filter.get_untracked(), SHOW_LIMIT);
        picked.update(|s| {
            for n in names {
                s.insert(n);
            }
        });
    };

    // 把勾选的模型名加进「该渠道认的模型名」表
    let add_picked = move |_| {
        let sel = picked.get_untracked();
        if sel.is_empty() {
            ctx.say("先勾选要加的模型名");
            return;
        }
        let added = {
            let mut rows = f_models.get_untracked();
            let n = add_picked_rows(&mut rows, &sel);
            f_models.set(rows);
            n
        };
        form_ver.update(|v| *v += 1);
        if added == 0 {
            ctx.say("勾选的这些已经在表里了");
        } else {
            ctx.say(format!(
                "加了 {added} 条 —— 右边留空就是同名发出去，要改名在这填"
            ));
        }
    };

    view! {
        <div class="page">
            <div class="toolbar">
                <button class="primary" on:click=open_new>"新建渠道"</button>
                <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                    {move || if busy.get() { "刷新中…" } else { "刷新" }}
                </button>
                <span class="spacer"></span>
                <span class="muted small">"「测一下」发的是"</span>
                <button
                    class="chip"
                    class:active=move || probe_kind.get() == "chat"
                    on:click=move |_| probe_kind.set("chat".into())
                >
                    "对话请求"
                </button>
                <button
                    class="chip"
                    class:active=move || probe_kind.get() == "embeddings"
                    on:click=move |_| probe_kind.set("embeddings".into())
                >
                    "向量请求"
                </button>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <table class="grid">
                <thead>
                    <tr>
                        <th class="num">"#"</th>
                        <th>"名称"</th>
                        <th>"base_url"</th>
                        <th>"状态"</th>
                        <th>"密钥"</th>
                        <th class="num">"请求头"</th>
                        <th class="num">"模型名"</th>
                        <th>"操作"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        let v = rows.get();
                        if v.is_empty() {
                            return view! {
                                <tr class="empty-row">
                                    <td colspan="8" class="muted empty-cell">
                                        "还没有渠道 —— 点左上「新建渠道」加一条上游。"
                                    </td>
                                </tr>
                            }.into_any();
                        }
                        v.into_iter()
                            .map(|l| {
                                let l2 = l.clone();
                                let l3 = l.clone();
                                let l4 = l.clone();
                                let (pid, pmsg, pok) = probe_msg
                                    .get()
                                    .map(|(i, m, ok)| (Some(i), m, ok))
                                    .unwrap_or((None, String::new(), false));
                                let show_probe = pid == Some(l.id) && !pmsg.is_empty();
                                view! {
                                    // data-label：手机上表格变卡片时当左侧小标签用
                                    <tr>
                                        <td class="num muted" data-label="#">{l.id}</td>
                                        <td data-label="名称">
                                            <b>{l.name.clone()}</b>
                                        </td>
                                        <td data-label="base_url">
                                            <code class="small">{l.base_url.clone()}</code>
                                        </td>
                                        <td data-label="状态">
                                            {if l.enabled {
                                                view! { <span class="tag ok">"启用"</span> }.into_any()
                                            } else {
                                                view! { <span class="tag muted">"停用"</span> }.into_any()
                                            }}
                                        </td>
                                        <td data-label="密钥">
                                            {if l.has_secret {
                                                view! { <span class="tag ok">"有"</span> }.into_any()
                                            } else {
                                                view! { <span class="tag warn">"没有"</span> }.into_any()
                                            }}
                                        </td>
                                        <td class="num" data-label="请求头">{l.headers.len()}</td>
                                        <td class="num" data-label="模型名">
                                            {if l.models.is_empty() {
                                                view! { <span class="muted">"原样透传"</span> }.into_any()
                                            } else if l.vision_count > 0 {
                                                // 这条渠道上有能看图的模型名 —— 一眼就能看见
                                                view! {
                                                    <span>
                                                        {l.models.len()}
                                                        <span class="tag vision" title="这些模型名能收图片输入">
                                                            {format!("{} 条能看图", l.vision_count)}
                                                        </span>
                                                    </span>
                                                }
                                                    .into_any()
                                            } else {
                                                view! { <span>{l.models.len()}</span> }.into_any()
                                            }}
                                        </td>
                                        <td class="actions">
                                            <button class="ghost" on:click=move |_| open_edit(l2.clone())>
                                                "编辑"
                                            </button>
                                            <button class="ghost" on:click=move |_| probe(l3.clone())>
                                                "测一下"
                                            </button>
                                            <button class="ghost danger" on:click=move |_| remove(l4.clone())>
                                                "删除"
                                            </button>
                                        </td>
                                    </tr>
                                    {show_probe.then(|| view! {
                                        <tr class="probe-row">
                                            <td></td>
                                            <td colspan="7" class=move || if pok { "probe ok" } else { "probe bad" }>
                                                {pmsg.clone()}
                                            </td>
                                        </tr>
                                    })}
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </tbody>
            </table>

            {move || if form.get() == Form::Closed {
                ().into_any()
            } else {
                let editing = matches!(form.get(), Form::Edit(_));
                view! {
                    <div class="modal-backdrop">
                        <div class="modal">
                            <h2>{if editing { "编辑渠道" } else { "新建渠道" }}</h2>
                            <div class="form-grid">
                                <label>"名称"</label>
                                <input type="text" prop:value=f_name placeholder="如：硅基流动"
                                    on:input=move |ev| f_name.set(event_target_value(&ev)) />
                                <label>"base_url"</label>
                                <input type="text" prop:value=f_url
                                    placeholder="如：https://api.siliconflow.cn/v1"
                                    on:input=move |ev| f_url.set(event_target_value(&ev)) />
                                <label>"密钥"</label>
                                <div>
                                    <input type="password" prop:value=f_key
                                        on:input=move |ev| f_key.set(event_target_value(&ev)) />
                                    <div class="muted small">{move || f_key_note.get()}</div>
                                </div>
                                <label>"启用"</label>
                                <div>
                                    <input type="checkbox" prop:checked=f_enabled
                                        on:change=move |ev| f_enabled.set(event_target_checked(&ev)) />
                                    <span class="muted small">"停用后这条渠道不再参与路由"</span>
                                </div>
                            </div>

                            <h3>"自定义请求头"</h3>
                            <p class="muted small">
                                "值支持模板：" <code>"{{uuid}}"</code> "（每次请求换新 UUID）、"
                                <code>"{{timestamp_ms}}"</code> "（当前毫秒）。"
                            </p>
                            <table class="grid tight">
                                <tbody>
                                    {move || {
                                        form_ver.get(); // 只在加/删行时重绘
                                        let v = f_headers.get_untracked();
                                        if v.is_empty() {
                                            return view! {
                                                <tr>
                                                    <td class="muted">"没有自定义请求头"</td>
                                                </tr>
                                            }.into_any();
                                        }
                                        v.iter()
                                            .enumerate()
                                            .map(|(i, h)| {
                                                let h = h.clone();
                                                let name0 = h.name.clone();
                                                let val0 = h.value.clone();
                                                view! {
                                                    <tr>
                                                        <td>
                                                            <input type="text" prop:value=name0
                                                                placeholder="头名，如 x-opencode-session"
                                                                on:input=move |ev| {
                                                                    let s = event_target_value(&ev);
                                                                    f_headers
                                                                        .update(|v| {
                                                                            if let Some(x) = v.get_mut(i) {
                                                                                x.name = s;
                                                                            }
                                                                        });
                                                                } />
                                                        </td>
                                                        <td>
                                                            <input type="text" prop:value=val0
                                                                placeholder="值"
                                                                on:input=move |ev| {
                                                                    let s = event_target_value(&ev);
                                                                    f_headers
                                                                        .update(|v| {
                                                                            if let Some(x) = v.get_mut(i) {
                                                                                x.value = s;
                                                                            }
                                                                        });
                                                                } />
                                                        </td>
                                                        <td>
                                                            <button class="ghost danger" on:click=move |_| {
                                                                f_headers.update(|v| { v.remove(i); });
                                                                form_ver.update(|v| *v += 1);
                                                            }>"删"</button>
                                                        </td>
                                                    </tr>
                                                }
                                            })
                                            .collect_view()
                                            .into_any()
                                    }}
                                </tbody>
                            </table>
                            <button class="ghost" on:click=move |_| {
                                f_headers.update(|v| v.push(HeaderPair::default()));
                                form_ver.update(|v| *v += 1);
                            }>"加一个请求头"</button>

                            <h3>"该渠道认的模型名"</h3>

                            <div class="pull-bar">
                                <button
                                    class="ghost"
                                    disabled=move || pulling.get() || !matches!(form.get(), Form::Edit(_))
                                    on:click=pull
                                >
                                    {move || if pulling.get() { "拉取中…" } else { "拉取上游模型清单" }}
                                </button>
                                <span class="muted small">
                                    "用这条渠道自己的 base_url、密钥和自定义请求头，真打一次 "
                                    <code>"GET /v1/models"</code>
                                    "；拉不到就如实报错，不会给你一份编出来的清单。"
                                </span>
                            </div>
                            {move || if matches!(form.get(), Form::New) {
                                view! {
                                    <p class="muted small">
                                        "这条渠道还没保存，所以还拿不到它的凭据去拉 —— 先点下面的「保存」，接着就能拉了。"
                                    </p>
                                }
                                    .into_any()
                            } else {
                                ().into_any()
                            }}
                            {move || {
                                pull_msg
                                    .get()
                                    .map(|(m, ok)| {
                                        view! {
                                            <div class=if ok { "banner ok" } else { "banner bad" }>
                                                {m}
                                            </div>
                                        }
                                    })
                            }}
                            {move || {
                                let all = pulled.get();
                                if all.is_empty() {
                                    return ().into_any();
                                }
                                let shown = filter_models(&all, &pull_filter.get(), SHOW_LIMIT);
                                let total = all.len();
                                let shown_n = shown.len();
                                let picked_n = picked.get().len();
                                let rows = shown
                                    .into_iter()
                                    .map(|name| {
                                        let key = name.clone();
                                        let checked = picked.get().contains(&name);
                                        let already = f_models
                                            .get_untracked()
                                            .iter()
                                            .any(|m| m.model.trim() == name.trim());
                                        let key2 = key.clone();
                                        view! {
                                            <label class="pick">
                                                <input
                                                    type="checkbox"
                                                    prop:checked=checked
                                                    on:change=move |ev| {
                                                        if event_target_checked(&ev) {
                                                            picked.update(|s| {
                                                                s.insert(key.clone());
                                                            });
                                                        } else {
                                                            picked.update(|s| {
                                                                s.remove(&key);
                                                            });
                                                        }
                                                    }
                                                />
                                                <code>{key2}</code>
                                                {already
                                                    .then(|| view! { <span class="tag muted">"已在表里"</span> })}
                                            </label>
                                        }
                                    })
                                    .collect_view();
                                view! {
                                    <div class="pull-list">
                                        <div class="pull-head">
                                            <input
                                                type="text"
                                                prop:value=move || pull_filter.get()
                                                placeholder="按关键字过滤，如 deepseek"
                                                on:input=move |ev| pull_filter.set(event_target_value(&ev))
                                            />
                                            <button class="ghost" on:click=select_shown>"全选当前显示"</button>
                                            <button
                                                class="ghost"
                                                on:click=move |_| picked.set(BTreeSet::new())
                                            >
                                                "全部取消"
                                            </button>
                                            <button class="ghost" on:click=add_picked>
                                                "把勾选的加进下表"
                                            </button>
                                            <span class="muted small">
                                                {format!(
                                                    "共 {total} 个，列出 {shown_n} 个，已勾选 {picked_n} 个",
                                                )}
                                            </span>
                                        </div>
                                        <div class="pull-rows">{rows}</div>
                                        {(total > shown_n)
                                            .then(|| {
                                                view! {
                                                    <p class="muted small">
                                                        "列表只列出了前面这些 —— 用上面的过滤框找具体的名字（勾选状态不会丢）。"
                                                    </p>
                                                }
                                            })}
                                    </div>
                                }
                                    .into_any()
                            }}

                            <p class="muted small">
                                "同一条渠道对不同模型组可能要用不同的名字（比如这条渠道要"
                                <code>"deepseek/deepseek-v4.1-flash"</code>
                                "，别的渠道只要 " <code>"v4.1flash"</code>
                                "）。左填客户端打的模型名，右填这条渠道要发的名字；右边留空 = 同名发出去。"
                            </p>
                            <p class="muted small">
                                "「能看图」= 这个模型名收得下图片输入。"
                                <b>"别猜名字，点「测视觉」"</b>
                                "：它会真发一张图问一个只有看图才答得出的问题（左上角什么颜色），"
                                "答对了才勾上。带图的请求只会发给勾了的渠道；一条都没勾就回 422 说清楚，"
                                "而不是把图悄悄丢掉。"
                            </p>
                            <table class="grid tight">
                                <tbody>
                                    {move || {
                                        form_ver.get();
                                        let v = f_models.get_untracked();
                                        if v.is_empty() {
                                            return view! {
                                                <tr>
                                                    <td class="muted">
                                                        "没配 → 客户端打什么名字就原样发给上游"
                                                    </td>
                                                </tr>
                                            }.into_any();
                                        }
                                        v.iter()
                                            .enumerate()
                                            .map(|(i, m)| {
                                                let m = m.clone();
                                                let model0 = m.model.clone();
                                                let up0 = m.upstream.clone().unwrap_or_default();
                                                // 「测视觉」要知道这一行属于哪条渠道（表单里必须是已保存的渠道）
                                                let channel_for_test = match form.get_untracked() {
                                                    Form::Edit(id) => Some(id),
                                                    _ => None,
                                                };
                                                let model_for_test = m.model.clone();
                                                let up_for_test = m.upstream.clone().unwrap_or_default();
                                                // 结果行：这一条的渠道 id 对上了才显示
                                                let (vt_show, vt_text, vt_ok) = vt_msg
                                                    .get()
                                                    .map(|(id, t, ok)| (Some(id), t, ok))
                                                    .unwrap_or((None, String::new(), false));
                                                let show_vt = vt_show == channel_for_test.filter(|_| !vt_text.is_empty());
                                                view! {
                                                    <tr>
                                                        <td>
                                                            <input type="text" prop:value=model0
                                                                placeholder="客户端打的模型名"
                                                                on:input=move |ev| {
                                                                    let s = event_target_value(&ev);
                                                                    f_models
                                                                        .update(|v| {
                                                                            if let Some(x) = v.get_mut(i) {
                                                                                x.model = s;
                                                                            }
                                                                        });
                                                                } />
                                                        </td>
                                                        <td class="arrow">"→"</td>
                                                        <td>
                                                            <input type="text" prop:value=up0
                                                                placeholder="这条渠道要发的名字（留空=同名）"
                                                                on:input=move |ev| {
                                                                    let s = event_target_value(&ev);
                                                                    f_models
                                                                        .update(|v| {
                                                                            if let Some(x) = v.get_mut(i) {
                                                                                x.upstream = if s.trim().is_empty() {
                                                                                    None
                                                                                } else {
                                                                                    Some(s)
                                                                                };
                                                                            }
                                                                        });
                                                                } />
                                                        </td>
                                                        <td class="vision-cell">
                                                            <label class="pick" title="这个模型名能不能收图片输入（带图的请求只会发给勾了的渠道）">
                                                                <input type="checkbox"
                                                                    prop:checked=move || {
                                                                        f_models
                                                                            .get()
                                                                            .get(i)
                                                                            .map(|x| x.vision)
                                                                            .unwrap_or(false)
                                                                    }
                                                                    on:change=move |ev| {
                                                                        let on = event_target_checked(&ev);
                                                                        f_models
                                                                            .update(|v| {
                                                                                if let Some(x) = v.get_mut(i) {
                                                                                    x.vision = on;
                                                                                }
                                                                            });
                                                                    } />
                                                                "能看图"
                                                            </label>
                                                        </td>
                                                        <td>
                                                            <button
                                                                class="ghost"
                                                                disabled=move || vt_busy.get().is_some()
                                                                on:click=move |_| {
                                                                    match channel_for_test {
                                                                        Some(id) => {
                                                                            vision_test((
                                                                                id,
                                                                                model_for_test.clone(),
                                                                                up_for_test.clone(),
                                                                            ))
                                                                        }
                                                                        None => {
                                                                            ctx
                                                                                .say(
                                                                                    "这条渠道还没保存：先点「保存」，再测它认不认得图",
                                                                                )
                                                                        }
                                                                    }
                                                                }
                                                            >
                                                                "测视觉"
                                                            </button>
                                                        </td>
                                                        <td>
                                                            <button class="ghost danger" on:click=move |_| {
                                                                f_models.update(|v| { v.remove(i); });
                                                                form_ver.update(|v| *v += 1);
                                                            }>"删"</button>
                                                        </td>
                                                    </tr>
                                                    {show_vt.then(|| view! {
                                                        <tr class="probe-row">
                                                            <td></td>
                                                            <td colspan="5" class=if vt_ok { "probe ok" } else { "probe bad" }>
                                                                {vt_text.clone()}
                                                            </td>
                                                        </tr>
                                                    })}
                                                }
                                            })
                                            .collect_view()
                                            .into_any()
                                    }}
                                </tbody>
                            </table>
                            <button class="ghost" on:click=move |_| {
                                f_models.update(|v| v.push(ModelPair::default()));
                                form_ver.update(|v| *v += 1);
                            }>"加一条模型名"</button>

                            <div class="modal-foot">
                                <button class="primary" on:click=save>"保存"</button>
                                {editing
                                    .then(|| {
                                        view! { <button class="ghost danger" on:click=clear_key>"清空密钥"</button> }
                                    })}
                                <button class="ghost" on:click=move |_| form.set(Form::Closed)>
                                    "取消"
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn filter_is_case_insensitive_and_limited() {
        let all = names(&["deepseek-v4.1-flash", "BGE-M3", "gpt-oss"]);
        assert_eq!(filter_models(&all, "", 10).len(), 3, "不过滤就全列");
        assert_eq!(filter_models(&all, "deep", 10), names(&["deepseek-v4.1-flash"]));
        assert_eq!(filter_models(&all, "BGE", 10), names(&["BGE-M3"]), "过滤不分大小写");
        assert_eq!(filter_models(&all, "  gpt  ", 10), names(&["gpt-oss"]), "两头空白忽略");
        assert_eq!(filter_models(&all, "", 2).len(), 2, "显示有上限，免得塞爆 DOM");
        assert!(filter_models(&all, "没有这个名字", 10).is_empty());
    }

    #[test]
    fn adding_never_duplicates_and_never_adds_blanks() {
        let mut rows = vec![ModelPair {
            model: "bge-m3".into(),
            upstream: Some("x".into()),
            vision: true,
        }];
        let mut picked = BTreeSet::new();
        picked.insert("bge-m3".to_string()); // 表里已经有了
        picked.insert("   ".to_string()); // 空白，跳过
        picked.insert("deepseek-v4.1-flash".to_string());

        assert_eq!(add_picked_rows(&mut rows, &picked), 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].model, "deepseek-v4.1-flash");
        assert_eq!(rows[1].upstream, None, "加进来的默认同名发出去");
        assert_eq!(rows[0].upstream.as_deref(), Some("x"), "已有的行不许被动过");
        // 从上游清单加进来的行**不许猜能力**：一律先当纯文本，要标得实测
        assert!(!rows[1].vision, "拉回来的模型名不能自动当能看图");
        assert!(rows[0].vision, "已有行的标记不许被动过");

        // 连点两次不该堆重复行
        assert_eq!(add_picked_rows(&mut rows, &picked), 0);
        assert_eq!(rows.len(), 2);
    }
}
