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
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::{debug, info, warn};

use crate::{
    auth_state::{AuthStateOverlay, RuntimeFailureState},
    error_events::{ErrorEvent, ErrorScope},
    telemetry::{RequestTelemetry, StreamCompletionGuard},
    usage_queue::UsageQueue,
};

use super::protocol::ResponsesRequestIr;
use super::sse::{ResponsesSseFramer, extract_completed_response_from_sse};
use super::{
    ResponsesRequest, handler::auth_overlay_index, response_from_aggregated_json_body,
    response_from_body, response_with_stream,
};

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
        &usage_queue,
        &auth_state,
    )
    .await?;

    match upstream_result {
        UpstreamExecutionResult::NonStreaming(response) => {
            let _provider = response.provider;
            let _events = response.events;
            if let Some(auth) = selected_auth {
                auth_state.clear_success(&auth_overlay_index(auth), &execution_plan.model);
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
                if let Some(auth) = selected_auth {
                    auth_state.clear_success(&auth_overlay_index(auth), &execution_plan.model);
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
            if let Some(auth) = selected_auth {
                auth_state.clear_success(&auth_overlay_index(auth), &execution_plan.model);
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
) -> Result<UpstreamExecutionResult> {
    if let Some(auth) = selected_auth {
        if upstream.can_execute_for_auth(auth) {
            let mut last_error = None;
            // 只有在下游响应还没提交时，才允许在 auth 绑定链内继续重试。
            for candidate in auth_retry_chain(snapshot, execution_plan, auth) {
                match upstream
                    .execute_responses_for_auth(&candidate, request.clone(), proxy_override)
                    .await
                {
                    Ok(response) => return Ok(response),
                    Err(err) if should_retry_auth_bound_error(&err) => {
                        let failure =
                            classify_failure(&err, &candidate, &execution_plan.model, telemetry);
                        apply_failure(auth_state, usage_queue, &failure);
                        let reason = failure.reason.as_str();
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
                        let failure =
                            classify_failure(&err, &candidate, &execution_plan.model, telemetry);
                        apply_failure(auth_state, usage_queue, &failure);
                        telemetry.record_auth_failure(failure.reason.as_str());
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

#[derive(Debug, Clone)]
struct ClassifiedFailure {
    auth_index: String,
    model: String,
    request_id: String,
    scope: ErrorScope,
    status_code: u16,
    error_code: String,
    message: String,
    retry_after_ms: u64,
    cooldown_until: Option<OffsetDateTime>,
    quota_exceeded: bool,
    reason: String,
}

fn classify_failure(
    err: &anyhow::Error,
    auth: &AuthRecord,
    model: &str,
    telemetry: &RequestTelemetry,
) -> ClassifiedFailure {
    let status_code = classify_status_code(err);
    let message = err.to_string();
    let error_code = classify_error_code(&message, status_code);
    let retry_after_ms = extract_retry_after_ms(&message);
    let now = OffsetDateTime::now_utc();
    let (scope, cooldown_until, quota_exceeded, reason) = match status_code {
        401 => (
            ErrorScope::Auth,
            Some(now + Duration::minutes(30)),
            false,
            "auth_401".to_string(),
        ),
        402 | 403 => (
            ErrorScope::Auth,
            Some(now + Duration::minutes(30)),
            false,
            "auth_403".to_string(),
        ),
        404 => (
            ErrorScope::Model,
            Some(now + Duration::hours(12)),
            false,
            "not_found".to_string(),
        ),
        429 if is_codex_quota_exhaustion_error(&message.to_ascii_lowercase()) => (
            ErrorScope::Model,
            retry_after_deadline(now, retry_after_ms).or(Some(now + Duration::seconds(1))),
            true,
            "usage_limit_reached".to_string(),
        ),
        400 | 422 if is_model_support_error_message(&message) => (
            ErrorScope::Model,
            Some(now + Duration::hours(12)),
            false,
            "model_not_supported".to_string(),
        ),
        408 | 500 | 502 | 503 | 504 => (
            ErrorScope::Model,
            Some(now + Duration::seconds(60)),
            false,
            "transient_upstream".to_string(),
        ),
        _ => (
            ErrorScope::Model,
            None,
            false,
            classify_precommit_retry(err).reason().to_string(),
        ),
    };
    ClassifiedFailure {
        auth_index: auth_overlay_index(auth),
        model: model.to_string(),
        request_id: telemetry.error_event_request_id(),
        scope,
        status_code,
        error_code,
        message,
        retry_after_ms,
        cooldown_until,
        quota_exceeded,
        reason,
    }
}

fn apply_failure(
    auth_state: &AuthStateOverlay,
    usage_queue: &UsageQueue,
    failure: &ClassifiedFailure,
) {
    if failure.cooldown_until.is_some() {
        let state = RuntimeFailureState {
            unavailable: true,
            status_message: failure.reason.clone(),
            last_error_code: failure.error_code.clone(),
            last_error_message: failure.message.clone(),
            next_retry_after: failure.cooldown_until,
            quota_exceeded: failure.quota_exceeded,
            quota_reason: if failure.quota_exceeded {
                failure.reason.clone()
            } else {
                String::new()
            },
            updated_at: Some(OffsetDateTime::now_utc()),
        };
        match failure.scope {
            ErrorScope::Auth => auth_state.set_auth_failure(&failure.auth_index, state),
            ErrorScope::Model => {
                auth_state.set_model_failure(&failure.auth_index, &failure.model, state)
            }
        }
    }
    usage_queue.enqueue_error(ErrorEvent {
        timestamp: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        request_id: failure.request_id.clone(),
        provider: "codex".to_string(),
        model: failure.model.clone(),
        auth_index: failure.auth_index.clone(),
        scope: failure.scope,
        status_code: failure.status_code,
        error_code: failure.error_code.clone(),
        message: failure.message.clone(),
        retry_after_ms: failure.retry_after_ms,
        cooldown_until: failure
            .cooldown_until
            .and_then(|value| value.format(&Rfc3339).ok())
            .unwrap_or_default(),
        quota_exceeded: failure.quota_exceeded,
        reason: failure.reason.clone(),
    });
}

fn retry_after_deadline(now: OffsetDateTime, retry_after_ms: u64) -> Option<OffsetDateTime> {
    (retry_after_ms > 0).then_some(now + Duration::milliseconds(retry_after_ms as i64))
}

fn classify_status_code(err: &anyhow::Error) -> u16 {
    let message = err.to_string().to_ascii_lowercase();
    for code in [401u16, 402, 403, 404, 408, 422, 429, 500, 502, 503, 504] {
        if message.contains(&format!("upstream codex error {code}")) {
            return code;
        }
    }
    0
}

fn classify_error_code(message: &str, status_code: u16) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("invalid_api_key")
        || lower.contains("invalid or expired token")
        || lower.contains("refresh_token_reused")
    {
        return "authentication_error".to_string();
    }
    if lower.contains("usage_limit_reached") {
        return "usage_limit_reached".to_string();
    }
    if is_model_support_error_message(message) {
        return "model_not_supported".to_string();
    }
    if status_code == 404 {
        return "not_found".to_string();
    }
    if matches!(status_code, 408 | 500 | 502 | 503 | 504) {
        return "transient_upstream".to_string();
    }
    "upstream_error".to_string()
}

fn is_model_support_error_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "model_not_supported",
        "requested model is not supported",
        "unsupported model",
        "not available for your plan",
        "not available for your account",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn extract_retry_after_ms(message: &str) -> u64 {
    extract_json_number(message, "resets_in_seconds").saturating_mul(1000)
}

fn extract_json_number(message: &str, key: &str) -> u64 {
    let needle = format!("\"{key}\":");
    let Some(start) = message.find(&needle) else {
        return 0;
    };
    let mut digits = String::new();
    for ch in message[start + needle.len()..].chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    digits.parse::<u64>().unwrap_or(0)
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

/// 下游首字节尚未提交前，auth 绑定错误是否允许切换到下一个候选 auth 的分类结果。
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
