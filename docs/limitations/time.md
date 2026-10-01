# `time` module

Monty implements the `time` module's clocks, `sleep()`, the conversion functions and the four zone constants
`timezone`, `altzone`, `daylight` and `tzname`.
The session's [OS policy](../security.md#the-clock) selects the clock, zone, sleep and process-clock
behavior.

`asyncio.sleep()` is documented in [asyncio.md](asyncio.md); it shares
`time.sleep()`'s sleep mode, while its delay argument follows CPython's `asyncio.sleep()` (a negative delay waits
zero seconds rather than raising).

## Module surface

`time`, `time_ns`, `monotonic`, `monotonic_ns`, `perf_counter`, `perf_counter_ns`, `process_time`,
`process_time_ns`, `thread_time`, `thread_time_ns`, `sleep`, `gmtime`, `localtime`, `mktime`, `asctime`, `ctime`,
`strftime`, `strptime` and the four zone constants exist.
`struct_time` is not a name in the module (see below), and `tzset`, `get_clock_info`, `clock_gettime`,
`clock_settime`, `pthread_getcpuclockid` and the `CLOCK_*` constants raise `AttributeError` rather than being stubbed.

## The wall clocks

`time.time()`, `time_ns()`, `monotonic()`, `monotonic_ns()`, `perf_counter()` and `perf_counter_ns()` all read the
[session clock](datetime.md#reading-the-clock), without applying `timezone`.
Under `'call_host'` every one of them reaches the `os=` handler as the single OS function `time.time`, with the
name of the function that asked as its argument (`'time.monotonic'`, `'time.perf_counter_ns'`, ...), so a handler can
answer them differently or ignore the distinction.
The handler answers epoch seconds as a float in every case, so under `'call_host'` the `_ns` forms carry only the
float's precision — about 240 nanoseconds at the current epoch.
An unanswered call raises `RuntimeError: 'time.time' is not supported in this environment` in the bindings, or
`NotImplementedError: OS function 'time.time' not implemented with standard execution` in non-suspending Rust
execution.

Because `monotonic()` and `perf_counter()` are the wall clock:

- Their reference point is the Unix epoch, where CPython's is undefined and typically small.
- They are not strictly monotonic under the default `'system'` clock: a step in the host's clock moves them with it.
    CPython reads `CLOCK_MONOTONIC`, which cannot go backwards.
- A fixed clock returns the same value on every call, so `while time.perf_counter() - start < 1:` never terminates.

`time_ns()` raises `OverflowError: timestamp out of range for platform time_t` for a fixed clock outside roughly
1677..2262, the span an `int` of nanoseconds covers; CPython's `time.time_ns()` has no such bound.

## The process clocks

`process_time()`, `process_time_ns()`, `thread_time()` and `thread_time_ns()` read the session's `process_time` policy
rather than its clock, and never suspend.

- `'zero'` (the default) returns `0.0` / `0` on every call, so elapsed execution time is not observable in the
    sandbox even under the default system clock.
    A loop waiting on it never terminates.
- `'elapsed'` returns the session's accumulated execution time: the same clock `max_feed_duration` is charged
    against, which pauses while suspended on the host or sleeping.
    It is wall time while the interpreter runs, not CPU time, so it differs from CPython's while the process is
    descheduled.

Monty has no threads, so `thread_time()` is the same clock as `process_time()`.

## `struct_time`

`gmtime()` names its zone `UTC` (in `tm_zone` and `%Z`); glibc CPython says `GMT`.
eleven attributes.
It differs from CPython's structseq in two ways:

- **`tm_gmtoff` and `tm_zone` are tuple items**, so `len(t)` is 11 rather than 9, `t == (1970, 1, 1, ...)` against a
    9-tuple is `False`, and nine-way unpacking raises `ValueError`.
    `t[:9]` and the named attributes behave as in CPython.
- **`time.struct_time` is not a name in the module**, so `isinstance(t, time.struct_time)` raises `AttributeError`
    and `type(t)` is Monty's named-tuple type.
    Like the other structseqs (see [namedtuple.md](namedtuple.md)), instances have no `_fields`, `_replace` or
    `_asdict`.

Where CPython requires a time tuple of exactly nine items, Monty accepts nine or eleven, so its own `struct_time`
values are accepted everywhere CPython's are.

## `gmtime()`, `localtime()`, `mktime()`

`localtime()` breaks the instant down in the [session zone](datetime.md#reading-the-clock), setting `tm_isdst` from
whether the zone is on its daylight half at that instant and `tm_zone`/`tm_gmtoff` from the zone; CPython reads the
host's zone.
`mktime()` inverts `localtime()` in the same zone.
Its `tm_isdst` argument is ignored: an ambiguous or skipped wall time takes CPython's `fold=0` reading, where CPython
lets `tm_isdst=0` or `1` choose the half and guesses only for `-1`.

Without an argument `gmtime()`, `localtime()` and `ctime()` read the clock, and so suspend under `'call_host'`,
naming themselves as the caller.
An explicit `secs` outside the range a `datetime` can hold raises `OverflowError: timestamp out of range for platform time_t`; CPython's limit is the platform's `time_t`.

## `strftime()`

`time.strftime()` supports the directives `datetime.strftime()` does (see
[datetime.md](datetime.md#formatting)), reading `%Z` and `%z` from the tuple's own zone when it has one and from the
session zone's standard or daylight half (by `tm_isdst`) for a bare 9-tuple.
Two divergences from CPython's platform `strftime`:

- `%f` renders `000000`, since the shared formatter treats it as microseconds and a `struct_time` has none; glibc
    CPython emits `%f` verbatim.
- Unknown directives pass through verbatim, matching glibc CPython rather than macOS (`%Q` is `'%Q'`, not `'Q'`).

Without a time argument it reads the clock, suspending under `'call_host'` with `time.strftime` as the caller.

## `strptime()`

`time.strptime()` parses the directives `datetime.strptime()` does (see [datetime.md](datetime.md#datetime)).
A `%z` in the format is parsed and validated but its offset is not recorded on the result, which has
`tm_gmtoff`/`tm_zone` of `None`; CPython sets `tm_gmtoff` from it.

## Zone constants

The constants describe the [session zone](datetime.md#reading-the-clock); in CPython the same names read the
host's zone from the C library.
A named zone gives its standard and daylight halves from 1 January and 1 July of the clock's year, as CPython does:
`Europe/London` reports `0, -3600, 1, ('GMT', 'BST')`.
A fixed zone has one half: `timezone` and `altzone` are both the offset in seconds west of UTC, `daylight` is `0`
and `tzname` repeats the zone's name (`UTC±HH:MM` when it has none), so the default session reports
`0, 0, 0, ('UTC', 'UTC')`.
A named zone under `datetime='call_host'` leaves the four names absent, raising `AttributeError`: the module is
created when it is imported, without suspending to the host for the year.

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
