# Rust Errors, Auth Health, and Cooldown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add single-node Rust data-plane support for `errors` event production, runtime auth/model health accumulation, cooldown recommendations, and routing/retry behavior that consumes those states.

**Architecture:** Introduce a Rust-local runtime overlay for auth/model health, publish best-effort error events on the existing RESP `errors` channel, and consult the overlay during auth selection and pre-commit retry. Keep snapshot as the configuration fact source and avoid Go protocol changes.

**Tech Stack:** Rust, Axum, Tokio, serde, existing Rust data-plane modules, repo `.ai-harness` docs

---

### Task 1: Add Runtime Auth State Modules

**Files:**
- Create: `rust/cliproxy-data-plane/src/auth_state.rs`
- Create: `rust/cliproxy-data-plane/src/error_events.rs`
- Modify: `rust/cliproxy-data-plane/src/app.rs`
- Modify: `rust/cliproxy-data-plane/src/http.rs`

- [ ] Define in-memory auth-level and model-level runtime state with lookup, update, clear, and cooldown query helpers.
- [ ] Define the error-event payload type and serialization helpers.
- [ ] Thread the new shared state through app startup and request app state wiring.

### Task 2: Produce Real `errors` Events

**Files:**
- Modify: `rust/cliproxy-data-plane/src/usage_queue.rs`
- Modify: `rust/cliproxy-data-plane/src/redis_protocol.rs`
- Modify: `rust/cliproxy-data-plane/src/error_events.rs`

- [ ] Add a queue publish path for error payloads that targets only `errors` subscribers.
- [ ] Keep `usage` and `errors` channels separate while preserving current RESP behavior.
- [ ] Add or update queue/protocol tests so `SUBSCRIBE errors` receives real payloads.

### Task 3: Classify Upstream Failures Into Health and Cooldown

**Files:**
- Modify: `rust/cliproxy-data-plane/src/responses/upstream.rs`
- Modify: `rust/cliproxy-data-plane/src/responses.rs`
- Modify: `rust/cliproxy-data-plane/src/telemetry.rs`

- [ ] Add CPA-aligned failure classification for auth-level versus model-level failures.
- [ ] Map failures to cooldown windows and quota flags.
- [ ] Update request telemetry flow so failures publish error events after state mutation.
- [ ] Add focused unit tests for `401`, `403`, `404`, `429`, `5xx`, and unsupported-model cases.

### Task 4: Make Route Selection Respect Runtime Cooldowns

**Files:**
- Modify: `rust/cliproxy-data-plane/src/responses/handler.rs`
- Modify: `rust/cliproxy-data-plane/src/responses.rs`
- Modify: `rust/cliproxy-data-plane/src/auth_state.rs`

- [ ] Filter auth candidates using snapshot cooldown plus overlay auth/model cooldown.
- [ ] Return a stable blocked/cooldown error when every candidate is unavailable.
- [ ] Add tests covering candidate filtering and all-candidates-blocked behavior.

### Task 5: Make Pre-Commit Retry Skip Failed Auths

**Files:**
- Modify: `rust/cliproxy-data-plane/src/responses/upstream.rs`
- Modify: `rust/cliproxy-data-plane/src/auth_state.rs`
- Test: `rust/cliproxy-data-plane/tests/http_routes.rs`

- [ ] After a retry-switchable pre-commit failure, mark the auth/model unavailable and skip it for subsequent retry candidates.
- [ ] Keep post-commit streaming behavior unchanged.
- [ ] Add route-level coverage proving the next candidate is used and the failed auth is skipped.

### Task 6: Update Docs and Validate

**Files:**
- Modify: `.ai-harness/shared/current-state.md`
- Modify: `.ai-harness/shared/architecture.md`
- Modify: `.ai-harness/shared/testing.md`
- Modify: `rust/cliproxy-data-plane/docs/current/当前架构说明.md`
- Modify: `rust/cliproxy-data-plane/docs/current/用量队列契约与差距.md`
- Create or modify: `docs/operations/` runbook file if needed

- [ ] Document the new `errors` channel producer, runtime overlay, and cooldown semantics.
- [ ] Add a short runbook for observing and recovering from Rust auth/model cooldowns.
- [ ] Run `cargo fmt --manifest-path rust/cliproxy-data-plane/Cargo.toml`.
- [ ] Run `cargo test --workspace --manifest-path rust/cliproxy-data-plane/Cargo.toml`.
