use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    response::IntoResponse,
    routing::{get, post},
};
use cliproxy_common_types::health::ServiceState;
use cliproxy_router_core::RouterCore;
use cliproxy_upstream_runtime::UpstreamRuntime;
use serde::Serialize;
use tower_http::trace::TraceLayer;

use crate::responses::{ResponsesRequest, handle_responses};
use crate::runtime::{RuntimeInfo, RuntimeStateHandle};

#[derive(Clone)]
struct AppState {
    runtime: RuntimeStateHandle,
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
    let state = AppState {
        runtime,
        router_core: RouterCore::new(),
        upstream,
    };
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
