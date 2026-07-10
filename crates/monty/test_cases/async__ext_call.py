# call-external
# run-async
# Test async external function calls (coroutines)

# === Basic async external call ===
result = await async_call(42)  # pyright: ignore
assert result == 42

# === Async call with string ===
s = await async_call('hello')  # pyright: ignore
assert s == 'hello'

# === Async call with list ===
lst = await async_call([1, 2, 3])  # pyright: ignore
assert lst == [1, 2, 3]

# === Multiple async calls ===
a = await async_call(10)  # pyright: ignore
b = await async_call(20)  # pyright: ignore
assert a + b == 30

# === Gather multiple external async calls ===
import asyncio

results = await asyncio.gather(async_call(1), async_call(2), async_call(3))  # pyright: ignore
assert results == [1, 2, 3]

# === Gather with mixed external calls ===
results = await asyncio.gather(async_call('a'), async_call('b'))  # pyright: ignore
assert results == ['a', 'b']


# === Gather mixing coroutines and external futures ===
async def add(a, b):
    return a + b


async def multiply(a, b):
    return a * b


# Mix: coroutine first, external future second
results = await asyncio.gather(add(1, 2), async_call(10))  # pyright: ignore
assert results == [3, 10]

# Mix: external future first, coroutine second
results = await asyncio.gather(async_call(20), multiply(3, 4))  # pyright: ignore
assert results == [20, 12]

# Mix: multiple of each interleaved
results = await asyncio.gather(add(5, 5), async_call('x'), multiply(2, 3), async_call('y'))  # pyright: ignore
assert results == [10, 'x', 6, 'y']


# === Coroutine with nested external awaits ===
async def double_external(x):
    val = await async_call(x)
    return val * 2


results = await asyncio.gather(double_external(5), async_call(100))  # pyright: ignore
assert results == [10, 100]


# === Coroutine with multiple nested awaits ===
async def triple_add(a, b, c):
    x = await async_call(a)
    y = await async_call(b)
    return x + y + c


results = await asyncio.gather(triple_add(1, 2, 3), async_call(50))  # pyright: ignore
assert results == [6, 50]


# === Gather with the same external future passed twice ===
f = async_call(7)
results = await asyncio.gather(f, f)  # pyright: ignore
assert results == [7, 7], f'duplicate external future dedup: {results}'

# Mixed with unique external futures around the duplicate.
g = async_call('dup')
results = await asyncio.gather(async_call('a'), g, async_call('b'), g)  # pyright: ignore
assert results == ['a', 'dup', 'b', 'dup'], f'mixed external future dedup: {results}'


# === Same external future shared across sibling gathers ===
# Two concurrent helpers each await their own gather around the SAME external
# future. In Monty the future is still `Pending` at this point so the second
# gather sees the awaiter slot already taken and raises RuntimeError. In
# CPython the test harness now returns an already-resolved `asyncio.Future`,
# which permits multi-await, so both gathers settle with `[99]`. Both
# outcomes are accepted as they reflect a genuine semantic difference (Monty
# rejects concurrent awaiters on a `Pending` future; multi-awaiter on
# `Pending` is a planned follow-up).
async def helper(future):
    return await asyncio.gather(future)


shared = async_call(99)
try:
    result = await asyncio.gather(helper(shared), helper(shared))  # pyright: ignore
    assert result == [[99], [99]], f'CPython multi-await of resolved future: {result}'
except RuntimeError as e:
    assert str(e) == 'cannot reuse already awaited future', f'Monty rejection: {e}'


# === Re-awaiting a resolved external future returns the cached value ===

reawait_f = async_call('cached')
first = await reawait_f  # pyright: ignore
second = await reawait_f  # pyright: ignore
third = await reawait_f  # pyright: ignore
assert first == 'cached', f'first await: {first}'
assert second == 'cached', f'second await: {second}'
assert third == 'cached', f'third await: {third}'

reawait_list = async_call([1, 2, 3])
first_list = await reawait_list  # pyright: ignore
second_list = await reawait_list  # pyright: ignore
assert first_list is second_list
first_list.append(99)
assert second_list == [1, 2, 3, 99]
