# run-async
# Multiple sequential awaits


async def get_value(x):
    return x * 2


a = await get_value(1)  # pyright: ignore
b = await get_value(2)  # pyright: ignore
c = await get_value(3)  # pyright: ignore

assert a == 2
assert b == 4
assert c == 6
assert a + b + c == 12
