# run-async
# === match statements inside async functions, with await in guards and bodies ===
import asyncio


async def value(x=None):
    await asyncio.sleep(0)
    return 'rejected' if x == 0 else 'awaited'


async def choose(x):
    match x:
        case int() if await value(x) == 'awaited':
            return 'int'
        case int(n):
            return f'guard failed for {n}'
        case str(s):
            return await value() + ' ' + s
    return 'other'


assert await choose(1) == 'int'  # pyright: ignore
assert await choose(0) == 'guard failed for 0'  # pyright: ignore
assert await choose('x') == 'awaited x'  # pyright: ignore
assert await choose(1.5) == 'other'  # pyright: ignore
