use std::{net::SocketAddr, path::PathBuf};

use anyhow::bail;
use clap::Parser;
use cliproxy_runtime_config_client::{RuntimeConfigClientConfig, SnapshotSource};
use cliproxy_upstream_runtime::{CodexConfig, OpenAiConfig, UpstreamRuntimeConfig};

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

    #[arg(
        long,
        env = "CLIPROXY_OPENAI_BASE_URL",
        default_value = "https://api.openai.com/v1"
    )]
    pub openai_base_url: String,

    #[arg(long, env = "CLIPROXY_OPENAI_API_KEY")]
    pub openai_api_key: Option<String>,

    #[arg(
        long,
        env = "CLIPROXY_CODEX_BASE_URL",
        default_value = "https://chatgpt.com/backend-api/codex"
    )]
    pub codex_base_url: String,

    #[arg(long, env = "CLIPROXY_CODEX_TOKEN")]
    pub codex_token: Option<String>,

    #[arg(
        long,
        env = "CLIPROXY_CODEX_USER_AGENT",
        default_value = "cliproxy-data-plane/0.1.0"
    )]
    pub codex_user_agent: String,

    #[arg(long, env = "CLIPROXY_CODEX_OPENAI_BETA")]
    pub codex_openai_beta: Option<String>,
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

    pub fn upstream_runtime_config(&self) -> UpstreamRuntimeConfig {
        UpstreamRuntimeConfig {
            openai: self
                .openai_api_key
                .as_ref()
                .filter(|value| !value.trim().is_empty())
                .map(|api_key| OpenAiConfig {
                    base_url: self.openai_base_url.trim().to_string(),
                    api_key: api_key.trim().to_string(),
                }),
            codex: self
                .codex_token
                .as_ref()
                .filter(|value| !value.trim().is_empty())
                .map(|token| CodexConfig {
                    base_url: self.codex_base_url.trim().to_string(),
                    token: token.trim().to_string(),
                    user_agent: self.codex_user_agent.trim().to_string(),
                    openai_beta: self
                        .codex_openai_beta
                        .as_ref()
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty()),
                }),
        }
    }
}
