# Commands

## Dependency / Setup

- Go modules download happens automatically via normal Go tooling.
- Docker build downloads Go modules during image build.
- Explicit install command: `待确认`

## Local Start

Go server:

```bash
go run ./cmd/server
go run ./cmd/server --config <path>
```

Common flags supported by the main server include:

```bash
--config <path>
--tui
--standalone
--local-model
--no-browser
--oauth-callback-port <port>
```

Rust data plane local dev:

```bash
cargo run --manifest-path rust/cliproxy-data-plane/Cargo.toml -- --bind-addr 127.0.0.1:4100 --snapshot-url http://127.0.0.1:8317/v0/management/runtime-snapshot --snapshot-bearer-token test-management-key
```

Dev stack helpers:

```bash
make dev-stack
make dev-stack-url
make stop-stack
make restart-stack
make status-stack
make ps-stack
make kill-stack-orphans
make logs-stack
make logs-go
make logs-rust
```

## Build

Go:

```bash
go build -o cli-proxy-api ./cmd/server
go build -o /tmp/cli-proxy-api-check ./cmd/server && rm -f /tmp/cli-proxy-api-check
```

Rust:

```bash
cargo build --manifest-path rust/cliproxy-data-plane/Cargo.toml
```

## Test

Go:

```bash
go test ./...
go test -v -run TestName ./path/to/pkg
```

Rust:

```bash
cargo test --workspace --manifest-path rust/cliproxy-data-plane/Cargo.toml
cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test http_routes
```

Dev smoke:

```bash
make test-responses
```

## Format / Lint

Go:

```bash
gofmt -w .
```

Rust:

```bash
cargo fmt --manifest-path rust/cliproxy-data-plane/Cargo.toml
```

Lint command:

- `待确认`

## Docker

Build:

```bash
docker build -f Dockerfile.embedded -t rustyllh/ox-cli-proxy-api:latest .
docker build -f Dockerfile.embedded -t ox-cli-proxy-api:local .
```

Embedded Rust data-plane image:

```bash
docker build -f Dockerfile.embedded -t rustyllh/ox-cli-proxy-api:embedded .
```

Manual CI workflow:

```bash
gh workflow run docker-embedded-image.yml -f image_tag=v0.0.1 -f platforms=linux/amd64,linux/arm64 -f push=true
gh workflow run docker-embedded-image.yml -f dockerhub_namespace=<namespace> -f image_tag=v0.0.1 -f platforms=linux/amd64,linux/arm64 -f push=true
gh workflow run docker-embedded-image.yml -f image_tag=v0.0.2-debug -f platforms=linux/amd64 -f rust_profile=debug -f push=true
```

Compose:

```bash
docker compose up -d
docker compose down
docker compose logs -f
./docker-build.sh
```

## Deployment

- Binary run:

```bash
./cli-proxy-api --config config.yaml
```

- Default Docker / Compose deployment pulls `rustyllh/ox-cli-proxy-api:latest`; source-based local builds use `docker-build.sh` / `docker-build.ps1` with `Dockerfile.embedded`.
- Other deployment scripts or orchestration targets: `待确认`

## Snapshot / Runtime Inspection

```bash
make snapshot-stack
make snapshot-current
make snapshot-rs
make diff-snapshots
```
