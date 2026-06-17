use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use cliproxy_common_types::{
    routing::{ExecutionPlan, StickinessSource},
    snapshot::{AuthRecord, RoutingStrategy, RuntimeSnapshot},
    upstream::ProviderKind,
};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRequest {
    pub requested_model: String,
    pub session_id: Option<String>,
    pub pinned_auth_id: Option<String>,
}

#[derive(Clone, Default)]
pub struct RouterCore {
    state: Arc<Mutex<RouterState>>,
}

#[derive(Default)]
struct RouterState {
    affinity: HashMap<String, AffinityEntry>,
    round_robin_offsets: HashMap<String, usize>,
    had_expired_or_invalid_affinity: bool,
}

struct AffinityEntry {
    auth_id: String,
    expires_at: Instant,
}

impl RouterCore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plan(&self, snapshot: &RuntimeSnapshot, request: PlanRequest) -> Result<ExecutionPlan> {
        if !snapshot.routes.responses {
            bail!("responses route is disabled by runtime snapshot");
        }

        let provider = ProviderKind::Codex;
        let provider_key = provider_key(provider);
        if !provider_enabled(snapshot, provider_key) {
            bail!("provider {provider_key} is disabled");
        }

        let model = resolve_model(snapshot, provider_key, &request.requested_model)?;
        let available = available_auths(snapshot, provider_key, &model);
        if available.is_empty() {
            bail!("no available auth for provider {provider_key} and model {model}");
        }

        if let Some(pinned_auth_id) = trim_to_owned(request.pinned_auth_id.as_deref()) {
            let chosen = available
                .iter()
                .find(|auth| auth.id == pinned_auth_id)
                .ok_or_else(|| anyhow!("pinned auth {pinned_auth_id} is unavailable"))?;
            return Ok(build_plan(
                provider,
                &model,
                chosen.id.clone(),
                retries_for_available(&available, &chosen.id, None),
                StickinessSource::PinnedAuth,
            ));
        }

        let session_key = if snapshot.routing.session_affinity {
            trim_to_owned(request.session_id.as_deref())
                .map(|session_id| format!("{provider_key}::{session_id}::{model}"))
        } else {
            None
        };

        let highest_priority = available
            .first()
            .map(|auth| auth.priority)
            .ok_or_else(|| anyhow!("no available auth after filtering"))?;
        let highest: Vec<&AuthRecord> = available
            .iter()
            .copied()
            .filter(|auth| auth.priority == highest_priority)
            .collect();

        if let Some(session_key) = session_key.as_deref() {
            let mut state = self.state.lock().expect("router state lock poisoned");
            if let Some(auth_id) = state.get_affinity(session_key) {
                if available.iter().any(|auth| auth.id == auth_id) {
                    return Ok(build_plan(
                        provider,
                        &model,
                        auth_id.clone(),
                        retries_for_available(&available, &auth_id, Some(&highest)),
                        StickinessSource::SessionAffinity,
                    ));
                }
                state.had_expired_or_invalid_affinity = true;
            }

            let chosen = select_by_strategy(&mut state, snapshot, provider_key, &model, &highest)?
                .id
                .clone();
            state.set_affinity(
                session_key.to_string(),
                chosen.clone(),
                Duration::from_secs(snapshot.routing.session_ttl_seconds.max(1)),
            );
            let source = if state.had_expired_or_invalid_affinity {
                StickinessSource::ReboundSessionAffinity
            } else {
                StickinessSource::Strategy
            };
            state.had_expired_or_invalid_affinity = false;

            return Ok(build_plan(
                provider,
                &model,
                chosen.clone(),
                retries_for_available(&available, &chosen, Some(&highest)),
                source,
            ));
        }

        let mut state = self.state.lock().expect("router state lock poisoned");
        let chosen = select_by_strategy(&mut state, snapshot, provider_key, &model, &highest)?
            .id
            .clone();
        Ok(build_plan(
            provider,
            &model,
            chosen.clone(),
            retries_for_available(&available, &chosen, Some(&highest)),
            StickinessSource::Strategy,
        ))
    }
}

impl RouterState {
    fn get_affinity(&mut self, key: &str) -> Option<String> {
        let now = Instant::now();
        match self.affinity.get(key) {
            Some(entry) if entry.expires_at > now => Some(entry.auth_id.clone()),
            Some(_) => {
                self.affinity.remove(key);
                self.had_expired_or_invalid_affinity = true;
                None
            }
            None => None,
        }
    }

    fn set_affinity(&mut self, key: String, auth_id: String, ttl: Duration) {
        self.affinity.insert(
            key,
            AffinityEntry {
                auth_id,
                expires_at: Instant::now() + ttl,
            },
        );
    }

    fn next_round_robin_offset(&mut self, key: &str, len: usize) -> usize {
        if len <= 1 {
            return 0;
        }
        let offset = self.round_robin_offsets.entry(key.to_string()).or_insert(0);
        let current = *offset % len;
        *offset = offset.saturating_add(1);
        current
    }
}

fn build_plan(
    provider: ProviderKind,
    model: &str,
    auth_id: String,
    retry_candidates: Vec<String>,
    stickiness_source: StickinessSource,
) -> ExecutionPlan {
    ExecutionPlan {
        provider,
        model: model.to_string(),
        auth_id,
        retry_candidates,
        stickiness_source,
    }
}

fn provider_key(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Codex => "codex",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Mock => "mock",
    }
}

fn provider_enabled(snapshot: &RuntimeSnapshot, provider_key: &str) -> bool {
    snapshot
        .providers
        .get(provider_key)
        .map(|config| config.enabled)
        .unwrap_or(false)
}

fn resolve_model(
    snapshot: &RuntimeSnapshot,
    provider_key: &str,
    requested_model: &str,
) -> Result<String> {
    let requested_model = requested_model.trim();
    if requested_model.is_empty() {
        bail!("requested model must not be empty");
    }

    if let Some(resolved) = snapshot
        .model_aliases
        .get(provider_key)
        .and_then(|aliases| aliases.get(requested_model))
    {
        return Ok(resolved.clone());
    }

    if snapshot
        .models
        .get(provider_key)
        .map(|models| models.iter().any(|model| model == requested_model))
        .unwrap_or(false)
    {
        return Ok(requested_model.to_string());
    }

    bail!("requested model {requested_model} is not routable for provider {provider_key}")
}

fn available_auths<'a>(
    snapshot: &'a RuntimeSnapshot,
    provider_key: &str,
    model: &str,
) -> Vec<&'a AuthRecord> {
    let now = OffsetDateTime::now_utc();
    let mut auths = snapshot
        .auth_pool
        .iter()
        .filter(|auth| auth.enabled)
        .filter(|auth| auth.provider == provider_key)
        .filter(|auth| {
            auth.supports_models
                .iter()
                .any(|supported| supported == model)
        })
        .filter(|auth| !cooldown_active(auth.cooldown_until.as_deref(), now))
        .collect::<Vec<_>>();

    auths.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.id.cmp(&right.id))
    });
    auths
}

fn cooldown_active(cooldown_until: Option<&str>, now: OffsetDateTime) -> bool {
    cooldown_until
        .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
        .map(|until| until > now)
        .unwrap_or(false)
}

fn select_by_strategy<'a>(
    state: &mut RouterState,
    snapshot: &RuntimeSnapshot,
    provider_key: &str,
    model: &str,
    highest: &[&'a AuthRecord],
) -> Result<&'a AuthRecord> {
    if highest.is_empty() {
        bail!("no highest-priority auth candidates available");
    }

    match snapshot.routing.strategy {
        RoutingStrategy::FillFirst => Ok(highest[0]),
        RoutingStrategy::RoundRobin => {
            let rr_key = format!("{provider_key}::{model}::{}", highest[0].priority);
            let offset = state.next_round_robin_offset(&rr_key, highest.len());
            Ok(highest[offset])
        }
    }
}

fn retries_for_available(
    available: &[&AuthRecord],
    chosen_auth_id: &str,
    highest: Option<&[&AuthRecord]>,
) -> Vec<String> {
    let mut retries = Vec::new();

    if let Some(highest) = highest {
        for auth in highest {
            if auth.id != chosen_auth_id {
                retries.push(auth.id.clone());
            }
        }
    }

    for auth in available {
        if auth.id != chosen_auth_id && !retries.iter().any(|id| id == &auth.id) {
            retries.push(auth.id.clone());
        }
    }

    retries
}

fn trim_to_owned(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn extract_codex_session_id(metadata: Option<&Value>) -> Option<String> {
    let metadata = metadata?;
    if let Some(value) = metadata.get("user_id").and_then(Value::as_str) {
        if value.trim_start().starts_with('{') {
            if let Ok(parsed) = serde_json::from_str::<Value>(value) {
                if let Some(session_id) = parsed.get("session_id").and_then(Value::as_str) {
                    return trim_to_owned(Some(session_id))
                        .map(|session_id| format!("codex:{session_id}"));
                }
            }
        }
    }
    metadata
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|session_id| trim_to_owned(Some(session_id)))
        .map(|session_id| format!("codex:{session_id}"))
}

pub fn extract_pinned_auth_id(metadata: Option<&Value>) -> Option<String> {
    metadata
        .and_then(|metadata| metadata.get("pinned_auth_id"))
        .and_then(Value::as_str)
        .and_then(|auth_id| trim_to_owned(Some(auth_id)))
}

#[cfg(test)]
mod tests {
    use cliproxy_common_types::{
        routing::{ExecutionPlan, StickinessSource},
        snapshot::{
            AuthRecord, ProviderConfig, RouteConfig, RoutingConfig, RoutingStrategy,
            RuntimeSnapshot,
        },
        upstream::ProviderKind,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    use super::{PlanRequest, RouterCore, extract_codex_session_id, extract_pinned_auth_id};

    fn snapshot_with_auths(auth_pool: Vec<AuthRecord>) -> RuntimeSnapshot {
        let mut providers = BTreeMap::new();
        providers.insert("codex".to_string(), ProviderConfig { enabled: true });

        let mut model_aliases = BTreeMap::new();
        model_aliases.insert(
            "codex".to_string(),
            BTreeMap::from([("codex-latest".to_string(), "gpt-5-codex".to_string())]),
        );

        let mut models = BTreeMap::new();
        models.insert("codex".to_string(), vec!["gpt-5-codex".to_string()]);

        RuntimeSnapshot {
            version: "test-v1".to_string(),
            generated_at: "2026-06-14T00:00:00Z".to_string(),
            source_instance_id: "test".to_string(),
            routes: RouteConfig {
                responses: true,
                chat_completions: false,
                messages: false,
            },
            routing: RoutingConfig {
                strategy: RoutingStrategy::FillFirst,
                session_affinity: true,
                session_ttl_seconds: 3600,
            },
            providers,
            model_aliases,
            models,
            auth_pool,
            ..RuntimeSnapshot::default()
        }
    }

    fn auth(id: &str, priority: i32, enabled: bool, cooldown_until: Option<&str>) -> AuthRecord {
        AuthRecord {
            id: id.to_string(),
            provider: "codex".to_string(),
            auth_kind: String::new(),
            priority,
            enabled,
            supports_models: vec!["gpt-5-codex".to_string()],
            labels: vec![],
            execution: Default::default(),
            cooldown_until: cooldown_until.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn fill_first_prefers_highest_priority_and_resolves_alias() {
        let snapshot = snapshot_with_auths(vec![
            auth("auth-low", 10, true, None),
            auth("auth-high-b", 100, true, None),
            auth("auth-high-a", 100, true, None),
        ]);
        let router = RouterCore::new();

        let plan = router
            .plan(
                &snapshot,
                PlanRequest {
                    requested_model: "codex-latest".to_string(),
                    session_id: None,
                    pinned_auth_id: None,
                },
            )
            .expect("plan");

        assert_eq!(
            plan,
            ExecutionPlan {
                provider: ProviderKind::Codex,
                model: "gpt-5-codex".to_string(),
                auth_id: "auth-high-a".to_string(),
                retry_candidates: vec!["auth-high-b".to_string(), "auth-low".to_string()],
                stickiness_source: StickinessSource::Strategy,
            }
        );
    }

    #[test]
    fn round_robin_only_rotates_inside_highest_priority_layer() {
        let mut snapshot = snapshot_with_auths(vec![
            auth("auth-p100-a", 100, true, None),
            auth("auth-p100-b", 100, true, None),
            auth("auth-p050", 50, true, None),
        ]);
        snapshot.routing.strategy = RoutingStrategy::RoundRobin;
        let router = RouterCore::new();

        let first = router
            .plan(
                &snapshot,
                PlanRequest {
                    requested_model: "gpt-5-codex".to_string(),
                    session_id: None,
                    pinned_auth_id: None,
                },
            )
            .expect("first");
        let second = router
            .plan(
                &snapshot,
                PlanRequest {
                    requested_model: "gpt-5-codex".to_string(),
                    session_id: None,
                    pinned_auth_id: None,
                },
            )
            .expect("second");

        assert_eq!(first.auth_id, "auth-p100-a");
        assert_eq!(second.auth_id, "auth-p100-b");
        assert_eq!(
            first.retry_candidates,
            vec!["auth-p100-b".to_string(), "auth-p050".to_string()]
        );
        assert_eq!(
            second.retry_candidates,
            vec!["auth-p100-a".to_string(), "auth-p050".to_string()]
        );
    }

    #[test]
    fn session_affinity_reuses_previous_auth_when_available() {
        let mut snapshot = snapshot_with_auths(vec![
            auth("auth-a", 100, true, None),
            auth("auth-b", 100, true, None),
        ]);
        snapshot.routing.strategy = RoutingStrategy::RoundRobin;
        let router = RouterCore::new();

        let first = router
            .plan(
                &snapshot,
                PlanRequest {
                    requested_model: "gpt-5-codex".to_string(),
                    session_id: Some("session-1".to_string()),
                    pinned_auth_id: None,
                },
            )
            .expect("first");
        let second = router
            .plan(
                &snapshot,
                PlanRequest {
                    requested_model: "gpt-5-codex".to_string(),
                    session_id: Some("session-1".to_string()),
                    pinned_auth_id: None,
                },
            )
            .expect("second");

        assert_eq!(first.auth_id, "auth-a");
        assert_eq!(second.auth_id, "auth-a");
        assert_eq!(second.stickiness_source, StickinessSource::SessionAffinity);
    }

    #[test]
    fn session_affinity_rebinds_when_cached_auth_is_unavailable() {
        let snapshot = snapshot_with_auths(vec![
            auth("auth-a", 100, true, None),
            auth("auth-b", 100, true, None),
        ]);
        let router = RouterCore::new();

        let first = router
            .plan(
                &snapshot,
                PlanRequest {
                    requested_model: "gpt-5-codex".to_string(),
                    session_id: Some("session-1".to_string()),
                    pinned_auth_id: None,
                },
            )
            .expect("first");
        assert_eq!(first.auth_id, "auth-a");

        let degraded_snapshot = snapshot_with_auths(vec![
            auth("auth-a", 100, true, Some("2999-01-01T00:00:00Z")),
            auth("auth-b", 100, true, None),
        ]);

        let second = router
            .plan(
                &degraded_snapshot,
                PlanRequest {
                    requested_model: "gpt-5-codex".to_string(),
                    session_id: Some("session-1".to_string()),
                    pinned_auth_id: None,
                },
            )
            .expect("second");

        assert_eq!(second.auth_id, "auth-b");
        assert_eq!(
            second.stickiness_source,
            StickinessSource::ReboundSessionAffinity
        );
    }

    #[test]
    fn pinned_auth_overrides_strategy_and_affinity() {
        let mut snapshot = snapshot_with_auths(vec![
            auth("auth-a", 100, true, None),
            auth("auth-b", 100, true, None),
        ]);
        snapshot.routing.strategy = RoutingStrategy::RoundRobin;
        let router = RouterCore::new();

        let plan = router
            .plan(
                &snapshot,
                PlanRequest {
                    requested_model: "gpt-5-codex".to_string(),
                    session_id: Some("session-1".to_string()),
                    pinned_auth_id: Some("auth-b".to_string()),
                },
            )
            .expect("plan");

        assert_eq!(plan.auth_id, "auth-b");
        assert_eq!(plan.stickiness_source, StickinessSource::PinnedAuth);
    }

    #[test]
    fn codex_metadata_extractors_follow_expected_fields() {
        let metadata = json!({
            "user_id": "{\"device_id\":\"device-a\",\"account_uuid\":\"\",\"session_id\":\"session-7\"}",
            "pinned_auth_id": "auth-b"
        });

        assert_eq!(
            extract_codex_session_id(Some(&metadata)).as_deref(),
            Some("codex:session-7")
        );
        assert_eq!(
            extract_pinned_auth_id(Some(&metadata)).as_deref(),
            Some("auth-b")
        );
    }
}
