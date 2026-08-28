# monty-alloc

The global allocator [Monty](https://github.com/pydantic/monty) workers run
under: it counts live bytes against the sandbox session's soft and hard memory
limits, and ends the process if the hard limit or system allocator refuses.

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

// After each request, from the session the worker now holds.
monty_alloc::set_limit(Some(8 * 1024 * 1024), false).unwrap();
```

The soft limit is the worker baseline plus the session's budget. The interpreter
reads real usage at execution checkpoints and raises `MemoryError` after crossing
it. A higher hard limit leaves room for exception machinery and allocations
between checkpoints; crossing it ends the worker. `None` lifts both limits. See
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
