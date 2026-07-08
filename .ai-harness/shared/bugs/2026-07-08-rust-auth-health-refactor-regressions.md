# Rust Auth Health Refactor Regressions

## Symptom

- After the auth/cooldown refactor, a successful failover request could leave the failed primary auth immediately reusable.
- If `execution_plan.auth_id` was missing from the latest snapshot while `selected_auth` was still available, auth-bound retry could silently skip the selected auth and fall back to generic upstream execution.
- Malformed snapshot auth records with both empty `auth_index` and empty `id` could panic on the request path.

## Impact

- Retryable auth failures could fail to produce a durable cooldown, weakening the new single-node auth health loop.
- Snapshot/plan inconsistency could bypass auth-bound retry semantics and change upstream selection behavior.
- Bad snapshot data could crash the Rust data-plane request path instead of failing gracefully.

## Root Cause

- Success cleanup in `responses/upstream.rs` was attributed to the originally selected auth instead of the actual candidate that succeeded after failover.
- The refactor reused snapshot-based retry-chain construction for upstream execution, which accidentally dropped the previous “always start from `selected_auth`” fallback.
- `AuthKey::from_auth_record` changed from string fallback logic to `expect(...)`, turning malformed auth identity data into a panic.

## Fix

- Upstream execution now returns the actual successful auth candidate, and health cleanup is applied to that candidate only.
- Auth-bound retry now uses a dedicated chain builder that always starts from `selected_auth` and then appends snapshot retry candidates.
- `AuthKey::from_auth_record` now returns `Option<AuthKey>` and all callers degrade gracefully when no stable auth identity is available.

## Validation

- `cargo fmt --manifest-path rust/cliproxy-data-plane/Cargo.toml`
- `cargo test --workspace --manifest-path rust/cliproxy-data-plane/Cargo.toml`
- Added regression coverage for:
  - failover success preserving cooldown on the failed primary auth
  - auth-bound retry preserving `selected_auth` when snapshot primary is missing
  - malformed auth records not producing an `AuthKey`
