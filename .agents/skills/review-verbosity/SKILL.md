---
name: review-verbosity
description: Rewrite excessively verbose comments and docstrings added by this branch, and delete tautologous ones. Use after writing a feature to tighten up its comments; this skill edits code rather than only reporting.
---

# Verbosity review

New docstrings and comments are often excessively verbose. Rewrite them to communicate
the relevant information more concisely. This skill **edits** — it does not just report.

## Instructions

Review the branch against its merge base, exhaustively:

```bash
git fetch origin main
git diff --stat origin/main...HEAD   # which crates changed
git diff origin/main...HEAD
```

Dispatch one sub-agent per changed crate to go through that crate's diff in full — the
point is exhaustive coverage, which a single pass over a large diff will not achieve.
Have each sub-agent report the edits it made, then review the result yourself.

In every comment and docstring added or changed by the branch:

- **Cut it down.** Comments and field docstrings should almost never exceed 3 lines,
  mostly 1. Function and struct docstrings should be <= 5 lines.
- **Delete tautology.** A comment restating what the code plainly says earns nothing.
- **Delete unnecessary detail** — narration of the obvious, restated type signatures,
  history of how the code came to be, hedging.
- **Keep the motivation.** Why the code exists, what it is for, and the foot-guns of
  using it are the valuable parts. Do not remove important or relevant information to
  hit a line count.
- **Drop over-long examples.** Examples belong only on public items and must be <= 8
  lines; anything longer goes. Every example must run in tests — never `ignore`.
- **Fix anything out of date.** A comment the branch made wrong must be corrected, not
  left.

Python docstrings are markdown: single backticks, never RST double-backticks.

## Afterwards

```bash
make format-rs
make lint-rs
```

Report what you cut, by file, in a couple of lines — not a full inventory.

## Guidelines

- Only touch comments and docstrings this branch added or changed, unless the user asks
  for a wider sweep.
- Do not change code behaviour. If a comment is wrong because the *code* is wrong, say
  so rather than rewording the comment to match.
- Concision is the goal, not terseness: a comment nobody can follow is worse than a
  long one.
