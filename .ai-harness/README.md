# AI Harness

- This directory stores AI collaboration context for this repository.
- Repository documents are the source of truth; chat history is not.
- New coding-agent sessions should read the shared files before making changes.

## Layout

- `shared/`
  - Durable repository facts that should be versioned and reviewed.
- `local/`
  - High-churn working state for individual collaborators. This content is intentionally not committed.

## Shared Files

- `shared/project-context.md`
  - High-level project overview: goals, stack, entrypoints, config, dependencies, deployment.
- `shared/current-state.md`
  - Durable implementation snapshot: implemented capabilities, known risks, unresolved boundaries, and active direction at a repository level.
- `shared/architecture.md`
  - Codebase structure, module responsibilities, data/control flow, configuration and logging strategy.
- `shared/conventions.md`
  - Project-specific coding and documentation conventions inferred from the repository.
- `shared/commands.md`
  - Common build, run, test, Docker, and helper commands that are actually supported by the repo.
- `shared/testing.md`
  - Test strategy, validation guidance, and test coverage risks.
- `shared/decisions/`
  - ADRs for architecture, API, config, deployment, and model-boundary changes.
- `shared/features/`
  - Feature-level behavior maps and boundaries.
- `shared/bugs/`
  - Bug records: symptom, root cause, fix, prevention.

## Local Files

- `local/session-log/`
  - Optional per-session notes for a single collaborator or agent.
- `local/scratch/`
  - Temporary analysis, drafts, or generated notes that are not durable repository facts.

## When To Update Shared Docs

- Update `shared/current-state.md` when:
  - implemented capabilities materially change
  - durable active development direction changes
  - new long-lived risks or unresolved boundaries become clear
- Update `shared/architecture.md` when:
  - module boundaries, data flow, control flow, configuration flow, or extension points change
- Update `shared/conventions.md` when:
  - stable project conventions change
- Update `shared/commands.md` and `shared/testing.md` when:
  - supported commands or validation workflow change
- Update `shared/features/` when:
  - user-visible behavior changes
- Update `shared/bugs/` when:
  - a real bug is fixed or a recurring failure mode is understood
- Update `shared/decisions/` when:
  - architecture, API surface, config format, deployment mode, or data model changes
- Keep high-frequency task tracking, work journals, and scratch notes under `local/` instead of the shared files.

## Reading Order For New Sessions

1. `AGENTS.md`
2. `.ai-harness/README.md`
3. `.ai-harness/shared/project-context.md`
4. `.ai-harness/shared/current-state.md`
5. `.ai-harness/shared/architecture.md`
6. `.ai-harness/shared/conventions.md`
7. `.ai-harness/shared/commands.md`
8. `.ai-harness/shared/testing.md`
9. Relevant files under `shared/decisions/`, `shared/features/`, and `shared/bugs/`

If information is uncertain, it should be marked `待确认`.
