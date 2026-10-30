# /// monty
# call-external = true
# ///
# Host hints come from the harness's `typing.get_type_hints` answer, which mirrors
# the annotations in `scripts/test_fixtures.py`; CPython reads those directly.
import functools
import tools
from typing import get_type_hints

# === Host functions ===
assert get_type_hints(add_ints) == {'a': int, 'b': int, 'return': int}
assert get_type_hints(concat_strings) == {'a': str, 'b': str, 'return': str}
assert get_type_hints(raise_error) == {'exc_type': str, 'message': str, 'return': type(None)}

hints = get_type_hints(add_ints)
assert list(hints) == ['a', 'b', 'return']
assert hints['a'] is int
assert hints['return'] is int
assert get_type_hints(raise_error)['return'] is type(None)

# === Host classes ===
assert get_type_hints(type(make_point())) == {'x': int, 'y': int}
assert get_type_hints(type(make_mutable_point())) == {'x': int, 'y': int}
assert get_type_hints(type(make_user('Samuel'))) == {'name': str, 'active': bool}
assert get_type_hints(type(make_empty())) == {}
assert list(get_type_hints(type(make_user('Samuel')))) == ['name', 'active']

# === Host module attributes ===
assert get_type_hints(tools.add_ints) == {'a': int, 'b': int, 'return': int}
assert get_type_hints(tools.concat_strings) == {'a': str, 'b': str, 'return': str}
assert get_type_hints(tools.nested.return_value) == get_type_hints(return_value)
assert get_type_hints(tools) == {}

# === Each call asks again and returns a fresh dict ===
first = get_type_hints(add_ints)
first['extra'] = bytes
assert first.pop('a') is int
assert get_type_hints(add_ints) == {'a': int, 'b': int, 'return': int}
assert get_type_hints(add_ints) is not get_type_hints(add_ints)

for _ in range(50):
    hints = get_type_hints(concat_strings)
assert hints == {'a': str, 'b': str, 'return': str}

# === Arguments other than the object ===
# the host never sees the namespaces (see limitations/host-values.md), so none here shadow a hint
assert get_type_hints(add_ints, {}) == {'a': int, 'b': int, 'return': int}
assert get_type_hints(add_ints, None, {'unused': str}) == {'a': int, 'b': int, 'return': int}
assert get_type_hints(add_ints, include_extras=True) == get_type_hints(add_ints)
assert get_type_hints(type(make_point()), {}, {}) == {'x': int, 'y': int}

# === Wrapped host functions ===
cached = functools.lru_cache(add_ints)
assert get_type_hints(cached) == {'a': int, 'b': int, 'return': int}
assert get_type_hints(functools.partial(add_ints, 1)) == {}


@functools.lru_cache(maxsize=4)
def wraps_host(a: int, b: str) -> int:
    return add_ints(a, len(b))


assert get_type_hints(wraps_host) == {'a': int, 'b': str, 'return': int}
assert wraps_host(1, 'abc') == 4

# === Called from nested scopes ===


def from_function(func):
    return get_type_hints(func)


assert from_function(concat_strings) == {'a': str, 'b': str, 'return': str}


def outer():
    def inner():
        return get_type_hints(type(make_point()))

    return inner()


assert outer() == {'x': int, 'y': int}

all_hints = [get_type_hints(f) for f in [add_ints, concat_strings, raise_error]]
assert all_hints == [
    {'a': int, 'b': int, 'return': int},
    {'a': str, 'b': str, 'return': str},
    {'exc_type': str, 'message': str, 'return': type(None)},
]
by_name = {name: get_type_hints(f)['return'] for name, f in [('add', add_ints), ('concat', concat_strings)]}
assert by_name == {'add': int, 'concat': str}
assert list(get_type_hints(f) for f in [add_ints]) == [{'a': int, 'b': int, 'return': int}]
assert (lambda f: get_type_hints(f))(raise_error)['message'] is str

# === Mixed with other host calls ===
assert add_ints(1, 2) == 3
assert get_type_hints(add_ints)['a'] is int
assert concat_strings('a', 'b') == 'ab'
point = make_point()
assert get_type_hints(type(point)) == {'x': int, 'y': int}
assert point.sum() == 3
assert {name: (get_type_hints(add_ints)[name], add_ints(1, 1)) for name in ['a', 'b']} == {
    'a': (int, 2),
    'b': (int, 2),
}

# === Within exception handling ===
try:
    raise_error('ValueError', 'boom')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'boom'
    assert get_type_hints(raise_error)['exc_type'] is str
finally:
    assert get_type_hints(add_ints)['b'] is int

try:
    get_type_hints(add_ints)['missing']
    assert False, 'expected KeyError'
except KeyError as exc:
    assert str(exc) == "'missing'"
