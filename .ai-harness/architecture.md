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

## Core Module Responsibilities

- `internal/api/server.go`
  - Build and wire the main Go HTTP server
- `internal/api/handlers/management/`
  - Management API for config, auth, plugins, runtime snapshot, and admin operations
- `internal/dataplane/snapshot/`
  - Build runtime snapshot exported from Go
- `internal/dataplane/notifier/`
  - Notify Rust data plane when effective snapshot version changes
- `internal/watcher/`
  - React to config and auth file changes
- `sdk/cliproxy/`
  - Reusable embedded service abstraction
- `rust/cliproxy-data-plane/src/runtime.rs`
  - Hold current runtime snapshot and runtime metadata
- `rust/cliproxy-data-plane/src/http.rs`
  - Rust HTTP routes
- `rust/cliproxy-data-plane/src/responses.rs`
  - Rust `/v1/responses` handling and upstream dispatch

## Main Data Flow

- Go main path:
  - config + auth -> route selection -> executor / translator -> upstream provider -> normalized response
- Go -> Rust runtime-config path:
  - Go config/auth state -> runtime snapshot -> Rust pull / apply
- Rust responses path:
  - request -> runtime snapshot + router core -> upstream runtime -> normalized downstream response

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
  - serve HTTP routes

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
- Rust:
  - CLI flags + environment variables
  - runtime snapshot from file or HTTP

## Logging Strategy

- Go:
  - Logrus
  - optional file logging
  - request logs and error logs configurable
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
