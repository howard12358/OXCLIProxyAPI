use axum::{
    Json, Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    response::IntoResponse,
    routing::post,
};
use cliproxy_common_types::{
    snapshot::{AuthExecution, AuthRecord, CodexExecution},
    upstream::ProviderKind,
};
use cliproxy_upstream_runtime::{
    CodexConfig, OpenAiConfig, ProxySetting, UpstreamExecutionResult, UpstreamRequest,
    UpstreamRuntime, UpstreamRuntimeConfig,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::net::TcpListener;

async fn spawn_upstream_server() -> String {
    async fn responses(headers: HeaderMap, request: Request<Body>) -> impl IntoResponse {
        let body = request
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let payload: Value = serde_json::from_slice(&body).expect("parse request");
        let stream = payload
            .get("stream")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let auth = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let provider = if auth.contains("codex-token") {
            "codex"
        } else {
            "openai"
        };

        if stream {
            let body = Body::from(format!(
                "event: response.created\ndata: {{\"provider\":\"{}\",\"account_id\":\"{}\"}}\n\n",
                provider,
                headers
                    .get("chatgpt-account-id")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
            ));
            (
                StatusCode::OK,
                [(CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                body,
            )
                .into_response()
        } else {
            Json(json!({
                "provider": provider,
                "echo_model": payload.get("model").and_then(|value| value.as_str()).unwrap_or_default(),
                "auth": auth,
                "accept": headers
                    .get("accept")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
                "connection": headers
                    .get("connection")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
                "originator": headers
                    .get("originator")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
                "session_id": headers
                    .get("session_id")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
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
        .expect("bind upstream listener");
    let addr = listener.local_addr().expect("listener addr");
    let app = Router::new().route("/responses", post(responses));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });
    format!("http://{}", addr)
}

#[tokio::test]
async fn execute_responses_calls_openai_upstream() {
    let base_url = spawn_upstream_server().await;
    let runtime = UpstreamRuntime::new(UpstreamRuntimeConfig {
        upstream_proxy: None,
        http_proxy: None,
        https_proxy: None,
        openai: Some(OpenAiConfig {
            base_url,
            api_key: "openai-key".to_string(),
        }),
        codex: None,
    });

    let result = runtime
        .execute_responses(UpstreamRequest {
            model: "gpt-5".to_string(),
            body: br#"{"model":"gpt-5","stream":false}"#.to_vec(),
            stream: false,
        }, None)
        .await
        .expect("execute upstream");

    match result {
        UpstreamExecutionResult::NonStreaming(response) => {
            assert_eq!(response.provider, ProviderKind::OpenAi);
            let payload: Value =
                serde_json::from_slice(&response.body).expect("parse response body");
            assert_eq!(payload["provider"], "openai");
        }
        UpstreamExecutionResult::Streaming(_) => panic!("expected non-streaming response"),
    }
}

#[tokio::test]
async fn execute_responses_calls_codex_upstream() {
    let base_url = spawn_upstream_server().await;
    let runtime = UpstreamRuntime::new(UpstreamRuntimeConfig {
        upstream_proxy: None,
        http_proxy: None,
        https_proxy: None,
        openai: None,
        codex: Some(CodexConfig {
            base_url,
            token: "codex-token".to_string(),
            user_agent: "cliproxy-test".to_string(),
            openai_beta: Some("responses=v1".to_string()),
        }),
    });

    let result = runtime
        .execute_responses(UpstreamRequest {
            model: "gpt-5-codex".to_string(),
            body: br#"{"model":"gpt-5-codex","stream":false}"#.to_vec(),
            stream: false,
        }, None)
        .await
        .expect("execute upstream");

    match result {
        UpstreamExecutionResult::NonStreaming(response) => {
            assert_eq!(response.provider, ProviderKind::Codex);
            let payload: Value =
                serde_json::from_slice(&response.body).expect("parse response body");
            assert_eq!(payload["provider"], "codex");
        }
        UpstreamExecutionResult::Streaming(_) => panic!("expected non-streaming response"),
    }
}

#[tokio::test]
async fn execute_responses_for_auth_uses_selected_codex_oauth_credential() {
    let base_url = spawn_upstream_server().await;
    let runtime = UpstreamRuntime::new(UpstreamRuntimeConfig {
        upstream_proxy: None,
        http_proxy: None,
        https_proxy: None,
        openai: None,
        codex: Some(CodexConfig {
            base_url: base_url.clone(),
            token: "global-codex-token".to_string(),
            user_agent: "cliproxy-global".to_string(),
            openai_beta: Some("responses=v1".to_string()),
        }),
    });
    let auth = AuthRecord {
        id: "auth-codex-oauth-1".to_string(),
        provider: "codex".to_string(),
        auth_kind: "oauth".to_string(),
        usage_source: "codex-user@example.com".to_string(),
        priority: 100,
        enabled: true,
        supports_models: vec!["gpt-5-codex".to_string()],
        labels: vec!["paid".to_string()],
        execution: AuthExecution {
            codex: Some(CodexExecution {
                access_token: "auth-specific-token".to_string(),
                account_id: "acct_42".to_string(),
                base_url,
                user_agent:
                    "codex-tui/0.135.0 (Mac OS 26.5.0; arm64) iTerm.app/3.6.10 (codex-tui; auth-42)"
                        .to_string(),
                openai_beta: "responses=auth".to_string(),
            }),
        },
        cooldown_until: None,
    };

    let result = runtime
        .execute_responses_for_auth(
            &auth,
            UpstreamRequest {
                model: "gpt-5-codex".to_string(),
                body: br#"{"model":"gpt-5-codex","stream":false}"#.to_vec(),
                stream: false,
            },
            None,
        )
        .await
        .expect("execute upstream");

    match result {
        UpstreamExecutionResult::NonStreaming(response) => {
            let payload: Value =
                serde_json::from_slice(&response.body).expect("parse response body");
            assert_eq!(payload["provider"], "openai");
            assert_eq!(payload["auth"], "Bearer auth-specific-token");
            assert_eq!(payload["accept"], "application/json");
            assert_eq!(payload["originator"], "codex-tui");
            assert!(payload["session_id"].as_str().unwrap_or_default().len() > 0);
            assert_eq!(payload["account_id"], "acct_42");
            assert_eq!(
                payload["user_agent"],
                "codex-tui/0.135.0 (Mac OS 26.5.0; arm64) iTerm.app/3.6.10 (codex-tui; auth-42)"
            );
            assert_eq!(payload["openai_beta"], "responses=auth");
        }
        UpstreamExecutionResult::Streaming(_) => panic!("expected non-streaming response"),
    }
}

#[test]
fn proxy_setting_parses_inherit_direct_and_socks5h() {
    let inherit = ProxySetting::parse("").expect("parse inherit");
    assert!(matches!(inherit, ProxySetting::Inherit));

    let direct = ProxySetting::parse("direct").expect("parse direct");
    assert!(matches!(direct, ProxySetting::Direct));

    let socks5h = ProxySetting::parse("socks5h://127.0.0.1:7897").expect("parse socks5h");
    match socks5h {
        ProxySetting::Proxy(url) => assert_eq!(url.as_str(), "socks5h://127.0.0.1:7897"),
        other => panic!("expected explicit proxy, got {other:?}"),
    }
}
