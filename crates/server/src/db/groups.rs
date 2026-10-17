//! 模型组仓储：一个对外模型名 → 一张**有序**渠道列表。顺序即优先级。

use rusqlite::{params, Connection, OptionalExtension, Row};

use tokengo_shared::{Channel, ChannelId, ModelGroup};

use super::{DbError, Result};

/// 库里的一组。
#[derive(Debug, Clone, PartialEq)]
pub struct GroupRow {
    pub name: String,
    pub enabled: bool,
    /// 已按 ordinal 排好序
    pub channels: Vec<ChannelId>,
}

fn row_to_group(r: &Row<'_>) -> rusqlite::Result<GroupRow> {
    Ok(GroupRow {
        name: r.get(0)?,
        enabled: r.get::<_, i64>(1)? != 0,
        channels: Vec::new(),
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
    let mut stmt =
        conn.prepare("SELECT name, enabled FROM model_groups ORDER BY name")?;
    let mut rows: Vec<GroupRow> = stmt.query_map([], row_to_group)?.collect::<rusqlite::Result<_>>()?;
    for g in &mut rows {
        load_channels(conn, g)?;
    }
    Ok(rows)
}

pub fn get(conn: &Connection, name: &str) -> Result<Option<GroupRow>> {
    let mut stmt = conn.prepare("SELECT name, enabled FROM model_groups WHERE name = ?1")?;
    let mut g: Option<GroupRow> = stmt.query_row([name], row_to_group).optional()?;
    if let Some(g) = &mut g {
        load_channels(conn, g)?;
    }
    Ok(g)
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
pub fn resolve_channels(conn: &Connection, group_name: &str) -> Result<Vec<Channel>> {
    let Some(g) = get(conn, group_name)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(g.channels.len());
    for (ord, id) in g.channels.iter().enumerate() {
        let Some(l) = super::channels::get(conn, *id)? else {
            continue;
        };
        let mapping: Option<(Option<String>, i64)> = conn
            .query_row(
                "SELECT upstream_model, vision FROM channel_models WHERE channel_id = ?1 AND model_name = ?2",
                params![id.0, g.name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (alias, vision) = match mapping {
            Some((up, v)) => (up.filter(|u| !u.trim().is_empty()), v != 0),
            None => (None, false),
        };
        out.push(Channel {
            id: l.id,
            name: l.name,
            base_url: l.base_url,
            enabled: l.enabled,
            priority: ord as i32,
            model_alias: alias,
            vision,
            headers: l.headers,
        });
    }
    Ok(out)
}

/// 对外可见的一个模型名 + 它这条组能不能吃图片（组里**任意**一条渠道标了能看图就算）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Servable {
    pub name: String,
    pub vision: bool,
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
                name: g.name.clone(),
                vision: group_can_see(conn, &g.name)?,
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
                base_url: format!("http://127.0.0.1/{}", name),
                enabled: true,
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

        // 换个组名：这条渠道上没有那一行 → 一律按"不能看图"算，绝不靠猜
        upsert(&conn, "BAAI/bge-m3", true, &[a]).unwrap();
        let ls = resolve_channels(&conn, "BAAI/bge-m3").unwrap();
        assert!(!ls[0].vision, "换了个组名就是另一行，不能沿用 auto 那行的标记");
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

        channels::update(&conn, see, "能看图", "http://127.0.0.1/see", false).unwrap();
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
        upsert(&conn, "auto", true, &[a]).unwrap();
        upsert(&conn, "empty", true, &[]).unwrap();
        upsert(&conn, "off", false, &[a]).unwrap();
        assert_eq!(
            list_servable(&conn).unwrap(),
            vec![Servable {
                name: "auto".into(),
                vision: false
            }]
        );

        channels::update(&conn, a, "a", "http://x", false).unwrap();
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
}
