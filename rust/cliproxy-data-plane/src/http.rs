use axum::{
    Json, Router,
    extract::{Query, State, rejection::JsonRejection},
    http::HeaderMap,
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

use crate::auth_state::AuthStateOverlay;
use crate::responses::{ResponsesRequest, handle_responses};
use crate::runtime::{RuntimeInfo, RuntimeStateHandle};
use crate::telemetry::AppTelemetry;
use crate::usage_queue::UsageQueue;

/// Axum 路由共享状态。
///
/// 这里集中放置 HTTP 层需要访问的长生命周期组件，避免每个 handler
/// 自己重新拼装 runtime、router-core 或 telemetry。
#[derive(Clone)]
struct AppState {
    runtime: RuntimeStateHandle,
    snapshot_client: Option<RuntimeConfigClient>,
    router_core: RouterCore,
    upstream: UpstreamRuntime,
    telemetry: AppTelemetry,
    usage_queue: UsageQueue,
    auth_state: AuthStateOverlay,
}

/// `/healthz` 的轻量健康响应，只反映进程当前服务状态。
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: ServiceState,
    service: &'static str,
    version: &'static str,
}

/// `/readyz` 的就绪响应，会额外带上 runtime 元信息，方便排查 snapshot 状态。
#[derive(Debug, Serialize)]
struct ReadyResponse {
    ready: bool,
    status: ServiceState,
    runtime: RuntimeInfo,
}

pub fn router(runtime: RuntimeStateHandle, upstream: UpstreamRuntime) -> Router {
    router_with_snapshot_client(runtime, upstream, None)
}

/// Go 管理面推送 snapshot 变更通知时使用的最小请求体。
#[derive(Debug, Deserialize)]
struct SnapshotNotifyRequest {
    version: String,
    generated_at: String,
    reason: String,
}

/// snapshot notify 被接受后的确认响应。
#[derive(Debug, Serialize)]
struct SnapshotNotifyResponse {
    accepted: bool,
}

pub fn router_with_snapshot_client(
    runtime: RuntimeStateHandle,
    upstream: UpstreamRuntime,
    snapshot_client: Option<RuntimeConfigClient>,
) -> Router {
    router_with_snapshot_client_and_usage_queue(
        runtime,
        upstream,
        snapshot_client,
        UsageQueue::new(),
        AuthStateOverlay::new(),
    )
}

pub fn router_with_snapshot_client_and_usage_queue(
    runtime: RuntimeStateHandle,
    upstream: UpstreamRuntime,
    snapshot_client: Option<RuntimeConfigClient>,
    usage_queue: UsageQueue,
    auth_state: AuthStateOverlay,
) -> Router {
    // HTTP 层只负责暴露固定管理/数据面入口，真正的路由选择在
    // `/v1/responses` 内部再交给 router-core。
    let state = AppState {
        runtime,
        snapshot_client,
        router_core: RouterCore::new(),
        upstream,
        telemetry: AppTelemetry::new_with_usage_queue(usage_queue.clone()),
        usage_queue,
        auth_state,
    };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v0/runtime/snapshot", get(get_runtime_snapshot))
        .route("/v0/runtime/snapshot-notify", post(post_snapshot_notify))
        .route("/v0/management/usage-queue", get(get_usage_queue))
        .route("/v1/responses", post(post_responses))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

#[derive(Debug, Deserialize)]
struct UsageQueueQuery {
    count: Option<usize>,
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

/// CPA 兼容的 usage queue HTTP pop 入口。
///
/// 语义与 Go 管理面保持一致：`count` 省略时弹出 1 条，弹出后即消费。
async fn get_usage_queue(
    State(state): State<AppState>,
    Query(query): Query<UsageQueueQuery>,
) -> impl IntoResponse {
    let count = query.count.unwrap_or(1);
    if count == 0 {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"count must be a positive integer"})),
        )
            .into_response();
    }

    Json(state.usage_queue.pop_oldest_json(count)).into_response()
}

/// 返回当前已经应用到 Rust 进程内存中的 runtime snapshot。
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

/// `/v1/responses` HTTP 入口。
///
/// 这里只做 JSON 反序列化和请求级 header 提取，主链路编排交给
/// `handle_responses()`。
async fn post_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<ResponsesRequest>, JsonRejection>,
) -> impl IntoResponse {
    match payload {
        Ok(Json(request)) => {
            let request_id = headers
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let api_key = downstream_api_key(&headers);
            handle_responses(
                state.runtime,
                state.router_core,
                state.upstream,
                state.telemetry,
                state.usage_queue,
                state.auth_state,
                request,
                request_id,
                api_key,
            )
            .await
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

fn downstream_api_key(headers: &HeaderMap) -> Option<String> {
    // CPA 原生 usage 记录使用认证中间件写入的 userApiKey。
    // Rust 数据面不经过 Go 的 Gin context，只能在入口处从下游请求头恢复同一语义。
    for name in ["authorization", "x-api-key", "x-goog-api-key"] {
        let Some(value) = headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if name == "authorization" {
            let Some((scheme, token)) = value.split_once(' ') else {
                continue;
            };
            if scheme.eq_ignore_ascii_case("bearer") {
                let token = token.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
            continue;
        }
        return Some(value.to_string());
    }
    None
}

/// 接收 Go 管理面的 snapshot 变更通知，并异步触发一次主动刷新。
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
    // notify 只负责触发拉取，不直接携带完整 snapshot，避免配置事实源分叉。
    tokio::spawn(async move {
        runtime.refresh_once(&snapshot_client).await;
    });

    (
        axum::http::StatusCode::ACCEPTED,
        Json(SnapshotNotifyResponse { accepted: true }),
    )
        .into_response()
}
