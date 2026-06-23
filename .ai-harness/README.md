# AI Harness

- This directory stores long-lived AI collaboration context for this repository.
- Repository documents are the source of truth; chat history is not.
- New coding-agent sessions should read these files before making changes.

## Files

- `project-context.md`
  - High-level project overview: goals, stack, entrypoints, config, dependencies, deployment.
- `project-state.md`
  - Current implementation snapshot: implemented capabilities, active direction, risks, pending areas.
- `architecture.md`
  - Codebase structure, module responsibilities, data/control flow, configuration and logging strategy.
- `conventions.md`
  - Project-specific coding and documentation conventions inferred from the repository.
- `commands.md`
  - Common build, run, test, Docker, and helper commands that are actually supported by the repo.
- `testing.md`
  - Test strategy, validation guidance, and test coverage risks.
- `session-log.md`
  - Running log of non-trivial agent sessions.
- `session-summary-template.md`
  - Template for future session entries.

## Subdirectories

- `decisions/`
  - ADRs for architecture, API, config, deployment, and model-boundary changes.
- `features/`
  - Feature-level behavior maps and boundaries.
- `bugs/`
  - Bug records: symptom, root cause, fix, prevention.

## When To Update

- Update `project-state.md` when:
  - implemented capabilities materially change
  - active development direction changes
  - new risks or pending work become clear
- Update `architecture.md` when:
  - module boundaries, data flow, control flow, configuration flow, or extension points change
- Update `conventions.md` when:
  - stable project conventions change
- Update `commands.md` and `testing.md` when:
  - supported commands or validation workflow change
- Update `features/` when:
  - user-visible behavior changes
- Update `bugs/` when:
  - a real bug is fixed or a recurring failure mode is understood
- Update `decisions/` when:
  - architecture, API surface, config format, deployment mode, or data model changes
- Update `session-log.md` after each non-trivial agent session.

## Reading Order For New Sessions

1. `AGENTS.md`
2. `.ai-harness/README.md`
3. `.ai-harness/project-state.md`
4. `.ai-harness/architecture.md`
5. `.ai-harness/conventions.md`
6. `.ai-harness/commands.md`
7. `.ai-harness/testing.md`
8. Relevant files under `decisions/`, `features/`, and `bugs/`

If information is uncertain, it should be marked `待确认`.
