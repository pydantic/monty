# Getting Started with Python

## Installation

```bash
uv add pydantic-monty
```

Or with pip:

```bash
pip install pydantic-monty
```

Requires Python 3.10 or newer.
The download is about 4.5 MB and there is nothing else to run: no daemon, no image, no API key.

`pydantic-monty` is a metapackage with no code of its own; it pins two distributions:

- [`pydantic-monty-client`](https://pypi.org/project/pydantic-monty-client/), the `pydantic_monty` module you import.
- [`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/), the `monty` binary that the worker
    subprocesses run, shipped the same way `uv` and `ruff` ship theirs.

Installing the wheel places the binary in the environment's scripts directory, so there is no extra setup step.
Install `pydantic-monty-client` alone when the binary comes from somewhere else, such as a base image or a system
package.

`Monty(binary_path=...)` overrides binary resolution.
When it is omitted, the binary is resolved from the `MONTY_BIN` environment variable, then the environment's scripts
directory (where `pydantic-monty-runtime` installs it), then `PATH`.
If you are running untrusted code, pin `binary_path` explicitly rather than relying on `PATH`.

The same binary is also a REPL and a file runner; see [command line](../cli.md).

## First run

Everything in `pydantic_monty` starts with a pool of worker subprocesses.
Execution never happens in your process: a Monty process can never be made fully crash-proof against memory errors
triggered by adversarial code, so the interpreter always runs in a worker that can crash without taking your process
down.

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

`Monty()` configures the pool; the workers are spawned by `with`.
`pool.checkout()` dedicates one worker to one REPL session.
`feed_run` executes a snippet and returns the value of its trailing expression.
`inputs` are values the snippet can read; `external_lookup` holds the host functions it can call.

## Sessions keep state

Session state — globals, functions, classes — persists across `feed_run` calls on the same checkout:

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        session.feed_run('x = 40')
        print(session.feed_run('x + 2'))
        #> 42
```

When the `with` block on the session exits, the worker goes back to the pool and the session state is gone.

## Getting values in

There are two ways to give the sandbox values from the host.

`inputs` binds values as globals eagerly, before the snippet runs.
Every entry is converted and bound once, whether or not the code uses it.

`external_lookup` resolves names lazily, when the code reads them.
A callable entry becomes a [host function](../host-functions.md) the sandbox can call; any other value is converted and
returned when the name is read; a name that is absent raises `NameError` inside the sandbox.
In the first example, `x` and `y` were bound before the snippet ran, and `double` was resolved when the snippet called
it.

A name present in both is served by the eager `inputs` binding.

### Which values cross the boundary

`None`, `bool`, `int` (arbitrary precision), `float`, `str`, `bytes`, `list`, `tuple`, `dict`, `set`, `frozenset`,
`Ellipsis`, `NotImplemented`, `datetime.date`, `datetime.datetime`, `datetime.timedelta`, `datetime.timezone`, named
tuples, exception instances, and the type objects Monty models (`int`, `str`, `datetime.date`, ...) all convert in both
directions.
Class instances differ in each direction: a host instance enters only wrapped in `ClassInstance`, and a sandbox-defined
instance comes out as a read-only `MontyClassProxy`.
See [host objects](../host-objects.md).
Put callables in `external_lookup`, where they become [host functions](../host-functions.md); a callable in `inputs`
binds only a reference the sandbox still resolves through `external_lookup` when it is called.

POSIX paths convert too — `pathlib.PurePosixPath` and `pathlib.PosixPath`, which is what `Path()` builds on Linux and
macOS.
They come back as `PurePosixPath`, and a `PureWindowsPath` / `WindowsPath` is rejected, because paths inside the sandbox
are always POSIX.

Anything else is rejected with `MontyConversionError` before it reaches the sandbox:

```python
from decimal import Decimal

from pydantic_monty import Monty, MontyConversionError

with Monty() as pool:
    with pool.checkout() as session:
        try:
            session.feed_run('v', inputs={'v': Decimal('1.5')})
        except MontyConversionError as exc:
            print(exc)
            """
            Cannot convert decimal.Decimal to Monty value — wrap class instances in pydantic_monty.ClassInstance(...)
            """
```

## What the sandbox cannot reach

With nothing mounted, the sandbox has no filesystem; `open()` raises `PermissionError` because no mount exists, not
because a check blocked it.
Resource limits are set per session on `checkout()`; operations whose size is predictable are refused before the
allocation is attempted:

```python
from pydantic_monty import Monty, MontyRuntimeError

code = """
try:
    open('/etc/passwd')
except PermissionError as e:
    denied = str(e)
denied
"""

with Monty() as pool:
    with pool.checkout(
        limits={'max_memory': 10_000_000, 'max_duration_secs': 1.0}
    ) as session:
        print(session.feed_run(code))
        #> Permission denied: '/etc/passwd'
        try:
            session.feed_run("'x' * 10**12")
        except MontyRuntimeError as exc:
            print(exc.display(format='type-msg').split(':')[0])
            #> MemoryError
```

An infinite loop hits `max_duration_secs` the same way, raising `TimeoutError`.
Type checking is also configured on `checkout()`:

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

See [resource limits](../resource-limits.md), [type checking](../type-checking.md) and the [security
model](../security.md).

## Async

`AsyncMonty` is the asyncio counterpart.
Worker I/O runs off the event loop, and host functions may be coroutines:

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

There is no event loop inside the sandbox — the host is the loop.
Sandboxed `async def` and `await` work, and `asyncio` exposes exactly `run` and `gather`, the latter running host calls
concurrently.
`asyncio.create_task`, `asyncio.sleep` and everything else in the module do not exist.
See [`limitations/asyncio.md`](../limitations/asyncio.md).

## Pausing at host calls

`feed_run` answers every host call for you.
`feed_start` hands control back at each one instead, as a snapshot you can inspect, store with `dump()`, or resume:

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

`snapshot.dump()` returns bytes that a fresh session's `load_snapshot()` turns back into the same paused snapshot, in
another process or on another machine.
See [snapshots](../snapshots.md).

## Capturing printed output

By default the sandbox's `print()` goes to your process's stdout and stderr.
Pass `print_callback` to intercept it:

```python
from pydantic_monty import CollectString, Monty

with Monty() as pool:
    with pool.checkout() as session:
        collector = CollectString()
        session.feed_run("print('from the sandbox')", print_callback=collector)
        print(repr(collector.output))
        #> 'from the sandbox\n'
```

`CollectStreams` collects `(stream, text)` tuples instead, so you can tell stdout from stderr.
Both cap collected output at 10 MiB by default; pass `max_bytes=None` to disable the cap.
That cap is separate from [`max_memory`](../resource-limits.md), and it is enforced in your process as the output
arrives, not by the worker.

Exceeding it fails the feed with `MontyRuntimeError` wrapping a `MemoryError`; call `exc.exception()` for the
`MemoryError` itself.
Sandboxed code cannot catch it, so a `print()` loop cannot swallow the cap.

A plain callable works too, receiving `(stream, text)`:

```python
from pydantic_monty import Monty

seen: list[tuple[str, str]] = []

with Monty() as pool:
    with pool.checkout() as session:
        session.feed_run(
            "print('hello')",
            print_callback=lambda stream, text: seen.append((stream, text)),
        )
        print(seen)
        #> [('stdout', 'hello\n')]
```

Output arrives in chunks, not one call per `print()`.
The worker batches it, sending a chunk once about 8 KiB accumulates or the oldest byte has waited out
`print_flush_interval` — 0.005 seconds by default, settable on `checkout()`, and `0` to restore one chunk per line.
Whatever the interval, output is flushed before a host call and before a feed ends, so it never arrives out of order.

## Errors

Every Monty error subclasses `MontyError`:

| Exception              | Raised when                               | Session survives                             |
| ---------------------- | ----------------------------------------- | -------------------------------------------- |
| `MontySyntaxError`     | The snippet does not parse                | yes                                          |
| `MontyTypingError`     | Type checking rejected the snippet        | yes                                          |
| `MontyRuntimeError`    | The code raised at runtime                | yes — but discard it after a resource limit  |
| `MontyConversionError` | A host value cannot cross the boundary    | from `inputs` yes, from `external_lookup` no |
| `MontyCrashedError`    | The worker died, or hit `request_timeout` | no                                           |

`inputs` are converted before the snippet runs, so a rejected value leaves the session untouched.
An `external_lookup` value is converted mid-execution, while the worker is suspended on the name read, so the checkout
is discarded and reusing it raises `RuntimeError: this checkout has already been finished`.
Check out again to retry.

A `MontyRuntimeError` carrying `TimeoutError`, or a `MemoryError` from the sandbox heap, is a [resource
limit](../resource-limits.md#after-a-limit-fires) rather than ordinary sandbox code raising.
The pool leaves the checkout open, but the heap behind it is no longer trustworthy, so discard it rather than feeding it
again.
A spent `max_duration_secs` budget is cumulative, so later feeds re-raise `TimeoutError` anyway; after a `max_memory`
trip they may quietly succeed.
`max_suspensions` limits host calls and raises a pool-generated `RuntimeError` such as `suspension limit 1000 exceeded`.
The feed ends cleanly; later code runs until it suspends again.

The print-collector cap is not one of these, though it looks identical from the outside: same `MontyRuntimeError`, same
`MemoryError`, same `memory limit exceeded: ...` message.
If you collect printed output at all — and the collectors are capped by default — you cannot tell the two apart from the
exception alone, and in the collector case nothing is wrong with the session.
See [`limitations/print.md`](../limitations/print.md).

`MontySyntaxError` and `MontyRuntimeError` carry a Monty traceback:

```python
from pydantic_monty import Monty, MontyRuntimeError

code = """
def f():
    raise ValueError('boom')

f()
"""

with Monty() as pool:
    with pool.checkout() as session:
        try:
            session.feed_run(code)
        except MontyRuntimeError as exc:
            print(exc.display(format='type-msg'))
            #> ValueError: boom
            print([frame.function_name for frame in exc.traceback()])
            #> ['<module>', 'f']
```

`display()` also takes `'traceback'` (the default, a full CPython-style traceback) and `'msg'`.
`exc.exception()` returns the inner exception as a native Python exception object.

`MontyCrashedError` is the one that loses the session.
The pool has already replaced the worker by the time you catch it, so retrying on a fresh checkout is safe:

```python test="skip"
from pydantic_monty import Monty, MontyCrashedError

hostile_code = '...'

with Monty() as pool:
    with pool.checkout() as session:
        try:
            session.feed_run(hostile_code)  # even a segfault is contained
        except MontyCrashedError:
            ...  # the worker died; the pool already replaced it
```

## Configuring the pool

```python test="skip"
from pydantic_monty import Monty

pool = Monty(
    binary_path=None,  # explicit path to the `monty` worker binary
    min_processes=1,  # workers spawned eagerly and kept warm
    max_processes=None,  # cap on live workers; defaults to the CPU count
    checkout_timeout=None,  # seconds `checkout()` waits for a free worker
    request_timeout=None,  # hard per-turn deadline; kills the worker
    max_checkouts_per_worker=None,  # recycle a worker after N sessions
)
```

`request_timeout` is a per-turn host-side backstop: a worker that exceeds it is killed and the call raises
`MontyCrashedError` with `timed_out=True`.
It catches hangs the in-sandbox limits cannot see, because those are only checked at interpreter checkpoints.
A loop of quick host calls resets it each turn; set [`max_duration_secs`](../resource-limits.md) as well.

`AsyncMonty` takes the same arguments.

## Where next

- [`pydantic_monty` API reference](../api/python/pools.md) — every class, method and option.
- [Host functions](../host-functions.md) — the only way code in the sandbox reaches anything outside it.
- [Host objects](../host-objects.md) — exposing objects and classes with per-attribute and per-method policies.
- [Filesystem access](../filesystem.md) — mounts and the `os` callback.
- [Snapshots](../snapshots.md) — `feed_start`, `dump()` and resuming later.
- [The Python subset](../limitations/index.md) — what the sandbox can actually run.

Worked examples using Pydantic AI live in [`examples/`](https://github.com/pydantic/monty/tree/main/examples).
