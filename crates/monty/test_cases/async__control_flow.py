# run-async
# Control flow (break/continue/return) through try/finally around awaits.
# The unwind machinery must behave identically when suspension points sit
# inside the protected regions.


async def value(v):
    return v


async def boom(msg):
    raise ValueError(msg)


# === finally with break swallows an exception from an awaited coroutine ===
async def swallow_awaited_error():
    log = []
    while True:
        try:
            await boom('swallowed')
        finally:
            log.append('finally')
            break
    return log


assert await swallow_awaited_error() == ['finally']  # pyright: ignore

# === return in finally swallows an awaited exception ===


async def return_in_finally():
    try:
        await boom('swallowed')
    finally:
        return 'returned'


assert await return_in_finally() == 'returned'  # pyright: ignore

# === await inside the finally body on the exception path ===


async def await_in_finally():
    log = []
    try:
        try:
            raise ValueError('original')
        finally:
            log.append(await value('from finally'))
    except ValueError as e:
        log.append(str(e))
    return log


assert await await_in_finally() == ['from finally', 'original']  # pyright: ignore

# === return value awaited before the finally runs ===


async def order_check():
    log = []

    async def tag(v):
        log.append(v)
        return v

    async def inner():
        try:
            return await tag('value')
        finally:
            log.append('finally')

    result = await inner()
    return result, log


assert await order_check() == ('value', ['value', 'finally'])  # pyright: ignore

# === break through finally with an await between iterations ===


async def loop_with_awaits():
    log = []
    for i in range(5):
        try:
            log.append(await value(i))
            if i == 1:
                break
        finally:
            log.append('finally')
    return log


assert await loop_with_awaits() == [0, 'finally', 1, 'finally']  # pyright: ignore
