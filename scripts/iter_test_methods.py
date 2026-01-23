"""
External function implementations for iter mode tests.

These implementations mirror the behavior of `dispatch_external_call` in the Rust test runner
so that iter mode tests produce identical results in both Monty and CPython.

This module is shared between:
- scripts/run_traceback.py (for traceback tests)
- crates/monty/tests/datatest_runner.rs (via include_str! for CPython execution)
"""

from dataclasses import dataclass


def add_ints(a: int, b: int) -> int:
    return a + b


def concat_strings(a: str, b: str) -> str:
    return a + b


def return_value(x: object) -> object:
    return x


def get_list() -> list[int]:
    return [1, 2, 3]


def raise_error(exc_type: str, message: str) -> None:
    exc_types: dict[str, type[Exception]] = {
        'ValueError': ValueError,
        'TypeError': TypeError,
        'KeyError': KeyError,
        'RuntimeError': RuntimeError,
    }
    raise exc_types[exc_type](message)


@dataclass(frozen=True)
class Point:
    x: int
    y: int


def make_point() -> Point:
    return Point(x=1, y=2)


@dataclass
class MutablePoint:
    x: int
    y: int


def make_mutable_point() -> MutablePoint:
    return MutablePoint(x=1, y=2)


@dataclass(frozen=True)
class User:
    name: str
    active: bool = True


def make_user(name: str) -> User:
    return User(name=name, active=True)


@dataclass
class Empty:
    pass


def make_empty() -> Empty:
    return Empty()


async def async_call(x: object) -> object:
    """Async function that returns its argument.

    This is a coroutine - it returns a future that resolves to the given value.
    Used for testing async external function calls.
    """
    return x


# All external functions available to iter mode tests
ITER_MODE_GLOBALS: dict[str, object] = {
    'add_ints': add_ints,
    'concat_strings': concat_strings,
    'return_value': return_value,
    'get_list': get_list,
    'raise_error': raise_error,
    'make_point': make_point,
    'make_mutable_point': make_mutable_point,
    'make_user': make_user,
    'make_empty': make_empty,
    'async_call': async_call,
}
