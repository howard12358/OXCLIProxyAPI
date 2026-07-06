# 0010-docker-compose-defaults-to-embedded-remote-image.md

## Status

Accepted

## Context

On the `rusty` branch, the intended default runtime path is embedded Rust data-plane
handling for `/v1/responses`. However, the repository root deployment entrypoint
still reflected the older Go-only container path:

- `docker-compose.yml` built `Dockerfile`
- a stale data-plane state mount targeted the old fixed home-directory path
- helper scripts still framed Docker usage as a choice between prebuilt Go-only
  images and source builds

That made the repository harder to use than upstream CPA's common "clone, edit
config, docker compose up" flow, and it also mismatched the branch's actual
default runtime model.

## Decision

For the repository root deployment path on the `rusty` branch:

- `docker-compose.yml` defaults to pulling `rustyllh/ox-cli-proxy-api:latest`
- helper scripts `docker-build.sh` and `docker-build.ps1` build from local source
  into the separate image tag `ox-cli-proxy-api:local`
- source-based local builds continue to use `Dockerfile.embedded`
- the stale `/root/.local/state/cliproxy/data-plane` compose mount is removed

This keeps the repository's default Docker entrypoint aligned with the branch's
embedded runtime semantics.

## Consequences

Positive:

- clone-edit-config-then-`docker compose up -d` now works against the intended
  embedded runtime path without requiring a local build toolchain
- repo deployment behavior is easier to understand because remote deploy and
  source-build flows use distinct image tags
- the stale fixed-path data-plane mount no longer suggests an outdated state-dir
  contract

Negative:

- the root compose path no longer represents the older Go-only container
  deployment model
- source-based local verification now depends on the helper script or an
  explicit compose override instead of plain `docker compose up -d`

Risk:

- collaborators relying on the older root compose local-build semantics may need
  to adjust any local scripts that expected `docker compose up -d` to rebuild
  from source

## Alternatives Considered

1. Keep the old Go-only root compose and add a second embedded compose file.
   - Rejected because it preserves a split default path and keeps the repo entry
     point misaligned with the `rusty` branch runtime model.

2. Keep root compose on a local embedded build.
   - Rejected because it mixes user deployment and source-build validation under
     the same image tag and makes the default deployment path depend on a local
     build toolchain.
