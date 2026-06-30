# Current State

This document records durable repository-level state. It is not a per-session task tracker.

## Current Stable Architecture

- Main Go server is the primary proxy runtime and management plane.
- Go server exposes API routes, management routes, auth handling, model registry, watcher-based reload, and SDK hooks.
- Rust data plane exists as a separate workspace and can serve `/v1/responses`.
- Rust data plane consumes Go-exported runtime snapshot by file or HTTP.
- Rust data plane now aligns milestone 8 with CPA usage-queue semantics instead of exposing a separate Prometheus-style route.
- Rust `/v1/responses` no longer falls back to local mock responses when no real upstream is available.

## Implemented Capabilities

- OpenAI / Gemini / Claude / Codex / Grok compatible proxy interfaces
- OAuth-backed provider login flows for multiple providers
- Multi-account routing and load balancing
- Go watcher-based config / auth reload
- Optional plugin system
- SDK under `sdk/cliproxy`
- Rust data plane:
  - health / readiness
  - runtime snapshot load / refresh
  - `/v1/responses`
  - explicit minimal request IR + stream-event IR inside the `/v1/responses` pipeline
  - `/v1/responses` SSE frame repair and completed-output repair on the HTTP streaming path
  - direct `502 upstream_unavailable` behavior when `/v1/responses` cannot construct a real upstream execution path
  - CPA-shaped usage queue payload emission for `/v1/responses` when `usage_queue.enabled=true` and `usage_queue.backend=redis`
  - async usage payload production with log-backed sink for `/v1/responses` requests
  - pre-commit auth retry classification for `/v1/responses`
  - upstream request/response redaction helpers for logging
  - snapshot notify endpoint
  - runtime snapshot observation endpoint

## Unfinished Or 待确认 Capabilities

- Full production-grade Go-managed Rust instance registry / heartbeat lifecycle is not fully confirmed from current repository state.
- Formal multi-instance Rust data-plane management is `待确认`.
- Some deployment and management-center flows are partially documented externally; exact in-repo completeness is `待确认`.

## Current Active Development Direction

- Go-managed Rust data plane for `/v1/responses`
- Runtime snapshot export / refresh / observability
- Dev stack tooling in `Makefile`
- Unified upstream proxy behavior between Go and Rust
- CPA-aligned Rust usage queue integration for `/v1/responses`

## Frozen Key Decisions

- Repository docs should be treated as the fact source.
- Runtime snapshot is the effective configuration contract for Rust data plane.
- Go should notify Rust of snapshot changes; Rust should pull full snapshot afterward.
- Polling remains as fallback even if notify exists.

## Known Risks

- Large mixed Go codebase with many provider flows increases accidental regression risk.
- Rust data-plane productization work may diverge from Go behavior if proxy, routing, or auth semantics are changed in only one runtime.
- Test coverage appears stronger in selected areas than for full end-to-end production flows.
- Some architectural intent lives in docs and current worktree changes, not only in released code.
- Rust currently emits CPA-shaped usage payloads but does not yet expose the full CPA redis subscription/pop protocol from the Rust process.
- Rust milestone-6 parity coverage now includes fixture-driven SSE framer checks derived from Go stream-repair samples, including malformed blank-line event/data cases, but it is still not a full Go fixture mirror.

## Collaboration Boundaries

- Durable shared facts belong in `.ai-harness/shared/`.
- Session notes, scratch analysis, and handoff drafts belong in `.ai-harness/local/`.
- If a current activity is only relevant to one session, it should not be added here.
