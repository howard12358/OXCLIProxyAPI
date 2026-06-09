use anyhow::Result;
use tokio::net::TcpListener;
use tracing::info;

use crate::config::Config;
use crate::http;

pub async fn run(config: Config) -> Result<()> {
    let listener = TcpListener::bind(config.bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let app = http::router(config);

    info!(address = %local_addr, "data plane listening");

    axum::serve(listener, app).await?;
    Ok(())
}
