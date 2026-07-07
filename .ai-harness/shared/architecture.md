# Architecture

## Overall Architecture

- Primary runtime:
  - Go HTTP server acting as proxy, management plane, and SDK host
- Secondary runtime:
  - Rust data plane specialized for `/v1/responses`
- Shared operational model:
  - config + auth state drive routing and upstream execution

## Directory Structure

- `cmd/server/`
  - Go server entrypoint
- `internal/api/`
  - Main Gin HTTP API, route setup, middleware, management handlers
- `internal/runtime/executor/`
  - Provider-specific Go runtime executors
- `internal/translator/`
  - Provider protocol translation layer
- `internal/registry/`
  - Model registry and updater
- `internal/watcher/`
  - Config and auth change watching / reload
- `internal/store/`
  - Storage backends and secret persistence
- `internal/managementasset/`
  - Management panel / asset handling
- `sdk/`
  - Embeddable SDK and shared utilities
- `rust/cliproxy-data-plane/`
  - Rust workspace for data-plane functionality
- `test/`
  - Go cross-module integration tests
- `docs/`
  - SDK docs and related documentation
- `rust/cliproxy-data-plane/docs/`
  - Rust data-plane docs split by `current/`, `roadmap/`, `design/`, and `history/`

## Core Module Responsibilities

- `internal/api/server.go`
  - Build and wire the main Go HTTP server
- `internal/api/handlers/management/`
  - Management API for config, auth, plugins, runtime snapshot, and admin operations
- `internal/dataplane/snapshot/`
  - Build runtime snapshot exported from Go
- `internal/dataplane/notifier/`
  - Notify Rust data plane when effective snapshot version changes
- `internal/api/dataplane_usage_bridge.go`
  - Resolve bridge enablement/auth config, subscribe to Rust data-plane usage queue, and re-enqueue records into CPA `internal/redisqueue` so external consumers can keep reading from CPA
- `internal/watcher/`
  - React to config and auth file changes
- `sdk/cliproxy/`
  - Reusable embedded service abstraction
- `rust/cliproxy-data-plane/src/runtime.rs`
  - Hold current runtime snapshot and runtime metadata
- `rust/cliproxy-data-plane/src/http.rs`
  - Rust HTTP routes for health, readiness, snapshot inspection, notify, and `/v1/responses`
- `rust/cliproxy-data-plane/src/usage_queue.rs`
  - CPA-compatible in-memory usage queue, subscriber fan-out, refresh control payload, and pop semantics
- `rust/cliproxy-data-plane/src/redis_protocol.rs`
  - CPA-compatible Redis RESP usage consumer protocol for `AUTH`, `SUBSCRIBE`, `LPOP`, and `RPOP`
- `rust/cliproxy-data-plane/src/telemetry.rs`
  - Request lifecycle telemetry, extracted usage observation helper, and CPA-shaped usage payload emission into the local usage queue
- `rust/cliproxy-data-plane/src/responses.rs`
  - Rust `/v1/responses` parent module with shared request/response types, centralized request metadata extraction, helpers, and unit tests
- `rust/cliproxy-data-plane/src/responses/`
  - Rust `/v1/responses` child modules split by responsibility:
  - `handler.rs` route handler orchestration and plan/bootstrap flow
  - `protocol.rs` minimal request / stream-event protocol IR for `/v1/responses`
  - `upstream.rs` real upstream execution, request normalization, and auth retry
  - `sse.rs` SSE framing and completed-output repair
- `rust/cliproxy-data-plane/crates/usage-events/`
  - CPA-shaped usage queue payload type and async producer for non-blocking sinks
- `rust/cliproxy-data-plane/crates/upstream-runtime/src/lib.rs`
  - upstream HTTP execution plus shared request/response redaction helpers for logging

## Main Data Flow

- Go main path:
  - config + auth -> route selection -> executor / translator -> upstream provider -> normalized response
- Go -> Rust runtime-config path:
  - Go config/auth state -> runtime snapshot -> Rust pull / apply
  - auth records include the Go-resolved `usage_source` and stable `auth_index` so Rust usage payloads can preserve CPA identity attribution semantics
- Rust responses path:
  - request -> centralized request metadata extraction -> request IR -> runtime snapshot + router core -> upstream runtime -> stream-event IR + normalized downstream response -> CPA-shaped usage payload emission
  - request metadata now owns shared extraction of `session_id`, `pinned_auth_id`, `reasoning.effort` / fallback `reasoning_effort`, and `service_tier` before routing and telemetry
  - if no real upstream execution path can be constructed, return error immediately instead of synthesizing local mock responses
- Rust usage-consumption path:
  - `/v1/responses` telemetry -> local CPA-compatible usage queue
  - Rust captures downstream API key headers at the HTTP boundary and emits them as CPA-compatible `api_key`; selected auth `usage_source` is emitted as CPA-compatible `source`; selected auth `auth_index` is emitted as CPA-compatible `auth_index`
  - TTFT is recorded once at the first observed upstream response byte and must not be overwritten by later chunks or repaired SSE frames
  - HTTP consumers can pop records with `/v0/management/usage-queue?count=N`
  - Redis RESP consumers can use the same TCP listener with `AUTH`, `SUBSCRIBE usage/errors`, and `LPOP/RPOP usage`
- External CPA usage-consumer path:
  - `cpa-usage-keeper` and other external consumers can continue connecting to CPA
  - when Go routes `/v1/responses` to Rust and usage statistics are enabled, CPA subscribes to Rust `usage` over Redis RESP and writes those records back into CPA `internal/redisqueue`
  - bridge enablement and auth are resolved once up front; RESP auth still uses the embedded/local management password when available, otherwise the plaintext `MANAGEMENT_PASSWORD` environment variable used by external dev-stack snapshot auth
  - if RESP subscription is unavailable or disconnects, CPA falls back to Rust `/v0/management/usage-queue?count=64` before retrying the RESP subscription

## Main Control Flow

- Go startup:
  - load `.env`
  - parse flags
  - load config
  - initialize plugin host, stores, auth manager, server
  - start watcher and HTTP server
- Rust startup:
  - parse CLI / env
  - build snapshot client
  - initial snapshot load
  - start periodic refresh
  - serve HTTP routes and Redis RESP usage consumers on the same TCP listener by sniffing the first connection byte
  - handle SIGTERM / Ctrl-C with graceful listener shutdown logs

## Error Handling Strategy

- Go:
  - prefer returned errors and structured logging
  - avoid panics in HTTP handlers
  - avoid `log.Fatal` / `log.Fatalf`
- Rust:
  - use `Result` / `anyhow`
  - update runtime state to degraded / failed when snapshot refresh fails

## Configuration Loading Strategy

- Go:
  - `config.yaml` / provided `--config`
  - `.env` autoload
  - optional storage backend env vars
  - watcher reloads config / auth state
  - on the `rusty` branch, `data-plane` defaults to embedded mode unless config explicitly selects `external` or `disabled`
- Rust:
  - CLI flags + environment variables
  - runtime snapshot from file or HTTP

## Logging Strategy

- Go:
  - Logrus
  - optional file logging
  - request logs and error logs configurable
  - embedded Rust data-plane supervisors materialize artifacts under the configured state directory; when `state-dir` is omitted this now defaults to the `CLIProxyAPI` executable directory, and child-process `stdout.log` / `stderr.log` live under `stateDir/logs/data-plane/` with embedded-specific rotation and cleanup while also being mirrored into the Go process stdout/stderr stream for container log visibility
- Rust:
  - `tracing` / `tracing-subscriber`

## Test Strategy

- Go:
  - package tests across `internal/` and integration tests under `test/`
- Rust:
  - workspace unit tests
  - HTTP route integration tests under `rust/cliproxy-data-plane/tests/`

## Extension Points

- Go plugin system under `plugins` config and `pluginhost`
- SDK embedding via `sdk/cliproxy`
- Rust data plane as optional execution path for `/v1/responses`

## Current Architecture Risks

- Cross-runtime behavior drift between Go and Rust
- Large Go server with multiple concerns increases change surface
- Some important behavior is optional or mode-dependent, which can obscure the real production path
- Full Rust data-plane lifecycle architecture remains only partially inferable from current repo state
