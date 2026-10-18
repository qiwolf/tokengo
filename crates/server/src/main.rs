//! tokengo 服务端入口。实体都在 `lib.rs`，这里只负责起监听。

use std::net::SocketAddr;

use tokengo_server::{app, AppState, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 日志：stdout（journald）+ 内存缓冲（面板能看）+ 文件（带轮转，有大小上限）。
    // 面板「日志」页读的就是内存缓冲那一份。
    let log_dir = std::env::var("TOKENGO_LOG_DIR")
        .ok()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("logs"));
    let (logs, log_control, log_file) = tokengo_server::logging::init(Some(log_dir));

    let cfg = Config::from_env()?;
    let addr: SocketAddr = cfg.listen.parse()?;

    // 开库、跑迁移、恢复路由状态、准备主密钥。任何一步失败都不要带着半残状态起服务。
    let state = AppState::init(cfg)?;
    state.attach_logs(logs, log_control.clone(), log_file);

    // 后台落账员：请求路径抢不到库锁的账（收尾/尝试/路由状态）由它补写。
    // 没有它的话，那部分账就丢了 —— 而请求本身绝不能因为记账抢不到锁而失败。
    state.spawn_writer();

    // 后台探活：无流量时也要按时回头试首选渠道，否则 10 分钟回切机制在夜里等于没有
    tokengo_server::prober::spawn(state.clone());

    // 后台清扫员：按保留天数清掉旧账（默认 30 天），并每周把腾出来的空间还给磁盘。
    // 不跑它的话，账目只进不出，一年能长 3–4 GB。
    tokengo_server::janitor::spawn(state.clone());

    tracing::info!(
        "tokengo {} 监听 {addr}（日志级别 {}；日志文件见面板「日志」页）",
        tokengo_shared::VERSION,
        log_control.level()
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}
