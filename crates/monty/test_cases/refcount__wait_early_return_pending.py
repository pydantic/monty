# FIRST_COMPLETED returns while other inputs are still pending. Their WaitSlot
# links and the consumed wait future must be reclaimed by task teardown.
import asyncio


async def blocked_pair():
    holder = []

    async def first():
        return await holder[0]

    async def second():
        return await holder[1]

    first_task = asyncio.create_task(first())
    second_task = asyncio.create_task(second())
    holder.extend([second_task, first_task])
    return first_task, second_task


async def done_child():
    return 1


async def main():
    done_task = asyncio.create_task(done_child())
    await done_task

    pending_one, pending_two = await blocked_pair()
    done, pending = await asyncio.wait([done_task, pending_one, pending_two], return_when=asyncio.FIRST_COMPLETED)
    assert done == {done_task}
    assert pending == {pending_one, pending_two}


asyncio.run(main())
# ref-counts={'asyncio': 1}
