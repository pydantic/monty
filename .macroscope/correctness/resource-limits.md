---
include:
  - "crates/**/*.rs"
exclude:
  - "crates/monty-bench/**"
  - "crates/fuzz/**"
---

Sandboxed Python can drive allocation, so the model to apply is: every
allocation whose size an attacker can influence must be both **bounded** and
**accounted**. Accounting happens two ways -- an explicit preflight charge to the
`ResourceTracker`, or the per-instruction `check_time()` poll that bounds any
work done between VM instruction boundaries. And a resource limit is a
**terminal** failure of the whole execution context, not a recoverable
per-operation error.

Flag allocations that escape that model: a size derived from untrusted input
with no corresponding charge or bound -- an amplifying `String` built without
`StringBuilder`, a collection grown by an untrusted count inside a native loop
that never yields to a VM boundary, a small input that fans out into a large
allocation. This is a resource-limit escape, so rate it high.

Do not flag the mirror image of the model, because it is correct by design and
reporting it is noise: an allocation already bounded by inputs that are
themselves charged; a fine-grained charge that a coarser mechanism (an outer
preflight, or the per-instruction poll) already bounds; or heap/refcount state
observed after a terminal resource limit -- the context is being torn down, so
that state is out of scope, not a leak. When unsure whether a charge is
redundant, prefer silence: a missed nit costs nothing, a false "unbounded
allocation" trains the team to ignore the check.
