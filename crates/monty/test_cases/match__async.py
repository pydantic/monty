# run-async
# === match statements inside async functions, with await in guards and bodies ===
import asyncio


async def value():
    await asyncio.sleep(0)
    return 'awaited'


async def choose(x):
    match x:
        case int() if await value() == 'awaited':
            return 'int'
        case str(s):
            return await value() + ' ' + s
    return 'other'


assert await choose(1) == 'int'  # pyright: ignore
assert await choose('x') == 'awaited x'  # pyright: ignore
assert await choose(1.5) == 'other'  # pyright: ignore
