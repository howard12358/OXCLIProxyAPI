# Features

## Purpose

- Record user-visible and developer-relevant feature behavior.
- Keep feature boundaries explicit for future changes.

## When To Add Or Update A Feature Document

- Add a feature doc when:
  - a new feature or subsystem is introduced
  - a current feature has behavior complex enough to deserve a dedicated map
- Update a feature doc when:
  - behavior changes
  - inputs / outputs change
  - edge cases or non-goals become clearer

## Naming

- File pattern:
  - `<short-feature-name>.md`
  - prefer kebab-case

## Template

```md
# Feature: <name>

## Purpose

这个功能解决什么问题。

## Current Behavior

当前行为。

## Inputs

输入。

## Outputs

输出。

## Main Flow

主要流程。

## Edge Cases

边界情况。

## Non-goals

明确不做什么。

## Related Files

相关代码文件。

## Related Decisions

相关 ADR。

## Validation

验证方式。
```
