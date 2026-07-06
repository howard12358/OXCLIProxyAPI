# Bug: Rust Usage Attribution Missing

## Date

2026-07-06

## Symptom

- `cpa-usage-keeper` request event details showed Rust `/v1/responses` events, but the
  source/API-key attribution was empty or marked as deleted.
- Keeper analysis pages could show latency data while token/cost/distribution sections had
  missing data for the same exported events.

## Impact

- Rust data-plane usage events could be recorded with token counts but not reliably grouped by
  downstream API key or credential identity.
- Identity-level stats in keeper did not align with the selected Codex auth even when
  `auth_index` matched the auth file name.

## Reproduction

- Send Codex CLI traffic through embedded Rust `/v1/responses`.
- Export keeper request events.
- Events contain `input_tokens`, `output_tokens`, and `total_tokens`, but `source`,
  `source_type`, and `cpa_api_key_id` are empty and `is_identity_deleted` is true.

## Root Cause

- Rust usage payloads set `source` and `api_key` to empty strings.
- The runtime snapshot did not export the Go-resolved usage source, such as the OAuth email
  returned by `Auth.AccountInfo()`.
- Rust HTTP handling did not capture the downstream CPA API key from request headers before
  forwarding to upstream.

## Fix

- Add `usage_source` to runtime snapshot auth records.
- Populate `usage_source` in Go from the selected auth's account info.
- Preserve `usage_source` in Rust common snapshot types.
- Capture downstream API keys from Rust `/v1/responses` request headers.
- Emit both `source` and `api_key` in Rust CPA-shaped usage payloads.

## Prevention

- Route-level Rust tests assert that usage queue payloads include source and downstream API key.
- Snapshot exporter tests assert Codex OAuth auths include `usage_source`.

## Related Files

- `internal/dataplane/snapshot/exporter.go`
- `rust/cliproxy-data-plane/crates/common-types/src/lib.rs`
- `rust/cliproxy-data-plane/src/http.rs`
- `rust/cliproxy-data-plane/src/telemetry.rs`
- `rust/cliproxy-data-plane/tests/http_routes.rs`

## Related Tests

- `go test ./internal/dataplane/snapshot`
- `cargo test --manifest-path rust/cliproxy-data-plane/Cargo.toml --test http_routes responses_route_usage_payload_includes_source_and_downstream_api_key -- --exact`
