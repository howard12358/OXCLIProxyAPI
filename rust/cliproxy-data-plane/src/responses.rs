use axum::{
    Json,
    body::Body,
    http::{HeaderValue, Response, StatusCode, header},
    response::IntoResponse,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

mod handler;
mod protocol;
pub(crate) mod sse;
mod upstream;

pub use handler::handle_responses;

const DEFAULT_CODEX_INSTRUCTIONS: &str = "You are Codex. Fulfill the user's request.";

/// `/v1/responses` 入口当前接受的下游请求结构。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
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
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
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

/// 从下游请求中抽取出来的轻量元信息，用于测试辅助、日志和粗粒度 token 估算。
#[cfg(test)]
#[derive(Debug, Clone)]
pub(super) struct RequestMetadata {
    model: String,
    prompt_preview: String,
    metadata_keys: usize,
}

#[cfg(test)]
fn extract_metadata(request: &ResponsesRequest) -> anyhow::Result<RequestMetadata> {
    let model = request.model.trim().to_string();
    if model.is_empty() {
        anyhow::bail!("model is required");
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

#[cfg(test)]
fn extract_prompt_preview(input: Option<&Value>) -> Option<String> {
    let input = input?;
    match input {
        Value::String(text) => Some(truncate_preview(text.trim(), 48)),
        Value::Array(items) => items.iter().find_map(extract_prompt_preview_from_value),
        Value::Object(map) => map.values().find_map(extract_prompt_preview_from_value),
        _ => None,
    }
}

#[cfg(test)]
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

#[cfg(test)]
fn truncate_preview(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
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
    use cliproxy_common_types::routing::{ExecutionPlan, StickinessSource};
    use serde::Deserialize;
    use serde_json::json;

    #[test]
    fn normalize_frame_has_event_and_data_lines() {
        let frame =
            normalize_test_sse_frame("response.created", json!({"type":"response.created"}));
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
            extra: BTreeMap::new(),
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
            extra: BTreeMap::new(),
        };
        let plan = ExecutionPlan {
            provider: cliproxy_common_types::upstream::ProviderKind::Codex,
            model: "gpt-5.5".to_string(),
            auth_id: "auth-1".to_string(),
            retry_candidates: vec![],
            stickiness_source: StickinessSource::Strategy,
        };

        let normalized = upstream::normalize_upstream_request(request, &plan);
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
            extra: BTreeMap::new(),
        };
        let plan = ExecutionPlan {
            provider: cliproxy_common_types::upstream::ProviderKind::OpenAi,
            model: "gpt-5".to_string(),
            auth_id: "auth-1".to_string(),
            retry_candidates: vec![],
            stickiness_source: StickinessSource::Strategy,
        };

        let normalized = upstream::normalize_upstream_request(request.clone(), &plan);
        assert_eq!(normalized.model, request.model);
        assert_eq!(normalized.input, request.input);
        assert_eq!(normalized.instructions, request.instructions);
        assert_eq!(normalized.store, request.store);
    }

    #[test]
    fn request_ir_emits_codex_payload_with_defaults_and_text_input_lifted_to_message() {
        let request = ResponsesRequest {
            model: "codex-latest".to_string(),
            stream: false,
            input: Some(json!("hello from ir")),
            instructions: None,
            metadata: Some(json!({"session_id":"sess_123"})),
            store: None,
            extra: BTreeMap::new(),
        };
        let plan = ExecutionPlan {
            provider: cliproxy_common_types::upstream::ProviderKind::Codex,
            model: "gpt-5.5".to_string(),
            auth_id: "auth-1".to_string(),
            retry_candidates: vec![],
            stickiness_source: StickinessSource::Strategy,
        };

        let ir = protocol::ResponsesRequestIr::from_downstream_request(&request);
        let emitted = ir.emit_upstream_request(&plan);

        assert_eq!(emitted.model, "gpt-5.5");
        assert_eq!(emitted.stream, request.stream);
        assert_eq!(emitted.store, Some(false));
        assert_eq!(
            emitted.instructions.as_deref(),
            Some(DEFAULT_CODEX_INSTRUCTIONS)
        );
        assert!(emitted.metadata.is_none());
        assert_eq!(
            emitted.input,
            Some(json!([
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": "hello from ir"
                        }
                    ]
                }
            ]))
        );
    }

    #[test]
    fn request_ir_preserves_codex_native_input_and_extra_fields() {
        let request = ResponsesRequest {
            model: "codex-latest".to_string(),
            stream: true,
            input: Some(json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": "hello from codex cli"
                        }
                    ]
                }
            ])),
            instructions: Some("system".to_string()),
            metadata: Some(json!({"client":"codex-test"})),
            store: Some(true),
            extra: std::collections::BTreeMap::from([
                (
                    "tools".to_string(),
                    json!([{"type":"function","name":"shell"}]),
                ),
                (
                    "include".to_string(),
                    json!(["reasoning.encrypted_content"]),
                ),
            ]),
        };
        let plan = ExecutionPlan {
            provider: cliproxy_common_types::upstream::ProviderKind::Codex,
            model: "gpt-5.5".to_string(),
            auth_id: "auth-1".to_string(),
            retry_candidates: vec![],
            stickiness_source: StickinessSource::Strategy,
        };

        let ir = protocol::ResponsesRequestIr::from_downstream_request(&request);
        let emitted = ir.emit_upstream_request(&plan);

        assert_eq!(emitted.input, request.input);
        assert_eq!(emitted.extra["tools"][0]["name"], "shell");
        assert_eq!(emitted.extra["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn request_ir_preserves_non_codex_request_shape() {
        let request = ResponsesRequest {
            model: "gpt-5".to_string(),
            stream: true,
            input: Some(
                json!([{"role":"user","content":[{"type":"input_text","text":"keep me"}]}]),
            ),
            instructions: Some("system".to_string()),
            metadata: Some(json!({"client":"test"})),
            store: Some(true),
            extra: BTreeMap::new(),
        };
        let plan = ExecutionPlan {
            provider: cliproxy_common_types::upstream::ProviderKind::OpenAi,
            model: "gpt-5".to_string(),
            auth_id: "auth-1".to_string(),
            retry_candidates: vec![],
            stickiness_source: StickinessSource::Strategy,
        };

        let ir = protocol::ResponsesRequestIr::from_downstream_request(&request);
        let emitted = ir.emit_upstream_request(&plan);

        assert_eq!(emitted, request);
    }

    #[test]
    fn stream_event_ir_parses_output_item_done_and_completed_frames() {
        let done = b"event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc-1\"}}\n\n";
        let completed = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-1\",\"output\":[]}}\n\n";

        let done_event = protocol::ResponsesStreamEventIr::from_sse_frame(done)
            .expect("parse done frame")
            .expect("stream event");
        let completed_event = protocol::ResponsesStreamEventIr::from_sse_frame(completed)
            .expect("parse completed frame")
            .expect("stream event");

        match done_event {
            protocol::ResponsesStreamEventIr::OutputItemDone(done) => {
                assert_eq!(done.output_index, Some(0));
                assert_eq!(done.item["id"], "fc-1");
            }
            other => panic!("unexpected done event: {other:?}"),
        }

        match completed_event {
            protocol::ResponsesStreamEventIr::Completed(completed) => {
                assert_eq!(completed.response["id"], "resp-1");
            }
            other => panic!("unexpected completed event: {other:?}"),
        }
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

        let aggregated = sse::extract_completed_response_from_sse(&bytes).expect("aggregate sse");
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
        let err = anyhow::anyhow!(
            "upstream codex error 429: {{\"error\":{{\"type\":\"usage_limit_reached\",\"message\":\"The usage limit has been reached\"}}}}"
        );
        assert!(upstream::should_retry_auth_bound_error(&err));
        let classification = upstream::classify_precommit_retry(&err);
        assert!(classification.should_retry());
        assert_eq!(classification.reason(), "usage_limit_reached");
    }

    #[test]
    fn should_retry_auth_bound_error_rejects_generic_429() {
        let err = anyhow::anyhow!("upstream codex error 429: too many requests");
        assert!(!upstream::should_retry_auth_bound_error(&err));
        let classification = upstream::classify_precommit_retry(&err);
        assert!(!classification.should_retry());
        assert_eq!(classification.reason(), "auth_429");
    }

    #[test]
    fn classify_precommit_retry_marks_auth_failures_and_quota_as_retryable() {
        let cases = [
            (
                "upstream codex error 401: {\"error\":{\"code\":\"invalid_api_key\"}}",
                "auth_401",
            ),
            (
                "upstream codex error 403: {\"error\":{\"code\":\"account_deactivated\"}}",
                "auth_403",
            ),
            (
                "upstream codex error 429: {\"error\":{\"type\":\"usage_limit_reached\",\"message\":\"The usage limit has been reached\"}}",
                "usage_limit_reached",
            ),
        ];

        for (message, reason) in cases {
            let err = anyhow::anyhow!(message);
            let classification = upstream::classify_precommit_retry(&err);
            assert!(classification.should_retry(), "{message}");
            assert_eq!(classification.reason(), reason, "{message}");
        }
    }

    #[test]
    fn classify_precommit_retry_marks_other_upstream_failures_as_non_retryable() {
        let cases = [
            ("upstream openai error 500: boom", "other"),
            ("upstream codex error 400: invalid_request", "other"),
        ];

        for (message, reason) in cases {
            let err = anyhow::anyhow!(message);
            let classification = upstream::classify_precommit_retry(&err);
            assert!(!classification.should_retry(), "{message}");
            assert_eq!(classification.reason(), reason, "{message}");
        }
    }

    #[test]
    fn sse_framer_reassembles_split_event_and_data_chunks() {
        let mut framer = sse::ResponsesSseFramer::default();

        let out1 = framer.push_chunk(Bytes::from_static(b"event: response.created"));
        let out2 = framer.push_chunk(Bytes::from_static(
            b"\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-1\"}}",
        ));
        let out3 = framer.push_chunk(Bytes::from_static(b"\n\n"));

        assert!(out1.is_empty());
        let mut frames = Vec::new();
        frames.extend(out2);
        frames.extend(out3.into_iter().filter(|frame| {
            std::str::from_utf8(frame)
                .map(|text| !text.trim().is_empty())
                .unwrap_or(true)
        }));
        assert_eq!(frames.len(), 1);
        assert_eq!(
            String::from_utf8(frames[0].to_vec()).expect("valid utf8"),
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-1\"}}\n\n"
        );
    }

    #[test]
    fn sse_framer_repairs_completed_output_from_done_items() {
        let mut framer = sse::ResponsesSseFramer::default();

        let done = framer.push_chunk(Bytes::from_static(
            b"data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc-1\",\"call_id\":\"call-1\",\"name\":\"shell\",\"arguments\":\"{}\"}}\n\n",
        ));
        let completed = framer.push_chunk(Bytes::from_static(
            b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-1\",\"output\":[]}}\n\n",
        ));

        assert_eq!(done.len(), 1);
        assert_eq!(completed.len(), 1);
        let payload = sse::sse_data_payload(&completed[0]).expect("sse payload");
        let output = serde_json::from_slice::<Value>(&payload).expect("json");
        assert_eq!(output["response"]["output"][0]["id"], "fc-1");
        assert_eq!(output["response"]["output"][0]["name"], "shell");
    }

    #[test]
    fn sse_framer_emits_complete_pending_frame_without_delimiter() {
        let mut framer = sse::ResponsesSseFramer::default();

        let out = framer.push_chunk(Bytes::from_static(
            b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-1\"}}",
        ));
        assert_eq!(
            String::from_utf8(out[0].to_vec()).expect("valid utf8"),
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-1\"}}\n\n"
        );
        assert!(framer.finish().is_empty());
    }

    #[derive(Debug, Deserialize)]
    struct SseParityFixture {
        name: String,
        chunks: Vec<String>,
        expected_frames: Vec<String>,
        expected_event_types: Vec<String>,
    }

    #[test]
    fn sse_framer_matches_go_parity_fixtures() {
        for fixture in load_sse_parity_fixtures() {
            let mut framer = sse::ResponsesSseFramer::default();
            let mut actual_frames = Vec::new();
            for chunk in fixture.chunks {
                actual_frames.extend(
                    framer
                        .push_chunk(Bytes::from(chunk))
                        .into_iter()
                        .map(|frame| String::from_utf8(frame.to_vec()).expect("valid utf8")),
                );
            }
            actual_frames.extend(
                framer
                    .finish()
                    .into_iter()
                    .map(|frame| String::from_utf8(frame.to_vec()).expect("valid utf8")),
            );

            assert_sse_frames_match(&fixture.name, &actual_frames, &fixture.expected_frames);

            let actual_event_types = actual_frames
                .iter()
                .filter_map(|frame| {
                    let payload = sse::sse_data_payload(frame.as_bytes())?;
                    let value = serde_json::from_slice::<Value>(&payload).ok()?;
                    value
                        .get("type")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>();
            assert_eq!(
                actual_event_types, fixture.expected_event_types,
                "fixture {} produced unexpected event sequence",
                fixture.name
            );
        }
    }

    fn load_sse_parity_fixtures() -> Vec<SseParityFixture> {
        [
            include_str!("responses/fixtures/sse_parity/separates_data_only_chunks.json"),
            include_str!(
                "responses/fixtures/sse_parity/repairs_empty_completed_output_from_done_items.json"
            ),
            include_str!(
                "responses/fixtures/sse_parity/repairs_mixed_indexed_and_unindexed_done_items.json"
            ),
            include_str!("responses/fixtures/sse_parity/repairs_multiline_completed_payload.json"),
            include_str!("responses/fixtures/sse_parity/reassembles_split_sse_event_chunks.json"),
            include_str!("responses/fixtures/sse_parity/preserves_valid_full_sse_event_chunk.json"),
            include_str!("responses/fixtures/sse_parity/buffers_split_data_payload_chunks.json"),
            include_str!(
                "responses/fixtures/sse_parity/tolerates_blank_line_between_event_and_data.json"
            ),
            include_str!(
                "responses/fixtures/sse_parity/drops_incomplete_trailing_data_chunk_on_flush.json"
            ),
        ]
        .into_iter()
        .map(|raw| serde_json::from_str(raw).expect("parse sse parity fixture"))
        .collect()
    }

    fn assert_sse_frames_match(name: &str, actual: &[String], expected: &[String]) {
        assert_eq!(
            actual.len(),
            expected.len(),
            "fixture {name} produced unexpected frame count"
        );
        for (index, (actual_frame, expected_frame)) in actual.iter().zip(expected).enumerate() {
            let actual_event = sse_event_name(actual_frame);
            let expected_event = sse_event_name(expected_frame);
            assert_eq!(
                actual_event, expected_event,
                "fixture {name} frame {index} produced unexpected event header"
            );

            let actual_payload = sse::sse_data_payload(actual_frame.as_bytes());
            let expected_payload = sse::sse_data_payload(expected_frame.as_bytes());
            match (actual_payload, expected_payload) {
                (Some(actual_payload), Some(expected_payload)) => {
                    if actual_payload == b"[DONE]" || expected_payload == b"[DONE]" {
                        assert_eq!(
                            actual_payload, expected_payload,
                            "fixture {name} frame {index} produced unexpected raw payload"
                        );
                    } else {
                        let actual_value =
                            serde_json::from_slice::<Value>(&actual_payload).expect("actual json");
                        let expected_value = serde_json::from_slice::<Value>(&expected_payload)
                            .expect("expected json");
                        assert_eq!(
                            actual_value, expected_value,
                            "fixture {name} frame {index} produced unexpected payload"
                        );
                    }
                }
                (None, None) => assert_eq!(
                    actual_frame, expected_frame,
                    "fixture {name} frame {index} produced unexpected raw frame"
                ),
                _ => panic!("fixture {name} frame {index} mismatched data payload presence"),
            }
        }
    }

    fn sse_event_name(frame: &str) -> Option<&str> {
        frame.lines().find_map(|line| {
            line.trim()
                .strip_prefix("event:")
                .map(str::trim_start)
                .filter(|value| !value.is_empty())
        })
    }

    fn normalize_test_sse_frame(event: &str, payload: Value) -> Bytes {
        let payload = serde_json::to_string(&payload).expect("serialize payload");
        let mut frame = String::new();
        if !event.trim().is_empty() {
            frame.push_str("event: ");
            frame.push_str(event.trim());
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
}
