use std::{
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use arc_swap::ArcSwapOption;
use cliproxy_common_types::{health::ServiceState, snapshot::RuntimeSnapshot};
use cliproxy_runtime_config_client::{RuntimeConfigClient, SnapshotUpdate};
use serde::Serialize;
use tracing::{info, warn};

use crate::config::Config;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub service: &'static str,
    pub version: &'static str,
    pub bind_addr: SocketAddr,
    pub state: ServiceState,
    pub snapshot_version: Option<String>,
    pub last_refresh_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug)]
struct RuntimeMetadata {
    state: ServiceState,
    snapshot_version: Option<String>,
    last_refresh_at: Option<String>,
    last_error: Option<String>,
}

#[derive(Clone)]
pub struct RuntimeStateHandle {
    inner: Arc<RuntimeState>,
}

struct RuntimeState {
    service: &'static str,
    version: &'static str,
    bind_addr: SocketAddr,
    snapshot: ArcSwapOption<RuntimeSnapshot>,
    metadata: RwLock<RuntimeMetadata>,
}

impl RuntimeStateHandle {
    pub fn new(config: &Config) -> Self {
        Self {
            inner: Arc::new(RuntimeState {
                service: env!("CARGO_PKG_NAME"),
                version: env!("CARGO_PKG_VERSION"),
                bind_addr: config.bind_addr,
                snapshot: ArcSwapOption::from(None::<Arc<RuntimeSnapshot>>),
                metadata: RwLock::new(RuntimeMetadata {
                    state: ServiceState::Starting,
                    snapshot_version: None,
                    last_refresh_at: None,
                    last_error: None,
                }),
            }),
        }
    }

    pub fn runtime_info(&self) -> RuntimeInfo {
        let meta = self
            .inner
            .metadata
            .read()
            .expect("runtime metadata lock poisoned");

        RuntimeInfo {
            service: self.inner.service,
            version: self.inner.version,
            bind_addr: self.inner.bind_addr,
            state: meta.state,
            snapshot_version: meta.snapshot_version.clone(),
            last_refresh_at: meta.last_refresh_at.clone(),
            last_error: meta.last_error.clone(),
        }
    }

    pub fn current_snapshot(&self) -> Option<Arc<RuntimeSnapshot>> {
        self.inner.snapshot.load_full()
    }

    pub fn current_snapshot_version(&self) -> Option<String> {
        self.current_snapshot()
            .map(|snapshot| snapshot.version.clone())
    }

    pub fn responses_route_enabled(&self) -> bool {
        self.current_snapshot()
            .map(|snapshot| snapshot.routes.responses)
            .unwrap_or(false)
    }

    pub fn mark_failed(&self, err: impl Into<String>) {
        let message = err.into();
        let mut meta = self
            .inner
            .metadata
            .write()
            .expect("runtime metadata lock poisoned");
        meta.state = ServiceState::Failed;
        meta.last_error = Some(message);
        meta.last_refresh_at = Some(now_timestamp_marker());
    }

    pub fn apply_snapshot(&self, snapshot: RuntimeSnapshot) {
        let version = snapshot.version.clone();
        self.inner.snapshot.store(Some(Arc::new(snapshot)));

        let mut meta = self
            .inner
            .metadata
            .write()
            .expect("runtime metadata lock poisoned");
        meta.state = ServiceState::Ready;
        meta.snapshot_version = Some(version);
        meta.last_refresh_at = Some(now_timestamp_marker());
        meta.last_error = None;
    }

    pub fn mark_degraded(&self, err: impl Into<String>) {
        let message = err.into();
        let mut meta = self
            .inner
            .metadata
            .write()
            .expect("runtime metadata lock poisoned");
        meta.state = ServiceState::Degraded;
        meta.last_error = Some(message);
        meta.last_refresh_at = Some(now_timestamp_marker());
    }

    pub fn record_unchanged_refresh(&self) {
        let mut meta = self
            .inner
            .metadata
            .write()
            .expect("runtime metadata lock poisoned");
        meta.state = ServiceState::Ready;
        meta.last_refresh_at = Some(now_timestamp_marker());
        meta.last_error = None;
    }

    pub async fn initial_load(&self, client: &RuntimeConfigClient) -> Result<()> {
        let update = client.fetch_update(None).await?;
        self.apply_update(update);
        Ok(())
    }

    pub async fn refresh_once(&self, client: &RuntimeConfigClient) {
        let current_version = self.current_snapshot_version();
        match client.fetch_update(current_version.as_deref()).await {
            Ok(update) => self.apply_update(update),
            Err(err) => {
                warn!(error = %err, "snapshot refresh failed");
                self.mark_degraded(err.to_string());
            }
        }
    }

    pub fn spawn_refresh_loop(&self, client: RuntimeConfigClient) {
        let state = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(client.config().poll_interval());
            interval.tick().await;

            loop {
                interval.tick().await;
                state.refresh_once(&client).await;
            }
        });
    }

    fn apply_update(&self, update: SnapshotUpdate) {
        if update.changed {
            let version = update.snapshot.version.clone();
            self.apply_snapshot(update.snapshot);
            info!(snapshot_version = %version, "applied runtime snapshot");
        } else {
            self.record_unchanged_refresh();
            info!("runtime snapshot unchanged");
        }
    }
}

fn now_timestamp_marker() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    format!("{seconds}s_since_epoch")
}
