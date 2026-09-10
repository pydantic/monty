# Test that unwinding a failed nested-gather commit releases every level. The
# innermost gather fails on an already-awaited coroutine, so each gather above it
# has to drop its result slots and its own awaiter as the commit stack unwinds.
import asyncio


async def leaf():
    return 1


async def slow():
    return 2


spent = leaf()
await spent  # pyright: ignore

inner = asyncio.gather(slow(), spent)
outer = asyncio.gather(asyncio.gather(inner))
try:
    await outer  # pyright: ignore
    assert False, 'expected the reused coroutine to raise'
except RuntimeError as exc:
    assert str(exc) == 'cannot reuse already awaited coroutine'
# ref-counts={'asyncio': 1, 'spent': 2, 'outer': 1, 'inner': 2}
