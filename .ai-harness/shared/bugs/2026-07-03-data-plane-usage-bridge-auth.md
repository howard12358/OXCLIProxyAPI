# Data-plane Usage Bridge RESP Auth Mismatch

## Symptom

- In `make dev-stack-url`, Go repeatedly logged:
  - `data-plane usage bridge subscribe failed: expected simple string prefix '+', got '-'`
- Rust data-plane usage queue still worked through direct RESP `AUTH test-management-key` and HTTP pop.

## Impact

- Go CPA could not maintain the preferred RESP `SUBSCRIBE usage` bridge to Rust in external `dev-stack-url` mode.
- The fallback HTTP pop path could still consume records, but logs were noisy and the preferred streaming bridge was unavailable.
- The original Go RESP reader hid the Rust error payload, so the log did not show the real `ERR invalid password` response.

## Root Cause

- Rust RESP auth uses the Rust `--snapshot-bearer-token` value.
- Embedded data-plane mode passes the same generated token into Go as a local management password, so the bridge can authenticate.
- External `dev-stack-url` mode started Rust with `MANAGEMENT_KEY=test-management-key`, but Go was not started with the same plaintext key in `MANAGEMENT_PASSWORD`.
- Go bridge only used `s.localPassword`, which is empty in external mode, so it sent an empty/incorrect `AUTH` password to Rust.

## Fix

- Go bridge auth now prefers the embedded/local password and falls back to the plaintext `MANAGEMENT_PASSWORD` environment variable.
- `make dev-stack` and `make dev-stack-url` now start Go with `MANAGEMENT_PASSWORD="$(MANAGEMENT_KEY)"`.
- Go RESP simple-string reader now returns RESP error frame contents, so auth failures log as `ERR invalid password` instead of only `got '-'`.

## Validation

- `go test ./internal/api -run 'TestDataPlaneUsageBridge'`
- `go build -o /tmp/cli-proxy-api-check ./cmd/server && rm -f /tmp/cli-proxy-api-check`
- Manual `make dev-stack-url` smoke in a single shell:
  - Go `/healthz` returned OK.
  - Rust `/readyz` returned ready.
  - Go no longer logged `ERR invalid password` or `expected simple string prefix '+', got '-'` after Rust became ready.
  - A Go `/v1/responses` request routed to Rust produced a failed usage record that was readable from Go `/v0/management/usage-queue`.
