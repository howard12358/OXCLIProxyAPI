use anyhow::Result;
use clap::Parser;
use cliproxy_data_plane::{app, config::Config};
use time::{UtcOffset, format_description::well_known::Rfc3339};
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::parse();
    init_tracing(&config.log_level);
    app::run(config).await
}

fn init_tracing(default_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(default_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let timer = OffsetTime::new(
        UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC),
        Rfc3339,
    );

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_timer(timer))
        .init();
}
