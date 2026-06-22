use anyhow::Result;
use cliproxy_runtime_config_client::RuntimeConfigClient;
use cliproxy_upstream_runtime::UpstreamRuntime;
use tokio::net::TcpListener;
use tracing::info;

use crate::config::Config;
use crate::http;
use crate::runtime::RuntimeStateHandle;

pub async fn run(config: Config) -> Result<()> {
    let runtime_state = RuntimeStateHandle::new(&config);
    let snapshot_client = RuntimeConfigClient::new(config.snapshot_client_config()?);
    let upstream_config = config.upstream_runtime_config();
    info!(
        upstream_http_proxy = upstream_config.http_proxy.as_deref().unwrap_or(""),
        upstream_https_proxy = upstream_config.https_proxy.as_deref().unwrap_or(""),
        "upstream proxy config loaded"
    );
    let upstream_runtime = UpstreamRuntime::new(upstream_config);
    if let Err(err) = runtime_state.initial_load(&snapshot_client).await {
        runtime_state.mark_failed(err.to_string());
        return Err(err);
    }
    runtime_state.spawn_refresh_loop(snapshot_client);

    let listener = TcpListener::bind(config.bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let app = http::router(runtime_state, upstream_runtime);

    info!(address = %local_addr, "data plane listening");

    axum::serve(listener, app).await?;
    Ok(())
}
