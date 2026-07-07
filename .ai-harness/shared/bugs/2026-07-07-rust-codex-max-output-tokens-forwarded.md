# Bug: Rust Codex max_output_tokens forwarded upstream

## Date

2026-07-07

## Symptom

- Rust data-plane `/v1/responses` returned an upstream Codex 400 when downstream clients sent `max_output_tokens`.
- The upstream error body reported:
  - `Unsupported parameter: max_output_tokens`

## Impact

- Real Codex CLI traffic routed through the Rust data plane could fail even though the same request shape was already tolerated by the Go-native CPA Codex path.
- This created behavior drift between the embedded Rust `/v1/responses` path and the original CPA Codex request normalization path.

## Reproduction

- Send a Codex `/v1/responses` request through the Rust data plane with:
  - `max_output_tokens`
- Inspect the echoed upstream payload in the route tests or the production upstream error body.

## Root Cause

- Rust `ResponsesRequest` preserves unknown top-level request fields in `extra`.
- Codex request emission in `src/responses/protocol.rs` preserved those extra fields for provider `Codex` without applying the same compatibility stripping that already exists in Go CPA.
- As a result, unsupported generation-control fields such as `max_output_tokens` were forwarded unchanged to Codex upstream.

## Fix

- Add Codex-specific extra-field stripping in Rust request emission before upstream serialization.
- Match the existing Go Codex compatibility behavior for:
  - `max_output_tokens`
  - `max_completion_tokens`
  - `temperature`
  - `top_p`
  - non-`priority` `service_tier`

## Prevention

- Keep a unit test on Codex request normalization.
- Keep a route test that asserts the actual forwarded upstream payload no longer contains the stripped fields.
- When Rust preserves new top-level Responses fields, compare the Codex emission path against the Go-native Codex translator before treating them as safe passthrough.

## Related Files

- `rust/cliproxy-data-plane/src/responses/protocol.rs`
- `rust/cliproxy-data-plane/src/responses.rs`
- `rust/cliproxy-data-plane/tests/http_routes.rs`

## Related Tests

- `responses::tests::normalize_upstream_request_strips_codex_unsupported_generation_fields`
- `responses_route_strips_codex_unsupported_generation_fields`
