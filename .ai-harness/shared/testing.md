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

Rust SSE parity test:

```bash
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml responses::tests::sse_framer_matches_go_parity_fixtures -- --exact
```

## Manual Validation

- Start Go/Rust local stack:

```bash
make dev-stack
```

or:

```bash
make dev-stack-url
```

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

- Smoke `/v1/responses`:

```bash
make test-responses
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
- Cross-runtime Go->Rust integration changes:
  - relevant Go package tests
  - Rust route/workspace tests
  - manual `make dev-stack` or `make dev-stack-url` verification when practical
- Deployment / embedded runtime verification changes:
  - syntax-check or dry-run the smoke script when changed
  - run `./scripts/embedded-smoke.sh` against a real embedded deployment when practical

## Current Coverage Risks

- Full provider behavior matrix is broad; not every path is likely covered equally.
- Multi-instance Rust data-plane lifecycle coverage is `待确认`.
- Some validation currently depends on local manual stack checks and snapshot inspection.
- Rust SSE parity coverage is better for selected Go stream-repair and malformed-stream samples than before, but the fixture set is still partial rather than exhaustive.

## If Tests Are Missing

- Add focused tests near touched modules.
- Prefer route-level tests for Rust data-plane behavior.
- Prefer package-level tests for Go management / snapshot / notifier behavior.
