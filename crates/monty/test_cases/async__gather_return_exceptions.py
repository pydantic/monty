# /// monty
# call-external = true
# run-async = true
# ///
import asyncio


async def ok(x):
    return x


async def boom(msg):
    raise ValueError(msg)


async def slow_ok(x):
    await asyncio.sleep(0)
    return x


# === Sandbox coroutines: exceptions land in the result list at their position ===
res = await asyncio.gather(ok(1), boom('a'), ok(3), return_exceptions=True)  # pyright: ignore
assert res[0] == 1
assert isinstance(res[1], ValueError) and str(res[1]) == 'a'
assert res[2] == 3
assert len(res) == 3

# === Siblings that finish after the failure still complete, order preserved ===
res = await asyncio.gather(slow_ok(1), boom('b'), slow_ok(3), boom('c'), return_exceptions=True)  # pyright: ignore
assert res[0] == 1 and res[2] == 3
assert str(res[1]) == 'b' and str(res[3]) == 'c'

# === No awaitables, and all successes ===
assert await asyncio.gather(return_exceptions=True) == []  # pyright: ignore
assert await asyncio.gather(ok(1), ok(2), return_exceptions=True) == [1, 2]  # pyright: ignore

# === Explicit False behaves like the default ===
try:
    await asyncio.gather(ok(1), boom('d'), return_exceptions=False)  # pyright: ignore
    assert False, 'should raise'
except ValueError as e:
    assert str(e) == 'd'

# === Truthiness of the flag ===
res = await asyncio.gather(boom('e'), return_exceptions=1)  # pyright: ignore
assert str(res[0]) == 'e'

# === External futures that raise ===
res = await asyncio.gather(async_call(1), async_fail('ValueError', 'ext'), async_call(3), return_exceptions=True)  # pyright: ignore
assert res[0] == 1 and res[2] == 3
assert isinstance(res[1], ValueError) and str(res[1]) == 'ext'

# === External failure and coroutine failure together ===
res = await asyncio.gather(  # pyright: ignore
    async_fail('KeyError', 'k'), boom('f'), ok(5), return_exceptions=True
)
assert isinstance(res[0], KeyError) and str(res[1]) == 'f' and res[2] == 5


# === Host failure inside a coroutine is a task failure, captured ===
async def wraps_ext():
    return await async_fail('RuntimeError', 'inner')


res = await asyncio.gather(wraps_ext(), ok(7), return_exceptions=True)  # pyright: ignore
assert isinstance(res[0], RuntimeError) and str(res[0]) == 'inner' and res[1] == 7

# === Already failed external future, awaited later in a gather ===
failed = async_fail('ValueError', 'early')
try:
    await failed  # pyright: ignore
except ValueError:
    pass
res = await asyncio.gather(failed, ok(1), return_exceptions=True)  # pyright: ignore
assert str(res[0]) == 'early' and res[1] == 1

# === Nested gathers: the flag applies per gather ===
inner = asyncio.gather(boom('g'), ok(2), return_exceptions=True)
res = await asyncio.gather(inner, ok(3))  # pyright: ignore
assert str(res[0][0]) == 'g' and res[0][1] == 2 and res[1] == 3

inner_strict = asyncio.gather(boom('h'), ok(2))
res = await asyncio.gather(inner_strict, ok(3), return_exceptions=True)  # pyright: ignore
assert isinstance(res[0], ValueError) and str(res[0]) == 'h' and res[1] == 3

# === Re-awaiting replays the cached result ===
g = asyncio.gather(boom('i'), ok(1), return_exceptions=True)
first = await g  # pyright: ignore
second = await g  # pyright: ignore
assert str(first[0]) == 'i' and str(second[0]) == 'i'

# === Failure without the flag still propagates ===
try:
    await asyncio.gather(ok(1), async_fail('ValueError', 'strict'))  # pyright: ignore
    assert False, 'should raise'
except ValueError as e:
    assert str(e) == 'strict'

# === Unknown keyword ===
try:
    asyncio.gather(timeout=1)
    assert False, 'should raise'
except TypeError as e:
    assert str(e) == "gather() got an unexpected keyword argument 'timeout'"
