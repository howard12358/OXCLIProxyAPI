use anyhow::Result;
use cliproxy_runtime_config_client::RuntimeConfigClient;
use cliproxy_upstream_runtime::UpstreamRuntime;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder as HyperBuilder,
    service::TowerToHyperService,
};
use tokio::{net::TcpListener, sync::watch};
use tracing::{error, info};

use crate::auth_state::AuthStateOverlay;
use crate::config::Config;
use crate::http;
use crate::redis_protocol;
use crate::runtime::RuntimeStateHandle;
use crate::usage_queue::UsageQueue;

/// Rust 数据平面进程的主启动流程。
///
/// 这里负责把进程级配置串成完整运行时：snapshot client、upstream runtime、
/// runtime state、后台刷新任务，以及最终的 HTTP 服务监听。
pub async fn run(config: Config) -> Result<()> {
    let runtime_state = RuntimeStateHandle::new(&config);
    let snapshot_client = RuntimeConfigClient::new(config.snapshot_client_config()?);
    let upstream_config = config.upstream_runtime_config();
    info!(
        upstream_proxy = upstream_config.upstream_proxy.as_deref().unwrap_or(""),
        upstream_http_proxy = upstream_config.http_proxy.as_deref().unwrap_or(""),
        upstream_https_proxy = upstream_config.https_proxy.as_deref().unwrap_or(""),
        "upstream proxy config loaded"
    );
    let upstream_runtime = UpstreamRuntime::new(upstream_config);
    if let Err(err) = runtime_state.initial_load(&snapshot_client).await {
        runtime_state.mark_failed(err.to_string());
        return Err(err);
    }
    runtime_state.spawn_refresh_loop(snapshot_client.clone());

    let listener = TcpListener::bind(config.bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let usage_queue = UsageQueue::new();
    if let Some(snapshot) = runtime_state.current_snapshot() {
        usage_queue.set_external_config(snapshot.usage_queue.external.clone());
    }
    let auth_state = AuthStateOverlay::new();
    let redis_auth_password = config.snapshot_bearer_token.clone();
    let app = http::router_with_snapshot_client_and_usage_queue(
        runtime_state,
        upstream_runtime,
        Some(snapshot_client),
        usage_queue.clone(),
        auth_state,
    );

    info!(address = %local_addr, "data plane listening");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        let signal = shutdown_signal().await;
        info!(signal, "shutdown signal received");
        let _ = shutdown_tx.send(true);
    });

    serve_listener(listener, app, usage_queue, redis_auth_password, shutdown_rx).await?;
    info!("data plane stopped");
    Ok(())
}

async fn serve_listener(
    listener: TcpListener,
    app: axum::Router,
    usage_queue: UsageQueue,
    redis_auth_password: Option<String>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        let (stream, peer_addr) = tokio::select! {
            accept_result = listener.accept() => accept_result?,
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return Ok(());
                }
                continue;
            }
        };
        let app = app.clone();
        let usage_queue = usage_queue.clone();
        let redis_auth_password = redis_auth_password.clone();
        tokio::spawn(async move {
            let mut prefix = [0; 1];
            match stream.peek(&mut prefix).await {
                Ok(0) => {}
                Ok(_) if redis_protocol::is_resp_prefix(prefix[0]) => {
                    if let Err(err) =
                        redis_protocol::handle_connection(stream, usage_queue, redis_auth_password)
                            .await
                    {
                        error!(%peer_addr, error = %err, "redis usage protocol connection failed");
                    }
                }
                Ok(_) => {
                    let io = TokioIo::new(stream);
                    let service = TowerToHyperService::new(app);
                    if let Err(err) = HyperBuilder::new(TokioExecutor::new())
                        .serve_connection(io, service)
                        .await
                    {
                        error!(%peer_addr, error = %err, "http connection failed");
                    }
                }
                Err(err) => {
                    error!(%peer_addr, error = %err, "connection protocol sniff failed");
                }
            }
        });
    }
}

async fn shutdown_signal() -> &'static str {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
        "ctrl_c"
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let terminate = async {
            match signal(SignalKind::terminate()) {
                Ok(mut stream) => {
                    let _ = stream.recv().await;
                    "sigterm"
                }
                Err(_) => "sigterm_unavailable",
            }
        };

        tokio::select! {
            signal = ctrl_c => signal,
            signal = terminate => signal,
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::watch;

    #[tokio::test]
    async fn serve_listener_returns_when_shutdown_is_requested() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let (tx, rx) = watch::channel(false);
        tx.send(true).expect("send shutdown");

        let result =
            serve_listener(listener, axum::Router::new(), UsageQueue::new(), None, rx).await;

        assert!(result.is_ok());
    }
}
