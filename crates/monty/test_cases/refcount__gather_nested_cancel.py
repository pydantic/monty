# Test that a nested GatherFuture is properly cleaned up when the outer gather fails.
# The sibling task and its inner gather are detached rather than cancelled, and are
# released when the run ends without the main task ever waiting on them again.
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
