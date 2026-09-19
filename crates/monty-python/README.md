# pydantic-monty-client

Python client for the Monty sandboxed Python interpreter.

Most users want [`pydantic-monty`](https://pypi.org/project/pydantic-monty/)
instead, which pulls in this package plus
[`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/) and
is documented in full on its PyPI page.

Install this package directly to use the websocket client alone,
or if you're installing the `monty` binary another way.

```bash
uv add pydantic-monty-client
# or
pip install pydantic-monty-client
```

## Usage with a remote monty server and websockets

You can use this library alone to connect to a remote monty server via websockets.

```python test="skip"
from pydantic_monty import AsyncMontyWebsocket


async def main() -> None:
    url = '...'
    async with AsyncMontyWebsocket(url) as pool:
        async with pool.checkout() as session:
            output = await session.feed_run('1 + 1')
            print('output ->', output)


if __name__ == '__main__':
    import asyncio

    asyncio.run(main())
```

## Usage with a local monty worker

Host objects and classes cross the boundary through the `ClassInstance` / `ClassType` wrappers; see the
`pydantic-monty` README.

This requires the `pydantic-monty-runtime` package, which is generally
installed as part of the `pydantic-monty` meta-package.

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout(limits={'max_suspensions': 100}) as session:
        print(session.feed_run('1 + 2'))
        #> 3
```

`max_suspensions` limits host-serviced suspensions per checkout (default
1000; it cannot be disabled). Exceeding it
aborts the feed with an uncatchable `RuntimeError`; the session remains usable,
but its suspension count remains spent.

or in async code:

```python
from pydantic_monty import AsyncMonty


async def main() -> None:
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            output = await session.feed_run('1 + 1')
            print('output from local worker ->', output)
            #> output from local worker -> 2


if __name__ == '__main__':
    import asyncio

    asyncio.run(main())
```

## Tracing snapshot handlers

All sync and async snapshot types provide `snapshot.trace_context()` for manual handlers.
It returns a standard OpenTelemetry `Context`, not a context manager, and requires `opentelemetry-api` to be installed.
Use OTel's `attach` / `detach` to activate it, including across `await` in the same task:

```python
from opentelemetry import context

from pydantic_monty import FunctionSnapshot, Monty, MontyComplete

with Monty() as pool:
    with pool.checkout() as session:
        snapshot = session.feed_start('greet(name)', inputs={'name': 'Ada'})
        assert isinstance(snapshot, FunctionSnapshot)
        token = context.attach(snapshot.trace_context())
        try:
            greeting = f'hello {snapshot.args[0]}'
        finally:
            context.detach(token)
        result = snapshot.resume({'return_value': greeting})
        assert isinstance(result, MontyComplete)
        print(result.output)
        #> hello Ada
```

The returned context preserves baggage and other entries captured at `feed_start` / `load_snapshot`, with the
suspension's span when Monty tracing is enabled.
Without Monty tracing it returns the captured context unchanged.
Context is not serialized: restoring captures the restoring caller's context instead.
The method does not activate the context or resume execution.
It raises `ImportError` without `opentelemetry-api`, or `RuntimeError` after resume.
Previously returned contexts remain usable but do not keep the suspension span open.
`resume_auto()` already activates the suspension span around callbacks.
See the [snapshot documentation](https://pydantic.dev/docs/monty/concepts/snapshots/).

## Restoring snapshots

`session.load_session()` and `session.load_snapshot()` require unmodified snapshots from a trusted, compatible Monty producer.
The caller must establish provenance and integrity before loading; Monty does not authenticate snapshots.
Invalid snapshots have no correctness or availability guarantees.
Successful loading does not establish validity.
See the [snapshot security documentation](https://pydantic.dev/docs/monty/concepts/security/#deserializing-snapshots).

## Working directory

Pass `cwd='/data'` to `session.feed_run()` or `session.feed_start()` to set the sandbox's virtual working directory.
The async session methods accept the same option.
The path must be absolute and uses POSIX `/` separators on every host.
On the first feed, omitting `cwd` selects the first mount's virtual path, or `/` if no mount is supplied.
The directory then persists across feeds, including successful `os.chdir(path=...)` calls, until another feed sets `cwd`.
`os.getcwd()` and `Path.cwd()` report it, and relative `open()`, `os`, and `pathlib` requests resolve against it.
Setting `cwd` does not grant filesystem access; provide `mount=` or `os=` to handle filesystem operations.

`OSAccess(max_urandom_bytes=...)` sets the largest `os.urandom()` request the default handler serves, 1 MiB by default.
Larger requests raise `MemoryError` before allocating.
Unseeded `random` generators request host entropy only under `auto_os_calls={'random_start': 'call_host'}`.
Otherwise they use worker OS entropy or the configured seed.

By default, `date.today()`, `datetime.now()` and `time.time()` read the worker's clock.
The pool handles `time.sleep()` and `asyncio.sleep()`, capped per call by `sleep_system_max`.
Setting `datetime` or `sleep` to `'call_host'` in `checkout(auto_os_calls=...)` routes those calls to `os=`.
`OSAccess` answers from the host process and caps each wait at `max_sleep`.

A `random.Random` instance or the `random.Random` class returned from the sandbox converts to its repr string.
Return the generated values or `rng.getstate()` instead.

See the [`pydantic-monty`](https://pypi.org/project/pydantic-monty/) README for
more details.
