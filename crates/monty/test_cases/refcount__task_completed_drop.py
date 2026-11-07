# Dropping a completed task's handle must release the task, its cached result
# and the coroutine it owns.
import asyncio


async def completes():
    return [1, 2, 3]


async def main():
    completed = asyncio.create_task(completes())
    results = await asyncio.gather(completed)
    assert results == [[1, 2, 3]]
    completed = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
