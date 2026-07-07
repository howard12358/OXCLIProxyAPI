# Current State

This document records durable repository-level state. It is not a per-session task tracker.

## Current Stable Architecture

- Main Go server is the primary proxy runtime and management plane.
- Go server exposes API routes, management routes, auth handling, model registry, watcher-based reload, and SDK hooks.
- Rust data plane exists as a separate workspace and can serve `/v1/responses`.
- On the `rusty` branch, an empty `data-plane` config now defaults to embedded Rust `/v1/responses`; explicit `data-plane.mode: disabled` is the opt-out.
- Rust data plane consumes Go-exported runtime snapshot by file or HTTP.
- Rust data plane now aligns milestone 8 with CPA usage-queue semantics instead of exposing a separate Prometheus-style route.
- Rust data plane accepts HTTP and CPA-compatible Redis RESP usage-consumer traffic on the same TCP listener by sniffing the first connection byte.
- Rust data plane now also produces real RESP `errors` events for `/v1/responses`, keeps an in-memory auth/model health overlay, and uses that overlay to filter auth candidates plus pre-commit retries.
- Go CPA now bridges Rust data-plane usage queue records back into CPA `internal/redisqueue` with RESP `SUBSCRIBE usage` first and HTTP pop fallback, so external usage consumers can keep connecting to CPA.
- Go runtime snapshots include auth `usage_source` for Rust usage attribution, and Rust `/v1/responses` usage payloads include downstream API key attribution when present.
- Go runtime snapshots also export stable auth `auth_index`, and Rust usage payloads now emit that index instead of raw auth IDs while recording TTFT from the first upstream body chunk.
- Rust `/v1/responses` usage telemetry now preserves downstream `reasoning.effort` / fallback `reasoning_effort` and `service_tier`, and TTFT is fixed at the first observed response byte instead of being overwritten by later chunks.
- Go data-plane usage bridge keeps the same external auth/fallback behavior, but bridge enablement and auth selection are now resolved through a single internal config step; Rust `/v1/responses` likewise centralizes request metadata extraction for router planning and telemetry reuse.
- Rust `/v1/responses` Codex upstream emission now explicitly aligns its compatibility boundary with the Go-native Codex translator for key request rewrites, including forced `parallel_tool_calls/include`, stripping unsupported generation / context fields, `system -> developer` input role normalization, and builtin tool alias normalization.
- External dev-stack usage bridging now aligns Go `MANAGEMENT_PASSWORD` with Rust `--snapshot-bearer-token`, so the preferred RESP subscription path authenticates in `make dev-stack-url`.
- Dedicated embedded Docker image build support exists through `Dockerfile.embedded` and the manual `docker-embedded-image` workflow, including selectable Rust `release` / `debug` build profiles; the existing tag-driven Docker release remains unchanged.
- The repository root `docker-compose.yml` now defaults to pulling `rustyllh/ox-cli-proxy-api:latest`, while `docker-build.sh` and `docker-build.ps1` remain the source-build entrypoints that produce a separate local image tag.
- A repository smoke script now exists for embedded Docker deployments, covering `healthz`, runtime snapshot, non-stream and stream `/v1/responses`, usage queue pop, keeper reachability, and visible `[rs-stdout]` / `[rs-stderr]` prefixes in `docker logs`.
- Embedded Rust data-plane state directories now keep the materialized binary and checksum files at the root while writing `stdout.log` and `stderr.log` under `logs/data-plane/`, with dedicated rotation and cleanup that is separate from Go application log retention; the supervisor also mirrors Rust stdout/stderr into the container's main stdout/stderr stream with stable prefixes so `docker logs` can see embedded Rust output.
- When `data-plane.embedded.state-dir` is omitted, the embedded Rust data-plane state directory now defaults to the directory containing the running `CLIProxyAPI` executable.
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
  - CPA-compatible in-memory usage queue with subscriber fan-out, `support_refresh` / `refresh` control payloads, and pop semantics
  - HTTP usage queue pop endpoint at `/v0/management/usage-queue`
  - Redis RESP usage protocol support for `AUTH`, `SUBSCRIBE usage/errors`, `LPOP usage`, and `RPOP usage`
  - real RESP `errors` payload production for `/v1/responses` upstream failures
  - Go-side data-plane usage bridge that subscribes to Rust `usage` over Redis RESP and re-enqueues records into CPA redisqueue, with HTTP pop fallback
  - Go-side bridge auth selection that uses the embedded/local management password first and falls back to `MANAGEMENT_PASSWORD` for external data-plane dev stacks
  - pre-commit auth retry classification for `/v1/responses`
  - single-node in-memory auth/model health overlay that derives cooldowns from Rust upstream failures and blocks candidate reuse until recovery
  - upstream request/response redaction helpers for logging
  - Codex native Responses array input and extra top-level request fields are preserved through Rust `/v1/responses` upstream normalization
  - Codex request emission compatibility matrix covering forced rewrites, filtered unsupported fields, conditional `service_tier`, `system -> developer`, and web-search builtin tool alias normalization
  - snapshot notify endpoint
  - runtime snapshot observation endpoint
  - graceful SIGTERM / Ctrl-C shutdown logging for the Rust data-plane listener

## Unfinished Or 待确认 Capabilities

- Full production-grade Go-managed Rust instance registry / heartbeat lifecycle is not fully confirmed from current repository state.
- Formal multi-instance Rust data-plane management is `待确认`.
- Some deployment and management-center flows are partially documented externally; exact in-repo completeness is `待确认`.
- Public `cpa-usage-keeper` login automation on the currently observed deployment remains `待确认` because the public login endpoints return `403 {"error":"fetch request required"}` while the same error string is not present in the inspected local keeper repository, indicating an external gate or deployed-version drift outside this repo.

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
- Rust usage queue now exposes the CPA-compatible HTTP and RESP consumption paths, and CPA bridges Rust usage back into the external CPA queue. Home-mode direct Rust `LPUSH usage` forwarding is still not implemented.
- Rust auth/model health overlay is memory-only and single-process; restart clears overlay state, and there is still no cross-instance synchronization.
- Rust milestone-6 parity coverage now includes fixture-driven SSE framer checks derived from Go stream-repair samples, including malformed blank-line event/data cases, but it is still not a full Go fixture mirror.

## Collaboration Boundaries

- Durable shared facts belong in `.ai-harness/shared/`.
- Session notes, scratch analysis, and handoff drafts belong in `.ai-harness/local/`.
- If a current activity is only relevant to one session, it should not be added here.
