# `time` module

Monty implements two functions from `time`: `time.time()` and `time.sleep()`.
Both are answered by the sandbox itself by default, or handed to the host, as the session's `AutoOsCalls` say
(`auto_os_calls` on `checkout()` in the bindings, `AutoOsCalls` in Rust).

`asyncio.sleep()` is documented in [asyncio.md](asyncio.md); it shares
`time.sleep()`'s handling of the delay argument and its sleep mode.

## Module surface

Only `time` and `sleep` exist. Every other name — the monotonic clocks
(`monotonic`, `perf_counter`, `process_time`, `thread_time` and their `_ns`
forms), `time_ns`, `struct_time`, `localtime`, `gmtime`, `mktime`, `strftime`,
`strptime`, `asctime`, `ctime`, `tzset`, `timezone`, `altzone`, `daylight`,
`tzname` — raises `AttributeError` rather than being stubbed.

## `time.time()`

`time.time()` reads the same source as `date.today()` and `datetime.now()` (see
[datetime.md](datetime.md#reading-the-clock)): the machine's clock by default, a frozen instant under a fixed
`datetime`, or the host under `datetime='call_host'`; the session's `timezone` does not apply to it.
A frozen instant never advances, so `time.time()` returns the same value on every call, and under `call_host` it
returns whatever the host answered with — a `float` of seconds since the Unix epoch, as in CPython, but with no
guarantee it agrees with the machine's clock or moves forward at all.
Where nothing answers a `call_host` call it raises: through the bindings with no `os=` handler,
`RuntimeError: 'time.time' is not supported in this environment`; under standard (non-suspending) Rust execution,
`NotImplementedError: OS function 'time.time' not implemented with standard execution`.

## `time.sleep()`

What `time.sleep()` does is the session's `sleep` setting:

- `'system'` (the default): the sandbox waits, each call cut to `sleep_system_max` (10 seconds unless
    changed; the CLI's `--max-sleep`). A longer request returns early with no error, where CPython would have waited,
    so `time.time()` advances by less than the sleep asked for. In the wasm worker the wait is a busy spin on the
    monotonic clock rather than a blocking sleep, since a browser has no synchronous one to offer, so a sleeping
    browser worker occupies a core for the duration; `sleep_system_max` bounds each spin.
- `'call_host'`: the call suspends and the host performs the wait, so how long it actually sleeps is the host's
    choice: `pydantic_monty`'s [`OSAccess`][pydantic_monty.OSAccess] caps it at `max_sleep` (default 10 seconds,
    `None` for no cap). A host may answer with any value, which is discarded: `time.sleep()` always evaluates to
    `None`. A host answering it with a future gets
    `RuntimeError: time.sleep cannot be answered with a future` in the sandbox — the call is a wait, so there is
    nothing to resume into. Where nothing answers the call it raises as `time.time()` does above.
- `'zero'`: returns at once without waiting.

## Sleeping does not consume the execution-time limits

`max_feed_duration` and `max_turn_duration` measure execution time, and the clock stops while the sandbox waits — in
the sandbox or on the host — so a sleep costs nothing against them, however long it lasts.
A sandbox sleep is not a suspension either, so `max_suspensions` does not count it; it is charged to
`max_total_sleep` instead, the cumulative time the sandbox may sleep itself, and a sleep that would take the total
over is refused before it waits with an uncatchable `TimeoutError: sleep limit exceeded: <total> > <limit>` — the
Rust `Duration` debug renderings, e.g. `1.5s > 1s`. Without that limit a sandbox that sleeps in a loop ends on the
host's own turn deadline (`request_timeout` for the pools), reached after at most `sleep_system_max` per iteration.
Under `call_host` each sleep is one suspension (two when an `asyncio.sleep()` answered with a future is awaited
later), so `max_suspensions` (default 1000) bounds it as well, and `max_total_sleep` does not apply.
See [resource_limits.md](resource_limits.md).

## `time.sleep()` arguments

The argument is validated the same way in every sleep mode, before any wait.
The `OverflowError` past ~9223372036.85 seconds is CPython's,
`timestamp out of range for C PyTime_t`. What does not happen is the `OSError: [Errno 22] Invalid argument` CPython's platform sleep
raises for a delay just *under* that boundary: Monty accepts it, and `sleep_system_max` (or the host) cuts it short.
