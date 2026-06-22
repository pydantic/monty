# run-async
"""Gather failure must clean up committed siblings and nested failures."""

import asyncio


# === Commit-time orphan: Failed nested gather is the second item ===
# Outer spawns slow() first, then hits `g_failed` which raises synchronously.
# Rollback must cancel slow()'s task.
async def boom_orphan():
    raise ValueError('boom')


async def slow_orphan():
    return 'slow ok'


g_failed = asyncio.gather(boom_orphan())
try:
    await g_failed  # pyright: ignore
    assert False, 'g_failed should have raised'
except ValueError as e:
    assert str(e) == 'boom', f'first await: {e}'

try:
    await asyncio.gather(slow_orphan(), g_failed)  # pyright: ignore
    assert False, 'reuse of failed gather should raise'
except ValueError as e:
    assert str(e) == 'boom', f'second await: {e}'

# If `slow_orphan`'s task was orphaned, this later gather would trip stale
# scheduler state.
result_after_orphan = await asyncio.gather(slow_orphan())  # pyright: ignore
assert result_after_orphan == ['slow ok'], f'post-orphan gather: {result_after_orphan}'


# === Double-fail: nested gather whose only child raises ===
# The outer failure walk must not re-fail an inner gather that already failed.
async def boom_double_fail():
    raise ValueError('double-fail err')


async def double_fail_main():
    await asyncio.gather(asyncio.gather(boom_double_fail()))


try:
    await double_fail_main()  # pyright: ignore
    assert False, 'double_fail_main should have raised'
except ValueError as e:
    assert str(e) == 'double-fail err', f'double-fail error: {e}'


# === Three-deep nested gather with the deepest child raising ===
# Failure walks up two GatherSlot links; both ancestors must skip the inner
# gather that already failed.
async def boom_triple():
    raise ValueError('triple')


async def triple_main():
    await asyncio.gather(asyncio.gather(asyncio.gather(boom_triple())))


try:
    await triple_main()  # pyright: ignore
    assert False, 'triple_main should have raised'
except ValueError as e:
    assert str(e) == 'triple', f'triple-nested error: {e}'


# === Sibling-failure-with-orphan: outer has nested-gather + coroutine ===
# One inner child raises and propagates upward while the outer sibling has
# already committed. Teardown must cancel that sibling and skip the failed inner.
async def boom_a():
    raise NotImplementedError('a')


async def boom_b():
    raise NotImplementedError('b')


async def ext_c():
    raise NotImplementedError('c')


async def sibling_main():
    inner = asyncio.gather(boom_a(), boom_b())
    outer = asyncio.gather(inner, ext_c())
    try:
        await outer
        assert False, 'sibling_main should have raised'
    except NotImplementedError as e:
        # Any committed sibling may win before teardown reaches the rest.
        assert str(e) in ('a', 'b', 'c'), f'sibling error: {e}'


await sibling_main()  # pyright: ignore


# === Rolled-back gather caches the error: re-await replays it ===
# Re-awaiting a rolled-back gather must replay the cached error.
async def boom_replay():
    raise ValueError('replay')


async def slow_replay():
    return 1


g_replay = asyncio.gather(boom_replay())
try:
    await g_replay  # pyright: ignore
except ValueError:
    pass

outer_replay = asyncio.gather(slow_replay(), g_replay)
try:
    await outer_replay  # pyright: ignore
    assert False, 'first outer_replay should raise'
except ValueError as e:
    assert str(e) == 'replay', f'first outer_replay: {e}'

# Second await on the same outer gather instance — must replay, not retry.
try:
    await outer_replay  # pyright: ignore
    assert False, 'second outer_replay should raise'
except ValueError as e:
    assert str(e) == 'replay', f'second outer_replay: {e}'


# === Cross-gather double-spawn rollback works mid-tree ===
# Cross-gather coroutine reuse must also roll back siblings committed before
# the duplicate spawn is detected.
async def make_payload():
    return 'payload'


c_shared = make_payload()
g_share_1 = asyncio.gather(c_shared)
g_share_2 = asyncio.gather(c_shared)
try:
    await asyncio.gather(g_share_1, g_share_2)  # pyright: ignore
    assert False, 'shared-coroutine outer gather should raise'
except RuntimeError as e:
    assert str(e) == 'cannot reuse already awaited coroutine', f'cross-gather: {e}'

# Heap/scheduler still usable.
final = await asyncio.gather(make_payload(), make_payload())  # pyright: ignore
assert final == ['payload', 'payload'], f'final gather: {final}'
