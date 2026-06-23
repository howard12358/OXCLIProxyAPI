# Project State

## Current Stable Architecture

- Main Go server is the primary proxy runtime and management plane.
- Go server exposes API routes, management routes, auth handling, model registry, watcher-based reload, and SDK hooks.
- Rust data plane exists as a separate workspace and can serve `/v1/responses`.
- Rust data plane consumes Go-exported runtime snapshot by file or HTTP.

## Implemented Capabilities

- OpenAI / Gemini / Claude / Codex / Grok compatible proxy interfaces
- OAuth-backed provider login flows for multiple providers
- Multi-account routing and load balancing
- Go watcher-based config / auth reload
- Optional plugin system
- SDK under `sdk/cliproxy`
- Rust data plane:
  - health / readiness
  - runtime snapshot load / refresh
  - `/v1/responses`
  - snapshot notify endpoint
  - runtime snapshot observation endpoint

## Unfinished Or 待确认 Capabilities

- Full production-grade Go-managed Rust instance registry / heartbeat lifecycle is not fully confirmed from current repository state.
- Formal multi-instance Rust data-plane management is `待确认`.
- Some deployment and management-center flows are partially documented externally; exact in-repo completeness is `待确认`.

## Current Active Development Direction

- Go-managed Rust data plane for `/v1/responses`
- Runtime snapshot export / refresh / observability
- Dev stack tooling in `Makefile`
- Unified upstream proxy behavior between Go and Rust

## Frozen Key Decisions

- Repository docs should be treated as the fact source.
- Runtime snapshot is the effective configuration contract for Rust data plane.
- Go should notify Rust of snapshot changes; Rust should pull full snapshot afterward.
- Polling remains as fallback even if notify exists.

## Known Risks

- Large mixed Go codebase with many provider flows increases accidental regression risk.
- Rust data-plane productization work may diverge from Go behavior if proxy, routing, or auth semantics are changed in only one runtime.
- Test coverage appears stronger in selected areas than for full end-to-end production flows.
- Some architectural intent lives in docs and current worktree changes, not only in released code.

## Likely Next Tasks

- Continue productizing Go-managed Rust data plane
- Improve multi-instance coordination and observability
- Expand integration coverage around snapshot changes and notify behavior
- Keep `.ai-harness/` updated as implementation solidifies
