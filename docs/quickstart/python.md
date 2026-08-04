# Python QuickStart

```bash
uv add pydantic-monty
```

Everything in `pydantic_monty` starts with a pool of worker subprocesses. Execution never
happens in your process: a Monty process can never be made fully crash-proof against
memory errors triggered by adversarial code, so the sandbox always runs somewhere it can
safely die.

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        print(session.feed_run('1 + 2'))
        #> 3
```

`Monty()` configures the pool; the workers are spawned by `with`. `pool.checkout()`
dedicates one worker to one REPL session. `feed_run` executes a snippet and returns the
value of its trailing expression.

## Sessions keep state

Session state — globals, functions, classes — persists across `feed_run` calls on the
same checkout:

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        session.feed_run('x = 40')
        print(session.feed_run('x + 2'))
        #> 42
```

When the `with` block on the session exits, the worker goes back to the pool and the
session state is gone.

## Getting values in

There are two ways to give the sandbox values from the host.

`inputs` binds values as globals eagerly, before the snippet runs. Every entry is
converted and bound once, whether or not the code uses it.

`external_lookup` resolves names lazily, when the code reads them. A callable entry
becomes a [host function](../host-functions.md) the sandbox can call; any other value is
converted and returned when the name is read; a name that is absent raises `NameError`
inside the sandbox.

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

A name present in both is served by the eager `inputs` binding.

### Which values cross the boundary

`None`, `bool`, `int` (arbitrary precision), `float`, `str`, `bytes`, `list`, `tuple`,
`dict`, `set`, `frozenset`, `Ellipsis`, `NotImplemented`, `datetime.date`,
`datetime.datetime`, `datetime.timedelta`, `datetime.timezone`, named tuples, dataclass
instances, exception instances, and the type objects Monty models (`int`, `str`,
`datetime.date`, ...) all convert in both directions. A callable is not a value: put it in
`external_lookup`, where it becomes a [host function](../host-functions.md).

POSIX paths convert too — `pathlib.PurePosixPath` and `pathlib.PosixPath`, which is what
`Path()` builds on Linux and macOS. They come back as `PurePosixPath`, and a
`PureWindowsPath` / `WindowsPath` is rejected, because paths inside the sandbox are always
POSIX.

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
            #> Cannot convert decimal.Decimal to Monty value
```

## Async

`AsyncMonty` is the asyncio counterpart. Worker I/O runs off the event loop, and host
functions may be coroutines:

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

There is no event loop inside the sandbox — the host is the loop. Sandboxed `async def`
and `await` work, and `asyncio` exposes exactly `run` and `gather`, the latter running
host calls concurrently. `asyncio.create_task`, `asyncio.sleep` and everything else in the
module do not exist. See
[`limitations/asyncio.md`](https://github.com/pydantic/monty/blob/main/limitations/asyncio.md).

## Capturing printed output

By default the sandbox's `print()` goes to your process's stdout and stderr. Pass
`print_callback` to intercept it:

```python
from pydantic_monty import CollectString, Monty

with Monty() as pool:
    with pool.checkout() as session:
        collector = CollectString()
        session.feed_run("print('from the sandbox')", print_callback=collector)
        print(repr(collector.output))
        #> 'from the sandbox\n'
```

`CollectStreams` collects `(stream, text)` tuples instead, so you can tell stdout from
stderr. Both cap collected output at 10 MiB by default and raise `MemoryError` past it;
pass `max_bytes=None` to disable the cap. That cap is separate from
[`max_memory`](../resource-limits.md).

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

Output arrives in chunks flushed at newline boundaries or once roughly 8 KiB
accumulates, not one call per `print()`.

## Errors

Every Monty error subclasses `MontyError`:

| Exception | Raised when | Session survives |
| --- | --- | --- |
| `MontySyntaxError` | The snippet does not parse | yes |
| `MontyTypingError` | Type checking rejected the snippet | yes |
| `MontyRuntimeError` | The code raised at runtime | yes |
| `MontyConversionError` | A host value cannot cross the boundary | from `inputs` yes, from `external_lookup` no |
| `MontyCrashedError` | The worker died, or hit `request_timeout` | no |

`inputs` are converted before the snippet runs, so a rejected value leaves the session
untouched. An `external_lookup` value is converted mid-execution, while the worker is
suspended on the name read, so the checkout is discarded and reusing it raises
`RuntimeError: this checkout has already been finished`. Check out again to retry.

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

`display()` also takes `'traceback'` (the default, a full CPython-style traceback) and
`'msg'`. `exc.exception()` returns the inner exception as a native Python exception
object.

`MontyCrashedError` is the one that loses the session. The pool has already replaced the
worker by the time you catch it, so retrying on a fresh checkout is safe:

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

## Limits and type checking

Both are configured per session, on `checkout()`:

```python
from pydantic_monty import Monty, MontyRuntimeError

with Monty(request_timeout=10) as pool:
    with pool.checkout(limits={'max_duration_secs': 0.1}) as session:
        try:
            session.feed_run('while True:\n    pass')
        except MontyRuntimeError as exc:
            print(exc.display(format='type-msg').split(':')[0])
            #> TimeoutError
```

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

See [resource limits](../resource-limits.md) and [type checking](../type-checking.md).

## Configuring the pool

```python test="skip"
from pydantic_monty import Monty

pool = Monty(
    binary_path=None,  # explicit path to the `monty` worker binary
    min_processes=1,  # workers spawned eagerly and kept warm
    max_processes=None,  # cap on live workers; defaults to the CPU count
    checkout_timeout=None,  # seconds `checkout()` waits for a free worker
    request_timeout=None,  # hard per-call deadline; kills the worker
    max_checkouts_per_worker=None,  # recycle a worker after N sessions
)
```

`request_timeout` is the host-side backstop: a worker that exceeds it is killed and the
call raises `MontyCrashedError` with `timed_out=True`. It catches hangs the in-sandbox
limits cannot see, because those are only checked at interpreter checkpoints.

`AsyncMonty` takes the same arguments.

## Where next

- [Host functions](../host-functions.md) — the only way code in the sandbox reaches
  anything outside it.
- [Filesystem access](../filesystem.md) — mounts and the `os` callback.
- [Snapshots](../snapshots.md) — `feed_start`, `dump()` and resuming later.
- [The Python subset](../python-subset.md) — what the sandbox can actually run.

Worked examples using Pydantic AI live in
[`examples/`](https://github.com/pydantic/monty/tree/main/examples).
