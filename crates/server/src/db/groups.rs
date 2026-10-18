//! 模型组仓储：一个对外模型名 → 一张**有序**渠道列表。顺序即优先级。

use rusqlite::{params, Connection, OptionalExtension, Row};

use tokengo_shared::{
    same_model_name, Channel, ChannelId, KindSource, ModelGroup, ModelKind,
};

use super::{DbError, Result};

/// 库里的一组。
#[derive(Debug, Clone, PartialEq)]
pub struct GroupRow {
    pub name: String,
    pub enabled: bool,
    /// 已按 ordinal 排好序
    pub channels: Vec<ChannelId>,
    /// 这组是哪种模型（对话 / 向量 / 重排 / 还没定）。
    pub kind: ModelKind,
    /// 类型是谁定的：实测学的还是人手动选的。
    pub kind_source: KindSource,
}

fn row_to_group(r: &Row<'_>) -> rusqlite::Result<GroupRow> {
    Ok(GroupRow {
        name: r.get(0)?,
        enabled: r.get::<_, i64>(1)? != 0,
        channels: Vec::new(),
        kind: ModelKind::parse(&r.get::<_, String>(2)?),
        kind_source: KindSource::parse(&r.get::<_, String>(3)?),
    })
}

fn load_channels(conn: &Connection, g: &mut GroupRow) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT channel_id FROM model_group_channels WHERE group_name = ?1 ORDER BY ordinal",
    )?;
    g.channels = stmt
        .query_map([&g.name], |r| r.get::<_, i64>(0).map(ChannelId))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(())
}

pub fn list(conn: &Connection) -> Result<Vec<GroupRow>> {
    let mut stmt = conn.prepare(
        "SELECT name, enabled, kind, kind_source FROM model_groups ORDER BY name",
    )?;
    let mut rows: Vec<GroupRow> = stmt.query_map([], row_to_group)?.collect::<rusqlite::Result<_>>()?;
    for g in &mut rows {
        load_channels(conn, g)?;
    }
    Ok(rows)
}

pub fn get(conn: &Connection, name: &str) -> Result<Option<GroupRow>> {
    let mut stmt = conn.prepare(
        "SELECT name, enabled, kind, kind_source FROM model_groups WHERE name = ?1",
    )?;
    let mut g: Option<GroupRow> = stmt.query_row([name], row_to_group).optional()?;
    if let Some(g) = &mut g {
        load_channels(conn, g)?;
    }
    Ok(g)
}

/// 一键启用/停用这组（面板状态列那个开关点的就是它）。
///
/// **只改一列**，不像 `upsert` 那样整组重写 —— 面板上点一下开关不该顺带把渠道顺序
/// 或类型改掉（那两个各有各的入口）。
pub fn set_enabled(conn: &Connection, name: &str, enabled: bool) -> Result<()> {
    let n = conn.execute(
        "UPDATE model_groups SET enabled = ?2 WHERE name = ?1",
        params![name.trim(), enabled as i64],
    )?;
    if n == 0 {
        return Err(DbError::Other(format!("模型组 {} 不存在", name.trim())));
    }
    Ok(())
}

/// 人在面板上定这组的类型。`kind = None`（或 Unknown）= 交还给实测（下次真收到请求再学）。
///
/// **手动定过之后实测不许再覆盖** —— 用户拍过的板不被流量改掉。
pub fn set_kind(conn: &Connection, name: &str, kind: ModelKind) -> Result<()> {
    let (k, src) = if kind.is_set() {
        (kind.as_str(), KindSource::Manual.as_str())
    } else {
        ("", KindSource::Unknown.as_str())
    };
    let n = conn.execute(
        "UPDATE model_groups SET kind = ?2, kind_source = ?3 WHERE name = ?1",
        params![name.trim(), k, src],
    )?;
    if n == 0 {
        return Err(DbError::Other(format!("模型组 {} 不存在", name.trim())));
    }
    Ok(())
}

/// 实测学到的类型：**只在「还没定」时写**，已经有结论（不管谁定的）就一个字都不动。
///
/// 返回 true 表示这次真写进去了（调用方据此决定要不要刷内存里的那份）。
pub fn learn_kind(conn: &Connection, name: &str, kind: ModelKind) -> Result<bool> {
    if !kind.is_set() {
        return Ok(false);
    }
    let n = conn.execute(
        "UPDATE model_groups SET kind = ?2, kind_source = ?3
          WHERE name = ?1 AND kind = ''",
        params![name.trim(), kind.as_str(), KindSource::Auto.as_str()],
    )?;
    Ok(n > 0)
}

/// 新建或覆盖一组。`channels` 的顺序就是优先级顺序。
///
/// **整体一个事务**：引用了不存在的渠道时必须一点痕迹都不留 —— 否则会留下一个
/// 空壳模型组（名字在、渠道丢了），面板上看着像配好了，实际一条渠道都没有。
pub fn upsert(conn: &Connection, name: &str, enabled: bool, channels: &[ChannelId]) -> Result<()> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(DbError::Other("模型组名不能为空".into()));
    }
    super::in_tx(conn, |c| {
        // 先校验再写：所有渠道都得存在
        for channel in channels {
            let exists: i64 =
                c.query_row("SELECT COUNT(*) FROM channels WHERE id = ?1", [channel.0], |r| {
                    r.get(0)
                })?;
            if exists == 0 {
                return Err(DbError::Other(format!(
                    "模型组 {name} 引用了不存在的渠道 {channel}"
                )));
            }
        }
        c.execute(
            "INSERT INTO model_groups (name, enabled) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET enabled = excluded.enabled",
            params![name, enabled as i64],
        )?;
        c.execute("DELETE FROM model_group_channels WHERE group_name = ?1", [&name])?;
        for (ord, channel) in channels.iter().enumerate() {
            c.execute(
                "INSERT INTO model_group_channels (group_name, channel_id, ordinal) VALUES (?1, ?2, ?3)",
                params![name, channel.0, ord as i64],
            )?;
        }
        Ok(())
    })
}

pub fn delete(conn: &Connection, name: &str) -> Result<()> {
    conn.execute("DELETE FROM model_groups WHERE name = ?1", [name])?;
    conn.execute("DELETE FROM routing_state WHERE group_name = ?1", [name])?;
    Ok(())
}

/// 把库里的一组解成路由要用的形状（`ModelGroup`）。
pub fn resolve(conn: &Connection, name: &str) -> Result<Option<ModelGroup>> {
    let Some(g) = get(conn, name)? else {
        return Ok(None);
    };
    Ok(Some(ModelGroup {
        name: g.name,
        channels: g.channels,
        enabled: g.enabled,
    }))
}

/// 取出这一组要用到的渠道，已按优先级定好 `priority`、按组名查好 `model_alias`。
///
/// `model_alias` 按 **组名** 查（客户端打 `auto`，就把渠道表里 model_name=`auto`
/// 的那行当别名）。查不到就 None = 原样透传。
///
/// **视觉能力也挂在同一行上**：`auto` 在 A 渠道上落到能看图的模型、在 B 渠道上落到
/// 纯文本模型是常事，所以 `vision` 跟着这条渠道这一行的标记走，不做全局推断。
///
/// **勾选就是白名单，而且是严格白名单**（v0.9.0，用户明确要求）：
/// 一条渠道**只认它勾上的那些名字**。这个组名没勾 → 直接跳过，请求不会发给它；
/// **一个模型名都没勾的渠道谁也不认**（v0.7.0 那版是「没勾 = 什么都认」，v0.9.0 取消了这个例外）。
///
/// 所以新建渠道保存完还不通，得回「模型」页（或编辑弹窗）把要放行的名字逐条勾上。
/// 为什么这么做：面板上的勾选本来就是用户在说「这条渠道认哪些模型」，可以前它只决定
/// 别名/能不能看图/丢字段，请求照发 —— 取消勾选不是「不用它」，反而因为别名失效更容易
/// 失败。现在勾选与「会不会收到这个名字的请求」是同一件事。
///
/// 名字比对走 `same_model_name`（大小写/分隔符/行尾括注与 free 都归一），
/// 免得 `DeepSeek-V4.1-Flash` 与 `deepseek-v4.1-flash` 这种差异静默失效。
pub fn resolve_channels(conn: &Connection, group_name: &str) -> Result<Vec<Channel>> {
    let Some(g) = get(conn, group_name)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(g.channels.len());
    for (ord, id) in g.channels.iter().enumerate() {
        let Some(l) = super::channels::get(conn, *id)? else {
            continue;
        };
        // 严格白名单：没勾这个名字（含「一个都没勾」）→ 跳过这条渠道。
        let Some(mapping) = scope_mapping(&l.models, &g.name) else {
            tracing::debug!(
                "渠道 {} 没勾「{}」这个名字{}，这次请求跳过它",
                l.id,
                g.name,
                if l.models.is_empty() {
                    "（一个模型名都没勾：没勾就不放行）"
                } else {
                    "（勾了别的名字）"
                }
            );
            continue;
        };
        let alias = mapping.upstream.clone().filter(|u| !u.trim().is_empty());
        let vision = mapping.vision;
        let drop_fields = mapping.drop_fields.clone();
        out.push(Channel {
            id: l.id,
            name: l.name,
            base_url: l.base_url,
            enabled: l.enabled,
            priority: ord as i32,
            max_concurrent: l.max_concurrent,
            model_alias: alias,
            vision,
            drop_fields,
            unwrap_envelope: l.unwrap_envelope,
            headers: l.headers,
        });
    }
    Ok(out)
}

/// 这条渠道认不认 `group_name` 这个名字：认 → 返回这一行（带别名/能看图/丢字段），
/// 不认 → `None`。
///
/// **严格白名单（v0.9.0）**：只有勾上的名字才认。一个都没勾的渠道谁也不认 ——
/// 以前那个「没勾 = 什么都认」的例外取消了。
fn scope_mapping<'a>(
    models: &'a [super::channels::ModelMapping],
    group_name: &str,
) -> Option<&'a super::channels::ModelMapping> {
    models
        .iter()
        .find(|m| same_model_name(&m.model, group_name))
}

/// 这一组里**因为没勾这个名字而被跳过**的渠道名（给报错文案与面板用）。
///
/// 打一个模型名却一条渠道都没有时，最可能的原因就是它：组里挂着渠道，但那几条渠道
/// 勾的是别的名字。这时报错得说清是「没勾」，别让人去查 base_url。
/// 这个组里**这次请求不会发给它**的渠道：`(渠道名, 它勾过名字没有)`。
///
/// 严格白名单之后有两种原因：勾了别的名字（勾过 = true），或者一个都没勾（false）。
pub fn channels_skipped_by_scope(
    conn: &Connection,
    group_name: &str,
) -> Result<Vec<(String, bool)>> {
    let Some(g) = get(conn, group_name)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for id in g.channels.iter() {
        let Some(l) = super::channels::get(conn, *id)? else {
            continue;
        };
        // 带一个「它到底勾过名字没有」：两种情况的修法不一样，404 文案要分开说。
        if scope_mapping(&l.models, &g.name).is_none() {
            out.push((l.name, !l.models.is_empty()));
        }
    }
    Ok(out)
}

/// 对外可见的一个模型名 + 它这条组能不能吃图片（组里**任意**一条渠道标了能看图就算）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Servable {
    pub name: String,
    pub vision: bool,
    /// 这组是哪种模型 —— `/v1/models` 与面板都靠它区分「这是向量模型还是对话模型」。
    pub kind: ModelKind,
    /// 类型是谁定的（还没定 / 实测学的 / 人选的）。
    pub kind_source: KindSource,
}

/// 对外可见的模型名清单（供 `/v1/models`）。只列真实配好的组，且组里至少有一条可用渠道。
pub fn list_servable(conn: &Connection) -> Result<Vec<Servable>> {
    let mut out = Vec::new();
    for g in list(conn)? {
        if !g.enabled {
            continue;
        }
        let channels = resolve_channels(conn, &g.name)?;
        if channels.iter().any(|l| l.routable()) {
            out.push(Servable {
                // 只有对话模型才谈「能不能看图」：向量/重排模型压根不收图，
                // 给它们挂个「不收图」的标签只会让人以为配错了。
                vision: g.kind != ModelKind::Embeddings
                    && g.kind != ModelKind::Rerank
                    && group_can_see(conn, &g.name)?,
                name: g.name.clone(),
                kind: g.kind,
                kind_source: g.kind_source,
            });
        }
    }
    Ok(out)
}

/// 这一组现在有没有「能看图」的可用渠道。带图的请求要按它决定走不走得通。
pub fn group_can_see(conn: &Connection, group_name: &str) -> Result<bool> {
    Ok(resolve_channels(conn, group_name)?
        .iter()
        .any(|l| l.routable() && l.vision))
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
                max_concurrent: 0,
                base_url: format!("http://127.0.0.1/{}", name),
                enabled: true,
                unwrap_envelope: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn upsert_keeps_channel_order() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "硅基流动");
        let b = channel(&conn, "opencode-go");
        let c = channel(&conn, "cline");
        upsert(&conn, "auto", true, &[a, b, c]).unwrap();

        let g = get(&conn, "auto").unwrap().unwrap();
        assert_eq!(g.channels, vec![a, b, c]);

        // 换顺序：这就是「优先级」在库里的样子
        upsert(&conn, "auto", true, &[b, a, c]).unwrap();
        assert_eq!(get(&conn, "auto").unwrap().unwrap().channels, vec![b, a, c]);
    }

    #[test]
    fn upsert_replaces_membership_not_appends() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "a");
        let b = channel(&conn, "b");
        upsert(&conn, "auto", true, &[a, b]).unwrap();
        upsert(&conn, "auto", true, &[b]).unwrap();
        assert_eq!(get(&conn, "auto").unwrap().unwrap().channels, vec![b]);
    }

    #[test]
    fn rejects_unknown_channel() {
        let conn = open_in_memory().unwrap();
        let r = upsert(&conn, "auto", true, &[ChannelId(999)]);
        assert!(r.is_err());
        assert!(get(&conn, "auto").unwrap().is_none(), "失败要整体不生效");
    }

    /// 反证（真踩过）：已经有的一组装到一半失败，**不能把原来配好的那组弄丢**。
    /// 之前没上事务，第 2 条渠道校验失败时第一条已经被删掉了。
    #[test]
    fn a_failed_upsert_leaves_the_previous_group_intact() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "a");
        let b = channel(&conn, "b");
        upsert(&conn, "auto", true, &[a, b]).unwrap();

        // 想把组成 [a, 不存在的渠道]，必须失败且原样不动
        let r = upsert(&conn, "auto", true, &[a, ChannelId(999)]);
        assert!(r.is_err());
        assert_eq!(
            get(&conn, "auto").unwrap().unwrap().channels,
            vec![a, b],
            "失败的保存把原来配好的渠道弄丢了"
        );
    }

    /// 反证：一组里前面几条渠道都合法、最后一条不合法时，连主行都不该建。
    #[test]
    fn a_failed_upsert_does_not_create_a_hollow_group() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "a");
        assert!(upsert(&conn, "brand-new", true, &[a, ChannelId(999)]).is_err());
        assert!(
            get(&conn, "brand-new").unwrap().is_none(),
            "留下了一个没有渠道的空壳模型组"
        );
    }

    #[test]
    fn resolve_channels_sets_priority_and_alias_by_group_name() {
        let conn = open_in_memory().unwrap();
        let sf = channel(&conn, "硅基流动");
        let cg = channel(&conn, "command-go");
        // 严格白名单：两条渠道都得勾上 auto，才都会被服务
        channels::set_models(&conn, sf, &[ModelMapping::new("auto", None, false)]).unwrap();
        channels::set_models(
            &conn,
            cg,
            &[ModelMapping::new(
                "auto",
                Some("deepseek/deepseek-v4.1-flash".into()),
                false,
            )],
        )
        .unwrap();
        upsert(&conn, "auto", true, &[sf, cg]).unwrap();

        let ls = resolve_channels(&conn, "auto").unwrap();
        assert_eq!(ls.len(), 2);
        assert_eq!(ls[0].id, sf);
        assert_eq!(ls[0].priority, 0);
        assert_eq!(ls[0].model_alias, None, "硅基流动没配别名，原样透传");
        assert_eq!(ls[1].id, cg);
        assert_eq!(ls[1].priority, 1);
        assert_eq!(
            ls[1].upstream_model("auto"),
            "deepseek/deepseek-v4.1-flash",
            "按渠道改名的关键是这个"
        );
    }

    /// 视觉标记必须**按组名那一行**取：同一批渠道，`auto` 这行标了能看图，
    /// 别的组名那行没标，各自算各自的。
    #[test]
    fn resolve_channels_carries_the_vision_flag_of_that_group_name() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "能看图的上游");
        let b = channel(&conn, "纯文本上游");
        channels::set_models(
            &conn,
            a,
            &[ModelMapping::new("auto", Some("glm-4v-flash".into()), true)],
        )
        .unwrap();
        channels::set_models(
            &conn,
            b,
            &[ModelMapping::new("auto", Some("deepseek-v4.1-flash".into()), false)],
        )
        .unwrap();
        upsert(&conn, "auto", true, &[a, b]).unwrap();

        let ls = resolve_channels(&conn, "auto").unwrap();
        assert!(ls[0].vision, "标了能看图的渠道要带出来");
        assert!(!ls[1].vision);

        // 换个组名：a 勾过名字（勾了 auto），却没勾这一个 → 这次请求根本不会发给它。
        // （v0.7.0 起「勾了名字 = 白名单」，见 resolve_channels 的说明。）
        upsert(&conn, "BAAI/bge-m3", true, &[a]).unwrap();
        assert!(
            resolve_channels(&conn, "BAAI/bge-m3").unwrap().is_empty(),
            "勾了别的名字的渠道，不该收到这个名字的请求"
        );

        // 一个名字都没勾的渠道（c）：严格白名单下谁也不认（v0.9.0）。
        let c = channel(&conn, "没勾过的上游");
        upsert(&conn, "BAAI/bge-m3", true, &[c]).unwrap();
        assert!(
            resolve_channels(&conn, "BAAI/bge-m3").unwrap().is_empty(),
            "没勾任何名字的渠道不该收到任何模型名的请求"
        );

        // 勾了这一个、只是没配别名：照样发给它，名字原样用。
        let d = channel(&conn, "勾了但没配别名");
        channels::set_models(
            &conn,
            d,
            &[ModelMapping::new("BAAI/bge-m3", None, false)],
        )
        .unwrap();
        upsert(&conn, "BAAI/bge-m3", true, &[c, d]).unwrap();
        let ls = resolve_channels(&conn, "BAAI/bge-m3").unwrap();
        assert_eq!(ls.len(), 1);
        assert!(!ls[0].vision, "它自己那一行没标能看图，就别无中生有");
    }

    /// 带图的请求能不能走这一组：只要有一条**可用且标了**的渠道就算能走。
    /// 停用的渠道不算（否则图会发到一条已经关掉的渠道的配置上，等于没有）。
    #[test]
    fn group_can_see_needs_a_routable_vision_channel() {
        let conn = open_in_memory().unwrap();
        let text = channel(&conn, "纯文本");
        let see = channel(&conn, "能看图");
        channels::set_models(
            &conn,
            see,
            &[ModelMapping::new("auto", Some("glm-4v-flash".into()), true)],
        )
        .unwrap();
        upsert(&conn, "auto", true, &[text, see]).unwrap();
        assert!(group_can_see(&conn, "auto").unwrap());

        channels::update(&conn, see, "能看图", "http://127.0.0.1/see", false, false, 0).unwrap();
        assert!(!group_can_see(&conn, "auto").unwrap(), "停用的渠道不算数");

        upsert(&conn, "空的", true, &[text]).unwrap();
        assert!(!group_can_see(&conn, "空的").unwrap());
        assert!(!group_can_see(&conn, "没这组").unwrap());
    }

    #[test]
    fn resolve_channels_carries_custom_headers() {
        let conn = open_in_memory().unwrap();
        let oc = channel(&conn, "opencode-go");
        channels::set_headers(
            &conn,
            oc,
            &[("x-opencode-session".into(), "{{uuid}}".into())],
        )
        .unwrap();
        channels::set_models(&conn, oc, &[ModelMapping::new("auto", None, false)]).unwrap();
        upsert(&conn, "auto", true, &[oc]).unwrap();
        let ls = resolve_channels(&conn, "auto").unwrap();
        assert_eq!(ls[0].headers.len(), 1);
        assert_eq!(ls[0].headers[0].1, "{{uuid}}");
    }

    #[test]
    fn deleting_a_channel_drops_it_from_groups() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "a");
        let b = channel(&conn, "b");
        upsert(&conn, "auto", true, &[a, b]).unwrap();
        channels::delete(&conn, a).unwrap();
        assert_eq!(get(&conn, "auto").unwrap().unwrap().channels, vec![b]);
    }

    #[test]
    fn servable_list_hides_disabled_channels_and_empty_groups() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "a");
        channels::set_models(&conn, a, &[ModelMapping::new("auto", None, false)]).unwrap();
        upsert(&conn, "auto", true, &[a]).unwrap();
        upsert(&conn, "empty", true, &[]).unwrap();
        upsert(&conn, "off", false, &[a]).unwrap();
        assert_eq!(
            list_servable(&conn).unwrap(),
            vec![Servable {
                name: "auto".into(),
                vision: false,
                kind: ModelKind::Unknown,
                kind_source: KindSource::Unknown,
            }]
        );

        channels::update(&conn, a, "a", "http://x", false, false, 0).unwrap();
        assert!(
            list_servable(&conn).unwrap().is_empty(),
            "组里全是停用的渠道，就不该再对外暴露这个模型名"
        );
    }

    /// 清单里要如实说这个模型名能不能吃图 —— 客户端（Hermes 之类）按它决定
    /// 要不要把图发过来。
    #[test]
    fn the_servable_list_says_which_model_names_can_see() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "能看图");
        let b = channel(&conn, "纯文本");
        channels::set_models(
            &conn,
            a,
            &[
                ModelMapping::new("auto", Some("glm-4v-flash".into()), true),
                ModelMapping::new("BAAI/bge-m3", None, false),
            ],
        )
        .unwrap();
        upsert(&conn, "auto", true, &[b, a]).unwrap();
        upsert(&conn, "BAAI/bge-m3", true, &[a]).unwrap();

        let m = list_servable(&conn).unwrap();
        let auto = m.iter().find(|s| s.name == "auto").unwrap();
        let vec = m.iter().find(|s| s.name == "BAAI/bge-m3").unwrap();
        assert!(auto.vision, "auto 后面有能看图的渠道");
        assert!(!vec.vision, "向量那条组里没有能看图的渠道");
    }

    /// `/v1/models` 只列真实能用的 —— 用户明确要求过。
    #[test]
    fn only_really_usable_models_are_advertised() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "a");
        channels::set_models(
            &conn,
            a,
            &[
                ModelMapping::new("auto", None, false),
                ModelMapping::new("deepseek-v4.1-flash", None, false),
            ],
        )
        .unwrap();
        upsert(&conn, "auto", true, &[a]).unwrap();
        upsert(&conn, "deepseek-v4.1-flash", true, &[a]).unwrap();
        let m: Vec<String> = list_servable(&conn).unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(m, vec!["auto".to_string(), "deepseek-v4.1-flash".to_string()]);
        // 库里没有目录表，模型名只来自用户亲手建的组
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM model_groups", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    /// 类型：先空着 → 实测学到一次 → **之后再也不被覆盖**（不管来的是实测还是别人）。
    #[test]
    fn the_kind_is_learned_once_and_then_never_overwritten_by_traffic() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "a");
        upsert(&conn, "BAAI/bge-m3", true, &[a]).unwrap();

        let g = get(&conn, "BAAI/bge-m3").unwrap().unwrap();
        assert_eq!(g.kind, ModelKind::Unknown, "新建的组一律「还没定」，不许按名字猜");
        assert_eq!(g.kind_source, KindSource::Unknown);

        // 第一次真有向量请求进来 → 学上了
        assert!(learn_kind(&conn, "BAAI/bge-m3", ModelKind::Embeddings).unwrap());
        let g = get(&conn, "BAAI/bge-m3").unwrap().unwrap();
        assert_eq!(g.kind, ModelKind::Embeddings);
        assert_eq!(g.kind_source, KindSource::Auto);

        // 再来一次实测（这次说是对话）→ **一个字都不许改**，也不该报告「改了」
        assert!(!learn_kind(&conn, "BAAI/bge-m3", ModelKind::Chat).unwrap());
        let g = get(&conn, "BAAI/bge-m3").unwrap().unwrap();
        assert_eq!(g.kind, ModelKind::Embeddings, "已经定下来的类型不许被流量改掉");
        assert_eq!(g.kind_source, KindSource::Auto, "来源也不许被改");

        // 人手动定 → 来源变 manual，而且实测更动不了它
        set_kind(&conn, "BAAI/bge-m3", ModelKind::Chat).unwrap();
        let g = get(&conn, "BAAI/bge-m3").unwrap().unwrap();
        assert_eq!(g.kind, ModelKind::Chat);
        assert_eq!(g.kind_source, KindSource::Manual);
        assert!(!learn_kind(&conn, "BAAI/bge-m3", ModelKind::Embeddings).unwrap());
        assert_eq!(get(&conn, "BAAI/bge-m3").unwrap().unwrap().kind, ModelKind::Chat);

        // 交还给实测（auto）→ 清空，下次流量再学
        set_kind(&conn, "BAAI/bge-m3", ModelKind::Unknown).unwrap();
        let g = get(&conn, "BAAI/bge-m3").unwrap().unwrap();
        assert_eq!(g.kind, ModelKind::Unknown);
        assert!(learn_kind(&conn, "BAAI/bge-m3", ModelKind::Embeddings).unwrap());
    }

    /// 反证：向量组 / 重排组**不该**被贴上「能看图」的标签（它们压根不收图）。
    #[test]
    fn vector_and_rerank_groups_are_never_marked_as_seeing_images() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "既标了能看图又能发向量");
        channels::set_models(
            &conn,
            a,
            &[
                ModelMapping::new("BAAI/bge-m3", None, true),
                ModelMapping::new("auto", None, true),
            ],
        )
        .unwrap();
        upsert(&conn, "BAAI/bge-m3", true, &[a]).unwrap();
        upsert(&conn, "auto", true, &[a]).unwrap();
        set_kind(&conn, "BAAI/bge-m3", ModelKind::Embeddings).unwrap();

        let m = list_servable(&conn).unwrap();
        let vec = m.iter().find(|s| s.name == "BAAI/bge-m3").unwrap();
        let chat = m.iter().find(|s| s.name == "auto").unwrap();
        assert!(!vec.vision, "向量模型不看图，不许挂「能看图」");
        assert_eq!(vec.kind, ModelKind::Embeddings);
        assert!(chat.vision, "对话组照旧按渠道上的标记说能不能看图");

        // 还没定的组按「可能能看图」处理（保持老行为，不因为没类型就丢掉能力）
        channels::set_models(
            &conn,
            a,
            &[ModelMapping::new("新的", None, true)],
        )
        .unwrap();
        upsert(&conn, "新的", true, &[a]).unwrap();
        let new = list_servable(&conn).unwrap().into_iter().find(|s| s.name == "新的").unwrap();
        assert_eq!(new.kind, ModelKind::Unknown);
        assert!(new.vision, "还没定的组不该被当成向量组、把能看图的能力丢掉");
    }

    /// 勾选就是白名单，而且是**严格**白名单（v0.9.0）。
    ///
    /// 一条渠道只认勾上的那些；**一个名字都没勾** → 谁也不认（v0.7.0 那版是「什么都认」，
    /// 用户 2026-10-18 明确改成严格版）。
    #[test]
    fn declaring_one_name_turns_a_channel_into_a_whitelist() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "渠道A");
        let b = channel(&conn, "渠道B");

        // 谁都没勾：两个组名都不放行（严格版，v0.9.0 起）
        upsert(&conn, "auto", true, &[a, b]).unwrap();
        upsert(&conn, "deepseek-v4.1-flash", true, &[a, b]).unwrap();
        assert!(
            resolve_channels(&conn, "auto").unwrap().is_empty(),
            "没勾任何名字 = 谁也不认"
        );
        assert!(resolve_channels(&conn, "deepseek-v4.1-flash").unwrap().is_empty());

        // B 勾上 auto（严格版下它也只认 auto）
        channels::set_models(&conn, b, &[ModelMapping::new("auto", None, false)]).unwrap();
        assert_eq!(resolve_channels(&conn, "auto").unwrap().len(), 1);
        assert_eq!(
            channels_skipped_by_scope(&conn, "auto").unwrap(),
            vec![("渠道A".to_string(), false)],
            "A 一个都没勾 → 跳过，理由标成「没勾过名字」"
        );

        // A 勾了 auto（带别名与能看图）
        channels::set_models(
            &conn,
            a,
            &[ModelMapping::new("auto", Some("v4.1flash".into()), true)],
        )
        .unwrap();

        let auto = resolve_channels(&conn, "auto").unwrap();
        assert_eq!(auto.len(), 2, "勾了的那个名字当然还发给它");
        assert_eq!(auto[0].upstream_model("auto"), "v4.1flash");

        // flash：两条渠道都没勾这个名字 → 谁都不服务（严格版）
        assert!(
            resolve_channels(&conn, "deepseek-v4.1-flash").unwrap().is_empty(),
            "都没勾这个名字，请求不该发给任何一条"
        );

        // 报错文案要能说清是「没勾」，别让人去查 base_url；
        // 括号里那个 bool = 它到底勾过名字没有（false = 一个都没勾，修法不同）
        assert_eq!(
            channels_skipped_by_scope(&conn, "deepseek-v4.1-flash").unwrap(),
            vec![("渠道A".to_string(), true), ("渠道B".to_string(), true)]
        );
    }

    /// 名字比对归一：大小写、分隔符、行尾括注与 free 都算同一个名字 ——
    /// 不然手抄错一个大小写，别名/能看图/丢字段就全静默失效了。
    #[test]
    fn the_name_match_ignores_case_and_separators() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "渠道A");
        channels::set_models(
            &conn,
            a,
            &[ModelMapping::new(
                "DeepSeek-V4.1-Flash",
                Some("deepseek/deepseek-v4.1-flash".into()),
                true,
            )],
        )
        .unwrap();
        upsert(&conn, "deepseek-v4.1-flash", true, &[a]).unwrap();
        upsert(&conn, "DeepSeek V4.1 Flash Free", true, &[a]).unwrap();

        for g in ["deepseek-v4.1-flash", "DeepSeek V4.1 Flash Free"] {
            let ls = resolve_channels(&conn, g).unwrap();
            assert_eq!(ls.len(), 1, "组名 {g} 该认出这条渠道");
            assert_eq!(
                ls[0].upstream_model(g),
                "deepseek/deepseek-v4.1-flash",
                "组名写法不一样也得带上别名"
            );
            assert!(ls[0].vision, "能看图的标记也跟这一行走");
        }
    }

    /// 全组都被闸掉时，这个模型名就不再对外暴露（`/v1/models` 不该列它）。
    #[test]
    fn a_fully_gated_group_is_not_advertised() {
        let conn = open_in_memory().unwrap();
        let a = channel(&conn, "渠道A");
        channels::set_models(&conn, a, &[ModelMapping::new("别的名字", None, false)]).unwrap();
        upsert(&conn, "auto", true, &[a]).unwrap();
        assert!(
            list_servable(&conn).unwrap().is_empty(),
            "一条渠道都没认这个名字，就不该对外说它可用"
        );
    }

    /// 能看图的标记也要过闸门：标了能看图、但没勾这个名字的渠道，
    /// 不能让它把整组撑成「能看图」。
    #[test]
    fn group_can_see_ignores_channels_gated_out_by_scope() {
        let conn = open_in_memory().unwrap();
        let seeing = channel(&conn, "能看图的");
        let text = channel(&conn, "纯文本");
        channels::set_models(
            &conn,
            seeing,
            &[ModelMapping::new("别的名字", Some("glm-4v-flash".into()), true)],
        )
        .unwrap();
        channels::set_models(
            &conn,
            text,
            &[ModelMapping::new("auto", Some("deepseek-v4.1-flash".into()), false)],
        )
        .unwrap();
        upsert(&conn, "auto", true, &[seeing, text]).unwrap();
        assert!(
            !group_can_see(&conn, "auto").unwrap(),
            "没勾这个名字的渠道，就算标了能看图也不算这一组的能力"
        );
    }
}
