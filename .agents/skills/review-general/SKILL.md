---
name: review-general
description: Review the current branch against its merge base for bugs, CPython divergence, sandbox escapes, resource-limit escapes, performance regressions, verbose comments and missing ./limitations/ or docs/ updates. Use for a general pre-merge review of a branch or PR.
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
- **Lifetime ordering** — a handle used after a call that can drop, cancel or settle its
    owner (for example `cancel_task` or `cleanup_current_task`) must have been taken or
    `inc_ref`'d first; require the fire-and-forget test shape where the sandbox holds no
    reference.
- **Async path parity** — direct resume, pending future, eager/timer and failure paths
    share their invariants; no stale ready-queue entry, no dispatch without a current
    task, cleanup before returning an error.
- **CPython divergence** — different results, exception types or messages, missing
    attributes. Check anything you're unsure of with `python-playground`.
- **Sandbox escapes** — sandboxed code reaching the host filesystem, environment,
    network or subprocesses.
- **Resource-limit escapes** — allocations not charged to the tracker (an unbounded or
    amplifying `String` build without `StringBuilder`), unbounded loops, recursion without
    a depth guard.
- **Peak allocations** — for unbounded or input-amplifying allocations, preflight the
    peak live set: source, destination, temporaries, container growth and the new heap
    entry, not one copy; terminal resource errors must not become catchable task failures.
- **Performance** — regressions the branch introduces, and improvements you spot.
- **Verbose comments** — docstrings and comments should be concise as per `AGENTS.md`.
- **Cleanups** — misplaced logic, functions grown too complex.
- **Duplicated code** — repeated logic that belongs in a shared helper or method.
- **Early returns instead of a tail expression** — a function that `return`s from several
    branches, where one `if`/`match` expression ending in a tail expression would be cleaner.
    Guard clauses at the top of a function are fine (see `AGENTS.md` "Code style").
- **Newspaper order** — public and primary functions at the top of a file, with private
    helpers and sub-functions below the function that uses them (see `AGENTS.md` "NOTES").
- **`./limitations/`** — a new divergence with no entry is a finding.
- **Docs parity** — a user-visible change reflected in only one of `README.md`, `docs/`,
    `limitations/` and the crate READMEs is a finding. `AGENTS.md` "Documentation surfaces
    that must stay in sync" has the obligation table. For a full pass, delegate to the
    `docs-parity-reviewer` subagent.
- **Renames and rustdoc links** — after a rename, run
    `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc -p <crate> --no-deps`;
    module docs, ownership notes, test headers and limitations pages must describe the
    new API.

## Report

Concise, most severe first. Per finding: `file:line`, what's wrong, the concrete failure
it causes. Don't pad with what the branch got right.

Report only, unless the user asks for fixes.
