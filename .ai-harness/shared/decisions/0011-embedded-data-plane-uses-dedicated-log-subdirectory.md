# 0011-embedded-data-plane-uses-dedicated-log-subdirectory.md

## Status

Accepted

## Context

Embedded Rust data-plane logs were previously written to the same `logs/`
directory tree used by the Go server:

- `logs/stdout.log`
- `logs/stderr.log`

That kept the child-process logs colocated with the main runtime, but it also
blurred operational ownership:

- Go application logs and Rust child-process logs shared one directory
- Go log-directory cleanup could delete Rust log files as part of the same pool
- Rust logs had no dedicated rotation or retention policy

The requested operational model is to keep embedded Rust logs under the same
overall runtime tree while separating them from Go logs and giving them an
independent retention policy.

## Decision

For embedded Rust data-plane mode:

- Rust child-process logs move from `stateDir/logs/` to
  `stateDir/logs/data-plane/`
- the Go embedded supervisor continues to own stdout/stderr file placement
- embedded Rust stdout/stderr files use internal size-based rotation and backup
  retention
- embedded Rust log cleanup runs only inside `stateDir/logs/data-plane/`
  instead of sharing the Go log-directory cleanup scope

This keeps embedded startup behavior unchanged while making Rust log ownership
clearer.

## Consequences

Positive:

- Go and embedded Rust logs are separated by directory
- Rust stdout/stderr now have dedicated rotation and retention behavior
- Go application log cleanup no longer implicitly governs embedded Rust logs

Negative:

- existing operators must look under `logs/data-plane/` for embedded Rust logs
- the embedded supervisor now owns a small additional background cleaner

Risk:

- scripts or dashboards that assumed `logs/stdout.log` and `logs/stderr.log`
  need to be updated

## Alternatives Considered

1. Keep Rust logs in `logs/` and rely on Go's shared log-directory cleanup.
   - Rejected because it preserves unclear ownership and does not give Rust logs
     a dedicated retention policy.

2. Move file logging completely into the Rust binary.
   - Rejected for now because embedded mode still benefits from Go owning child
     process lifecycle and output placement, and the requested change can be
     achieved without changing external startup behavior.
