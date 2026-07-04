# 0007-embedded-docker-image-workflow.md

## Status

Accepted

## Context

The existing Docker Hub release workflow builds the Go server with the normal `Dockerfile`.
That image does not build or package the Rust data-plane binary, so enabling
`data-plane.mode: embedded` in a container would fail unless an external
`CLIPROXY_DATA_PLANE_BINARY_PATH` is also provided.

Production validation for the Rust data plane needs a separate image path that can be tested
without changing the existing tag-driven Docker release workflow.

## Decision

- Keep the existing tag-triggered `docker-image` workflow unchanged.
- Add `Dockerfile.embedded` as a dedicated image build that:
  - builds `rust/cliproxy-data-plane` in release mode,
  - generates the Go embedded artifact with `cmd/embed_data_plane`,
  - builds the Go server with `-tags release_embedded_artifact`.
- Add a manual `docker-embedded-image` workflow using `workflow_dispatch`.
- Publish the embedded image as `ox-cli-proxy-api` under the Docker Hub namespace from
  `DOCKERHUB_USERNAME` by default, with an optional manual namespace override.
- Default the manual workflow tag to `v0.0.1` for `linux/amd64,linux/arm64`.

## Consequences

- The normal Docker release remains Go-only until explicitly changed.
- Embedded Rust data-plane testing can use an isolated Docker tag.
- Multi-arch embedded images are the default validation target.
