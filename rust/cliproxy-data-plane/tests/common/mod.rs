use axum::{
    Json as AxumJson, Router as AxumRouter,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    response::IntoResponse,
    routing::post as axum_post,
};
use cliproxy_common_types::snapshot::{
    AuthExecution, AuthRecord, CodexExecution, ProviderConfig, RoutingStrategy, RuntimeSnapshot,
};
use cliproxy_data_plane::{config::Config, runtime::RuntimeStateHandle};
use cliproxy_upstream_runtime::{
    CodexConfig, OpenAiConfig, UpstreamRuntime, UpstreamRuntimeConfig,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::net::TcpListener;

pub fn test_runtime(responses_enabled: bool) -> RuntimeStateHandle {
    let config = Config {
        bind_addr: "127.0.0.1:4100".parse().expect("parse addr"),
        log_level: "info".to_string(),
        snapshot_file: None,
        snapshot_url: None,
        snapshot_bearer_token: None,
        snapshot_poll_seconds: 30,
        upstream_proxy: None,
        upstream_http_proxy: None,
        upstream_https_proxy: None,
        openai_base_url: "https://api.openai.com/v1".to_string(),
        openai_api_key: None,
        codex_base_url: "https://chatgpt.com/backend-api/codex".to_string(),
        codex_token: None,
        codex_user_agent: "cliproxy-data-plane-test".to_string(),
        codex_openai_beta: None,
    };
    let runtime = RuntimeStateHandle::new(&config);
    let mut snapshot = RuntimeSnapshot {
        version: "test-v1".to_string(),
        generated_at: "2026-06-11T00:00:00Z".to_string(),
        source_instance_id: "test".to_string(),
        ..RuntimeSnapshot::default()
    };
    snapshot.listeners.public_http = ":8317".to_string();
    snapshot.routes.responses = responses_enabled;
    snapshot
        .providers
        .insert("codex".to_string(), ProviderConfig { enabled: true });
    snapshot.model_aliases.insert(
        "codex".to_string(),
        std::collections::BTreeMap::from([("codex-latest".to_string(), "gpt-5-codex".to_string())]),
    );
    snapshot
        .models
        .insert("codex".to_string(), vec!["gpt-5-codex".to_string()]);
    snapshot.auth_pool.push(codex_oauth_auth(
        "auth-codex-1",
        100,
        "codex-access-token",
        "acct_http_test",
        None,
    ));
    runtime.apply_snapshot(snapshot);
    runtime
}

pub fn test_runtime_with_auths(
    responses_enabled: bool,
    strategy: RoutingStrategy,
    auths: Vec<AuthRecord>,
) -> RuntimeStateHandle {
    let runtime = test_runtime(responses_enabled);
    let mut snapshot = runtime
        .current_snapshot()
        .expect("snapshot")
        .as_ref()
        .clone();
    snapshot.routing.strategy = strategy;
    snapshot.auth_pool = auths;
    runtime.apply_snapshot(snapshot);
    runtime
}

pub fn codex_oauth_auth(
    id: &str,
    priority: i32,
    access_token: &str,
    account_id: &str,
    base_url: Option<&str>,
) -> AuthRecord {
    AuthRecord {
        id: id.to_string(),
        provider: "codex".to_string(),
        auth_kind: "oauth".to_string(),
        priority,
        enabled: true,
        supports_models: vec!["gpt-5-codex".to_string()],
        labels: vec!["paid".to_string()],
        execution: AuthExecution {
            codex: Some(CodexExecution {
                access_token: access_token.to_string(),
                account_id: account_id.to_string(),
                base_url: base_url.unwrap_or_default().to_string(),
                user_agent: format!("codex-tui/{id}"),
                openai_beta: "responses=v1".to_string(),
            }),
        },
        cooldown_until: None,
    }
}

pub fn test_upstream() -> UpstreamRuntime {
    UpstreamRuntime::new(UpstreamRuntimeConfig::default())
}

pub fn openai_upstream(base_url: String) -> UpstreamRuntime {
    UpstreamRuntime::new(UpstreamRuntimeConfig {
        upstream_proxy: None,
        http_proxy: None,
        https_proxy: None,
        openai: Some(OpenAiConfig {
            base_url,
            api_key: "openai-key".to_string(),
        }),
        codex: None,
    })
}

pub fn codex_upstream(base_url: String) -> UpstreamRuntime {
    UpstreamRuntime::new(UpstreamRuntimeConfig {
        upstream_proxy: None,
        http_proxy: None,
        https_proxy: None,
        openai: None,
        codex: Some(CodexConfig {
            base_url,
            token: String::new(),
            user_agent: "cliproxy-global".to_string(),
            openai_beta: Some("responses=v1".to_string()),
        }),
    })
}

pub async fn spawn_openai_upstream() -> String {
    async fn responses(headers: HeaderMap, request: Request<Body>) -> impl IntoResponse {
        let auth = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = request
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let payload: Value = serde_json::from_slice(&body).expect("parse payload");
        let stream = payload
            .get("stream")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

        if stream {
            let account_id = headers
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            (
                StatusCode::OK,
                [("content-type", "text/event-stream; charset=utf-8")],
                Body::from(format!(
                    concat!(
                        "event: response.created\n",
                        "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp-stream-1\",\"status\":\"in_progress\"}}}}\n\n",
                        "event: response.completed\n",
                        "data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp-stream-1\",\"object\":\"response\",\"status\":\"completed\",\"provider\":\"openai\",\"auth\":\"{}\",\"account_id\":\"{}\",\"model\":{},\"received_payload\":{},\"output\":[{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"stream ok\"}}]}}]}}}}\n\n"
                    ),
                    auth,
                    account_id,
                    payload["model"],
                    payload
                )),
            )
                .into_response()
        } else {
            AxumJson(json!({
                "provider": "openai",
                "auth": auth,
                "model": payload["model"],
                "received_payload": payload,
                "account_id": headers
                    .get("chatgpt-account-id")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
                "user_agent": headers
                    .get("user-agent")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
                "openai_beta": headers
                    .get("openai-beta")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
            }))
            .into_response()
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");
    let app = AxumRouter::new().route("/responses", axum_post(responses));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });
    format!("http://{}", addr)
}

pub async fn spawn_codex_failover_upstream() -> String {
    async fn responses(headers: HeaderMap, request: Request<Body>) -> impl IntoResponse {
        let auth = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if auth == "Bearer codex-token-a" {
            return (
                StatusCode::UNAUTHORIZED,
                [("content-type", "application/json")],
                Body::from(
                    json!({
                        "error": {
                            "message": "account deactivated",
                            "type": "invalid_request_error",
                            "code": "account_deactivated"
                        },
                        "status": 401
                    })
                    .to_string(),
                ),
            )
                .into_response();
        }

        let body = request
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let payload: Value = serde_json::from_slice(&body).expect("parse payload");
        let stream = payload
            .get("stream")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if stream {
            (
                StatusCode::OK,
                [("content-type", "text/event-stream; charset=utf-8")],
                Body::from(format!(
                    "event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp-retry-1\",\"object\":\"response\",\"status\":\"completed\",\"auth\":\"{}\",\"model\":{}}}}}\n\n",
                    auth,
                    payload["model"]
                )),
            )
                .into_response()
        } else {
            AxumJson(json!({
                "object": "response",
                "status": "completed",
                "auth": auth,
                "model": payload["model"]
            }))
            .into_response()
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");
    let app = AxumRouter::new().route("/responses", axum_post(responses));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });
    format!("http://{}", addr)
}

pub async fn spawn_codex_quota_failover_upstream() -> String {
    async fn responses(headers: HeaderMap, request: Request<Body>) -> impl IntoResponse {
        let auth = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if auth == "Bearer codex-token-a" {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("content-type", "application/json")],
                Body::from(
                    json!({
                        "error": {
                            "type": "usage_limit_reached",
                            "message": "The usage limit has been reached",
                            "plan_type": "free",
                            "resets_in_seconds": 900
                        }
                    })
                    .to_string(),
                ),
            )
                .into_response();
        }

        let body = request
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let payload: Value = serde_json::from_slice(&body).expect("parse payload");
        let stream = payload
            .get("stream")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if stream {
            (
                StatusCode::OK,
                [("content-type", "text/event-stream; charset=utf-8")],
                Body::from(format!(
                    "event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp-quota-retry-1\",\"object\":\"response\",\"status\":\"completed\",\"auth\":\"{}\",\"model\":{}}}}}\n\n",
                    auth,
                    payload["model"]
                )),
            )
                .into_response()
        } else {
            AxumJson(json!({
                "object": "response",
                "status": "completed",
                "auth": auth,
                "model": payload["model"]
            }))
            .into_response()
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");
    let app = AxumRouter::new().route("/responses", axum_post(responses));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });
    format!("http://{}", addr)
}
