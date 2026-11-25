# Several coroutines await one task through gather. Completing the task must
# release every awaiter record, not just the first one.
import asyncio


async def child():
    return [7]


async def waiter(task):
    return await task


async def main():
    task = asyncio.create_task(child())
    results = await asyncio.gather(waiter(task), waiter(task), task)
    assert results == [[7], [7], [7]]
    assert results[0] is results[1]
    assert results[1] is results[2]


asyncio.run(main())
# ref-counts={'asyncio': 1}
