# run-async
"""Mid-commit errors in `asyncio.gather` must roll back already-committed
siblings, and failure propagation must not re-fail an already-Failed nested
gather.

Two distinct topologies historically reached the same
`expect("...non-Awaited gather")` panics at `async_exec.rs:957` /
`async_exec.rs:1030`:

1. **Commit-time orphan.** An earlier item in the outer gather successfully
   commits (spawn / sub-awaiter install / nested-gather Awaited), then a
   *later* item raises synchronously (already-`Failed` nested gather, etc.).
   The outer gather never reaches `Awaited`, but the earlier sibling's
   task/awaiter survives in the scheduler, points back at the still-`Pending`
   outer, and panics on next pickup. Rollback cancels the sibling.
2. **Double-fail on nested gather.** A child coroutine of `g_inner` raises;
   propagation walks `g_inner → g_outer` via `Awaiter::GatherSlot`. `g_outer`
   then iterates its own `pending_children`, which still contains the
   already-`Failed` `g_inner`, and previously called `g_inner.fail(...)` a
   second time. The teardown helper now skips nested children that aren't
   `Awaited`.
"""

import asyncio


# === Commit-time orphan: Failed nested gather is the second item ===
# Smallest published repro of the orphan. Outer spawns slow() first, then hits
# `g_failed` which raises synchronously. Rollback must cancel slow()'s task;
# the post-error gather then runs cleanly.
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

# If `slow_orphan`'s task was orphaned, scheduling another await would let it
# complete and trip `resolve_child` on a `Pending` gather (the panic at
# `async_exec.rs:957`).
result_after_orphan = await asyncio.gather(slow_orphan())  # pyright: ignore
assert result_after_orphan == ['slow ok'], f'post-orphan gather: {result_after_orphan}'


# === Double-fail: nested gather whose only child raises ===
# `gather(gather(b()))` previously triggered `expect("fail called on
# non-Awaited gather")` because the outer's failure walk re-failed the
# already-Failed inner.
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
# Failure walks up two GatherSlot links; both ancestors must skip the
# already-Failed entry in their own pending_children.
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
# `gather(gather(boom_a(), boom_b()), ext_c())` — inner gather has two
# children; one raises and propagates upward, leaving outer's other coroutine
# (ext_c) committed. The fail-walk on outer must (a) cancel ext_c, (b) skip
# the already-Failed inner.
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
        # The first child of inner to be scheduled raises and is the one
        # whose error wins; both 'a' and 'b' are valid depending on schedule
        # order. Same for ext_c('c').
        assert str(e) in ('a', 'b', 'c'), f'sibling error: {e}'


await sibling_main()  # pyright: ignore


# === Rolled-back gather caches the error: re-await replays it ===
# After rollback transitions the outer gather to `Failed`, a second `await`
# on the same instance must replay the cached error rather than retry the
# (still-broken) commit.
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
# Cross-gather coroutine reuse no longer panics — but it now also has to
# roll back: in `await gather(g1, g2)`, g1 commits (spawns c), then g2 hits
# spawn-None for the same c. The rollback must cancel c's task spawned by g1
# so a subsequent gather over a fresh coroutine completes cleanly.
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
