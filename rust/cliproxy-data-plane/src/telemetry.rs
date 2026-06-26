use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

use cliproxy_common_types::{
    routing::ExecutionPlan,
    snapshot::{AuthRecord, RuntimeSnapshot},
    upstream::ProviderKind,
};
use cliproxy_usage_events::{
    UsageEventProducer, UsageQueueFail, UsageQueuePayload, UsageQueueTokens,
};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const RESPONSES_ENDPOINT: &str = "POST /v1/responses";
const RUST_RESPONSES_EXECUTOR: &str = "RustResponsesExecutor";
const DEFAULT_SERVICE_TIER: &str = "default";

#[derive(Clone)]
pub struct AppTelemetry {
    usage_events: UsageEventProducer,
}

impl AppTelemetry {
    pub fn new() -> Self {
        Self {
            usage_events: UsageEventProducer::new_log(256),
        }
    }

    pub fn new_request(
        &self,
        request_model: &str,
        _stream: bool,
        snapshot: Option<&RuntimeSnapshot>,
        request_id: Option<String>,
    ) -> RequestTelemetry {
        let usage_queue = snapshot.map(|value| &value.usage_queue);
        RequestTelemetry {
            app: self.clone(),
            started_at: Instant::now(),
            request_model: request_model.to_string(),
            usage_queue_enabled: usage_queue
                .map(|value| value.enabled && value.backend.trim().eq_ignore_ascii_case("redis"))
                .unwrap_or(false),
            state: Arc::new(RequestTelemetryState {
                request_id: Mutex::new(request_id.unwrap_or_default()),
                ..RequestTelemetryState::default()
            }),
        }
    }
}

#[cfg(test)]
impl AppTelemetry {
    fn new_test(buffer: usize) -> (Self, tokio::sync::mpsc::Receiver<UsageQueuePayload>) {
        let (usage_events, rx) = UsageEventProducer::new_channel(buffer);
        (Self { usage_events }, rx)
    }
}

#[derive(Default)]
struct RequestTelemetryState {
    provider: Mutex<String>,
    resolved_model: Mutex<String>,
    auth_id: Mutex<String>,
    auth_type: Mutex<String>,
    request_id: Mutex<String>,
    response_id: Mutex<String>,
    response_headers: Mutex<BTreeMap<String, Vec<String>>>,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    reasoning_tokens: AtomicU64,
    cached_tokens: AtomicU64,
    cache_read_tokens: AtomicU64,
    cache_creation_tokens: AtomicU64,
    total_tokens: AtomicU64,
    first_byte_ms: AtomicU64,
    first_byte_recorded: AtomicBool,
    finished: AtomicBool,
}

#[derive(Clone)]
pub struct RequestTelemetry {
    app: AppTelemetry,
    started_at: Instant,
    request_model: String,
    usage_queue_enabled: bool,
    state: Arc<RequestTelemetryState>,
}

impl RequestTelemetry {
    pub fn bind_execution_plan(&self, plan: &ExecutionPlan, selected_auth: Option<&AuthRecord>) {
        self.set_provider(provider_name(plan.provider));
        self.set_resolved_model(plan.model.clone());
        self.set_auth_id(plan.auth_id.clone());
        if let Some(auth) = selected_auth {
            self.set_auth_type(auth.auth_kind.clone());
        }
    }

    pub fn mark_first_byte(&self) {
        let _ = self.state.first_byte_recorded.compare_exchange(
            false,
            true,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        if self.state.first_byte_recorded.load(Ordering::SeqCst) {
            let elapsed = self.started_at.elapsed().as_millis() as u64;
            self.state.first_byte_ms.store(elapsed, Ordering::SeqCst);
        }
    }

    pub fn observe_response_json_bytes(&self, bytes: &[u8]) {
        if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
            self.observe_response_json_value(&value);
        }
    }

    pub fn record_auth_failure(&self, _reason: &str) {}

    pub fn record_auth_retry(&self, _reason: &str) {}

    pub fn observe_response_json_value(&self, value: &Value) {
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            self.set_response_id(id.to_string());
        }
        self.observe_usage_value(value.get("usage"));
    }

    pub fn observe_response_headers(&self, headers: &BTreeMap<String, String>) {
        let normalized = headers
            .iter()
            .map(|(key, value)| (key.clone(), vec![value.clone()]))
            .collect::<BTreeMap<_, _>>();
        *self
            .state
            .response_headers
            .lock()
            .expect("response_headers lock poisoned") = normalized;
    }

    pub fn observe_sse_frame(&self, frame: &[u8]) {
        let Some(payload) = crate::responses::sse::sse_data_payload(frame) else {
            return;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&payload) else {
            return;
        };
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.usage" => self.observe_usage_value(value.get("usage")),
            "response.completed" => {
                if let Some(response) = value.get("response") {
                    self.observe_response_json_value(response);
                }
            }
            _ => {}
        }
    }

    pub fn finish_success(&self) {
        self.finish(false, 200, "");
    }

    pub fn finish_error(&self, status_code: u16, body: &str) {
        self.finish(true, status_code, body);
    }

    pub fn finish_cancelled(&self) {
        if self
            .state
            .finished
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
    }

    fn finish(&self, failed: bool, status_code: u16, body: &str) {
        if self
            .state
            .finished
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        if !self.usage_queue_enabled {
            return;
        }

        let payload = UsageQueuePayload {
            timestamp: now_rfc3339(),
            latency_ms: self.started_at.elapsed().as_millis() as u64,
            ttft_ms: self.state.first_byte_ms.load(Ordering::SeqCst),
            source: String::new(),
            auth_index: self.auth_id(),
            tokens: UsageQueueTokens {
                input_tokens: self.state.input_tokens.load(Ordering::SeqCst),
                output_tokens: self.state.output_tokens.load(Ordering::SeqCst),
                reasoning_tokens: self.state.reasoning_tokens.load(Ordering::SeqCst),
                cached_tokens: self.state.cached_tokens.load(Ordering::SeqCst),
                cache_read_tokens: self.state.cache_read_tokens.load(Ordering::SeqCst),
                cache_creation_tokens: self.state.cache_creation_tokens.load(Ordering::SeqCst),
                total_tokens: self.state.total_tokens.load(Ordering::SeqCst),
            },
            failed,
            fail: UsageQueueFail {
                status_code: if failed { status_code.max(400) } else { 200 },
                body: if failed {
                    body.trim().to_string()
                } else {
                    String::new()
                },
            },
            response_headers: self.response_headers(),
            provider: self.provider(),
            executor_type: RUST_RESPONSES_EXECUTOR.to_string(),
            model: self.resolved_model(),
            alias: self.request_model.clone(),
            endpoint: RESPONSES_ENDPOINT.to_string(),
            auth_type: self.auth_type(),
            api_key: String::new(),
            request_id: self.effective_request_id(),
            reasoning_effort: String::new(),
            service_tier: DEFAULT_SERVICE_TIER.to_string(),
        };
        let _ = self.app.usage_events.try_emit(payload);
    }

    fn observe_usage_value(&self, usage: Option<&Value>) {
        let Some(usage) = usage else {
            return;
        };
        let input_tokens = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let output_tokens = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let reasoning_tokens = usage
            .get("reasoning_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let cached_tokens = usage
            .get("cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let cache_read_tokens = usage
            .get("cache_read_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let cache_creation_tokens = usage
            .get("cache_creation_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let total_tokens = usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input_tokens.saturating_add(output_tokens));
        self.state
            .input_tokens
            .store(input_tokens, Ordering::SeqCst);
        self.state
            .output_tokens
            .store(output_tokens, Ordering::SeqCst);
        self.state
            .reasoning_tokens
            .store(reasoning_tokens, Ordering::SeqCst);
        self.state
            .cached_tokens
            .store(cached_tokens, Ordering::SeqCst);
        self.state
            .cache_read_tokens
            .store(cache_read_tokens, Ordering::SeqCst);
        self.state
            .cache_creation_tokens
            .store(cache_creation_tokens, Ordering::SeqCst);
        self.state
            .total_tokens
            .store(total_tokens, Ordering::SeqCst);
    }

    fn set_provider(&self, value: &str) {
        *self.state.provider.lock().expect("provider lock poisoned") = value.to_string();
    }

    fn set_resolved_model(&self, value: String) {
        *self
            .state
            .resolved_model
            .lock()
            .expect("resolved_model lock poisoned") = value;
    }

    fn set_auth_id(&self, value: String) {
        *self.state.auth_id.lock().expect("auth_id lock poisoned") = value;
    }

    fn set_auth_type(&self, value: String) {
        *self
            .state
            .auth_type
            .lock()
            .expect("auth_type lock poisoned") = value;
    }

    fn set_response_id(&self, value: String) {
        *self
            .state
            .response_id
            .lock()
            .expect("response_id lock poisoned") = value;
    }

    fn provider(&self) -> String {
        let provider = self
            .state
            .provider
            .lock()
            .expect("provider lock poisoned")
            .clone();
        if provider.is_empty() {
            "unknown".to_string()
        } else {
            provider
        }
    }

    fn resolved_model(&self) -> String {
        let model = self
            .state
            .resolved_model
            .lock()
            .expect("resolved_model lock poisoned")
            .clone();
        if model.is_empty() {
            self.request_model.clone()
        } else {
            model
        }
    }

    fn auth_id(&self) -> String {
        self.state
            .auth_id
            .lock()
            .expect("auth_id lock poisoned")
            .clone()
    }

    fn auth_type(&self) -> String {
        let auth_type = self
            .state
            .auth_type
            .lock()
            .expect("auth_type lock poisoned")
            .clone();
        if auth_type.is_empty() {
            "unknown".to_string()
        } else {
            auth_type
        }
    }

    fn effective_request_id(&self) -> String {
        let request_id = self
            .state
            .request_id
            .lock()
            .expect("request_id lock poisoned")
            .clone();
        if !request_id.is_empty() {
            return request_id;
        }
        self.state
            .response_id
            .lock()
            .expect("response_id lock poisoned")
            .clone()
    }

    fn response_headers(&self) -> BTreeMap<String, Vec<String>> {
        self.state
            .response_headers
            .lock()
            .expect("response_headers lock poisoned")
            .clone()
    }
}

pub struct StreamCompletionGuard {
    telemetry: RequestTelemetry,
    completed: bool,
}

impl StreamCompletionGuard {
    pub fn new(telemetry: RequestTelemetry) -> Self {
        Self {
            telemetry,
            completed: false,
        }
    }

    pub fn finish_success(&mut self) {
        self.telemetry.finish_success();
        self.completed = true;
    }

    pub fn finish_error(&mut self) {
        self.telemetry.finish_error(502, "upstream stream error");
        self.completed = true;
    }
}

impl Drop for StreamCompletionGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.telemetry.finish_cancelled();
        }
    }
}

pub fn provider_name(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Codex => "codex",
        ProviderKind::Mock => "mock",
    }
}

pub fn classify_auth_failure_reason(err: &anyhow::Error) -> &'static str {
    let message = err.to_string().to_ascii_lowercase();
    if message.contains("upstream codex error 401") || message.contains("invalid_api_key") {
        "auth_401"
    } else if message.contains("upstream codex error 403")
        || message.contains("account_deactivated")
    {
        "auth_403"
    } else if message.contains("usage_limit_reached")
        || message.contains("the usage limit has been reached")
    {
        "usage_limit_reached"
    } else if message.contains("upstream codex error 429") {
        "auth_429"
    } else {
        "other"
    }
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cliproxy_common_types::{
        routing::{ExecutionPlan, StickinessSource},
        snapshot::{AuthRecord, RuntimeSnapshot, UsageQueueConfig},
        upstream::ProviderKind,
    };
    use serde_json::json;

    use super::{AppTelemetry, RequestTelemetry};

    fn test_snapshot(enabled: bool, backend: &str) -> RuntimeSnapshot {
        RuntimeSnapshot {
            usage_queue: UsageQueueConfig {
                enabled,
                backend: backend.to_string(),
            },
            ..RuntimeSnapshot::default()
        }
    }

    fn test_plan() -> ExecutionPlan {
        ExecutionPlan {
            provider: ProviderKind::Codex,
            model: "gpt-5-codex".to_string(),
            auth_id: "auth-codex-1".to_string(),
            retry_candidates: Vec::new(),
            stickiness_source: StickinessSource::Strategy,
        }
    }

    fn test_auth() -> AuthRecord {
        AuthRecord {
            id: "auth-codex-1".to_string(),
            auth_kind: "oauth".to_string(),
            ..AuthRecord::default()
        }
    }

    fn observe_success(telemetry: &RequestTelemetry) {
        telemetry.observe_response_headers(&BTreeMap::from([(
            "x-upstream-request-id".to_string(),
            "upstream-1".to_string(),
        )]));
        telemetry.observe_response_json_value(&json!({
            "id": "resp_1",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20,
                "total_tokens": 30
            }
        }));
        telemetry.mark_first_byte();
        telemetry.finish_success();
    }

    #[tokio::test]
    async fn emits_cpa_shaped_usage_queue_payload_for_successful_request() {
        let (app, mut rx) = AppTelemetry::new_test(4);
        let snapshot = test_snapshot(true, "redis");
        let request = app.new_request("codex-latest", false, Some(&snapshot), Some("req-1".into()));
        request.bind_execution_plan(&test_plan(), Some(&test_auth()));

        observe_success(&request);

        let payload = rx.recv().await.expect("usage payload");
        assert_eq!(payload.provider, "codex");
        assert_eq!(payload.executor_type, "RustResponsesExecutor");
        assert_eq!(payload.model, "gpt-5-codex");
        assert_eq!(payload.alias, "codex-latest");
        assert_eq!(payload.endpoint, "POST /v1/responses");
        assert_eq!(payload.auth_type, "oauth");
        assert_eq!(payload.auth_index, "auth-codex-1");
        assert_eq!(payload.request_id, "req-1");
        assert_eq!(payload.tokens.total_tokens, 30);
        assert_eq!(payload.fail.status_code, 200);
        assert!(!payload.failed);
        assert_eq!(
            payload.response_headers.get("x-upstream-request-id"),
            Some(&vec!["upstream-1".to_string()])
        );
    }

    #[tokio::test]
    async fn does_not_emit_payload_when_usage_queue_backend_is_not_redis() {
        let (app, mut rx) = AppTelemetry::new_test(4);
        let snapshot = test_snapshot(true, "log");
        let request = app.new_request("codex-latest", false, Some(&snapshot), None);
        request.bind_execution_plan(&test_plan(), Some(&test_auth()));

        observe_success(&request);

        assert!(rx.try_recv().is_err());
    }
}
