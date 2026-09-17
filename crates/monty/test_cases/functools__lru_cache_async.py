# run-async
# Caching an `async def` stores the coroutine the call returns, not the value it
# resolves to, so the second call hands back a coroutine that has already been
# awaited. CPython behaves the same way; this is the footgun the `functools`
# docs warn about, not a Monty divergence.
import functools

calls = []


@functools.cache
async def fetch(n):
    calls.append(n)
    return n * 2


assert (await fetch(1)) == 2  # pyright: ignore
assert calls == [1]
assert fetch.cache_info() == (0, 1, None, 1)

try:
    await fetch(1)  # pyright: ignore
    assert False, 'expected the second await to fail'
except RuntimeError as exc:
    assert str(exc) == 'cannot reuse already awaited coroutine'

# The body ran once: the second call never reached it.
assert calls == [1]
assert fetch.cache_info() == (1, 1, None, 1)

# A different argument is a different coroutine.
assert (await fetch(2)) == 4  # pyright: ignore
assert calls == [1, 2]
