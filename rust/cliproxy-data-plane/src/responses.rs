use std::{convert::Infallible, time::Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_stream::stream;
use axum::{
    Json,
    body::Body,
    http::{HeaderValue, Response, StatusCode, header},
    response::IntoResponse,
};
use bytes::Bytes;
use cliproxy_common_types::routing::ExecutionPlan;
use cliproxy_common_types::snapshot::{AuthRecord, RuntimeSnapshot};
use cliproxy_router_core::{
    PlanRequest, RouterCore, extract_codex_session_id, extract_pinned_auth_id,
};
use cliproxy_upstream_runtime::{UpstreamExecutionResult, UpstreamRequest, UpstreamRuntime};
use futures_util::{StreamExt, stream::Stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::runtime::RuntimeStateHandle;

const DEFAULT_CODEX_INSTRUCTIONS: &str = "You are Codex. Fulfill the user's request.";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResponsesRequest {
    pub model: String,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorDetail {
    message: String,
    #[serde(rename = "type")]
    kind: &'static str,
    code: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
struct MockCompletedResponse {
    id: String,
    object: &'static str,
    model: String,
    status: &'static str,
    output: Vec<Value>,
    usage: Value,
}

#[derive(Debug, Clone)]
struct RequestMetadata {
    model: String,
    prompt_preview: String,
    metadata_keys: usize,
}

#[derive(Debug, Clone)]
struct MockSseEvent {
    event: String,
    payload: Value,
}

pub async fn handle_responses(
    runtime: RuntimeStateHandle,
    router_core: RouterCore,
    upstream: UpstreamRuntime,
    request: ResponsesRequest,
) -> Response<Body> {
    if !runtime.responses_route_enabled() {
        return error_response(
            StatusCode::NOT_FOUND,
            "responses route is disabled by runtime snapshot",
            "route_disabled",
        );
    }

    let request_meta = match extract_metadata(&request) {
        Ok(meta) => meta,
        Err(err) => {
            return error_response(StatusCode::BAD_REQUEST, &err.to_string(), "invalid_request");
        }
    };
    let (snapshot, execution_plan) = match build_execution_plan(&runtime, &router_core, &request) {
        Ok(resolved) => resolved,
        Err(err) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
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
                error_response(StatusCode::BAD_GATEWAY, &err.to_string(), "upstream_error")
            }
        };
    }

    if request.stream {
        match streaming_response(request, request_meta, &execution_plan).await {
            Ok(response) => response,
            Err(err) => error_response(StatusCode::BAD_GATEWAY, &err.to_string(), "upstream_error"),
        }
    } else {
        match non_streaming_response(request, request_meta, &execution_plan) {
            Ok(response) => response.into_response(),
            Err(err) => error_response(StatusCode::BAD_GATEWAY, &err.to_string(), "upstream_error"),
        }
    }
}

fn build_execution_plan(
    runtime: &RuntimeStateHandle,
    router_core: &RouterCore,
    request: &ResponsesRequest,
) -> Result<(std::sync::Arc<RuntimeSnapshot>, ExecutionPlan)> {
    let snapshot = runtime
        .current_snapshot()
        .ok_or_else(|| anyhow!("runtime snapshot is not loaded"))?;
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

async fn execute_real_upstream(
    upstream: UpstreamRuntime,
    request: ResponsesRequest,
    snapshot: &RuntimeSnapshot,
    execution_plan: &ExecutionPlan,
    selected_auth: Option<&AuthRecord>,
) -> Result<Response<Body>> {
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
    )
    .await?;

    match upstream_result {
        UpstreamExecutionResult::NonStreaming(response) => {
            let _provider = response.provider;
            let _events = response.events;
            Ok(response_from_body(response.head, response.body))
        }
        UpstreamExecutionResult::Streaming(response) => {
            if aggregate_codex_stream {
                let _provider = response.provider;
                let _events = response.events;
                let body = aggregate_streaming_response_body(response.first_chunk, response.stream)
                    .await?;
                return Ok(response_from_aggregated_json_body(response.head, body));
            }
            let _provider = response.provider;
            let _events = response.events;
            let first_chunk = response.first_chunk;
            let tail = response.stream;
            let body = Body::from_stream(stream! {
                yield Ok::<Bytes, Infallible>(first_chunk);
                futures_util::pin_mut!(tail);
                while let Some(item) = tail.next().await {
                    match item {
                        Ok(bytes) => yield Ok(bytes),
                        Err(err) => {
                            let frame = Bytes::from(format!(
                                "event: response.error\ndata: {{\"type\":\"response.error\",\"message\":{}}}\n\n",
                                serde_json::to_string(&err.to_string()).unwrap_or_else(|_| "\"upstream stream error\"".to_string())
                            ));
                            yield Ok(frame);
                            break;
                        }
                    }
                }
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
                        info!(
                            failed_auth_id = %candidate.id,
                            next_retry_candidates = ?execution_plan.retry_candidates,
                            reason = %err,
                            "responses upstream auth failed, retrying next candidate"
                        );
                        last_error = Some(err);
                    }
                    Err(err) => return Err(err),
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

fn should_retry_auth_bound_error(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("upstream codex error 401")
        || message.contains("upstream codex error 403")
        || is_codex_quota_exhaustion_error(&message)
        || message.contains("account_deactivated")
        || message.contains("invalid_api_key")
}

fn is_codex_quota_exhaustion_error(message: &str) -> bool {
    message.contains("upstream codex error 429")
        && (message.contains("usage_limit_reached")
            || message.contains("the usage limit has been reached"))
}

fn log_upstream_failure(err: &anyhow::Error, execution_plan: &ExecutionPlan) {
    warn!(
        provider = ?execution_plan.provider,
        auth_id = %execution_plan.auth_id,
        resolved_model = %execution_plan.model,
        stickiness_source = ?execution_plan.stickiness_source,
        error = %err,
        "responses upstream execution failed"
    );

    let chain = err
        .chain()
        .enumerate()
        .map(|(index, cause)| format!("{index}: {cause}"))
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

fn normalize_upstream_request(
    mut request: ResponsesRequest,
    execution_plan: &ExecutionPlan,
) -> ResponsesRequest {
    if execution_plan.provider != cliproxy_common_types::upstream::ProviderKind::Codex {
        return request;
    }

    request.store = Some(false);
    request.metadata = None;

    if request
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        request.instructions = Some(DEFAULT_CODEX_INSTRUCTIONS.to_string());
    }

    if let Some(Value::String(text)) = request.input.take() {
        request.input = Some(json!([
            {
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": text
                    }
                ]
            }
        ]));
    }

    request
}

async fn aggregate_streaming_response_body(
    first_chunk: Bytes,
    mut tail: cliproxy_upstream_runtime::ByteStream,
) -> Result<Bytes> {
    let mut combined = Vec::from(first_chunk.as_ref());
    while let Some(item) = tail.next().await {
        let bytes = item?;
        combined.extend_from_slice(&bytes);
    }
    extract_completed_response_from_sse(&combined)
}

fn extract_completed_response_from_sse(bytes: &[u8]) -> Result<Bytes> {
    for frame in sse_frames(bytes) {
        let Some(payload) = sse_data_payload(frame) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&payload) else {
            continue;
        };
        if value
            .get("type")
            .and_then(Value::as_str)
            .map(|value| value == "response.completed")
            .unwrap_or(false)
        {
            let response = value
                .get("response")
                .ok_or_else(|| anyhow!("response.completed event missing response payload"))?;
            return serde_json::to_vec(response)
                .map(Bytes::from)
                .context("failed to serialize aggregated response body");
        }
    }
    bail!("upstream stream did not produce response.completed")
}

fn sse_frames(bytes: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index..].starts_with(b"\r\n\r\n") {
            frames.push(&bytes[start..index]);
            index += 4;
            start = index;
            continue;
        }
        if bytes[index..].starts_with(b"\n\n") {
            frames.push(&bytes[start..index]);
            index += 2;
            start = index;
            continue;
        }
        index += 1;
    }

    if start < bytes.len() {
        frames.push(&bytes[start..]);
    }

    frames
}

fn sse_data_payload(frame: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(frame).ok()?;
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(data) = trimmed.strip_prefix("data:") {
            lines.push(data.trim_start().to_string());
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n").into_bytes())
}

fn non_streaming_response(
    _request: ResponsesRequest,
    request_meta: RequestMetadata,
    execution_plan: &ExecutionPlan,
) -> Result<Json<MockCompletedResponse>> {
    let response_id = mock_response_id(&execution_plan.model);
    let output_text = build_output_text(&request_meta);
    let output = vec![json!({
        "id": format!("{response_id}_item_0"),
        "type": "message",
        "role": "assistant",
        "content": [
            {
                "type": "output_text",
                "text": output_text
            }
        ]
    })];
    let usage = json!({
        "input_tokens": estimate_input_tokens(&request_meta),
        "output_tokens": estimate_output_tokens(&output),
        "total_tokens": estimate_input_tokens(&request_meta) + estimate_output_tokens(&output)
    });

    Ok(Json(MockCompletedResponse {
        id: response_id,
        object: "response",
        model: execution_plan.model.clone(),
        status: "completed",
        output,
        usage,
    }))
}

async fn streaming_response(
    request: ResponsesRequest,
    request_meta: RequestMetadata,
    execution_plan: &ExecutionPlan,
) -> Result<Response<Body>> {
    let start = Instant::now();
    let events = mock_upstream_events(&request, &request_meta, execution_plan)?;
    let mut frames = events
        .into_iter()
        .map(|event| normalize_sse_frame(&event))
        .collect::<Vec<_>>()
        .into_iter();

    let first_frame = frames
        .next()
        .ok_or_else(|| anyhow!("mock upstream produced no frames during bootstrap"))?;
    let first_byte_ms = start.elapsed().as_millis() as u64;

    let tail_stream = frame_stream(first_frame.clone(), frames.collect(), start);
    let body = Body::from_stream(tail_stream);

    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );

    info!(
        model = %request_meta.model,
        resolved_model = %execution_plan.model,
        auth_id = %execution_plan.auth_id,
        prompt_preview = %request_meta.prompt_preview,
        metadata_keys = request_meta.metadata_keys,
        first_byte_ms,
        "responses stream bootstrap ready"
    );

    Ok(response)
}

fn frame_stream(
    first_frame: Bytes,
    rest: Vec<Bytes>,
    start: Instant,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    stream! {
        yield Ok(first_frame);
        for frame in rest {
            yield Ok(frame);
        }
        let stream_duration_ms = start.elapsed().as_millis() as u64;
        info!(stream_duration_ms, "responses stream completed");
    }
}

fn mock_upstream_events(
    request: &ResponsesRequest,
    request_meta: &RequestMetadata,
    execution_plan: &ExecutionPlan,
) -> Result<Vec<MockSseEvent>> {
    if execution_plan.model.trim().is_empty() {
        bail!("model must not be empty");
    }

    let response_id = mock_response_id(&execution_plan.model);
    let output_text = build_output_text(request_meta);
    let usage = json!({
        "input_tokens": estimate_input_tokens(request_meta),
        "output_tokens": estimate_output_tokens_from_text(&output_text),
        "total_tokens": estimate_input_tokens(request_meta) + estimate_output_tokens_from_text(&output_text)
    });

    Ok(vec![
        MockSseEvent {
            event: "response.created".to_string(),
            payload: json!({
                "type": "response.created",
                "response": {
                    "id": response_id,
                    "model": request.model,
                    "status": "in_progress"
                }
            }),
        },
        MockSseEvent {
            event: "response.output_text.delta".to_string(),
            payload: json!({
                "type": "response.output_text.delta",
                "delta": output_text
            }),
        },
        MockSseEvent {
            event: "response.usage".to_string(),
            payload: json!({
                "type": "response.usage",
                "usage": usage
            }),
        },
        MockSseEvent {
            event: "response.completed".to_string(),
            payload: json!({
                "type": "response.completed",
                "response": {
                    "id": response_id,
                    "model": request.model,
                    "status": "completed",
                    "output": [
                        {
                            "id": format!("{response_id}_item_0"),
                            "type": "message",
                            "role": "assistant",
                            "content": [
                                {
                                    "type": "output_text",
                                    "text": build_output_text(request_meta)
                                }
                            ]
                        }
                    ],
                    "usage": usage
                }
            }),
        },
    ])
}

fn normalize_sse_frame(event: &MockSseEvent) -> Bytes {
    let payload = serde_json::to_string(&event.payload).unwrap_or_else(|_| "{}".to_string());
    let mut frame = String::new();

    if !event.event.trim().is_empty() {
        frame.push_str("event: ");
        frame.push_str(event.event.trim());
        frame.push('\n');
    }

    for line in payload.lines() {
        frame.push_str("data: ");
        frame.push_str(line);
        frame.push('\n');
    }
    frame.push('\n');

    Bytes::from(frame)
}

fn extract_metadata(request: &ResponsesRequest) -> Result<RequestMetadata> {
    let model = request.model.trim().to_string();
    if model.is_empty() {
        bail!("model is required");
    }

    let prompt_preview = request
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| truncate_preview(value, 48))
        .or_else(|| extract_prompt_preview(request.input.as_ref()))
        .unwrap_or_else(|| "empty-input".to_string());

    let metadata_keys = request
        .metadata
        .as_ref()
        .and_then(|value| value.as_object())
        .map(|object| object.len())
        .unwrap_or(0);

    Ok(RequestMetadata {
        model,
        prompt_preview,
        metadata_keys,
    })
}

fn extract_prompt_preview(input: Option<&Value>) -> Option<String> {
    let input = input?;
    match input {
        Value::String(text) => Some(truncate_preview(text.trim(), 48)),
        Value::Array(items) => items.iter().find_map(extract_prompt_preview_from_value),
        Value::Object(map) => map.values().find_map(extract_prompt_preview_from_value),
        _ => None,
    }
}

fn extract_prompt_preview_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| truncate_preview(trimmed, 48))
        }
        Value::Array(items) => items.iter().find_map(extract_prompt_preview_from_value),
        Value::Object(map) => map.values().find_map(extract_prompt_preview_from_value),
        _ => None,
    }
}

fn truncate_preview(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn build_output_text(request_meta: &RequestMetadata) -> String {
    format!(
        "mock responses ingress accepted model={} preview={}",
        request_meta.model, request_meta.prompt_preview
    )
}

fn estimate_input_tokens(request_meta: &RequestMetadata) -> u64 {
    request_meta.prompt_preview.chars().count().max(1) as u64
}

fn estimate_output_tokens(output: &[Value]) -> u64 {
    output
        .iter()
        .map(|item| item.to_string().chars().count() as u64)
        .sum::<u64>()
        .max(1)
}

fn estimate_output_tokens_from_text(text: &str) -> u64 {
    text.chars().count().max(1) as u64
}

fn mock_response_id(model: &str) -> String {
    let normalized = model.replace(['/', ' '], "-");
    format!("resp_mock_{normalized}")
}

fn error_response(status: StatusCode, message: &str, code: &'static str) -> Response<Body> {
    (
        status,
        Json(ErrorResponse {
            error: ErrorDetail {
                message: message.to_string(),
                kind: "invalid_request_error",
                code,
            },
        }),
    )
        .into_response()
}

fn response_from_body(
    head: cliproxy_common_types::upstream::UpstreamResponseHead,
    body: Bytes,
) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::from_u16(head.status).unwrap_or(StatusCode::OK);
    apply_upstream_headers(response.headers_mut(), &head.headers, false);
    response
}

fn response_from_aggregated_json_body(
    head: cliproxy_common_types::upstream::UpstreamResponseHead,
    body: Bytes,
) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::from_u16(head.status).unwrap_or(StatusCode::OK);
    apply_upstream_headers(response.headers_mut(), &head.headers, true);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn response_with_stream(
    head: cliproxy_common_types::upstream::UpstreamResponseHead,
    body: Body,
) -> Response<Body> {
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::from_u16(head.status).unwrap_or(StatusCode::OK);
    apply_upstream_headers(response.headers_mut(), &head.headers, false);
    response
}

fn apply_upstream_headers(
    headers: &mut axum::http::HeaderMap,
    source: &std::collections::BTreeMap<String, String>,
    body_rewritten: bool,
) {
    for (key, value) in source {
        if should_skip_upstream_header(key, body_rewritten) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            axum::http::header::HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.insert(name, value);
        }
    }
}

fn should_skip_upstream_header(name: &str, body_rewritten: bool) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    ) {
        return true;
    }

    body_rewritten
        && matches!(
            normalized.as_str(),
            "content-length" | "content-encoding" | "content-range"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_frame_has_event_and_data_lines() {
        let frame = normalize_sse_frame(&MockSseEvent {
            event: "response.created".to_string(),
            payload: json!({"type":"response.created"}),
        });
        let text = String::from_utf8(frame.to_vec()).expect("valid utf8");
        assert!(text.starts_with("event: response.created\n"));
        assert!(text.contains("data: {\"type\":\"response.created\"}\n\n"));
    }

    #[test]
    fn extract_metadata_uses_input_when_instructions_missing() {
        let request = ResponsesRequest {
            model: "gpt-5".to_string(),
            stream: true,
            input: Some(json!([{"content":"hello world from input"}])),
            instructions: None,
            metadata: Some(json!({"client":"test"})),
            store: None,
        };

        let meta = extract_metadata(&request).expect("extract metadata");
        assert_eq!(meta.model, "gpt-5");
        assert!(meta.prompt_preview.contains("hello world"));
        assert_eq!(meta.metadata_keys, 1);
    }

    #[test]
    fn normalize_upstream_request_adds_codex_defaults() {
        let request = ResponsesRequest {
            model: "codex-latest".to_string(),
            stream: false,
            input: Some(Value::String("hello from codex".to_string())),
            instructions: None,
            metadata: None,
            store: None,
        };
        let plan = ExecutionPlan {
            provider: cliproxy_common_types::upstream::ProviderKind::Codex,
            model: "gpt-5.5".to_string(),
            auth_id: "auth-1".to_string(),
            retry_candidates: vec![],
            stickiness_source: cliproxy_common_types::routing::StickinessSource::Strategy,
        };

        let normalized = normalize_upstream_request(request, &plan);
        assert_eq!(normalized.store, Some(false));
        assert_eq!(
            normalized.instructions.as_deref(),
            Some(DEFAULT_CODEX_INSTRUCTIONS)
        );
        assert_eq!(
            normalized.input,
            Some(json!([
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": "hello from codex"
                        }
                    ]
                }
            ]))
        );
    }

    #[test]
    fn normalize_upstream_request_preserves_non_codex_request() {
        let request = ResponsesRequest {
            model: "gpt-5".to_string(),
            stream: false,
            input: Some(json!("hello")),
            instructions: None,
            metadata: Some(json!({"client":"test"})),
            store: None,
        };
        let plan = ExecutionPlan {
            provider: cliproxy_common_types::upstream::ProviderKind::OpenAi,
            model: "gpt-5".to_string(),
            auth_id: "auth-1".to_string(),
            retry_candidates: vec![],
            stickiness_source: cliproxy_common_types::routing::StickinessSource::Strategy,
        };

        let normalized = normalize_upstream_request(request.clone(), &plan);
        assert_eq!(normalized.model, request.model);
        assert_eq!(normalized.input, request.input);
        assert_eq!(normalized.instructions, request.instructions);
        assert_eq!(normalized.store, request.store);
    }

    #[test]
    fn extract_completed_response_from_sse_returns_response_payload() {
        let bytes = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-1\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-1\",\"object\":\"response\",\"status\":\"completed\"}}\n\n"
        )
        .as_bytes()
        .to_vec();

        let aggregated = extract_completed_response_from_sse(&bytes).expect("aggregate sse");
        let payload: Value = serde_json::from_slice(&aggregated).expect("parse aggregated body");
        assert_eq!(payload["id"], "resp-1");
        assert_eq!(payload["object"], "response");
        assert_eq!(payload["status"], "completed");
    }

    #[test]
    fn apply_upstream_headers_strips_hop_by_hop_headers() {
        let mut headers = axum::http::HeaderMap::new();
        let source = std::collections::BTreeMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("connection".to_string(), "keep-alive".to_string()),
            ("keep-alive".to_string(), "timeout=4".to_string()),
            ("proxy-connection".to_string(), "keep-alive".to_string()),
            ("transfer-encoding".to_string(), "chunked".to_string()),
        ]);

        apply_upstream_headers(&mut headers, &source, false);

        assert_eq!(
            headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert!(headers.get(header::CONNECTION).is_none());
        assert!(headers.get("keep-alive").is_none());
        assert!(headers.get("proxy-connection").is_none());
        assert!(headers.get(header::TRANSFER_ENCODING).is_none());
    }

    #[test]
    fn apply_upstream_headers_drops_entity_headers_when_body_is_rewritten() {
        let mut headers = axum::http::HeaderMap::new();
        let source = std::collections::BTreeMap::from([
            ("content-length".to_string(), "999".to_string()),
            ("content-encoding".to_string(), "gzip".to_string()),
            ("content-range".to_string(), "bytes 0-10/10".to_string()),
            ("x-request-id".to_string(), "req_123".to_string()),
        ]);

        apply_upstream_headers(&mut headers, &source, true);

        assert!(headers.get(header::CONTENT_LENGTH).is_none());
        assert!(headers.get(header::CONTENT_ENCODING).is_none());
        assert!(headers.get(header::CONTENT_RANGE).is_none());
        assert_eq!(
            headers
                .get("x-request-id")
                .and_then(|value| value.to_str().ok()),
            Some("req_123")
        );
    }

    #[test]
    fn should_retry_auth_bound_error_accepts_codex_quota_exhaustion() {
        let err = anyhow!(
            "upstream codex error 429: {{\"error\":{{\"type\":\"usage_limit_reached\",\"message\":\"The usage limit has been reached\"}}}}"
        );
        assert!(should_retry_auth_bound_error(&err));
    }

    #[test]
    fn should_retry_auth_bound_error_rejects_generic_429() {
        let err = anyhow!("upstream codex error 429: too many requests");
        assert!(!should_retry_auth_bound_error(&err));
    }
}
