use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    response::IntoResponse,
    routing::{get, post},
};
use cliproxy_common_types::health::ServiceState;
use cliproxy_router_core::RouterCore;
use cliproxy_runtime_config_client::RuntimeConfigClient;
use cliproxy_upstream_runtime::UpstreamRuntime;
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::responses::{ResponsesRequest, handle_responses};
use crate::runtime::{RuntimeInfo, RuntimeStateHandle};

#[derive(Clone)]
struct AppState {
    runtime: RuntimeStateHandle,
    snapshot_client: Option<RuntimeConfigClient>,
    router_core: RouterCore,
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
    router_with_snapshot_client(runtime, upstream, None)
}

#[derive(Debug, Deserialize)]
struct SnapshotNotifyRequest {
    version: String,
    generated_at: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct SnapshotNotifyResponse {
    accepted: bool,
}

pub fn router_with_snapshot_client(
    runtime: RuntimeStateHandle,
    upstream: UpstreamRuntime,
    snapshot_client: Option<RuntimeConfigClient>,
) -> Router {
    let state = AppState {
        runtime,
        snapshot_client,
        router_core: RouterCore::new(),
        upstream,
    };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v0/runtime/snapshot", get(get_runtime_snapshot))
        .route("/v1/responses", post(post_responses))
        .route("/v0/runtime/snapshot-notify", post(post_snapshot_notify))
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

async fn get_runtime_snapshot(State(state): State<AppState>) -> impl IntoResponse {
    match state.runtime.current_snapshot() {
        Some(snapshot) => {
            (axum::http::StatusCode::OK, Json(snapshot.as_ref().clone())).into_response()
        }
        None => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": {
                    "message": "runtime snapshot is not loaded",
                    "type": "server_error",
                    "code": "runtime_snapshot_unavailable"
                }
            })),
        )
            .into_response(),
    }
}

async fn post_responses(
    State(state): State<AppState>,
    payload: Result<Json<ResponsesRequest>, JsonRejection>,
) -> impl IntoResponse {
    match payload {
        Ok(Json(request)) => {
            handle_responses(state.runtime, state.router_core, state.upstream, request).await
        }
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

async fn post_snapshot_notify(
    State(state): State<AppState>,
    payload: Result<Json<SnapshotNotifyRequest>, JsonRejection>,
) -> impl IntoResponse {
    let Ok(Json(request)) = payload else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "message": "invalid snapshot notify payload",
                    "type": "invalid_request_error",
                    "code": "invalid_json"
                }
            })),
        )
            .into_response();
    };

    if request.version.trim().is_empty()
        || request.generated_at.trim().is_empty()
        || request.reason.trim().is_empty()
    {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "message": "snapshot notify payload requires version, generated_at, and reason",
                    "type": "invalid_request_error",
                    "code": "invalid_request"
                }
            })),
        )
            .into_response();
    }

    let Some(snapshot_client) = state.snapshot_client.clone() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": {
                    "message": "snapshot notify unavailable without runtime config client",
                    "type": "server_error",
                    "code": "snapshot_notify_unavailable"
                }
            })),
        )
            .into_response();
    };

    let runtime = state.runtime.clone();
    info!(
        snapshot_version = %request.version,
        generated_at = %request.generated_at,
        reason = %request.reason,
        "snapshot notify accepted"
    );
    tokio::spawn(async move {
        runtime.refresh_once(&snapshot_client).await;
    });

    (
        axum::http::StatusCode::ACCEPTED,
        Json(SnapshotNotifyResponse { accepted: true }),
    )
        .into_response()
}
