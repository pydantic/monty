---
name: review-general
description: Review the current branch against its merge base for bugs, CPython divergence, sandbox escapes, resource-limit escapes, performance regressions, verbose comments and missing ./limitations/ updates. Use for a general pre-merge review of a branch or PR.
---

# General branch review

Review everything this branch changes, in detail, against its merge base, and write up
a concise report.

## Instructions

Get the diff against the merge base:

```bash
git fetch origin main
git diff origin/main...HEAD          # merge-base diff
git diff --stat origin/main...HEAD   # scope first
```

Read the changed files in full, not just the hunks — a hunk is rarely enough to judge
correctness. Then report on each of:

- **Bugs** — logic errors, refcount leaks (missing `drop_with` on an early-return or
  `continue` branch), borrow/aliasing mistakes, unhandled error paths.
- **Behaviour divergence from CPython** — different results, different exception types
  or messages, missing attributes. Use the `python-playground` skill to check any case
  you are unsure of against real CPython.
- **Sandbox escapes** — anything that could let sandboxed code reach the host
  filesystem, environment, network or subprocesses.
- **Resource-limit escapes** — untracked allocations (a `String` built without
  `StringBuilder`), unbounded loops, recursion without a depth guard.
- **Performance** — regressions introduced by the branch, and improvements you spot.
- **Verbose comments or docstrings** — flag them here; use the `review-verbosity`
  skill to actually rewrite them.
- **Code patterns that could be cleaned up** — duplication, misplaced logic, functions
  that have grown too complex.
- **`./limitations/`** — check the branch updates it wherever user-visible behaviour
  changed. A new divergence with no entry is a finding.

## Report

Write the findings as a concise report in your response, ordered most to least severe.
For each finding give `file:line`, what is wrong, and the concrete failure it causes.
Do not pad the report with things the branch got right.

Report only, unless the user asks for fixes.
