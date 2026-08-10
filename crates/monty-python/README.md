# pydantic-monty-client

Python client for the Monty sandboxed Python interpreter.

Most users want [`pydantic-monty`](https://pypi.org/project/pydantic-monty/)
instead, which pulls in this package plus
[`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/) and
is documented in full on its PyPI page.

Install this package directly to use the websocket client alone,
or if you're installing the  `monty` binary another way.

```bash
uv add pydantic-monty-client
# or
pip install pydantic-monty-client
```

# Usage with a remote monty server and websockets

You can use this library alone to connect to a remote monty server via websockets.

```python
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

This requires the `pydantic-monty-runtime` package, which is generally
installed as part of the `pydantic-monty` meta-package.

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        print(session.feed_run('1 + 2'))
        #> 3
```

or in async code:

```python
from pydantic_monty import AsyncMonty


async def main() -> None:
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            output = await session.feed_run('1 + 1')
            print('output from local worker ->', output)


if __name__ == '__main__':
    import asyncio
    asyncio.run(main())
```

See the [`pydantic-monty`](https://pypi.org/project/pydantic-monty/) README for
more details.
