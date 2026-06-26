# Local AI Harness Workspace

- This directory is for local-only collaboration state.
- Content here is intentionally excluded from Git so multiple collaborators do not fight over volatile notes.

## Intended Uses

- `session-log/`
  - Optional per-session notes, working journals, or handoff drafts.
- `scratch/`
  - Temporary research notes, experiments, or generated analysis that are not durable repository facts.

## Rules

- Do not treat files here as the repository source of truth.
- Promote durable facts into `.ai-harness/shared/` only after they are stable enough to review and keep in Git.
- If you need a quick session note template, include:
  - goal
  - files touched
  - behavior changed
  - validation run
  - follow-ups
