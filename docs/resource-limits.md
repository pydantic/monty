# Resource Limits

Untrusted code will eventually try to allocate forever or loop forever.
Monty enforces hard limits on memory, execution time and recursion depth, configured per session.

```python
from pydantic_monty import Monty, MontyRuntimeError

limits = {
    'max_memory': 10_000_000,
    'max_duration_secs': 1.0,
    'max_recursion_depth': 100,
}

with Monty() as pool:
    with pool.checkout(limits=limits) as session:
        try:
            session.feed_run('x = [0] * 100_000_000')
        except MontyRuntimeError as exc:
            print(exc.display(format='type-msg').split(':')[0])
            #> MemoryError
```

## The five settings

| Key | Meaning |
| --- | --- |
| `max_memory` | Maximum heap memory in bytes |
| `max_duration_secs` | Maximum cumulative execution time in seconds |
| `max_recursion_depth` | Maximum function call stack depth (default 1000) |
| `gc_interval` | Run garbage collection every N allocations |
| `max_suspensions` | Maximum host round trips (external calls, `os` callbacks, name lookups, future resolution) per session |

Every key is optional.
Omit `max_memory`, `max_duration_secs` or `max_suspensions`, or set them to `None`, to disable that limit.
`max_recursion_depth` cannot be disabled: omitting it, or passing `None`, leaves the 1000-frame default.
`gc_interval` omitted or `None` uses the built-in schedule of every 100,000 allocations; collection cannot be turned
off.

In JavaScript the same fields are `maxMemory`, `maxDurationSecs`, `maxRecursionDepth`, `gcInterval` and
`maxSuspensions`, passed as `limits` to `pool.checkout()`.
In Rust they are the fields of `monty_types::ResourceLimits`, where the duration is a `Duration` named `max_duration`.

## Memory

`max_memory` budgets the bytes a worker requests from its global allocator, counted from the leanest the worker process
has been.
Everything the session allocates counts against it, including retained compiled code and interpreter internals.
It is not a ceiling on process RSS.
Allocations are counted as requested, so per-allocation overhead and fragmentation sit outside the count, as does memory
obtained without the allocator: thread stacks, the binary's mapped image, a direct `mmap`.
Size the limit with headroom, and use an OS or cgroup limit to bound the process itself.

Operations whose result size is predictable from their inputs are **pre-checked before allocating**, above a 100 KB
threshold: integer multiplication, left shift, integer power, sequence repeat (`'x' * n`), `str.replace` /
`bytes.replace`, the padding methods, and f-string formatting with a dynamic width or precision.
So `'x' * 10**12` fails immediately rather than after consuming the machine's memory.

A few integer operations carry their own caps regardless of `max_memory`:

- `base ** exp` with an exponent above `u32::MAX` raises `OverflowError`.
- `int(s, base)` rejects strings over 4,300 digits before the quadratic BigInt parse, matching CPython's
  `sys.int_info.default_max_str_digits`.

## Time

`max_duration_secs` counts **cumulative execution time**, not wall clock:

- The clock runs only while the interpreter executes bytecode.
- It is paused while execution is suspended waiting on the host — a [host function](host-functions.md) that takes a
  minute costs nothing.
- It accumulates across `feed_run` calls for the life of the session.
- It is serialized into [snapshots](snapshots.md), so a restored session resumes its budget rather than restarting from
  zero.
- There is no way for sandboxed code to observe the budget or the time remaining.

The in-sandbox check runs at interpreter checkpoints, so it cannot catch code that wedges the interpreter itself.
Two host-side backstops cover that:

- **`request_timeout`** on the pool is a hard per-turn deadline.
  A worker that exceeds it is killed and the call raises `MontyCrashedError` with `timed_out=True`.
  Each resume after a host-function or mount call starts a new deadline, so a program that suspends often can outlive
  any single timeout.
- **The duration backstop.** For sessions with a `max_duration_secs` limit, the worker reports its execution time on
  every protocol turn, and the host kills the worker a grace period after the budget expires.
  The grace period defaults to 1 second; in JavaScript it is the `durationLimitGrace` pool option (`null` disables it),
  and from Python it is not currently configurable.

Set `max_duration_secs` for untrusted code that may suspend repeatedly; `request_timeout` alone does not bound the
overall call.

## Recursion

Python-level call depth defaults to **1000 frames**; the 1001st nested call raises `RecursionError`.
Unlike the memory and time limits, `RecursionError` is catchable inside the sandbox, matching CPython.
Sandboxed code cannot raise the ceiling — `sys.setrecursionlimit` is not available in production builds.

Each `await` boundary counts as one frame, so `await` chains do not amplify depth.

Callbacks the interpreter evaluates synchronously — `map()`, `filter()`, `sorted(key=...)`, `min`/`max(key=...)`,
recursive `__repr__`/`__str__` — re-enter on the native Rust stack rather than the heap-allocated frame stack.
Those are capped independently at a lower fixed depth, so Monty raises `RecursionError` before a native stack overflow
could abort the process.

## Suspensions

`max_suspensions` counts external calls, host-object method calls and construction, lazy attribute lookups, `os`
callbacks, name lookups and future-resolution events.
These host round trips are outside `max_memory`; each [`ClassType`](host-objects.md) construction with `init=True` also
adds an instance-store entry.
Because `max_duration_secs` pauses during suspensions, a snippet could otherwise retry rejected calls indefinitely.

The pool enforces the limit per checkout.
A host driving the interpreter directly counts suspensions and calls `abort` itself; the limit only travels in the
`ResourceTracker`, see the [Rust quickstart](quickstart/rust.md).
The first suspension over the budget aborts the feed with an uncatchable
`RuntimeError: suspension limit exceeded: 4 > 3` at the call site.
The session stays consistent and can be dumped; later feeds run until they suspend.
Restoring a dump preserves the limit but resets the count to zero; a `max_suspensions` set on the restoring checkout
caps the dump's, so a worker cannot report a looser one.

## What is not covered

- **Compilation time.** Parsing and bytecode compilation happen before the VM exists and are not charged to the duration
  budget; memory retained by compiled code does count toward `max_memory` in workers.
  Compilation has its own structural caps (AST nesting at 200 levels, bytecode operand sizes, comprehension nesting, and
  a 1,024-copy cap on `finally` expansion that raises `SyntaxError`).
  A host accepting untrusted source should still isolate compilation, as the subprocess and WebAssembly runtimes do.
- **Print collectors.** `CollectString` and `CollectStreams` live in the host process, so their 10 MiB default cap is
  separate from `max_memory`.
- **Mount memory.** Each [mount](filesystem.md) has its own `memory_usage_limit`, defaulting to 100 MB, shared between
  retained overlay data and transient results.
- **`json.loads` nesting**, capped at 200 levels independently of the recursion limit.
- **The host instance store.** Every `ClassInstance`/`ClassType` wrapper sent into a session (nested wrappers,
  `init=True` constructions and `convert_value` wraps included) is retained in the host process until the session
  ends; re-sending a wrapper with the same id reuses its entry, distinct wrappers accumulate; see
  [host objects](host-objects.md#values-returned-by-methods).

## After a limit fires

A memory or time limit is **terminal**.
Sandboxed code cannot catch it, and once it fires **no guarantees are made about heap state or reference counts** — the
heap may hold orphaned objects with wrong refcounts.
Discard the session rather than continuing to run code in it.
`max_suspensions` also raises uncatchably, but ends the feed cleanly.
The session remains usable until code suspends again.

The pool does **not** do this for you.
The checkout stays open and accepts further `feed_run` calls.
Because `max_duration_secs` is a cumulative budget, once it is spent every later feed immediately fails with the same
`TimeoutError`; after a `max_memory` trip a later feed may quietly succeed against a heap you can no longer trust.
Ending the session is your job.
A caught `RecursionError` is the exception; it does not invalidate anything and execution may continue.

Full details, including the exact pre-check thresholds, live in
[`limitations/resource_limits.md`](https://github.com/pydantic/monty/blob/main/limitations/resource_limits.md).
