# xfail=cpython
# Multiple sequential awaits


async def get_value(x):
    return x * 2


a = await get_value(1)
b = await get_value(2)
c = await get_value(3)

assert a == 2, 'first await'
assert b == 4, 'second await'
assert c == 6, 'third await'
assert a + b + c == 12, 'sum of sequential awaits'
