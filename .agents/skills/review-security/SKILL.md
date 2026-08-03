---
name: review-security
description: Security review of the current branch against its merge base — sandbox escapes, memory errors, panics and resource-limit bypasses. Use when reviewing changes for security risk, or before merging anything touching heap.rs, path_security.rs, the wire protocol or the pool.
---

# Security review

Security and safety are absolutely critical for Monty: it runs untrusted, potentially
malicious Python. Review this branch on that basis.

## Instructions

```bash
git fetch origin main
git diff origin/main...HEAD
```

Review the changes **and any code they touch** — a caller made unsafe by a changed
callee is in scope even if that caller is not in the diff. Ask specifically:

- **Could the sandbox be escaped?** Filesystem access outside a mount, path traversal,
  symlinks resolving outside the mount boundary, network access, subprocess execution,
  import-system abuse, external-function/callback misuse, information leaking through
  error messages or timing.
- **Could memory errors or panics be caused?** Any `unsafe`, refcount errors leading to
  use-after-free or double-free, unchecked indexing, integer overflow, unbounded
  recursion reaching a stack-overflow abort, `unwrap`/`expect` reachable from
  sandboxed input.
- **Could resource limits be exceeded?** Allocations that bypass the `ResourceTracker`
  (notably `String` built without `StringBuilder`), loops with no fuel check,
  amplification where a small input produces a huge allocation.
- **Is untrusted input still treated as untrusted?** Wire frames from a child process
  are hostile: decoding and proto→Rust conversion must validate everything and never
  panic. (Snapshots/dumps are trusted by contract — hosts sign and verify them.)

Pay extra attention to `crates/monty/src/heap.rs` and
`crates/monty-fs/src/path_security.rs`, the two most security-critical files. Any
change to either needs careful justification.

Also check the public API surface: could a developer using `pydantic_monty` or
`@pydantic/monty` plausibly misuse this change to expose their host?

## Report

Write a concise report in your response. For each finding: the attack, `file:line`,
the sandboxed Python (or hostile wire frame) that triggers it, and the impact. Where
you can, demonstrate it with the `python-playground` skill rather than asserting it.

Say plainly which areas you checked and found clean — for a security review, coverage
matters as much as the findings.

Report only, unless the user asks for fixes.
