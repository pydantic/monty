# Test that a nested GatherFuture is released when the outer gather fails and the
# detached sibling never runs again. Nothing balances these refs mid-run — the
# end-of-run scheduler teardown does — so this covers the sibling that never
# finishes; refcount__gather_detached_sibling.py covers the one that does.
import asyncio


async def inner_task():
    return 1


async def task_with_inner_gather():
    # This inner gather should be cancelled when the outer gather fails
    result = await asyncio.gather(inner_task(), inner_task())
    return result


async def task_fail():
    raise ValueError('outer task failed')


try:
    result = await asyncio.gather(task_with_inner_gather(), task_fail())  # pyright: ignore
except ValueError:
    pass
# ref-counts={'asyncio': 1}
