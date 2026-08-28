# monty-alloc

The global allocator [Monty](https://github.com/pydantic/monty) workers run
under: it exposes live-byte usage to the interpreter and ends the process if
the worker's hard memory ceiling or system allocator refuses an allocation.

Monty executes untrusted Python, so a host has to be able to cap what a session
may allocate. Enforcing that in the allocator catches every byte the worker asks
for, wherever in the process it is asked for. Counting rather than asking the
kernel for `RLIMIT_AS` is what makes the limit portable and its units
meaningful: it bounds bytes the process requested, not virtual address space, so
mapped text, thread stacks and file mappings do not consume the budget. The
tradeoff is that it binds only what reaches this allocator — everything
sandboxed code can allocate, but not a direct `mmap`.

```rust
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

// The caller chooses headroom above the interpreter's soft limit.
let hard_budget = monty_types::memory_limit_with_headroom(Some(8 * 1024 * 1024), false);
monty_alloc::set_hard_limit(hard_budget).unwrap();
```

The interpreter compares live usage with the session's soft limit at execution
checkpoints and raises `MemoryError` after crossing it. The caller gives this
crate a higher hard budget, leaving room for exception machinery and allocations
between checkpoints; this crate adds the worker baseline and ends the worker if
that ceiling is crossed. `None` lifts the hard ceiling. See
`limitations/resource_limits.md` for how each outcome surfaces to a host.

## Ending the process

Exceeding the hard limit cannot raise a Python exception — it happens below the
interpreter — so the worker dies and its host replaces it. Neither a panic
(whose machinery allocates) nor a plain abort will do: `SIGABRT` is also what a
stack overflow produces, and a host that cannot tell those apart cannot report
`MemoryError`.

The `exit-code` feature picks how the process ends:

- **on** — `process::exit(monty_types::OOM_EXIT_CODE)`, the dedicated status a
  parent reads to classify the death. Used by the `monty subprocess` worker,
  whose parent is [`monty-pool`](https://crates.io/crates/monty-pool).
- **off** (default) — `process::abort()`, which on wasm is a trap. A wasm
  module has no exit status to offer, and its host already treats a turn that
  ends without a terminating event as a dead instance.

Only a binary or a wasm module may declare a `#[global_allocator]`: in a native
cdylib it would hijack the allocator of the embedding host process.

## Only crates in this workspace

Published so the `monty` binary can be, not for direct use. On a 32-bit target
a limit near 4 GiB saturates the arithmetic and leaves the worker
uncapped.
