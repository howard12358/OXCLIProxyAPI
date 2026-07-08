# Testing

## Automated Test Commands

Go full test:

```bash
go test ./...
```

Go targeted test:

```bash
go test -v -run TestName ./path/to/pkg
```

Go targeted packages used in recent work:

```bash
go test ./internal/dataplane/notifier ./internal/dataplane/snapshot ./internal/api/handlers/management
```

Rust workspace test:

```bash
cargo test --workspace --manifest-path rust/cliproxy-data-plane/Cargo.toml
```

Rust route-focused test:

```bash
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test http_routes
```

Rust auth/queue focused tests:

```bash
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml usage_queue::tests::subscribe_errors_receives_error_payload -- --exact
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml auth_state::tests::clears_expired_entries_on_lookup -- --exact
```

Rust SSE parity test:

```bash
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml responses::tests::sse_framer_matches_go_parity_fixtures -- --exact
```

Cross-runtime snapshot contract tests:

```bash
go test ./internal/dataplane/snapshot -run TestBuildRuntimeSnapshotGolden -count=1
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test contract
```

Rust SSE contract tests:

```bash
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test contract sse_golden
```

Rust auth retry contract tests:

```bash
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test contract auth_retry
```

Rust `/v1/responses` golden contract tests:

```bash
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test contract responses_golden
```

Rust `/v1/responses` stream abort contract tests:

```bash
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test contract stream_abort
```

Go-native `/v1/responses` golden contract test:

```bash
go test ./sdk/api/handlers/openai -run TestOpenAIResponsesNativeCodexMatchesSharedGoldenFixture -count=1
```

Rust usage queue contract tests:

```bash
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test contract usage_queue
```

Go data-plane proxy and usage bridge tests:

```bash
go test ./internal/api -run 'TestDataPlaneUsageBridge|TestResponsesRouteProxiesToDataPlaneWhenConfigured|TestCodexDirectResponsesRouteProxiesToDataPlaneWhenConfigured|TestResponsesRouteUsesUpdatedRuntimeDataPlaneBaseURL' -count=1
```

## Manual Validation

- Start Go/Rust local stack:

```bash
make dev-stack
```

This is the preferred local validation path because it exercises the embedded Rust data-plane lifecycle used by production-like deployments.

- Check Go health:

```bash
curl http://127.0.0.1:8317/healthz
```

- Check Rust readiness:

```bash
curl http://127.0.0.1:4100/readyz
```

- Check Go snapshot:

```bash
curl http://127.0.0.1:8317/v0/management/runtime-snapshot \
  -H 'Authorization: Bearer test-management-key'
```

- Check Rust applied snapshot:

```bash
curl http://127.0.0.1:4100/v0/runtime/snapshot
```

- Diff current Go/Rust snapshots:

```bash
make diff-snapshots
```

- Production-style embedded smoke:

```bash
MANAGEMENT_KEY=<management-key> \
API_KEY=<api-key> \
CPA_BASE_URL=http://127.0.0.1:18317 \
KEEPER_URL=http://127.0.0.1:28081 \
CONTAINER_NAME=ox-cli-proxy-api \
./scripts/embedded-smoke.sh
```

## Recommended Commands Before / After Changes

Before changes:

- Inspect current worktree:

```bash
git status --short
```

After Go changes:

```bash
gofmt -w .
go build -o /tmp/cli-proxy-api-check ./cmd/server && rm -f /tmp/cli-proxy-api-check
go test ./...            # or narrower relevant packages
```

After Rust changes:

```bash
cargo fmt --manifest-path rust/cliproxy-data-plane/Cargo.toml
cargo test --workspace --manifest-path rust/cliproxy-data-plane/Cargo.toml
```

## Which Changes Need Which Tests

- Go API / management / snapshot changes:
  - relevant Go package tests
  - Go server build verification
- Rust HTTP / runtime / upstream changes:
  - relevant Rust test target or full workspace test
  - SSE framing/repair changes should also run `cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test contract sse_golden`
  - `/v1/responses` output-shape changes should also run `cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test contract responses_golden`
- Rust auth health / cooldown / errors-channel changes:
  - full Rust workspace test or at minimum `http_routes` plus focused queue/auth-state tests
  - auth retry/cooldown semantic changes should also run `cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test contract auth_retry`
- Cross-runtime Go->Rust integration changes:
  - relevant Go package tests
  - Rust route/workspace tests
  - snapshot-boundary changes should also run the shared Go/Rust snapshot contract tests
  - usage bridge or Rust usage queue protocol changes should also run the Rust usage queue contract tests and Go data-plane usage bridge tests
  - manual `make dev-stack` verification first when practical
- Deployment / embedded runtime verification changes:
  - syntax-check or dry-run the smoke script when changed
  - run `./scripts/embedded-smoke.sh` against a real embedded deployment when practical

## Current Coverage Risks

- Full provider behavior matrix is broad; not every path is likely covered equally.
- Multi-instance Rust data-plane lifecycle coverage is `待确认`.
- Some validation currently depends on local manual stack checks and snapshot inspection.
- The shared runtime snapshot contract fixtures currently cover Go exporter -> Rust parse/validate only; they do not yet cover schema version migration or broader negative fixtures.
- The `/v1/responses` shared golden fixture now pins a Go-native Codex executor path and the Rust data-plane path against the same request/response contract; broader provider and streaming golden coverage is still partial.
- Rust `/v1/responses` stream abort coverage now exercises both the streaming downstream path and the aggregate (stream=false over Codex SSE) path when the upstream connection drops mid-stream; the scenario matrix remains partial.
- Rust auth health overlay currently has unit coverage and route coverage around retry paths, but it is still memory-only and not exercised under multi-process or restart scenarios.

## If Tests Are Missing

- Add focused tests near touched modules.
- Prefer route-level tests for Rust data-plane behavior.
- Prefer package-level tests for Go management / snapshot / notifier behavior.
