# 0009-embedded-data-plane-state-dir-near-cli-proxy-api.md

## Status

Accepted

## Context

Embedded Rust data-plane artifacts were previously materialized into a
platform-specific fixed state directory such as:

- Linux: `~/.local/state/cliproxy/data-plane`
- macOS: `~/Library/Application Support/cliproxy/data-plane`
- Windows: `LocalAppData/cliproxy/data-plane`

That default made deployed instances harder to inspect because the Rust binary,
its checksum/version files, and its logs were detached from the main
`CLIProxyAPI` runtime directory. On the `rusty` branch, the common operational
expectation is that embedded Rust artifacts should live alongside the main
`CLIProxyAPI` executable unless an explicit override is configured.

## Decision

For embedded Rust data-plane mode:

- explicit `data-plane.embedded.state-dir` continues to win
- otherwise, the default state directory resolves to the directory containing
  the running `CLIProxyAPI` executable
- embedded Rust artifacts, metadata files, and the `logs/` subdirectory are
  therefore colocated with the main server binary by default

## Consequences

Positive:

- operational files are easier to discover in deployed environments
- embedded Rust artifacts now follow the lifecycle of the main binary directory
- container and VPS layouts no longer depend on user-home platform conventions

Negative:

- default artifact/log locations change for installs that previously relied on
  the implicit platform-specific path
- binaries launched from read-only directories must set an explicit
  `data-plane.embedded.state-dir`

Risk:

- existing operators may look in the old state directory until deployment docs
  and scripts are updated

## Alternatives Considered

1. Keep the platform-specific default state directory.
   - Rejected because it keeps embedded Rust operational files detached from
     the main deployment directory.

2. Use a subdirectory under the executable directory by default.
   - Rejected because the requested operational model is to colocate the
     extracted Rust files with `CLIProxyAPI` directly, while still allowing an
     explicit override when stricter separation is needed.
