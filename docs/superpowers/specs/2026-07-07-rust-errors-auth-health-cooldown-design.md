# Rust Errors, Auth Health, and Cooldown Design

## Goal

Add the minimum single-node Rust data-plane support for:

- real `errors` channel production
- runtime auth/model health state accumulation
- cooldown recommendations that affect auth selection and pre-commit retry
- operational documentation for observing and recovering these states

This design only applies to Rust `/v1/responses`.

## Scope

In scope:

- Rust in-memory runtime overlay for auth/model failure state
- `errors` RESP subscription payload production
- cooldown classification based on current Rust upstream failures
- routing and pre-commit retry using the runtime overlay
- tests and docs for the new behavior

Out of scope:

- Go control-plane protocol changes
- snapshot schema changes
- multi-instance synchronization
- persisted health/cooldown state
- Home-mode `LPUSH usage`

## Constraints

- Keep the change local to the Rust data plane.
- Preserve the existing runtime snapshot as the only config fact source.
- Do not block the hot path on event publication.
- Prefer behavior alignment with CPA Go auth manager semantics where practical.
- Single-process memory state is acceptable for the first version.

## Current Baseline

The current Rust data plane already has:

- usage queue production for `/v1/responses`
- RESP `SUBSCRIBE usage/errors`
- pre-commit retry classification for selected upstream failures
- snapshot-exported `cooldown_until` on auth records

The current gap is that:

- `errors` has protocol support but no payload producer
- Rust does not accumulate runtime auth/model health beyond snapshot cooldown
- retry and route selection do not share a local health/cooldown overlay

## Design

### 1. Runtime Overlay

Add a Rust-local runtime overlay that sits beside the snapshot runtime state.

The overlay stores transient auth health derived from observed upstream failures. It does not mutate snapshot auth records and does not survive process restart.

Two scopes are required:

- auth-level state
- model-level state keyed by `auth_index + resolved model`

Each state record stores:

- `status`
- `unavailable`
- `status_message`
- `last_error_code`
- `last_error_message`
- `next_retry_after`
- `quota_exceeded`
- `quota_reason`
- `updated_at`

### 2. Error Event Contract

Add a distinct error-event payload for the RESP `errors` channel.

Minimum fields:

- `timestamp`
- `request_id`
- `provider`
- `model`
- `auth_index`
- `scope`
- `status_code`
- `error_code`
- `message`
- `retry_after_ms`
- `cooldown_until`
- `quota_exceeded`
- `reason`

These events are best-effort, non-blocking, and should be published after the runtime overlay is updated.

### 3. Failure Classification

Map upstream failures into overlay state using CPA-like semantics.

Auth-level failures:

- `401`
- explicit authentication errors such as `invalid_api_key`, `invalid or expired token`, `refresh_token_reused`
- auth-level `402/403`

Model-level failures:

- `429 usage_limit_reached`
- `404 not_found`
- `400/422 model_not_supported`
- `408/500/502/503/504`

Cooldown rules:

- `401` -> auth-level cooldown `30m`
- `402/403` -> auth-level cooldown `30m`
- `404` -> model-level cooldown `12h`
- `429 usage_limit_reached` -> retry hint from upstream when available, otherwise local backoff
- `408/500/502/503/504` -> model-level transient cooldown `60s`
- `400/422 model_not_supported` -> model-level cooldown `12h`

### 4. Route Selection

Before upstream execution, Rust should filter candidate auths with three inputs:

- snapshot `cooldown_until`
- auth-level overlay cooldown
- model-level overlay cooldown for the resolved execution model

This filtering happens in Rust after router planning, without changing snapshot schema or router-core interfaces.

If every candidate is blocked, Rust should return a stable cooldown/unavailable error rather than selecting a blocked auth.

### 5. Pre-Commit Retry

When upstream execution fails before any downstream bytes are committed:

1. classify the failure
2. update the overlay
3. publish an `errors` event
4. skip the failed auth if the classification is retry-switchable
5. continue with the next retry candidate

Retry-switchable classes for the first version:

- auth failures
- quota failures
- model-not-supported / not-found failures
- transient upstream failures

Once bytes are committed downstream, Rust keeps current behavior and does not attempt cross-auth failover.

## Module Layout

New modules:

- `src/auth_state.rs`
- `src/error_events.rs`

Updated modules:

- `src/app.rs`
- `src/http.rs`
- `src/usage_queue.rs`
- `src/redis_protocol.rs`
- `src/responses/handler.rs`
- `src/responses/upstream.rs`

## Testing Strategy

Required coverage:

- error payload publication on `errors`
- auth-level and model-level overlay updates
- cooldown filtering during auth selection
- pre-commit retry skipping blocked auths
- success clearing previously accumulated failure state

## Operational Notes

The first version is intentionally single-node and memory-only. Recovery happens through:

- successful later requests
- waiting for cooldown expiry
- process restart
- existing Go-side quota reset or auth refresh workflows where applicable

This is sufficient for the current single-machine operating mode.
