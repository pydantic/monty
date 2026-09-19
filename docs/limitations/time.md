# `time` module

Monty implements two functions from `time`, `time.time()` and `time.sleep()`, and the four zone constants
`timezone`, `altzone`, `daylight` and `tzname`.
The session's [automatic OS call policy](../security.md#the-clock) selects the clock, zone and sleep behavior.

`asyncio.sleep()` is documented in [asyncio.md](asyncio.md); it shares
`time.sleep()`'s sleep mode, while its delay argument follows CPython's `asyncio.sleep()` (a negative delay waits
zero seconds rather than raising).

## Module surface

Only `time`, `sleep`, `timezone`, `altzone`, `daylight` and `tzname` exist. Every other name — the monotonic clocks
(`monotonic`, `perf_counter`, `process_time`, `thread_time` and their `_ns`
forms), `time_ns`, `struct_time`, `localtime`, `gmtime`, `mktime`, `strftime`,
`strptime`, `asctime`, `ctime`, `tzset` — raises `AttributeError` rather than being stubbed.

## Zone constants

The constants describe the [session zone](datetime.md#reading-the-clock), a fixed offset, where CPython reads the
host's zone from the C library.
`timezone` and `altzone` are both the offset in seconds west of UTC, `daylight` is `0` and `tzname` repeats the
zone's name (`UTC±HH:MM` when it has none), so the default session reports `0, 0, 0, ('UTC', 'UTC')`.
Under `timezone='call_host'` the four names are absent and raise `AttributeError`: the module is created when it is
imported, without suspending to the host.

## `time.time()`

`time.time()` uses the [session clock](datetime.md#reading-the-clock), without applying `timezone`.
A fixed clock returns the same value on every call.
Under `'call_host'`, the handler's answer need not match the system clock or advance between calls.
An unanswered call raises `RuntimeError: 'time.time' is not supported in this environment` in the bindings, or
`NotImplementedError: OS function 'time.time' not implemented with standard execution` in non-suspending Rust execution.

## `time.sleep()`

The session's `sleep` setting determines the wait:

- `'system'` (the default) caps delays at `sleep_system_max`, ten seconds by default (`--max-sleep` in the CLI).
    Longer requests return early without error, unlike CPython.
- `'call_host'` delegates the uncapped delay to the `os=` handler.
    [`OSAccess`][pydantic_monty.OSAccess] caps it at `max_sleep` (default ten seconds, `None` for no cap).
    Unanswered calls raise as for `time.time()` above.
- `'zero'`: returns at once without waiting.

In both suspending modes a host may answer with any value, which is discarded: `time.sleep()` always evaluates to
`None`.
Answering with a future raises `RuntimeError: time.sleep cannot be answered with a future` in the sandbox.

## Sleeping does not consume the execution-time limits

Sleep suspensions pause `max_feed_duration` and `max_turn_duration`.
The pools and CLI instead enforce `max_suspensions` and, for system sleeps, `max_total_sleep`.
Non-suspending `MontyRun::run` waits inline and enforces neither limit.
Zero-mode sleeps and zero-delay system `asyncio.sleep()` do not suspend.
See [resource_limits.md](resource_limits.md#sleep) for accounting and errors.

## `time.sleep()` arguments

The argument is validated the same way in every sleep mode, before any wait.
The `OverflowError` past ~9223372036.85 seconds is CPython's,
`timestamp out of range for C PyTime_t`. What does not happen is the `OSError: [Errno 22] Invalid argument` CPython's platform sleep
raises for a delay just *under* that boundary: Monty accepts it, and `sleep_system_max` (or the `os=` handler) cuts it
short.
