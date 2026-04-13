# Test that GatherFuture and coroutines are properly cleaned up when a task raises.
# When one task fails, sibling tasks should be cancelled and all resources freed.
import asyncio


async def task_ok():
    return 1


async def task_fail():
    raise ValueError('task failed')


try:
    result = await asyncio.gather(task_ok(), task_fail())  # pyright: ignore
except ValueError:
    pass
# ref-counts={'asyncio': 1}
