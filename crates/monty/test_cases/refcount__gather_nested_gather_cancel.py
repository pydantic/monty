# Test that a nested GatherFuture *item* is released once the run ends, having
# been detached when its parent gather failed. Unlike
# refcount__gather_nested_cancel, the inner gather is a direct item of the
# gather the detached task is blocked on, not reached via a coroutine.
import asyncio


async def leaf():
    return 1


async def task_with_gather_item():
    return await asyncio.gather(asyncio.gather(leaf()))


async def task_fail():
    raise ValueError('outer task failed')


try:
    result = await asyncio.gather(task_with_gather_item(), task_fail())  # pyright: ignore
except ValueError:
    pass
# ref-counts={'asyncio': 1}
