---
name: review-security
description: Security review of the current branch against its merge base — sandbox escapes, memory errors, panics and resource-limit bypasses. Use when reviewing changes for security risk, or before merging anything touching the heap/ module, path_security.rs, the wire protocol or the pool.
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
is in scope even if it isn't in the diff.

## The sandbox worker

The interpreter and the worker process around it run the untrusted code. Ask:

- **Sandbox escape?** Network, subprocesses, import-system abuse, callback misuse,
    leaks through error messages or timing.
- **Memory errors?** Worse than panics: nothing stops, state is silently corrupt, and it
    can become arbitrary execution. `unsafe`, refcount errors causing use-after-free or
    double-free, unchecked indexing, aliasing violations, integer overflow feeding a
    length or index.
- **Ownership across teardown:** before a call that can drop, cancel or settle a value,
    confirm every handle used afterwards was taken or `inc_ref`'d first. Require a
    fire-and-forget test with no sandbox reference; tests that hold or await the handle
    mask scheduler-only ownership.
- **Host-call resumption:** any host-call reply (including sleep and timer) must run the
    ready queue before the suspended task continues. A test that asserts only the pending
    set can encode the starvation bug.
- **Resource limits bypassed?** Allocations dodging the `ResourceTracker` (`String`
    without `StringBuilder`), loops with no fuel check, small input → huge allocation.
- **Untrusted input still untrusted?** Wire frames from a child are hostile: decoding
    and proto→Rust conversion must validate everything and never panic. Snapshot provenance
    and integrity are the host's responsibility. Invalid snapshots may panic, abort, hang or
    produce wrong results, but must not cause UB; do not require semantic validation solely
    for tampered snapshots. Keep checks needed for memory safety or transport compatibility.
- **Panics or aborts?** `unwrap`/`expect` reachable from sandboxed input, unbounded
    recursion hitting a stack-overflow abort.

Weight both classes by where they land. In a pool worker the process dies, the parent
replaces the child and raises an exception — contained. Nothing else is: in host/parent
code (`monty-pool`, `monty-proto` decoding, `monty-fs`, the bindings), or in a Rust
embedder calling the `monty` crate in-process, the same bug takes down the application.
**Scrutinise those hardest**, especially anything handling a frame from a child.

## The client and parent

The subprocess parent and the gRPC client (`monty-pool` and the bindings) run inside the
user's application. A vulnerability here is even more critical than one in the worker,
since the client always runs in a trusted environment with no sandbox around it. A
sandbox, a hostile frame or a hostile server must not be able to:

- **Harm the client**: a panic, a memory error, excessive memory, excessive compute or an
    excessive delay.
- **Contaminate another session**: one session's data reaching, corrupting or being
    observed by another — a worker reused without a clean reset, a shared buffer or
    stream, a mixed-up id. This is a very serious incident; treat any doubt as a finding.
- **Escape a mount**: mounts are serviced here, on the client, so filesystem access
    outside a mount — path traversal, symlinks resolving out of bounds — reads or alters
    the user's environment. A very serious breach.

The `crates/monty/src/heap/` module and `crates/monty-fs/src/path_security.rs` are
security-critical; any change to either needs careful justification. Also check the
public API: could a `pydantic_monty` or `@pydantic/monty` user misuse this to expose
their host?

## Report

Per finding: the attack, `file:line`, the sandboxed Python or hostile frame that triggers
it, and the impact. Demonstrate with `python-playground` rather than asserting where you
can. Say which areas you checked and found clean — coverage matters as much as findings.

Report only, unless the user asks for fixes.
