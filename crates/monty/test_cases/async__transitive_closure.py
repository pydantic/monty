# run-async
# Async coroutines capturing variables more than one level up. Calling an async
# function builds a coroutine (a separate frame-setup path from a sync call), so
# this exercises transitive cell threading on the coroutine path too.


# === Async coroutine reads a variable two levels up ===
def outer(a):
    def mid():
        async def inner():
            return a * 3

        return inner()

    return mid()


assert (await outer(7)) == 21, 'async two-level read'  # pyright: ignore


# === Async coroutine writes a grandparent local via nonlocal ===
def counter():
    n = 0

    def mid():
        async def bump():
            nonlocal n
            n += 1
            return n

        return bump

    return mid()


c = counter()
assert (await c()) == 1, 'async nonlocal write, first call'  # pyright: ignore
assert (await c()) == 2, 'async nonlocal write, second call'  # pyright: ignore


# === Each async closure instance captures its own cell ===
def make(n):
    def mid():
        async def add(x):
            return x + n

        return add

    return mid()


assert (await make(3)(10)) == 13, 'first async closure captures n=3'  # pyright: ignore
assert (await make(5)(10)) == 15, 'second async closure captures n=5'  # pyright: ignore
