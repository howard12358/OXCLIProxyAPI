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

use crate::telemetry::{RequestTelemetry, StreamCompletionGuard};

use super::protocol::ResponsesRequestIr;
use super::sse::{ResponsesSseFramer, extract_completed_response_from_sse};
use super::{
    ResponsesRequest, response_from_aggregated_json_body, response_from_body, response_with_stream,
};

pub(super) async fn execute_real_upstream(
    upstream: UpstreamRuntime,
    request: ResponsesRequest,
    snapshot: &RuntimeSnapshot,
    execution_plan: &ExecutionPlan,
    selected_auth: Option<&AuthRecord>,
    telemetry: RequestTelemetry,
) -> Result<axum::http::Response<Body>> {
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
    let upstream_result = execute_upstream_with_retries(
        &upstream,
        snapshot,
        execution_plan,
        selected_auth,
        &upstream_request_template,
        snapshot.network.upstream_proxy.as_deref(),
        &telemetry,
    )
    .await?;

    match upstream_result {
        UpstreamExecutionResult::NonStreaming(response) => {
            let _provider = response.provider;
            let _events = response.events;
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
                telemetry.observe_response_headers(&response.head.headers);
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
            telemetry.observe_response_headers(&response.head.headers);
            let first_chunk = response.first_chunk;
            let tail = response.stream;
            let body = Body::from_stream(stream! {
                let mut completion_guard = StreamCompletionGuard::new(telemetry.clone());
                let mut framer = ResponsesSseFramer::default();
                for frame in framer.push_chunk(first_chunk) {
                    telemetry.mark_first_byte();
                    telemetry.observe_sse_frame(&frame);
                    yield Ok::<Bytes, Infallible>(frame);
                }
                futures_util::pin_mut!(tail);
                while let Some(item) = tail.next().await {
                    match item {
                        Ok(bytes) => {
                            for frame in framer.push_chunk(bytes) {
                                telemetry.mark_first_byte();
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
                                telemetry.mark_first_byte();
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
                    telemetry.mark_first_byte();
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
) -> Result<UpstreamExecutionResult> {
    if let Some(auth) = selected_auth {
        if upstream.can_execute_for_auth(auth) {
            let mut last_error = None;
            for candidate in auth_retry_chain(snapshot, execution_plan, auth) {
                match upstream
                    .execute_responses_for_auth(&candidate, request.clone(), proxy_override)
                    .await
                {
                    Ok(response) => return Ok(response),
                    Err(err) if should_retry_auth_bound_error(&err) => {
                        let reason = classify_precommit_retry(&err).reason();
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
                        telemetry.record_auth_failure(classify_precommit_retry(&err).reason());
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
                .await;
        }
    }

    upstream
        .execute_responses(request.clone(), proxy_override)
        .await
}

fn auth_retry_chain(
    snapshot: &RuntimeSnapshot,
    execution_plan: &ExecutionPlan,
    selected_auth: &AuthRecord,
) -> Vec<AuthRecord> {
    let mut chain = vec![selected_auth.clone()];
    for auth_id in &execution_plan.retry_candidates {
        if let Some(auth) = snapshot.auth_pool.iter().find(|auth| auth.id == *auth_id) {
            chain.push(auth.clone());
        }
    }
    chain
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrecommitRetryClassification {
    Retryable(&'static str),
    NonRetryable(&'static str),
}

impl PrecommitRetryClassification {
    pub(super) fn should_retry(self) -> bool {
        matches!(self, Self::Retryable(_))
    }

    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::Retryable(reason) | Self::NonRetryable(reason) => reason,
        }
    }
}

pub(super) fn classify_precommit_retry(err: &anyhow::Error) -> PrecommitRetryClassification {
    let message = err.to_string().to_ascii_lowercase();
    if message.contains("upstream codex error 401") || message.contains("invalid_api_key") {
        return PrecommitRetryClassification::Retryable("auth_401");
    }
    if message.contains("upstream codex error 403") || message.contains("account_deactivated") {
        return PrecommitRetryClassification::Retryable("auth_403");
    }
    if is_codex_quota_exhaustion_error(&message) {
        return PrecommitRetryClassification::Retryable("usage_limit_reached");
    }
    if message.contains("upstream codex error 429") {
        return PrecommitRetryClassification::NonRetryable("auth_429");
    }
    PrecommitRetryClassification::NonRetryable("other")
}

pub(super) fn should_retry_auth_bound_error(err: &anyhow::Error) -> bool {
    classify_precommit_retry(err).should_retry()
}

fn is_codex_quota_exhaustion_error(message: &str) -> bool {
    message.contains("upstream codex error 429")
        && (message.contains("usage_limit_reached")
            || message.contains("the usage limit has been reached"))
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
    let ir = ResponsesRequestIr::from_downstream_request(&request);
    debug_assert!(!ir.model().trim().is_empty());
    ir.emit_upstream_request(execution_plan)
}

async fn aggregate_streaming_response_body(
    first_chunk: Bytes,
    mut tail: cliproxy_upstream_runtime::ByteStream,
    telemetry: RequestTelemetry,
) -> Result<Bytes> {
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
