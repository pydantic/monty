# `time` module

Monty implements two functions from `time`, both answered by the host rather
than by the interpreter: `time.time()` and `time.sleep()`.

`asyncio.sleep()` is documented in [asyncio.md](asyncio.md); it shares
`time.sleep()`'s handling of the delay argument.

## Module surface

Only `time` and `sleep` exist. Every other name — the monotonic clocks
(`monotonic`, `perf_counter`, `process_time`, `thread_time` and their `_ns`
forms), `time_ns`, `struct_time`, `localtime`, `gmtime`, `mktime`, `strftime`,
`strptime`, `asctime`, `ctime`, `tzset`, `timezone`, `altzone`, `daylight`,
`tzname` — raises `AttributeError` rather than being stubbed.

## Both functions need a host

`time.time()` and `time.sleep()` suspend the sandbox with an OS call, the way
`date.today()` and `datetime.now()` do. A host that answers neither leaves them
raising:

- Through the pool (`pydantic_monty`, `@pydantic/monty`, `monty-pool`) they
    reach the `os=` handler. With no handler, `RuntimeError: 'time.time' is not supported in this environment`.
- Under standard (non-suspending) execution — `MontyRun::run`, and so the
    `monty` CLI running a file without mounts — `time.time()` is answered from
    the runner's `HostClock` (the machine's clock by default, `HostClock::Denied`
    raising `NotImplementedError`), but `time.sleep()` always raises
    `NotImplementedError: OS function 'time.sleep' not implemented with standard execution`. Nothing waits inside the interpreter: a wait has to happen where
    a deadline can be enforced.

Since the host performs the wait, how long `time.sleep()` actually sleeps is the
host's choice. A host may cap it, ignore it, or refuse it.

## The clock is the host's

`time.time()` returns whatever the host answered with, so it need not agree with
the machine's clock, need not advance between calls, and is not guaranteed to
move forward at all. It is a `float` of seconds since the Unix epoch, as in
CPython.

## Sleeping does not consume the execution-time limit

`max_duration` measures execution time, and the clock stops while the sandbox is
suspended — so a sleep costs nothing against it, however long it lasts. What
bounds sleeping instead:

- `max_suspensions` (default 1000), since each sleep is one suspension (two
    when an `asyncio.sleep()` answered with a future is awaited later). The
    pools and the CLI enforce it; a direct Rust host counts suspensions itself.
- The host's own turn deadline (`request_timeout` for the pools).

A sandbox that sleeps in a loop therefore ends its turn on the host's deadline
rather than on `MemoryError`/time limits. See
[resource_limits.md](resource_limits.md).

## `time.sleep()` arguments

The `OverflowError` past ~9223372036.85 seconds is CPython's,
`timestamp out of range for C PyTime_t`. What does not happen is the `OSError: [Errno 22] Invalid argument` CPython's platform sleep
raises for a delay just *under* that boundary: Monty accepts it and passes it
to the host, where it becomes a wait that outlives any turn deadline.

The hosts Monty ships cut long sleeps short rather than wait them out:
`pydantic_monty`'s [`OSAccess`][pydantic_monty.OSAccess] at its `max_sleep` (default 10 seconds, `None`
for no cap) and the `monty` CLI at `--max-sleep` (default 10). Sandboxed code
sees the call return early with no error, where CPython would have waited, so
`time.time()` advances by less than the sleep asked for.

A host may answer `time.sleep()` with a value, and it is discarded:
`time.sleep()` always evaluates to `None`. A host answering it with a future
instead gets `RuntimeError: time.sleep cannot be answered with a future` in the
sandbox — the call is a wait, so there is nothing to resume into.
