# The same coroutine can back only one task. Both task handles own the
# coroutine reference, so dropping them must release it exactly twice.
import asyncio


async def child():
    return 1


async def main():
    coro = child()
    first = asyncio.create_task(coro)
    second = asyncio.create_task(coro)
    assert await first == 1
    try:
        await second
        raise AssertionError('expected RuntimeError')
    except RuntimeError:
        pass
    first = None
    second = None
    coro = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
