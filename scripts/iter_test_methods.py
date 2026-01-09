"""
External function implementations for iter mode tests.

These implementations mirror the behavior of `dispatch_external_call` in the Rust test runner
so that iter mode tests produce identical results in both Monty and CPython.

This module is shared between:
- scripts/run_traceback.py (for traceback tests)
- crates/monty/tests/datatest_runner.rs (via include_str! for CPython execution)
"""

from typing import Any, NoReturn


def add_ints(a: int, b: int) -> int:
    return a + b


def concat_strings(a: str, b: str) -> str:
    return a + b


def return_value(x: Any) -> Any:
    return x


def get_list() -> list[int]:
    return [1, 2, 3]


def raise_error(exc_type: str, message: str) -> NoReturn:
    exc_types: dict[str, type[Exception]] = {
        'ValueError': ValueError,
        'TypeError': TypeError,
        'KeyError': KeyError,
        'RuntimeError': RuntimeError,
    }
    raise exc_types[exc_type](message)


def make_point() -> Any:
    from dataclasses import dataclass

    @dataclass(frozen=True)
    class Point:
        x: int
        y: int

    return Point(x=1, y=2)


def make_mutable_point() -> Any:
    from dataclasses import dataclass

    @dataclass
    class Point:
        x: int
        y: int

    return Point(x=1, y=2)


def make_user(name: str) -> Any:
    from dataclasses import dataclass

    @dataclass(frozen=True)
    class User:
        name: str
        active: bool = True

    return User(name=name, active=True)


# All external functions available to iter mode tests
ITER_MODE_GLOBALS: dict[str, Any] = {
    'add_ints': add_ints,
    'concat_strings': concat_strings,
    'return_value': return_value,
    'get_list': get_list,
    'raise_error': raise_error,
    'make_point': make_point,
    'make_mutable_point': make_mutable_point,
    'make_user': make_user,
}
