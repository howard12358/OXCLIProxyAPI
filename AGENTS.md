# AGENTS.md

This file defines long-lived rules for coding agents working in this repository.

## Source Of Truth

- Repository documents are the source of truth.
- Chat history is not the source of truth.
- If information is uncertain, mark it `待确认`. Do not invent facts.

## Required Reading Before Changes

- Read this file first.
- Then read:
  - `.ai-harness/README.md`
  - `.ai-harness/shared/project-context.md`
  - `.ai-harness/shared/current-state.md`
  - `.ai-harness/shared/architecture.md`
  - `.ai-harness/shared/conventions.md`
  - `.ai-harness/shared/commands.md`
  - `.ai-harness/shared/testing.md`
- Also read any relevant files under:
  - `.ai-harness/shared/decisions/`
  - `.ai-harness/shared/features/`
  - `.ai-harness/shared/bugs/`

## When `.ai-harness/` Must Be Updated

- Update `.ai-harness/shared/current-state.md` when durable implemented capabilities, known risks, or unresolved boundaries change.
- Update `.ai-harness/shared/architecture.md` when module boundaries, data flow, control flow, extension points, config flow, or runtime topology change.
- Update `.ai-harness/shared/conventions.md` when stable project conventions change.
- Update `.ai-harness/shared/commands.md` or `.ai-harness/shared/testing.md` when supported commands or validation workflow change.
- Update `.ai-harness/shared/features/` when user-visible behavior changes.
- Update `.ai-harness/shared/bugs/` when a bug is fixed and root cause is understood.
- Update `.ai-harness/shared/decisions/` when architecture, public API, config format, deployment mode, directory structure, or data model changes.
- Use `.ai-harness/local/` for per-session notes, scratch work, and other high-churn local state. Do not commit local working-state files.

## Things That Must Not Change Silently

Without explicit user request and corresponding documentation updates, do not silently change:

- architecture
- public APIs
- database schema or data model
- config format
- CLI commands or flags
- deployment mode
- directory structure

If such a change is necessary:

1. Create or update an ADR under `.ai-harness/shared/decisions/`
2. Update the relevant architecture / state / commands / testing docs
3. Then implement the code change

## General Change Rules

- Prefer small, local changes.
- Do not mix feature development, bug fixing, and refactoring in one change unless explicitly required.
- Do not treat unrelated cleanup as part of the task.
- Do not delete existing files unless explicitly requested.

## Bug Fix Workflow

1. Capture symptom and impact.
2. Confirm current behavior from code and tests.
3. Fix the bug with the smallest reasonable scope.
4. Run relevant validation.
5. Add or update a bug record under `.ai-harness/shared/bugs/`.
6. Update `.ai-harness/shared/current-state.md` only if the fix changes durable project facts, risks, or boundaries.

## Feature Development Workflow

1. Read relevant `.ai-harness/` docs and related code.
2. Confirm whether behavior is already documented under `.ai-harness/shared/features/`.
3. Implement with minimal scope.
4. Run relevant validation.
5. Update or add feature docs under `.ai-harness/shared/features/`.
6. Update `.ai-harness/shared/current-state.md` if durable capability, known risk, or unresolved boundary changed.

## Architecture Change Workflow

1. Confirm the change is actually required.
2. Write or update an ADR first.
3. Update `.ai-harness/shared/architecture.md` and related docs.
4. Implement the code change.
5. Run broader validation than a normal local change.

## Testing And Validation Requirements

- Run relevant tests for the area you changed, or explain why tests were not run.
- For Go changes:
  - run `gofmt -w`
  - run relevant `go test` commands
  - verify build with `go build -o /tmp/cli-proxy-api-check ./cmd/server && rm -f /tmp/cli-proxy-api-check`
- For Rust changes:
  - run `cargo fmt --manifest-path rust/cliproxy-data-plane/Cargo.toml`
  - run relevant `cargo test` commands
- For cross-runtime changes, prefer both automated tests and manual stack validation when practical.

## Stable Repository Rules

- Comments in code must be English only.
- If editing code that already contains non-English comments, translate them to English rather than adding more non-English comments.
- For user-visible strings, preserve the language already used in that area.
- New Markdown docs should be English unless the file is explicitly language-specific.
- Do not use `log.Fatal` / `log.Fatalf`; return errors and log with context instead.
- Avoid panics in HTTP handlers.
- Do not make standalone changes to `internal/translator/` unless broader work requires it.
- `internal/runtime/executor/` should contain executors and their unit tests only; helpers go under `internal/runtime/executor/helps/`.
- Timeouts are allowed only during credential acquisition, except the existing documented exceptions already present in the codebase.
- Shared `.ai-harness` files must stay durable and collaboration-safe; volatile task tracking belongs under `.ai-harness/local/`.
