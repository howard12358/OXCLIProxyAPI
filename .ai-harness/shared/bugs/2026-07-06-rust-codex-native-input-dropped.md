# Bug: Rust Codex Native Input Dropped

## Date

2026-07-06

## Symptom

- Embedded Rust data-plane `/v1/responses` returned `502 Bad Gateway` for real Codex CLI traffic.
- Rust upstream logs showed chatgpt.com rejected the forwarded request:
  - `Missing required parameter: 'input'.`
- The upstream dispatch log body had `model`, `instructions`, `store`, and `stream`, but no `input`.

## Impact

- Production-style embedded Rust `/v1/responses` could not handle Codex CLI requests that sent native Responses array input.
- Extra top-level Codex request fields such as `tools`, `include`, and `text` were also not preserved by the Rust typed request path.

## Reproduction

- Run Codex CLI `0.142.5` against an embedded Rust data-plane route.
- Go request error logs show the downstream request includes top-level `input` as an array.
- Rust upstream logs show the forwarded request omits `input` before calling `https://chatgpt.com/backend-api/codex/responses`.

## Root Cause

- Rust Codex normalization handled legacy string input with `request.input.take()`.
- When `input` was a native Responses array, the pattern did not match `Value::String`, but `take()` had already removed the field.
- `ResponsesRequest` only modeled a small fixed set of fields, so unknown but valid top-level Codex fields were dropped during deserialize/serialize.

## Fix

- Preserve non-string `input` values during Codex upstream normalization.
- Continue lifting legacy string `input` into Codex message array shape.
- Add a flattened `extra` field to preserve unknown top-level request fields through the Rust data-plane path.

## Prevention

- Keep route-level coverage for Codex native array input and extra fields.
- Avoid destructive `Option::take()` pattern matching unless unmatched values are explicitly restored.

## Related Files

- `rust/cliproxy-data-plane/src/responses.rs`
- `rust/cliproxy-data-plane/src/responses/protocol.rs`
- `rust/cliproxy-data-plane/tests/http_routes.rs`

## Related Tests

- `responses::tests::request_ir_preserves_codex_native_input_and_extra_fields`
- `responses_route_preserves_codex_native_input_and_extra_fields`
