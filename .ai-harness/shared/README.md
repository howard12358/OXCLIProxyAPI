# Shared AI Harness Context

- This directory contains durable repository facts that should be committed to Git.
- Shared files should stay stable enough for multi-collaborator use and code review.
- Do not store personal work journals, temporary task tracking, or scratch analysis here.

## Contents

- `project-context.md`
  - Stable project overview, entrypoints, dependencies, and deployment context.
- `current-state.md`
  - Durable capability snapshot, known risks, unresolved boundaries, and current direction.
- `architecture.md`
  - Repository structure and control/data flow.
- `conventions.md`
  - Stable coding and documentation conventions.
- `commands.md`
  - Supported commands and helper workflows.
- `testing.md`
  - Validation strategy and coverage limits.
- `decisions/`
  - ADRs for durable technical decisions.
- `features/`
  - Feature behavior maps and boundaries.
- `bugs/`
  - Bug records with symptom, root cause, and prevention notes.

## Shared Document Rules

- Keep shared docs low-churn and repository-scoped.
- Prefer durable facts over personal interpretation.
- If something is uncertain, mark it `待确认`.
- If content is only useful within a single session, put it under `.ai-harness/local/` instead.
