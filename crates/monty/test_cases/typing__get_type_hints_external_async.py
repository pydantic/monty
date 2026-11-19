# /// monty
# call-external = true
# run-async = true
# ///
# Host hints asked for between awaits and pending external futures.
import asyncio
from typing import get_type_hints


async def hints_after_await(func):
    value = await async_call(1)
    return value, get_type_hints(func)


# === Before and after an await ===
assert get_type_hints(add_ints) == {'a': int, 'b': int, 'return': int}
assert await hints_after_await(concat_strings) == (1, {'a': str, 'b': str, 'return': str})  # pyright: ignore
assert get_type_hints(type(make_point())) == {'x': int, 'y': int}

# === Interleaved in gathered tasks ===
results = await asyncio.gather(  # pyright: ignore
    hints_after_await(add_ints),
    hints_after_await(raise_error),
    async_call('done'),
)
assert results == [
    (1, {'a': int, 'b': int, 'return': int}),
    (1, {'exc_type': str, 'message': str, 'return': type(None)}),
    'done',
]

# === While an external future is pending ===
pending = async_call(2)
hints = get_type_hints(type(make_user('Samuel')))
assert hints == {'name': str, 'active': bool}
assert await pending == 2  # pyright: ignore


# === In an exception handler after a failed await ===
async def hints_on_failure():
    try:
        await async_fail('KeyError', 'gone')
        assert False, 'expected KeyError'
    except KeyError:
        return get_type_hints(add_ints)


assert await hints_on_failure() == {'a': int, 'b': int, 'return': int}  # pyright: ignore
