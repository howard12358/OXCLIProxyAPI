# Rust Data Plane Fallback & Disable

This document covers the operational controls for the embedded Rust `/v1/responses`
data plane on the `rusty` branch.

## 1. Is Embedded Rust Data Plane Default?

Yes. On the `rusty` branch, if `data-plane` is entirely omitted from the config,
`DataPlaneConfig.EffectiveMode()` returns `"embedded"`.

## 2. How Go Launches the Rust Data Plane

When mode is `"embedded"`, the Go process starts a Rust `cliproxy-data-plane` child
process during startup. Key details:

- The binary is materialized from the container image or local build.
- Bind address defaults to `127.0.0.1:4100`.
- The Rust process polls Go's runtime snapshot endpoint every 30 seconds (default).
- stdout/stderr are mirrored into the container log stream with `[rs-stdout]` /
  `[rs-stderr]` prefixes.

## 3. How Rust Pulls the Runtime Snapshot

Rust reads the Go runtime snapshot via HTTP:

```bash
GET /v0/management/runtime-snapshot
Authorization: Bearer <management-key>
```

Configurable via:

- `--snapshot-url` (CLI) or `CLIPROXY_SNAPSHOT_URL` (env)
- `--snapshot-bearer-token` (CLI) or `CLIPROXY_SNAPSHOT_BEARER_TOKEN` (env)

Alternatively, a file-based snapshot source can be used:

```bash
--snapshot-file /path/to/snapshot.json
```

## 4. What `/healthz` and `/readyz` Mean

| Endpoint | Meaning |
| -------- | ------- |
| `GET /healthz` | Returns `{status, service, version}`. `starting` on boot, `ready` on healthy. |
| `GET /readyz` | Same as healthz but also returns `{runtime: {version, generated_at, ...}}`. |

Both endpoints are served by the Rust data plane on its bind address (default `127.0.0.1:4100`).

## 5. Rust Startup Failure Behavior

If the Rust child process fails to start or cannot load a valid snapshot:

- Go logs the startup error with the `[rs-stderr]` prefix.
- `/v1/responses` requests fall back to the Go native executor path if `data-plane.mode` is not explicitly `embedded`.
- With explicit `embedded` mode, the Go process may enter a degraded state until the Rust child recovers.

## 6. Rust `/readyz` Degraded Behavior

When Rust returns a `degraded` or `failed` service state:

- The runtime snapshot is either stale or missing.
- Rust will respond `503` on `/v1/responses` with `runtime_snapshot_unavailable`.
- The Go process can detect this and fall back to Go native routing.

## 7. How to Manually Disable Rust Data Plane

Set in your `config.yaml`:

```yaml
data-plane:
  mode: disabled
```

Or any of the equivalent values:

```yaml
data-plane:
  mode: "off"
```

```yaml
data-plane:
  mode: "none"
```

Any of these cause `EffectiveMode()` to return `""` (disabled), and Go will route
`/v1/responses` through the Go-native executor.

## 8. How to Fall Back to Go Native `/v1/responses`

There are two paths to fallback:

**Permanent (config change):** Set `data-plane.mode: disabled` and restart.

**Runtime (no restart required):** If `DataPlaneConfig.ResponsesBaseURL` is unset
(or set to `""`) and the embedded Rust child is not running, Go will serve requests
natively. The Go process auto-detects when the Rust child stops accepting connections.

## 9. How to View Rust Data Plane Logs

```bash
# Docker logs (mirrored with prefixes)
docker logs -f ox-cli-proxy-api | grep '\[rs-'

# File logs (on mounted volume)
tail -f logs/data-plane/stdout.log
tail -f logs/data-plane/stderr.log
```

Key log markers:

- `data plane listening` — Rust process healthy and accepting connections
- `runtime snapshot applied` — Rust has loaded a valid snapshot
- `ERROR` / `[rs-stderr]` — startup or request failures

## 10. How to Confirm a Request Used Rust Data Plane

The Rust data plane adds a header to responses when it handles a request:

```bash
curl -v http://127.0.0.1:8317/v1/responses ... 2>&1 | grep -i 'x-cliproxy\|server'
```

Additionally, check usage queue records. Rust-originated payloads have
`executor_type: "RustResponsesExecutor"`; the current Go-native Codex executor
emits `executor_type: "CodexExecutor"` (the Go type name).

## 11. Rollback Steps

To roll back to Go-native `/v1/responses`:

1. Add `data-plane.mode: disabled` to your `config.yaml`:

   ```yaml
   data-plane:
     mode: disabled
   ```

2. Restart the container:

   ```bash
   docker compose down
   docker compose up -d
   ```

3. Verify the Rust child is not running:

   ```bash
   docker compose logs cli-proxy-api --tail=50 | grep 'rs-stdout\|rs-stderr'
   ```

   There should be no recent `[rs-stdout]` / `[rs-stderr]` lines.

4. Send a test request and check the usage queue — executor_type should be
   `CodexExecutor`, not `RustResponsesExecutor`.

## Reference: DataPlaneConfig

```go
type DataPlaneConfig struct {
    Mode                     string                    `yaml:"mode,omitempty"`
    ResponsesBaseURL         string                    `yaml:"responses-base-url"`
    Embedded                 EmbeddedDataPlaneConfig   `yaml:"embedded,omitempty"`
    RuntimeResponsesBaseURL  string                    `yaml:"-" json:"-"`
}
```

EffectiveMode resolution order:
1. `"disabled"` / `"off"` / `"none"` → disabled
2. `"embedded"` / `"external"` → explicit mode
3. `ResponsesBaseURL` is set → `"external"` (legacy)
4. `Embedded.Enabled` is true → `"embedded"`
5. Default → `"embedded"`
