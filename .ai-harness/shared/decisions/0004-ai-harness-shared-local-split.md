# 0004-ai-harness-shared-local-split.md

## Status

Accepted

## Context

The original `.ai-harness` structure mixed durable repository facts with volatile collaboration state in the same tracked location. In practice, files such as `session-log.md` and broad `project-state.md` updates create unnecessary merge conflicts in multi-collaborator workflows, while still being too transient to justify long-term versioned history.

The repository still needs a committed source of truth for architecture, feature boundaries, risks, testing guidance, and similar long-lived context. The problem is not whether AI harness content belongs in Git; the problem is that durable facts and high-churn working state were not separated.

## Decision

Split `.ai-harness` into two collaboration scopes:

- `.ai-harness/shared/`
  - committed durable repository facts
- `.ai-harness/local/`
  - untracked local working state

Under this model:

- required-reading documents for agents move to `.ai-harness/shared/`
- `project-state.md` is renamed to `current-state.md` to emphasize durable repository state rather than live task tracking
- `session-log.md` is removed from the committed collaboration contract
- session logs, scratch notes, and temporary handoff material belong under `.ai-harness/local/`

## Consequences

Positive:

- lower merge-conflict pressure during parallel collaboration
- clearer separation between source-of-truth documents and disposable working notes
- easier review of meaningful shared documentation updates

Negative:

- collaborators must consciously promote durable local findings into shared docs
- some previously committed per-session history is no longer preserved in Git by default

Risks:

- if collaborators overuse `.ai-harness/local/`, useful findings may fail to graduate into shared facts
- agents must follow the new reading paths consistently or they may miss relevant shared context

## Alternatives Considered

- Keep the old single-directory model and only narrow `project-state.md`
  - rejected because `session-log.md` and similar process files would still create avoidable conflicts
- Remove almost all `.ai-harness` content from Git
  - rejected because the repository would lose a durable in-repo fact base for future collaborators and agents
