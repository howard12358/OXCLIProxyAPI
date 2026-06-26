# Decisions / ADRs

## Purpose

- Record durable architecture and engineering decisions.
- Store only decisions that are stable enough to matter across collaborators and sessions.
- Required for changes that affect:
  - architecture
  - interfaces / public APIs
  - configuration format
  - deployment mode
  - data models / schemas

## When To Add A New ADR

- New architectural direction
- Change to external or internal contract that other components rely on
- Change to config file format or CLI behavior
- Change to deployment or topology model
- Significant cross-module design choice that should not live only in code review or chat
- Changes to the `.ai-harness/shared/` collaboration contract or repository documentation model

## Naming

- File pattern:
  - `000N-short-kebab-case-title.md`
- Keep numbering monotonic.

## Template

```md
# 000N-title.md

## Status

Proposed | Accepted | Deprecated | Superseded

## Context

背景和问题。

## Decision

最终决策。

## Consequences

影响，包括优点、缺点、风险。

## Alternatives Considered

考虑过但未采用的方案。
```
