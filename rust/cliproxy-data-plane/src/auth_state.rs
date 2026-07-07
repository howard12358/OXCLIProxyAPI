use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use cliproxy_common_types::snapshot::AuthRecord;
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
    auth: HashMap<AuthKey, RuntimeBlockState>,
    model: HashMap<ModelKey, RuntimeBlockState>,
}

/// Rust 本地 overlay 里 auth 维度的稳定键。
///
/// 当前优先使用 snapshot 导出的 `auth_index`，只有其为空时才回退到 `auth.id`，
/// 这样既保留与 Go 当前快照契约的一致性，也避免在各调用点重复写 fallback 规则。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthKey(String);

impl AuthKey {
    pub fn new(raw: impl AsRef<str>) -> Option<Self> {
        let value = raw.as_ref().trim();
        (!value.is_empty()).then(|| Self(value.to_string()))
    }

    pub fn from_auth_record(auth: &AuthRecord) -> Self {
        Self::new(auth.auth_index.trim())
            .or_else(|| Self::new(auth.id.trim()))
            .expect("auth record must have auth_index or id")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Rust 本地 overlay 里 model 维度的稳定键。
///
/// model 级冷却当前仍然绑定在单个 auth 作用域内，不跨 auth 共享。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelKey {
    auth: AuthKey,
    model: String,
}

impl ModelKey {
    pub fn new(auth: AuthKey, model: impl AsRef<str>) -> Option<Self> {
        let model = model.as_ref().trim();
        (!model.is_empty()).then(|| Self {
            auth,
            model: model.to_string(),
        })
    }

    pub fn auth(&self) -> &AuthKey {
        &self.auth
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

/// overlay 中的阻塞状态显式建模。
///
/// absence 仍然表示 healthy；当前只持久化“正在冷却”的阻塞态，
/// 这样既把状态语义从字段袋子里拉出来，又不改变现有内存占用和清理策略。
#[derive(Debug, Clone)]
pub enum RuntimeBlockState {
    CoolingDown(RuntimeCooldown),
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeCooldown {
    pub unavailable: bool,
    pub status_message: String,
    pub last_error_code: String,
    pub last_error_message: String,
    pub next_retry_after: Option<OffsetDateTime>,
    pub quota_exceeded: bool,
    pub quota_reason: String,
    pub updated_at: Option<OffsetDateTime>,
}

impl RuntimeBlockState {
    pub fn cooling_down(cooldown: RuntimeCooldown) -> Self {
        Self::CoolingDown(cooldown)
    }

    pub fn is_active_at(&self, now: OffsetDateTime) -> bool {
        match self {
            Self::CoolingDown(cooldown) => {
                cooldown.unavailable
                    && cooldown
                        .next_retry_after
                        .map(|deadline| deadline > now)
                        .unwrap_or(false)
            }
        }
    }

    pub fn cooldown_until_rfc3339(&self) -> String {
        match self {
            Self::CoolingDown(cooldown) => cooldown
                .next_retry_after
                .and_then(|value| value.format(&Rfc3339).ok())
                .unwrap_or_default(),
        }
    }

    pub fn next_retry_after(&self) -> Option<OffsetDateTime> {
        match self {
            Self::CoolingDown(cooldown) => cooldown.next_retry_after,
        }
    }
}

impl AuthStateOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_auth_failure(&self, auth_key: AuthKey, state: RuntimeBlockState) {
        let mut inner = self.inner.lock().expect("auth state lock poisoned");
        inner.auth.insert(auth_key, state);
    }

    pub fn set_model_failure(&self, model_key: ModelKey, state: RuntimeBlockState) {
        let mut inner = self.inner.lock().expect("auth state lock poisoned");
        inner.model.insert(model_key, state);
    }

    pub fn clear_success(&self, model_key: &ModelKey) {
        let mut inner = self.inner.lock().expect("auth state lock poisoned");
        inner.auth.remove(model_key.auth());
        inner.model.remove(model_key);
    }

    pub fn auth_blocked_until(
        &self,
        auth_key: &AuthKey,
        now: OffsetDateTime,
    ) -> Option<OffsetDateTime> {
        self.prune_expired(now);
        let inner = self.inner.lock().expect("auth state lock poisoned");
        inner
            .auth
            .get(auth_key)
            .and_then(|state| state.is_active_at(now).then_some(state.next_retry_after()))
            .flatten()
    }

    pub fn model_blocked_until(
        &self,
        model_key: &ModelKey,
        now: OffsetDateTime,
    ) -> Option<OffsetDateTime> {
        self.prune_expired(now);
        let inner = self.inner.lock().expect("auth state lock poisoned");
        inner
            .model
            .get(model_key)
            .and_then(|state| state.is_active_at(now).then_some(state.next_retry_after()))
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
            AuthKey::new("auth-1").expect("auth key"),
            RuntimeBlockState::cooling_down(RuntimeCooldown {
                unavailable: true,
                next_retry_after: Some(now - Duration::seconds(1)),
                ..RuntimeCooldown::default()
            }),
        );

        assert!(
            overlay
                .auth_blocked_until(&AuthKey::new("auth-1").expect("auth key"), now)
                .is_none()
        );
    }

    #[test]
    fn auth_key_prefers_snapshot_auth_index() {
        let auth = AuthRecord {
            id: "auth-id".to_string(),
            auth_index: "auth-index".to_string(),
            ..AuthRecord::default()
        };

        assert_eq!(AuthKey::from_auth_record(&auth).as_str(), "auth-index");
    }
}
