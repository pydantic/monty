---
name: review-verbosity
description: Rewrite excessively verbose comments and docstrings added by this branch, and delete tautologous ones. Use after writing a feature to tighten up its comments; this skill edits code rather than only reporting.
---

# Verbosity review

New comments and docstrings are usually too verbose. Rewrite them concisely. This skill
**edits** — it doesn't just report.

```bash
git diff --stat origin/main...HEAD   # which crates changed
git diff origin/main...HEAD
```

Dispatch one sub-agent per changed area — a crate, or any other group of changed files,
so nothing outside `crates/` is missed — then review their edits yourself. A single pass
over a large diff won't be exhaustive. In every comment the branch touched:

- **Cut it down.** Comments and field docstrings rarely over 3 lines, mostly 1; function
  and struct docstrings <= 5.
- **Delete tautology** — restating what the code plainly says earns nothing.
- **Delete narration** of the obvious, restated type signatures, how the code came to
  be, hedging.
- **Keep the motivation** — why the code exists and its foot-guns are the valuable part.
  Don't cut real information to hit a line count.
- **Drop over-long examples.** Public items only, <= 8 lines, and every one must run in
  tests — never `ignore`.
- **Fix what's out of date.** A comment the branch made wrong gets corrected, not left.

Python docstrings are markdown: single backticks, never RST double-backticks.

Only touch what this branch added or changed, unless asked for a wider sweep. Don't
change behaviour — if a comment is wrong because the *code* is wrong, say so rather than
rewording it to match. Concision, not terseness: an unfollowable comment is worse than a
long one.

```bash
make format-rs && make lint-rs
git diff --stat   # format-rs is `cargo +nightly fmt --all` — check it touched only your files
```

Then report what you cut, by file, in a couple of lines — not a full inventory.
