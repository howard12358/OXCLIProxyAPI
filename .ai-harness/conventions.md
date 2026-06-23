# Conventions

## Naming

- Follow existing Go package and file naming conventions.
- Keep names explicit and behavior-oriented.
- Preserve existing public API names unless a deliberate API change is approved.

## Directory Rules

- `internal/runtime/executor/`
  - executors and their unit tests only
- Helper/supporting files for executors:
  - `internal/runtime/executor/helps/`
- Avoid standalone changes limited to `internal/translator/` unless broader work requires it.

## Package / Module Boundaries

- Keep Go management, provider execution, translation, watcher, and store responsibilities separated.
- Keep Rust data-plane runtime, HTTP, and upstream execution concerns separated.
- Prefer small, local changes over cross-cutting refactors.

## Error Handling

- Go:
  - wrap errors with context where useful
  - no `log.Fatal` / `log.Fatalf`
  - avoid panics in handlers
- Rust:
  - prefer `Result`
  - avoid `unwrap`-style behavior outside tests where practical

## Logging

- Use structured logging.
- Do not log secrets, tokens, or sensitive auth material.
- Keep logs actionable and scoped to the change being made.

## Configuration

- Do not silently change config format.
- `config.yaml` and `config.example.yaml` are part of the interface.
- `.env` support and storage-backend env variables are part of startup behavior.

## Testing

- Run relevant tests for touched areas.
- After Go changes:
  - format with `gofmt -w`
  - verify server build
- After Rust changes:
  - format with `cargo fmt`
  - run targeted or workspace tests depending on scope

## Documentation

- New Markdown docs should be English unless the file is explicitly language-specific.
- If information is uncertain, mark it `待确认`.
- Do not treat chat history as a stable source; move durable facts into repo docs.

## Commit Guidance

- Keep behavior changes, bug fixes, and refactors separated when possible.
- Avoid mixing unrelated cleanup into feature work.

## Agent Rules

- Read `AGENTS.md` and relevant `.ai-harness/` docs before changing code.
- Update `.ai-harness/` when:
  - behavior changes
  - architecture changes
  - bug root causes become clear
  - commands / test workflow change
- Prefer small, local modifications.
- Do not silently modify:
  - architecture
  - public API
  - config format
  - CLI commands
  - deployment mode
  - directory structure
- If such a change is required, add or update an ADR first.
