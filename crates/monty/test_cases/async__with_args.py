# run-async
# Async function with arguments


async def add(a, b):
    return a + b


result = await add(10, 20)  # pyright: ignore
assert result == 30, 'async function should handle arguments'

# With keyword arguments
result2 = await add(a=5, b=15)  # pyright: ignore
assert result2 == 20, 'async function should handle keyword arguments'
