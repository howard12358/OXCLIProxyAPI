# Contract Coverage

This document records what each contract test file actually covers, verified against
source code (not docs). All test files live under `rust/cliproxy-data-plane/tests/`.

Last updated: 2026-07-09

---

## 1. request_emission.rs

**1 test: `codex_request_emission_matches_golden_fixtures`**

Walks `testdata/contract/responses/request_emission/`, finds all `*.request.json` files,
and for each drives a full `/v1/responses` call through a mock upstream that captures
the actual JSON request body sent by Rust. Normalized keys compared against `*.expected.json`.

### Covered (10 fixture pairs)

| Fixture | What It Validates |
| ------- | ----------------- |
| `input_string` | `"input": "hello"` lifted to `[{"role":"user","content":"hello"}]` |
| `input_messages_array` | messages-array input preserved with model resolved to `gpt-5-codex` |
| `system_role_to_developer` | `role: "system"` rewritten to `role: "developer"`, order preserved |
| `reasoning_effort` | `reasoning: {effort: "medium"}` extracted from request extras |
| `service_tier` | `service_tier: "priority"` preserved in upstream request |
| `tools_empty_parallel_removed` | empty `tools: []` + `parallel_tool_calls: true` → stripped |
| `tools_non_empty_parallel_preserved` | non-empty tools array + `parallel_tool_calls` preserved |
| `include_encrypted_content_injected` | `include: [{encrypted_content: ...}]` injected |
| `unsupported_generation_fields_removed` | `temperature`, `top_p`, `max_output_tokens` stripped |
| `web_search_preview_normalized` | `web_search_preview` builtin tool alias → `web_search` |

### Not Covered

- Empty `instructions` default (no dedicated fixture; implicitly tested in other fixtures)
- `builtin tool alias` normalization beyond `web_search_preview`
- Error responses to Codex (only happy-path 200)
- `context` field stripping (no fixture)

---

## 2. snapshot_schema.rs

**10 tests: 1 positive + 9 negative**

Uses `load_invalid_snapshot()` to read `testdata/contract/runtime_snapshot.invalid_*.json`,
parses through `serde_json`, then calls `validate_snapshot()` and asserts the error contains
the expected field name.

| Test | Fixture | Asserted Field |
| ---- | ------- | -------------- |
| `parses_go_exported_runtime_snapshot_golden` | `runtime_snapshot.codex.golden.json` | parse + validate passes |
| `rejects_..._missing_version_fixture` | `invalid_missing_version.json` | `snapshot.version` |
| `rejects_..._missing_generated_at` | `invalid_missing_generated_at.json` | `snapshot.generated_at` |
| `rejects_..._missing_source_instance_id` | `invalid_missing_source_instance_id.json` | `snapshot.source_instance_id` |
| `rejects_..._codex_missing_access_token` | `invalid_codex_missing_access_token.json` | `execution.codex.access_token` |
| `rejects_..._empty_auth_index` | `invalid_empty_auth_index.json` | `auth_index` |
| `rejects_..._empty_model_alias_target` | `invalid_empty_model_alias_target.json` | `model_aliases` |
| `rejects_..._provider_missing_model` | `invalid_provider_missing_model.json` | `models` |
| `rejects_..._empty_provider_key` | `invalid_empty_provider_key.json` | `providers` |
| `rejects_..._route_missing_target` | `invalid_route_missing_target.json` | `listeners.public_http` |

### Not Covered

- Schema version migration (snapshot struct field additions/removals)
- Missing `auth_pool` entirely
- Missing `listeners` section entirely
- `routing.strategy` invalid enum values
- `usage_queue` missing `backend` when `enabled=true`

---

## 3. stream_abort.rs

**3 tests**

| Test | Scenario | What It Verifies |
| ---- | -------- | ---------------- |
| `stream_true_aborts_after_created_emits_error_frame` | upstream SSE drops mid-stream on stream=true path | Rust emits `response.error` SSE frame; status=200 |
| `stream_false_aggregate_aborts_after_created_returns_bad_gateway` | upstream SSE drops mid-stream on stream=false (aggregate) path | Rust returns HTTP 502 |
| `downstream_client_drop_cancels_upstream_stream` | downstream client reads 2 SSE frames then drops | upstream mock observes write error; usage queue contains no success record |

### Coverage Details

- `downstream_client_drop_cancels_upstream_stream` starts a real `axum::serve` listener,
  connects via `reqwest` client, reads 2 frames, drops response, waits for upstream
  `AtomicBool` to be set via write error. Then checks `usage_queue.pop_oldest_json(10)`
  has no success-completed record.
- `spawn_slow_infinite_sse_upstream()` sends HTTP/1.1 headers + SSE frames in a loop,
  detecting `write_all` errors.
- Does NOT explicitly verify "no post-commit auth retry" (single auth in test, no retry
  candidates configured, retry would only trigger if auths > 1 and first fails).

### Not Covered

- Multi-auth retry on abort (single auth only)
- Connection count leak over repeated aborts (single iteration)
- Abort mid-header (before first SSE frame)

---

## 4. home_usage_lpush.rs

**1 test: `home_mode_lpush_usage_to_external_redis_queue`**

Full end-to-end: real `/v1/responses` request → Codex mock upstream returns SSE →
Rust usage telemetry fires → external LPUSH via RESP to a fake Redis server.

### Covered

- Fake RESP Redis (custom `parse_resp_array` parser) captures `AUTH usage-secret`
- Captures `LPUSH usage <JSON payload>`
- Payload verified: `provider="codex"`, `model="gpt-5-codex"`, `source="auth-codex-a@example.test"`
- External LPUSH is fire-and-forget (tokio::spawn); test polls for up to 2.5s
- Rust is configured via both `UsageQueue::set_external_config()` and `RuntimeSnapshot.usage_queue.external`

### Not Covered

- External Redis unreachable (should not block `/v1/responses`)
- Wrong password (should warn but not fail request)
- Multiple concurrent LPUSH calls

---

## 5. usage_queue.rs (contract)

**3 tests**

| Test | Scenario |
| ---- | -------- |
| `http_usage_queue_pop_is_fifo_and_consuming` | 2 real `/v1/responses` requests → HTTP GET `/v0/management/usage-queue?count=1` twice → assert FIFO order; third pop returns empty |
| `resp_lpop_rpop_and_auth_follow_usage_contract` | Pre-enqueue 2 records → RESP AUTH fail → RESP AUTH pass → LPOP → RPOP → empty LPOP returns `$-1` |
| `resp_subscribe_receives_usage_without_buffering_http_fallback_copy` | RESP SUBSCRIBE `usage` → receive `support_refresh` control message → enqueue → receive broadcast → assert local queue is empty (subscriber bypass) |

### Coverage Details

- `spawn_redis_protocol()` starts real `redis_protocol::handle_connection` on a TCP listener
- RESP client uses `TcpStream::write_all` + `read_resp_text_until` with `\r\n` CRLF format
- `parse_bulk_json` / `parse_last_bulk_json` extract JSON from RESP bulk strings
- Does NOT test HTTP fallback explicitly (subscriber test asserts local queue empty, implying subscriber consumed it)

### Not Covered

- Multiple concurrent subscribers
- RESP `RPOP` with count argument (currently `RPOP usage 1` not tested)
- RESP subscribe `errors` channel
- RESP `UNSUBSCRIBE`

---

## 6. responses_golden.rs

**1 test: `non_stream_response_matches_golden_fixture`**

Loads `testdata/contract/responses/non_stream_aggregates_codex_stream.json` fixture.
Sends request to Rust `/v1/responses` with `stream: false`. The Codex path forces
`stream: true` to upstream, then aggregates the SSE stream back to a JSON response.
Result is compared to `expected_response` in the fixture — exact JSON match.

### Covered

- Non-stream Codex response aggregation end-to-end
- `spawn_openai_upstream()` mock returns both stream and non-stream responses

### Not Covered

- Stream=true golden (SSE golden is handled by `sse_golden.rs`)
- Different model providers
- Error response golden fixtures

---

## 7. sse_golden.rs

**2 tests**

| Test | Fixture | What It Verifies |
| ---- | ------- | ---------------- |
| `stream_repairs_completed_output_against_golden_fixture` | `stream_repairs_completed_output.json` | Rust SSE framer repairs `response.completed` output |
| `stream_preserves_done_and_non_json_frames_against_golden_fixture` | `stream_preserves_done_and_non_json.json` | Rust SSE framer preserves `[DONE]` and non-JSON frames |

Both load a raw upstream SSE payload, serve it through a mock upstream, and compare
Rust's stream=true output (after SSE framing) byte-for-byte with `expected_body`.

---

## 8. auth_retry.rs

**3 tests, each loading a JSON fixture from `testdata/contract/auth/`**

| Test | Fixture |
| ---- | ------- |
| `retries_next_auth_after_401_fixture` | `retry_on_401.json` |
| `retries_next_auth_after_403_fixture` | `retry_on_403.json` |
| `retries_next_auth_after_usage_limit_fixture` | `retry_on_429_usage_limit.json` |

### Coverage Details

- 2 auths configured: `auth-codex-a` (primary) and `auth-codex-b` (fallback)
- Mock upstream returns fixture-specified status on first auth, succeeds on second
- Verifies both auths were seen in order (Bearer codex-token-a, then Bearer codex-token-b)
- Verifies `cooldown_scope`: `auth` (401) blocks entire auth, `model` (429) blocks model only
- Verifies second request skips primary auth when `expect_second_request_skips_primary=true`
- Uses `AuthStateOverlay` directly to query cooldown state

### Not Covered

- Retry chain with 3+ auths
- Retry on timeout/connection errors
- Cooldown expiry over time
- RESP errors channel emission on retry

---

## 9. http_routes.rs

**22 tests** covering the full `/v1/responses` Rust data-plane HTTP surface:

- Route enable/disable (`responses_route_returns_not_found_when_disabled`)
- Snapshot management (`snapshot_notify_triggers_runtime_refresh`, `runtime_snapshot_endpoint_returns_applied_snapshot`)
- Usage queue (`usage_queue_endpoint_pops_requested_records_once`)
- Upstream routing (`responses_route_returns_bad_gateway_when_no_real_upstream_is_available`)
- Codex upstream execution (`responses_route_resolves_codex_alias_before_upstream_execution`, `responses_route_executes_selected_codex_oauth_auth_end_to_end`, `responses_route_executes_auth_bound_codex_upstream_without_global_token`)
- Codex failover (`responses_route_fails_over_on_codex_quota_exhaustion`)
- Codex payload normalization (`responses_route_normalizes_codex_payload_for_upstream`, `responses_route_preserves_codex_native_input_and_extra_fields`, `responses_route_strips_codex_unsupported_generation_fields`, `responses_route_applies_codex_compatibility_rewrites`)
- Usage telemetry (`responses_route_usage_payload_includes_source_and_downstream_api_key`, `responses_route_usage_payload_includes_reasoning_effort_and_service_tier`)
- Stream/aggregate (`responses_route_aggregates_codex_stream_for_non_stream_clients`, `responses_streaming_prefers_real_openai_upstream`, `responses_stream_repairs_completed_output_from_split_upstream_frames`)
- Auth retry (`responses_route_retries_next_auth_after_retryable_codex_failure`, `responses_route_keeps_failed_primary_auth_in_cooldown_after_successful_failover`)
- Metrics (`metrics_endpoint_is_not_exposed`)

---

## 10. common/mod.rs

**Test helper library** — provides shared fixtures for all contract tests:

| Function | Purpose |
| -------- | ------- |
| `test_runtime()` | Creates `RuntimeStateHandle` with minimal valid snapshot |
| `test_runtime_with_auths()` | Same + custom auth pool + routing strategy |
| `codex_oauth_auth()` | Builds `AuthRecord` for Codex OAuth with configurable token/base_url |
| `test_upstream()` | `UpstreamRuntime` with default (disabled) config |
| `openai_upstream(base_url)` | `UpstreamRuntime` pointing at OpenAi API base URL |
| `codex_upstream(base_url)` | `UpstreamRuntime` pointing at Codex API base URL |
| `spawn_openai_upstream()` | Mock Axum upstream that returns predictable JSON/SSE |
| `spawn_codex_failover_upstream()` | Mock upstream that 401s on `auth-codex-a`, succeeds on others |
| `spawn_codex_quota_failover_upstream()` | Mock upstream that 429s on `auth-codex-a`, succeeds on others |

---

## 11. contract.rs

Module registrar. Declares `mod common;` and `#[path]` attributes for each contract test module.
Does not contain test logic itself.

---

## Summary Matrix

| Contract File | Tests | Scope |
| ------------- | ----- | ----- |
| `request_emission.rs` | 1 (10 fixtures) | Codex upstream request body golden |
| `snapshot_schema.rs` | 10 (1+9) | RuntimeSnapshot parse + validate |
| `stream_abort.rs` | 3 | Upstream abort + downstream abort + aggregate abort |
| `home_usage_lpush.rs` | 1 | External RESP LPUSH usage via real request |
| `usage_queue.rs` | 3 | HTTP pop + RESP LPOP/RPOP/AUTH + SUBSCRIBE |
| `responses_golden.rs` | 1 | Non-stream Codex response aggregation golden |
| `sse_golden.rs` | 2 | SSE frame repair + [DONE] preservation |
| `auth_retry.rs` | 3 | 401/403/429 retry + cooldown scope + skip-primary |
| `http_routes.rs` | 22 | Full `/v1/responses` HTTP surface |
| `common/mod.rs` | helpers | Test runtime, auth, upstream mocks |
| `contract.rs` | registrar | Module declarations |
