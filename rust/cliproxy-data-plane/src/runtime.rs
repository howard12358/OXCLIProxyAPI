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

/// 对外暴露的运行时状态快照，用于健康检查和调试接口返回。
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

/// 进程内部维护的运行时元数据。
///
/// 真正的大对象 snapshot 单独放在 `ArcSwapOption` 里，这里只保留状态面信息，
/// 便于低成本读写。
#[derive(Debug)]
struct RuntimeMetadata {
    state: ServiceState,
    snapshot_version: Option<String>,
    last_refresh_at: Option<String>,
    last_error: Option<String>,
}

/// runtime 状态访问句柄。
///
/// HTTP 层和刷新任务都通过它访问当前已生效的 snapshot 与进程状态。
#[derive(Clone)]
pub struct RuntimeStateHandle {
    inner: Arc<RuntimeState>,
}

/// Rust 数据平面的内存态运行时。
///
/// `snapshot` 走原子替换，保证新旧快照切换时，在途请求仍可安全持有旧快照。
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

    /// 首次加载前或无法继续服务时，把 runtime 标记为 failed。
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

    /// 原子应用一份新的 runtime snapshot，并把服务状态切到 ready。
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

    /// 在已有可用 snapshot 的前提下，刷新失败时降级为 degraded。
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

    /// 上游返回“配置未变化”时，也刷新一次活跃时间戳。
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

    /// 进程启动后的首次 snapshot 拉取。
    ///
    /// 这里失败会向上传递，由启动流程决定是 fail closed 还是退出。
    pub async fn initial_load(&self, client: &RuntimeConfigClient) -> Result<()> {
        let update = client.fetch_update(None).await?;
        self.apply_update(update);
        Ok(())
    }

    /// 执行一次主动刷新。
    ///
    /// 如果已经有历史成功快照，失败时进入 degraded；不会直接清空现有 snapshot。
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

    /// 启动后台轮询刷新任务。
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

    /// 把 snapshot client 返回的更新结果折叠成运行时状态变更。
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
