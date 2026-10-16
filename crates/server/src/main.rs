//! tokengo 服务端入口。实体都在 `lib.rs`，这里只负责起监听。

use std::net::SocketAddr;

use tokengo_server::{app, Config};

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

    tracing::info!("tokengo {} 监听 {addr}", tokengo_shared::VERSION);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(&cfg)).await?;
    Ok(())
}
