---
name: review-general
description: Review the current branch against its merge base for bugs, CPython divergence, sandbox escapes, resource-limit escapes, performance regressions, verbose comments and missing ./limitations/ updates. Use for a general pre-merge review of a branch or PR.
---

# General branch review

```bash
git diff --stat origin/main...HEAD   # scope first
git diff origin/main...HEAD
```

Read the changed files in full — a hunk is rarely enough to judge correctness. Look for:

- **Bugs** — logic errors, `DropWithContext` values not released on every exit path (the
  fix is `defer_drop!`/`DropGuard`, not more `drop_with` calls), borrow/aliasing
  mistakes, unhandled error paths.
- **CPython divergence** — different results, exception types or messages, missing
  attributes. Check anything you're unsure of with `python-playground`.
- **Sandbox escapes** — sandboxed code reaching the host filesystem, environment,
  network or subprocesses.
- **Resource-limit escapes** — allocations not charged to the tracker (an unbounded or
  amplifying `String` build without `StringBuilder`), unbounded loops, recursion without
  a depth guard.
- **Performance** — regressions the branch introduces, and improvements you spot.
- **Verbose comments** — docstrings and comments should be concise as per `CLAUDE.md`.
- **Cleanups** — duplication, misplaced logic, functions grown too complex.
- **`./limitations/`** — a new divergence with no entry is a finding.

## Report

Concise, most severe first. Per finding: `file:line`, what's wrong, the concrete failure
it causes. Don't pad with what the branch got right.

Report only, unless the user asks for fixes.
