# Project Context

## Project Name

- `CLIProxyAPI` / `OXCLIProxyAPI`

## Project Goal

- Provide OpenAI / Gemini / Claude / Codex / Grok compatible proxy APIs for CLI-oriented AI tools.
- Support multi-account routing, OAuth-backed credentials, and compatible upstream access through a local proxy server.
- Provide a reusable Go SDK for embedding core proxy behavior.
- Current repository also contains an optional Rust data plane focused on `/v1/responses`.

## Main Technology Stack

- Go `1.26+`
- Rust workspace under `rust/cliproxy-data-plane`
- Gin for the main Go HTTP API
- Axum for the Rust data plane HTTP API
- Logrus for Go logging
- `reqwest` for Rust HTTP clients
- Docker / Docker Compose for containerized deployment

## Core Business Flow

- Client sends OpenAI-compatible or provider-compatible requests to the local proxy.
- Go server authenticates the caller, selects a credential, normalizes provider-specific behavior, and forwards to upstream providers.
- Config and auth material can be loaded from local files and optional storage backends.
- Optional Rust data plane can handle `/v1/responses`, consuming runtime snapshot from Go and calling upstream providers.

## Main Entrypoints

- Go server:
  - `cmd/server/main.go`
- Rust data plane:
  - `rust/cliproxy-data-plane/src/main.rs`
  - `rust/cliproxy-data-plane/src/app.rs`

## Configuration

- Primary config file:
  - `config.yaml`
- Example config:
  - `config.example.yaml`
- `.env` is auto-loaded from the working directory by the Go server.
- Auth material defaults under:
  - `auths/` in container mounts
  - `~/.cli-proxy-api` by config default
- Optional storage backends:
  - Postgres
  - Git-backed token store
  - Object store

## External Dependencies

- OAuth / provider endpoints for Gemini / Claude / Codex / Grok and compatible upstreams
- GitHub release / asset access for some management assets and plugin store behavior
- Optional:
  - Postgres
  - Git remote
  - Object store

## Deployment

- Local binary execution:
  - `go run ./cmd/server`
- Default embedded Docker image build for source-based local deployment via `Dockerfile.embedded`
- Default container orchestration entrypoint via `docker-compose.yml`, which pulls `rustyllh/ox-cli-proxy-api:latest`
- Rust data plane local dev stack via `Makefile`
- Production deployment details beyond Docker and local binary are `待确认`

## Current Known Constraints

- Repository contains a large Go main server plus an in-progress Rust data plane integration.
- `runtime snapshot` is used to provide Go-managed runtime config to the Rust data plane.
- Some project docs exist in English, Chinese, and Japanese.
- Stable configuration format should not be changed silently.

## Uncertain / 待确认

- Whether the Rust data plane is already part of the main supported production deployment path or still optional / experimental.
- Whether a formal release process exists beyond Docker image builds and repository releases.
