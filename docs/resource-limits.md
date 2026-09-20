# Resource Limits

Untrusted code will eventually try to allocate forever or loop forever.
Monty enforces hard limits on memory, execution time and recursion depth, configured per session.

=== "Python"

    ```python
    from pydantic_monty import Monty, MontyRuntimeError

    limits = {
        'max_memory': 10_000_000,
        'max_feed_duration_secs': 1.0,
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

=== "TypeScript"

    ```ts
    import { Monty, MontyRuntimeError } from '@pydantic/monty'

    const limits = {
      maxMemory: 10_000_000,
      maxFeedDurationSecs: 1,
      maxRecursionDepth: 100,
    }

    await using pool = await Monty.create()
    await using session = await pool.checkout({ limits })
    try {
      await session.feedRun('x = [0] * 100_000_000')
    } catch (err) {
      if (!(err instanceof MontyRuntimeError)) throw err
      console.log(err.display('type-msg').split(':')[0]) // MemoryError
    }
    ```

## The six settings

| Key                      | Meaning                                                                                                               |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------- |
| `max_memory`             | Maximum heap memory in bytes                                                                                          |
| `max_feed_duration_secs` | Maximum execution time per `feed_run`, in seconds                                                                     |
| `max_turn_duration_secs` | Maximum execution time per host round trip, in seconds                                                                |
| `max_recursion_depth`    | Maximum function call stack depth (default 1000)                                                                      |
| `gc_interval`            | Run garbage collection every N allocations                                                                            |
| `max_suspensions`        | Maximum host round trips (external calls, `os` callbacks, name lookups, future resolution) per session (default 1000) |
| `max_total_sleep_secs`   | Maximum cumulative time the host waits on `time.sleep()` and `asyncio.sleep()`, in seconds                            |

Every key is optional.
Omit `max_memory`, either duration key or `max_total_sleep_secs`, or set them to `None`, to disable that limit.
`max_recursion_depth` and `max_suspensions` cannot be disabled: omitting either, or passing `None`, leaves its 1000
default.
`gc_interval` omitted or `None` uses the built-in schedule of every 100,000 allocations; collection cannot be turned
off.

In JavaScript the same fields are `maxMemory`, `maxFeedDurationSecs`, `maxTurnDurationSecs`,
`maxRecursionDepth`, `gcInterval`, `maxSuspensions` and `maxTotalSleepSecs`, passed as `limits` to `pool.checkout()`.
In Rust they are the fields of [`monty_types::ResourceLimits`](api/rust/monty-types.md#resourcelimits), where the
durations are `Duration`s named `max_feed_duration`, `max_turn_duration` and `max_total_sleep`.

## Memory

`max_memory` budgets the bytes a worker requests from its global allocator, counted from the leanest the worker process
has been.
Everything the session allocates counts against it, including retained compiled code and interpreter internals.
It is not a ceiling on process RSS.
Allocations are counted as requested, so per-allocation overhead and fragmentation sit outside the count, as does memory
obtained without the allocator: thread stacks, the binary's mapped image, a direct `mmap`.
Size the limit with headroom, and use an OS or cgroup limit to bound the process itself.

Operations whose result size is predictable from their inputs are **pre-checked before allocating**, above a 100 KB
threshold, including integer multiplication, division and `divmod`, left shift, integer power, sequence repeat
(`'x' * n`), `str.replace` / `bytes.replace`, `re.sub`, the padding methods, deque rotation and slicing, materialising
an iterator into a container, and f-string, `str.format()` or `%` formatting with a dynamic width or precision.
So `'x' * 10**12` fails immediately rather than after consuming the machine's memory.

Containers a program grows one element at a time are pre-checked as well, at the point the buffer would reallocate
rather than on every push: `list.append` and `list.insert`, `deque.append` and `deque.appendleft`, `set.add`, and
assigning a new dict key.
So are the value buffers a single call fills: the argument pack behind `f(*args)`, the array `json.loads` parses, the
pieces `re.split` collects, and the list `re.findall` builds for a pattern with at most one capture group.
A wider `findall`, and `re.finditer`, allocate an object per match and are not covered — see
[the limitations note](limitations/resource_limits.md).

A few integer operations carry their own caps regardless of `max_memory`:

- `base ** exp` with an exponent above `u32::MAX` raises `OverflowError`, except for bases 0, 1 and -1.
- `int(s, base)` rejects strings over 4,300 digits before the quadratic BigInt parse when the base is not a power of
    two, matching CPython's `sys.int_info.default_max_str_digits`.

## Time

Both duration limits count **execution time**, not wall clock:

- The clock runs only while the interpreter executes bytecode.
- It is paused while execution is suspended waiting on the host — a [host function](host-functions.md) that takes a
    minute costs nothing, and neither does a `time.sleep()` or `asyncio.sleep()`, which the host waits out.
    Under the default `'system'` sleep mode the pool charges those sleeps to `max_total_sleep_secs` instead, refusing
    a sleep that would take the total over with an uncatchable `TimeoutError`; each is also a suspension, so
    `max_suspensions` bounds a sleeping loop as well. Under `'call_host'` the host waits uncharged, and a `'zero'`
    sleep does not suspend at all. See [security](security.md#waiting).
- There is no way for sandboxed code to observe a budget or the time remaining.
    `os_policy={'process_time': 'elapsed'}` exposes this same execution clock as `time.process_time()`; the
    default `'zero'` keeps it hidden.

They read the same clock and differ only in when it restarts:

| Key                      | Restarts            | Bounds                                            |
| ------------------------ | ------------------- | ------------------------------------------------- |
| `max_feed_duration_secs` | at each feed        | one feed, spanning every host round trip it makes |
| `max_turn_duration_secs` | at each host answer | the stretch of code between two host round trips  |

So a turn's time is also charged to its feed, and whichever budget is tightest fires first.
When one check blows both, the feed limit is reported.

There is no cumulative per-session budget: every limit restarts, so a long-lived session is bounded per request rather
than over its lifetime.
Cap what a session may cost in total from the host side, by counting the execution time each turn reports and ending
the checkout yourself.

`max_feed_duration_secs` is serialized into [snapshots](snapshots.md), so a restored session resumes that budget rather
than restarting from zero.
A snapshot is only ever taken between turns, so `max_turn_duration_secs` has nothing to carry.

Reach for `max_feed_duration_secs` to keep a long-lived session responsive per request, and
`max_turn_duration_secs` to bound one uninterrupted stretch of sandbox code, so a host driving the session gets control
back within a known time.
Neither bounds how long a host callback itself may take: the clock is paused for exactly that, and a host that needs
to bound its own waiting wants `request_timeout`.

Exceeding either raises `TimeoutError` in the sandbox.
The next feed resets both clocks, so the worker keeps serving the session — but a time limit stops the sandbox
mid-operation, leaving no guarantees about its heap.
Discard the session rather than feeding it again; see
[after a terminal resource error](limitations/resource_limits.md#after-a-terminal-resource-error).

### Host-side backstops

The in-sandbox check runs at interpreter checkpoints, so it cannot catch code that wedges the interpreter itself.
Host-side deadlines cover that:

- **`request_timeout`** on the pool is a hard per-turn deadline.
    A worker that exceeds it is killed and the call raises [`MontyCrashedError`][pydantic_monty.MontyCrashedError] with `timed_out=True`.
    Each resume after a host-function or mount call starts a new deadline, so a program that suspends often can outlive
    any single timeout.
- **A duration backstop per limit.** For each duration budget a session sets, the worker reports its consumed time on
    every protocol turn, and the host kills the worker a grace period after the budget expires.

The grace is what the sandbox gets to raise `TimeoutError` itself and keep the session alive; missing it costs the
session, so set it well above how long a checkpoint may be away.
Each grace defaults to 1 second and is a pool option: `feed_duration_limit_grace` and `turn_duration_limit_grace`
in Python, `feedDurationLimitGrace` and `turnDurationLimitGrace` in JavaScript.
`None` (`null` in JavaScript) disables that backstop, leaving only the in-sandbox check and `request_timeout`.

Set a duration limit for untrusted code that may suspend repeatedly; `request_timeout` alone does not bound the
overall call.
These deadlines are polled: synchronous host telemetry callbacks and decoding a large reply can delay enforcement.
Neither deadline covers [host mount I/O](filesystem.md#io-timeouts-and-cancellation).

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
callbacks (the sleeps among them in every mode but `'zero'`, the clock only under `'call_host'`), name lookups and
future-resolution events.
These host round trips are outside `max_memory`; each [`ClassType`](host-objects.md) construction with `init=True` also
adds an instance-store entry.
Because the duration limits pause during suspensions, a snippet could otherwise retry rejected calls indefinitely.

The pool enforces the limit per checkout; the default is 1000, and a host that needs more sets a larger number.
A host driving the interpreter directly counts suspensions and calls `abort` itself; the limit only travels in the
[`ResourceTracker`](api/rust/monty-types.md#resourcetracker), see the [Rust quickstart](quickstart/rust.md).
The first suspension over the budget aborts the feed with an uncatchable
`RuntimeError: suspension limit 3 exceeded` at the call site.
The session stays consistent and can be dumped; later feeds run until they suspend.
Restoring a dump preserves the limit but resets the count to zero; a `max_suspensions` set on the restoring checkout
caps the dump's, so a worker cannot report a looser one.

## What is not covered

- **Compilation time.** Parsing and bytecode compilation happen before the VM exists and are not charged to the duration
    budget; memory retained by compiled code does count toward `max_memory` in workers.
    Compilation has its own structural caps (AST nesting at 200 levels, bytecode operand sizes, comprehension nesting, and
    a 1,024-copy cap on `finally` expansion that raises `SyntaxError`).
    A host accepting untrusted source should still isolate compilation, as the subprocess and WebAssembly runtimes do.
    Source compiled at runtime by `eval()` / `exec()` is charged against the duration budget.
    Once a snippet starts executing, its compilation products stay allocated for the rest of the session;
    snippets rejected before execution retain none of them (see [eval_exec.md](limitations/eval_exec.md)).
- **Protocol decoding.** Protobuf frames have a separate cumulative allocation budget, covering generated messages
    and decoded values before allocation; see [message limits](limitations/host-values.md#message-size).
    It applies per frame, not to total host memory or subsequent host conversions.
- **Print collectors.** [`CollectString`][pydantic_monty.CollectString] and [`CollectStreams`][pydantic_monty.CollectStreams] live in the host process, so their 10 MiB default cap is
    separate from `max_memory`.
- **Mount memory.** Each [mount](filesystem.md) has its own `memory_usage_limit`, defaulting to 100 MB, shared between
    retained overlay data and transient results.
- **`json.loads` nesting**, capped at 200 levels independently of the recursion limit.
- **Host entropy.** Python's `AbstractOS.urandom()` raises `MemoryError` before allocating when a request exceeds
    `max_urandom_bytes`, 1 MiB by default (`OSAccess(max_urandom_bytes=...)`).
    A custom entropy callback allocates in the host process, so it must apply its own cap.
- **The host instance store.** Every [`ClassInstance`][pydantic_monty.ClassInstance]/[`ClassType`][pydantic_monty.ClassType] wrapper sent into a session (nested wrappers,
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
Both duration budgets restart, so a later feed runs — against a heap that a trip still leaves untrustworthy.
After a `max_memory` trip a later feed may likewise quietly succeed against a heap you can no longer trust.
Ending the session is your job.
A caught `RecursionError` is the exception; it does not invalidate anything and execution may continue.

Full details, including the exact pre-check thresholds, live in
[`limitations/resource_limits.md`](limitations/resource_limits.md).
