# This file contains intentional type errors to test the type checker.
# Each section demonstrates a different category of type error.
# ===

import sys
from typing import assert_type


def takes_int(x: int) -> None:
    pass


def takes_str(x: str) -> None:
    pass


def takes_list_int(x: list[int]) -> None:
    pass


# Wrong primitive types
takes_int('hello')
takes_int(3.14)
takes_str(42)
takes_str([1, 2, 3])

# Wrong container element types
takes_list_int(['a', 'b', 'c'])
takes_list_int([1.0, 2.0, 3.0])


# === Invalid return types ===


def should_return_int() -> int:
    return 'oops'


def should_return_str() -> str:
    return 123


def should_return_list_int() -> list[int]:
    return ['a', 'b']


def should_return_none() -> None:
    return 42


# === Type mismatches in expressions ===


def get_int() -> int:
    return 42


def get_str() -> str:
    return 'hello'


# Binary operations with incompatible types
result1 = get_int() + get_str()
result2 = get_str() - get_int()


# === assert_type failures ===

x: int = 42
assert_type(x, str)

y: list[int] = [1, 2, 3]
assert_type(y, list[str])


# === Attribute errors ===


class MyClass:
    def __init__(self) -> None:
        self.value: int = 42


obj = MyClass()
z = obj.nonexistent_attr


# === Too many / too few arguments ===


def takes_two(a: int, b: str) -> None:
    pass


takes_two(1)
takes_two(1, 'hello', 'extra')


# === Wrong keyword arguments ===

takes_two(a=1, c='wrong')


# === Calling non-callable ===

not_callable: int = 42
not_callable()

print(sys.copyright)
