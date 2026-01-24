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
