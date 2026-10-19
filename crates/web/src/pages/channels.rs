//! 渠道管理：增删改查 + 自定义请求头 + 按渠道改模型名 + 「测一下」连通性 + 拉上游模型清单。
//!
//! 三条纪律都落在界面上：
//!
//! - **「测一下」不给假绿点**：真发一个请求，连不上/上游不收都如实报红，并写清原因；
//!   这条渠道没配模型名就直接说"探不了"，而不是拿虚拟组名 `auto` 去凑一个 400。
//! - **密钥留空 = 不改**：编辑一条渠道时密钥框留空表示沿用已存的；想清掉要**明确**点
//!   「清空密钥」。不然每次改个名字都得重输一遍密钥。
//! - **清单是上游说的，不是我们编的**：拉不到就只说实话（连不上/HTTP 404/格式没认出），
//!   绝不给一份看着挺像的假清单。上游报的**模型说明**（归属方、上下文长度、价钱、
//!   它自称能不能收图）一并显示；它没说的字段，界面上一个标签都不标。
//! - **「能不能看图」只认实测**：模型名里带 vl/vision 不代表能看图（deepseek 的 flash
//!   就能读图，有些带 vision 的名字反倒是纯文本）。所以每一条上都有「测一下能不能看图」：
//!   真发一张图问一个只有看图才答得出的问题（左上角什么颜色），答对了才勾上。
//!   勾了才有资格接带图的请求 —— 没勾的渠道，带图请求一条都不会发给它。
//!
//! ## 编辑弹窗（v0.5.0 重做）
//!
//! 弹窗顶上一排切换条，分三段：**基本信息**（名称/地址/密钥/启用/响应解包）、
//! **请求头**、**模型**。为什么分三段：三段各自都能很长，平铺在一屏里手机上一定
//! 看不到底（用户报过"这些内容看不见"），切换条把"哪一段有活干"摆在最上面。
//!
//! 「模型」那一段是主角，一条规则贯穿始终：
//!
//! > **一行 = 一条映射，键是"客户端里写的名字"。勾着 = 这条渠道认这个名字。**
//!
//! 它把原先的两步走合成一步：勾选状态本身就是保存内容（以前要"先在清单里勾、
//! 再点一下加进下表"，勾了忘了加就等于白勾）；上游给的名字和说明直接挂在勾选项上；
//! 客户端名字跟上游名字不一样时（`auto` → `v4.1flash` 这种），用「加一个自己的名字」
//! 加一条，再在它的「详情」里填上游实际叫什么。

use leptos::prelude::*;

use crate::api;
use crate::fmt;
use crate::model::{Channel, HeaderPair, ModelPair, UpstreamModel};
use crate::pages::PageHead;
use crate::Ctx;

/// 上游清单一次最多显示这么多行（真有上游报上千个模型，全塞进 DOM 会卡）。
const SHOW_LIMIT: usize = 200;

/// 弹窗三段的名字（`tab` 信号里存的就是这几个字面量）。
const TAB_BASIC: &str = "basic";
const TAB_HEADERS: &str = "headers";
const TAB_MODELS: &str = "models";

fn catalog_model_kind<'a>(m:&'a UpstreamModel,known:&'a [crate::model::NameRoute])->&'a str {
 let kind=m.kind.as_deref().filter(|k|!k.is_empty()).or_else(||known.iter().find(|r|tokengo_shared::same_model_name(&r.name,&m.id)).map(|r|r.kind.as_str())).unwrap_or("");
 match kind {"chat"|"embeddings"|"rerank"|"image"|"audio"|"video"=>kind,_=>"unknown"}
}

/// 按关键字过滤上游报的模型名（大小写不敏感），最多取 `limit` 个。
pub(crate) fn filter_catalog(all: &[UpstreamModel], filter: &str, limit: usize) -> Vec<UpstreamModel> {
    let needle = filter.trim().to_lowercase();
    all.iter()
        .filter(|m| needle.is_empty() || m.id.to_lowercase().contains(&needle))
        .take(limit)
        .cloned()
        .collect()
}

/// 表里「客户端名字 = client」的那一条映射。
pub(crate) fn mapping_for<'a>(rows: &'a [ModelPair], client: &str) -> Option<&'a ModelPair> {
    rows.iter().find(|m| m.model.trim() == client.trim())
}

/// 这条渠道认不认这个名字（= 表里有没有这条映射）。面板上的勾就认它。
pub(crate) fn is_in_use(rows: &[ModelPair], client: &str) -> bool {
    mapping_for(rows, client).is_some()
}

/// 加一条映射：客户端名字 = `name`，上游同名（上游那栏留空 = 同名发出去）。
/// 已经有了、或者名字是空的就不动，回 `false`。
pub(crate) fn add_mapping(rows: &mut Vec<ModelPair>, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || is_in_use(rows, name) {
        return false;
    }
    rows.push(ModelPair {
        model: name.to_string(),
        upstream: None,
        // 刚加进来的名字**不许猜能力**：要标能看图得点这一条的「测一下能不能看图」。
        vision: false,
        // 要丢的字段也一样不许猜：默认一个字段都不丢。
        drop_fields: String::new(),
    });
    true
}

/// 批量加（「全选当前显示的」）。回真正加了几条。
pub(crate) fn add_many(rows: &mut Vec<ModelPair>, names: &[String]) -> usize {
    let mut n = 0;
    for name in names {
        if add_mapping(rows, name) {
            n += 1;
        }
    }
    n
}

/// 删掉「客户端名字 = client」的映射，回删了几条。
pub(crate) fn drop_mapping(rows: &mut Vec<ModelPair>, client: &str) -> usize {
    let before = rows.len();
    rows.retain(|m| m.model.trim() != client.trim());
    before - rows.len()
}

/// 这一条额外配过东西吗（能看图 / 要丢的字段）—— 取消勾选前要提醒一句，
/// 不然"顺手把配置删掉了"看不出来。
pub(crate) fn mapping_has_config(rows: &[ModelPair], client: &str) -> bool {
    mapping_for(rows, client)
        .map(|m| m.vision || !m.drop_fields.trim().is_empty())
        .unwrap_or(false)
}

/// 同一个客户端名字出现两次的名字（保存前拦下来：一个名字只能有一条映射）。
pub(crate) fn duplicate_client_names(rows: &[ModelPair]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut dup: Vec<String> = Vec::new();
    for m in rows {
        let n = m.model.trim().to_string();
        if n.is_empty() {
            continue;
        }
        if seen.contains(&n) {
            if !dup.contains(&n) {
                dup.push(n);
            }
        } else {
            seen.push(n);
        }
    }
    dup
}

/// 面板上「同时最多几发」那一格：填了数字就用它，填了乱七八糟的东西就当 0（不限），
/// 不能因为一格填错让整张表单存不下去。
fn parse_cap(text: &str) -> u32 {
    text.trim().parse::<u32>().unwrap_or(0)
}

/// 表单 → 要提交的映射表：两头空白去掉、空名字的行丢掉，并如实回报丢了几条。
pub(crate) fn mappings_to_save(rows: &[ModelPair]) -> (Vec<ModelPair>, usize) {
    let mut out: Vec<ModelPair> = Vec::new();
    let mut skipped = 0usize;
    for m in rows {
        let client = m.model.trim();
        if client.is_empty() {
            skipped += 1;
            continue;
        }
        let upstream = m
            .upstream
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        out.push(ModelPair {
            model: client.to_string(),
            upstream,
            vision: m.vision,
            drop_fields: m.drop_fields.clone(),
        });
    }
    (out, skipped)
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

    // 弹窗停在哪一段
    let tab = RwSignal::new(TAB_BASIC.to_string());

    // 「拉取上游模型清单」的状态
    let pulled = RwSignal::new(Vec::<UpstreamModel>::new());
    let pull_msg = RwSignal::new(None::<(String, bool)>);
    let pulling = RwSignal::new(false);
    // 上次问到上游清单是什么时候（毫秒；0 = 从来没问过）。
    // 打开弹窗先读库把它显示出来 —— 不用为了看一眼清单又去骚扰一次上游。
    let cat_at = RwSignal::new(0i64);
    let pull_filter = RwSignal::new(String::new());
    let catalog_kind=RwSignal::new("all".to_string());
    let known_kinds=RwSignal::new(Vec::<crate::model::NameRoute>::new());
    leptos::task::spawn_local(async move {if let Ok(r)=api::model_routes().await {known_kinds.set(r.models);}});
    let visible_catalog=move ||{
        let known=known_kinds.get();let selected=catalog_kind.get();
        pulled.get().into_iter().filter(|m|selected=="all" || catalog_model_kind(m,&known)==selected).collect::<Vec<_>>()
    };
    // 「加一个自己的名字」输入框里正打着的字
    // 模型那一段的结构变化（勾/取消/加/删/拉到清单）后 +1 —— 只有它会重排那一列。
    // **打字不重排**，否则正在输入的框会丢焦点。
    let mver = RwSignal::new(0u32);
    // 哪一条的「详情」摊开了（键 = 客户端名字）
    let m_open = RwSignal::new(None::<String>);

    // 「测一下能不能看图」的状态：(渠道 id, 给人看的结果, 好不好)
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
    // 这条渠道的非流式响应要不要剥壳（cline.bot 那种 `{"data": {…}, "success": true}`）
    let f_unwrap = RwSignal::new(false);
    // 这条渠道「同时最多几发」：0 = 不限。填了就在网关门口排队/换渠道，
    // 免得一次铺开几十发把上游那把 key 打成 429（上游多数是按凭据限并发的）。
    let f_cap = RwSignal::new(0u32);
    // 输入框是文本，提交时再解析成数字（RwSignal<String> 免得打字时被反复 parse 打断）
    let f_cap_text = RwSignal::new("0".to_string());
    // 表单行的重绘开关：只有加/删行才 +1。**打字时不重绘**，否则输入框会丢焦点。
    let form_ver = RwSignal::new(0u32);

    // 重新进表单：清掉上一次拉的清单，免得看着像这条渠道的
    let reset_pull = move || {
        pulled.set(Vec::new());
        cat_at.set(0);
        pull_msg.set(None);
        pull_filter.set(String::new());
        m_open.set(None);
        mver.update(|v| *v += 1);
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
        f_unwrap.set(false);
        f_cap.set(0);
        f_cap_text.set("0".to_string());
        reset_pull();
        form_ver.update(|v| *v += 1);
        tab.set(TAB_BASIC.to_string());
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
        f_unwrap.set(l.unwrap_envelope);
        f_cap.set(l.max_concurrent);
        f_cap_text.set(l.max_concurrent.to_string());
        reset_pull();
        form_ver.update(|v| *v += 1);
        // 编辑已有渠道：直接停在「模型」那一栏 —— 这一栏才是「挑这条渠道认哪些名字、
        // 去上游拉清单」的地方，停在「基本信息」会让人以为没处可拉（用户实测反馈过）。
        tab.set(TAB_MODELS.to_string());
        form.set(Form::Edit(l.id));
        // 打开就把它**上次问到**的清单读出来（读库，不联网）：省一次上游请求
        // （有的上游要花钱、有限流），也让人一眼看见「上次问到几点」。
        // 要刷新得点上面那个按钮 —— 面板不替你偷偷问上游。
        let id = l.id;
        wasm_bindgen_futures::spawn_local(async move {
            let Ok(r) = api::channel_catalog(id).await else {
                return;
            };
            if form.get_untracked() != Form::Edit(id) {
                return; // 这期间又打开了别的渠道/新表单，别把它的界面覆盖掉
            }
            cat_at.set(r.fetched_at);
            pulled.set(if r.info.is_empty() {
                r.models
                    .into_iter()
                    .map(|id| UpstreamModel { id, ..Default::default() })
                    .collect()
            } else {
                r.info
            });
            mver.update(|v| *v += 1);
        });
    };

    let save = move |_| {
        let name = f_name.get_untracked().trim().to_string();
        let url = f_url.get_untracked().trim().to_string();
        if name.is_empty() || url.is_empty() {
            ctx.say("名称和接口地址都得填");
            return;
        }
        let dup = duplicate_client_names(&f_models.get_untracked());
        if !dup.is_empty() {
            ctx.say(format!(
                "一个客户端名字只能有一条：「{}」出现了两次，改掉再保存",
                dup.join("」「")
            ));
            return;
        }
        let key = f_key.get_untracked();
        let id = match form.get_untracked() {
            Form::Edit(i) => Some(i),
            _ => None,
        };
        let headers = f_headers.get_untracked();
        let (models, skipped) = mappings_to_save(&f_models.get_untracked());
        let enabled = f_enabled.get_untracked();
        let unwrap = f_unwrap.get_untracked();
        let cap = parse_cap(&f_cap_text.get_untracked());
        // 编辑时留空 = 不改（传 None）；新建时空 = 这条渠道没密钥（也传 None，后端一样）
        let key_arg = if key.trim().is_empty() { None } else { Some(key) };

        wasm_bindgen_futures::spawn_local(async move {
            match api::save_channel(id, &name, &url, enabled, unwrap, cap, key_arg.as_deref(), &headers, &models)
                .await
            {
                Ok(new_id) => {
                    let tail = if skipped > 0 {
                        format!("（有 {skipped} 条没填客户端名字，没存）")
                    } else {
                        String::new()
                    };
                    // v0.11.0 起「勾上就能用」：勾了就出现在「模型」页、客户端马上能打这个名字。
                    // 没有第二层（模型组）要建，所以这里保存完就完事。
                    let made_note = String::new();
                    match new_id {
                        // 新建：**表单不关**，直接切成"编辑这条新渠道"并跳到「模型」那一段 ——
                        // 这样能马上用它的 URL/密钥去问上游有哪些模型，不用先关再点编辑
                        Some(i) => {
                            form.set(Form::Edit(i));
                            f_key.set(String::new());
                            f_key_note.set("留空 = 沿用已存的密钥；要清掉请点「清空密钥」。");
                            tab.set(TAB_MODELS.to_string());
                            ctx.say(format!("渠道已新建{tail}{made_note} —— 接着点「获取上游模型」"));
                        }
                        None => {
                            form.set(Form::Closed);
                            ctx.say(format!("渠道已保存{tail}{made_note}"));
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
        let Form::Edit(id) = form.get_untracked() else {
            return;
        };
        let name = f_name.get_untracked();
        let url = f_url.get_untracked();
        let enabled = f_enabled.get_untracked();
        let unwrap = f_unwrap.get_untracked();
        let cap = parse_cap(&f_cap_text.get_untracked());
        let headers = f_headers.get_untracked();
        let (models, _) = mappings_to_save(&f_models.get_untracked());
        wasm_bindgen_futures::spawn_local(async move {
            // 传空串 = 明确清掉密钥
            match api::save_channel(Some(id), &name, &url, enabled, unwrap, cap, Some(""), &headers, &models).await {
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

    // 上移 / 下移一位。顺序全站只有一份（`channels.priority`）：一个模型名下先试哪条、
    // 「模型」页上显示的顺序，都跟着它走 —— 所以只在「渠道」页调。
    let move_one = move |l: Channel, up: bool| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::move_channel(l.id, up).await {
                Ok(()) => reload(),
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("挪不动：{}", e.message));
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

    // 一键启用/停用：卡片右上角那个开关点的就是它。只改这一列，密钥/请求头/模型名都不碰。
    let toggle_enabled = move |l: Channel| {
        let want = !l.enabled;
        let (id, name) = (l.id, l.name.clone());
        wasm_bindgen_futures::spawn_local(async move {
            match api::set_channel_enabled(id, want).await {
                Ok(()) => {
                    ctx.say(if want {
                        format!("渠道「{name}」已启用")
                    } else {
                        format!("渠道「{name}」已停用 —— 不会再往这家上游发请求")
                    });
                    reload();
                }
                Err(e) => {
                    ctx.note_err(&e);
                    ctx.say(format!("{}没成功：{}", if want { "启用" } else { "停用" }, e.message));
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
                    s.push_str(if r.ok { "通了" } else { "没通" });
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

    // 「测一下能不能看图」：给这一条真发一张带图的小请求，看这条渠道认不认得出来。
    //
    // 为什么要真发：我们不像上游 FreeLLMAPI 那样有一份带能力标记的目录
    // （本项目的上游全是自己加的），而"名字里带 vl"这种推断经常错。
    // 测通了我们顺手把这行标上（服务端的 apply），界面上勾也随之亮起。
    let vision_test = move |(channel_id, model, upstream): (i64, String, String)| {
        if model.trim().is_empty() {
            ctx.say("这一条没有客户端名字，先填上再测");
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
                    if r.retried_with_more_tokens {
                        s.push_str("；第一次给的额度不够、回复被截断，已给更多额度重问过");
                    } else if !r.ok {
                        if let Some(t) = r.max_tokens {
                            s.push_str(&format!("；这次给了 {t} 输出额度"));
                        }
                    }
                    if let Some(reply) = r.reply.as_deref().filter(|x| !x.trim().is_empty()) {
                        s.push_str(&format!("；它回的话「{reply}」"));
                    }
                    if let Some(n) = &r.note {
                        s.push_str(&format!("；{n}"));
                    }
                    if r.ok {
                        // 实测通过就是通过 —— 先在屏幕上勾上。服务端能顺手落库就落
                        // （applied）；没落（比如这一条还没保存过）也要说清楚差哪一步，
                        // 别让人以为已经记下了。
                        f_models.update(|v| {
                            if let Some(x) = v.iter_mut().find(|x| x.model.trim() == model.trim()) {
                                x.vision = true;
                            }
                        });
                        if r.applied {
                            s.push_str("；已顺手勾上「能看图」");
                        } else {
                            s.push_str("；先在屏幕上勾上了「能看图」—— 这一条还没存过，点「保存」写进网关");
                        }
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

    // 拉这条渠道的上游模型清单：真打一次 GET /v1/models，连名字带说明一起拿回来。
    let pull = move |_| {
        let Form::Edit(id) = form.get_untracked() else {
            ctx.say("这条渠道还没保存：先点「保存」，再用它自己的 URL 和密钥去问上游");
            return;
        };
        pulling.set(true);
        pull_msg.set(Some(("正在问上游要模型清单…".into(), true)));
        wasm_bindgen_futures::spawn_local(async move {
            match api::pull_channel_models(id).await {
                Ok(r) => {
                    pulling.set(false);
                    let mut s=if r.ok {format!("已获取 {} 个模型",r.count)}else{"获取失败".into()};
                    if r.ok {
                        cat_at.set(r.fetched_at);
                        if let Some(ms)=r.latency_ms{s.push_str(&format!(" · {}",fmt::dur(ms)));}
                        if !r.added.is_empty() || !r.removed.is_empty(){s.push_str(&format!(" · 新增 {} / 减少 {}",r.added.len(),r.removed.len()));}
                    } else {
                        if let Some(code)=r.http_status{s.push_str(&format!(" · HTTP {code}"));}
                        if let Some(note)=&r.note{s.push_str(&format!("：{note}"));}
                    }
                    pull_msg.set(Some((s, r.ok)));
                    pull_filter.set(String::new());
                    // 勾选状态**不用重置**：它就是模型表里那几条映射。
                    // 拉清单只换"能看见什么"（上游报的名字 + 说明）。
                    pulled.set(if r.info.is_empty() {
                        r.models
                            .into_iter()
                            .map(|id| UpstreamModel { id, ..Default::default() })
                            .collect()
                    } else {
                        r.info
                    });
                    mver.update(|v| *v += 1);
                }
                Err(e) => {
                    pulling.set(false);
                    ctx.note_err(&e);
                    pull_msg.set(Some((format!("拉不了：{}", e.message), false)));
                }
            }
        });
    };

    // 「全选当前显示的」：把清单里现在显示的那些名字都加进模型表。
    let select_shown = move |_| {
        let names: Vec<String> =
            filter_catalog(&visible_catalog(), &pull_filter.get_untracked(), SHOW_LIMIT)
                .into_iter()
                .map(|m| m.id)
                .collect();
        if names.is_empty() {
            ctx.say("现在没有可勾的名字");
            return;
        }
        let added = {
            let mut v = f_models.get_untracked();
            let n = add_many(&mut v, &names);
            f_models.set(v);
            n
        };
        mver.update(|v| *v += 1);
        if added == 0 {
            ctx.say("现在显示的名字都已经在用着");
        } else {
            ctx.say(format!("加了 {added} 条 —— 要改上游实际叫的名字，点那一条的「详情」"));
        }
    };

    // 「取消当前显示的勾选」：只动清单里这次报的、现在显示着的那些。
    let unselect_shown = move |_| {
        let names: Vec<String> =
            filter_catalog(&visible_catalog(), &pull_filter.get_untracked(), SHOW_LIMIT)
                .into_iter()
                .map(|m| m.id)
                .collect();
        let mut removed = 0usize;
        {
            let mut v = f_models.get_untracked();
            for n in &names {
                removed += drop_mapping(&mut v, n);
            }
            f_models.set(v);
        }
        mver.update(|v| *v += 1);
        if removed == 0 {
            ctx.say("现在显示的名字本来就没在用");
        } else {
            ctx.say(format!("取消了 {removed} 条（这些名字这条渠道不再认）"));
        }
    };

    view! {
        <div class="page">
            <PageHead
                title="渠道"
                desc="各个上游的地址、密钥，和它认哪些模型。"
            >
                <>
                    <button class="primary" on:click=open_new>"新建渠道"</button>
                    <button class="ghost" disabled=move || busy.get() on:click=move |_| reload()>
                        {move || if busy.get() { "刷新中…" } else { "刷新" }}
                    </button>
                </>
            </PageHead>

            <div class="toolbar">
                <span class="seg">
                    <span class="seg-label">"「测一下」发的是"</span>
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
                </span>
            </div>

            {move || err.get().map(|e| view! { <div class="banner bad">{e}</div> })}

            <div class="chan-list">
                {move || {
                    let v = rows.get();
                    if v.is_empty() {
                        return view! {
                            <div class="empty">
                                <p>"还没有渠道。"</p>
                                <p>"点右上角「新建渠道」加一条上游 —— 填上它的地址和密钥，就能把请求转过去了。"</p>
                            </div>
                        }.into_any();
                    }
                    let total = v.len();
                    v.into_iter()
                        .enumerate()
                        .map(|(idx, l)| {
                            let l2 = l.clone();
                            let l3 = l.clone();
                            let l4 = l.clone();
                            let l_up = l.clone();
                            let l_down = l.clone();
                            let first_one = idx == 0;
                            let last_one = idx + 1 == total;
                            let (pid, pmsg, pok) = probe_msg
                                .get()
                                .map(|(i, m, ok)| (Some(i), m, ok))
                                .unwrap_or((None, String::new(), false));
                            let show_probe = pid == Some(l.id) && !pmsg.is_empty();
                            // 摘要跟路由口径一致（严格白名单）：只认勾上的那些；一个都没勾 =
                            // 这条渠道现在什么都不能用。
                            let model_txt = if l.models.is_empty() {
                                "一个都没勾（现在不能用）".to_string()
                            } else if l.vision_count > 0 {
                                format!("只认这 {} 个（{} 条能看图）", l.models.len(), l.vision_count)
                            } else {
                                format!("只认这 {} 个", l.models.len())
                            };
                            view! {
                                <div class="chan-card">
                                    <div class="chan-head">
                                        <span class="chan-id">
                                            "第 " {idx + 1} " 位"
                                        </span>
                                        <b class="chan-name">{l.name.clone()}</b>
                                        <span class="spacer"></span>
                                        {
                                            let (cls, tip) = if l.enabled {
                                                ("switch on", "点一下停用：不会再往这家上游发请求")
                                            } else {
                                                ("switch", "点一下启用")
                                            };
                                            let tg = toggle_enabled.clone();
                                            let l2 = l.clone();
                                            view! {
                                                <button class=cls title=tip on:click=move |_| tg(l2.clone())>
                                                    <span class="knob"></span>
                                                    <span class="switch-text">
                                                        {if l.enabled { "启用" } else { "停用" }}
                                                    </span>
                                                </button>
                                            }
                                        }
                                    </div>
                                    <div class="chan-url">
                                        <code class="small">{l.base_url.clone()}</code>
                                    </div>
                                    <div class="chan-meta">
                                        <span>
                                            "密钥："
                                            {if l.has_secret {
                                                view! { <span class="tag ok">"有"</span> }.into_any()
                                            } else {
                                                view! { <span class="tag warn">"没有"</span> }.into_any()
                                            }}
                                        </span>
                                        <span>{format!("请求头：{}", l.headers.len())}</span>
                                        <span>{format!("模型名：{model_txt}")}</span>
                                        {l.unwrap_envelope.then(|| view! {
                                            <span class="tag">"响应解包"</span>
                                        })}
                                        {(l.max_concurrent > 0).then(|| view! {
                                            <span class="tag">{format!("同时最多 {} 发", l.max_concurrent)}</span>
                                        })}
                                    </div>
                                    <div class="chan-actions">
                                        <button
                                            class="ghost"
                                            disabled=first_one
                                            title="往上挪一位：请求会更早试它"
                                            on:click=move |_| move_one(l_up.clone(), true)
                                        >
                                            "↑"
                                        </button>
                                        <button
                                            class="ghost"
                                            disabled=last_one
                                            title="往下挪一位：请求会更晚试它"
                                            on:click=move |_| move_one(l_down.clone(), false)
                                        >
                                            "↓"
                                        </button>
                                        <button class="ghost" on:click=move |_| open_edit(l2.clone())>
                                            "编辑"
                                        </button>
                                        <button class="ghost" on:click=move |_| probe(l3.clone())>
                                            "测一下"
                                        </button>
                                        <button class="ghost danger" on:click=move |_| remove(l4.clone())>
                                            "删除"
                                        </button>
                                    </div>
                                    {show_probe.then(|| view! {
                                        <div class=move || if pok { "chan-probe ok" } else { "chan-probe bad" }>
                                            {pmsg.clone()}
                                        </div>
                                    })}
                                </div>
                            }
                        })
                        .collect_view()
                        .into_any()
                }}
            </div>

            {move || if form.get() == Form::Closed {
                ().into_any()
            } else {
                let editing = matches!(form.get(), Form::Edit(_));
                let channel_id = match form.get() {
                    Form::Edit(i) => Some(i),
                    _ => None,
                };
                view! {
                    <div class="modal-backdrop">
                        <div class="modal channel-editor">
                            <div class="modal-head">
                                <h2>{if editing { "编辑渠道" } else { "新建渠道" }}</h2>
                                <span class="muted small">
                                    {move || {
                                        let models = f_models.get().len();
                                        let headers = f_headers.get().len();
                                        format!("{models} 条模型 · {headers} 条请求头")
                                    }}
                                </span>
                            </div>

                            <div class="modal-tabs">
                                <button
                                    class="tab"
                                    class:active=move || tab.get() == TAB_BASIC
                                    on:click=move |_| tab.set(TAB_BASIC.to_string())
                                >
                                    "基本信息"
                                </button>
                                <button
                                    class="tab"
                                    class:active=move || tab.get() == TAB_HEADERS
                                    on:click=move |_| tab.set(TAB_HEADERS.to_string())
                                >
                                    "请求头"
                                    <span class="tag muted">{move || f_headers.get().len()}</span>
                                </button>
                                <button
                                    class="tab"
                                    class:active=move || tab.get() == TAB_MODELS
                                    on:click=move |_| tab.set(TAB_MODELS.to_string())
                                >
                                    "模型"
                                    <span class="tag muted">{move || f_models.get().len()}</span>
                                </button>
                            </div>

                            <div class="modal-body">
                                // ———————— 基本信息 ————————
                                {move || if tab.get() != TAB_BASIC {
                                    ().into_any()
                                } else {
                                    view! {
                                        <p class="muted small">
                                            "这条渠道认哪些模型、怎么去上游拉清单，都在上面「模型」那一栏里。"
                                        </p>
                                        <div class="form-grid">
                                            <label>"名称"</label>
                                            <input type="text" prop:value=f_name placeholder="如：硅基流动"
                                                on:input=move |ev| f_name.set(event_target_value(&ev)) />
                                            <label>"接口地址"</label>
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
                                                <span class="muted small">"停用后不会再往这家上游发请求"</span>
                                            </div>
                                            <label>"响应解包"</label>
                                            <div>
                                                <input type="checkbox" prop:checked=f_unwrap
                                                    on:change=move |ev| f_unwrap.set(event_target_checked(&ev)) />
                                                <span class="muted small">
                                                    "这家上游把正常回复包了一层外壳时勾上（形状是 "
                                                    <code>"{\"data\": {…}, \"success\": true}"</code>
                                                    "）。只有认得出那种形状才会剥，认不出就原样回给你。"
                                                    "正常的上游不用勾。"
                                                </span>
                                            </div>
                                            <div class="field">
                                                <label>"同时最多几发"</label>
                                                <input type="text" inputmode="numeric" prop:value=f_cap_text
                                                    on:input=move |ev| f_cap_text.set(event_target_value(&ev)) />
                                                <span class="muted small">
                                                    "0 = 不限。填了就在门口排队：这条渠道同时最多跑这么多发，"
                                                    "多出来的先去别的渠道，全满才排队 —— "
                                                    "上游按 key 限并发时（免费额度多数如此）填它防 429。"
                                                </span>
                                            </div>
                                        </div>
                                    }.into_any()
                                }}

                                // ———————— 请求头 ————————
                                {move || if tab.get() != TAB_HEADERS {
                                    ().into_any()
                                } else {
                                    view! {
                                        <p class="muted small">
                                            "值支持模板：" <code>"{{uuid}}"</code> "（每次请求换新 UUID）、"
                                            <code>"{{timestamp_ms}}"</code> "（当前毫秒）、"
                                            <code>"{{session_id}}"</code>
                                            "（同一段对话里保持不变 —— 上游靠它认出\"这还是同一段对话\"，"
                                            "能少算一遍；探活和拉清单没有对话，这个头那时不会发出去）。"
                                        </p>
                                        <div class="table-wrap">
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
                                        </div>
                                        <button class="ghost" on:click=move |_| {
                                            f_headers.update(|v| v.push(HeaderPair::default()));
                                            form_ver.update(|v| *v += 1);
                                        }>"加一个请求头"</button>
                                    }.into_any()
                                }}

                                // ———————— 模型 ————————
                                {move || if tab.get() != TAB_MODELS {
                                    ().into_any()
                                } else {
                                    view! {
                                        <div class="pull-bar">
                                            // 这是「模型」段的主操作，用默认的描边按钮（.ghost 是纯文字，
                                            // 用户实测反馈「看着不像能点的」）
                                            <button
                                                disabled=move || pulling.get() || channel_id.is_none()
                                                on:click=pull
                                            >
                                                {move || if pulling.get() { "获取中…" } else { "获取上游模型" }}
                                            </button>
                                        </div>
                                        {channel_id.is_none().then(|| view! {
                                            <p class="muted small">
                                                "这条渠道还没保存，拿不到它的密钥去问 —— 先点下面的「保存」，弹窗会停在这一段。"
                                            </p>
                                        })}
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

                                        <nav class="catalog-kind-tabs">{[("all","全部"),("chat","对话"),("embeddings","嵌入"),("rerank","重排"),("image","图像"),("audio","音频"),("video","视频"),("unknown","待识别")].into_iter().map(|(k,label)|view!{<button class:active=move ||catalog_kind.get()==k on:click=move |_|catalog_kind.set(k.into())>{label}<small>{move ||{let known=known_kinds.get();pulled.get().iter().filter(|m|k=="all"||catalog_model_kind(m,&known)==k).count()}}</small></button>}).collect_view()}</nav>
                                        <div class="pull-head">
                                            <input
                                                type="text"
                                                prop:value=move || pull_filter.get()
                                                placeholder="按关键字过滤，如 deepseek"
                                                on:input=move |ev| pull_filter.set(event_target_value(&ev))
                                            />
                                            <button class="ghost" on:click=select_shown>"全选当前"</button>
                                            <button class="ghost" on:click=unselect_shown>"取消当前"</button>
                                            <span class="muted small">
                                                {move || {
                                                    let total = pulled.get().len();
                                                    let shown = filter_catalog(
                                                            &visible_catalog(),
                                                            &pull_filter.get_untracked(),
                                                            SHOW_LIMIT,
                                                        )
                                                        .len();
                                                    // 只数**屏幕上这几种行**里勾了几个 —— 以前数的是这条渠道
                                                    // 所有勾过的名字（含 auto 这类不在清单里的），于是「勾了 2 个」
                                                    // 会显示成「认 3 个」（用户 2026-10-18 报的）。
                                                    let shown_names = filter_catalog(
                                                        &visible_catalog(),
                                                        &pull_filter.get_untracked(),
                                                        SHOW_LIMIT,
                                                    );
                                                    let checked = shown_names
                                                        .iter()
                                                        .filter(|m| is_in_use(&f_models.get(), &m.id))
                                                        .count();
                                                    if total == 0 {
                                                        // 还没有清单：这行没什么可数的（列表自己会说怎么办）
                                                        String::new()
                                                    } else {
                                                        // 清单是什么时候拿的：做成行内的一个括注，
                                                        // 不再单起一段说明文字
                                                        let at = cat_at.get();
                                                        let now = crate::now_ms();
                                                        let when = if at <= 0 {
                                                            String::new()
                                                        } else if at >= now {
                                                            "刚刚".to_string()
                                                        } else {
                                                            fmt::ago(now, at)
                                                        };
                                                        if when.is_empty() {
                                                            format!(
                                                                "上游报 {total} 个 · 列出 {shown} 个 · 勾了 {checked} 个",
                                                            )
                                                        } else {
                                                            format!(
                                                                "上游报 {total} 个（{when}）· 列出 {shown} 个 · 勾了 {checked} 个",
                                                            )
                                                        }
                                                    }
                                                }}
                                            </span>
                                        </div>

                                        <div class="m-list">
                                            {move || {
                                                // 结构性的东西：清单 + 过滤 + mver（勾/取消/加/删之后重排）。
                                                // 打字**不**在这儿重排 —— 输入框在「详情」里，重排会丢焦点。
                                                mver.get();
                                                let catalog = visible_catalog();
                                                let filter = pull_filter.get();
                                                // 这份清单只放**上游报回来的名字**（用户 2026-10-18：
                                                // 上游没有的名字不许混进来 —— 虚拟组名 auto 这类会让人
                                                // 以为上游真有这个模型）。自己认的名字不在这一列显示。
                                                let names: Vec<String> =
                                                    filter_catalog(&catalog, &filter, SHOW_LIMIT)
                                                        .into_iter()
                                                        .map(|m| m.id)
                                                        .collect();
                                                if names.is_empty() {
                                                    return view! {
                                                        <p class="muted small">
                                                            "还没有模型名，请先获取上游模型。"
                                                        </p>
                                                    }.into_any();
                                                }
                                                names
                                                    .into_iter()
                                                    .map(|name| {
                                                        let info = catalog.iter().find(|c| c.id == name).cloned();
                                                        let owner = info.as_ref().and_then(|i| i.owned_by.clone());
                                                        let ctx_len = info.as_ref().and_then(|i| i.context_window);
                                                        let price = info
                                                            .as_ref()
                                                            .and_then(|i| i.price_note.clone().map(|p| (p, i.is_free)));
                                                        let vision_claim = info.as_ref().and_then(|i| i.vision);
                                                        // 每条闭包各拿一份克隆，别互相移动
                                                        let n_ck = name.clone();
                                                        let n_add = name.clone();
                                                        let n_del_ck = name.clone();
                                                        let n_open = name.clone();
                                                        let n_close_btn = name.clone();
                                                        let n_close_body = name.clone();
                                                        let n_field = name.clone();
                                                        let n_vis_ck = name.clone();
                                                        let n_vis_ch = name.clone();
                                                        let n_test = name.clone();
                                                        let cur = f_models
                                                            .get_untracked()
                                                            .into_iter()
                                                            .find(|m| m.model.trim() == name.trim());
                                                        let cur_up =
                                                            cur.as_ref().and_then(|m| m.upstream.clone()).unwrap_or_default();
                                                        let open_me = move |_| {
                                                            if m_open.get_untracked().as_deref()
                                                                == Some(n_open.as_str())
                                                            {
                                                                m_open.set(None);
                                                            } else {
                                                                m_open.set(Some(n_open.clone()));
                                                            }
                                                        };
                                                        let is_open_btn = move || {
                                                            m_open.get().as_deref() == Some(n_close_btn.as_str())
                                                        };
                                                        let is_open_body = move || {
                                                            m_open.get().as_deref() == Some(n_close_body.as_str())
                                                        };
                                                        let n_on = name.clone();
                                                        view! {
                                                            <div
                                                                class="m-row"
                                                                class:on=move || is_in_use(&f_models.get(), &n_on)
                                                            >
                                                                <div class="m-col">
                                                                    <div class="m-line">
                                                                        <label
                                                                            class="m-pick"
                                                                            title="勾上 = 这条渠道认这个名字"
                                                                        >
                                                                        <input
                                                                            type="checkbox"
                                                                            prop:checked=move || {
                                                                                is_in_use(&f_models.get(), &n_ck)
                                                                            }
                                                                            on:change=move |ev| {
                                                                                if event_target_checked(&ev) {
                                                                                    f_models
                                                                                        .update(|v| {
                                                                                            add_mapping(v, &n_add);
                                                                                        });
                                                                                    mver.update(|v| *v += 1);
                                                                                    ctx.say("加上了 —— 记得点「保存」");
                                                                                } else {
                                                                                    let had = mapping_has_config(
                                                                                        &f_models.get_untracked(),
                                                                                        &n_del_ck,
                                                                                    );
                                                                                    let go = !had
                                                                                        || web_sys::window()
                                                                                            .and_then(|w| {
                                                                                                w
                                                                                                    .confirm_with_message(
                                                                                                        &format!(
                                                                                                            "「{n_del_ck}」这一条配过「能看图」或「要丢的字段」，取消勾选会一起删掉。继续？",
                                                                                                        ),
                                                                                                    )
                                                                                                    .ok()
                                                                                            })
                                                                                            .unwrap_or(false);
                                                                                    if go {
                                                                                        f_models
                                                                                            .update(|v| {
                                                                                                drop_mapping(v, &n_del_ck);
                                                                                            });
                                                                                        if m_open.get_untracked().as_deref()
                                                                                            == Some(n_del_ck.as_str())
                                                                                        {
                                                                                            m_open.set(None);
                                                                                        }
                                                                                        mver.update(|v| *v += 1);
                                                                                        ctx.say("取消了 —— 记得点「保存」");
                                                                                    }
                                                                                }
                                                                            }
                                                                        />
                                                                        </label>
                                                                        <code class="m-name">{name.clone()}</code>
                                                                        <span class="spacer"></span>
                                                                        <button class="ghost" on:click=open_me>
                                                                            {move || if is_open_btn() { "收起" } else { "详情" }}
                                                                        </button>
                                                                    </div>
                                                                    <div class="m-tags">
                                                                    {owner
                                                                        .map(|o| view! { <span class="tag muted">{o}</span> })}
                                                                    {ctx_len
                                                                        .map(|c| {
                                                                            view! {
                                                                                <span class="tag" title="上游声明的上下文长度，非容量实测">
                                                                                    {format!("上下文 {} · 上游声明", fmt::ctx(c))}
                                                                                </span>
                                                                            }
                                                                        })}
                                                                    {price
                                                                        .map(|(p, free)| {
                                                                            let cls = if free { "tag ok" } else { "tag muted" };
                                                                            view! { <span class=cls title="上游报的价钱">{p}</span> }
                                                                        })}
                                                                    {match vision_claim {
                                                                        Some(true) => {
                                                                            view! {
                                                                                <span class="tag" title="上游声明支持图片输入，自动关联到本渠道对应的上游模型；不是视觉实测结果">
                                                                                    "图片输入 · 上游声明"
                                                                                </span>
                                                                            }
                                                                                .into_any()
                                                                        }
                                                                        Some(false) => {
                                                                            view! {
                                                                                <span class="tag muted" title="上游说这个模型不收图">
                                                                                    "上游说不能收图"
                                                                                </span>
                                                                            }
                                                                                .into_any()
                                                                        }
                                                                        None => ().into_any(),
                                                                    }}

                                                                    </div>
                                                                {move || if !is_open_body() {
                                                                    ().into_any()
                                                                } else {
                                                                    // 这一块每次重画都要重新拿一份克隆：
                                                                    // 里面的 on:input 是 move 闭包，不克隆的话
                                                                    // 这个块就成 FnOnce 了（Leptos 要它能反复跑）。
                                                                    let n_field = n_field.clone();
                                                                    let n_vis_ck = n_vis_ck.clone();
                                                                    let n_vis_ch = n_vis_ch.clone();
                                                                    let n_test = n_test.clone();
                                                                    // 箭头右边那个名字要跟着输入框走 —— 用一个只更新
                                                                    // 这个文本节点的闭包，别让它把整块详情重排（会丢焦点）
                                                                    let map_field = n_field.clone();
                                                                    view! {
                                                                        <div class="m-detail">
                                                                            <div class="m-map">
                                                                                "客户端打 " <code>{n_field.clone()}</code>
                                                                                " → 发给上游 "
                                                                                <code>
                                                                                    {move || {
                                                                                        let up = f_models
                                                                                            .get()
                                                                                            .into_iter()
                                                                                            .find(|x| x.model.trim() == map_field.trim())
                                                                                            .and_then(|x| x.upstream)
                                                                                            .unwrap_or_default();
                                                                                        if up.trim().is_empty() { map_field.clone() } else { up }
                                                                                    }}
                                                                                </code>
                                                                            </div>
                                                                            <ContextCapacity channel=match form.get_untracked() { Form::Edit(id)=>Some(id), _=>None } model=n_field.clone() upstream=cur_up.clone() ctx=ctx />
                                                                            <div class="m-f">
                                                                                <span>"能看图"</span>
                                                                                <label class="pick" title="这个模型收不收得下图片（带图的请求只发给勾了的渠道）">
                                                                                    <input type="checkbox"
                                                                                        prop:checked=move || {
                                                                                            f_models
                                                                                                .get()
                                                                                                .iter()
                                                                                                .find(|x| x.model.trim() == n_vis_ck.trim())
                                                                                                .map(|x| x.vision)
                                                                                                .unwrap_or(false)
                                                                                        }
                                                                                        on:change=move |ev| {
                                                                                            let on = event_target_checked(&ev);
                                                                                            f_models
                                                                                                .update(|v| {
                                                                                                    if let Some(x) = v
                                                                                                        .iter_mut()
                                                                                                        .find(|x| x.model.trim() == n_vis_ch.trim())
                                                                                                    {
                                                                                                        x.vision = on;
                                                                                                    }
                                                                                                });
                                                                                        } />
                                                                                    "能收图"
                                                                                </label>
                                                                                <button
                                                                                    class="ghost"
                                                                                    disabled=move || vt_busy.get().is_some()
                                                                                    on:click=move |_| {
                                                                                        match channel_id {
                                                                                            Some(id) => {
                                                                                                let up = f_models
                                                                                                    .get_untracked()
                                                                                                    .into_iter()
                                                                                                    .find(|x| x.model.trim() == n_test.trim())
                                                                                                    .and_then(|x| x.upstream)
                                                                                                    .unwrap_or_default();
                                                                                                vision_test((id, n_test.clone(), up))
                                                                                            }
                                                                                            None => {
                                                                                                ctx
                                                                                                    .say(
                                                                                                        "还没保存：先点「保存」，再测它认不认得图",
                                                                                                    )
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                >
                                                                                    "测一下能不能看图"
                                                                                </button>
                                                                            </div>
                                                                            <p class="muted small">
                                                                                "「能看图」可来自上游图片输入声明、人工设置或视觉实测；点上面按钮可真发图验证，"
                                                                                "答对后会自动勾上。带图的请求只会发给勾了的渠道 —— 一条都没勾就回 422 说清楚，"
                                                                                "而不是把图悄悄丢掉。"
                                                                            </p>
                                                                            {move || {
                                                                                match vt_msg.get() {
                                                                                    Some((id, text, ok)) if Some(id) == channel_id => {
                                                                                        view! {
                                                                                            <div class=if ok { "banner ok" } else { "banner bad" }>
                                                                                                {text}
                                                                                            </div>
                                                                                        }
                                                                                            .into_any()
                                                                                    }
                                                                                    _ => ().into_any(),
                                                                                }
                                                                            }}

                                                                        </div>
                                                                    }.into_any()
                                                                }}
                                                                </div>
                                                            </div>
                                                        }
                                                    })
                                                    .collect_view()
                                                    .into_any()
                                            }}
                                        </div>



                                    }.into_any()
                                }}
                            </div>

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

    fn up(id: &str) -> UpstreamModel {
        UpstreamModel {
            id: id.to_string(),
            ..Default::default()
        }
    }

    fn mapping(model: &str, upstream: Option<&str>) -> ModelPair {
        ModelPair {
            model: model.to_string(),
            upstream: upstream.map(str::to_string),
            vision: false,
            drop_fields: String::new(),
        }
    }

    #[test]
    fn filter_is_case_insensitive_and_limited() {
        let all = vec![up("deepseek-v4.1-flash"), up("BGE-M3"), up("gpt-oss")];
        let ids = |v: Vec<UpstreamModel>| v.into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(filter_catalog(&all, "", 10).len(), 3, "不过滤就全列");
        assert_eq!(ids(filter_catalog(&all, "deep", 10)), vec!["deepseek-v4.1-flash"]);
        assert_eq!(ids(filter_catalog(&all, "BGE", 10)), vec!["BGE-M3"], "过滤不分大小写");
        assert_eq!(ids(filter_catalog(&all, "  gpt  ", 10)), vec!["gpt-oss"], "两头空白忽略");
        assert_eq!(filter_catalog(&all, "", 2).len(), 2, "显示有上限，免得塞爆 DOM");
        assert!(filter_catalog(&all, "没有这个名字", 10).is_empty());
    }

    /// 勾选状态就是模型表本身：加一条就"认这个名字"，删一条就不再认。
    #[test]
    fn ticking_a_name_is_the_mapping_itself() {
        let mut rows: Vec<ModelPair> = Vec::new();
        assert!(!is_in_use(&rows, "deepseek-flash"));

        assert!(add_mapping(&mut rows, "deepseek-flash"));
        assert!(is_in_use(&rows, "deepseek-flash"), "加进来就该是认它");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].upstream, None, "没填上游名字 = 同名发出去");
        assert!(!rows[0].vision, "刚加的名字不许猜能力");
        assert!(rows[0].drop_fields.is_empty(), "刚加的名字一个字段都不丢");

        // 连点两次不该堆重复行
        assert!(!add_mapping(&mut rows, "deepseek-flash"));
        assert_eq!(rows.len(), 1);

        // 空名字进不来；两头的空白不算名字的一部分
        assert!(!add_mapping(&mut rows, "   "));
        assert!(add_mapping(&mut rows, "  bge-m3  "));
        assert!(is_in_use(&rows, "bge-m3"));
        assert_eq!(rows.len(), 2);
    }

    /// 取消勾选 = 把那一条删掉；删之前要能问出"这一条配过东西吗"，
    /// 不然用户不知道自己的「能看图」/「要丢的字段」跟着没了。
    #[test]
    fn unticking_removes_it_and_reports_what_would_be_lost() {
        let mut rows = vec![mapping("auto", Some("v4.1flash")), mapping("v4.1flash", None)];
        rows[1].vision = true;

        assert!(!mapping_has_config(&rows, "auto"), "这条路没配过东西");
        assert!(mapping_has_config(&rows, "v4.1flash"), "标过能看图，取消前要问一句");
        assert!(!mapping_has_config(&rows, "没有的名字"));

        assert_eq!(drop_mapping(&mut rows, "v4.1flash"), 1);
        assert_eq!(rows.len(), 1, "只删同名的那一条，别名那条不动");
        assert_eq!(rows[0].model, "auto");
        assert_eq!(drop_mapping(&mut rows, "v4.1flash"), 0, "再删一次什么也不删");
        // 别名那条也在用它，但键是客户端名字
        assert!(!is_in_use(&rows, "v4.1flash"));
        assert!(is_in_use(&rows, "auto"));
    }

    #[test]
    fn bulk_ticking_never_double_adds() {
        let mut rows: Vec<ModelPair> = Vec::new();
        let list: Vec<String> = ["a", "b", "a", "  ", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(add_many(&mut rows, &list), 3, "重复的和空名字不算");
        assert_eq!(add_many(&mut rows, &list), 0);
        assert_eq!(rows.len(), 3);
    }

    /// 保存前把重复的客户端名字拦下来：一个名字只能一条映射，
    /// 不拦的话后写的那条会把前一条盖掉，用户改过的「能看图」就悄悄没了。
    #[test]
    fn duplicate_client_names_are_caught_before_saving() {
        let rows = vec![
            mapping("auto", Some("v4.1flash")),
            mapping("bge-m3", None),
            mapping(" auto ", Some("x")),
            mapping("", None),
            mapping("", Some("y")),
        ];
        assert_eq!(duplicate_client_names(&rows), vec!["auto".to_string()]);
    }

    #[test]
    fn saving_trims_and_drops_blank_rows_honestly() {
        let rows = vec![
            ModelPair {
                model: "  auto  ".into(),
                upstream: Some("   ".into()),
                vision: true,
                drop_fields: "reasoning_effort".into(),
            },
            mapping("bge-m3", Some(" BAAI/bge-m3 ")),
            mapping("   ", None),
        ];
        let (out, skipped) = mappings_to_save(&rows);
        assert_eq!(skipped, 1, "空名字的行要如实回报丢了几条");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].model, "auto");
        assert_eq!(out[0].upstream, None, "上游那栏全是空白 = 同名发出去");
        assert!(out[0].vision, "已经实测标上的能力不许被抹掉");
        assert_eq!(out[0].drop_fields, "reasoning_effort");
        assert_eq!(out[1].model, "bge-m3");
        assert_eq!(out[1].upstream.as_deref(), Some("BAAI/bge-m3"), "两头空白去掉");
    }

}

// ─────────── 渠道页加模型 = 顺手建组（2026-10-18，按用户要求改） ───────────

#[component]
fn ContextCapacity(channel: Option<i64>, model: String, upstream: String, ctx: Ctx) -> impl IntoView {
    let upstream = if upstream.trim().is_empty() { model.clone() } else { upstream };
    let model = StoredValue::new(model);
    let upstream = StoredValue::new(upstream);
    let data = RwSignal::new(serde_json::Value::Null);
    let capacity = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let load = move || {
        let Some(id) = channel else { message.set("请先保存模型映射，再设置上下文".into()); return; };
        busy.set(true);
        leptos::task::spawn_local(async move {
            match api::context_capacity(id, &model.get_value(), &upstream.get_value()).await {
                Ok(v) => { capacity.set(v["override_tokens"].as_i64().map(|n|n.to_string()).unwrap_or_default()); data.set(v); message.set(String::new()); },
                Err(e) => { ctx.note_err(&e); message.set(e.message); }
            }
            busy.set(false);
        });
    };
    load();
    let save_capacity = move |clear:bool| {
        let Some(id)=channel else {return;};
        let tokens=if clear {None} else {
            match capacity.get_untracked().parse::<i64>() {
                Ok(n) if n>0=>Some(n), _=>{message.set("请输入正整数 tokens".into());return;}
            }
        };
        busy.set(true);
        leptos::task::spawn_local(async move {
            match api::set_context_override(id,&model.get_value(),&upstream.get_value(),tokens).await {
                Ok(())=>load(), Err(e)=>{ctx.note_err(&e);message.set(e.message);busy.set(false);}
            }
        });
    };
    view! {
        <div class="m-f">
            <span>"上下文长度（tokens）"</span>
            <p>{move || data.get()["override_tokens"].as_i64().map(|n|format!("当前用户确认：{n} tokens（优先）")).unwrap_or_else(||"尚无用户确认容量".into())}</p>
            <div class="row">
                <input type="number" min="1" step="1" placeholder="正整数 tokens" prop:value=move ||capacity.get() on:input=move |ev|capacity.set(event_target_value(&ev)) />
                <button disabled=move ||busy.get() on:click=move |_|save_capacity(false)>"保存"</button>
                <button disabled=move ||busy.get() || data.get()["override_tokens"].is_null() on:click=move |_|save_capacity(true)>"清除"</button>
            </div>
            <p class="bad-text">{move ||message.get()}</p>
        </div>
    }
}
