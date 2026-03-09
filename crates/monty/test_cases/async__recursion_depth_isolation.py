# call-external
# run-async
# Test that recursion depth is per-task, not global.
#
# With a recursion limit of 50, a gathered task that recurses 40 deep
# should NOT eat into another task's budget. Without per-task depth
# tracking, the second task inherits the first task's depth and hits
# the limit prematurely.
import asyncio


async def recurse_then_call(n):
    """Recurse n levels deep, then make an external call at the bottom."""
    if n == 0:
        return await async_call('done')
    return await recurse_then_call(n - 1)


# Each task recurses 40 deep independently.
# With a global depth counter, the second task would start at depth 40
# and blow the limit at depth 80 (well above the 50 limit).
# With correct per-task tracking, each task sees its own depth of 40.
results = await asyncio.gather(  # pyright: ignore
    recurse_then_call(40),
    recurse_then_call(40),
)
assert results == ['done', 'done'], f'both tasks should complete: {results}'
