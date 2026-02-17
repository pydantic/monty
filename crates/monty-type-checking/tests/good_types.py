import asyncio
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import assert_type

# === Type checking helper functions ===


def check_int(x: int) -> None:
    pass


def check_float(x: float) -> None:
    pass


def check_str(x: str) -> None:
    pass


def check_bool(x: bool) -> None:
    pass


def check_bytes(x: bytes) -> None:
    pass


def check_list_int(x: list[int]) -> None:
    pass


def check_list_str(x: list[str]) -> None:
    pass


def check_tuple_int(x: tuple[int, ...]) -> None:
    pass


def check_dict_str_int(x: dict[str, int]) -> None:
    pass


def check_set_int(x: set[int]) -> None:
    pass


def check_frozenset_int(x: frozenset[int]) -> None:
    pass


# === Value getter functions ===


def get_int() -> int:
    return 123


def get_float() -> float:
    return 3.14


def get_str() -> str:
    return 'hello'


def get_list_int() -> list[int]:
    return [1, 2, 3]


def get_list_str() -> list[str]:
    return ['a', 'b', 'c']


def get_object() -> object:
    return object()


def get_dict_str_int() -> dict[str, int]:
    return {'a': 1, 'b': 2}


def get_set_str() -> set[str]:
    return {'a', 'b', 'c'}


def get_frozenset_str() -> frozenset[str]:
    return frozenset({'a', 'b', 'c'})


def get_tuple_str_int() -> tuple[str, int]:
    return ('hello', 42)


def get_bytes() -> bytes:
    return b'hello'


# === Core Types ===

obj = object()
t = type(42)


# === Primitive Types ===

# bool
check_bool(True)
check_bool(False)
check_bool(bool(1))
check_bool(bool(''))

# int
check_int(42)
check_int(int('42'))
check_int(int(3.14))
check_int(int(get_int()))
check_int(int(get_float()))

# float
check_float(3.14)
check_float(float('3.14'))
check_float(float(42))
f = get_float()
assert_type(f, float)


# === String and Bytes Types ===

# str
check_str('hello')
check_str(str(42))
check_str(str(b'hello', 'utf-8'))
check_str(str(get_int()))

# bytes
check_bytes(b'hello')
check_bytes(bytes('hello', 'utf-8'))
check_bytes(bytes(10))
check_bytes(bytes([65, 66, 67]))
check_bytes(bytes(get_int()))
k2 = get_bytes()
assert_type(k2, bytes)


# === Container Types ===

# list
check_list_int([1, 2, 3])
check_list_str(list('abc'))
check_list_int(list(range(10)))
m2 = get_list_int()
assert_type(m2, list[int])
m3 = get_list_str()
assert_type(m3, list[str])

# tuple
check_tuple_int(tuple([1, 2, 3]))
p2 = get_tuple_str_int()
assert_type(p2, tuple[str, int])

# dict
check_dict_str_int({'a': 1, 'b': 2})
check_dict_str_int(dict(a=1, b=2))
d = get_dict_str_int()
assert_type(d, dict[str, int])

# set
check_set_int({1, 2, 3})
check_set_int(set([1, 2, 3]))

# frozenset
check_frozenset_int(frozenset([1, 2, 3]))

# range
w = range(get_int())
assert_type(w, range)

# slice
sl1 = slice(10)
sl2 = slice(0, 10)
sl3 = slice(0, 10, 2)


# === Builtin Functions ===

# abs
check_int(abs(-5))
check_float(abs(-3.14))

# all / any
check_bool(all([True, False]))
check_bool(any([True, False]))
aa = all(get_list_int())
assert_type(aa, bool)

# bin / hex / oct
check_str(bin(42))
check_str(hex(255))
check_str(oct(8))
ac = bin(get_int())
assert_type(ac, str)

# chr / ord
check_str(chr(65))
check_int(ord('A'))
af = chr(get_int())
assert_type(af, str)
ag = ord(get_str())
assert_type(ag, int)

# divmod
dm = divmod(10, 3)

# hash
check_int(hash('hello'))
ai = hash(get_str())
assert_type(ai, int)

# id
check_int(id(object()))
ak = id(get_object())
assert_type(ak, int)

# isinstance
check_bool(isinstance(42, int))
al = isinstance(get_object(), int)
assert_type(al, bool)

# len
check_int(len([1, 2, 3]))
an = len(get_list_int())
assert_type(an, int)

# max / min
check_int(max(1, 2, 3))
check_int(min(1, 2, 3))

# pow
check_int(pow(2, 3))
check_float(pow(2.0, 3.0))

# print
aw = print(get_str())
assert_type(aw, None)

# repr
check_str(repr(42))
ax = repr(get_int())
assert_type(ax, str)

# round
check_int(round(3.7))

# sorted
check_list_int(sorted([3, 1, 2]))

# sum
check_int(sum([1, 2, 3]))
ba = sum(get_list_int())
assert_type(ba, int)

# type
bf = type(get_int())
assert_type(bf, type[int])


# === Iterator Types ===

# enumerate
for i_enum, v_enum in enumerate([1, 2, 3]):
    check_int(i_enum)
    check_int(v_enum)

# reversed
for v_rev in reversed([1, 2, 3]):
    check_int(v_rev)

# zip
for a_zip, b_zip in zip([1, 2], ['a', 'b']):
    check_int(a_zip)
    check_str(b_zip)


# === Literal Types ===

bk = None
assert_type(bk, None)


# === Exception Types ===

e1 = BaseException('error')
e2 = Exception('error')
e3 = SystemExit(1)
e4 = KeyboardInterrupt()
e5 = ArithmeticError('error')
e6 = OverflowError('error')
e7 = ZeroDivisionError('error')
e8 = LookupError('error')
e9 = IndexError('error')
e10 = KeyError('key')
e11 = RuntimeError('error')
e12 = NotImplementedError('error')
e13 = RecursionError('error')
e14 = AttributeError('error')
e15 = AssertionError('error')
e16 = MemoryError('error')
e17 = NameError('error')
e18 = SyntaxError('error')
e19 = TimeoutError('error')
e20 = TypeError('error')
e21 = ValueError('error')
e22 = StopIteration()


# === Exception Inheritance ===


def handle_base(e: BaseException) -> None:
    pass


handle_base(Exception('error'))
handle_base(ValueError('error'))
handle_base(KeyError('key'))
handle_base(ZeroDivisionError('error'))


def handle_exception(e: Exception) -> None:
    pass


handle_exception(ValueError('error'))
handle_exception(TypeError('error'))
handle_exception(RuntimeError('error'))


# === Try/Except ===

try:
    x_try = 1 / 0
except ZeroDivisionError as e_try:
    check_str(str(e_try))

try:
    d_try: dict[str, int] = {}
    v_try = d_try['missing']
except KeyError:
    pass

try:
    lst_try: list[int] = []
    v_lst = lst_try[0]
except IndexError:
    pass


# === Raise ===


def may_fail(x_fail: int) -> int:
    if x_fail < 0:
        raise ValueError('x must be non-negative')
    if x_fail == 0:
        raise ZeroDivisionError('x cannot be zero')
    return 100 // x_fail


def not_implemented() -> None:
    raise NotImplementedError('subclass must implement')


print(sys.version)
print(sys.version_info)
print(None, file=sys.stdout)
print(None, file=sys.stderr)

# === async ===


async def foo(a: int):
    return a * 2


async def bar():
    await foo(1)
    await foo(2)
    await foo(3)


await asyncio.gather(bar())  # pyright: ignore


@dataclass
class Point:
    x: int
    y: float


p = Point(1, 2)
assert_type(p.x, int)
assert_type(p.y, float)
p.x = 3
print(p)

path = Path(__file__)
assert_type(path, Path)
# assert_type(path.name, str)

p2 = path.parent
assert_type(p2, Path)

p3 = path / 'test.txt'
assert_type(p3, Path)
assert p3.name == 'test.txt'

x = os.getenv('foobar')
assert_type(x, str | None)

y = os.getenv('foobar', default=int('123'))
assert_type(y, str | int)

x2 = os.environ.get('foobar')
assert_type(x2, str | None)
