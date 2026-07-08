use std::convert::Infallible;

use anyhow::{Context, Result};
use async_stream::stream;
use axum::body::Body;
use bytes::Bytes;
use cliproxy_common_types::{
    routing::ExecutionPlan,
    snapshot::{AuthRecord, RuntimeSnapshot},
};
use cliproxy_upstream_runtime::{UpstreamExecutionResult, UpstreamRequest, UpstreamRuntime};
use futures_util::StreamExt;
use tracing::{debug, info, warn};

use crate::{
    auth_state::AuthStateOverlay,
    telemetry::{RequestTelemetry, StreamCompletionGuard},
    usage_queue::UsageQueue,
};

use super::failure::classify_upstream_failure;
use super::health::{HealthRecorder, RecordedFailure};
use super::protocol::ResponsesRequestIr;
use super::sse::{ResponsesSseFramer, extract_completed_response_from_sse};
use super::{
    ResponsesRequest, response_from_aggregated_json_body, response_from_body, response_with_stream,
};

struct UpstreamExecutionOutcome {
    response: UpstreamExecutionResult,
    successful_auth: Option<AuthRecord>,
}

/// 在路由已经选定 model/auth 后，执行真实上游请求链路。
///
/// 对 Codex 来说，即使下游是非流式客户端，也仍然先走流式上游，
/// 这样 Rust 路径才能复用 CPA 的 SSE 修复与终态聚合逻辑。
pub(super) async fn execute_real_upstream(
    upstream: UpstreamRuntime,
    request: ResponsesRequest,
    snapshot: &RuntimeSnapshot,
    execution_plan: &ExecutionPlan,
    selected_auth: Option<&AuthRecord>,
    telemetry: RequestTelemetry,
    usage_queue: UsageQueue,
    auth_state: AuthStateOverlay,
) -> Result<axum::http::Response<Body>> {
    let health_recorder = HealthRecorder::new(auth_state.clone(), usage_queue.clone());
    let downstream_stream = request.stream;
    let aggregate_codex_stream = execution_plan.provider
        == cliproxy_common_types::upstream::ProviderKind::Codex
        && !downstream_stream;
    let mut upstream_request = normalize_upstream_request(request, execution_plan);
    upstream_request.model = execution_plan.model.clone();
    if aggregate_codex_stream {
        upstream_request.stream = true;
    }
    let request_body =
        serde_json::to_vec(&upstream_request).context("failed to serialize responses request")?;
    info!(
        provider = ?execution_plan.provider,
        auth_id = %execution_plan.auth_id,
        resolved_model = %execution_plan.model,
        stickiness_source = ?execution_plan.stickiness_source,
        "responses execution plan selected"
    );
    let upstream_request_template = UpstreamRequest {
        model: upstream_request.model.clone(),
        body: request_body,
        stream: upstream_request.stream,
    };
    let upstream_outcome = execute_upstream_with_retries(
        &upstream,
        snapshot,
        execution_plan,
        selected_auth,
        &upstream_request_template,
        snapshot.network.upstream_proxy.as_deref(),
        &telemetry,
        &usage_queue,
        &auth_state,
    )
    .await?;

    match upstream_outcome.response {
        UpstreamExecutionResult::NonStreaming(response) => {
            let _provider = response.provider;
            let _events = response.events;
            if let Some(auth) = upstream_outcome.successful_auth.as_ref() {
                health_recorder.record_success(auth, &execution_plan.model);
            }
            telemetry.observe_response_headers(&response.head.headers);
            telemetry.mark_first_byte();
            telemetry.observe_response_json_bytes(response.body.as_ref());
            telemetry.finish_success();
            Ok(response_from_body(response.head, response.body))
        }
        UpstreamExecutionResult::Streaming(response) => {
            if aggregate_codex_stream {
                let _provider = response.provider;
                let _events = response.events;
                if let Some(auth) = upstream_outcome.successful_auth.as_ref() {
                    health_recorder.record_success(auth, &execution_plan.model);
                }
                telemetry.observe_response_headers(&response.head.headers);
                // 对齐 Go usage reporter：收到上游首个 body chunk 就记首字延迟，
                // 不等待完整 SSE frame 组装完成，避免 chunk 边界把 TTFT 抬到接近总耗时。
                telemetry.mark_first_byte();
                let body = aggregate_streaming_response_body(
                    response.first_chunk,
                    response.stream,
                    telemetry.clone(),
                )
                .await?;
                telemetry.mark_first_byte();
                telemetry.observe_response_json_bytes(body.as_ref());
                telemetry.finish_success();
                return Ok(response_from_aggregated_json_body(response.head, body));
            }
            let _provider = response.provider;
            let _events = response.events;
            if let Some(auth) = upstream_outcome.successful_auth.as_ref() {
                health_recorder.record_success(auth, &execution_plan.model);
            }
            telemetry.observe_response_headers(&response.head.headers);
            let first_chunk = response.first_chunk;
            let tail = response.stream;
            let body = Body::from_stream(stream! {
                let mut completion_guard = StreamCompletionGuard::new(telemetry.clone());
                let mut framer = ResponsesSseFramer::default();
                telemetry.mark_first_byte();
                for frame in framer.push_chunk(first_chunk) {
                    telemetry.observe_sse_frame(&frame);
                    yield Ok::<Bytes, Infallible>(frame);
                }
                futures_util::pin_mut!(tail);
                while let Some(item) = tail.next().await {
                    match item {
                        Ok(bytes) => {
                            telemetry.mark_first_byte();
                            for frame in framer.push_chunk(bytes) {
                                telemetry.observe_sse_frame(&frame);
                                yield Ok(frame);
                            }
                        }
                        Err(err) => {
                            let frame = Bytes::from(format!(
                                "event: response.error\ndata: {{\"type\":\"response.error\",\"message\":{}}}\n\n",
                                serde_json::to_string(&err.to_string()).unwrap_or_else(|_| "\"upstream stream error\"".to_string())
                            ));
                            for pending in framer.finish() {
                                telemetry.observe_sse_frame(&pending);
                                yield Ok(pending);
                            }
                            telemetry.observe_sse_frame(&frame);
                            yield Ok(frame);
                            completion_guard.finish_error();
                            break;
                        }
                    }
                }
                for frame in framer.finish() {
                    telemetry.observe_sse_frame(&frame);
                    yield Ok(frame);
                }
                completion_guard.finish_success();
            });
            Ok(response_with_stream(response.head, body))
        }
    }
}

async fn execute_upstream_with_retries(
    upstream: &UpstreamRuntime,
    snapshot: &RuntimeSnapshot,
    execution_plan: &ExecutionPlan,
    selected_auth: Option<&AuthRecord>,
    request: &UpstreamRequest,
    proxy_override: Option<&str>,
    telemetry: &RequestTelemetry,
    usage_queue: &UsageQueue,
    auth_state: &AuthStateOverlay,
) -> Result<UpstreamExecutionOutcome> {
    let health_recorder = HealthRecorder::new(auth_state.clone(), usage_queue.clone());
    if let Some(auth) = selected_auth {
        if upstream.can_execute_for_auth(auth) {
            let mut last_error = None;
            // 只有在下游响应还没提交时，才允许在 auth 绑定链内继续重试。
            for candidate in auth_bound_retry_chain(snapshot, execution_plan, auth) {
                match upstream
                    .execute_responses_for_auth(&candidate, request.clone(), proxy_override)
                    .await
                {
                    Ok(response) => {
                        return Ok(UpstreamExecutionOutcome {
                            response,
                            successful_auth: Some(candidate),
                        });
                    }
                    Err(err) if should_retry_auth_bound_error(&err) => {
                        let classification = classify_upstream_failure(&err);
                        let reason = classification.kind.reason();
                        if let Some(failure) = RecordedFailure::new(
                            &candidate,
                            &execution_plan.model,
                            telemetry.error_event_request_id(),
                            classification,
                        ) {
                            health_recorder.record_failure(failure);
                        }
                        telemetry.record_auth_failure(reason);
                        telemetry.record_auth_retry(reason);
                        info!(
                            failed_auth_id = %candidate.id,
                            next_retry_candidates = ?execution_plan.retry_candidates,
                            reason = %reason,
                            "responses upstream auth failed, retrying next candidate"
                        );
                        last_error = Some(err);
                    }
                    Err(err) => {
                        let classification = classify_upstream_failure(&err);
                        if let Some(failure) = RecordedFailure::new(
                            &candidate,
                            &execution_plan.model,
                            telemetry.error_event_request_id(),
                            classification.clone(),
                        ) {
                            health_recorder.record_failure(failure);
                        }
                        telemetry.record_auth_failure(classification.kind.reason());
                        return Err(err);
                    }
                }
            }
            if let Some(err) = last_error {
                return Err(err);
            }
        } else {
            return upstream
                .execute_responses(request.clone(), proxy_override)
                .await
                .map(|response| UpstreamExecutionOutcome {
                    response,
                    successful_auth: None,
                });
        }
    }

    upstream
        .execute_responses(request.clone(), proxy_override)
        .await
        .map(|response| UpstreamExecutionOutcome {
            response,
            successful_auth: None,
        })
}

pub(super) fn auth_bound_retry_chain(
    snapshot: &RuntimeSnapshot,
    execution_plan: &ExecutionPlan,
    selected_auth: &AuthRecord,
) -> Vec<AuthRecord> {
    let mut chain = vec![selected_auth.clone()];
    for auth_id in &execution_plan.retry_candidates {
        if auth_id == &selected_auth.id {
            continue;
        }
        if let Some(auth) = snapshot.auth_pool.iter().find(|auth| auth.id == *auth_id) {
            chain.push(auth.clone());
        }
    }
    chain
}

pub(super) fn log_upstream_failure(err: &anyhow::Error, execution_plan: &ExecutionPlan) {
    let error = redact_error_chain(err);
    warn!(
        provider = ?execution_plan.provider,
        auth_id = %execution_plan.auth_id,
        resolved_model = %execution_plan.model,
        stickiness_source = ?execution_plan.stickiness_source,
        error = %error,
        "responses upstream execution failed"
    );

    let chain = err
        .chain()
        .enumerate()
        .map(|(index, cause)| format!("{index}: {}", redact_error_text(&cause.to_string())))
        .collect::<Vec<_>>()
        .join(" | ");

    debug!(
        provider = ?execution_plan.provider,
        auth_id = %execution_plan.auth_id,
        resolved_model = %execution_plan.model,
        error_chain = %chain,
        "responses upstream failure chain"
    );
}

fn redact_error_chain(err: &anyhow::Error) -> String {
    redact_error_text(&err.to_string())
}

fn redact_error_text(text: &str) -> String {
    cliproxy_upstream_runtime::redact_url(text)
        .replace("Bearer ", "Bearer <redacted>")
        .replace("bearer ", "bearer <redacted>")
}

pub(super) fn normalize_upstream_request(
    request: ResponsesRequest,
    execution_plan: &ExecutionPlan,
) -> ResponsesRequest {
    // 把 provider 定制化请求整形收口到显式 request IR 后面。
    let ir = ResponsesRequestIr::from_downstream_request(&request);
    debug_assert!(!ir.model().trim().is_empty());
    ir.emit_upstream_request(execution_plan)
}

async fn aggregate_streaming_response_body(
    first_chunk: Bytes,
    mut tail: cliproxy_upstream_runtime::ByteStream,
    telemetry: RequestTelemetry,
) -> Result<Bytes> {
    // 非流式下游客户端仍然先经过 SSE 修复路径，再折叠回
    // `response.completed.response` 里的终态 JSON。
    let mut framer = ResponsesSseFramer::default();
    let mut combined = Vec::new();
    for frame in framer.push_chunk(first_chunk) {
        telemetry.observe_sse_frame(&frame);
        combined.extend_from_slice(&frame);
    }
    while let Some(item) = tail.next().await {
        let bytes = item?;
        for frame in framer.push_chunk(bytes) {
            telemetry.observe_sse_frame(&frame);
            combined.extend_from_slice(&frame);
        }
    }
    for frame in framer.finish() {
        telemetry.observe_sse_frame(&frame);
        combined.extend_from_slice(&frame);
    }
    extract_completed_response_from_sse(&combined)
}

#[cfg(test)]
pub(super) type PrecommitRetryClassification = super::failure::PrecommitRetryClassification;

#[cfg(test)]
pub(super) fn classify_precommit_retry(err: &anyhow::Error) -> PrecommitRetryClassification {
    super::failure::classify_precommit_retry(err)
}

pub(super) fn should_retry_auth_bound_error(err: &anyhow::Error) -> bool {
    super::failure::should_retry_auth_bound_error(err)
}
