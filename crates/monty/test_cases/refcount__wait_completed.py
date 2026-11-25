# A completed wait() must release duplicate inputs, its WaitFuture, its input
# list and the WaitSlot links it installed on each task. Using FIRST_EXCEPTION
# with no failures also exercises the ALL_COMPLETED fallback.
import asyncio


async def child():
    return 1


async def main():
    task = asyncio.create_task(child())
    waiting = asyncio.wait([task, task], return_when=asyncio.FIRST_EXCEPTION)
    done, pending = await waiting
    assert done == {task}
    assert pending == set()
    try:
        await waiting
        raise AssertionError('expected RuntimeError')
    except RuntimeError:
        pass
    waiting = None
    task = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
