# xfail=cpython
# Async functions returning different types


async def return_int():
    return 42


async def return_str():
    return 'hello'


async def return_list():
    return [1, 2, 3]


async def return_none():
    pass


i = await return_int()
assert i == 42, 'should return int'

s = await return_str()
assert s == 'hello', 'should return str'

lst = await return_list()
assert lst == [1, 2, 3], 'should return list'

n = await return_none()
assert n is None, 'should return None implicitly'
