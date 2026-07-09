use axum::{body::Body, http::Response};
use cliproxy_common_types::{
    routing::ExecutionPlan,
    snapshot::{AuthRecord, RuntimeSnapshot},
};
use cliproxy_router_core::{PlanRequest, RouterCore};
use cliproxy_upstream_runtime::UpstreamRuntime;

use crate::auth_state::AuthStateOverlay;
use crate::runtime::RuntimeStateHandle;
use crate::telemetry::AppTelemetry;
use crate::usage_queue::UsageQueue;

use super::routing_policy::resolve_effective_plan;
use super::upstream::{execute_real_upstream, log_upstream_failure};
use super::{ResponsesRequest, ResponsesRequestMetadata, error_response};

/// Rust 数据平面的 `/v1/responses` 主入口。
///
/// 这里负责校验路由是否可用、基于当前 runtime snapshot 构建执行计划，
/// 并在拿到可执行的真实 upstream 能力后直接进入主链路。
#[allow(clippy::too_many_arguments)]
pub async fn handle_responses(
    runtime: RuntimeStateHandle,
    router_core: RouterCore,
    upstream: UpstreamRuntime,
    telemetry: AppTelemetry,
    usage_queue: UsageQueue,
    auth_state: AuthStateOverlay,
    request: ResponsesRequest,
    request_id: Option<String>,
    api_key: Option<String>,
) -> Response<Body> {
    let request_meta = match request.metadata() {
        Ok(meta) => meta,
        Err(err) => {
            return error_response(
                axum::http::StatusCode::BAD_REQUEST,
                &err.to_string(),
                "invalid_request",
            );
        }
    };
    let request_telemetry = telemetry.new_request(
        &request_meta.model,
        request.stream,
        runtime.current_snapshot().as_deref(),
        request_id,
        api_key,
        &request_meta.reasoning_effort,
        &request_meta.service_tier,
    );

    if !runtime.responses_route_enabled() {
        request_telemetry.finish_error(404, "responses route is disabled by runtime snapshot");
        return error_response(
            axum::http::StatusCode::NOT_FOUND,
            "responses route is disabled by runtime snapshot",
            "route_disabled",
        );
    }

    let (snapshot, execution_plan) =
        match build_execution_plan(&runtime, &router_core, &request_meta) {
            Ok(resolved) => resolved,
            Err(err) => {
                request_telemetry.finish_error(503, &err.to_string());
                return error_response(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    &err.to_string(),
                    "routing_unavailable",
                );
            }
        };
    let (execution_plan, selected_auth) =
        match resolve_effective_plan(snapshot.as_ref(), &execution_plan, &auth_state) {
            Some(resolved) => resolved,
            None => {
                let message = "all auth candidates are cooling down or unavailable";
                request_telemetry.finish_error(429, message);
                return error_response(
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    message,
                    "model_cooldown",
                );
            }
        };
    request_telemetry.bind_execution_plan(&execution_plan, selected_auth);

    // Rust 数据平面不再对 `/v1/responses` 做本地 mock 兜底；
    // 只要无法构造真实上游执行能力，就直接返回错误，与 CPA 生产语义保持一致。
    if !upstream_enabled_for_request(&upstream, selected_auth) {
        let message = "responses upstream is not configured for selected auth";
        request_telemetry.finish_error(502, message);
        return error_response(
            axum::http::StatusCode::BAD_GATEWAY,
            message,
            "upstream_unavailable",
        );
    }

    match execute_real_upstream(
        upstream,
        request,
        snapshot.as_ref(),
        &execution_plan,
        selected_auth,
        request_telemetry.clone(),
        usage_queue,
        auth_state,
    )
    .await
    {
        Ok(response) => response,
        Err(err) => {
            log_upstream_failure(&err, &execution_plan);
            request_telemetry.finish_error(502, &err.to_string());
            error_response(
                axum::http::StatusCode::BAD_GATEWAY,
                &err.to_string(),
                "upstream_error",
            )
        }
    }
}

fn build_execution_plan(
    runtime: &RuntimeStateHandle,
    router_core: &RouterCore,
    request_meta: &ResponsesRequestMetadata,
) -> anyhow::Result<(std::sync::Arc<RuntimeSnapshot>, ExecutionPlan)> {
    let snapshot = runtime
        .current_snapshot()
        .ok_or_else(|| anyhow::anyhow!("runtime snapshot is not loaded"))?;
    let plan = router_core.plan(
        snapshot.as_ref(),
        PlanRequest {
            requested_model: request_meta.model.clone(),
            session_id: request_meta.session_id.clone(),
            pinned_auth_id: request_meta.pinned_auth_id.clone(),
        },
    )?;
    Ok((snapshot, plan))
}

fn upstream_enabled_for_request(
    upstream: &UpstreamRuntime,
    selected_auth: Option<&AuthRecord>,
) -> bool {
    upstream.is_enabled()
        || selected_auth
            .map(|auth| upstream.can_execute_for_auth(auth))
            .unwrap_or(false)
}
