# Security Model

Monty is designed to run code that a language model wrote and nobody reviewed.
This page describes what that buys you and what it does not.

!!! warning "Experimental"
    Monty is still in development and has not been independently audited.
    Treat these guarantees as design intent backed by tests, not as a certification.
    If you find a way out of the sandbox, please [open an issue](https://github.com/pydantic/monty/issues).

## What "secure" means here

Monty is a **language-level sandbox**, not an OS-level one.
There is no container, no seccomp filter and no VM.
The isolation comes from the interpreter itself: sandboxed code cannot express an operation that touches the host,
because the interpreter implements no such operation.

Concretely:

- **There is no ambient authority.** With no mounts and no host functions configured, the sandbox cannot read a file,
  read an environment variable, open a socket, or spawn a process.
  Not "it is blocked" — the capability does not exist in the bytecode VM.
- **The interpreter performs no filesystem I/O at all.** It suspends with a description of the operation it wants, and a
  host component decides what to do about it.
  All filesystem code lives in a separate crate (`monty-fs`) that worker artifacts do not even link in some builds.
- **The dangerous modules are absent, not stubbed.** `socket`, `subprocess`, `multiprocessing`, `threading` and `ctypes`
  are not importable, and are also missing from the bundled typeshed, so [type checking](type-checking.md) rejects code
  that uses them before it runs.
- **`eval`, `exec`, `compile`, `globals`, `locals` and `__import__` do not exist.**
- **No FFI, no C dependencies.** Nothing in the sandbox can call into native code.

## The two host-access mechanisms

Everything the sandbox can reach outside itself goes through one of two mechanisms, and both are opt-in per feed.

### Host functions

Names the sandbox does not define are resolved against the `external_lookup` you supply.
A callable entry becomes a function the sandbox can call: execution suspends, **your** code runs on the host with your
process's full authority, and execution resumes with the result.
See [host functions](host-functions.md).

Monty guarantees that the sandbox reaches nothing you did not hand it.
It cannot guarantee that what you handed it is safe.
A host function that takes a path and reads it, or takes a URL and fetches it, is an unconstrained filesystem or network
primitive that you wrote.
Validate arguments in the host function as you would validate any untrusted input.

### Mounts and the `os` callback

Host directories are mounted into the sandbox at virtual paths, and only inside a mount can `open()` and `pathlib` do
anything.
A separate `os=` callback handles operations no mount covers.
See [filesystem access](filesystem.md).

Confinement is structural rather than checked:

- Each mount opens a `cap_std::fs::Dir` descriptor once, at mount time, and every operation runs relative to it — `..`,
  symlinks and directories swapped mid-operation cannot reach outside the mount, because no resolution step could leave
  it.
- `..` and `.` are collapsed in the virtual namespace before anything touches the filesystem.
- Symlinks with absolute targets are refused in read-only and read-write mounts, even when the target is inside the
  mount; overlay mounts refuse symlinks entirely.
- Null bytes in any path component are rejected.
- Paths handed back to the sandbox (from `Path.resolve()`, for example) are virtual paths.
  A host path never leaks in.

`/tmp`, `/etc`, `/proc`, `/dev`, `~` and the host working directory are not reachable unless you mount them.

## Crash isolation

A Monty process can never be made fully crash-proof against memory errors — a stack-overflow abort or an allocator abort
takes down the process it happens in.
The Python package and the native `@pydantic/monty` binding therefore never run the interpreter in your process: every
session runs in a `monty` worker subprocess.

The WebAssembly build has no subprocess to use.
In a browser it runs off-thread in a `Worker`; under Node, which has no global `Worker`, `@pydantic/monty/wasm` runs
in-process outright.
See [in-process execution](#in-process-execution).

When a worker dies, the pool observes the death, discards the worker, spawns a replacement, and the call raises
`MontyCrashedError` (`PoolError::Crashed` in Rust).
The session is lost; your process is not.

Two more properties of the worker boundary matter:

- **Workers spawn with an empty environment** (Windows keeps only `SystemRoot`), so host secrets are never in a worker's
  memory to begin with.
- **The parent treats every frame from a worker as untrusted input.** A worker could in principle be compromised, so
  wire decoding validates everything, enforces depth and size budgets, and never panics on malformed data.
  A worker that violates the protocol is discarded.

From Rust, this is why [`monty-pool`](quickstart/rust.md) is the recommended entry point rather than the in-process
`monty` crate.

## Resource exhaustion

Untrusted code will try to allocate forever or loop forever.
See [resource limits](resource-limits.md) for the full picture; the security-relevant parts:

- `max_memory` is a budget on **user-visible data**, not a hard ceiling on process RSS.
  Per-object sizing is approximate and elides bookkeeping overhead.
  Size the limit with headroom, and keep the worker-level backstop.
- `max_duration_secs` counts **cumulative execution time**, not wall clock.
  The clock is paused while the sandbox waits on a host function, so a slow host function does not consume the budget.
  It accumulates across feeds for the life of the session.
- Because the in-sandbox time check only runs at interpreter checkpoints, both the pool's `request_timeout` and the
  automatic `duration_limit_grace` backstop kill the worker from outside.
  Keep at least one of them on when running untrusted code.
  Every local pool (`Monty`, `AsyncMonty`, JavaScript `Monty.create()`, `PoolConfig::subprocess`) defaults
  `request_timeout` to no deadline; only `AsyncMontyWebsocket` sets one, at 10 seconds.
  And `duration_limit_grace` only does anything for a session that set `max_duration_secs`, so a default local pool has
  neither backstop.
- **After a memory or time limit fires, no guarantees are made about heap state or reference counts.** Discard the
  session rather than continuing to run code in it.
  The pool does not do this for you, and the two limits do not even fail alike: a spent `max_duration_secs` budget is
  cumulative, so every later feed fails with the same `TimeoutError`, while after a `max_memory` trip a later feed may
  quietly succeed against a corrupted heap.
- Compilation is not charged against the duration budget.
  It has its own structural caps (AST nesting, bytecode operand sizes, comprehension nesting, `finally` expansion), but
  a host accepting untrusted source should still isolate compilation — as the subprocess and WebAssembly runtimes do.

## Where the guarantees weaken

### Your own callbacks

Host functions, the `os=` callback, and `CallbackFile` in the Python `OSAccess` helper all execute in the host process.
`OSAccess` backed by `MemoryFile` objects is fully sandboxed; `OSAccess` backed by `CallbackFile` is exactly as
sandboxed as the callback you wrote.

### In-process execution

The Rust `monty` crate and the WebAssembly in-process degrade run the interpreter in the calling process.
The language-level sandbox still holds, but crash isolation does not: an abort in the sandbox is an abort in your
process.
In the browser, a real `Worker` restores isolation and gives the watchdog a hard kill via `Worker.terminate()`; where no
`Worker` exists, the same API degrades to in-process with no preemption.

### Remote workers

`AsyncMontyWebsocket` (Python) and `PoolConfig::websocket` (Rust) dial a remote worker instead of spawning a local one.
**A remote peer need not be a Monty sandbox at all.** It may be real CPython with no sandbox, no resource limits and
full host access, relying on deployment isolation — a container or VM per session — rather than on the interpreter.
None of the guarantees on this page transfer across that boundary; they become properties of whatever is running on the
other end.

### Pin the worker binary

The worker binary is resolved from the explicit path you pass, then `MONTY_BIN`, then the bundled platform package, then
`PATH`.
When running untrusted code, pass the path explicitly rather than letting `PATH` decide which binary gets to be your
sandbox.

### Deserializing snapshots

[Snapshots](snapshots.md) are opaque bytes restored into a worker.
Treat a snapshot from an untrusted source the way you would treat any untrusted serialized data: restore it into a
worker you are willing to lose.

## The parts that are most security-critical

If you are reviewing or contributing to Monty, two files carry most of the weight:

- `crates/monty/src/heap.rs` — the heap and reference counting.
- `crates/monty-fs/src/mount_table.rs` — the mount boundary: the `Dir` descriptor every filesystem operation runs
  against, with `path_security.rs` beside it holding the virtual-path policy.

Changes to any of them need careful security review.
The repository's [`review-security` skill](https://github.com/pydantic/monty/tree/main/.agents/skills/review-security)
exists for exactly that.
