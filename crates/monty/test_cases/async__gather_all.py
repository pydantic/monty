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
