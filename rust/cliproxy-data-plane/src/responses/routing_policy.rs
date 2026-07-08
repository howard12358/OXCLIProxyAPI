use cliproxy_common_types::{
    routing::ExecutionPlan,
    snapshot::{AuthRecord, RuntimeSnapshot},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::auth_state::{AuthKey, AuthStateOverlay, ModelKey};

/// 基于 snapshot cooldown 和 Rust 本地 overlay，求出当前请求真正可执行的 auth 链。
///
/// router-core 仍然负责“理想计划”，这里负责“在当前瞬时健康状态下还能不能执行”。
pub(super) fn resolve_effective_plan<'a>(
    snapshot: &'a RuntimeSnapshot,
    execution_plan: &ExecutionPlan,
    auth_state: &AuthStateOverlay,
) -> Option<(ExecutionPlan, Option<&'a AuthRecord>)> {
    let now = OffsetDateTime::now_utc();
    let chain = auth_retry_chain(snapshot, execution_plan)?;
    let mut available = Vec::new();
    for auth in chain {
        if auth_blocked(auth, &execution_plan.model, auth_state, now) {
            continue;
        }
        available.push(auth);
    }
    let selected = *available.first()?;
    let retry_candidates = available
        .iter()
        .skip(1)
        .map(|auth| auth.id.clone())
        .collect::<Vec<_>>();
    Some((
        ExecutionPlan {
            provider: execution_plan.provider,
            model: execution_plan.model.clone(),
            auth_id: selected.id.clone(),
            retry_candidates,
            stickiness_source: execution_plan.stickiness_source.clone(),
        },
        Some(selected),
    ))
}

pub(super) fn auth_retry_chain<'a>(
    snapshot: &'a RuntimeSnapshot,
    execution_plan: &ExecutionPlan,
) -> Option<Vec<&'a AuthRecord>> {
    let mut chain = Vec::new();
    chain.push(
        snapshot
            .auth_pool
            .iter()
            .find(|auth| auth.id == execution_plan.auth_id)?,
    );
    for auth_id in &execution_plan.retry_candidates {
        if let Some(auth) = snapshot.auth_pool.iter().find(|auth| auth.id == *auth_id) {
            chain.push(auth);
        }
    }
    Some(chain)
}

pub(super) fn snapshot_cooldown_active(auth: &AuthRecord, now: OffsetDateTime) -> bool {
    auth.cooldown_until
        .as_deref()
        .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
        .map(|deadline| deadline > now)
        .unwrap_or(false)
}

fn auth_blocked(
    auth: &AuthRecord,
    model: &str,
    auth_state: &AuthStateOverlay,
    now: OffsetDateTime,
) -> bool {
    if snapshot_cooldown_active(auth, now) {
        return true;
    }
    let Some(auth_key) = AuthKey::from_auth_record(auth) else {
        return false;
    };
    let Some(model_key) = ModelKey::new(auth_key.clone(), model) else {
        return auth_state.auth_blocked_until(&auth_key, now).is_some();
    };
    auth_state.auth_blocked_until(&auth_key, now).is_some()
        || auth_state.model_blocked_until(&model_key, now).is_some()
}
