# call-external
# run-async
# Test async external function calls (coroutines)

# === Basic async external call ===
result = await async_call(42)  # pyright: ignore
assert result == 42, 'async_call should return awaited value'

# === Async call with string ===
s = await async_call('hello')  # pyright: ignore
assert s == 'hello', 'async_call should work with strings'

# === Async call with list ===
lst = await async_call([1, 2, 3])  # pyright: ignore
assert lst == [1, 2, 3], 'async_call should work with lists'

# === Multiple async calls ===
a = await async_call(10)  # pyright: ignore
b = await async_call(20)  # pyright: ignore
assert a + b == 30, 'multiple async calls should work'

# === Gather multiple external async calls ===
import asyncio

results = await asyncio.gather(async_call(1), async_call(2), async_call(3))  # pyright: ignore
assert results == [1, 2, 3], 'gather should collect external async results in order'

# === Gather with mixed external calls ===
results = await asyncio.gather(async_call('a'), async_call('b'))  # pyright: ignore
assert results == ['a', 'b'], 'gather should work with string returns'


# === Gather mixing coroutines and external futures ===
async def add(a, b):
    return a + b


async def multiply(a, b):
    return a * b


# Mix: coroutine first, external future second
results = await asyncio.gather(add(1, 2), async_call(10))  # pyright: ignore
assert results == [3, 10], 'gather should work with coroutine then external future'

# Mix: external future first, coroutine second
results = await asyncio.gather(async_call(20), multiply(3, 4))  # pyright: ignore
assert results == [20, 12], 'gather should work with external future then coroutine'

# Mix: multiple of each interleaved
results = await asyncio.gather(add(5, 5), async_call('x'), multiply(2, 3), async_call('y'))  # pyright: ignore
assert results == [10, 'x', 6, 'y'], 'gather should handle interleaved coroutines and external futures'


# === Coroutine with nested external awaits ===
async def double_external(x):
    val = await async_call(x)
    return val * 2


results = await asyncio.gather(double_external(5), async_call(100))  # pyright: ignore
assert results == [10, 100], 'gather should work with coroutine that awaits external'


# === Coroutine with multiple nested awaits ===
async def triple_add(a, b, c):
    x = await async_call(a)
    y = await async_call(b)
    return x + y + c


results = await asyncio.gather(triple_add(1, 2, 3), async_call(50))  # pyright: ignore
assert results == [6, 50], 'gather should work with coroutine with multiple external awaits'


# === Gather with the same external future passed twice ===
f = async_call(7)
results = await asyncio.gather(f, f)  # pyright: ignore
assert results == [7, 7], f'duplicate external future dedup: {results}'

# Mixed with unique external futures around the duplicate.
g = async_call('dup')
results = await asyncio.gather(async_call('a'), g, async_call('b'), g)  # pyright: ignore
assert results == ['a', 'dup', 'b', 'dup'], f'mixed external future dedup: {results}'


# === Same external future shared across sibling gathers raises ===
# Two concurrent helpers each await their own gather around the SAME external
# future. Without rejection, the second gather's `register_gather_for_call`
# would silently overwrite the first in the scheduler's waiters map, leaving
# the first gather permanently blocked. We treat the second use as a reuse of
# an already-awaited future, mirroring direct `await f; await f` behaviour.
# CPython models the test fixture's `async_call` as `async def`, so it raises
# `cannot reuse already awaited coroutine` at the same point.
async def helper(future):
    return await asyncio.gather(future)


shared = async_call(99)
try:
    await asyncio.gather(helper(shared), helper(shared))  # pyright: ignore
    assert False, 'should have raised RuntimeError'
except RuntimeError as e:
    assert str(e) in (
        'cannot reuse already awaited future',  # Monty: ExternalFuture path
        'cannot reuse already awaited coroutine',  # CPython: coroutine path
    ), f'unexpected error: {e}'
