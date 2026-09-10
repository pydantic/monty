# run-async
# Async function with arguments


async def add(a, b):
    return a + b


result = await add(10, 20)  # pyright: ignore
assert result == 30

# With keyword arguments
result2 = await add(a=5, b=15)  # pyright: ignore
assert result2 == 20


# Exact positional arguments move directly into the new coroutine namespace.
async def exact_async(first, second, /):
    local = [first, second]
    return local


first = [1]
second = {'value': 2}
pending = exact_async(first, second)
exact_result = await pending  # pyright: ignore
assert exact_result[0] is first
assert exact_result[1] is second

try:
    exact_async(first)
    assert False, 'expected TypeError for missing positional argument'
except TypeError as exc:
    # `endswith`, not `==`: CPython prefixes nested functions with their
    # `__qualname__` (here the harness's wrapping function), which Monty
    # doesn't implement (see limitations/language.md).
    assert str(exc).endswith("exact_async() missing 1 required positional argument: 'second'")


# Async functions with owned cells retain the regular namespace setup path.
async def async_with_cell(value):
    def read():
        return value

    return read()


assert await async_with_cell(42) == 42  # pyright: ignore
