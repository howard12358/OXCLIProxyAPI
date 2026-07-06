# 0008-rusty-defaults-to-embedded-data-plane.md

## Status

Accepted

## Context

The `rusty` branch is intended to exercise the embedded Rust data plane as the
default `/v1/responses` path. Requiring users to repeat the same `data-plane`
block in every deployment config creates unnecessary config drift between
environments:

- `mode: embedded`
- `embedded.enabled: true`
- `embedded.bind-addr: 127.0.0.1:4100`
- `embedded.log-level: info`
- `embedded.startup-timeout-seconds: 20`

The Go service already has stable embedded defaults for bind address, log
level, state directory, and startup timeout. The remaining gap is that an empty
`data-plane` config still resolves to "disabled", which forces explicit config
to activate the exact same behavior.

## Decision

On the `rusty` branch:

- an empty `data-plane` config resolves to `embedded` mode by default
- `responses-base-url` without an explicit mode still implies `external` mode
- explicit `mode: embedded` and `mode: external` remain unchanged
- explicit `mode: disabled` (also `off` / `none`) turns the Rust data plane off

This keeps embedded defaults implicit while preserving an explicit escape hatch
for environments that need to disable the Rust data plane.

## Consequences

Positive:

- embedded deployments no longer need repetitive default `data-plane` config
- local, staging, and production configs can stay closer to each other
- the existing embedded supervisor defaults become the effective source of truth

Negative:

- branch behavior changes: configs that previously omitted `data-plane` now
  start the embedded Rust data plane by default
- operators who want Go-only `/v1/responses` behavior must set
  `data-plane.mode: disabled`

Risk:

- tests and docs must clearly encode the new default so collaborators do not
  assume empty config still disables the Rust data plane

## Alternatives Considered

1. Keep empty config disabled and require explicit embedded config everywhere.
   - Rejected because it preserves repetitive config drift without adding real
     control.

2. Default to embedded without any explicit disable mode.
   - Rejected because it removes a clean way to opt out on this branch.
