use std::net::SocketAddr;

use clap::Parser;
use serde::Serialize;

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
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub service: &'static str,
    pub version: &'static str,
    pub bind_addr: SocketAddr,
}

impl Config {
    pub fn runtime_info(&self) -> RuntimeInfo {
        RuntimeInfo {
            service: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
            bind_addr: self.bind_addr,
        }
    }
}
