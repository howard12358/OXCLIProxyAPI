# Embedded Production Deployment

This document describes the minimal production deployment path for the `rusty`
branch when `/v1/responses` runs through the embedded Rust data plane.

## Scope

This is the intended single-host deployment path:

- one `CLIProxyAPI` container
- optional `cpa-usage-keeper`
- local `config.yaml`
- local `auths/`
- local `logs/`

It does not cover external Rust data-plane mode.

## Minimal CPA Compose

```yaml
services:
  cli-proxy-api:
    image: rustyllh/ox-cli-proxy-api:latest
    pull_policy: always
    container_name: ox-cli-proxy-api
    environment:
      DEPLOY: ${DEPLOY:-}
    ports:
      - "18317:8317"
    volumes:
      - ${CLI_PROXY_CONFIG_PATH:-./config.yaml}:/CLIProxyAPI/config.yaml
      - ${CLI_PROXY_AUTH_PATH:-./auths}:/root/.cli-proxy-api
      - ${CLI_PROXY_LOG_PATH:-./logs}:/CLIProxyAPI/logs
    restart: unless-stopped
```

## Minimal CPA + Usage Keeper Compose

```yaml
services:
  cli-proxy-api:
    image: rustyllh/ox-cli-proxy-api:latest
    pull_policy: always
    container_name: ox-cli-proxy-api
    environment:
      DEPLOY: ${DEPLOY:-}
    ports:
      - "18317:8317"
    volumes:
      - ${CLI_PROXY_CONFIG_PATH:-./config.yaml}:/CLIProxyAPI/config.yaml
      - ${CLI_PROXY_AUTH_PATH:-./auths}:/root/.cli-proxy-api
      - ${CLI_PROXY_LOG_PATH:-./logs}:/CLIProxyAPI/logs
    restart: unless-stopped

  cpa-usage-keeper:
    image: ghcr.io/willxup/cpa-usage-keeper:latest
    container_name: ox-cpa-usage-keeper
    restart: unless-stopped
    depends_on:
      - cli-proxy-api
    ports:
      - "28081:8080"
    environment:
      TZ: Asia/Shanghai
      CPA_BASE_URL: http://cli-proxy-api:8317
      CPA_PUBLIC_URL: http://YOUR_PUBLIC_HOST:18317
      CPA_MANAGEMENT_KEY: YOUR_MANAGEMENT_PASSWORD
      REDIS_QUEUE_ADDR: cli-proxy-api:8317
      AUTH_ENABLED: "true"
      LOGIN_PASSWORD: YOUR_KEEPER_LOGIN_PASSWORD
    volumes:
      - ./keeper:/data
```

## Required Local Files

- `config.yaml`
- `auths/`
- `logs/`

The embedded Rust data plane does not need a separate binary mount or a fixed
`/root/.local/state/cliproxy/data-plane` mount.

## Effective Embedded Defaults

On the `rusty` branch, if `data-plane` is omitted entirely:

- mode defaults to `embedded`
- bind address defaults to `127.0.0.1:4100`
- state directory defaults to the directory containing `CLIProxyAPI`

You only need to set `data-plane` explicitly when overriding these defaults or
when disabling embedded mode.

## Logging

Container stdout/stderr:

- `docker logs ox-cli-proxy-api`
- embedded Rust stdout/stderr are mirrored into the same container log stream
  with `[rs-stdout]` / `[rs-stderr]` prefixes

Mounted log directory:

- Go application logs remain under `./logs/`
- embedded Rust file logs are written under:
  - `./logs/data-plane/stdout.log`
  - `./logs/data-plane/stderr.log`

## Startup

```bash
docker compose up -d
```

## Quick Verification

Health:

```bash
curl http://127.0.0.1:18317/healthz
```

Runtime snapshot:

```bash
curl http://127.0.0.1:18317/v0/management/runtime-snapshot \
  -H 'Authorization: Bearer YOUR_MANAGEMENT_PASSWORD'
```

Responses smoke test:

```bash
curl http://127.0.0.1:18317/v1/responses \
  -H 'Authorization: Bearer YOUR_API_KEY' \
  -H 'Content-Type: application/json' \
  --data '{"model":"gpt-5.5","input":"reply with exactly OK"}'
```

Rust log file tail:

```bash
tail -f logs/data-plane/stdout.log
tail -f logs/data-plane/stderr.log
```

Container log tail:

```bash
docker logs -f ox-cli-proxy-api
```

Full embedded smoke:

```bash
MANAGEMENT_KEY=YOUR_MANAGEMENT_PASSWORD \
API_KEY=YOUR_API_KEY \
CPA_BASE_URL=http://127.0.0.1:18317 \
KEEPER_URL=http://127.0.0.1:28081 \
CONTAINER_NAME=ox-cli-proxy-api \
./scripts/embedded-smoke.sh
```

Keeper login automation boundary:

- The smoke script checks keeper reachability only.
- Public keeper login automation can be blocked by an environment-specific gate
  in front of the deployed keeper entrypoint.
- See [keeper-public-entry-boundary.md](./keeper-public-entry-boundary.md).
