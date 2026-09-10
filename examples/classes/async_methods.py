"""Async methods on a ClassInstance with AsyncMonty.

A wrapped method may be an `async def`: sandbox code awaits it like any
external call, the coroutine runs on the host event loop, and the resolved
value crosses back.
"""

import asyncio

from pydantic_monty import AsyncMonty, ClassInstance


class Fetcher:
    async def fetch(self, url: str) -> str:
        await asyncio.sleep(0)  # real I/O goes here
        return f'contents of {url}'


async def main() -> None:
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            result = await session.feed_run(
                'await client.fetch("https://example.com")',
                inputs={'client': ClassInstance(Fetcher(), allowed_methods={'fetch'})},
            )
    assert result == 'contents of https://example.com'
    print(result)


if __name__ == '__main__':
    asyncio.run(main())
