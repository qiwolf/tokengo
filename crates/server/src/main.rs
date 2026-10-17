//! tokengo 服务端入口。实体都在 `lib.rs`，这里只负责起监听。

use std::net::SocketAddr;

use tokengo_server::{app, AppState, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tokengo_server=info,tower_http=warn".into()),
        )
        .init();

    let cfg = Config::from_env()?;
    let addr: SocketAddr = cfg.listen.parse()?;

    // 开库、跑迁移、恢复路由状态、准备主密钥。任何一步失败都不要带着半残状态起服务。
    let state = AppState::init(cfg)?;

    // 后台探活：无流量时也要按时回头试首选渠道，否则 10 分钟回切机制在夜里等于没有
    tokengo_server::prober::spawn(state.clone());

    // 后台清扫员：按保留天数清掉旧账（默认 30 天），并每周把腾出来的空间还给磁盘。
    // 不跑它的话，账目只进不出，一年能长 3–4 GB。
    tokengo_server::janitor::spawn(state.clone());

    tracing::info!("tokengo {} 监听 {addr}", tokengo_shared::VERSION);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}
