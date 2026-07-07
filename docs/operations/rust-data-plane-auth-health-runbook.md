# Rust Data Plane Auth Health Runbook

## Scope

This runbook covers the single-node Rust data-plane runtime overlay used by `/v1/responses`.

It does not describe multi-instance coordination or Go-managed health reconciliation.

## What Exists

Rust now maintains an in-memory auth/model health overlay for `/v1/responses`.

It is fed by upstream failures and currently drives:

- RESP `errors` event publication
- auth/model cooldown decisions
- candidate filtering before upstream execution
- pre-commit retry skipping failed auths

The overlay is memory-only. Restarting the Rust data plane clears it.

## Observe Errors

Subscribe to the Rust RESP `errors` channel:

```bash
redis-cli -p 4100
AUTH <snapshot-bearer-token>
SUBSCRIBE errors
```

Each payload includes:

- `provider`
- `model`
- `auth_index`
- `scope`
- `status_code`
- `error_code`
- `message`
- `retry_after_ms`
- `cooldown_until`
- `quota_exceeded`
- `reason`

## Interpret Scope

- `scope=auth`
  - the auth itself is considered temporarily unavailable
  - typical causes: `401`, auth-level `403`, token invalidation
- `scope=model`
  - the auth is blocked only for the resolved model path
  - typical causes: `429 usage_limit_reached`, `404`, unsupported model, transient upstream failures

## Current Cooldown Semantics

- `401` -> auth-level cooldown `30m`
- `402/403` -> auth-level cooldown `30m`
- `404` -> model-level cooldown `12h`
- `429 usage_limit_reached` -> upstream reset hint when present, otherwise local fallback
- `408/500/502/503/504` -> model-level cooldown `60s`
- `400/422 model_not_supported` -> model-level cooldown `12h`

## Recovery Options

1. Wait for `cooldown_until` to expire.
2. Let a later successful request clear the same auth/model overlay entry.
3. Restart the Rust data plane to clear the memory-only overlay.
4. If the root cause is quota state in Go/CPA, use the existing Go-side quota reset workflow.

## Troubleshooting Checklist

1. Confirm the route is hitting Rust `/v1/responses`.
2. Subscribe to `errors` and reproduce the failing request.
3. Check whether the failure is `scope=auth` or `scope=model`.
4. Check whether `cooldown_until` is still in the future.
5. If all candidates appear blocked, confirm the snapshot auth pool itself is not already exporting `cooldown_until`.
6. If behavior differs from Go, compare the upstream error body with the Rust classification rules.
