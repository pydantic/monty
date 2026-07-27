# run-async
# A decorated `async def` is awaited like any other coroutine — both when the
# decorator returns the function untouched and when it wraps it in another
# coroutine function.


def identity(fn):
    return fn


def double(fn):
    async def wrapper(n):
        return await fn(n) * 2

    return wrapper


# === An identity decorator leaves the coroutine function alone ===
@identity
async def plain(n):
    return n + 1


assert (await plain(1)) == 2  # pyright: ignore


# === A decorator may replace it with another coroutine function ===
@double
async def doubled(n):
    return n + 1


assert (await doubled(1)) == 4  # pyright: ignore


# === Stacked: `double` applies first, then `identity` passes the wrapper on ===
@identity
@double
async def stacked(n):
    return n + 1


assert (await stacked(3)) == 8  # pyright: ignore


# === A decorator can capture an enclosing local and use it in the wrapper ===
def make_offset(step):
    def deco(fn):
        async def wrapper(n):
            return await fn(n) + step

        return wrapper

    return deco


@make_offset(100)
async def offset(n):
    return n


assert (await offset(5)) == 105  # pyright: ignore
