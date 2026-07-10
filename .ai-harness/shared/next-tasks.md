# Next Tasks: Embedded Rust Data Plane Release Gate

## Current Phase

Rust `/v1/responses` embedded data-plane 已具备较完整的契约测试体系和运维文档。

当前重点从"代码正确性"转向"发布可控性"。

## P0 — Must Complete Before Default Enable

- [ ] Republish `rustyllh/ox-cli-proxy-api:latest` through `docker-embedded-image` and inspect its manifest from an ARM64 host
- [ ] Execute embedded smoke in a real Docker environment (`scripts/embedded-smoke.sh`)
- [ ] Record smoke result in `current-state.md` and smoke runbook
- [ ] Verify `data-plane.mode: disabled` fallback path in a running deployment
- [x] Confirm executor identity: Rust uses `RustResponsesExecutor`; Go native uses `CodexExecutor`
- [x] Add GitHub Actions CI for Rust `fmt` / `clippy` / `cargo test`
- [x] Add GitHub Actions CI for Go full test / build
- [x] Document release gate checklist (see `release-gate.md`)

## P1 — Should Complete Before Default Enable

- [x] Create mock-upstream baseline benchmark runbook
- [ ] Compare Go native vs embedded Rust `/v1/responses` (non-stream response correctness)
- [ ] Exercise the `data-plane.mode: disabled` fallback in a real container
- [x] Verify stream abort connection cleanup under concurrent load (10 clients)

## P2 — Nice to Have

- [ ] Add CI or manual workflow note for embedded Docker smoke
- [ ] Measure RSS, CPU, P95/P99 for Rust vs Go `/v1/responses` under mock upstream
- [ ] Add RESP `errors` channel contract test
- [ ] Add multi-auth retry-on-abort contract test
- [x] Document default-enable criteria explicitly

## Not Doing Now

- Provider expansion (Claude / Gemini / Grok)
- Go management plane refactor
- Config format change
- Real upstream load test
- Home LPUSH usage as mainline feature
- Multi-instance Rust data-plane coordination
