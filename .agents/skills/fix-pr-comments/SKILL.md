---
name: fix-pr-comments
description: Read every review comment on the current PR, fix the ones that need addressing, and mark all the threads resolved. Use when asked to deal with PR comments, review feedback, or bot review findings.
---

# Fix PR comments

Go through the review comments on the pull request, decide which ones are real, fix
those, and resolve the threads.

## Instructions

### 1. Read the comments

```bash
gh pr view --json number,title,url
gh pr view --comments                  # top-level conversation
```

Inline review threads need GraphQL, which is also where the thread IDs and resolved
state come from:

```bash
gh api graphql -f query='
query($owner:String!, $repo:String!, $pr:Int!) {
  repository(owner:$owner, name:$repo) {
    pullRequest(number:$pr) {
      reviewThreads(first: 100) {
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          comments(first: 20) { nodes { author { login } body } }
        }
      }
    }
  }
}' -F owner=pydantic -F repo=monty -F pr=<PR_NUMBER>
```

Work through the unresolved threads. Include bot reviewers (cubic, Copilot, …) — they
are usually the bulk of them.

### 2. Judge each one

Read the code around each comment before deciding; a review comment is a claim, not a
fact. Sort them into:

- **Fix** — a real bug, security risk, CPython divergence, or a clear improvement.
- **Skip** — wrong, already handled elsewhere, or a style preference that contradicts
  `CLAUDE.md`. Project convention wins over a bot's generic advice.

Do not fix something just because it was flagged, and do not skip something merely
because fixing it is awkward.

### 3. Fix

Make the changes, then:

```bash
make format-rs && make lint-rs   # if Rust changed
make lint-py                     # if Python changed
make test                        # or the specific test binary
```

Add a test for anything that was an actual bug.

### 4. Resolve the threads

Resolve every thread you looked at — both the ones you fixed and the ones you decided
against:

```bash
gh api graphql -f query='
mutation($id: ID!) {
  resolveReviewThread(input: {threadId: $id}) { thread { isResolved } }
}' -F id=<THREAD_ID>
```

Reply first on any thread you are **not** acting on, so the reasoning is on the record:

```bash
gh pr comment <PR_NUMBER> --body '...'
```

### 5. Report

Summarise in your response: what you fixed, what you skipped and why. Keep it short.

## Guidelines

- Commit and push only if the user asks — see `CLAUDE.md`.
- If a comment points at a genuine design problem too large to fix in passing, say so
  and leave the thread unresolved rather than papering over it.
