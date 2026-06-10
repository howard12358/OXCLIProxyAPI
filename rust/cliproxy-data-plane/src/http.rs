use axum::{Json, Router, extract::State, response::IntoResponse, routing::get};
use cliproxy_common_types::health::ServiceState;
use serde::Serialize;
use tower_http::trace::TraceLayer;

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
