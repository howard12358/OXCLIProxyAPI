use axum::{Json, Router, extract::State, response::IntoResponse, routing::get};
use serde::Serialize;
use tower_http::trace::TraceLayer;

use crate::config::{Config, RuntimeInfo};

#[derive(Clone)]
struct AppState {
    runtime: RuntimeInfo,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct ReadyResponse {
    ready: bool,
    runtime: RuntimeInfo,
}

pub fn router(config: Config) -> Router {
    let state = AppState {
        runtime: config.runtime_info(),
    };

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok",
        service: state.runtime.service,
        version: state.runtime.version,
    })
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    Json(ReadyResponse {
        ready: true,
        runtime: state.runtime,
    })
}
