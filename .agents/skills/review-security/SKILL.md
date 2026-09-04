---
name: review-security
description: Security review of the current branch against its merge base — sandbox escapes, memory errors, panics and resource-limit bypasses. Use when reviewing changes for security risk, or before merging anything touching heap.rs, path_security.rs, the wire protocol or the pool.
---

# Security review

Monty runs untrusted, potentially malicious Python. Review this branch on that basis.

```bash
git diff origin/main...HEAD
```

Use a subagent to run `.agents/skills/fix-pr-comments/pr-threads.sh` (from the
`fix-pr-comments` skill) for security findings already raised on the PR, and confirm
each is properly addressed.

Cover the changes **and any code they touch** — a caller made unsafe by a changed callee
is in scope even if it isn't in the diff. Ask:

- **Sandbox escape?** Filesystem access outside a mount, path traversal, symlinks
    resolving out of bounds, network, subprocesses, import-system abuse, callback misuse,
    leaks through error messages or timing.
- **Memory errors?** Worse than panics: nothing stops, state is silently corrupt, and it
    can become arbitrary execution. `unsafe`, refcount errors causing use-after-free or
    double-free, unchecked indexing, aliasing violations, integer overflow feeding a
    length or index.
- **Resource limits bypassed?** Allocations dodging the `ResourceTracker` (`String`
    without `StringBuilder`), loops with no fuel check, small input → huge allocation.
- **Untrusted input still untrusted?** Wire frames from a child are hostile: decoding
    and proto→Rust conversion must validate everything and never panic. (Snapshots and
    dumps are trusted by contract — hosts sign and verify them.)
- **Panics or aborts?** `unwrap`/`expect` reachable from sandboxed input, unbounded
    recursion hitting a stack-overflow abort.
- **Mount escapes?** Any behaviour that allows sandbox code to escape a filesystem mount
    and read or alter files outside the mount point. This is particularly severe since
    mounts are run on the host/client connecting to a sandbox - accessing that environment
    is a very serious breach of the sandbox and security issue.

Weight both classes by where they land. In a pool worker the process dies, the parent
replaces the child and raises an exception — contained. Nothing else is: in host/parent
code (`monty-pool`, `monty-proto` decoding, `monty-fs`, the bindings), or in a Rust
embedder calling the `monty` crate in-process, the same bug takes down the application.
**Scrutinise those hardest**, especially anything handling a frame from a child.

`crates/monty/src/heap.rs` and `crates/monty-fs/src/path_security.rs` are the two most
security-critical files; any change to either needs careful justification. Also check the
public API: could a `pydantic_monty` or `@pydantic/monty` user misuse this to expose
their host?

## Report

Per finding: the attack, `file:line`, the sandboxed Python or hostile frame that triggers
it, and the impact. Demonstrate with `python-playground` rather than asserting where you
can. Say which areas you checked and found clean — coverage matters as much as findings.

Report only, unless the user asks for fixes.
