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
