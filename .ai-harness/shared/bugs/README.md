# Bugs

## Purpose

- Record real bug fixes and recurring failure modes.
- Keep these records stable enough to help future collaborators, not as temporary debugging notes.

## When To Add A Bug Document

- A bug has been reproduced and fixed
- A bug root cause is known even if full prevention work is pending
- A recurring operational failure should be documented for future sessions

## Naming

- File pattern:
  - `YYYY-MM-DD-short-kebab-case-title.md`

## Template

```md
# Bug: <short title>

## Date

YYYY-MM-DD

## Symptom

现象。

## Impact

影响范围。

## Reproduction

复现方式。

## Root Cause

根因。

## Fix

修复方式。

## Prevention

如何避免再次发生。

## Related Files

相关文件。

## Related Tests

相关测试。
```
