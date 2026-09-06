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

## Working directory

Pass `cwd='/data'` to `session.feed_run()` or `session.feed_start()` to set the sandbox's virtual working directory.
The async session methods accept the same option.
The path must be absolute and uses POSIX `/` separators on every host.
On the first feed, omitting `cwd` selects the first mount's virtual path, or `/` if no mount is supplied.
The directory then persists across feeds, including successful `os.chdir(path=...)` calls, until another feed sets `cwd`.
`os.getcwd()` and `Path.cwd()` report it, and relative `open()`, `os`, and `pathlib` requests resolve against it.
Setting `cwd` does not grant filesystem access; provide `mount=` or `os=` to handle filesystem operations.

See the [`pydantic-monty`](https://pypi.org/project/pydantic-monty/) README for
more details.
