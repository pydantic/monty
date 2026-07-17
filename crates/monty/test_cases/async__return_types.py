# run-async
# Async functions returning different types


async def return_int():
    return 42


async def return_str():
    return 'hello'


async def return_list():
    return [1, 2, 3]


async def return_none():
    pass


i = await return_int()  # pyright: ignore
assert i == 42

s = await return_str()  # pyright: ignore
assert s == 'hello'

lst = await return_list()  # pyright: ignore
assert lst == [1, 2, 3]

n = await return_none()  # pyright: ignore
assert n is None
