# Bug: Rust Usage Auth Index And TTFT Mismatch

## Date

2026-07-06

## Symptom

- `cpa-usage-keeper` request events from Rust `/v1/responses` showed the correct OAuth email in
  `source`, but still rendered the source row with the deleted badge.
- The same Rust events could show implausibly high `speed_tps` values because `ttft_ms` was almost
  equal to `latency_ms`.

## Impact

- Keeper could not resolve Rust usage events back to active CPA usage identities even when the
  request was served by a live auth file.
- Request event diagnostics overstated output speed for some streaming Codex responses, which made
  keeper's per-request speed display misleading.

## Reproduction

- Route Codex CLI traffic through embedded Rust `/v1/responses`.
- Inspect keeper request events for successful Rust SSE requests.
- Observe `source` populated, but keeper still tags the row as deleted and shows extreme
  `speed_tps` values when `ttft_ms` nearly equals `latency_ms`.

## Root Cause

- Go native usage reporting emits `auth_index = auth.EnsureIndex()`, but Rust usage telemetry
  emitted `auth_index = execution_plan.auth_id`.
- Keeper resolves source rows against active usage identities by `auth_index`, so a raw auth ID
  does not match the synced identity row.
- Rust streaming TTFT was recorded only when the SSE framer emitted the first complete frame.
  When upstream chunk boundaries delayed frame completion, the recorded TTFT drifted toward total
  latency.

## Fix

- Export stable `auth_index` in the Go runtime snapshot auth record.
- Preserve `auth_index` in Rust snapshot types and emit it in Rust usage payloads.
- Keep `auth_id` only for Rust routing/execution decisions; do not reuse it as keeper-facing
  `auth_index`.
- Record streaming TTFT when the first upstream body chunk arrives, before SSE frame repair emits
  a complete frame downstream.

## Prevention

- Snapshot exporter tests assert the Rust auth snapshot contains a stable `auth_index`.
- Rust telemetry and route tests assert the emitted usage payload uses snapshot `auth_index`.
- Cross-runtime tests continue validating Rust usage payload shape against CPA-compatible fields.

## Related Files

- `internal/dataplane/snapshot/exporter.go`
- `rust/cliproxy-data-plane/crates/common-types/src/lib.rs`
- `rust/cliproxy-data-plane/src/telemetry.rs`
- `rust/cliproxy-data-plane/src/responses/upstream.rs`
- `rust/cliproxy-data-plane/tests/http_routes.rs`
