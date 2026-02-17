# Test that GatherFuture and coroutines are properly cleaned up after gather completes.
# The strict matching check will fail if the GatherFuture leaks (heap_count > unique_refs).
import asyncio


async def task1():
    return 1


async def task2():
    return 2


result = await asyncio.gather(task1(), task2())  # pyright: ignore
result
# ref-counts={'result': 2, 'asyncio': 1}
