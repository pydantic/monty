# run-async
# call-external
# Control flow (break/continue/return) through try/finally around awaits.
# The unwind machinery must behave identically when suspension points sit
# inside the protected regions — whether the awaited coroutine completes
# immediately (local coroutines) or genuinely suspends and resumes
# (pending external futures resolved via ResolveFutures, with a dump/load
# round-trip at every suspension in the test harness).


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

# === continue in finally swallows an awaited exception and resumes the loop ===


async def continue_swallows_awaited_error():
    log = []
    for i in range(3):
        try:
            log.append(await value(i))
            if i < 2:
                await boom('swallowed')
            log.append('clean')
        finally:
            log.append('finally')
            if i < 2:
                continue
        log.append('after')
    return log


assert await continue_swallows_awaited_error() == [0, 'finally', 1, 'finally', 2, 'clean', 'finally', 'after']  # pyright: ignore

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

# === pending external await: break in finally swallows the suspended failure ===
# `async_fail` suspends into ResolveFutures and resumes with the exception
# in-flight; the finally then suspends again on its own pending await before
# break swallows the exception.


async def swallow_pending_failure():
    log = []
    while True:
        try:
            await async_fail('ValueError', 'pending failure')  # pyright: ignore
        finally:
            log.append(await async_call('finally'))  # pyright: ignore
            break
    return log


assert await swallow_pending_failure() == ['finally']  # pyright: ignore

# === pending external await: return in finally swallows the suspended failure ===


async def return_swallows_pending_failure():
    try:
        await async_fail('ValueError', 'pending failure')  # pyright: ignore
    finally:
        return await async_call('returned')  # pyright: ignore


assert await return_swallows_pending_failure() == 'returned'  # pyright: ignore

# === pending external await: continue in finally swallows per-iteration failures ===


async def continue_swallows_pending_failures():
    log = []
    for i in range(3):
        try:
            log.append(await async_call(i))  # pyright: ignore
            if i < 2:
                await async_fail('ValueError', 'pending failure')  # pyright: ignore
        finally:
            if i < 2:
                continue
        log.append('clean')
    return log


assert await continue_swallows_pending_failures() == [0, 1, 2, 'clean']  # pyright: ignore
