import asyncio


# === Basic asyncio.run ===
async def simple():
    return 42


result = asyncio.run(simple())
assert result == 42, f'basic asyncio.run failed: {result}'


# === With arguments ===
async def add(a, b):
    return a + b


result = asyncio.run(add(10, 20))
assert result == 30, f'asyncio.run with args failed: {result}'


# === Nested awaits inside the coroutine ===
async def inner():
    return 'hello'


async def outer():
    val = await inner()
    return val + ' world'


result = asyncio.run(outer())
assert result == 'hello world', f'nested awaits failed: {result}'


# === asyncio.gather inside asyncio.run ===
async def double(x):
    return x * 2


async def run_gather():
    results = await asyncio.gather(double(1), double(2), double(3))
    return results


result = asyncio.run(run_gather())
assert result == [2, 4, 6], f'gather inside run failed: {result}'
