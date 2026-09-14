# call-external
# run-async
# `asyncio.sleep()`: awaited on its own, gathered, and with a `result`.
import asyncio


def check(fn, expected):
    try:
        fn()
        raise AssertionError('expected failure')
    except Exception as exc:
        got = f'{type(exc).__name__}: {exc}'
        assert got == expected, f'{got!r} != {expected!r}'


# === awaiting ===
assert await asyncio.sleep(0) is None  # pyright: ignore
assert await asyncio.sleep(0.001) is None  # pyright: ignore
assert await asyncio.sleep(0, 'done') == 'done'  # pyright: ignore
assert await asyncio.sleep(0, result='kw') == 'kw'  # pyright: ignore
assert await asyncio.sleep(0, [1, 2]) == [1, 2]  # pyright: ignore

# === gathered ===
assert await asyncio.gather(asyncio.sleep(0, 1), asyncio.sleep(0.001, 2)) == [1, 2]  # pyright: ignore


async def wait_then(value):
    await asyncio.sleep(0.001)
    return value * 2


assert await asyncio.gather(wait_then(1), wait_then(2), wait_then(3)) == [2, 4, 6]  # pyright: ignore

# === delays CPython accepts unchanged ===
# a negative delay returns immediately rather than raising, as it does on CPython
assert await asyncio.sleep(-5, 'negative') == 'negative'  # pyright: ignore

# NaN is the one delay CPython refuses, though Monty raises it at the call
# rather than at the await
try:
    await asyncio.sleep(float('nan'), 'nan')  # pyright: ignore
    raise AssertionError('expected failure')
except ValueError as exc:
    assert str(exc) == 'Invalid delay: NaN (not a number)'

# === signature errors ===
check(lambda: asyncio.sleep(), "TypeError: sleep() missing 1 required positional argument: 'delay'")
check(lambda: asyncio.sleep(0, 1, 2), 'TypeError: sleep() takes from 1 to 2 positional arguments but 3 were given')
check(lambda: asyncio.sleep(0, bad=1), "TypeError: sleep() got an unexpected keyword argument 'bad'")

# a non-numeric delay fails as the `delay <= 0` comparison CPython makes,
# though Monty raises it at the call rather than at the await
try:
    await asyncio.sleep('a')  # pyright: ignore
    raise AssertionError('expected failure')
except TypeError as exc:
    assert str(exc) == "'<=' not supported between instances of 'str' and 'int'"


# only real numbers are delays: unlike `time.sleep()`, an __index__-able class
# is rejected, since CPython's `delay <= 0` never reaches __index__


class Index:
    def __index__(self) -> int:
        return 0


try:
    await asyncio.sleep(Index())  # pyright: ignore
    raise AssertionError('expected failure')
except TypeError as exc:
    assert str(exc) == "'<=' not supported between instances of 'Index' and 'int'"
