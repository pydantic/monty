# run-async
# Nested async function calls


async def inner():
    return 42


async def outer():
    value = await inner()
    return value + 8


result = await outer()  # pyright: ignore
assert result == 50, 'nested async calls should work'
