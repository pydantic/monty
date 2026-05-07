# run-async
import asyncio


# === Basic gather ===
async def task1():
    return 1


async def task2():
    return 2


result = await asyncio.gather(task1(), task2())  # pyright: ignore
assert result == [1, 2], 'gather should return results as a list'


# === Result ordering ===
# Results should be in argument order, not completion order
async def slow():
    return 'slow'


async def fast():
    return 'fast'


result = await asyncio.gather(slow(), fast())  # pyright: ignore
assert result == ['slow', 'fast'], 'gather should preserve argument order'

# === Empty gather ===
result = await asyncio.gather()  # pyright: ignore
assert result == [], 'empty gather should return empty list'


# === Single coroutine ===
async def single():
    return 42


result = await asyncio.gather(single())  # pyright: ignore
assert result == [42], 'gather with single coroutine should return list with one element'

# === repr of gather function ===
r = repr(asyncio.gather)
assert r.startswith('<function gather at 0x'), f'repr should start with: {r}'

# === TypeError for non-awaitable argument ===
try:
    await asyncio.gather(123)  # pyright: ignore
    assert False, 'should have raised TypeError'
except TypeError as e:
    assert str(e) == 'An asyncio.Future, a coroutine or an awaitable is required'


# === *args unpacking with gather ===
async def a():
    return 'a'


async def b():
    return 'b'


async def c():
    return 'c'


# Unpack a list of coroutines
coros = [a(), b(), c()]
result = await asyncio.gather(*coros)  # pyright: ignore
assert result == ['a', 'b', 'c'], f'gather with *args unpacking: {result}'

# Unpack with mixed args
result = await asyncio.gather(a(), *[b(), c()])  # pyright: ignore
assert result == ['a', 'b', 'c'], f'gather with mixed args and *unpacking: {result}'

# Unpack empty list
result = await asyncio.gather(*[])  # pyright: ignore
assert result == [], f'gather with empty *args: {result}'

# Unpack tuple
coro_tuple = (a(), b())
result = await asyncio.gather(*coro_tuple)  # pyright: ignore
assert result == ['a', 'b'], f'gather with *tuple unpacking: {result}'


# === gather with the same coroutine passed twice ===
dup_runs = [0]


async def dup():
    dup_runs[0] += 1
    return 1


dup_coro = dup()
result = await asyncio.gather(dup_coro, dup_coro)  # pyright: ignore
assert result == [1, 1], f'expected [1, 1], got {result}'
assert dup_runs[0] == 1, f'coroutine body should run once, ran {dup_runs[0]} times'


# Three duplicates and a mix of duplicates with a unique coroutine.
async def dup3():
    return 'x'


dup3_coro = dup3()
result = await asyncio.gather(dup3_coro, dup3_coro, dup3_coro)  # pyright: ignore
assert result == ['x', 'x', 'x'], f'expected three xs, got {result}'


async def alpha():
    return 'a'


async def beta():
    return 'b'


a_coro = alpha()
b_coro = beta()
result = await asyncio.gather(a_coro, b_coro, a_coro)  # pyright: ignore
assert result == ['a', 'b', 'a'], f'mixed dedup: expected [a, b, a], got {result}'


# === gather with an already-awaited coroutine raises RuntimeError ===
async def already():
    return 1


already_coro = already()
await already_coro  # pyright: ignore
try:
    await asyncio.gather(already_coro)  # pyright: ignore
    assert False, 'should have raised RuntimeError'
except RuntimeError as e:
    assert str(e) == 'cannot reuse already awaited coroutine', f'unexpected error: {e}'
