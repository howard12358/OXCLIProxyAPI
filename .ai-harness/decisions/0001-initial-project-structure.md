# 0001-initial-project-structure.md

## Status

Accepted

## Context

当前仓库已经形成：

- Go 主服务作为主要代理和管理平面
- Rust 工作区作为可选数据平面
- YAML 配置 + 本地 / 可选远端存储的配置与鉴权模型
- Go 与 Rust 各自独立测试

需要把这些已有结构记录为初始决策，避免后续会话误判项目边界。

## Decision

- Primary technology stack:
  - Go `1.26+`
  - Rust workspace under `rust/cliproxy-data-plane`
- Current top-level structure:
  - `cmd/` server and utility entrypoints
  - `internal/` Go application internals
  - `sdk/` embeddable SDK
  - `rust/cliproxy-data-plane/` Rust data-plane workspace
  - `test/` Go integration-style tests
  - `docs/` repository docs
- Current entrypoints:
  - Go server: `cmd/server/main.go`
  - Rust data plane: `rust/cliproxy-data-plane/src/main.rs`
- Current configuration model:
  - `config.yaml`
  - `config.example.yaml`
  - `.env` auto-loaded
  - auth directory default under `~/.cli-proxy-api`
  - optional Postgres / Git / object store backends
- Current testing model:
  - Go package tests and `test/` integration tests
  - Rust workspace tests and route tests
- Current deployment model:
  - local binary run
  - Docker image build
  - `docker-compose.yml`
  - broader deployment model: `待确认`

## Consequences

- Future agents should preserve this split architecture unless an ADR supersedes it.
- Changes to entrypoints, config format, or deployment mode must be documented.
- Rust data-plane role should be treated carefully because repository state suggests active evolution.

## Alternatives Considered

- No ADR and rely on code layout only
  - rejected because cross-session agents can misread evolving structure
- Describe only Go server and ignore Rust workspace
  - rejected because Rust workspace is already part of the repository and influences current development
