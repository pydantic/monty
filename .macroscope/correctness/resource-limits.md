---
include:
  - "crates/**/*.rs"
exclude:
  - "crates/monty-bench/**"
  - "crates/fuzz/**"
---

Sandboxed Python can drive allocation, so the model to apply is: any allocation
whose size an attacker can influence must be bounded, and which mechanism bounds
it depends on the shape of the allocation.

- A single allocation sized from untrusted input -- one opcode computing a large
  size and allocating it in one shot -- must be preflighted with an explicit
  charge to the `ResourceTracker` before it allocates. `check_time()` cannot
  cover this: it only probes memory *already* allocated, so a one-shot
  allocation can overshoot `max_memory` or OOM before the next poll ever runs.
- An incremental native loop that allocates a bounded amount per iteration must
  call `check_time()` each iteration. The poll catches cumulative memory overage
  (and elapsed time) within one iteration's worth of overshoot, which is the
  correct and sufficient guard for that shape.

A preflight charge and the poll are therefore not interchangeable: the first
bounds the size of a single allocation, the second bounds the accumulation of
many small ones. And a resource limit is a terminal failure of the whole
execution context, not a recoverable per-operation error.

Flag allocations that escape this model: a one-shot allocation sized from
untrusted input with no preflight charge (an amplifying `String` built without
`StringBuilder`, a collection reserved to an untrusted count), or an unbounded
native loop with no `check_time()`. A resource-limit escape rates high.

Do not flag the mirror image, which is correct by design and whose reporting is
noise: an allocation already bounded by inputs that are themselves charged; an
incremental loop that already polls `check_time()` (do not also demand a
per-element preflight there); a fine-grained charge a correct outer preflight
already covers; or heap/refcount state observed after a terminal resource limit
-- the context is being torn down, so that state is out of scope, not a leak.
When unsure whether a charge is redundant, prefer silence: a missed nit costs
nothing, a false "unbounded allocation" trains the team to ignore the check.
