# Bug: Rust Usage TTFT Overwrite And Missing Request Metadata

## Date

2026-07-06

## Symptom

- Rust `/v1/responses` request events in `cpa-usage-keeper` could show implausibly high `speed_tps`
  because `ttft_ms` often ended up almost equal to `latency_ms`.
- The same Rust events emitted empty `reasoning_effort` and only the default `service_tier`, even when
  the downstream request explicitly carried `reasoning.effort` and `service_tier`.

## Impact

- Keeper request detail pages overstated output speed, sometimes by orders of magnitude.
- Rust usage events lost request-level metadata that Go native CPA already exposes to usage sinks.

## Reproduction

- Send Codex `/v1/responses` traffic through the Rust data plane with usage queue enabled.
- Inspect keeper request events for successful responses.
- Observe many rows where `ttft_ms ~= latency_ms`, causing inflated `speed_tps`.
- Send a request body with `reasoning: { "effort": "high" }` and `service_tier: "priority"`;
  observe Rust usage events still report empty/default values.

## Root Cause

- `RequestTelemetry::mark_first_byte()` used `compare_exchange(false, true, ...)` but then
  unconditionally re-read the flag and overwrote `first_byte_ms` on every later call.
- Rust request telemetry never captured downstream `reasoning.effort` or `service_tier` from the
  `/v1/responses` request body, so final usage payload assembly fell back to empty/default values.

## Fix

- Make `mark_first_byte()` write `first_byte_ms` only on the successful first transition from
  `false -> true`.
- Extract request-level `reasoning.effort`, fallback `reasoning_effort`, and `service_tier` from
  `ResponsesRequest`.
- Store these values in request telemetry state and emit them into the CPA-shaped usage payload.

## Prevention

- Add a telemetry unit test asserting repeated `mark_first_byte()` calls do not change the first
  recorded offset.
- Add telemetry and route tests asserting Rust usage payloads preserve requested
  `reasoning_effort` and `service_tier`.

## Related Files

- `rust/cliproxy-data-plane/src/telemetry.rs`
- `rust/cliproxy-data-plane/src/responses.rs`
- `rust/cliproxy-data-plane/src/responses/handler.rs`
- `rust/cliproxy-data-plane/tests/http_routes.rs`
