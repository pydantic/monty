# run-async
# Multiple sequential awaits


async def get_value(x):
    return x * 2


a = await get_value(1)  # pyright: ignore
b = await get_value(2)  # pyright: ignore
c = await get_value(3)  # pyright: ignore

assert a == 2, 'first await'
assert b == 4, 'second await'
assert c == 6, 'third await'
assert a + b + c == 12, 'sum of sequential awaits'
