# pydantic-monty

Python bindings for the Monty sandboxed Python interpreter.

Execution always happens in a pool of `monty` worker subprocesses: a monty
process can never be made fully crash-proof against memory errors (stack
overflows, allocator aborts) triggered by adversarial input, so crash
isolation is built in. A crashed worker raises `MontyCrashedError` and is
replaced transparently — your process is never at risk.

## Installation

```bash
uv add pydantic-monty
# or
pip install pydantic-monty
```

`pydantic-monty` is a metapackage with no code of its own; it installs the two
distributions that make up a working sandbox:

- [`pydantic-monty-client`](https://pypi.org/project/pydantic-monty-client/) —
    the `pydantic_monty` module you import (pool, sessions, value conversion)
- [`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/) —
    the `monty` worker binary the pool spawns, shipped the same way `uv` and
    `ruff` ship their binaries

Install `pydantic-monty-client` on its own when the worker binary comes from
somewhere else — a base image, a system package, a build of this repo — and
point `pydantic_monty` at it via `MONTY_BIN`, `binary_path=`, or `PATH`.

## CLI

Usage without installing via [uvx](https://docs.astral.sh/uv/guides/tools/):

```bash
uvx pydantic-monty --help
```

`uvx pydantic-monty` runs a REPL, or `uvx pydantic-monty <file>` runs a file.

Or to install `monty` locally, run

```bash
uv tool install pydantic-monty-runtime
# then to run the repl:
monty
# or run a file:
monty <file>
# or for help:
monty --help
```

Within an environment that already has `pydantic-monty` installed,
`python -m pydantic_monty` runs the same binary.

## Usage

### Basic execution

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        print(session.feed_run('1 + 2'))
        #> 3
```

`Monty()` is a pool of workers; `pool.checkout()` dedicates one worker to a
REPL session. Session state persists across `feed_run` calls:

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        session.feed_run('x = 40')
        print(session.feed_run('x + 2'))
        #> 42
```

### Async

`AsyncMonty` is the asyncio counterpart: worker I/O runs off the event loop,
and external functions may be coroutines.

```python
import asyncio

from pydantic_monty import AsyncMonty


async def fetch(url: str) -> str:
    await asyncio.sleep(0.01)
    return f'contents of {url}'


async def main():
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            result = await session.feed_run(
                "await fetch('https://example.com')",
                external_lookup={'fetch': fetch},
            )
    print(result)
    #> contents of https://example.com


asyncio.run(main())
```

### Input variables and external lookup

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        result = session.feed_run(
            'double(x) + y',
            inputs={'x': 5, 'y': 1},
            external_lookup={'double': lambda x: x * 2},
        )
    print(result)
    #> 11
```

### Host objects and classes

Wrap a host object in `ClassInstance` to let the sandbox read chosen attributes and call chosen methods on it, or a
class in `ClassType` with `init=True` to let sandbox code construct it; every policy is an allow-list, and the sandbox returning the
object hands you the original back.

```python
from dataclasses import dataclass

from pydantic_monty import ClassInstance, ClassType, Monty


@dataclass
class Person:
    name: str
    age: int

    def greeting(self) -> str:
        return f'hi {self.name}'


person = Person(name='Samuel', age=4)
with Monty() as pool:
    with pool.checkout() as session:
        wrapper = ClassInstance(person, eager_attrs='all', allowed_methods={'greeting'})
        code = 'assert user.greeting() == "hi Samuel"\nuser'
        result = session.feed_run(code, inputs={'user': wrapper})
        print(result is person)
        #> True
        wrapper = ClassType(Person, init=True, instance_eager_attrs='all')
        print(session.feed_run('Person("Ada", 36).name', inputs={'Person': wrapper}))
        #> Ada
```

Method return values are not wrapped automatically: override `convert_value` to wrap derived objects with policies you
choose (each wrapper is kept by the session until it closes). Instances defined inside the sandbox arrive as read-only
`MontyClassProxy` stand-ins. See the [host objects docs](https://github.com/pydantic/monty/blob/main/docs/host-objects.md).

### Snapshots: pausing and resuming execution

`feed_start` is the suspendable counterpart of `feed_run`: instead of driving a
snippet to completion, it hands control back at each external call, OS call,
name lookup, or future resolution as a *snapshot*. You answer with
`snapshot.resume(...)`, which returns the next snapshot or a `MontyComplete`.

```python
from pydantic_monty import FunctionSnapshot, Monty, MontyComplete

with Monty() as pool:
    with pool.checkout() as session:
        snapshot = session.feed_start('greet(name) + "!"', inputs={'name': 'Ada'})
        assert isinstance(snapshot, FunctionSnapshot)
        print(snapshot.function_name, snapshot.args)
        #> greet ('Ada',)
        result = snapshot.resume({'return_value': 'hello Ada'})
        assert isinstance(result, MontyComplete)
        print(result.output)
        #> hello Ada!
```

To iterate a snippet to completion without answering each suspension by hand,
pass an `external_lookup` (and/or `os`) to `feed_start` and drive with
`snapshot.resume_auto()`, which resolves each external call and name lookup from
them automatically — the same resolution `feed_run` performs, but one step at a
time so you can inspect or `dump()` each snapshot along the way:

```python
from pydantic_monty import Monty, MontyComplete

with Monty() as pool:
    with pool.checkout() as session:
        snapshot = session.feed_start(
            'greet(name) + "!"',
            inputs={'name': 'Ada'},
            external_lookup={'greet': lambda n: f'hello {n}'},
        )
        while not isinstance(snapshot, MontyComplete):
            snapshot = snapshot.resume_auto()
        print(snapshot.output)
        #> hello Ada!
```

On `AsyncMonty`, `external_lookup` callables may be coroutine functions and
`resume_auto` is awaitable (`snapshot = await snapshot.resume_auto()`); a
coroutine external is awaited directly when the snapshot's `allow_eager_await`
is true, and otherwise concurrently, settled via an `AsyncFutureSnapshot`.

`snapshot.dump()` serializes the paused worker to bytes; a fresh session's
`load_snapshot` restores it and returns the snapshot to resume. This lets you
checkpoint execution and continue it later, even in a different process:

```python
from pydantic_monty import FunctionSnapshot, Monty, MontyComplete

with Monty() as pool:
    with pool.checkout() as session:
        snapshot = session.feed_start(
            'fetch(url)', inputs={'url': 'https://example.com'}
        )
        blob = snapshot.dump()

    # later — restore into a fresh session and resume
    with pool.checkout() as session:
        snapshot = session.load_snapshot(blob)
        assert isinstance(snapshot, FunctionSnapshot)
        result = snapshot.resume({'return_value': 'page contents'})
        assert isinstance(result, MontyComplete)
        print(result.output)
        #> page contents
```

If the paused feed used filesystem `mount`s, re-supply the same ones to
`load_snapshot(blob, mount=...)` — their host paths are not stored in the dump.

`session.dump()` between feeds serializes an idle session instead; restore it
with `session.load_session(blob)` (which returns `None`) and keep feeding. Both
`load_session` and `load_snapshot` are valid only on a fresh session, before
any feed; using the wrong one for a dump's kind raises. `AsyncMonty` sessions
expose the same `feed_start` / `load_session` / `load_snapshot`, with awaitable
`resume(...)`.

### Resource limits

Limits are enforced inside the worker; the pool's `request_timeout` is a
host-side backstop that kills a hung worker outright. Installed telemetry
invokes trusted Python SDK callbacks synchronously; enforcement is
delayed while such a callback runs. `max_feed_duration_secs` and
`max_turn_duration_secs` limit *execution* time — the clock runs only while
the interpreter executes, never while suspended waiting on the host — over one
`feed_run` and one host round trip, restarting at each feed and at each host
answer. Neither accumulates over a session's lifetime; bounding that is the
host's job. The worker reports its consumed time on every protocol turn, and
each budget is additionally backstopped by killing the worker a grace period
after it expires, covering hangs the in-sandbox limit cannot catch (its check
only runs at interpreter checkpoints). The graces are the pool's
`feed_duration_limit_grace` and `turn_duration_limit_grace` (1s each;
`None` disables that backstop). `max_suspensions`
limits the host round trips the pool services per checkout; exceeding it ends
the feed with an uncatchable `RuntimeError`.

```python
from pydantic_monty import Monty, MontyRuntimeError

with Monty(request_timeout=10) as pool:
    with pool.checkout(limits={'max_feed_duration_secs': 0.1}) as session:
        try:
            session.feed_run('while True:\n    pass')
        except MontyRuntimeError as exc:
            print(exc.display(format='type-msg').split(':')[0])
            #> TimeoutError
```

### Clock, sleeping and entropy

By default the sandbox answers these itself, with no `os=` handler involved:
`date.today()`, `datetime.now()` and `time.time()` read the worker's clock;
`time.sleep()` and `asyncio.sleep()` wait inside the worker, each call cut to
`sandbox_sleep_clamp` (10 seconds), with gathered `asyncio.sleep()` calls
overlapping; and an unseeded `random` seeds itself from the worker's OS
entropy. A sandbox wait costs nothing against `max_duration_secs` and is not a
suspension, so `request_timeout` is what bounds a sleeping loop. Four
`checkout()` arguments change that, for the life of the session:

```python
from datetime import datetime

from pydantic_monty import Monty

code = """
import random, time
from datetime import datetime
time.sleep(3600)
f'{datetime.now():%Y-%m-%d %H:%M} {random.random():.4f}'
"""

# datetime: 'system' (default), 'call_host' or a datetime
# sleep: 'sandbox_sleep' (default), 'zero' or 'call_host'
# sandbox_sleep_clamp: seconds per sandbox sleep; float('inf') for no cap
# random_start: 'random' (default) or {'seed': int | float | str | bytes}
with Monty() as pool:
    with pool.checkout(
        datetime=datetime(2026, 1, 1, 9, 30),
        sleep='zero',
        sandbox_sleep_clamp=0.5,
        random_start={'seed': 42},
    ) as session:
        print(session.feed_run(code))
        #> 2026-01-01 09:30 0.6394
```

A naive `datetime` freezes the clock at that wall time in UTC (an aware one
sets the sandbox's local zone to its `utcoffset()`). `'zero'` makes both
sleeps return at once. `{'seed': s}` starts the module-level `random`
generator exactly as `random.seed(s)` would (unseeded `random.Random()`
instances take deterministic states derived from it); `random.seed()` in the
sandbox still applies afterwards. `'call_host'` on `datetime` or `sleep` sends
those calls to the `os=` handler instead — `OSAccess` answers them from the
host process, capping each wait at its `max_sleep` — and explicit
`os.urandom()` calls always reach the handler.

### Type checking

Monty bundles [ty](https://docs.astral.sh/ty/): each fed snippet can be
type-checked inside the worker before it runs, with successfully executed
snippets accumulating into the checking context.

```python
from pydantic_monty import Monty, MontyTypingError

with Monty() as pool:
    with pool.checkout(type_check=True) as session:
        try:
            session.feed_run("x: int = 'not an int'")
        except MontyTypingError as exc:
            print('invalid-assignment' in exc.display())
            #> True
```

`type_check_format` picks the rendering — ty's `'full'` (the default: source
snippet and carets), `'concise'`, `'azure'`, `'json'`, `'jsonlines'`,
`'rdjson'`, `'pylint'`, `'gitlab'` or `'github'` — and `type_check_color` adds
ANSI colour to `'full'` and `'concise'`. Both are `checkout()` arguments rather
than `display()` arguments because the diagnostics are rendered inside the
worker: ty's structured diagnostics resolve their spans against the type
checker's database, so only the rendered text crosses the wire.

```python
from pydantic_monty import Monty, MontyTypingError

with Monty() as pool:
    with pool.checkout(type_check=True, type_check_format='concise') as session:
        try:
            session.feed_run("x: int = 'not an int'")
        except MontyTypingError as exc:
            print(exc.display())
            """
            main.py:1:10: error[invalid-assignment] Object of type `Literal["not an int"]` is not assignable to `int`
            """
```

### Crash/failure isolation

Every failure in monty code execution raises a subclass of `MontyError`.

```python test="skip"
from pydantic_monty import Monty, MontyError

hostile_code = '...'

with Monty() as pool:
    with pool.checkout() as session:
        try:
            session.feed_run(hostile_code)  # even a segfault is contained
        except MontyError:
            ...  # the worker died; the pool already replaced it
```

### Observability

Install the optional OpenTelemetry API support, then call
`instrument_telemetry` with standard Python OpenTelemetry components before
creating a pool:

```bash
pip install 'pydantic-monty[opentelemetry]'
```

```python test="skip"
from opentelemetry import _logs, metrics, trace

from pydantic_monty import instrument_telemetry

instrument_telemetry(
    tracer=trace.get_tracer('pydantic-monty'),
    meter=metrics.get_meter('pydantic-monty'),
    logger=_logs.get_logger('pydantic-monty'),
)
```

Each component is optional. A configured tracer records each checkout as a
session span with nested feed and suspension spans. A logger records exceptions
and `print` output under those spans. An `AsyncMontyWebsocket` checkout also
sends the active context as W3C `traceparent`/`tracestate` headers on its
upgrade request, so a server that honours them can join the same trace. A meter
records live, immediately available and host-blocked worker counts, checkout
waits, worker deaths by reason, run durations and the sandbox execution time of
each feed.

The supplied OpenTelemetry providers own IDs, sampling, metric views and
aggregation, resources, readers, exporters, flushing, and shutdown. Logfire and
other OpenTelemetry distributions can therefore use the same instrumentation
path. [`logfire.instrument_monty()`](https://logfire.pydantic.dev/docs/reference/api/logfire/#logfire.Logfire.instrument_monty)
supplies components bound to its configured `Logfire` instance.

Metrics cover every checkout and record no sandbox-supplied values: their
attributes are closed sets, so nothing a script chooses (a called function's
name, an exception class, or a path) can become a dimension. Traces and logs do
record code, inputs, external calls, exceptions, and printed output; session
dumps and restores are recorded by size only. Instrumentation is disabled until
`instrument_telemetry` is called, and enabled instrumentation truncates large
values at the telemetry attribute size limit.

See the documentation for [filesystem mounts](https://github.com/pydantic/monty/blob/main/docs/filesystem.md),
[print buffering](https://github.com/pydantic/monty/blob/main/docs/limitations/print.md), and
[session snapshots](https://github.com/pydantic/monty/blob/main/docs/snapshots.md).
