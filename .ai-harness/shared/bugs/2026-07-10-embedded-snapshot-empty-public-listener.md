# Embedded Snapshot Empty Public Listener

## Symptom

An embedded Rust data-plane child repeatedly exited during startup with:

```text
snapshot.listeners.public_http must not be empty
```

The Go container continued restarting the child, preventing Rust readiness and any
embedded `/v1/responses` smoke validation.

## Root Cause

`BuildRuntimeSnapshot()` exported `data-plane.responses-base-url` as
`listeners.public_http`. In embedded mode the first snapshot is built before the
supervisor has populated the runtime Rust URL. Configurations that correctly use an
empty Go `host` (`:8317`) and omit `responses-base-url` therefore exported an empty
required snapshot field.

## Fix

The exporter now prefers an explicit or runtime data-plane base URL. When neither is
available, it derives a non-empty HTTP listener from the Go host and port, normalizing
empty and wildcard bind hosts to `127.0.0.1` for the local embedded child.

## Prevention

`TestBuildRuntimeSnapshotUsesGoListenerWhenDataPlaneURLIsNotReady` covers the startup
ordering boundary. The existing Rust snapshot contract test continues to reject empty
`listeners.public_http` values.
