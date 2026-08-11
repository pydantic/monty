# run-async
"""Cross-gather coroutine reuse must raise `RuntimeError`, not panic."""

import asyncio


# === Canonical: g1 = gather(c); g2 = gather(c); await gather(g1, g2) ===
async def foo_canonical():
    return [1, 2, 3]


async def main_canonical():
    c = foo_canonical()
    g1 = asyncio.gather(c)
    g2 = asyncio.gather(c)
    try:
        await asyncio.gather(g1, g2)
        assert False, 'cross-gather reuse should have raised RuntimeError'
    except RuntimeError as e:
        assert str(e) == 'cannot reuse already awaited coroutine', f'canonical: unexpected error: {e}'


await main_canonical()  # pyright: ignore


# === Nested: gather(gather(c), gather(c)) ===
# Same shape, simpler driver — no `main()` wrapper, no intermediate references.
async def foo_nested():
    return 1


c_nested = foo_nested()
try:
    await asyncio.gather(asyncio.gather(c_nested), asyncio.gather(c_nested))  # pyright: ignore
    assert False, 'nested cross-gather reuse should have raised RuntimeError'
except RuntimeError as e:
    assert str(e) == 'cannot reuse already awaited coroutine', f'nested: unexpected error: {e}'


# === Sequential: g1 = gather(c); g2 = gather(c); await g1; await g2 ===
# CPython: `await g1` resolves successfully with `[result]`; `await g2` raises
# because the coroutine was already driven by g1.
async def foo_seq():
    return 42


c_seq = foo_seq()
g1_seq = asyncio.gather(c_seq)
g2_seq = asyncio.gather(c_seq)
r1_seq = await g1_seq  # pyright: ignore
assert r1_seq == [42], f'sequential g1 should resolve to [42], got {r1_seq}'
try:
    await g2_seq  # pyright: ignore
    assert False, 'sequential g2 should have raised RuntimeError'
except RuntimeError as e:
    assert str(e) == 'cannot reuse already awaited coroutine', f'sequential g2: unexpected error: {e}'


# === Three separate gathers sharing one coroutine ===
async def foo_three():
    return 'x'


c_three = foo_three()
try:
    await asyncio.gather(asyncio.gather(c_three), asyncio.gather(c_three), asyncio.gather(c_three))  # pyright: ignore
    assert False, 'three-way cross-gather reuse should have raised RuntimeError'
except RuntimeError as e:
    assert str(e) == 'cannot reuse already awaited coroutine', f'three-way: unexpected error: {e}'


# === Direct await then gather: await c; await gather(c) ===
# Covered by the direct await state check; kept with the gather reuse matrix.
async def foo_direct_first():
    return 9


c_direct_first = foo_direct_first()
r_direct = await c_direct_first  # pyright: ignore
assert r_direct == 9, f'direct await should return 9, got {r_direct}'
try:
    await asyncio.gather(c_direct_first)  # pyright: ignore
    assert False, 'gather after direct await should have raised'
except RuntimeError as e:
    assert str(e) == 'cannot reuse already awaited coroutine', f'direct→gather: unexpected error: {e}'


# === Gather then direct await: g = gather(c); await g; await c ===
async def foo_gather_first():
    return 11


c_gather_first = foo_gather_first()
g_first = asyncio.gather(c_gather_first)
r_g = await g_first  # pyright: ignore
assert r_g == [11], f'gather should resolve to [11], got {r_g}'
try:
    await c_gather_first  # pyright: ignore
    assert False, 'direct await after gather should have raised'
except RuntimeError as e:
    assert str(e) == 'cannot reuse already awaited coroutine', f'gather→direct: unexpected error: {e}'


# === Cleanup smoke test: scheduler/heap still usable after the error path ===
# The scheduler and heap should still be usable after the error path.
async def foo_cleanup_doomed():
    return 1


async def foo_cleanup_ok():
    return 'ok'


c_doomed = foo_cleanup_doomed()
try:
    await asyncio.gather(asyncio.gather(c_doomed), asyncio.gather(c_doomed))  # pyright: ignore
    assert False, 'cleanup doomed gather should have raised'
except RuntimeError:
    pass

# Fresh coroutines should still drive a fresh gather to completion.
clean_result = await asyncio.gather(foo_cleanup_ok(), foo_cleanup_ok())  # pyright: ignore
assert clean_result == ['ok', 'ok'], f'post-error gather should still work, got {clean_result}'
