use axum::{
    Json,
    body::Body,
    http::{HeaderValue, Response, StatusCode, header},
    response::IntoResponse,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod handler;
mod mock;
mod sse;
mod upstream;

pub use handler::handle_responses;

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
pub(super) struct MockCompletedResponse {
    id: String,
    object: &'static str,
    model: String,
    status: &'static str,
    output: Vec<Value>,
    usage: Value,
}

#[derive(Debug, Clone)]
pub(super) struct RequestMetadata {
    model: String,
    prompt_preview: String,
    metadata_keys: usize,
}

#[derive(Debug, Clone)]
pub(super) struct MockSseEvent {
    event: String,
    payload: Value,
}

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
    use cliproxy_common_types::routing::{ExecutionPlan, StickinessSource};
    use serde_json::json;

    #[test]
    fn normalize_frame_has_event_and_data_lines() {
        let frame = mock::normalize_sse_frame(&MockSseEvent {
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
    }

    #[test]
    fn should_retry_auth_bound_error_rejects_generic_429() {
        let err = anyhow::anyhow!("upstream codex error 429: too many requests");
        assert!(!upstream::should_retry_auth_bound_error(&err));
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
}
