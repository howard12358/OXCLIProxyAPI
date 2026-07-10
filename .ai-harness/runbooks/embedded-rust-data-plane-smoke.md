# Embedded Rust Data Plane Smoke Runbook

This runbook validates the default embedded Rust data-plane deployment path.

> **Execution status (2026-07-10):** The repository workflow now builds and verifies
> `linux/amd64` and `linux/arm64` manifests. The currently published `latest` tag must
> be republished through `docker-embedded-image` before Apple Silicon validation can pass.

## Prerequisites

- Docker Engine and `docker compose` (v2) installed on Linux or macOS.
- `curl` or equivalent HTTP client.
- A valid `CPA_API_KEY` exported in your shell for the smoke requests.
- Valid `config.yaml` with OAuth credentials and `auths/` directory.

## Start the Stack

```bash
docker compose up -d
```

Wait 10-20 seconds for the Go management plane and embedded Rust data-plane to bootstrap.

Before an Apple Silicon deployment, verify the published manifest:

```bash
docker buildx imagetools inspect rustyllh/ox-cli-proxy-api:latest
```

The output must list both `linux/amd64` and `linux/arm64`. If it does not, republish
with the `docker-embedded-image` workflow; do not set a permanent Compose platform override.

## Verify Go Management Plane

```bash
curl http://127.0.0.1:8317/healthz
curl http://127.0.0.1:8317/readyz
```

Both should return HTTP 200 with JSON status.

## Verify Rust Data Plane Readiness

Rust logs are mirrored into the container stdout with `[rs-stdout]` / `[rs-stderr]` prefixes:

```bash
docker compose logs -f cli-proxy-api
```

Look for lines containing `data plane listening` and `runtime snapshot applied`.

## Non-Streaming `/v1/responses` Smoke

```bash
curl http://127.0.0.1:8317/v1/responses \
  -H "Authorization: Bearer $CPA_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-5-codex",
    "input": "hello",
    "stream": false
  }'
```

Expected: HTTP 200 JSON response containing `response.status` == `"completed"`.

## Streaming `/v1/responses` Smoke

```bash
curl -N http://127.0.0.1:8317/v1/responses \
  -H "Authorization: Bearer $CPA_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-5-codex",
    "input": "hello",
    "stream": true
  }'
```

Expected: SSE stream starting with `event: response.created` and ending with `event: response.completed`.

## Verify Usage Queue

```bash
curl "http://127.0.0.1:8317/v0/management/usage-queue?count=10"
```

Expected: JSON array with at least one CPA-shaped usage payload for the request you just sent.

## Inspect Logs on Failure

```bash
# Recent container logs
docker compose logs --tail=200 cli-proxy-api

# Follow logs while reproducing
docker compose logs -f cli-proxy-api
```

Search for:
- `[rs-stderr]` or `ERROR` for Rust data-plane errors.
- `upstream` or `codex` for request-routing issues.
- `usage queue` for usage-payload problems.

## Roll Back to Go Native Path

Set `data-plane.mode: disabled` in the runtime config (or env equivalent) and restart:

```bash
docker compose down
docker compose up -d
```

When disabled, `/v1/responses` is handled by the Go-native executor instead of the embedded Rust data-plane.

## Shutdown

```bash
docker compose down
```

## Automation

The same checks are automated in `./scripts/embedded-smoke.sh` for CI or repeated validation:

```bash
MANAGEMENT_KEY=<management-key> \
API_KEY=<api-key> \
CPA_BASE_URL=http://127.0.0.1:18317 \
KEEPER_URL=http://127.0.0.1:28081 \
CONTAINER_NAME=ox-cli-proxy-api \
./scripts/embedded-smoke.sh
```

The script validates response structure, SSE `response.created`/`response.completed`,
the expected usage `executor_type`, usage queue delivery, and Rust log prefixes. Set
`KEEPER_URL` only when the optional keeper reachability check is required.

## Disabled-to-Embedded Fallback Smoke

Prepare two complete configuration files using the same auth and usage settings:

- `disabled.yaml` contains `data-plane.mode: disabled`.
- `embedded.yaml` omits `data-plane` or selects `data-plane.mode: embedded`.

Then run the repeatable two-pass smoke:

```bash
EMBEDDED_CONFIG=/absolute/path/embedded.yaml \
DISABLED_CONFIG=/absolute/path/disabled.yaml \
MANAGEMENT_KEY=<management-key> \
API_KEY=<api-key> \
./scripts/data-plane-mode-smoke.sh
```

It starts disabled mode first and requires `CodexExecutor` usage without Rust child logs;
then it restarts in embedded mode and requires `RustResponsesExecutor` plus `[rs-*]` logs.
