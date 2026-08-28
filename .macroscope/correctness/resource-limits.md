---
include:
  - "crates/**/*.rs"
exclude:
  - "crates/monty-bench/**"
  - "crates/fuzz/**"
---

Sandboxed Python can drive allocation, so the model to apply is: any allocation
whose size an attacker can influence must be preflighted, memory and time are
guarded by different mechanisms, and a limit failure is terminal for the sandbox
but not for Rust-side cleanup.

- **Preflight attacker-influenceable sizes.** An allocation sized from untrusted
  input must be charged to the `ResourceTracker` (`check_allocation`) before it
  allocates, and the charged size must be the size actually allocated. A combined
  allocation must preflight the *combined* size, not rely on its inputs having
  been charged individually -- two separately-charged inputs can still sum past
  `max_memory`, which is why `concat_bytes`, `concat_allocate_str`, and the
  list/tuple/deque growth paths preflight the result. `check_time()` cannot stand
  in here: it only probes memory already allocated, after the fact.
- **`check_time()` guards time, not allocation size.** Call it in a native loop
  that can run unboundedly long so a CPU limit can fire. Do not require a
  per-iteration `check_time()` for memory's sake in a bounded loop -- AGENTS.md
  explicitly discourages that, and an oversized case a preflight missed is the
  allocator hard limit's job, not a per-iteration poll's.

Flag an attacker-influenceable allocation (single or combined) that reaches the
allocator with no `check_allocation` of its actual size -- an amplifying `String`
built without `StringBuilder`, a buffer or collection reserved to an untrusted or
combined count -- or a native loop that can run unboundedly with no
`check_time()`. A resource-limit escape rates high.

Do not flag an allocation whose actual size is itself directly preflighted or is
a small bounded result; a bounded native loop that leans on the hard-limit
backstop instead of a per-iteration memory poll (that is the intended pattern);
or a fine-grained charge a correct outer preflight already covers.

On a terminal limit the sandbox context is discarded, so the Python-visible heap
state afterward carries no guarantee -- do not report that. But the error unwinds
through Rust, where `Drop` still runs, so a Rust-side leak or refcount imbalance
on the unwind path is a real defect (it matters for in-process embedders and is
covered by memory-model tests) -- treat it under drop-discipline, not as out of
scope. When unsure whether a charge is redundant, prefer silence: a missed nit
costs nothing, a false "unbounded allocation" trains the team to ignore the check.
