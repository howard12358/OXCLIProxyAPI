use cliproxy_common_types::snapshot::AuthRecord;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    auth_state::{AuthKey, AuthStateOverlay, ModelKey, RuntimeBlockState, RuntimeCooldown},
    error_events::ErrorEvent,
    usage_queue::UsageQueue,
};

use super::failure::ClassifiedUpstreamFailure;

#[derive(Clone)]
pub(super) struct HealthRecorder {
    auth_state: AuthStateOverlay,
    usage_queue: UsageQueue,
}

impl HealthRecorder {
    pub(super) fn new(auth_state: AuthStateOverlay, usage_queue: UsageQueue) -> Self {
        Self {
            auth_state,
            usage_queue,
        }
    }

    /// 成功请求会清空该 auth 在当前 model 上的阻塞态。
    ///
    /// 这里保留与上一版一致的粗粒度语义：一次成功会同时清除 auth 级和该 model 级冷却，
    /// 不区分“部分恢复”与“完全恢复”。
    pub(super) fn record_success(&self, auth: &AuthRecord, model: &str) {
        let Some(auth_key) = AuthKey::from_auth_record(auth) else {
            return;
        };
        let Some(model_key) = ModelKey::new(auth_key, model) else {
            return;
        };
        self.auth_state.clear_success(&model_key);
    }

    pub(super) fn record_failure(&self, failure: RecordedFailure) {
        if let Some(cooldown_until) = failure.cooldown_until {
            let state = RuntimeBlockState::cooling_down(RuntimeCooldown {
                unavailable: true,
                status_message: failure.classification.kind.reason().to_string(),
                last_error_code: failure.classification.error_code.clone(),
                last_error_message: failure.classification.message.clone(),
                next_retry_after: Some(cooldown_until),
                quota_exceeded: failure.classification.kind.quota_exceeded(),
                quota_reason: if failure.classification.kind.quota_exceeded() {
                    failure.classification.kind.reason().to_string()
                } else {
                    String::new()
                },
                updated_at: Some(OffsetDateTime::now_utc()),
            });
            match failure.classification.kind.scope() {
                crate::error_events::ErrorScope::Auth => self
                    .auth_state
                    .set_auth_failure(failure.auth_key.clone(), state),
                crate::error_events::ErrorScope::Model => self
                    .auth_state
                    .set_model_failure(failure.model_key.clone(), state),
            }
        }

        self.usage_queue.enqueue_error(ErrorEvent {
            timestamp: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
            request_id: failure.request_id,
            provider: "codex".to_string(),
            model: failure.model_key.model().to_string(),
            auth_index: failure.auth_key.as_str().to_string(),
            scope: failure.classification.kind.scope(),
            status_code: failure.classification.status_code,
            error_code: failure.classification.error_code,
            message: failure.classification.message,
            retry_after_ms: failure.classification.retry_after_ms,
            cooldown_until: failure
                .cooldown_until
                .and_then(|value| value.format(&Rfc3339).ok())
                .unwrap_or_default(),
            quota_exceeded: failure.classification.kind.quota_exceeded(),
            reason: failure.classification.kind.reason().to_string(),
        });
    }
}

pub(super) struct RecordedFailure {
    auth_key: AuthKey,
    model_key: ModelKey,
    request_id: String,
    classification: ClassifiedUpstreamFailure,
    cooldown_until: Option<OffsetDateTime>,
}

impl RecordedFailure {
    pub(super) fn new(
        auth: &AuthRecord,
        model: &str,
        request_id: String,
        classification: ClassifiedUpstreamFailure,
    ) -> Option<Self> {
        let auth_key = AuthKey::from_auth_record(auth)?;
        let model_key = ModelKey::new(auth_key.clone(), model)?;
        let cooldown_until = classification
            .kind
            .cooldown_until(OffsetDateTime::now_utc(), classification.retry_after_ms);
        Some(Self {
            auth_key,
            model_key,
            request_id,
            classification,
            cooldown_until,
        })
    }
}
