# Rust Data Plane Shutdown Was Not Logged

## Symptom

When the dev stack was stopped, the Go log recorded API shutdown, but `temp/dev-rust.log` ended at normal runtime messages and did not show that the Rust data-plane process was exiting.

## Impact

Operators could not distinguish a clean Rust data-plane stop from an abrupt process disappearance by reading the Rust log alone.

## Root Cause

The Rust data plane served an infinite TCP accept loop without a shutdown signal path, so SIGTERM / Ctrl-C did not have an application-level branch that logged shutdown intent and completion.

The dev `stop-stack` target also needed to terminate the real listener process before cleaning up pidfile parent processes, so the Rust binary receives SIGTERM directly and can run its shutdown branch.

## Fix

- Added a shutdown-aware Rust listener loop that exits on SIGTERM / Ctrl-C.
- Added Rust log lines for `shutdown signal received` and `data plane stopped`.
- Updated `make stop-stack` to send termination to listening Go/Rust processes before pidfile cleanup.

## Validation

- `cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml app::tests::serve_listener_returns_when_shutdown_is_requested -- --exact`
- `cargo fmt --manifest-path rust/cliproxy-data-plane/Cargo.toml`
- `cargo test --workspace --manifest-path rust/cliproxy-data-plane/Cargo.toml`
- Manual `make dev-stack-url && make stop-stack` verification showed Rust log lines:
  - `shutdown signal received signal="sigterm"`
  - `data plane stopped`
