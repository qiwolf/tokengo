//! 模型名 → 渠道。**没有「模型组」这一层了**（2026-10-18，v0.11.0）。
//!
//! 一个模型名 = 勾了它的那些渠道，按渠道自己的全局顺序试（面板「渠道」页从上到下）。
//! 名字不用单独创建：在渠道里勾上就出现，取消勾选就消失。勾选就是白名单（v0.9.0 起），
//! 所以「路由」与「渠道里勾了什么」本来就是同一件事，以前那种「清单里有、打了 404」
//! 的两处打架不可能再出现。
//!
//! 保留名 `auto`：不是一个真模型名，没有自己的渠道、不落库、不写路由状态。客户端打
//! `auto` 时，网关在**用户勾选过的模型**里挑一个现在最优的现场发出去（见 [`auto_candidates`]）。
//! 用户口径：**不做兜底** —— 一切由用户勾选的模型决定；一个都没勾时，auto 对外不显示。

use std::collections::HashMap;

use rusqlite::Connection;
use tokengo_shared::{
    normalize_model_name, same_model_name, Channel, ChannelId, KindSource, ModelKind,
};

use super::{channels, kinds, Result};

/// 保留名：自动挑一个最优的模型发出去。
pub const AUTO_NAME: &str = "auto";

/// 「挑最优」看的是最近这段时间的成绩。太长的窗口会把几天前的毛病一直背着，
/// 太短又会被一次抖动带偏 —— 15 分钟与探活回切检查点同量级。
pub const AUTO_STATS_WINDOW_MS: i64 = 15 * 60 * 1000;

/// 这个名字是不是保留名 `auto`（归一后比：`Auto`、`AUTO` 都算）。
pub fn is_auto(name: &str) -> bool {
    normalize_model_name(name) == AUTO_NAME
}

pub fn enabled(conn:&Connection,name:&str)->Result<bool>{Ok(super::settings::get(conn,&format!("model_enabled:{}",normalize_model_name(name)))?.as_deref()!=Some("false"))}
pub fn visible(conn:&Connection,name:&str)->Result<bool>{Ok(super::settings::get(conn,&format!("model_visible:{}",normalize_model_name(name)))?.as_deref()!=Some("false"))}
pub fn set_visible(conn:&Connection,name:&str,value:bool)->Result<()>{super::settings::set(conn,&format!("model_visible:{}",normalize_model_name(name)),if value{"true"}else{"false"})}

pub fn set_enabled(conn:&Connection,name:&str,value:bool)->Result<()>{super::settings::set(conn,&format!("model_enabled:{}",normalize_model_name(name)),if value{"true"}else{"false"})}

/// 一个模型名 + 它现在的全部事实。**面板与对外清单共用这一份**，别在各处重算 ——
/// 各算各的就会漂移（以前就是这样：「模型」页按名字算、对外清单按组算）。
#[derive(Debug, Clone, PartialEq)]
pub struct NameRoute {
    /// 展示拼写：按渠道顺序第一个勾它的写法（归一后同一个名字只留一行）。
    pub name: String,
    pub enabled:bool,
    pub kind: ModelKind,
    pub kind_source: KindSource,
    pub kind_conflict: bool,
    pub kind_origin: String,
    /// 勾了它的渠道，按渠道顺序（含停用、没填 base_url 的）。
    pub declared_by: Vec<ChannelId>,
    /// 现在真能用的渠道（启用 + 填了 base_url），按渠道顺序 —— 路由就按这个顺序试。
    pub channels: Vec<ChannelId>,
    /// 这些可用渠道里有没有标了「能看图」的。
    pub vision_channels: bool,
    /// Conservative limit across usable mappings; absent if any target is unknown.
    pub context_window: Option<i64>,
    pub context_source: Option<String>,
    /// Nominal fallback, never advertised as a confirmed channel limit.
    pub nominal_context_window: Option<i64>,
    pub nominal_context_source: Option<String>,
    pub context_reason: String,
}

impl NameRoute {
    /// 现在有没有渠道能用（没有 = 客户端打这个名字会 404）。
    pub fn routed(&self) -> bool {
        self.enabled && !self.channels.is_empty()
    }

    /// 能不能吃图片输入。**只有对话类才谈这个**：向量 / 重排 / 图像 / 视频 / 音频
    /// 都不收图，给它们挂「能看图」只会让人以为配错了。
    pub fn can_see(&self) -> bool {
        matches!(self.kind, ModelKind::Chat | ModelKind::Unknown) && self.vision_channels
    }
}

/// 这条渠道现在能不能收请求（启用了、也填了 base_url）。
fn usable(l: &channels::ChannelRow) -> bool {
    l.enabled && l.provider_enabled && !l.base_url.trim().is_empty()
}

/// 渠道里勾过的**所有**名字（归一后去重，不含保留名 auto），按名字排。
///
/// 行的顺序是稳定的（按名字），刷新前后不会跳；`declared_by` / `channels` 里是渠道顺序。
pub fn list(conn: &Connection) -> Result<Vec<NameRoute>> {
    let chans = channels::list(conn)?;
    let kind_of = kinds::all(conn)?;
    let mut out: Vec<NameRoute> = Vec::new();
    for l in &chans {
        for m in &l.models {
            let raw = m.model.trim();
            // 保留名 auto 不是「一个模型」：它由 auto_candidates 现场算，不占一行。
            if raw.is_empty() || is_auto(raw) {
                continue;
            }
            let idx = match out.iter().position(|r| same_model_name(&r.name, raw)) {
                Some(i) => i,
                None => {
                    let k = kind_of.iter().find(|k| same_model_name(&k.name, raw));
                    out.push(NameRoute {
                        name: raw.to_string(),
                        enabled:enabled(conn,raw)?,
                        kind: k.map(|k| k.kind).unwrap_or_default(),
                        kind_source: k.map(|k| k.source).unwrap_or_default(),
                        kind_conflict: false,
                        kind_origin: k.map(|k| k.source.as_str()).unwrap_or("").to_string(),
                        declared_by: Vec::new(),
                        channels: Vec::new(),
                        vision_channels: false,
                        context_window: None,
                        context_source: None,
                        nominal_context_window: None,
                        nominal_context_source: None,
                        context_reason: String::new(),
                    });
                    out.len() - 1
                }
            };
            let r = &mut out[idx];
            if !r.declared_by.contains(&l.id) {
                r.declared_by.push(l.id);
            }
            if m.enabled && usable(l) && !super::faults::blocked(conn,l.id.0,raw,super::now_ms())? {
                if !r.channels.contains(&l.id) {
                    r.channels.push(l.id);
                }
                if m.vision {
                    r.vision_channels = true;
                }
            }
        }
    }
    let catalogs = chans.iter().map(|ch| {
        super::catalog::list(conn, ch.id).map(|entries| (ch.id, entries))
    }).collect::<Result<HashMap<_, _>>>()?;
    for r in &mut out {
        let policy = super::route_policy::load(conn, &r.name)?;
        super::route_policy::order_ids(&policy, &mut r.channels);
        r.vision_channels = chans.iter().filter(|c| r.channels.contains(&c.id))
            .any(|c| c.models.iter().any(|m| same_model_name(&m.model, &r.name) && m.enabled && m.vision));
        let mut confirmed = Vec::new();
        let mut manual_used = false;
        let mut upstream_used = false;
        for ch in chans.iter().filter(|ch| usable(ch) && r.channels.contains(&ch.id)) {
            for m in ch.models.iter().filter(|m| same_model_name(&m.model, &r.name) && m.enabled) {
                let manual = crate::model_registry::context_override(conn, ch.id, m)?;
                manual_used |= manual.is_some();
                let value = manual.or_else(|| crate::capability_detector::mapped_context(&catalogs[&ch.id], m));
                upstream_used |= manual.is_none() && value.is_some();
                confirmed.push(value);
            }
        }
        r.context_window = crate::capability_detector::conservative_context(confirmed);
        r.context_source = r.context_window.map(|_| if manual_used {
            if upstream_used { "user_confirmed+upstream" } else { "user_confirmed" }
        } else { "upstream" }.to_string());
        let mut nominal = Vec::new();
        let mut reasons = Vec::new();
        let mut registry_used = false;
        for ch in chans.iter().filter(|ch| usable(ch) && r.channels.contains(&ch.id)) {
            for m in ch.models.iter().filter(|m| same_model_name(&m.model, &r.name) && m.enabled) {
                if let Some(tokens) = crate::model_registry::context_override(conn, ch.id, m)?.or_else(|| crate::capability_detector::mapped_context(&catalogs[&ch.id], m)) {
                    nominal.push(Some(tokens));
                } else {
                    registry_used = true;
                    let context = crate::model_registry::mapped(conn, ch.id, m)?;
                    nominal.push(context.tokens);
                    if context.tokens.is_none() {
                        reasons.push(format!("渠道 {}：{}", ch.id.0, context.reason));
                    }
                }
            }
        }
        if registry_used {
            r.nominal_context_window = crate::capability_detector::conservative_context(nominal);
            r.nominal_context_source = r.nominal_context_window.map(|_| "models.dev".into());
        }
        r.context_reason = if r.context_window.is_some() {
            if manual_used { "用户确认的渠道容量优先；跨可用映射取保守最小值".into() } else { "上游声明；不是渠道容量实测".into() }
        } else if r.nominal_context_window.is_some() {
            "models.dev 精确身份；仅标称上下文，渠道容量未验证；取可用映射最小值".into()
        } else if reasons.is_empty() {
            "没有可用渠道上下文依据".into()
        } else {
            // Bound panel payload even for unusually large channel sets.
            reasons.into_iter().take(8).collect::<Vec<_>>().join("; ")
        };
        let mut claims = Vec::new();
        for ch in &chans {
            if !r.declared_by.contains(&ch.id) { continue; }
            let catalog = &catalogs[&ch.id];
            for m in &ch.models {
                if !same_model_name(&m.model, &r.name) { continue; }
                let upstream = m.upstream.as_deref().unwrap_or(&m.model);
                for entry in catalog {
                    if same_model_name(&entry.model, upstream) {
                        if let Some(raw) = &entry.kind {
                            let k = ModelKind::parse(raw);
                            if k.is_set() && !claims.contains(&k) { claims.push(k); }
                        }
                    }
                }
            }
        }
        r.kind_conflict = claims.len() > 1 || (r.kind.is_set() && claims.iter().any(|k| *k != r.kind));
        if !r.kind.is_set() && claims.len() == 1 {
            r.kind = claims[0];
            r.kind_source = KindSource::Auto;
            r.kind_origin = "upstream".to_string();
        }
    }
    out.sort_by_key(|r| r.name.to_lowercase());
    Ok(out)
}

/// 现在的名字清单（有渠道能用的那些）—— 对外 `/v1/models` 与面板都读它。
pub fn servable(conn: &Connection) -> Result<Vec<NameRoute>> {
    Ok(list(conn)?.into_iter().filter(|r| r.routed()).collect())
}

/// 一个名字在哪几行上写的（面板要显示「谁勾了它」），按渠道顺序。
pub fn routes_for(conn: &Connection, name: &str) -> Result<Option<NameRoute>> {
    Ok(list(conn)?
        .into_iter()
        .find(|r| same_model_name(&r.name, name)))
}

/// 路由要用的渠道链：勾了这个名字的渠道，按渠道顺序，带**这个模型名那一行**的
/// 别名 / 能看图 / 丢字段。
///
/// 别名与视觉能力都按「渠道 + 名字」这一行取，不做全局推断 —— 同一个模型名在 A 渠道上
/// 落到能看图的模型、在 B 渠道上是纯文本，是常事。
pub fn channels_for(conn: &Connection, name: &str) -> Result<Vec<Channel>> {
    let mut out = Vec::new();
    if !enabled(conn,name)? {return Ok(out);}
    for l in channels::list(conn)? {
        let Some(m) = l
            .models
            .iter()
            .find(|m| same_model_name(&m.model, name))
        else {
            continue;
        };
        let alias = m.upstream.clone().filter(|u| !u.trim().is_empty());
        out.push(Channel {
            id: l.id,
            name: l.name,
            base_url: l.base_url,
            enabled: l.enabled && l.provider_enabled && m.enabled,
            priority: out.len() as i32,
            max_concurrent: l.max_concurrent,
            model_alias: alias,
            vision: m.vision,
            drop_fields: m.drop_fields.clone(),
            unwrap_envelope: l.unwrap_envelope,
            headers: l.headers,
        });
    }
    super::route_policy::order_channels(&super::route_policy::load(conn, name)?, &mut out);
    Ok(out)
}

/// 这个端点对应的类型能不能走这个名字。
///
/// 对话请求只在「对话类」与「还没定」里挑（还没定的名字被打过一次就会定下来，
/// 不能因为没类型就把它排除在外）；向量 / 重排各挑各的。图像 / 视频 / 音频
/// 网关没有转发端点，永远不参与 —— 绝不许拿对话请求去打生图模型。
fn kind_serves(kind: ModelKind, want: ModelKind) -> bool {
    match want {
        ModelKind::Chat => matches!(kind, ModelKind::Chat | ModelKind::Unknown),
        ModelKind::Embeddings => kind == ModelKind::Embeddings,
        ModelKind::Rerank => kind == ModelKind::Rerank,
        _ => false,
    }
}

/// `auto` 的候选：用户勾选过的模型里，能走这个端点、现在有渠道可用的那几个，按「现在谁更靠谱」排。
///
/// **不做兜底**（用户口径，2026-10-18）：候选只来自「用户勾选过的模型」，没有别的来源 ——
/// 一个都没勾就是空表（对外不显示 auto，打了也是 404 说得清）。
///
/// 排序 = 实测成绩，不靠人排：先看最近 [`AUTO_STATS_WINDOW_MS`] 内谁没在失败
/// （渠道自己的毛病：连不上 / 5xx / 429 / 上游 200 却一句话不说），失败率相同再看谁平均快；
/// 这段时间一次都没打过的不吃亏，但排在有成绩的后面（没数据不能算「快」）。最后按名字保稳定。
///
/// 同一段对话的多次请求因此可能换模型 —— 这是「自动挑最优」的代价，换来的是
/// 一条渠道开始抽风时后续请求自动落到还好的那条上。
pub fn auto_candidates(
    conn: &Connection,
    want: ModelKind,
    want_vision: bool,
    now_ms: i64,
) -> Result<Vec<String>> {
    let stats=super::scores::all(conn,now_ms)?;
    let mut scored:Vec<(String,f64,i64)>=Vec::new();
    for r in list(conn)? {
        if !visible(conn,&r.name)? || super::model_access::internal_only(conn, &r.name)? {continue;}
        if super::route_policy::load(conn, &r.name)?.include_in_auto == Some(false) { continue; }
        if !r.routed() || !kind_serves(r.kind,want) || (want_vision && !r.can_see()) {continue;}
        let v=stats.get(&normalize_model_name(&r.name)).cloned().unwrap_or_default();
        scored.push((r.name,v.value(),v.avg_ms.unwrap_or(i64::MAX)));
    }
    scored.sort_by(|a,b|b.1.total_cmp(&a.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
    Ok(scored.into_iter().map(|x| x.0).collect())
}

/// `auto` 现在有没有可挑的模型（任意一类）。
///
/// 用户口径：「勾选（指定）使用的模型为 0 时，客户端则不显示 auto 模型」——
/// 所以对外清单里要不要列 auto，由它说了算。
pub fn auto_available(conn: &Connection) -> Result<bool> {
    for route in list(conn)? {
        if route.routed() && visible(conn,&route.name)? && super::route_policy::load(conn, &route.name)?.include_in_auto != Some(false) { return Ok(true); }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::channels::{self, ModelMapping, NewChannel};
    use crate::db::open_in_memory;

    fn channel(conn: &Connection, name: &str) -> ChannelId {
        channels::create(
            conn,
            &NewChannel {
                name: name.into(),
                base_url: format!("http://127.0.0.1/{name}"),
                enabled: true,
                unwrap_envelope: false,
                max_concurrent: 0,
            },
        )
        .unwrap()
    }

    fn declare(conn: &Connection, id: ChannelId, models: &[ModelMapping]) {
        channels::set_models(conn, id, models).unwrap();
    }

    /// 顺序：渠道页从上到下就是「先试哪条」，名字下面直接按它排。
    #[test]
    fn the_channel_order_is_the_try_order() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        let b = channel(&conn, "B");
        let c = channel(&conn, "C");
        for id in [a, b, c] {
            declare(&conn, id, &[ModelMapping::new("auto2", None, false)]);
        }
        // 新渠道默认排最后：A → B → C
        let ls = channels_for(&conn, "auto2").unwrap();
        assert_eq!(
            ls.iter().map(|l| l.id).collect::<Vec<_>>(),
            vec![a, b, c],
            "新渠道该排在最后"
        );

        // C 往上挪一位 → A → C → B
        channels::reorder(&conn, c, true).unwrap();
        let ls = channels_for(&conn, "auto2").unwrap();
        assert_eq!(ls.iter().map(|l| l.id).collect::<Vec<_>>(), vec![a, c, b]);
        assert_eq!(ls[1].priority, 1, "顺序就是路优先级");

        // 再往上 → C → A → B；到顶再点没反应也不报错
        channels::reorder(&conn, c, true).unwrap();
        channels::reorder(&conn, c, true).unwrap();
        assert_eq!(
            channels_for(&conn, "auto2")
                .unwrap()
                .iter()
                .map(|l| l.id)
                .collect::<Vec<_>>(),
            vec![c, a, b]
        );
    }

    #[test]
    fn catalog_types_follow_aliases_preserve_manual_and_report_conflicts() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        let b = channel(&conn, "B");
        declare(&conn, a, &[ModelMapping::new("public", Some("real".into()), false)]);
        let cat = crate::upstream::parse_model_catalog(r#"{"data":[{"id":"real","kind":"embeddings"}]}"#);
        super::super::catalog::replace(&conn,a,&cat,1).unwrap();
        let row = routes_for(&conn,"public").unwrap().unwrap();
        assert_eq!(row.kind, ModelKind::Embeddings);
        assert_eq!(row.kind_origin, "upstream");
        assert!(!row.kind_conflict);
        assert!(kinds::get(&conn,"public").unwrap().is_none(), "read must not write");
        kinds::set(&conn,"public",ModelKind::Chat).unwrap();
        let row = routes_for(&conn,"public").unwrap().unwrap();
        assert_eq!(row.kind,ModelKind::Chat);
        assert!(row.kind_conflict);
        kinds::set(&conn,"public",ModelKind::Unknown).unwrap();
        declare(&conn,b,&[ModelMapping::new("public",None,false)]);
        let cat = crate::upstream::parse_model_catalog(r#"{"data":[{"id":"public","kind":"chat"}]}"#);
        super::super::catalog::replace(&conn,b,&cat,2).unwrap();
        let row = routes_for(&conn,"public").unwrap().unwrap();
        assert_eq!(row.kind,ModelKind::Unknown);
        assert!(row.kind_conflict);
        assert!(channels_for(&conn,"public").unwrap().len() == 2);
    }

    /// 归一之后同名的写法只留一行，展示拼写取渠道顺序上第一次出现的那个。
    #[test]
    fn spellings_of_one_name_collapse_into_one_row() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        let b = channel(&conn, "B");
        declare(
            &conn,
            a,
            &[ModelMapping::new("DeepSeek-V4.1-Flash", None, false)],
        );
        declare(
            &conn,
            b,
            &[ModelMapping::new("deepseek-v4.1-flash", None, true)],
        );
        let rows = list(&conn).unwrap();
        assert_eq!(rows.len(), 1, "同一个名字只该有一行");
        assert_eq!(rows[0].name, "DeepSeek-V4.1-Flash", "取先出现的那份拼写");
        assert_eq!(rows[0].declared_by, vec![a, b]);
        assert!(rows[0].routed());
    }

    /// 一个名字没人勾 → 不在清单里，打了也没有渠道（这就是「勾选就是白名单」）。
    #[test]
    fn a_name_nobody_declares_is_not_servable() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        declare(&conn, a, &[ModelMapping::new("别的名字", None, false)]);
        let names: Vec<String> = servable(&conn).unwrap().into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["别的名字".to_string()], "清单里只该有勾过的名字");
        assert!(channels_for(&conn, "没人勾的名字").unwrap().is_empty());
        assert!(routes_for(&conn, "没人勾的名字").unwrap().is_none());
    }

    /// 预留名 auto 不占行：它不是一个模型，只是客户端能打的一个名字。
    #[test]
    fn the_reserved_name_is_not_a_row() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        declare(
            &conn,
            a,
            &[
                ModelMapping::new("auto", None, false),
                ModelMapping::new("真模型", None, false),
            ],
        );
        let names: Vec<String> = list(&conn).unwrap().into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["真模型".to_string()], "auto 不该占一行");
        assert!(is_auto("AUTO"));
        assert!(!is_auto("auto2"));
    }

    /// 用文档里那句反证收尾：**渠道停用之后这个名字就不再对外暴露**。
    #[test]
    fn disabling_the_only_channel_hides_the_name() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        declare(&conn, a, &[ModelMapping::new("auto2", None, false)]);
        assert_eq!(servable(&conn).unwrap().len(), 1);

        channels::set_enabled(&conn, a, false).unwrap();
        assert!(servable(&conn).unwrap().is_empty(), "渠道停用后不该再说它可用");
        let r = routes_for(&conn, "auto2").unwrap().unwrap();
        assert_eq!(r.declared_by, vec![a], "谁勾过它还是要看得见");
        assert!(!r.routed(), "但没有一条能用");
    }

    /// auto 的候选只认「勾选过的模型」，一个都没勾就是空的（不做兜底）。
    #[test]
    fn auto_has_no_candidates_when_nothing_is_checked() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        assert!(!auto_available(&conn).unwrap());
        assert!(auto_candidates(&conn, ModelKind::Chat, false, 0)
            .unwrap()
            .is_empty());

        // 只勾了 auto 自己（上游中继常见）→ 仍然一个候选都没有
        declare(&conn, a, &[ModelMapping::new("auto", None, false)]);
        assert!(!auto_available(&conn).unwrap());
        assert!(auto_candidates(&conn, ModelKind::Chat, false, 0)
            .unwrap()
            .is_empty());
    }

    /// 候选按类型分家：对话请求不挑向量模型，向量请求不挑对话模型。
    #[test]
    fn candidates_are_split_by_endpoint_kind() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        declare(
            &conn,
            a,
            &[
                ModelMapping::new("聊天的", None, false),
                ModelMapping::new("向量的", None, false),
                ModelMapping::new("还没定的", None, false),
            ],
        );
        kinds::set(&conn, "聊天的", ModelKind::Chat).unwrap();
        kinds::set(&conn, "向量的", ModelKind::Embeddings).unwrap();

        let chat = auto_candidates(&conn, ModelKind::Chat, false, 0).unwrap();
        assert_eq!(
            chat,
            vec!["聊天的".to_string(), "还没定的".to_string()],
            "对话请求只在对话类与还没定里挑"
        );
        let emb = auto_candidates(&conn, ModelKind::Embeddings, false, 0).unwrap();
        assert_eq!(emb, vec!["向量的".to_string()], "向量请求只挑向量模型");
    }

    /// 带图的请求只在「能看图」的模型里挑；一个都没有就是空表（外层会回 422）。
    #[test]
    fn vision_requests_only_pick_seeing_models() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        declare(
            &conn,
            a,
            &[
                ModelMapping::new("能看图", None, true),
                ModelMapping::new("纯文本", None, false),
            ],
        );
        assert_eq!(
            auto_candidates(&conn, ModelKind::Chat, true, 0).unwrap(),
            vec!["能看图".to_string()]
        );

        // 把「能看图」标成向量模型 → 它不算能看图的对话模型了
        kinds::set(&conn, "能看图", ModelKind::Embeddings).unwrap();
        assert!(auto_candidates(&conn, ModelKind::Chat, true, 0)
            .unwrap()
            .is_empty());
    }

    /// 排序按实测：最近在失败的排后面，其次看谁快，没数据的排在有数据的后面。
    #[test]
    fn the_order_follows_recent_results() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        let b = channel(&conn, "B");
        let c = channel(&conn, "C");
        declare(&conn, a, &[ModelMapping::new("坏掉的", None, false)]);
        declare(&conn, b, &[ModelMapping::new("慢的", None, false)]);
        declare(&conn, c, &[ModelMapping::new("快的", None, false)]);

        let now = crate::db::now_ms();
        let mk = |ch: ChannelId, outcome: &str, dur: i64| {
            let rid: i64 = conn
                .query_row(
                    "INSERT INTO requests (ts_ms, endpoint, model, status) VALUES (?1, '/v1/chat/completions', 'x', 'ok') RETURNING id",
                    [now],
                    |r| r.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT INTO attempts (request_id, ordinal, channel_id, outcome, duration_ms,model_name)
                 VALUES (?1, 0, ?2, ?3, ?4,(SELECT model_name FROM channel_models WHERE channel_id=?2 LIMIT 1))",
                rusqlite::params![rid, ch.0, outcome, dur],
            )
            .unwrap();
        };
        mk(a, "channel_fault", 10);
        mk(a, "channel_fault", 10);
        mk(b, "success", 900);
        mk(c, "success", 20);
        mk(c, "success", 30);

        assert_eq!(
            auto_candidates(&conn, ModelKind::Chat, false, now).unwrap(),
            vec!["快的".to_string(), "慢的".to_string(), "坏掉的".to_string()]
        );

        // 出了一条「客户端写错请求」的账：不算渠道的毛病，排序不该被它带偏
        mk(b, "client_fault", 5);
        assert_eq!(
            auto_candidates(&conn, ModelKind::Chat, false, now).unwrap()[0],
            "快的"
        );
    }

    /// 窗口外（太久以前）的成绩不算数：一条渠道早就好了，不该永远背着旧账。
    #[test]
    fn old_attempts_fall_out_of_the_window() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "A");
        declare(&conn, a, &[ModelMapping::new("老账", None, false)]);
        let old = crate::db::now_ms() - AUTO_STATS_WINDOW_MS - 60_000;
        let rid: i64 = conn
            .query_row(
                "INSERT INTO requests (ts_ms, endpoint, model, status) VALUES (?1, '/v1/chat/completions', 'x', 'ok') RETURNING id",
                [old],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO attempts (request_id, ordinal, channel_id, outcome, duration_ms)
             VALUES (?1, 0, ?2, 'channel_fault', 10)",
            rusqlite::params![rid, a.0],
        )
        .unwrap();
        assert_eq!(
            auto_candidates(&conn, ModelKind::Chat, false, crate::db::now_ms()).unwrap(),
            vec!["老账".to_string()],
            "窗口外的失败不该把它一直压在后面（它仍然在候选里）"
        );
    }
}
