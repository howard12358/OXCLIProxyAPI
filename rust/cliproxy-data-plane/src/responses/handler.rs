use axum::{body::Body, http::Response, response::IntoResponse};
use cliproxy_common_types::{
    routing::ExecutionPlan,
    snapshot::{AuthRecord, RuntimeSnapshot},
};
use cliproxy_router_core::{
    PlanRequest, RouterCore, extract_codex_session_id, extract_pinned_auth_id,
};
use cliproxy_upstream_runtime::UpstreamRuntime;

use crate::runtime::RuntimeStateHandle;

use super::mock::{non_streaming_response, streaming_response};
use super::upstream::{execute_real_upstream, log_upstream_failure};
use super::{ResponsesRequest, error_response, extract_metadata};

pub async fn handle_responses(
    runtime: RuntimeStateHandle,
    router_core: RouterCore,
    upstream: UpstreamRuntime,
    request: ResponsesRequest,
) -> Response<Body> {
    if !runtime.responses_route_enabled() {
        return error_response(
            axum::http::StatusCode::NOT_FOUND,
            "responses route is disabled by runtime snapshot",
            "route_disabled",
        );
    }

    let request_meta = match extract_metadata(&request) {
        Ok(meta) => meta,
        Err(err) => {
            return error_response(
                axum::http::StatusCode::BAD_REQUEST,
                &err.to_string(),
                "invalid_request",
            );
        }
    };
    let (snapshot, execution_plan) = match build_execution_plan(&runtime, &router_core, &request) {
        Ok(resolved) => resolved,
        Err(err) => {
            return error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                &err.to_string(),
                "routing_unavailable",
            );
        }
    };
    let selected_auth = auth_for_plan(snapshot.as_ref(), &execution_plan);

    if upstream_enabled_for_request(&upstream, selected_auth) {
        return match execute_real_upstream(
            upstream,
            request,
            snapshot.as_ref(),
            &execution_plan,
            selected_auth,
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                log_upstream_failure(&err, &execution_plan);
                error_response(
                    axum::http::StatusCode::BAD_GATEWAY,
                    &err.to_string(),
                    "upstream_error",
                )
            }
        };
    }

    if request.stream {
        match streaming_response(request, request_meta, &execution_plan).await {
            Ok(response) => response,
            Err(err) => error_response(
                axum::http::StatusCode::BAD_GATEWAY,
                &err.to_string(),
                "upstream_error",
            ),
        }
    } else {
        match non_streaming_response(request, request_meta, &execution_plan) {
            Ok(response) => response.into_response(),
            Err(err) => error_response(
                axum::http::StatusCode::BAD_GATEWAY,
                &err.to_string(),
                "upstream_error",
            ),
        }
    }
}

fn build_execution_plan(
    runtime: &RuntimeStateHandle,
    router_core: &RouterCore,
    request: &ResponsesRequest,
) -> anyhow::Result<(std::sync::Arc<RuntimeSnapshot>, ExecutionPlan)> {
    let snapshot = runtime
        .current_snapshot()
        .ok_or_else(|| anyhow::anyhow!("runtime snapshot is not loaded"))?;
    let plan = router_core.plan(
        snapshot.as_ref(),
        PlanRequest {
            requested_model: request.model.clone(),
            session_id: extract_codex_session_id(request.metadata.as_ref()),
            pinned_auth_id: extract_pinned_auth_id(request.metadata.as_ref()),
        },
    )?;
    Ok((snapshot, plan))
}

fn auth_for_plan<'a>(
    snapshot: &'a RuntimeSnapshot,
    plan: &ExecutionPlan,
) -> Option<&'a AuthRecord> {
    snapshot
        .auth_pool
        .iter()
        .find(|auth| auth.id == plan.auth_id)
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
