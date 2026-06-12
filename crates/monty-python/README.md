# pydantic-monty

Python bindings for the Monty sandboxed Python interpreter.

Execution always happens in a pool of `monty` worker subprocesses: a monty
process can never be made fully crash-proof against memory errors (stack
overflows, allocator aborts) triggered by adversarial input, so crash
isolation is built in. A crashed worker raises `MontyCrashedError` and is
replaced transparently — your process is never at risk.

## Installation

```bash
pip install pydantic-monty
```

This installs the `monty` worker binary via the
[`pydantic-monty-cli`](https://pypi.org/project/pydantic-monty-cli/)
dependency (the same way `uv` and `ruff` ship their binaries).

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
                external_functions={'fetch': fetch},
            )
    print(result)
    #> contents of https://example.com


asyncio.run(main())
```

### Input variables and external functions

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        result = session.feed_run(
            'double(x) + y',
            inputs={'x': 5, 'y': 1},
            external_functions={'double': lambda x: x * 2},
        )
    print(result)
    #> 11
```

### Resource limits

Limits are enforced inside the worker; the pool's `request_timeout` is a
host-side backstop that kills a hung worker outright. `max_duration_secs`
limits cumulative *execution* time — the clock runs only while the
interpreter executes, never while suspended waiting on the host, and
accumulates across feeds. The worker reports its execution time on every
protocol turn, and sessions with the limit are additionally killed
`duration_limit_grace` (1s, not currently configurable from Python) after
the remaining budget expires, covering hangs the in-sandbox limit cannot
catch (e.g. a blocking syscall inside a mount).

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

### Crash isolation

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

See `limitations/pool-architecture.md` in the repository for the behavioural
details of subprocess execution (worker-local mounts, line-buffered print
callbacks, session dumps).
