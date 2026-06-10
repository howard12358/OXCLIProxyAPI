use anyhow::Result;
use cliproxy_runtime_config_client::RuntimeConfigClient;
use tokio::net::TcpListener;
use tracing::info;

use crate::config::Config;
use crate::http;
use crate::runtime::RuntimeStateHandle;

pub async fn run(config: Config) -> Result<()> {
    let runtime_state = RuntimeStateHandle::new(&config);
    let snapshot_client = RuntimeConfigClient::new(config.snapshot_client_config()?);
    if let Err(err) = runtime_state.initial_load(&snapshot_client).await {
        runtime_state.mark_failed(err.to_string());
        return Err(err);
    }
    runtime_state.spawn_refresh_loop(snapshot_client);

    let listener = TcpListener::bind(config.bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let app = http::router(runtime_state);

    info!(address = %local_addr, "data plane listening");

    axum::serve(listener, app).await?;
    Ok(())
}
