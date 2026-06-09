# cliproxy-data-plane

Rust sidecar scaffold for CLIProxyAPI data-plane responsibilities.

## Current scope

- Binary crate with `tokio` runtime
- `axum` HTTP server
- Health endpoints: `/healthz`, `/readyz`
- CLI/env configuration for bind address and log level

## Run

```bash
cargo run -- --bind-addr 127.0.0.1:4100
```

Environment variables:

- `CLIPROXY_BIND`
- `CLIPROXY_LOG`

## Next steps

- Add Unix socket or gRPC control channel from Go
- Move streaming relay and websocket session handling here
- Keep Go as control plane and Rust as data plane
