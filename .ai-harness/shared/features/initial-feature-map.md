# Initial Feature Map

## Proxy API Server

- Responsibility:
  - expose OpenAI / Gemini / Claude / Codex / Grok compatible endpoints
  - authenticate callers
  - route to matching upstream credentials
- Entrypoints:
  - `cmd/server/main.go`
  - `internal/api/server.go`
- Config:
  - `config.yaml`
  - `config.example.yaml`
- Commands:
  - `go run ./cmd/server`
  - `go build -o cli-proxy-api ./cmd/server`

## Management API

- Responsibility:
  - manage config, auth files, runtime snapshot, plugin operations, and related admin actions
- Entrypoints:
  - `internal/api/handlers/management/`
- Config:
  - `remote-management`
- Commands:
  - direct HTTP access via `/v0/management/...`

## Auth / Credential Routing

- Responsibility:
  - load, store, refresh, and select multiple credentials across providers
- Entrypoints:
  - `internal/runtime/executor/`
  - `internal/watcher/`
  - `sdk/cliproxy/auth`
- Config:
  - auth files
  - provider key sections in YAML
  - optional storage backend env vars

## SDK Embedding

- Responsibility:
  - expose reusable proxy service behavior for other programs
- Entrypoints:
  - `sdk/cliproxy/`
- Related Docs:
  - `docs/sdk-usage.md`
  - `docs/sdk-advanced.md`
  - `docs/sdk-access.md`
  - `docs/sdk-watcher.md`

## Plugin System

- Responsibility:
  - support dynamic plugin loading and plugin-backed provider extensions
- Entrypoints:
  - `internal/pluginhost/`
  - `plugins` config section
- Config:
  - `plugins.enabled`
  - `plugins.dir`
  - `plugins.configs`

## Rust Data Plane

- Responsibility:
  - serve `/v1/responses`
  - repair and normalize SSE frames on the `/v1/responses` HTTP streaming path
  - emit CPA-shaped `/v1/responses` usage queue payloads into a local CPA-compatible usage queue
  - expose `/v0/management/usage-queue` and Redis RESP usage-consumer commands for CPA-compatible usage consumption
  - fail `/v1/responses` with a direct upstream error when no real upstream execution path is available
  - consume runtime snapshot from Go
  - expose health / readiness / runtime snapshot observation
- Entrypoints:
  - `rust/cliproxy-data-plane/src/main.rs`
  - `rust/cliproxy-data-plane/src/http.rs`
  - `rust/cliproxy-data-plane/src/telemetry.rs`
- Config:
  - Rust CLI flags / env vars
  - Go runtime snapshot
- Commands:
  - `cargo run --manifest-path rust/cliproxy-data-plane/Cargo.toml -- ...`
  - `make dev-stack`
  - `make dev-stack-url`

## Snapshot / Go-to-Rust Runtime Sync

- Responsibility:
  - export effective runtime config from Go
  - refresh Rust applied snapshot by polling and notify
- Entrypoints:
  - `internal/dataplane/snapshot/`
  - `internal/dataplane/notifier/`
  - `rust/cliproxy-data-plane/src/runtime.rs`
  - `rust/cliproxy-data-plane/src/http.rs`
- Commands:
  - `make snapshot-current`
  - `make snapshot-rs`
  - `make diff-snapshots`

## Missing Or 待确认

- Full production status of Rust data plane beyond current repository work.
- Formal multi-instance Rust lifecycle / registry feature completeness.
- Whether there are additional official deployment modes beyond local binary and Docker / Compose.
