use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Rust 本地运行时 auth/model 健康 overlay。
///
/// 这层状态只存在于当前数据平面进程内，不修改 snapshot，也不做持久化。
#[derive(Clone, Default)]
pub struct AuthStateOverlay {
    inner: Arc<Mutex<AuthStateInner>>,
}

#[derive(Default)]
struct AuthStateInner {
    auth: HashMap<String, RuntimeFailureState>,
    model: HashMap<(String, String), RuntimeFailureState>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeFailureState {
    pub unavailable: bool,
    pub status_message: String,
    pub last_error_code: String,
    pub last_error_message: String,
    pub next_retry_after: Option<OffsetDateTime>,
    pub quota_exceeded: bool,
    pub quota_reason: String,
    pub updated_at: Option<OffsetDateTime>,
}

impl RuntimeFailureState {
    pub fn is_active_at(&self, now: OffsetDateTime) -> bool {
        self.unavailable
            && self
                .next_retry_after
                .map(|deadline| deadline > now)
                .unwrap_or(false)
    }

    pub fn cooldown_until_rfc3339(&self) -> String {
        self.next_retry_after
            .and_then(|value| value.format(&Rfc3339).ok())
            .unwrap_or_default()
    }
}

impl AuthStateOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_auth_failure(&self, auth_index: &str, state: RuntimeFailureState) {
        let auth_index = auth_index.trim();
        if auth_index.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().expect("auth state lock poisoned");
        inner.auth.insert(auth_index.to_string(), state);
    }

    pub fn set_model_failure(&self, auth_index: &str, model: &str, state: RuntimeFailureState) {
        let auth_index = auth_index.trim();
        let model = model.trim();
        if auth_index.is_empty() || model.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().expect("auth state lock poisoned");
        inner
            .model
            .insert((auth_index.to_string(), model.to_string()), state);
    }

    pub fn clear_success(&self, auth_index: &str, model: &str) {
        let auth_index = auth_index.trim();
        let model = model.trim();
        if auth_index.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().expect("auth state lock poisoned");
        inner.auth.remove(auth_index);
        if !model.is_empty() {
            inner
                .model
                .remove(&(auth_index.to_string(), model.to_string()));
        }
    }

    pub fn auth_blocked_until(
        &self,
        auth_index: &str,
        now: OffsetDateTime,
    ) -> Option<OffsetDateTime> {
        self.prune_expired(now);
        let inner = self.inner.lock().expect("auth state lock poisoned");
        inner
            .auth
            .get(auth_index.trim())
            .and_then(|state| state.is_active_at(now).then_some(state.next_retry_after))
            .flatten()
    }

    pub fn model_blocked_until(
        &self,
        auth_index: &str,
        model: &str,
        now: OffsetDateTime,
    ) -> Option<OffsetDateTime> {
        self.prune_expired(now);
        let inner = self.inner.lock().expect("auth state lock poisoned");
        inner
            .model
            .get(&(auth_index.trim().to_string(), model.trim().to_string()))
            .and_then(|state| state.is_active_at(now).then_some(state.next_retry_after))
            .flatten()
    }

    fn prune_expired(&self, now: OffsetDateTime) {
        let mut inner = self.inner.lock().expect("auth state lock poisoned");
        inner.auth.retain(|_, state| state.is_active_at(now));
        inner.model.retain(|_, state| state.is_active_at(now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    #[test]
    fn clears_expired_entries_on_lookup() {
        let overlay = AuthStateOverlay::new();
        let now = OffsetDateTime::now_utc();
        overlay.set_auth_failure(
            "auth-1",
            RuntimeFailureState {
                unavailable: true,
                next_retry_after: Some(now - Duration::seconds(1)),
                ..RuntimeFailureState::default()
            },
        );

        assert!(overlay.auth_blocked_until("auth-1", now).is_none());
    }
}
