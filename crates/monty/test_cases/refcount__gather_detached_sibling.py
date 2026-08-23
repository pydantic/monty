# Test that a sibling left running by a failing gather balances its references
# once it finishes. The sibling outlives the gather that spawned it, so nothing
# but its own completion path is left to release its coroutine and results.
import asyncio


async def leaf():
    return [1, 2, 3]


async def survivor():
    for _ in range(2):
        await asyncio.gather(leaf(), leaf())


async def task_fail():
    raise ValueError('outer task failed')


try:
    await asyncio.gather(survivor(), task_fail())  # pyright: ignore
except ValueError:
    pass

# Give the detached sibling the turns it needs to run to completion.
for _ in range(6):
    await asyncio.gather(leaf())  # pyright: ignore
# ref-counts={'asyncio': 1}
