# A wait future that has materialised but is still waiting for pending inputs
# must release its input list and WaitSlot links when task teardown cancels the
# waiter.
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


async def noop():
    return None


async def blocked_wait(tasks, started):
    started.append(True)
    await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)


async def main():
    pending_one, pending_two = await blocked_pair()
    started = []
    waiter = asyncio.create_task(blocked_wait([pending_one, pending_two], started))
    for _ in range(3):
        if started:
            break
        await asyncio.create_task(noop())
    assert started
    waiter = None
    pending_one = None
    pending_two = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
