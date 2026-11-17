# /// monty
# skip-cpython-windows = true
# ///
# pydantic-core on Windows stops serializing at a shallower depth than 255,
# so CPython only confirms these on Linux and macOS.
from typing import Any

from pydantic import TypeAdapter

Any_ = TypeAdapter(Any)


def error(f):
    try:
        f()
    except ValueError as exc:
        return str(exc)
    assert False, 'expected an error'


def nested(n, leaf=None):
    x = [] if leaf is None else [leaf]
    for _ in range(n):
        x = [x]
    return x


def depth_of(x):
    n = 0
    while x:
        x = x[0]
        n += 1
    return n


# === Nesting depth: 255 levels of inferred values ===
# Python mode hands back what inference cannot descend into as it is: the same object
deep = nested(300)
dumped = Any_.dump_python(deep)
assert depth_of(dumped) == 300
inner, original = dumped, deep
for _ in range(255):
    assert inner is not original
    inner, original = inner[0], original[0]
assert inner is original
assert depth_of(TypeAdapter(list[Any]).dump_python(nested(10_000))) == 10_000
assert error(lambda: Any_.dump_python(nested(255), mode='json')) == 'Circular reference detected (depth exceeded)'
assert depth_of(Any_.dump_python(nested(254), mode='json')) == 254
assert error(lambda: Any_.dump_json(nested(300))) == (
    'Error serializing to JSON: ValueError: Circular reference detected (depth exceeded)'
)
# a scalar counts as a level too
assert error(lambda: Any_.dump_json(nested(254, 1))) == (
    'Error serializing to JSON: ValueError: Circular reference detected (depth exceeded)'
)
assert Any_.dump_json(nested(253, 1)).count(b'[') == 254
