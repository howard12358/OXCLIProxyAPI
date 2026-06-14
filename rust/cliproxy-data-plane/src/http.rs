use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    response::IntoResponse,
    routing::{get, post},
};
use cliproxy_common_types::health::ServiceState;
use cliproxy_upstream_runtime::UpstreamRuntime;
use serde::Serialize;
use tower_http::trace::TraceLayer;

use crate::responses::{ResponsesRequest, handle_responses};
use crate::runtime::{RuntimeInfo, RuntimeStateHandle};

#[derive(Clone)]
struct AppState {
    runtime: RuntimeStateHandle,
    upstream: UpstreamRuntime,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: ServiceState,
    service: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct ReadyResponse {
    ready: bool,
    status: ServiceState,
    runtime: RuntimeInfo,
}

pub fn router(runtime: RuntimeStateHandle, upstream: UpstreamRuntime) -> Router {
    let state = AppState { runtime, upstream };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/responses", post(post_responses))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let runtime = state.runtime.runtime_info();
    Json(HealthResponse {
        status: runtime.state,
        service: runtime.service,
        version: runtime.version,
    })
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let runtime = state.runtime.runtime_info();
    Json(ReadyResponse {
        ready: matches!(runtime.state, ServiceState::Ready),
        status: runtime.state,
        runtime,
    })
}

async fn post_responses(
    State(state): State<AppState>,
    payload: Result<Json<ResponsesRequest>, JsonRejection>,
) -> impl IntoResponse {
    match payload {
        Ok(Json(request)) => handle_responses(state.runtime, state.upstream, request).await,
        Err(err) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "message": err.body_text(),
                    "type": "invalid_request_error",
                    "code": "invalid_json"
                }
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json as AxumJson, Router as AxumRouter,
        body::Body,
        http::HeaderMap,
        http::{Request, StatusCode},
        routing::post as axum_post,
    };
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    use crate::{config::Config, runtime::RuntimeStateHandle};
    use cliproxy_common_types::snapshot::RuntimeSnapshot;
    use cliproxy_upstream_runtime::{OpenAiConfig, UpstreamRuntime, UpstreamRuntimeConfig};
    use serde_json::json;

    fn test_runtime(responses_enabled: bool) -> RuntimeStateHandle {
        let config = Config {
            bind_addr: "127.0.0.1:4100".parse().expect("parse addr"),
            log_level: "info".to_string(),
            snapshot_file: None,
            snapshot_url: None,
            snapshot_poll_seconds: 30,
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
        runtime.apply_snapshot(snapshot);
        runtime
    }

    fn test_upstream() -> UpstreamRuntime {
        UpstreamRuntime::new(UpstreamRuntimeConfig::default())
    }

    async fn spawn_openai_upstream() -> String {
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
                (
                    StatusCode::OK,
                    [("content-type", "text/event-stream; charset=utf-8")],
                    Body::from(format!(
                        "event: response.created\ndata: {{\"provider\":\"openai\",\"auth\":\"{}\"}}\n\n",
                        auth
                    )),
                )
                    .into_response()
            } else {
                AxumJson(json!({
                    "provider": "openai",
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

    fn openai_upstream(base_url: String) -> UpstreamRuntime {
        UpstreamRuntime::new(UpstreamRuntimeConfig {
            openai: Some(OpenAiConfig {
                base_url,
                api_key: "openai-key".to_string(),
            }),
            codex: None,
        })
    }

    #[tokio::test]
    async fn responses_route_returns_not_found_when_disabled() {
        let app = router(test_runtime(false), test_upstream());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"model":"gpt-5","stream":true,"input":"hello"}).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("call app");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn responses_stream_returns_normalized_sse_frames() {
        let app = router(test_runtime(true), test_upstream());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"model":"gpt-5","stream":true,"input":"hello"}).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("call app");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream; charset=utf-8")
        );

        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let text = String::from_utf8(body.to_vec()).expect("valid utf8");
        assert!(text.contains("event: response.created"));
        assert!(text.contains("event: response.usage"));
        assert!(text.contains("event: response.completed"));
    }

    #[tokio::test]
    async fn responses_non_streaming_prefers_real_openai_upstream() {
        let upstream_url = spawn_openai_upstream().await;
        let app = router(test_runtime(true), openai_upstream(upstream_url));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"model":"gpt-5","stream":false,"input":"hello"}).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("call app");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let payload: Value = serde_json::from_slice(&body).expect("parse body");
        assert_eq!(payload["provider"], "openai");
        assert_eq!(payload["auth"], "Bearer openai-key");
    }

    #[tokio::test]
    async fn responses_streaming_prefers_real_openai_upstream() {
        let upstream_url = spawn_openai_upstream().await;
        let app = router(test_runtime(true), openai_upstream(upstream_url));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"model":"gpt-5","stream":true,"input":"hello"}).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("call app");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let text = String::from_utf8(body.to_vec()).expect("valid utf8");
        assert!(text.contains("\"provider\":\"openai\""));
        assert!(text.contains("Bearer openai-key"));
    }
}
