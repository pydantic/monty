# run-async
# Basic async function that returns a value


async def foo():
    return 123


result = await foo()  # pyright: ignore
assert result == 123, 'async function should return awaited value'
