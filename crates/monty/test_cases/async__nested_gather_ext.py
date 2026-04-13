# call-external
# run-async
# Test nested asyncio.gather where outer gather spawns tasks that each
# do a sequential external await followed by an inner gather of external calls.
import asyncio


async def process_item(n):
    base = await async_call(n * 10)
    left = async_call(base + 1)
    right = async_call(base + 2)
    parts = await asyncio.gather(left, right)
    return {'n': n, 'parts': parts}


results = await asyncio.gather(  # pyright: ignore
    process_item(1), process_item(2), process_item(3)
)
assert results == [
    {'n': 1, 'parts': [11, 12]},
    {'n': 2, 'parts': [21, 22]},
    {'n': 3, 'parts': [31, 32]},
], f'nested gather with external calls: {results}'


# === Nested gather with generator unpacking ===
async def fetch_pair(key):
    val = await async_call(key)
    return val


async def fetch_all(keys):
    return await asyncio.gather(*(fetch_pair(k) for k in keys))


outer = await asyncio.gather(fetch_all([1, 2]), fetch_all([3, 4]))  # pyright: ignore
assert outer == [[1, 2], [3, 4]], f'nested gather with generator unpacking: {outer}'
