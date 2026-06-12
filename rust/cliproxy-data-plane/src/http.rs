use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    response::IntoResponse,
    routing::{get, post},
};
use cliproxy_common_types::health::ServiceState;
use serde::Serialize;
use tower_http::trace::TraceLayer;

use crate::responses::{ResponsesRequest, handle_responses};
use crate::runtime::{RuntimeInfo, RuntimeStateHandle};

#[derive(Clone)]
struct AppState {
    runtime: RuntimeStateHandle,
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

pub fn router(runtime: RuntimeStateHandle) -> Router {
    let state = AppState { runtime };
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
        Ok(Json(request)) => handle_responses(state.runtime, request).await,
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
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::{config::Config, runtime::RuntimeStateHandle};
    use cliproxy_common_types::snapshot::RuntimeSnapshot;
    use serde_json::json;

    fn test_runtime(responses_enabled: bool) -> RuntimeStateHandle {
        let config = Config {
            bind_addr: "127.0.0.1:4100".parse().expect("parse addr"),
            log_level: "info".to_string(),
            snapshot_file: None,
            snapshot_url: None,
            snapshot_poll_seconds: 30,
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

    #[tokio::test]
    async fn responses_route_returns_not_found_when_disabled() {
        let app = router(test_runtime(false));
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
        let app = router(test_runtime(true));
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
}
