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
use tracing::info;

use crate::runtime::RuntimeStateHandle;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResponsesRequest {
    pub model: String,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub input: Option<Value>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
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

    if upstream.is_enabled() {
        return match execute_real_upstream(upstream, request, &execution_plan, selected_auth).await
        {
            Ok(response) => response,
            Err(err) => error_response(StatusCode::BAD_GATEWAY, &err.to_string(), "upstream_error"),
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

async fn execute_real_upstream(
    upstream: UpstreamRuntime,
    request: ResponsesRequest,
    execution_plan: &ExecutionPlan,
    selected_auth: Option<&AuthRecord>,
) -> Result<Response<Body>> {
    let mut upstream_request = request.clone();
    upstream_request.model = execution_plan.model.clone();
    let request_body =
        serde_json::to_vec(&upstream_request).context("failed to serialize responses request")?;
    info!(
        provider = ?execution_plan.provider,
        auth_id = %execution_plan.auth_id,
        resolved_model = %execution_plan.model,
        stickiness_source = ?execution_plan.stickiness_source,
        "responses execution plan selected"
    );
    let upstream_result = if let Some(auth) = selected_auth {
        if upstream.can_execute_for_auth(auth) {
            upstream
                .execute_responses_for_auth(
                    auth,
                    UpstreamRequest {
                        model: upstream_request.model.clone(),
                        body: request_body,
                        stream: upstream_request.stream,
                    },
                )
                .await?
        } else {
            upstream
                .execute_responses(UpstreamRequest {
                    model: upstream_request.model.clone(),
                    body: request_body,
                    stream: upstream_request.stream,
                })
                .await?
        }
    } else {
        upstream
            .execute_responses(UpstreamRequest {
                model: upstream_request.model.clone(),
                body: request_body,
                stream: upstream_request.stream,
            })
            .await?
    };

    match upstream_result {
        UpstreamExecutionResult::NonStreaming(response) => {
            let _provider = response.provider;
            let _events = response.events;
            Ok(response_from_body(response.head, response.body))
        }
        UpstreamExecutionResult::Streaming(response) => {
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
    apply_upstream_headers(response.headers_mut(), &head.headers);
    response
}

fn response_with_stream(
    head: cliproxy_common_types::upstream::UpstreamResponseHead,
    body: Body,
) -> Response<Body> {
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::from_u16(head.status).unwrap_or(StatusCode::OK);
    apply_upstream_headers(response.headers_mut(), &head.headers);
    response
}

fn apply_upstream_headers(
    headers: &mut axum::http::HeaderMap,
    source: &std::collections::BTreeMap<String, String>,
) {
    for (key, value) in source {
        if let (Ok(name), Ok(value)) = (
            axum::http::header::HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.insert(name, value);
        }
    }
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
        };

        let meta = extract_metadata(&request).expect("extract metadata");
        assert_eq!(meta.model, "gpt-5");
        assert!(meta.prompt_preview.contains("hello world"));
        assert_eq!(meta.metadata_keys, 1);
    }
}
