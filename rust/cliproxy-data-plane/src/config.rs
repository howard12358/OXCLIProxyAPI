use std::{net::SocketAddr, path::PathBuf};

use anyhow::bail;
use clap::Parser;
use cliproxy_runtime_config_client::{RuntimeConfigClientConfig, SnapshotSource};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "cliproxy-data-plane",
    version,
    about = "Rust sidecar for CLIProxyAPI data-plane workloads"
)]
pub struct Config {
    #[arg(long, env = "CLIPROXY_BIND", default_value = "127.0.0.1:4100")]
    pub bind_addr: SocketAddr,

    #[arg(long, env = "CLIPROXY_LOG", default_value = "info")]
    pub log_level: String,

    #[arg(long, env = "CLIPROXY_SNAPSHOT_FILE")]
    pub snapshot_file: Option<PathBuf>,

    #[arg(long, env = "CLIPROXY_SNAPSHOT_URL")]
    pub snapshot_url: Option<String>,

    #[arg(long, env = "CLIPROXY_SNAPSHOT_POLL_SECONDS", default_value_t = 30)]
    pub snapshot_poll_seconds: u64,
}

impl Config {
    pub fn snapshot_client_config(&self) -> anyhow::Result<RuntimeConfigClientConfig> {
        let source = match (self.snapshot_file.clone(), self.snapshot_url.clone()) {
            (Some(path), None) => SnapshotSource::File { path },
            (None, Some(url)) => SnapshotSource::Http { url },
            (Some(_), Some(_)) => {
                bail!("snapshot source must use either --snapshot-file or --snapshot-url, not both")
            }
            (None, None) => {
                bail!("snapshot source is required; set --snapshot-file or --snapshot-url")
            }
        };

        Ok(RuntimeConfigClientConfig {
            source,
            poll_interval_seconds: self.snapshot_poll_seconds.max(1),
        })
    }
}
