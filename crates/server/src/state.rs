//! 进程共享状态。所有 handler 拿到的都是 `Arc<AppState>`。
//!
//! 两把锁分工：`db` 管持久化，`routing` 管"当前在用哪条渠道"。路由锁只圈住内存
//! 状态机的读写（微秒级），**绝不能把网络请求圈进去** —— 否则一条渠道卡住，
//! 整个网关的所有请求都跟着排队。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::path::Path;

use rusqlite::Connection;
use tokengo_shared::routing::Router as RoutingRouter;

use crate::config::Config;
use crate::crypto::SecretBox;
use crate::db;
use crate::gateway::EndpointKind;
use crate::gateway_key::GatewayKeyCache;
use crate::upstream::Upstream;

pub struct AppState {
    pub db: Mutex<Connection>,
    pub routing: Mutex<RoutingRouter>,
    pub secrets: SecretBox,
    pub cfg: Config,
    /// 打上游的 HTTP 客户端（连接池复用，进程里只有这一个）。
    pub upstream: Upstream,
    /// 对外 key 的校验快路径。argon2 太慢，不能每请求跑一遍。
    key_cache: GatewayKeyCache,
    /// 每个模型组平时走哪个端点 —— 后台探活要知道，否则给向量组发对话探活必 400。
    group_endpoints: Mutex<HashMap<String, EndpointKind>>,
}

impl AppState {
    /// 起进程时调一次：开库、跑迁移、恢复路由状态、准备主密钥。
    pub fn init(cfg: Config) -> anyhow::Result<Arc<Self>> {
        let conn = db::open(&cfg.db_path)?;
        tracing::info!("数据库 {} 就绪", cfg.db_path.display());

        // 把库切成「增量回收」模式。这一步必须在库还小的时候做（改这个属性要整库重写），
        // 而且**只做一次**：不做的话，下面清理任务删掉的行虽然腾出了空页，但 SQLite
        // 永远不把空页还给磁盘 —— 表现就是「说清了几千条，文件大小一点没变」。
        if let Ok(true) = db::maintenance::ensure_incremental_vacuum(&conn) {
            tracing::info!("数据库已切到增量回收模式，清理腾出的空间会真的还给磁盘");
        }

        let secrets = SecretBox::load_or_init(Path::new(&cfg.master_key_path))?;

        let rcfg = db::routing::load_config(&conn)?;
        let mut router = RoutingRouter::new(rcfg);
        let restored = db::routing::load_all_into(&conn, &mut router)?;
        if restored > 0 {
            tracing::info!("已恢复 {restored} 个模型组的路由状态");
        }
        if let Ok(n) = db::sessions::purge_expired(&conn) {
            if n > 0 {
                tracing::info!("清理了 {n} 个过期面板会话");
            }
        }

        Ok(Arc::new(Self {
            db: Mutex::new(conn),
            routing: Mutex::new(router),
            secrets,
            cfg,
            upstream: Upstream::new()?,
            key_cache: GatewayKeyCache::new(),
            group_endpoints: Mutex::new(HashMap::new()),
        }))
    }

    /// 校验对外统一 key。
    ///
    /// 第一次真跑 argon2（安全边界没变），之后走内存快路径；换 key 之后必须调
    /// `forget_gateway_key()`，否则旧 key 还能用最多一个缓存周期。
    pub fn check_gateway_key(&self, key: &str) -> bool {
        if key.trim().is_empty() {
            return false;
        }
        let now = db::now_ms();
        if self.key_cache.hit(key, now) {
            return true;
        }
        let ok = {
            let conn = self.conn();
            db::sessions::check_gateway_key(&conn, key).unwrap_or(false)
        };
        if ok {
            self.key_cache.remember(key, now);
        }
        ok
    }

    /// 换 key / 清 key 之后调它。
    pub fn forget_gateway_key(&self) {
        self.key_cache.forget();
    }

    /// 记下这个模型组平时走哪个端点（转发时顺手调，探活用）。
    pub fn note_group_endpoint(&self, group: &str, kind: EndpointKind) {
        let mut g = self.group_endpoints.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(group.to_string(), kind);
    }

    /// 这个模型组的探活该打哪个端点。没见过流量就按对话探（对话探活对向量组会 400，
    /// 探活那边把它当"没结论"，不会乱改路由状态）。
    pub fn group_endpoint(&self, group: &str) -> EndpointKind {
        self.group_endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(group)
            .copied()
            .unwrap_or(EndpointKind::Chat)
    }

    /// 拿数据库锁。锁中毒（有 handler panic 过）时直接恢复继续用 ——
    /// 死锁比 panic 更糟，宁可带着可能不一致的内存状态继续跑。
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 拿路由锁。**圈住的代码里不许有 await / 网络调用。**
    pub fn routing(&self) -> MutexGuard<'_, RoutingRouter> {
        self.routing.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 改了路由状态就立刻落库 —— 不然重启会丢掉"当前渠道"和"回切检查点"。
    ///
    /// **用 `try_lock` 而不是死等**：这个方法内部要同时拿路由锁和数据库锁。
    /// 如果调用方自己已经握着其中一把（比如函数开头 `let conn = state.conn();`
    /// 一直没放），死等就会把整个服务卡死 —— 而且日志里什么都看不到，只表现为
    /// "服务没反应"。改成立刻返回错误，至少能在日志里看见是哪调错了。
    pub fn persist_state(&self, group: &str) -> db::Result<()> {
        let st = {
            let router = self.routing.try_lock().map_err(|_| {
                db::DbError::Other(format!(
                    "内部错误：persist_state({group}) 被重入调用 —— 调用方正持有路由锁"
                ))
            })?;
            router.state(group).cloned()
        };
        let Some(st) = st else {
            return Ok(());
        };
        let conn = self.db.try_lock().map_err(|_| {
            db::DbError::Other(format!(
                "内部错误：persist_state({group}) 被重入调用 —— 调用方正持有数据库锁"
            ))
        })?;
        db::routing::save_state(&conn, group, &st)
    }
}

pub type SharedState = Arc<AppState>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::SecretBox;

    fn test_cfg(dir: &std::path::Path) -> Config {
        Config {
            listen: "127.0.0.1:0".into(),
            db_path: dir.join("t.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            web_dir: None,
        }
    }

    #[test]
    fn init_opens_db_and_creates_a_master_key_once() {
        std::env::remove_var(crate::crypto::MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = test_cfg(&dir);

        let a = AppState::init(cfg.clone()).unwrap();
        assert!(cfg.db_path.exists());
        assert!(std::path::Path::new(&cfg.master_key_path).exists());

        // 第二次启动复用同一把钥匙：A 存的密钥 B 能解
        let b = AppState::init(cfg.clone()).unwrap();
        let id = db::channels::create(
            &a.conn(),
            &db::channels::NewChannel {
                name: "t".into(),
                base_url: "http://x/v1".into(),
                enabled: true,
            },
        )
        .unwrap();
        db::channels::set_secret(&a.conn(), &a.secrets, id, "sk-abc").unwrap();
        assert_eq!(
            db::channels::secret(&b.conn(), &b.secrets, id).unwrap().unwrap(),
            "sk-abc",
            "重启后必须还能解出已存的密钥"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn routing_state_survives_a_restart_through_the_state_object() {
        std::env::remove_var(crate::crypto::MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = test_cfg(&dir);

        let channels = [
            tokengo_shared::ChannelId(1),
            tokengo_shared::ChannelId(2),
            tokengo_shared::ChannelId(3),
        ];
        let t0 = 1_800_000_000_000i64;

        let a = AppState::init(cfg.clone()).unwrap();
        {
            let mut r = a.routing();
            r.attempt_order("auto", &channels, t0);
            r.on_channel_exhausted("auto", channels[0], &channels, t0);
        }
        a.persist_state("auto").unwrap();

        let b = AppState::init(cfg).unwrap();
        assert_eq!(
            b.routing().state("auto").unwrap().active,
            Some(channels[1]),
            "重启后当前渠道必须接上"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 反证：RoutingConfig 存库后重启要能读回改过的值。
    #[test]
    fn routing_config_survives_a_restart() {
        std::env::remove_var(crate::crypto::MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = test_cfg(&dir);

        let a = AppState::init(cfg.clone()).unwrap();
        let mut c = a.routing().cfg.clone();
        c.reprobe_interval_ms = 60_000;
        db::routing::save_config(&a.conn(), &c).unwrap();

        let b = AppState::init(cfg).unwrap();
        assert_eq!(b.routing().cfg.reprobe_interval_ms, 60_000);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn secrets_helper_is_usable_independently() {
        let sb = SecretBox::from_hex(
            "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
        )
        .unwrap();
        let (n, c) = sb.seal("x").unwrap();
        assert_eq!(sb.open(&n, &c).unwrap(), "x");
    }

    /// 反证（真踩过，表现为"服务卡死、日志空白"）：调用方已经握着数据库锁时
    /// 再调 `persist_state`，必须立刻报错，**不能死等**。
    #[test]
    fn persist_state_reports_reentrancy_instead_of_deadlocking() {
        std::env::remove_var(crate::crypto::MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::init(test_cfg(&dir)).unwrap();

        {
            let mut r = st.routing();
            r.attempt_order(
                "auto",
                &[tokengo_shared::ChannelId(1), tokengo_shared::ChannelId(2)],
                1_800_000_000_000,
            );
        }

        // 握着数据库锁不放，然后再调
        let _held = st.conn();
        let r = st.persist_state("auto");
        assert!(r.is_err(), "重入必须报错");
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("重入"), "报错信息要说清原因: {msg}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 反证：握着路由锁时调 persist_state 同样要报错而不是卡死。
    #[test]
    fn persist_state_reports_reentrancy_on_the_routing_lock_too() {
        std::env::remove_var(crate::crypto::MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::init(test_cfg(&dir)).unwrap();

        let _held = st.routing();
        let r = st.persist_state("auto");
        assert!(r.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 正常路径：两个锁都没人拿的时候能顺利落库。
    #[test]
    fn persist_state_works_when_nothing_is_held() {
        std::env::remove_var(crate::crypto::MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::init(test_cfg(&dir)).unwrap();

        let channels = [tokengo_shared::ChannelId(1), tokengo_shared::ChannelId(2)];
        {
            let mut r = st.routing();
            r.attempt_order("auto", &channels, 1_800_000_000_000);
            r.on_channel_exhausted("auto", channels[0], &channels, 1_800_000_000_000);
        }
        st.persist_state("auto").unwrap();
        assert_eq!(
            db::routing::load_state(&st.conn(), "auto")
                .unwrap()
                .unwrap()
                .active,
            Some(channels[1])
        );

        // 未知组不算错，静默跳过
        st.persist_state("no-such-group").unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }
}
