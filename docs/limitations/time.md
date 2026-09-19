# `time` module

Monty implements two functions from `time`: `time.time()` and `time.sleep()`.
`time.time()` is answered by the sandbox itself by default, or handed to the host, and `time.sleep()` is always the
host's wait, as the session's `AutoOsCalls` say (`auto_os_calls` on `checkout()` in the bindings, `AutoOsCalls` in
Rust).

A `'system'` sleep reaches a host driving suspensions itself (`feed_start`, `RunProgress::OsCall`, the JavaScript
turn objects) as the OS call `system.sleep` (`system.async_sleep` for `asyncio.sleep()`), already cut to the maximum,
distinct from the `time.sleep` / `asyncio.sleep` calls a `'call_host'` handler receives.

`asyncio.sleep()` is documented in [asyncio.md](asyncio.md); it shares
`time.sleep()`'s sleep mode, while its delay argument follows CPython's `asyncio.sleep()` (a negative delay waits
zero seconds rather than raising).

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

- `'system'` (the default): the call suspends with the delay cut to `sleep_system_max` (10 seconds unless changed;
    the CLI's `--max-sleep`), and the host waits that long itself, without its `os=` handler: the pools, the CLI, and
    standard Rust execution, which is its own host and waits inline. A longer request returns early with no error,
    where CPython would have waited, so `time.time()` advances by less than the sleep asked for.
- `'call_host'`: the call suspends uncut and the host's `os=` handler performs the wait, so how long it actually
    sleeps is the handler's choice: `pydantic_monty`'s [`OSAccess`][pydantic_monty.OSAccess] caps it at `max_sleep`
    (default 10 seconds, `None` for no cap). Where nothing answers the call it raises as `time.time()` does above.
- `'zero'`: returns at once without waiting.

In both suspending modes a host may answer with any value, which is discarded: `time.sleep()` always evaluates to
`None`. A host answering it with a future gets `RuntimeError: time.sleep cannot be answered with a future` in the
sandbox — the call is a wait, so there is nothing to resume into.

## Sleeping does not consume the execution-time limits

`max_feed_duration` and `max_turn_duration` measure execution time, and the clock stops while the sandbox is suspended,
so a sleep costs nothing against them, however long it lasts.
In the pools and the CLI each sleep is one suspension (two when an `asyncio.sleep()` answered with a future is
awaited later), so `max_suspensions` (default 1000) bounds a sandbox that sleeps in a loop; Rust's non-suspending
`MontyRun::run` waits out a `'system'` sleep inline instead, and a zero-delay `asyncio.sleep()` settles without
suspending at all.
Under `'system'` the host also charges each sleep to `max_total_sleep`, the cumulative time the sandbox may ask it
to wait, and refuses the sleep that would take the total over before waiting, with an uncatchable
`TimeoutError: sleep limit exceeded: <total> > <limit>` — the Rust `Duration` debug renderings, e.g. `1.5s > 1s`.
Like `max_suspensions`, the interpreter only stores that limit: the pools, the CLI (with `--max-total-sleep`) and
the wasm pool enforce it as they wait, and Rust's non-suspending `MontyRun::run`, which waits inline, applies no
total. Under `call_host` `max_total_sleep` does not apply.
See [resource_limits.md](resource_limits.md).

## `time.sleep()` arguments

The argument is validated the same way in every sleep mode, before any wait.
The `OverflowError` past ~9223372036.85 seconds is CPython's,
`timestamp out of range for C PyTime_t`. What does not happen is the `OSError: [Errno 22] Invalid argument` CPython's platform sleep
raises for a delay just *under* that boundary: Monty accepts it, and `sleep_system_max` (or the `os=` handler) cuts it
short.
