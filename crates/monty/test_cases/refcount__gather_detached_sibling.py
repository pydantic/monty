# Test that a sibling left running by a failing gather balances its references
# once it finishes. The sibling outlives the gather that spawned it, so nothing
# but its own completion path is left to release its coroutine and results.
import asyncio

completed = 0


async def leaf():
    return [1, 2, 3]


async def survivor():
    global completed
    for _ in range(2):
        await asyncio.gather(leaf(), leaf())
    completed += 1


async def task_fail():
    raise ValueError('outer task failed')


try:
    await asyncio.gather(survivor(), task_fail())  # pyright: ignore
except ValueError:
    pass

# The detached sibling only advances while something else suspends, so drive it
# with top-level awaits until it reports completion instead of a fixed turn count.
for _ in range(20):
    if completed:
        break
    await asyncio.gather(leaf())  # pyright: ignore

# The loop above is bounded, so assert the sibling really ran: a scheduler change
# that stops driving it should fail here, not as an unexplained ref-count leak.
assert completed == 1
# ref-counts={'asyncio': 1}
