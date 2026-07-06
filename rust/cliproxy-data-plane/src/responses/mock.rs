use std::{convert::Infallible, time::Instant};

use anyhow::{Result, bail};
use async_stream::stream;
use axum::{
    Json,
    body::Body,
    http::{HeaderValue, Response, header},
};
use bytes::Bytes;
use cliproxy_common_types::routing::ExecutionPlan;
use futures_util::stream::Stream;
use serde_json::json;
use tracing::info;

use crate::telemetry::{RequestTelemetry, StreamCompletionGuard};

use super::{
    MockCompletedResponse, MockSseEvent, ResponsesRequest, ResponsesRequestMetadata,
    build_output_text, estimate_input_tokens, estimate_output_tokens,
    estimate_output_tokens_from_text, mock_response_id,
};

/// 本地 mock 的非流式 `/v1/responses` 回包。
///
/// 当真实 upstream 不可用时，这条路径提供最小可运行闭环，并继续喂 telemetry。
pub(super) fn non_streaming_response(
    _request: ResponsesRequest,
    request_meta: ResponsesRequestMetadata,
    execution_plan: &ExecutionPlan,
    telemetry: RequestTelemetry,
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
    telemetry.observe_response_json_value(&json!({
        "id": response_id,
        "status": "completed",
        "usage": usage
    }));

    Ok(Json(MockCompletedResponse {
        id: mock_response_id(&execution_plan.model),
        object: "response",
        model: execution_plan.model.clone(),
        status: "completed",
        output,
        usage,
    }))
}

/// 本地 mock 的流式 `/v1/responses` 回包。
pub(super) async fn streaming_response(
    request: ResponsesRequest,
    request_meta: ResponsesRequestMetadata,
    execution_plan: &ExecutionPlan,
    telemetry: RequestTelemetry,
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
        .ok_or_else(|| anyhow::anyhow!("mock upstream produced no frames during bootstrap"))?;
    let first_byte_ms = start.elapsed().as_millis() as u64;

    telemetry.mark_first_byte();
    telemetry.observe_sse_frame(&first_frame);
    let tail_stream = frame_stream(
        first_frame.clone(),
        frames.collect(),
        start,
        telemetry.clone(),
    );
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

/// 把 mock SSE frame 顺序写回给下游，并通过 completion guard 统一收口 telemetry。
fn frame_stream(
    first_frame: Bytes,
    rest: Vec<Bytes>,
    start: Instant,
    telemetry: RequestTelemetry,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    stream! {
        let mut completion_guard = StreamCompletionGuard::new(telemetry.clone());
        yield Ok(first_frame);
        for frame in rest {
            telemetry.observe_sse_frame(&frame);
            yield Ok(frame);
        }
        let stream_duration_ms = start.elapsed().as_millis() as u64;
        info!(stream_duration_ms, "responses stream completed");
        completion_guard.finish_success();
    }
}

/// 生成 mock upstream 的最小事件序列，覆盖 created / delta / usage / completed。
fn mock_upstream_events(
    request: &ResponsesRequest,
    request_meta: &ResponsesRequestMetadata,
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

/// 把 mock 事件编码成标准 SSE frame。
pub(super) fn normalize_sse_frame(event: &MockSseEvent) -> Bytes {
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
