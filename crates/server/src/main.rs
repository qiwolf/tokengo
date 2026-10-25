//! tokengo 服务端入口。实体都在 `lib.rs`，这里只负责起监听。

use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};

use tokengo_server::{app, AppState, Config};

fn main() -> anyhow::Result<()> {
    // A timed-out blocking outbox operation must not keep container shutdown alive forever.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(run());
    runtime.shutdown_timeout(Duration::from_secs(2));
    result
}

async fn run() -> anyhow::Result<()> {
    // 日志：stdout（journald）+ 内存缓冲（面板能看）+ 文件（带轮转，有大小上限）。
    // 面板「日志」页读的就是内存缓冲那一份。
    let log_dir = std::env::var("TOKENGO_LOG_DIR")
        .ok()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("logs"));
    let (logs, log_control, log_file) = tokengo_server::logging::init(Some(log_dir));

    let cfg = Config::from_env()?;
    let shutdown_budget = shutdown_budget()?;
    let addr: SocketAddr = cfg.listen.parse()?;

    // 开库、跑迁移、恢复路由状态、准备主密钥。任何一步失败都不要带着半残状态起服务。
    let state = AppState::init(cfg)?;
    state.attach_logs(logs, log_control.clone(), log_file);

    // 后台落账员：请求路径抢不到库锁的账（收尾/尝试/路由状态）由它补写。
    // 没有它的话，那部分账就丢了 —— 而请求本身绝不能因为记账抢不到锁而失败。
    state.spawn_writer();

    // 后台探活：无流量时也要按时回头试首选渠道，否则 10 分钟回切机制在夜里等于没有
    tokengo_server::prober::spawn(state.clone());
    tokengo_server::custom_probes::spawn(state.clone());
    tokengo_server::type_detector::spawn(state.clone());
    tokengo_server::vision_detector::spawn(state.clone());
    tokengo_server::capability_detector::spawn(state.clone());
    // Vector compatibility diagnostics run only when explicitly requested; never on a timer.

    // 后台清扫员：按保留天数清掉旧账（默认 30 天），并每周把腾出来的空间还给磁盘。
    // 不跑它的话，账目只进不出，一年能长 3–4 GB。
    tokengo_server::janitor::spawn(state.clone());

    tracing::info!(
        "tokengo {} 监听 {addr}（日志级别 {}；日志文件见面板「日志」页）",
        tokengo_shared::VERSION,
        log_control.level()
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let served = serve_until_shutdown(
        listener,
        app(state.clone()),
        shutdown_signal(),
        shutdown_budget,
    )
    .await;
    // These are HTTP and accounting bounds, not a global background-task quiescence guarantee.
    let drained = tokio::time::timeout(Duration::from_secs(10), flush_accounting(state)).await;
    served?;
    drained.map_err(|_| {
        anyhow::anyhow!("accounting shutdown timed out; deferred writes may remain")
    })??;
    tracing::info!(
        "HTTP drained and final accounting flush completed; background tasks stop with runtime"
    );
    Ok(())
}

fn shutdown_budget() -> anyhow::Result<Duration> {
    match std::env::var("TOKENGO_SHUTDOWN_TIMEOUT_SECS") {
        Ok(value) => {
            let seconds: u64 = value.parse().map_err(|_| {
                anyhow::anyhow!(
                    "TOKENGO_SHUTDOWN_TIMEOUT_SECS must be an integer between 1 and 3600"
                )
            })?;
            anyhow::ensure!(
                (1..=3600).contains(&seconds),
                "TOKENGO_SHUTDOWN_TIMEOUT_SECS must be between 1 and 3600"
            );
            Ok(Duration::from_secs(seconds))
        }
        Err(std::env::VarError::NotPresent) => Ok(Duration::from_secs(120)),
        Err(error) => Err(error.into()),
    }
}

async fn shutdown_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn serve_until_shutdown(
    listener: tokio::net::TcpListener,
    router: axum::Router,
    signal: impl Future<Output = anyhow::Result<()>>,
    budget: Duration,
) -> anyhow::Result<()> {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = stopped.await;
    });
    let server = std::future::IntoFuture::into_future(server);
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => return Ok(result?),
        result = signal => result?,
    }
    tracing::info!("shutdown requested: stop accepting connections, drain in-flight HTTP requests");
    let _ = stop.send(());
    tokio::time::timeout(budget, server)
        .await
        .map_err(|_| anyhow::anyhow!("HTTP shutdown timed out; in-flight requests may remain"))??;
    Ok(())
}

async fn flush_accounting(state: Arc<AppState>) -> anyhow::Result<()> {
    loop {
        let st = state.clone();
        let pending = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
            st.drain_writes();
            let pending = st.pending_writes();
            if pending == 0 {
                tokengo_server::billing_writer::replay(&st.conn(), &st.cfg.db_path)?;
            }
            Ok(pending)
        })
        .await??;
        if pending == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use tokio::sync::{oneshot, Notify};

    async fn held_server(
        budget: Duration,
    ) -> (
        tokio::task::JoinHandle<anyhow::Result<()>>,
        oneshot::Sender<()>,
        Arc<Notify>,
        Arc<Notify>,
        String,
    ) {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let e = entered.clone();
        let r = release.clone();
        let router = Router::new().route(
            "/",
            get(move || {
                let e = e.clone();
                let r = r.clone();
                async move {
                    e.notify_one();
                    r.notified().await;
                    "finished"
                }
            }),
        );
        let router = router.route(
            "/new",
            get(|| async { "must not be served after shutdown" }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve_until_shutdown(
            listener,
            router,
            async {
                stopped.await?;
                Ok(())
            },
            budget,
        ));
        (task, stop, entered, release, url)
    }

    #[tokio::test]
    async fn shutdown_waits_for_inflight_http_response() {
        let (task, stop, entered, release, url) = held_server(Duration::from_secs(2)).await;
        let next_url = format!("{url}new");
        let request =
            tokio::spawn(async move { reqwest::get(url).await.unwrap().text().await.unwrap() });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        stop.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!task.is_finished(), "must wait for the in-flight response");
        // On macOS a TCP handshake can still complete against the closed listener's
        // backlog, so assert the HTTP contract rather than TCP connect alone.
        assert!(
            reqwest::Client::new()
                .get(next_url)
                .timeout(Duration::from_millis(200))
                .send()
                .await
                .is_err(),
            "must not serve new HTTP requests"
        );
        release.notify_one();
        assert_eq!(request.await.unwrap(), "finished");
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn stuck_http_request_returns_shutdown_error_within_budget() {
        let (task, stop, entered, release, url) = held_server(Duration::from_millis(40)).await;
        let request = tokio::spawn(async move { reqwest::get(url).await });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        stop.send(()).unwrap();
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("HTTP shutdown timed out"));
        release.notify_one();
        let _ = request.await;
    }

    #[tokio::test]
    async fn final_flush_persists_queued_routing_state() {
        let dir = std::env::temp_dir().join(format!("tokengo-flush-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("key").to_string_lossy().into(),
            ..Config::default()
        };
        let state = AppState::init(cfg).unwrap();
        let channels = [tokengo_shared::ChannelId(1), tokengo_shared::ChannelId(2)];
        state
            .routing()
            .attempt_order("queued", &channels, 1_800_000_000_000);
        {
            let held = state.conn();
            state.persist_state("queued").unwrap();
            assert!(tokengo_server::db::routing::load_state(&held, "queued")
                .unwrap()
                .is_none());
        }
        assert_eq!(state.pending_writes(), 1);
        flush_accounting(state.clone()).await.unwrap();
        assert_eq!(state.pending_writes(), 0);
        assert!(
            tokengo_server::db::routing::load_state(&state.conn(), "queued")
                .unwrap()
                .is_some()
        );
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn corrupt_outbox_cannot_be_reported_as_successful_shutdown() {
        let dir = std::env::temp_dir().join(format!("tokengo-shutdown-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            db_path: dir.join("db"),
            master_key_path: dir.join("key").to_string_lossy().into(),
            ..Config::default()
        };
        let state = AppState::init(cfg).unwrap();
        let outbox = dir.join("db.billing-outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        std::fs::write(outbox.join("123.json"), b"invalid json").unwrap();
        let error = flush_accounting(state).await.unwrap_err();
        assert!(error.to_string().contains("reconciliation"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Runs in a subprocess so the OS signal cannot affect the test runner.
    #[cfg(unix)]
    #[test]
    #[ignore = "subprocess helper invoked by process_sigterm_drains_real_http_request"]
    fn signal_child() {
        let dir = std::path::PathBuf::from(std::env::var("TOKENGO_SHUTDOWN_TEST_DIR").unwrap());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let entered = dir.join("entered");
            let router = Router::new().route(
                "/",
                get(move || {
                    let entered = entered.clone();
                    async move {
                        std::fs::write(entered, b"yes").unwrap();
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        "finished"
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            std::fs::write(dir.join("addr"), listener.local_addr().unwrap().to_string()).unwrap();
            serve_until_shutdown(listener, router, shutdown_signal(), Duration::from_secs(2))
                .await
                .unwrap();
        });
        runtime.shutdown_timeout(Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_sigterm_drains_real_http_request() {
        let dir = std::env::temp_dir().join(format!("tokengo-signal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::signal_child", "--ignored"])
            .env("TOKENGO_SHUTDOWN_TEST_DIR", &dir)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let check = async {
            while !dir.join("addr").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let addr = std::fs::read_to_string(dir.join("addr")).unwrap();
            let request = tokio::spawn(async move {
                reqwest::get(format!("http://{addr}/"))
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap()
            });
            while !dir.join("entered").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(std::process::Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status()
                .unwrap()
                .success());
            assert_eq!(request.await.unwrap(), "finished");
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(5), check).await;
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        std::fs::remove_dir_all(dir).unwrap();
        result.unwrap();
    }
}
