# Pending tasks owned only by the scheduler must be released by end-of-run task
# teardown.
import asyncio


async def main():
    async def never_started():
        return 0

    unstarted = asyncio.create_task(never_started())
    unstarted = None

    holder = []

    async def first():
        return await holder[0]

    async def second():
        return await holder[1]

    first_task = asyncio.create_task(first())
    second_task = asyncio.create_task(second())
    holder.extend([second_task, first_task])
    holder.clear()
    first_task = None
    second_task = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
