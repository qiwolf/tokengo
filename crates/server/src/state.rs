//! 进程共享状态。所有 handler 拿到的都是 `Arc<AppState>`。
//!
//! 两把锁分工：`db` 管持久化，`routing` 管"当前在用哪条渠道"。路由锁只圈住内存
//! 状态机的读写（微秒级），**绝不能把网络请求圈进去** —— 否则一条渠道卡住，
//! 整个网关的所有请求都跟着排队。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use rusqlite::Connection;
use tokengo_shared::routing::Router as RoutingRouter;
use tokio::sync::Notify;

use crate::config::Config;
use crate::crypto::SecretBox;
use crate::db;
use crate::gateway::EndpointKind;
use crate::gateway_key::GatewayKeyCache;
use crate::upstream::Upstream;

/// 模型类型 → 探活该打哪个端点。**还没定的返回 None**（那就按对话探，
/// 探活那边把 400 当「没结论」，不会乱改路由状态）。
fn kind_to_endpoint(kind: tokengo_shared::ModelKind) -> Option<EndpointKind> {
    match kind {
        tokengo_shared::ModelKind::Chat => Some(EndpointKind::Chat),
        tokengo_shared::ModelKind::Embeddings => Some(EndpointKind::Embeddings),
        tokengo_shared::ModelKind::Rerank => Some(EndpointKind::Rerank),
        // 图像 / 视频 / 音频：网关目前没有这几个转发端点，探活也就无从谈起。
        // **不能**糊一个对话端点上去 —— 那会让探活拿对话请求去打生图模型。
        tokengo_shared::ModelKind::Image
        | tokengo_shared::ModelKind::Video
        | tokengo_shared::ModelKind::Audio
        | tokengo_shared::ModelKind::Unknown => None,
    }
}

#[derive(Default)]
pub struct ModelLoads(pub Mutex<HashMap<String, usize>>);
pub struct ModelLoadTicket {
    loads: Arc<ModelLoads>,
    key: String,
}
impl Drop for ModelLoadTicket {
    fn drop(&mut self) {
        let mut m = self.loads.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = m.get_mut(&self.key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.key);
            }
        }
    }
}
impl ModelLoads {
 pub fn acquire(self:&Arc<Self>,name:&str)->ModelLoadTicket {let key=tokengo_shared::normalize_model_name(name);*self.0.lock().unwrap_or_else(|e|e.into_inner()).entry(key.clone()).or_default()+=1;ModelLoadTicket{loads:self.clone(),key}}
    pub fn select(
        self: &Arc<Self>,
        names: &mut [String],
        explored: bool,
    ) -> Option<ModelLoadTicket> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !explored {
            names.sort_by_key(|n| {
                m.get(&tokengo_shared::normalize_model_name(n))
                    .copied()
                    .unwrap_or(0)
            });
        }
        let key = tokengo_shared::normalize_model_name(names.first()?);
        *m.entry(key.clone()).or_default() += 1;
        Some(ModelLoadTicket {
            loads: self.clone(),
            key,
        })
    }
    pub fn preview(&self, names: &mut [String]) {
        let m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        names.sort_by_key(|n| {
            m.get(&tokengo_shared::normalize_model_name(n))
                .copied()
                .unwrap_or(0)
        });
    }
}

pub struct AppState {
    pub tool_rates: crate::source_rates::SourceRates,
    pub oauth: crate::oauth::OAuthRuntime,
    pub admission: crate::admission::AdmissionState,
    pub panel_login_gate: crate::routes::accounts::LoginGate,
    pub auth_slots: Arc<tokio::sync::Semaphore>,
    pub model_loads: Arc<ModelLoads>,
    pub auto_sequence: std::sync::atomic::AtomicU64,
    pub db: Mutex<Connection>,
    pub routing: Mutex<RoutingRouter>,
    pub secrets: SecretBox,
    pub cfg: Config,
    /// 打上游的 HTTP 客户端（连接池复用，进程里只有这一个）。
    pub upstream: Upstream,
    /// 对外 key 的校验快路径。argon2 太慢，不能每请求跑一遍。
    key_cache: GatewayKeyCache,
    /// 每个模型名平时走哪个端点 —— 后台探活要知道，否则给向量模型发对话探活必 400。
    model_endpoints: Mutex<HashMap<String, EndpointKind>>,
    /// 请求路径上抢不到库锁的活先丢这儿，由后台落账员补写（见 `WriteJob`）。
    writer: WriteQueue,
    /// 每条渠道的「门口闸机」：同时最多几发（渠道 id → 闸机 + 当前配置的上限）。
    channel_gates: Mutex<HashMap<i64, Arc<ChannelGate>>>,
    credential_gates: Mutex<HashMap<i64, Arc<ChannelGate>>>,
    /// 最近的运行日志（内存里那份，面板「日志」页读它）。
    /// 用 OnceLock：服务启动时先建 state、再装订阅者，装完把这一格填上；
    /// 测试里不装，取值时就地造一个空的 —— 两边都不用改代码。
    logs: OnceLock<Arc<crate::logging::LogBuffer>>,
    /// 日志文件在哪（面板上显示给用户看）。
    log_file: OnceLock<Option<std::path::PathBuf>>,
    /// 运行时改日志级别（面板上「临时开 debug」走它）。
    log_control: OnceLock<Arc<crate::logging::LogControl>>,
}

/// 一件「待落账」的活。
///
/// **为什么要有它**：2026-10-18 压测（并发 64 打 1024 发）发现，并发一上来
/// 有 3%~4% 的请求拿到 HTTP 500，正文是「persist_state 被重入调用 —— 调用方正持有
/// 数据库锁」。真实情况是：那发请求**上游已经答好了**（尝试账全是 success），
/// 只是记账那一步用 `try_lock` 抢不到库锁 —— 而 `try_lock` 分不清「别人正握着」
/// 和「我自己握着」，于是把一次成功的请求当成内部错误丢掉。
///
/// 定下来的规矩：**记账绝不允许影响请求结果**。抢不到锁就把活丢进这个队列，
/// 由后台落账员（`spawn_writer`）补写；请求该成功就成功。
#[derive(Debug)]
pub enum WriteJob {
    Progress(crate::db::requests::ProgressArgs),
    /// 请求收尾。
    Finish(crate::db::requests::FinishArgs),
    /// 一次渠道尝试。
    Attempt(crate::db::requests::AttemptArgs),
    /// 落某个模型名的路由状态（当前渠道、回切检查点）。
    State(String),
}

/// 待落账队列：一把短锁 + 一个唤醒信号。
#[derive(Default)]
struct WriteQueue {
    jobs: Mutex<Vec<WriteJob>>,
    // Only one consumer may deliver and requeue a batch. Otherwise a newer
    // progress batch can overtake an older one after its queue has been taken.
    draining: Mutex<()>,
    in_flight: std::sync::atomic::AtomicUsize,
    notify: Notify,
    /// 已经丢过多少件（只为观测/测试用）。
    queued: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    after_take: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl WriteQueue {
    fn push(&self, job: WriteJob) {
        {
            let mut q = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            // 上限只是防失控：正常几百件就落完了，真堆到这个数说明库一直抢不到，
            // 宁可丢账也不能把内存吃光。
            if q.len() >= 20_000 {
                tracing::warn!("待落账队列已满 2 万件，丢掉最老的一件");
                q.remove(0);
            }
            q.push(job);
        }
        self.queued
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.notify.notify_one();
    }

    /// 尚未完成的活：排队中 + 正在交付/准备重排的批次。
    pub fn len(&self) -> usize {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.len() + self.in_flight.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl AppState {
    /// 起进程时调一次：开库、跑迁移、恢复路由状态、准备主密钥。
    /// 把真正的日志缓冲与级别控制装进来（`main` 里调；测试不调，取用时兜底为空缓冲）。
    pub fn attach_logs(
        &self,
        logs: Arc<crate::logging::LogBuffer>,
        control: Arc<crate::logging::LogControl>,
        file: Option<std::path::PathBuf>,
    ) {
        let _ = self.logs.set(logs);
        let _ = self.log_control.set(control);
        let _ = self.log_file.set(file);
    }

    /// 最近的运行日志。没装订阅者（测试）就地造一个空的，行为一致、不会崩。
    pub fn logs(&self) -> Arc<crate::logging::LogBuffer> {
        self.logs
            .get_or_init(|| crate::logging::LogBuffer::new(crate::logging::BUFFER_LINES))
            .clone()
    }

    /// 日志文件路径（没落文件时为 None）。
    pub fn log_file(&self) -> Option<std::path::PathBuf> {
        self.log_file.get().cloned().flatten()
    }

    /// 日志级别控制。没装订阅者时是空操作（改级别只记账，不影响任何东西）。
    pub fn log_control(&self) -> Arc<crate::logging::LogControl> {
        self.log_control
            .get_or_init(crate::logging::LogControl::detached)
            .clone()
    }

    pub fn init(cfg: Config) -> anyhow::Result<Arc<Self>> {
        let conn = db::open(&cfg.db_path)?;
        crate::billing_writer::replay(&conn, &cfg.db_path)?;
        db::billing::recover(&conn)?;
        let interrupted = db::requests::recover_interrupted(&conn)?;
        if interrupted > 0 {
            tracing::warn!(interrupted, "已收尾重启前中断的请求");
        }
        tracing::info!("数据库 {} 就绪", cfg.db_path.display());

        // 把库切成「增量回收」模式。这一步必须在库还小的时候做（改这个属性要整库重写），
        // 而且**只做一次**：不做的话，下面清理任务删掉的行虽然腾出了空页，但 SQLite
        // 永远不把空页还给磁盘 —— 表现就是「说清了几千条，文件大小一点没变」。
        if let Ok(true) = db::maintenance::ensure_incremental_vacuum(&conn) {
            tracing::info!("数据库已切到增量回收模式，清理腾出的空间会真的还给磁盘");
        }

        db::accounts::migrate_legacy_sessions(&conn)?;
        let secrets = SecretBox::load_or_init(Path::new(&cfg.master_key_path))?;
        db::gateway_keys::hydrate_legacy(&conn, &secrets)?;
        db::channels::migrate_headers(&conn, &secrets)?;
        db::provider_pool::backfill(&conn, &secrets)?;

        let rcfg = db::routing::load_config(&conn)?;
        let mut router = RoutingRouter::new(rcfg);
        let restored = db::routing::load_all_into(&conn, &mut router)?;
        if restored > 0 {
            tracing::info!("已恢复 {restored} 个模型名的路由状态");
        }
        if let Ok(n) = db::sessions::purge_expired(&conn) {
            if n > 0 {
                tracing::info!("清理了 {n} 个过期面板会话");
            }
        }

        // 把库里记下来的「每个名字是哪种模型」装进内存：探活要知道该打哪个端点。
        // **不装的话重启后一律按对话探** —— 拿对话请求去探向量模型必然 400，
        // 表现就是「重启后向量模型一直探不通、回切永远不成功」。
        let mut endpoints = HashMap::new();
        for k in db::kinds::all(&conn).unwrap_or_default() {
            if let Some(endpoint) = kind_to_endpoint(k.kind) {
                endpoints.insert(k.name.clone(), endpoint);
            }
        }
        if !endpoints.is_empty() {
            tracing::info!(
                "已恢复 {} 个模型名的用途（{}）",
                endpoints.len(),
                endpoints
                    .iter()
                    .map(|(n, k)| format!("{n}={}", k.as_model_kind().label()))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }

        let state = Arc::new(Self {
            tool_rates: Default::default(),
            oauth: crate::oauth::OAuthRuntime::new()?,
            admission: crate::admission::AdmissionState::default(),
            panel_login_gate: Default::default(),
            auth_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            model_loads: Arc::new(ModelLoads::default()),
            auto_sequence: std::sync::atomic::AtomicU64::new(0),
            db: Mutex::new(conn),
            routing: Mutex::new(router),
            secrets,
            cfg,
            upstream: Upstream::new()?,
            key_cache: GatewayKeyCache::new(),
            model_endpoints: Mutex::new(endpoints),
            writer: WriteQueue::default(),
            channel_gates: Mutex::new(HashMap::new()),
            credential_gates: Mutex::new(HashMap::new()),
            logs: OnceLock::new(),
            log_file: OnceLock::new(),
            log_control: OnceLock::new(),
        });
        if tokio::runtime::Handle::try_current().is_ok() {
            crate::billing_writer::spawn_retry(Arc::downgrade(&state));
        }
        Ok(state)
    }

    /// Validate a named gateway credential. DB -> cache serialization makes
    /// revocation immediate for requests authenticating after deletion completes.
    pub fn check_gateway_key(&self, key: &str) -> bool {
        if key.trim().is_empty() { return false; }
        let conn = self.conn();
        let Some((id, hash)) = db::gateway_keys::auth_candidate(&conn,key).ok().flatten() else { return false; };
        if db::identity::context(&conn,id).ok().flatten().is_none() { return false; }
        let credential = format!("{}:{key}:{hash}",key.len());
        let ok = self.key_cache.hit(&credential,db::now_ms()) || db::gateway_keys::verify_credential(key,&hash);
        if ok { self.key_cache.remember(&credential,db::now_ms()); }
        ok
    }

    /// Credential caches never cache ownership, permissions, expiry or ancestor status.
    pub async fn authenticate_context(self: &Arc<Self>, key: String) -> Option<db::identity::AuthContext> {
        if key.trim().is_empty() { return None; }
        let (id,hash) = {
            let conn = self.conn();
            let candidate = db::gateway_keys::auth_candidate(&conn,&key).ok().flatten()?;
            let context = db::identity::context(&conn,candidate.0).ok().flatten()?;
            let credential = format!("{}:{key}:{}",key.len(),candidate.1);
            if self.key_cache.hit(&credential,db::now_ms()) { return Some(context); }
            candidate
        };
        let credential = format!("{}:{key}:{hash}",key.len());
        // Random new keys have a fast digest. Only historical Argon2 work needs
        // the bounded blocking pool; neither path holds the database guard.
        let verified = if hash.starts_with("sha256:") {
            db::gateway_keys::verify_credential(&key,&hash)
        } else {
            let slot = self.auth_slots.clone().acquire_owned().await.ok()?;
            {
                let conn = self.conn();
                let current = db::gateway_keys::auth_candidate(&conn,&key).ok().flatten()?;
                if current != (id,hash.clone()) { return None; }
                let context = db::identity::context(&conn,id).ok().flatten()?;
                if self.key_cache.hit(&credential,db::now_ms()) { return Some(context); }
            }
            let verify_key = key.clone();
            let verify_hash = hash.clone();
            tokio::task::spawn_blocking(move || {
                let _slot = slot;
                db::gateway_keys::verify_credential(&verify_key,&verify_hash)
            }).await.ok()?
        };
        if !verified { return None; }
        // Serialize the final credential and live-context recheck with all DB mutations.
        let conn = self.conn();
        let current = db::gateway_keys::auth_candidate(&conn,&key).ok().flatten()?;
        if current != (id,hash) { return None; }
        let context = db::identity::context(&conn,id).ok().flatten()?;
        self.key_cache.remember(&credential,db::now_ms());
        Some(context)
    }

    pub async fn authenticate_gateway_key(self: &Arc<Self>, key: String) -> bool {
        self.authenticate_context(key).await.is_some()
    }

    /// 换 key / 清 key 之后调它。
    pub fn forget_gateway_key(&self) {
        self.key_cache.forget();
    }

    /// 记下这个模型名平时走哪个端点（转发时顺手调，探活用），并**第一次就落库**。
    ///
    /// 落库是给重启后的探活用的（内存那份一重启就没了）；只在「还没定」时写，
    /// 人手动定过或早就学过的一律不动（`db::kinds::learn` 里那条规矩）。
    ///
    /// **两把锁不许嵌套**：先放掉内存那把，再去拿库那把 —— 反过来的顺序在别处存在，
    /// 嵌套就是经典的双锁死锁。
    pub fn note_model_endpoint(&self, model: &str, kind: EndpointKind) {
        let changed = {
            let mut m = self.model_endpoints.lock().unwrap_or_else(|e| e.into_inner());
            m.insert(model.to_string(), kind) != Some(kind)
        };
        if !changed {
            return;
        }
        let mk = kind.as_model_kind();
        let learned = {
            let conn = self.conn();
            db::kinds::learn(&conn, model, mk).unwrap_or(false)
        };
        if learned {
            tracing::info!("模型名 {model} 的用途按实测记下来了：{}", mk.label());
        }
    }

    /// 这个模型名的探活该打哪个端点。没见过流量就按对话探（对话探活对向量模型会 400，
    /// 探活那边把它当"没结论"，不会乱改路由状态）。
    pub fn model_endpoint(&self, model: &str) -> EndpointKind {
        self.model_endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(model)
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

    /// 改了路由状态就落库 —— 不然重启会丢掉"当前渠道"和"回切检查点"。
    ///
    /// **它永远不会失败请求**（2026-10-18 定）：抢不到锁就把这个模型名标成「待落账」丢给
    /// 后台落账员，自己照样返回 Ok。
    ///
    /// 以前这里用 `try_lock` 失败就报错，本意是「调用方自己握着锁 = 写错了，别死等」；
    /// 可 `try_lock` 分不清「别人正握着」和「我自己握着」—— 并发一高，前者天天发生，
    /// 于是**把一次成功的请求当成内部错误回 500**（压测实测 3%~4%）。
    /// 现在两种情形都不再报错：交给后台补写，既不会死锁也不会丢账。
    pub fn persist_state(&self, model: &str) -> db::Result<()> {
        let st = match self.routing.try_lock() {
            Ok(router) => router.state(model).cloned(),
            Err(_) => {
                self.writer.push(WriteJob::State(model.to_string()));
                return Ok(());
            }
        };
        let Some(st) = st else {
            return Ok(());
        };
        match self.db.try_lock() {
            Ok(conn) => {
                if let Err(e) = db::routing::save_state(&conn, model, &st) {
                    tracing::warn!("落路由状态失败（{model}）：{e}；交给后台补写");
                    self.writer.push(WriteJob::State(model.to_string()));
                }
                Ok(())
            }
            Err(_) => {
                self.writer.push(WriteJob::State(model.to_string()));
                Ok(())
            }
        }
    }

    /// 往待落账队列里丢一件活（请求路径抢不到库锁时用）。
    pub fn enqueue_write(&self, job: WriteJob) {
        self.writer.push(job);
    }

    /// 尚未完成的待落账活，包含后台正在处理的批次（面板/测试用）。
    pub fn pending_writes(&self) -> usize {
        self.writer.len()
    }

    /// 把待落账的活补写进库，返回写成了几件。
    ///
    /// **只 try_lock**：抢不到就留到下一轮 —— 落账员自己绝不等待，所以它既不会
    /// 卡住自己，也不会卡住请求路径。同一个模型名的多条状态只写最后一条
    /// （先写中间态没意义）。
    pub fn drain_writes(&self) -> usize {
        let Ok(_consumer) = self.writer.draining.try_lock() else {
            return 0;
        };
        let taken: Vec<WriteJob> = {
            let mut q = self.writer.jobs.lock().unwrap_or_else(|e| e.into_inner());
            let taken = std::mem::take(&mut *q);
            // Update under the same queue lock used by len(): moving a batch
            // to the consumer must never make outstanding work appear empty.
            self.writer.in_flight.store(taken.len(), std::sync::atomic::Ordering::Relaxed);
            taken
        };
        if taken.is_empty() {
            return 0;
        }
        #[cfg(test)]
        {
            let hook = self.writer.after_take.lock().unwrap().clone();
            if let Some(hook) = hook { hook(); }
        }
        // 同一个模型名的状态倒着看，只留最新那条，再翻回原顺序。
        let mut keep: Vec<WriteJob> = Vec::with_capacity(taken.len());
        let mut seen: HashSet<String> = HashSet::new();
        for job in taken.into_iter().rev() {
            if let WriteJob::State(g) = &job {
                if !seen.insert(g.clone()) {
                    continue;
                }
            }
            keep.push(job);
        }
        keep.reverse();

        let mut done = 0usize;
        let mut retry: Vec<WriteJob> = Vec::new();
        for job in keep {
            // Preserve progress order even if another request currently holds the DB.
            if !retry.is_empty() { retry.push(job); continue; }
            let Ok(conn) = self.db.try_lock() else {
                retry.push(job);
                continue;
            };
            let ok = match &job {
                WriteJob::Progress(a) => crate::db::requests::progress_args(&conn, a).is_ok(),
                WriteJob::Finish(a) => crate::db::requests::finish_args(&conn, a).is_ok(),
                WriteJob::Attempt(a) => crate::db::requests::add_attempt_args(&conn, a).is_ok(),
                WriteJob::State(g) => {
                    let st = self.routing.try_lock().ok().and_then(|r| r.state(g).cloned());
                    match st {
                        Some(st) => match db::routing::save_state(&conn, g, &st) {
                            Ok(()) => true,
                            Err(e) => {
                                tracing::warn!("落路由状态失败（{g}）：{e}");
                                false
                            }
                        },
                        // 这个名字已经不在状态机里了（名字撤了）：不算失败，直接放过
                        None => true,
                    }
                }
            };
            drop(conn);
            if ok {
                done += 1;
            } else {
                retry.push(job);
            }
        }
        {
            let mut q = self.writer.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if !retry.is_empty() {
                // 插回队头，别让新活插到它们前面（顺序对同一个请求的账有意义）
                let mut rest = std::mem::take(&mut *q);
                retry.append(&mut rest);
                *q = retry;
                self.writer.notify.notify_one();
            }
            // Complete or requeue while holding the queue lock, so len() sees
            // either queued work or owned work, never a gap between the two.
            self.writer.in_flight.store(0, std::sync::atomic::Ordering::Relaxed);
        }
        done
    }

    /// 起后台落账员：队列里一有活就补写，平时每 500ms 看一眼。
    pub fn spawn_writer(self: &Arc<Self>) {
        let st = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = st.writer.notify.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                }
                // 让出一轮，别把唤醒当忙等
                tokio::task::yield_now().await;
                st.drain_writes();
            }
        });
    }

    /// Account-wide gate shared by all deployments/models using one real credential.
    pub fn credential_gate(&self, id: i64, limit: u32) -> Option<Arc<ChannelGate>> {
        let mut gates = self
            .credential_gates
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let gate = gates
            .entry(id)
            .or_insert_with(|| {
                Arc::new(ChannelGate {
                    counts: Mutex::new((0, limit)),
                    changed: Notify::new(),
                })
            })
            .clone();
        gate.counts.lock().unwrap_or_else(|e| e.into_inner()).1 = limit;
        gate.changed.notify_waiters();
        Some(gate)
    }
    pub fn credential_load(&self, id: i64) -> usize {
        let gates = self
            .credential_gates
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        gates
            .get(&id)
            .map(|gate| gate.counts.lock().unwrap_or_else(|e| e.into_inner()).0 as usize)
            .unwrap_or(0)
    }

    /// Return the channel gate, including unlimited mode so existing in-flight calls remain tracked.
    ///
    /// 每个渠道一把闸机、上限按渠道配：上游免费额度多数是**按凭据**限并发的，
    /// 同一把 key 上同时压太多发，上游直接 429/503（现网 4 天 503「bridge busy」151 次）。
    /// 面板上改上限时：调大立刻补名额；调小不强夺已经在跑的名额（等它们自己还回来），
    /// 所以刚调小的一瞬间可能略微超出新上限，之后自然收敛。
    pub fn channel_gate(&self, id: i64, limit: u32) -> Option<Arc<ChannelGate>> {
        let mut gates = self.channel_gates.lock().unwrap_or_else(|e| e.into_inner());
        let gate = gates
            .entry(id)
            .or_insert_with(|| {
                Arc::new(ChannelGate {
                    counts: Mutex::new((0, limit)),
                    changed: Notify::new(),
                })
            })
            .clone();
        gate.counts.lock().unwrap_or_else(|e| e.into_inner()).1 = limit;
        gate.changed.notify_waiters();
        Some(gate)
    }
}

/// Keep in-flight ownership independent of capacity, so shrinking never mints permits.
pub struct ChannelGate {
    counts: Mutex<(u32, u32)>,
    changed: Notify,
}
pub struct ChannelPermit(Arc<ChannelGate>);
impl Drop for ChannelPermit {
    fn drop(&mut self) {
        self.0.counts.lock().unwrap_or_else(|e| e.into_inner()).0 -= 1;
        self.0.changed.notify_waiters();
    }
}
impl ChannelGate {
    pub fn try_acquire_owned(self: Arc<Self>) -> Result<ChannelPermit, ()> {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if counts.1 != 0 && counts.0 >= counts.1 { return Err(()); }
        counts.0 += 1;
        drop(counts);
        Ok(ChannelPermit(self))
    }
    pub async fn acquire_owned(self: Arc<Self>) -> Result<ChannelPermit, ()> {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Ok(permit) = self.clone().try_acquire_owned() { return Ok(permit); }
            notified.await;
        }
    }
    #[cfg(test)]
    fn available_permits(&self) -> usize {
        let counts = self.counts.lock().unwrap();
        counts.1.saturating_sub(counts.0) as usize
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
           log_dir: None,
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
                unwrap_envelope: false,
                max_concurrent: 0,
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

    /// 反证（真踩过，表现为并发下 3%~4% 的请求拿到 500）：**库被别人占着时调
    /// `persist_state` 绝不能失败** —— 它要把活丢给后台落账员，然后照样返回 Ok。
    ///
    /// 旧行为：这里返回 `Err(重入调用)`，请求路径上的 `?` 把它变成 HTTP 500。
    #[test]
    fn a_busy_db_never_fails_persist_state_it_queues_instead() {
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

        // 别人（另一个任务）正握着数据库锁，这时候来落状态
        let held = st.conn();
        let r = st.persist_state("auto");
        assert!(r.is_ok(), "库被别人占着也不许失败：{r:?}");
        assert_eq!(st.pending_writes(), 1, "这一组要进待落账队列");
        assert!(
            db::routing::load_state(&held, "auto").unwrap().is_none(),
            "这会儿还没落库（库被别人占着），库里不该有这一组"
        );
        drop(held);

        // 后台落账员跑一轮，账就补上了
        assert!(st.drain_writes() >= 1);
        assert_eq!(st.pending_writes(), 0, "补写完队列要空");
        assert_eq!(
            db::routing::load_state(&st.conn(), "auto").unwrap().unwrap().active,
            Some(channels[1]),
            "补写后库里必须是新的当前渠道"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 反证：握着路由锁时同样不许失败、不许死等 —— 排队，回头补。
    #[test]
    fn persist_state_queues_when_the_routing_lock_is_held() {
        std::env::remove_var(crate::crypto::MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::init(test_cfg(&dir)).unwrap();

        let held = st.routing();
        let r = st.persist_state("auto");
        assert!(r.is_ok(), "握着路由锁也不许失败，更不许死等");
        assert_eq!(st.pending_writes(), 1);
        drop(held);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 同一组的多条待落状态只写最后一条（先写中间态没意义），且顺序不乱。
    #[test]
    fn only_the_newest_state_per_model_is_written_back() {
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
        // 连丢三条，只有最后一条（也就是当前状态）需要落
        st.enqueue_write(WriteJob::State("auto".into()));
        st.enqueue_write(WriteJob::State("auto".into()));
        st.enqueue_write(WriteJob::State("auto".into()));
        assert_eq!(st.pending_writes(), 3);
        assert_eq!(st.drain_writes(), 1, "三次只该写一次");
        assert_eq!(st.pending_writes(), 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 闸机：上限 0 = 不拦；配了上限就最多放那么多个名额过去，还回来才放新的。
    #[test]
    fn the_channel_gate_limits_how_many_requests_run_at_once() {
        std::env::remove_var(crate::crypto::MASTER_KEY_ENV);
        let dir = std::env::temp_dir().join(format!("tokengo-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = AppState::init(test_cfg(&dir)).unwrap();

        assert!(
            st.channel_gate(1, 0).is_some(),
            "unlimited requests still own tracked slots"
        );

        let gate = st.channel_gate(2, 2).unwrap();
        let a = gate.clone().try_acquire_owned().unwrap();
        let b = gate.clone().try_acquire_owned().unwrap();
        assert!(
            gate.clone().try_acquire_owned().is_err(),
            "上限 2，第三个必须拿不到名额"
        );
        drop(a);
        assert!(
            gate.clone().try_acquire_owned().is_ok(),
            "还回来一个就该放进一个"
        );
        drop(b);

        // 同一个渠道再问一次：还是那把闸机，不会翻倍放行
        let same = st.channel_gate(2, 2).unwrap();
        assert_eq!(same.available_permits(), 2);

        // 上限调大 → 立刻补名额
        let bigger = st.channel_gate(2, 5).unwrap();
        assert_eq!(bigger.available_permits(), 5);

        let smaller = st.channel_gate(2, 1).unwrap();
        assert_eq!(smaller.available_permits(), 1);
        let held = smaller.clone().try_acquire_owned().unwrap();
        st.channel_gate(2, 3);
        st.channel_gate(2, 1);
        assert_eq!(smaller.available_permits(), 0);
        assert!(smaller.clone().try_acquire_owned().is_err());
        drop(held);
        assert_eq!(smaller.available_permits(), 1);
        for limit in [10, 2, 3, 1] {
            st.channel_gate(2, limit);
        }
        assert_eq!(smaller.available_permits(), 1);

        let unlimited = st.channel_gate(2, 0);
        assert!(unlimited.is_some());
        let held = smaller.clone().try_acquire_owned().unwrap();
        st.channel_gate(2, 1);
        assert!(smaller.clone().try_acquire_owned().is_err());
        drop(held);
        assert_eq!(smaller.available_permits(), 1);

        let held: Vec<_> = (0..5)
            .map(|_| st.channel_gate(2, 5).unwrap().try_acquire_owned().unwrap())
            .collect();
        st.channel_gate(2, 2);
        assert!(smaller.clone().try_acquire_owned().is_err());
        drop(held);
        assert_eq!(smaller.available_permits(), 2);
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
        st.persist_state("no-such-model").unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod model_load_tests {
    use super::*;
    #[test]
    fn idle_selection_reserves_atomically_and_releases_on_drop() {
        let l = Arc::new(ModelLoads::default());
        let mut names = vec!["a".into(), "b".into()];
        let a = l.select(&mut names, false).unwrap();
        assert_eq!(names[0], "a");
        let mut names = vec!["a".into(), "b".into()];
        let b = l.select(&mut names, false).unwrap();
        assert_eq!(names[0], "b");
        drop(a);
        drop(b);
        assert!(l.0.lock().unwrap().is_empty());
    }
    #[test]
    fn direct_traffic_counts_and_exploration_stays_first() {
        let l = Arc::new(ModelLoads::default());
        let _t = l.acquire("a");
        let mut names = vec!["a".into(), "b".into()];
        l.preview(&mut names);
        assert_eq!(names[0], "b");
        let mut names = vec!["a".into(), "b".into()];
        let _e = l.select(&mut names, true);
        assert_eq!(names[0], "a");
    }
}

#[cfg(test)]
mod writer_concurrency_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Barrier,
    };
    fn fixture() -> (SharedState, std::path::PathBuf, i64) {
        let dir = std::env::temp_dir().join(format!("writer-concurrency-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = AppState::init(Config {
            db_path: dir.join("test.db"),
            master_key_path: dir.join("master.key").to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();
        let id =
            crate::db::requests::start(&state.conn(), "/v1/chat/completions", "test", None, false)
                .unwrap();
        (state, dir, id)
    }
    fn progress(id: i64, channel_id: i64) -> WriteJob {
        WriteJob::Progress(crate::db::requests::ProgressArgs {
            request_id: id,
            channel_id,
            served_model: format!("source-{channel_id}"),
            streamed: false,
        })
    }
    fn pause_first_batch(state: &SharedState) -> (Arc<Barrier>, Arc<Barrier>) {
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let first = AtomicBool::new(true);
        let a = entered.clone();
        let b = release.clone();
        *state.writer.after_take.lock().unwrap() = Some(Arc::new(move || {
            if first.swap(false, Ordering::SeqCst) {
                a.wait();
                b.wait();
            }
        }));
        (entered, release)
    }
    #[test]
    fn a_busy_database_requeues_without_losing_outstanding_count_or_order() {
        let (state, dir, id) = fixture();
        state.enqueue_write(progress(id, 1));
        let held = state.conn();
        assert_eq!(state.drain_writes(), 0);
        assert_eq!(state.pending_writes(), 1);
        state.enqueue_write(progress(id, 2));
        drop(held);
        assert_eq!(state.drain_writes(), 2);
        assert_eq!(state.pending_writes(), 0);
        let latest: i64 = state
            .conn()
            .query_row("SELECT channel_id FROM requests WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(latest, 2);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn pending_writes_counts_the_batch_currently_being_delivered() {
        let (state, dir, id) = fixture();
        let (entered, release) = pause_first_batch(&state);
        state.enqueue_write(progress(id, 1));
        let worker = std::thread::spawn({
            let state = state.clone();
            move || state.drain_writes()
        });
        entered.wait();
        let pending = state.pending_writes();
        release.wait();
        worker.join().unwrap();
        assert_eq!(pending, 1, "an in-flight batch is not a completed queue");
        assert_eq!(state.pending_writes(), 0);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn concurrent_drainers_cannot_deliver_newer_progress_before_an_older_batch() {
        let (state, dir, id) = fixture();
        let (entered, release) = pause_first_batch(&state);
        state.enqueue_write(progress(id, 1));
        let worker = std::thread::spawn({
            let state = state.clone();
            move || state.drain_writes()
        });
        entered.wait();
        state.enqueue_write(progress(id, 2));
        let second = state.drain_writes();
        release.wait();
        worker.join().unwrap();
        state.drain_writes();
        let latest: i64 = state
            .conn()
            .query_row("SELECT channel_id FROM requests WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            (second, latest),
            (0, 2),
            "one ordered consumer must own both delivery and requeue"
        );
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
