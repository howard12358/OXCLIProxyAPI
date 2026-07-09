use std::fs;

use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
use cliproxy_runtime_config_client::{
    RuntimeConfigClient, RuntimeConfigClientConfig, SnapshotSource,
};
use tempfile::tempdir;
use tokio::net::TcpListener;

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
          "auth_index": "auth-index-1",
          "provider": "codex",
          "auth_kind": "oauth",
          "priority": 100,
          "enabled": true,
          "supports_models": ["gpt-5-codex"],
          "labels": ["paid"],
          "execution": {
            "codex": {
              "access_token": "codex-access-token",
              "account_id": "acct_123",
              "base_url": "https://chatgpt.com/backend-api/codex",
              "user_agent": "codex-tui/0.135.0",
              "openai_beta": "responses=v1"
            }
          },
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

    let client = RuntimeConfigClient::new(RuntimeConfigClientConfig::new(SnapshotSource::File {
        path,
    }));
    let snapshot = client.fetch_snapshot().await.expect("fetch snapshot");
    assert_eq!(snapshot.version, "v1");
}

#[tokio::test]
async fn fetch_snapshot_rejects_codex_oauth_without_execution_access_token() {
    let dir = tempdir().expect("create temp dir");
    let path = dir.path().join("snapshot.json");
    let invalid = valid_snapshot_json().replace(
        "\"access_token\": \"codex-access-token\"",
        "\"access_token\": \"\"",
    );
    fs::write(&path, invalid).expect("write snapshot");

    let client = RuntimeConfigClient::new(RuntimeConfigClientConfig::new(SnapshotSource::File {
        path,
    }));
    let err = client
        .fetch_snapshot()
        .await
        .expect_err("expected invalid snapshot");
    assert!(err.to_string().contains("execution.codex.access_token"));
}

#[tokio::test]
async fn fetch_snapshot_from_http_uses_bearer_token() {
    async fn snapshot_endpoint(headers: axum::http::HeaderMap) -> impl IntoResponse {
        let auth = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if auth != "Bearer test-management-key" {
            return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
        (StatusCode::OK, valid_snapshot_json()).into_response()
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind snapshot listener");
    let addr = listener.local_addr().expect("listener addr");
    let app = Router::new().route("/snapshot", get(snapshot_endpoint));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve snapshot");
    });

    let client = RuntimeConfigClient::new(RuntimeConfigClientConfig::new(SnapshotSource::Http {
        url: format!("http://{addr}/snapshot"),
        bearer_token: Some("test-management-key".to_string()),
    }));
    let snapshot = client.fetch_snapshot().await.expect("fetch snapshot");
    assert_eq!(snapshot.version, "v1");
}
