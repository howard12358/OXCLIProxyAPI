# Embedded Rust Data Plane Release Gate

## Required Before Default Enable

### Code Quality
- [x] Rust `cargo fmt --all -- --check` passes
- [x] Rust `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- [x] Rust `cargo test --workspace --all-targets --all-features` passes
- [x] Rust contract tests pass (10 test files)
- [x] Go `go test ./...` and `go build ./...` pass

### Documentation
- [x] `contract-coverage.md` up to date
- [x] `current-state.md` reflects actual capabilities
- [x] `next-tasks.md` reflects current Release Gate phase
- [x] Fallback/disable ops doc exists (`docs/operations/rust-data-plane-fallback.md`)
- [x] Embedded smoke runbook exists (`.ai-harness/runbooks/embedded-rust-data-plane-smoke.md`)
- [x] `data-plane.mode: disabled` fallback path documented
- [ ] Embedded smoke executed in real Docker environment
- [ ] Fallback smoke executed or explicitly marked pending
- [x] Benchmark runbook exists

### CI / Automation
- [x] GitHub Actions CI for Rust fmt / clippy / tests (remote run success before strengthened command update)
- [x] GitHub Actions CI for Go full test / build (remote run success before strengthened command update)
- [x] Manual embedded Docker smoke workflow and fallback runner exist

## Default Enable Criteria

When all of the following are confirmed in a real deployment:

- `/healthz` returns `{"status":"ok"}`
- `/readyz` returns healthy with valid runtime snapshot
- `/v1/responses` `stream=false` returns correct JSON
- `/v1/responses` `stream=true` returns valid SSE stream
- `/v0/management/usage-queue` contains valid CPA-shaped payloads
- Logs show `RustResponsesExecutor` on the embedded path
- `data-plane.mode: disabled` falls back to `CodexExecutor`
- Downstream client abort does not leak upstream connections
- Snapshot validation rejects invalid snapshots
- Codex request emission golden tests pass

## Rollback Criteria

Roll back to Go native path if any of these occur:

- Rust child process cannot start (3 consecutive attempts)
- Rust `/readyz` remains degraded for > 60 seconds
- Snapshot refresh repeatedly fails (> 5 consecutive failures)
- `/v1/responses` 5xx rate exceeds 1%
- Stream abort leaks connections (connection count grows monotonically)
- Usage payloads disappear from the queue
- `executor_type` is inconsistent with config

Rollback steps:
```yaml
# config.yaml
data-plane:
  mode: disabled
```
```bash
docker compose down && docker compose up -d
```

## Not Required

- Real upstream load test (uses mock upstream only)
- Multi-instance Rust data-plane coordination
- Provider expansion beyond Codex
- Config format changes
