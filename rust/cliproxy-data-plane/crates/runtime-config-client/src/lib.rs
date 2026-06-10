use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use cliproxy_common_types::snapshot::RuntimeSnapshot;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotSource {
    File { path: PathBuf },
    Http { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeConfigClientConfig {
    pub source: SnapshotSource,
    pub poll_interval_seconds: u64,
}

impl RuntimeConfigClientConfig {
    pub fn new(source: SnapshotSource) -> Self {
        Self {
            source,
            poll_interval_seconds: 30,
        }
    }

    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_seconds.max(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotUpdate {
    pub snapshot: RuntimeSnapshot,
    pub changed: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfigClient {
    config: RuntimeConfigClientConfig,
    http_client: Client,
}

impl RuntimeConfigClient {
    pub fn new(config: RuntimeConfigClientConfig) -> Self {
        Self {
            config,
            http_client: Client::new(),
        }
    }

    pub fn config(&self) -> &RuntimeConfigClientConfig {
        &self.config
    }

    pub async fn fetch_snapshot(&self) -> Result<RuntimeSnapshot> {
        let raw = match &self.config.source {
            SnapshotSource::File { path } => tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("failed to read snapshot file {}", path.display()))?,
            SnapshotSource::Http { url } => self
                .http_client
                .get(url)
                .send()
                .await
                .with_context(|| format!("failed to request snapshot from {url}"))?
                .error_for_status()
                .with_context(|| format!("snapshot endpoint returned non-success for {url}"))?
                .text()
                .await
                .with_context(|| format!("failed to read snapshot body from {url}"))?,
        };

        let snapshot: RuntimeSnapshot =
            serde_json::from_str(&raw).context("failed to parse snapshot json")?;
        validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub async fn fetch_update(&self, current_version: Option<&str>) -> Result<SnapshotUpdate> {
        let snapshot = self.fetch_snapshot().await?;
        let changed = snapshot_changed(current_version, &snapshot.version);
        Ok(SnapshotUpdate { snapshot, changed })
    }
}

pub fn snapshot_changed(current_version: Option<&str>, next_version: &str) -> bool {
    match current_version {
        Some(current) => current.trim() != next_version.trim(),
        None => true,
    }
}

pub fn validate_snapshot(snapshot: &RuntimeSnapshot) -> Result<()> {
    if snapshot.version.trim().is_empty() {
        bail!("snapshot.version must not be empty");
    }
    if snapshot.generated_at.trim().is_empty() {
        bail!("snapshot.generated_at must not be empty");
    }
    if snapshot.source_instance_id.trim().is_empty() {
        bail!("snapshot.source_instance_id must not be empty");
    }
    if snapshot.listeners.public_http.trim().is_empty() {
        bail!("snapshot.listeners.public_http must not be empty");
    }

    for (provider, config) in &snapshot.providers {
        if provider.trim().is_empty() {
            bail!("snapshot.providers contains an empty provider key");
        }
        if config.enabled && !snapshot.models.contains_key(provider) {
            bail!("enabled provider {provider} is missing snapshot.models entry");
        }
    }

    for (provider, aliases) in &snapshot.model_aliases {
        if provider.trim().is_empty() {
            bail!("snapshot.model_aliases contains an empty provider key");
        }
        for (alias, upstream) in aliases {
            if alias.trim().is_empty() {
                bail!("snapshot.model_aliases contains an empty alias for provider {provider}");
            }
            if upstream.trim().is_empty() {
                bail!(
                    "snapshot.model_aliases contains an empty upstream model for provider {provider}"
                );
            }
        }
    }

    for auth in &snapshot.auth_pool {
        if auth.id.trim().is_empty() {
            bail!("snapshot.auth_pool contains auth with empty id");
        }
        if auth.provider.trim().is_empty() {
            bail!(
                "snapshot.auth_pool contains auth {} with empty provider",
                auth.id
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn valid_snapshot_json() -> &'static str {
        r#"{
          "version": "v1",
          "generated_at": "2026-06-10T00:00:00Z",
          "source_instance_id": "go-main-01",
          "listeners": {
            "public_http": ":8317"
          },
          "routes": {
            "responses": true,
            "chat_completions": false,
            "messages": false
          },
          "routing": {
            "strategy": "fill-first",
            "session_affinity": true,
            "session_ttl_seconds": 3600
          },
          "providers": {
            "codex": {
              "enabled": true
            }
          },
          "model_aliases": {
            "codex": {
              "codex-latest": "gpt-5-codex"
            }
          },
          "models": {
            "codex": ["gpt-5-codex"]
          },
          "auth_pool": [
            {
              "id": "auth-1",
              "provider": "codex",
              "priority": 100,
              "enabled": true,
              "supports_models": ["gpt-5-codex"],
              "labels": ["paid"],
              "cooldown_until": null
            }
          ],
          "usage_queue": {
            "enabled": true,
            "backend": "redis"
          },
          "feature_flags": {
            "enable_sse_repair": true
          }
        }"#
    }

    #[tokio::test]
    async fn fetch_snapshot_from_file_succeeds() {
        let dir = tempdir().expect("create temp dir");
        let path = dir.path().join("snapshot.json");
        fs::write(&path, valid_snapshot_json()).expect("write snapshot");

        let client =
            RuntimeConfigClient::new(RuntimeConfigClientConfig::new(SnapshotSource::File {
                path,
            }));
        let snapshot = client.fetch_snapshot().await.expect("fetch snapshot");
        assert_eq!(snapshot.version, "v1");
    }

    #[test]
    fn validate_snapshot_rejects_empty_version() {
        let mut snapshot: RuntimeSnapshot =
            serde_json::from_str(valid_snapshot_json()).expect("parse snapshot");
        snapshot.version.clear();
        let err = validate_snapshot(&snapshot).expect_err("validation should fail");
        assert!(err.to_string().contains("snapshot.version"));
    }

    #[test]
    fn snapshot_changed_detects_same_and_new_versions() {
        assert!(!snapshot_changed(Some("v1"), "v1"));
        assert!(snapshot_changed(Some("v1"), "v2"));
        assert!(snapshot_changed(None, "v1"));
    }
}
