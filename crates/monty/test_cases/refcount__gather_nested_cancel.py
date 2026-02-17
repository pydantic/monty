# Test that nested GatherFuture is properly cleaned up when outer task is cancelled.
# When one task in an outer gather fails, sibling tasks (including those with inner gathers)
# should be cancelled and all GatherFutures properly cleaned up.
import asyncio


async def inner_task():
    return 1


async def task_with_inner_gather():
    # This inner gather should be cancelled when the outer gather fails
    result = await asyncio.gather(inner_task(), inner_task())
    return result


async def task_fail():
    raise ValueError('outer task failed')


try:
    result = await asyncio.gather(task_with_inner_gather(), task_fail())  # pyright: ignore
except ValueError:
    pass
# ref-counts={'asyncio': 1}
